//! 系统信息面板：按连接的服务器显示系统 / CPU / 内存 / 网络 / 磁盘五个分区
//!（每个分区套 ui-kit `Panel`：标题栏 icon+标题+右侧 % 徽标 + body；无底色，
//! 边框=分割线基线色，四周 4px 留白；系统区四行左右布局——主机名/系统+内核/
//! 时区/运行时间；CPU 处理器行+折线图+每核占用条；内存环形图+四行；网络
//! 双序列折线图+上下行速度与总计；磁盘占用条+总计/已用）。
//!
//! 取数走与目录面板同一套 exec 通道：一条 [`PROBE_CMD`]（分节标记 `@@name`）
//! 一次性带回全部数据；面板可见（右面板开且当前页为系统信息）时 1s 状态
//! ticker 触发 [`AppUi::maybe_probe_sysinfo`]，探针在飞则跳过。
//!
//! 速率类（CPU%、上下行速率）由**相邻两次采样差分**得出（/proc 计数器单调递增，
//! 客户端时间戳计 Δt）；历史进环形队列（90 点 ≈ 90s）供折线图绘制。
//!
//! 非 Linux / 受限容器（无 /proc、BusyBox 无 df -B1 等）对应分节为空 → 该行
//! 显示「—」，不阻塞其余分节。

use std::collections::VecDeque;
use std::f64::consts::PI;
use std::time::Instant;

use gpui::{point, prelude::*, px, rgb, AnyElement, Bounds, Context, Hsla, Pixels, div};

use crate::app_ui::RightPage;
use crate::icons;
use crate::theme::{self, PalFx};
use ui_kit::base::panel::{Panel, PanelSize};
use ui_kit::pro::scroll_area::ScrollArea;

// ---------- 探针命令 ----------

/// 一次 exec 取全部系统信息；`@@name` 分节标记，POSIX sh 可执行（无 bashism）。
/// 每节 `|| true` 兜底：单节失败不拖垮整条命令（退出码非 0 会走 Err 路径）。
pub(crate) const PROBE_CMD: &str = r#"
echo @@host; hostname 2>/dev/null || true
echo @@ip; hostname -I 2>/dev/null || ip -4 route get 1 2>/dev/null || true
echo @@os; if [ -r /etc/os-release ]; then . /etc/os-release; echo "$PRETTY_NAME"; else uname -s; fi
echo @@kernel; uname -r 2>/dev/null || true
echo @@tz; readlink /etc/localtime 2>/dev/null || echo unknown
echo @@tzoff; date +%z 2>/dev/null || true
echo @@uptime; cat /proc/uptime 2>/dev/null || true
echo @@cpumodel; grep -m1 -e '^model name' -e '^Model' /proc/cpuinfo 2>/dev/null | cut -d: -f2- || true
echo @@stat; grep '^cpu' /proc/stat 2>/dev/null || true
echo @@mem; cat /proc/meminfo 2>/dev/null || true
echo @@net; cat /proc/net/dev 2>/dev/null || true
echo @@df; df -B1 -P / 2>/dev/null | tail -1 || true
echo @@end
"#;

// ---------- 解析 ----------

/// 一次探针的原样数据（分节解析产物；缺失分节为 None / 空集合）。
#[derive(Default, Debug, Clone, PartialEq)]
pub(crate) struct ProbeData {
    pub hostname: Option<String>,
    pub ip: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub tz: Option<String>,
    /// `date +%z` 形如 `+0800`。
    pub tz_off: Option<String>,
    pub uptime_s: Option<f64>,
    pub cpu_model: Option<String>,
    pub cpu: Option<CpuTicks>,
    pub mem: Option<MemInfo>,
    pub net: Option<NetTotals>,
    pub disk: Option<DiskInfo>,
}

/// /proc/stat 计数器（idle 含 iowait；total 为 user..steal 前 8 字段之和）。
/// cores 按 cpuN 行序（N 递增）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CpuTicks {
    pub idle: u64,
    pub total: u64,
    pub cores: Vec<(u64, u64)>,
}

/// /proc/meminfo 摘要（字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MemInfo {
    pub total: u64,
    pub free: u64,
    pub avail: u64,
    pub buffers: u64,
    pub cached: u64,
    pub sreclaim: u64,
}

/// /proc/net/dev 汇总（排除 lo；字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct NetTotals {
    pub rx: u64,
    pub tx: u64,
}

/// df -B1 -P / 摘要（字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DiskInfo {
    pub total: u64,
    pub used: u64,
}

/// 解析探针输出：`@@name` 起节、行归节；未知节忽略，节内首行优先。
pub(crate) fn parse_probe(out: &str) -> ProbeData {
    let mut d = ProbeData::default();
    let mut cur = "";
    let mut stat_lines: Vec<&str> = Vec::new();
    let mut ip_lines: Vec<&str> = Vec::new();
    for line in out.lines() {
        if let Some(name) = line.strip_prefix("@@") {
            cur = name.trim();
            continue;
        }
        match cur {
            "host" => {
                if d.hostname.is_none() {
                    d.hostname = nonempty(line);
                }
            }
            "ip" => ip_lines.push(line),
            "os" => {
                if d.os.is_none() {
                    d.os = nonempty(line);
                }
            }
            "kernel" => {
                if d.kernel.is_none() {
                    d.kernel = nonempty(line);
                }
            }
            "tz" => {
                if d.tz.is_none() {
                    d.tz = clean_tz(line);
                }
            }
            "tzoff" => {
                if d.tz_off.is_none() {
                    d.tz_off = valid_tzoff(line);
                }
            }
            "uptime" => {
                if d.uptime_s.is_none() {
                    d.uptime_s = line.split_whitespace().next().and_then(|s| s.parse().ok());
                }
            }
            "cpumodel" => {
                if d.cpu_model.is_none() {
                    d.cpu_model = nonempty(line.trim());
                }
            }
            "stat" => stat_lines.push(line),
            "mem" => parse_meminfo_line(line, &mut d),
            "net" => parse_netdev_line(line, &mut d),
            "df" => {
                if d.disk.is_none() {
                    d.disk = parse_df_line(line);
                }
            }
            _ => {}
        }
    }
    // IP：优先 ip-route 输出的 `src <addr>`；否则首行首 token。
    d.ip = ip_lines
        .iter()
        .find_map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            toks.iter()
                .position(|t| *t == "src")
                .and_then(|i| toks.get(i + 1))
                .and_then(|t| nonempty(t))
        })
        .or_else(|| {
            ip_lines
                .iter()
                .flat_map(|l| l.split_whitespace())
                .find(|t| t.contains('.') || t.contains(':'))
                .and_then(|t| nonempty(t))
        });
    // /proc/stat：`cpu ` 聚合行 + `cpuN` 每核行（按出现序即 N 序）。
    if !stat_lines.is_empty() {
        let mut agg = None;
        let mut cores = Vec::new();
        for line in &stat_lines {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.is_empty() {
                continue;
            }
            if cols[0] == "cpu" {
                agg = parse_stat_line(&cols);
            } else if let Some(n) = cols[0].strip_prefix("cpu").and_then(|n| n.parse::<usize>().ok()) {
                while cores.len() <= n {
                    cores.push((0, 0));
                }
                if let Some(t) = parse_stat_line(&cols) {
                    cores[n] = t;
                }
            }
        }
        if let Some((idle, total)) = agg {
            d.cpu = Some(CpuTicks { idle, total, cores });
        }
    }
    d
}

fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// zoneinfo 路径 → 区域名（`/usr/share/zoneinfo/Asia/Shanghai` → `Asia/Shanghai`）；
/// 兜底相对链接（`../usr/share/zoneinfo/…`）同样按 `zoneinfo/` 截取；其余不认。
fn clean_tz(line: &str) -> Option<String> {
    nonempty(line.split("zoneinfo/").nth(1).unwrap_or(""))
}

/// 校验 `±HHMM` 形态。
fn valid_tzoff(line: &str) -> Option<String> {
    let s = line.trim();
    let b = s.as_bytes();
    let ok = b.len() == 5
        && (b[0] == b'+' || b[0] == b'-')
        && b[1..].iter().all(u8::is_ascii_digit);
    ok.then(|| s.to_string())
}

fn parse_stat_line(cols: &[&str]) -> Option<(u64, u64)> {
    // cols[0] = "cpu…";字段 1..=8：user nice system idle iowait irq softirq steal
    //（旧内核不足 8 个字段按现有一律计入；guest 字段与 user 重复计数，不计）。
    if cols.len() < 5 {
        return None;
    }
    let v: Vec<u64> = cols[1..].iter().map(|s| s.parse().unwrap_or(0)).collect();
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((idle, total))
}

fn parse_meminfo_line(line: &str, d: &mut ProbeData) {
    let mut it = line.split_whitespace();
    let Some(key) = it.next().map(|k| k.trim_end_matches(':')) else { return };
    let Some(kb) = it.next().and_then(|v| v.parse::<u64>().ok()) else { return };
    let m = d.mem.get_or_insert_with(MemInfo::default);
    let b = kb * 1024;
    match key {
        "MemTotal" => m.total = b,
        "MemFree" => m.free = b,
        "MemAvailable" => m.avail = b,
        "Buffers" => m.buffers = b,
        "Cached" => m.cached = b,
        "SReclaimable" => m.sreclaim = b,
        _ => {}
    }
}

fn parse_netdev_line(line: &str, d: &mut ProbeData) {
    // 表头行无 ':'，天然跳过；`  eth0: rx_bytes … tx_bytes`（tx 在第 9 个字段）。
    let Some((iface, rest)) = line.split_once(':') else { return };
    if iface.trim() == "lo" || iface.trim().is_empty() {
        return;
    }
    let t: Vec<&str> = rest.split_whitespace().collect();
    if t.len() < 9 {
        return;
    }
    let (Some(rx), Some(tx)) = (t[0].parse::<u64>().ok(), t[8].parse::<u64>().ok()) else { return };
    let n = d.net.get_or_insert_with(NetTotals::default);
    n.rx += rx;
    n.tx += tx;
}

fn parse_df_line(line: &str) -> Option<DiskInfo> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() < 3 {
        return None;
    }
    Some(DiskInfo {
        total: f[1].parse().ok()?,
        used: f[2].parse().ok()?,
    })
}

// ---------- 面板状态（差分 + 历史） ----------

/// 折线图历史容量（1s 一采 ≈ 90s 窗口）。
const HIST_CAP: usize = 90;

/// 静态信息（每次探针整体覆写；字段缺失显示「—」）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SysStatic {
    pub hostname: Option<String>,
    /// 已解析未展示（IP 行按需求移除；数据与探针管道保留，恢复展示即可用）。
    #[allow(dead_code)]
    pub ip: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub tz: Option<String>,
    /// `Asia/Shanghai (GMT+8)` 的后半；UTC（+0）不带后缀。
    pub tz_gmt: Option<String>,
    pub cpu_model: Option<String>,
    pub cores: usize,
}

/// 一次差分后的展示样本（速率缺基准时为 0，不阻塞静态部分显示）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SysLive {
    pub cpu_pct: f32,
    pub cores_pct: Vec<f32>,
    /// 字节/秒。
    pub rx_rate: f64,
    pub tx_rate: f64,
    pub rx_total: u64,
    pub tx_total: u64,
    pub mem_total: u64,
    pub mem_used: u64,
    pub mem_cache: u64,
    pub mem_avail: u64,
    pub disk_total: u64,
    pub disk_used: u64,
    pub uptime_s: f64,
}

/// 一个已连接 tab 的系统信息面板状态。
pub(crate) struct SysInfo {
    /// 最近一次探针错误（已有数据后失败保持旧值静默，不写 error）。
    pub error: Option<String>,
    pub stat: Option<SysStatic>,
    pub live: Option<SysLive>,
    prev_cpu: Option<(Instant, CpuTicks)>,
    prev_net: Option<(Instant, NetTotals)>,
    pub hist_cpu: VecDeque<f32>,
    pub hist_rx: VecDeque<f32>,
    pub hist_tx: VecDeque<f32>,
    /// 探针在飞（防 ticker 重复并发）。
    pub probing: bool,
}

impl SysInfo {
    fn new() -> Self {
        Self {
            error: None,
            stat: None,
            live: None,
            prev_cpu: None,
            prev_net: None,
            hist_cpu: VecDeque::new(),
            hist_rx: VecDeque::new(),
            hist_tx: VecDeque::new(),
            probing: false,
        }
    }

    /// 吸收一次探针结果：覆写静态信息、差分出速率、推进历史。
    pub(crate) fn ingest(&mut self, d: ProbeData, now: Instant) {
        self.error = None;
        self.stat = Some(SysStatic {
            hostname: d.hostname,
            ip: d.ip,
            os: d.os,
            kernel: d.kernel,
            tz: d.tz,
            tz_gmt: d.tz_off.as_deref().and_then(gmt_label),
            cpu_model: d.cpu_model,
            cores: d.cpu.as_ref().map(|c| c.cores.len()).unwrap_or(0),
        });

        // CPU 差分（无基准 → 0%，首次采样只铺基准）。
        let (mut cpu_pct, mut cores_pct) = (0f32, Vec::new());
        if let Some(c) = &d.cpu {
            if let Some((t0, p)) = self.prev_cpu.take() {
                if now.duration_since(t0).as_secs_f64() > 0. {
                    cpu_pct = usage_pct(p.idle, p.total, c.idle, c.total);
                    cores_pct = c
                        .cores
                        .iter()
                        .zip(p.cores.iter())
                        .map(|(a, b)| usage_pct(b.0, b.1, a.0, a.1))
                        .collect();
                }
            }
            self.prev_cpu = Some((now, c.clone()));
        }
        // 网络速率差分。
        let (mut rx_rate, mut tx_rate) = (0f64, 0f64);
        if let Some(n) = &d.net {
            if let Some((t0, p)) = self.prev_net.take() {
                let dt = now.duration_since(t0).as_secs_f64();
                if dt > 0. {
                    rx_rate = n.rx.saturating_sub(p.rx) as f64 / dt;
                    tx_rate = n.tx.saturating_sub(p.tx) as f64 / dt;
                }
            }
            self.prev_net = Some((now, *n));
        }
        // 内存口径与 free(1) 一致：cache = Buffers+Cached+SReclaimable，
        // used = Total − Free − cache，avail = MemAvailable。
        let (mt, mu, mc, ma) = d
            .mem
            .map(|m| {
                let cache = m.buffers.saturating_add(m.cached).saturating_add(m.sreclaim);
                let used = m.total.saturating_sub(m.free).saturating_sub(cache);
                (m.total, used, cache, m.avail)
            })
            .unwrap_or((0, 0, 0, 0));
        let (dt, du) = d.disk.map(|x| (x.total, x.used)).unwrap_or((0, 0));

        push_hist(&mut self.hist_cpu, cpu_pct);
        push_hist(&mut self.hist_rx, rx_rate as f32);
        push_hist(&mut self.hist_tx, tx_rate as f32);
        self.live = Some(SysLive {
            cpu_pct,
            cores_pct,
            rx_rate,
            tx_rate,
            rx_total: d.net.map(|n| n.rx).unwrap_or(0),
            tx_total: d.net.map(|n| n.tx).unwrap_or(0),
            mem_total: mt,
            mem_used: mu,
            mem_cache: mc,
            mem_avail: ma,
            disk_total: dt,
            disk_used: du,
            uptime_s: d.uptime_s.unwrap_or(0.),
        });
    }
}

fn push_hist(h: &mut VecDeque<f32>, v: f32) {
    h.push_back(v);
    while h.len() > HIST_CAP {
        h.pop_front();
    }
}

fn usage_pct(idle0: u64, total0: u64, idle1: u64, total1: u64) -> f32 {
    let di = idle1.saturating_sub(idle0) as f64;
    let dt = total1.saturating_sub(total0) as f64;
    if dt <= 0. {
        0.
    } else {
        ((1. - di / dt) * 100.).clamp(0., 100.) as f32
    }
}

/// `+0800` → `GMT+8`（半小时时区 `GMT+5.5`；+0 不带后缀返回 None）。
pub(crate) fn gmt_label(off: &str) -> Option<String> {
    let b = off.as_bytes();
    if b.len() != 5 || (b[0] != b'+' && b[0] != b'-') {
        return None;
    }
    let (Ok(hh), Ok(mm)) = (off[1..3].parse::<f32>(), off[3..5].parse::<f32>()) else {
        return None;
    };
    let h = hh + mm / 60.;
    if h == 0. {
        return None;
    }
    let sign = if b[0] == b'+' { "+" } else { "-" };
    let hs = if h.fract() == 0. { format!("{}", h as i32) } else { format!("{h:.1}") };
    Some(format!("GMT{sign}{hs}"))
}

/// 运行时长（天/时/时/分双档显示，语言跟随界面）。
pub(crate) fn fmt_uptime(loc: crate::i18n::Locale, secs: f64) -> String {
    let s = secs.max(0.) as u64;
    let (d, h, m, ss) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60, s % 60);
    match loc {
        crate::i18n::Locale::Zh => {
            if d > 0 {
                format!("{d} 天 {h} 小时")
            } else if h > 0 {
                format!("{h} 小时 {m} 分")
            } else {
                format!("{m} 分 {ss} 秒")
            }
        }
        crate::i18n::Locale::En => {
            if d > 0 {
                format!("{d}d {h}h")
            } else if h > 0 {
                format!("{h}h {m}m")
            } else {
                format!("{m}m {ss}s")
            }
        }
    }
}

// ---------- 行为（AppUi 扩展） ----------

impl crate::app_ui::AppUi {
    /// ticker / 页面切换入口：面板可见且活跃 tab 在线时触发一次探针（在飞跳过）。
    pub(crate) fn maybe_probe_sysinfo(&mut self, cx: &mut Context<Self>) {
        if !self.right_open || self.right_page != RightPage::SysInfo {
            return;
        }
        let Some(tab) = self.active_live_tab() else { return };
        let id = tab.id;
        let st = self.sys_data.entry(id).or_insert_with(SysInfo::new);
        if st.probing {
            return;
        }
        st.probing = true;
        self.sysinfo_probe(id, cx);
    }

    /// 后台跑探针；结果差分入状态并刷新。
    fn sysinfo_probe(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(session) = self.live_session(tab_id) else {
            if let Some(st) = self.sys_data.get_mut(&tab_id) {
                st.probing = false;
            }
            return;
        };
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let ran = crate::RUNTIME
                .get()
                .expect("tokio runtime")
                .spawn(async move {
                    tokio::time::timeout(
                        crate::dir_panel::EXEC_TIMEOUT,
                        session.exec_output(PROBE_CMD),
                    )
                    .await
                })
                .await;
            let out: Result<String, String> = match ran {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => Err("timeout".into()),
                Err(_) => Err("background task failed".into()),
            };
            let _ = this.update(cx, |ui, cx| {
                let now = Instant::now();
                let Some(st) = ui.sys_data.get_mut(&tab_id) else { return };
                st.probing = false;
                match out {
                    Ok(text) => st.ingest(parse_probe(&text), now),
                    // 已有数据后偶发失败（超时/断连瞬间）保持旧值静默；
                    // 从未成功过才显示错误。
                    Err(e) => {
                        if st.stat.is_none() {
                            st.error = Some(e);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---------- 渲染 ----------

    /// 系统信息页右面板（右面板区域整页：标题行 + 滚动内容 + 宽度拖柄）。
    pub(crate) fn render_sys_panel(&self, win_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        div()
            .id("right-panel-sys")
            .absolute()
            .top_0()
            .right(px(crate::theme::RIGHT_TOOLBAR_W))
            .w(px(self.right_w))
            .h(px(win_h))
            .flex_col()
            .bg(pal.surface(pal.panel))
            .border_l_1()
            .border_color(rgb(pal.divider()))
            .overflow_hidden()
            .child(self.render_sys_title(cx))
            .child(self.render_sys_area(cx))
            .child(self.render_right_width_handle(cx))
            .into_any_element()
    }

    /// 标题行：「系统信息 · 服务器名」+ ↻（立即重探）。
    pub(crate) fn render_sys_title(&self, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let mut row = div()
            .id("sys-title")
            .flex()
            .items_center()
            .h(px(crate::dir_panel::DIR_TITLE_H))
            .px_3()
            .flex_none()
            .text_size(px(11.))
            .text_color(rgb(pal.dim))
            .child(div().child(crate::i18n::t(cx, crate::i18n::Tr::SysTitle)));
        if let Some(tab) = self.active_live_tab() {
            let name: String = tab.title.chars().take(18).collect();
            row = row.child(div().text_color(rgb(pal.muted)).child(format!(" · {name}")));
        }
        row.child(div().flex_1())
            // .child(
            //     div()
            //         .id("sys-refresh")
            //         .flex()
            //         .items_center()
            //         .justify_center()
            //         .size(px(20.))
            //         .rounded_md()
            //         .text_size(px(12.))
            //         .text_color(rgb(pal.dim))
            //         .hover(|s| s.bg(pal.surface(pal.card)).text_color(rgb(pal.text)))
            //         .child("↻")
            //         .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
            //             // 在飞则不叠加；否则立即探一轮（ticker 最长 1s 后也会补）。
            //             this.maybe_probe_sysinfo(cx);
            //             cx.notify();
            //         })),
            // )
            .into_any_element()
    }

    /// 内容区四态：未连接 / 首探失败（错误）/ 首探在飞（等待）/ 滚动分区内容。
    pub(crate) fn render_sys_area(&self, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let live = self
            .active_live_tab()
            .and_then(|t| self.sys_data.get(&t.id).map(|st| (t.id, st)));
        let area = div()
            .id("sys-area")
            .absolute()
            .top(px(crate::dir_panel::DIR_TITLE_H))
            .bottom_0()
            .left_0()
            .right_0();
        let hint = |el: gpui::Stateful<gpui::Div>, color: u32, text: String| {
            el.flex()
                .items_center()
                .justify_center()
                .child(div().px_3().text_size(px(11.)).text_color(rgb(color)).child(text))
                .into_any_element()
        };
        match live {
            None => hint(area, pal.dim, crate::i18n::t(cx, crate::i18n::Tr::SysEmptyHint).into()),
            Some((_, st)) if st.stat.is_none() && st.error.is_some() => {
                hint(area, pal.error, st.error.clone().unwrap_or_default())
            }
            Some((_, st)) if st.stat.is_none() => {
                hint(area, pal.dim, crate::i18n::t(cx, crate::i18n::Tr::SysNoData).into())
            }
            Some((_, st)) => area
                .child(
                    ScrollArea::new(
                        self.sys_scroll.clone(),
                        self.render_sys_sections(st, cx),
                    )
                    .theme(self.theme().kit()),
                )
                .into_any_element(),
        }
    }

    /// 分区内容（滚动容器子内容；宽度按面板宽扣滚动条与内边距）。
    /// 每分区套 ui-kit `Panel`（Sm 模板：icon+标题+%徽标 tools+body；
    /// 无底色、边框=分割线基线色、四周 4px 留白）。
    fn render_sys_sections(&self, st: &SysInfo, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let kit = pal.kit();
        let content_w = (self.right_w - 14.).max(120.);
        let inner = content_w - PanelSize::Sm.content_inset();
        let stat = st.stat.as_ref().expect("render_sys_sections 仅在有数据时调用");
        let live = st.live.as_ref();

        // —— 系统：四行左右布局（主机名 / 系统+内核 / 时区 / 运行时间；值均纯文本）——
        let tz = match (&stat.tz, &stat.tz_gmt) {
            (Some(z), Some(g)) => format!("{z} ({g})"),
            (Some(z), None) => z.clone(),
            (None, _) => "—".into(),
        };
        // 「内核」行 = 操作系统 + 内核合成（缺一个剩另一个；都缺显示「—」）。
        let os_kernel = match (&stat.os, &stat.kernel) {
            (Some(o), Some(k)) => format!("{o} · {k}"),
            (Some(o), None) => o.clone(),
            (None, Some(k)) => k.clone(),
            (None, None) => "—".into(),
        };
        let sys_body = div()
            .flex()
            .flex_col()
            .gap_1_5()
            .child(sys_row(
                cx,
                crate::i18n::Tr::SysHostname,
                stat.hostname.clone().unwrap_or_else(|| "—".into()),
                &pal,
            ))
            .child(sys_row(cx, crate::i18n::Tr::SysKernel, os_kernel, &pal))
            .child(sys_row(cx, crate::i18n::Tr::SysTimezone, tz, &pal))
            .child(sys_row(
                cx,
                crate::i18n::Tr::SysUptime,
                fmt_uptime(self.loc, live.map(|l| l.uptime_s).unwrap_or(0.)),
                &pal,
            ));

        // —— CPU ——
        let cpu_pct = live.map(|l| l.cpu_pct).unwrap_or(0.);
        let cpu_body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(sys_row(
                cx,
                crate::i18n::Tr::SysCpuModel,
                stat.cpu_model.clone().unwrap_or_else(|| "—".into()),
                &pal,
            ))
            .child(chart_card(
                cpu_chart(canvas_w(inner), 52., st.hist_cpu.iter().copied().collect(), &pal),
                &pal,
            ))
            .children((1..=stat.cores).map(|i| {
                let pct = live
                    .and_then(|l| l.cores_pct.get(i - 1).copied())
                    .unwrap_or(0.);
                core_row(i, pct, inner, &pal)
            }));

        // —— 内存 ——
        let (mt, mu, mc, ma) = live
            .map(|l| (l.mem_total, l.mem_used, l.mem_cache, l.mem_avail))
            .unwrap_or((0, 0, 0, 0));
        let used_frac = if mt > 0 { mu as f32 / mt as f32 } else { 0. };
        let cache_frac = if mt > 0 { mc as f32 / mt as f32 } else { 0. };
        let mem_body = div()
            .flex()
            .items_center()
            .gap_3()
            .child(chart_card(
                donut(64., used_frac, cache_frac, &pal),
                &pal,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(sys_row(cx, crate::i18n::Tr::SysMemTotal, theme::fmt_bytes(mt), &pal))
                    .child(sys_row(cx, crate::i18n::Tr::SysMemUsed, theme::fmt_bytes(mu), &pal))
                    .child(sys_row(cx, crate::i18n::Tr::SysMemCache, theme::fmt_bytes(mc), &pal))
                    .child(sys_row(cx, crate::i18n::Tr::SysMemAvail, theme::fmt_bytes(ma), &pal)),
            );

        // —— 网络 ——
        let (rx_rate, tx_rate, rx_total, tx_total) = live
            .map(|l| (l.rx_rate, l.tx_rate, l.rx_total, l.tx_total))
            .unwrap_or((0., 0., 0, 0));
        let fmt_rate = |v: f64| {
            crate::i18n::tf(cx, crate::i18n::Tr::RateSuffix, &[&theme::fmt_bytes(v.max(0.) as u64)])
        };
        let net_body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(chart_card(
                net_chart(
                    canvas_w(inner),
                    52.,
                    st.hist_rx.iter().copied().collect(),
                    st.hist_tx.iter().copied().collect(),
                    &pal,
                ),
                &pal,
            ))
            .child(net_row(
                cx,
                crate::i18n::Tr::SysNetDown,
                pal.accent,
                format!("{} · {}", fmt_rate(rx_rate), theme::fmt_bytes(rx_total)),
                &pal,
            ))
            .child(net_row(
                cx,
                crate::i18n::Tr::SysNetUp,
                pal.ok,
                format!("{} · {}", fmt_rate(tx_rate), theme::fmt_bytes(tx_total)),
                &pal,
            ));

        // —— 磁盘 ——
        let (dt, du) = live.map(|l| (l.disk_total, l.disk_used)).unwrap_or((0, 0));
        let disk_pct = if dt > 0 { du as f32 / dt as f32 } else { 0. };
        let disk_body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(usage_bar(inner, disk_pct, &pal))
            .child(sys_row(cx, crate::i18n::Tr::SysDiskTotal, theme::fmt_bytes(dt), &pal))
            .child(sys_row(cx, crate::i18n::Tr::SysDiskUsed, theme::fmt_bytes(du), &pal));

        div()
            .id("sys-sections")
            .w(px(content_w))
            // 面板四周 4px 留白：左/上/下 4px；右侧不加——content_w 已扣 14px
            // 滚动条区（thumb 左缘在 −10），卡片右缘距 thumb 恰 4px，与左对称。
            // 相邻面板间距 8px = 各自 4px 外边距之和。
            .py_1()
            .pl_1()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                // sys_body
                Panel::new()
                    .size(PanelSize::Sm)
                    .icon(icons::asset_path(icons::MONITOR))
                    .title(crate::i18n::t(cx, crate::i18n::Tr::SysSecSystem))
                    .body(sys_body)
                    .theme(kit),
            )
            .child(
                Panel::new()
                    .size(PanelSize::Sm)
                    .icon(icons::asset_path(icons::CPU))
                    .title(crate::i18n::t(cx, crate::i18n::Tr::SysSecCpu))
                    .tools(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(pal.accent))
                            .child(format!("{:.0}%", cpu_pct)),
                    )
                    .body(cpu_body)
                    .theme(kit),
            )
            .child(
                Panel::new()
                    .size(PanelSize::Sm)
                    .icon(icons::asset_path(icons::LAYERS))
                    .title(crate::i18n::t(cx, crate::i18n::Tr::SysSecMem))
                    .body(mem_body)
                    .theme(kit),
            )
            .child(
                Panel::new()
                    .size(PanelSize::Sm)
                    .icon(icons::asset_path(icons::NETWORK))
                    .title(crate::i18n::t(cx, crate::i18n::Tr::SysSecNet))
                    .body(net_body)
                    .theme(kit),
            )
            .child(
                Panel::new()
                    .size(PanelSize::Sm)
                    .icon(icons::asset_path(icons::HARD_DRIVE))
                    .title(crate::i18n::t(cx, crate::i18n::Tr::SysSecDisk))
                    .tools(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(pal.accent))
                            .child(format!("{:.0}%", disk_pct * 100.)),
                    )
                    .body(disk_body)
                    .theme(kit),
            )
            .into_any_element()
    }
}

// ---------- 渲染基元（无状态小件） ----------

/// 图表可用宽：扣 chart_card 的内边距 2×2 与边框 1×2（卡片外缘恰为 inner）。
fn canvas_w(inner: f32) -> f32 {
    (inner - 6.).max(60.)
}

fn sys_row(cx: &gpui::App, label: crate::i18n::Tr, value: String, pal: &PalFx) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(rgb(pal.dim))
                .child(crate::i18n::t(cx, label)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(10.5))
                .text_color(rgb(pal.muted))
                .flex()
                .justify_end()
                .child(value),
        )
}

fn net_row(
    cx: &gpui::App,
    label: crate::i18n::Tr,
    color: u32,
    value: String,
    pal: &PalFx,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().size(px(7.)).flex_none().rounded_full().bg(rgb(color)))
        .child(
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(rgb(pal.dim))
                .child(crate::i18n::t(cx, label)),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(10.5))
                .text_color(rgb(pal.muted))
                .child(value),
        )
}

/// 每核占用行：`CPU1  ▮▮▮▯▯  23%`。
/// bar 宽 = inner −（标签 36 + 百分比 32 + 两个 gap_2 共 16）——行总宽恰为
/// inner，百分比右缘与其他行（sys_row 的值列）严格右对齐。
fn core_row(i: usize, pct: f32, inner: f32, pal: &PalFx) -> AnyElement {
    let bar_w = (inner - 84.).max(40.);
    div()
        .id(("sys-core", i))
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(36.))
                .flex_none()
                .text_size(px(10.))
                .text_color(rgb(pal.dim))
                .child(format!("CPU{i}")),
        )
        .child(usage_bar(bar_w, pct / 100., pal))
        .child(
            div()
                .w(px(32.))
                .flex_none()
                .text_size(px(10.))
                .text_color(rgb(pal.muted))
                .flex()
                .justify_end()
                .child(format!("{:.0}%", pct)),
        )
        .into_any_element()
}

/// 占用条：浅轨道 + 强调色填充（frac 夹 0..1）。
fn usage_bar(w: f32, frac: f32, pal: &PalFx) -> gpui::Div {
    let frac = frac.clamp(0., 1.);
    div()
        .w(px(w))
        .h(px(5.))
        .flex_none()
        .rounded_full()
        .bg(rgb(pal.border))
        .overflow_hidden()
        .child(div().h_full().w(px(frac * w)).rounded_full().bg(rgb(pal.accent)))
}

/// 图表卡片：细边框容器包住 canvas（边框色随主题）。
fn chart_card(canvas: impl IntoElement, pal: &PalFx) -> gpui::Div {
    div()
        .px(px(2.))
        .py(px(2.))
        .rounded_md()
        .border_1()
        .border_color(rgb(pal.border))
        .child(canvas)
}

// —— canvas 图表 ——

/// CPU 单序列折线图（0..100 归一）。
fn cpu_chart(w: f32, h: f32, hist: Vec<f32>, pal: &PalFx) -> impl IntoElement {
    line_chart(w, h, vec![(hist, 100., pal.accent)])
}

/// 网络双序列折线图（rx=下行 accent、tx=上行 ok；共用最大值归一）。
fn net_chart(
    w: f32,
    h: f32,
    rx: Vec<f32>,
    tx: Vec<f32>,
    pal: &PalFx,
) -> impl IntoElement {
    let max = rx
        .iter()
        .chain(tx.iter())
        .fold(0f32, |m, v| m.max(*v))
        .max(1024.);
    line_chart(w, h, vec![(rx, max, pal.accent), (tx, max, pal.ok)])
}

/// 折线图：每序列只描线（1.5px 描边路径，不铺区域填充）。
/// 经 `PathBuilder::stroke` 曲面化——直接 fill 开放折线是零宽多边形不可见。
fn line_chart(w: f32, h: f32, series: Vec<(Vec<f32>, f32, u32)>) -> impl IntoElement {
    gpui::canvas(
        move |b: Bounds<Pixels>, _, _| b,
        move |b: Bounds<Pixels>, _, window: &mut gpui::Window, _cx| {
            for (data, max, color) in &series {
                if data.len() < 2 {
                    continue;
                }
                let (l, t) = (f32::from(b.origin.x), f32::from(b.origin.y));
                let (r, bm) = (
                    f32::from(b.origin.x) + f32::from(b.size.width),
                    f32::from(b.origin.y) + f32::from(b.size.height),
                );
                let n = data.len();
                let step = (r - l) / (n as f32 - 1.);
                let inner_h = (bm - t - 2.).max(1.);
                let x = |i: usize| l + i as f32 * step;
                let y = |v: f32| bm - (v / max).clamp(0., 1.) * inner_h;
                let mut line = gpui::PathBuilder::stroke(px(1.5));
                line.move_to(point(px(x(0)), px(y(data[0]))));
                for (i, v) in data.iter().enumerate().skip(1) {
                    line.line_to(point(px(x(i)), px(y(*v))));
                }
                if let Ok(path) = line.build() {
                    window.paint_path(path, Hsla::from(rgb(*color)));
                }
            }
        },
    )
    .size(px(w))
    .h(px(h))
}

/// 内存环形图：已用(accent) + 缓存(warn) + 空闲(border) 三段**环扇形**
/// （外弧 → 内弧 → 闭合，`PathBuilder::fill` 真曲面化）。中心不铺色——
/// 无底色面板下透出宿主背景（旧实现回填不透明 panel 色圆盘，在毛玻璃
/// 背景上是一块突兀实心色，且旧式 `gpui::Path` 按起点三角扇拼接非真填充）。
fn donut(size: f32, used: f32, cache: f32, pal: &PalFx) -> impl IntoElement {
    // 闭包须 'static：颜色先拷出（PalFx 带生命周期不可捕获）。
    let (border, accent, warn) = (pal.border, pal.accent, pal.warn);
    gpui::canvas(
        move |b: Bounds<Pixels>, _, _| b,
        move |b: Bounds<Pixels>, _, window: &mut gpui::Window, _cx| {
            let (cxp, cyp) = (
                f32::from(b.origin.x) + f32::from(b.size.width) / 2.,
                f32::from(b.origin.y) + f32::from(b.size.height) / 2.,
            );
            let r_out = f32::from(b.size.width).min(f32::from(b.size.height)) / 2. - 0.5;
            let r_in = r_out * 0.62;
            let pt = |a: f64, r: f32| {
                point(px(cxp + a.cos() as f32 * r), px(cyp + a.sin() as f32 * r))
            };
            let mut a0 = -PI / 2.;
            let mut draw_wedge = |frac: f32, color: u32| {
                if frac <= 0.001 {
                    return;
                }
                let a1 = a0 + frac as f64 * 2. * PI;
                let gap = 0.02;
                let s = a0 + gap;
                let e = (a1 - gap).max(s);
                let steps = (((e - s) / 0.06).ceil() as i32).max(1);
                let at = |i: i32, r: f32| pt(s + (e - s) * i as f64 / steps as f64, r);
                let mut path = gpui::PathBuilder::fill();
                // 外弧 s→e，接内弧 e→s，闭合成环扇形（简单多边形，填充即扇环）。
                path.move_to(at(0, r_out));
                for i in 1..=steps {
                    path.line_to(at(i, r_out));
                }
                path.line_to(at(steps, r_in));
                for i in (0..steps).rev() {
                    path.line_to(at(i, r_in));
                }
                path.close();
                if let Ok(p) = path.build() {
                    window.paint_path(p, Hsla::from(rgb(color)));
                }
                a0 = a1;
            };
            for (frac, color) in [(used.clamp(0., 1.), accent), (cache.clamp(0., 1.), warn)] {
                draw_wedge(frac, color);
            }
            // 空闲段（余量）用描边色。
            draw_wedge(1. - used.clamp(0., 1.) - cache.clamp(0., 1.), border);
        },
    )
    .size(px(size))
    .flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_sample() -> String {
        concat!(
            "@@host\ni-z94f3x2y\n@@ip\n172.17.0.2 10.0.0.8\n",
            "@@os\nUbuntu 22.04.4 LTS\n@@kernel\n5.15.0-112-generic\n",
            "@@tz\n/usr/share/zoneinfo/Asia/Shanghai\n@@tzoff\n+0800\n",
            "@@uptime\n123456.78 999999.99\n",
            "@@cpumodel\n Intel(R) Xeon(R) Platinum 8269CY CPU @ 2.50GHz\n",
            "@@stat\n",
            "cpu  100 20 30 2000 50 10 5 5\n",
            "cpu0 50 10 15 1000 25 5 2 3\n",
            "cpu1 50 10 15 1000 25 5 3 2\n",
            "@@mem\n",
            "MemTotal:       16425396 kB\n",
            "MemFree:         599996 kB\n",
            "MemAvailable:   6029432 kB\n",
            "Buffers:         512000 kB\n",
            "Cached:         9216000 kB\n",
            "SReclaimable:    409600 kB\n",
            "@@net\n",
            "Inter-|   Receive                                                |  Transmit\n",
            " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
            "    lo: 123456 1000 0 0 0 0 0 0 123456 1000 0 0 0 0 0 0\n",
            "  eth0: 2000000 3000 0 0 0 0 0 0 1000000 2000 0 0 0 0 0 0\n",
            "@@df\n",
            "/dev/vda1 42157981696 12863488000 27176161280 32% /\n",
            "@@end\n",
        )
        .into()
    }

    /// 全分节解析：字段逐项、lo 排除、kB→B、/proc/stat 聚合与每核。
    #[test]
    fn parse_full_probe() {
        let d = parse_probe(&probe_sample());
        assert_eq!(d.hostname.as_deref(), Some("i-z94f3x2y"));
        assert_eq!(d.ip.as_deref(), Some("172.17.0.2"));
        assert_eq!(d.os.as_deref(), Some("Ubuntu 22.04.4 LTS"));
        assert_eq!(d.kernel.as_deref(), Some("5.15.0-112-generic"));
        assert_eq!(d.tz.as_deref(), Some("Asia/Shanghai"));
        assert_eq!(d.tz_off.as_deref(), Some("+0800"));
        assert_eq!(d.uptime_s, Some(123456.78));
        assert_eq!(
            d.cpu_model.as_deref(),
            Some("Intel(R) Xeon(R) Platinum 8269CY CPU @ 2.50GHz")
        );
        // 聚合行：user+nice+sys+irq+sirq+steal=170 忙，idle+iowait=2050 闲。
        let c = d.cpu.unwrap();
        assert_eq!(c.total, 100 + 20 + 30 + 2000 + 50 + 10 + 5 + 5);
        assert_eq!(c.idle, 2000 + 50);
        assert_eq!(c.cores.len(), 2);
        assert_eq!(c.cores[0], (1000 + 25, 50 + 10 + 15 + 1000 + 25 + 5 + 2 + 3));
        // 内存 kB→B。
        let m = d.mem.unwrap();
        assert_eq!(m.total, 16425396 * 1024);
        assert_eq!(m.avail, 6029432 * 1024);
        // 网络：eth0 计入、lo 排除。
        let n = d.net.unwrap();
        assert_eq!(n.rx, 2_000_000);
        assert_eq!(n.tx, 1_000_000);
        let dk = d.disk.unwrap();
        assert_eq!(dk.total, 42157981696);
        assert_eq!(dk.used, 12863488000);
    }

    /// ip-route 兜底输出（`… src 172.17.0.2`）优先取 src。
    #[test]
    fn parse_ip_route_fallback() {
        let out = "@@ip\n1.1.1.1 dev eth0 table main src 172.17.0.2 uid 0\n@@end\n";
        let d = parse_probe(out);
        assert_eq!(d.ip.as_deref(), Some("172.17.0.2"));
    }

    /// 部分输出（非 Linux / 受限容器）：缺失分节保持 None，不 panic。
    #[test]
    fn parse_partial_probe() {
        let d = parse_probe("@@host\nbox\n@@stat\ncpu  1 2 3 4 5 6 7 8\n@@end\n");
        assert_eq!(d.hostname.as_deref(), Some("box"));
        assert!(d.ip.is_none());
        assert!(d.mem.is_none());
        assert!(d.cpu.is_some());
        // 相对 zoneinfo 链接也能截出区域名；非法 tzoff 不认。
        let d = parse_probe("@@tz\n../usr/share/zoneinfo/Europe/Berlin\n@@tzoff\n+8o00\n@@end\n");
        assert_eq!(d.tz.as_deref(), Some("Europe/Berlin"));
        assert!(d.tz_off.is_none());
    }

    /// meminfo 混杂行 / netdev 坏行不 panic 不计入。
    #[test]
    fn parse_tolerates_noise() {
        let d = parse_probe(concat!(
            "@@mem\nSwapTotal: 0 kB\nMemTotal: 1024 kB\nbadline\n",
            "@@net\nno colon header\nshort: 1 2 3\n",
            "@@df\nFilesystem 1B-blocks Used Available Use% Mounted\n",
            "@@end\n",
        ));
        assert_eq!(d.mem.unwrap().total, 1024 * 1024);
        assert!(d.net.is_none());
        assert!(d.disk.is_none(), "df 表头行不当作数据");
    }

    /// 差分链路：两次采样 → CPU/速率正确、历史推进并封顶、口径换算。
    #[test]
    fn ingest_deltas_and_history() {
        let t0 = Instant::now();
        let mut st = SysInfo::new();
        // 首采：无基准 → 0%，只铺静态与历史首点。
        st.ingest(parse_probe(&probe_sample()), t0);
        assert_eq!(st.live.as_ref().unwrap().cpu_pct, 0.);
        assert_eq!(st.hist_cpu.len(), 1);
        // 次采（+2s）：总 tick +200（其中 idle +150）→ 忙 25%；
        // rx +400_000 B / 2s = 200_000 B/s。
        let mut d2 = parse_probe(&probe_sample());
        let c = d2.cpu.as_mut().unwrap();
        c.idle += 150;
        c.total += 200;
        let n = d2.net.as_mut().unwrap();
        n.rx += 400_000;
        n.tx += 100_000;
        st.ingest(d2, t0 + std::time::Duration::from_secs(2));
        let l = st.live.as_ref().unwrap();
        assert_eq!(l.cpu_pct, 25.);
        assert_eq!(l.rx_rate as u64, 200_000);
        assert_eq!(l.tx_rate as u64, 50_000);
        assert_eq!(st.hist_cpu.len(), 2);
        // 内存：cache=(Buffers+Cached+SReclaimable)；used=total-free-cache。
        let m = &st.stat.as_ref().unwrap();
        assert_eq!(m.cores, 2);
        let used = 16425396 * 1024 - 599996 * 1024 - (512000 + 9216000 + 409600) * 1024;
        assert_eq!(l.mem_cache, (512000 + 9216000 + 409600) * 1024);
        assert_eq!(l.mem_used, used);
        assert_eq!(l.uptime_s, 123456.78);
        // 历史封顶。
        for i in 0..HIST_CAP {
            st.ingest(parse_probe(&probe_sample()), t0 + std::time::Duration::from_secs(3 + i as u64));
        }
        assert_eq!(st.hist_cpu.len(), HIST_CAP);
    }

    /// GMT 标签：整时/半小时/UTC/非法。
    #[test]
    fn gmt_labels() {
        assert_eq!(gmt_label("+0800").as_deref(), Some("GMT+8"));
        assert_eq!(gmt_label("-0530").as_deref(), Some("GMT-5.5"));
        assert_eq!(gmt_label("+0000"), None, "UTC 不带后缀");
        assert_eq!(gmt_label("bad"), None);
    }

    /// 运行时长双语格式。
    #[test]
    fn uptime_formats() {
        use crate::i18n::Locale;
        assert_eq!(fmt_uptime(Locale::Zh, 90061.), "1 天 1 小时");
        assert_eq!(fmt_uptime(Locale::En, 90061.), "1d 1h");
        assert_eq!(fmt_uptime(Locale::Zh, 3661.), "1 小时 1 分");
        assert_eq!(fmt_uptime(Locale::En, 65.), "1m 5s");
    }
}
