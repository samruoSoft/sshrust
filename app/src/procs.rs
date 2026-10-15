//! 进程面板：远端**全量**进程列表（PID / CPU% / MEM% / RSS / 命令），
//! 表头点击排序（同列翻转方向、异列取自然方向——数值列降序、文本列升序；
//! 同值按 PID 稳定次序）。CPU% 为**双拍差分瞬时值**：一条 [`PROBE_CMD`]
//! 内 `ps` 相隔 1s 采两拍，按 `cputimes`（累计 CPU 秒）差 ÷ 远端时间戳 Δt
//! 得出（多核可超 100%）；差分不上的（探针期间新起的进程 / 远端 ps 无
//! cputimes 列 / 时间戳异常）回落 ps 自带的 %CPU（生命周期平均）。
//!
//! 面板分**上下两区**（结构同目录页树/传输区）：上=虚拟滚动列表，中间 5px
//! 分割线可拖（[`ProcSplitDrag`]，上移=详情变高，高度持久化
//! `procs.detail.h`），下=「进程详细」钉底——点击列表行选中（按 pid 保持），
//! 展示完整字段（父进程/用户/线程/运行时长/状态解读）与不截断的命令行，
//! 并用**本地规则库** [`proc_kind`] 判断进程作用（双语词条）；标题栏带
//! 「外部搜索」按钮（系统浏览器查进程名，AI 识别留作后续扩展入口）。
//!
//! 取数走与目录/系统信息页同一套 exec 通道（同一连接另开 exec channel，
//! 不占 shell、不碰控制台）；探针自带 sleep 1，故比 sysinfo 多一层节流
//!（在飞或距上次发起 <4s 跳过，见 [`PROBE_MIN_INTERVAL`]）。
//!
//! ps 输出的列布局**由表头驱动**（[`PsLayout`] 按列名找索引）：procps 全量
//! 11 列（含 PPID/NLWP/ELAPSED 详情字段），降级 7 列（BSD ps，详情字段缺
//! 显示「—」）；args（命令行）为末列按「前 N-1 列定长切分 + 剩余整体归
//! args」解析，命令行内的空格天然保真。列表可能上千行：正文接 ui-kit
//! `ScrollArea` **虚拟模式**（自管滚动偏移，只渲染可视行窗口）。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{prelude::*, px, rgb, AnyElement, Context, CursorStyle, Div, MouseButton, Window, div};

use crate::app_ui::RightPage;
use crate::theme::{self, PalFx};
use ui_kit::pro::scroll_area::ScrollArea;

// ---------- 探针命令 ----------

/// 一次 exec 取双拍进程快照；`@@name` 分节标记，POSIX sh 可执行（无 bashism）。
/// ps 走降级链：procps 全量（含详情字段 + cputimes）→ 同字段去详情（BSD ps /
/// 旧 procps）→ 整节失败静默（对应节为空，不拖垮退出码）。`date +%s.%N`
/// 供客户端差分计 Δt；不支持 %N 的系统（BSD date）输出带字面 `%N`，解析
/// 失败回退标称 1s 窗口。
pub(crate) const PROBE_CMD: &str = r#"
echo @@t1; date +%s.%N 2>/dev/null || true
echo @@ps1; ps -eo pid,ppid,user:20,pcpu,pmem,rss,nlwp,cputimes,etime,stat,args 2>/dev/null || ps -eo pid,user:24,pcpu,pmem,rss,stat,args 2>/dev/null || true
sleep 1
echo @@t2; date +%s.%N 2>/dev/null || true
echo @@ps2; ps -eo pid,ppid,user:20,pcpu,pmem,rss,nlwp,cputimes,etime,stat,args 2>/dev/null || ps -eo pid,user:24,pcpu,pmem,rss,stat,args 2>/dev/null || true
echo @@end
"#;

/// 进程探针最小轮询间隔（探针自带 sleep 1，sysinfo 的 1s ticker 直连会排队）。
const PROBE_MIN_INTERVAL: Duration = Duration::from_secs(4);

// ---------- 解析（表头驱动） ----------

/// 一次探针的原样数据（分节解析产物；缺失分节为 None）。
#[derive(Default, Debug, Clone, PartialEq)]
pub(crate) struct ProbeData {
    /// 第一拍远端时间戳（`date +%s.%N`；解析失败 None）。
    t1: Option<f64>,
    t2: Option<f64>,
    ps1: Option<PsSnap>,
    ps2: Option<PsSnap>,
}

/// ps 列布局：**按表头列名找数据行的 token 索引**（procps 11 列 / 降级 7 列
/// 任意布局自适应；args 恒为末列，其后 token 全归命令行）。
#[derive(Debug, Clone, PartialEq)]
struct PsLayout {
    has_ct: bool,
    pid: usize,
    ppid: Option<usize>,
    user: Option<usize>,
    pcpu: usize,
    pmem: usize,
    rss: usize,
    nlwp: Option<usize>,
    cputime: Option<usize>,
    elapsed: Option<usize>,
    stat: usize,
    args: usize,
}

impl PsLayout {
    fn parse(header: &[&str]) -> Option<Self> {
        let col = |name: &str| header.iter().position(|h| *h == name);
        Some(Self {
            has_ct: col("CPUTIMES").is_some(),
            pid: col("PID")?,
            ppid: col("PPID"),
            user: col("USER"),
            pcpu: col("%CPU")?,
            pmem: col("%MEM")?,
            rss: col("RSS")?,
            nlwp: col("NLWP"),
            cputime: col("CPUTIMES"),
            elapsed: col("ELAPSED"),
            stat: col("STAT")?,
            args: col("COMMAND").or_else(|| col("ARGS"))?,
        })
    }
}

/// 一拍 `ps` 快照（表头定布局 + 数据行）。
#[derive(Debug, Clone, PartialEq)]
struct PsSnap {
    layout: PsLayout,
    /// COMMAND 是否末列（其后所有 token 归命令行；procps 恒真）。
    args_tail: bool,
    rows: Vec<ProcRaw>,
}

/// ps 单行原样字段（列缺失为 None）。
#[derive(Debug, Clone, PartialEq)]
struct ProcRaw {
    pid: u32,
    ppid: Option<u32>,
    user: Option<String>,
    /// ps 的 %CPU：进程生命周期平均（差分不上时的回落值）。
    pcpu: f32,
    pmem: f32,
    /// KB。
    rss_kb: u64,
    /// 线程数。
    nlwp: Option<u32>,
    /// 累计 CPU 秒（`CPUTIMES` 列，带 2 位小数）。
    cputime_s: Option<f64>,
    /// 运行时长秒（`ELAPSED` `[[dd-]hh:]mm:ss` 解析）。
    elapsed_s: Option<f64>,
    stat: String,
    /// 完整命令行（末列，内部空格保真）。
    args: String,
}

/// 解析探针输出：`@@name` 起节、行归节；未知节忽略，t 节首行优先。
pub(crate) fn parse_probe(out: &str) -> ProbeData {
    let mut d = ProbeData::default();
    let mut cur = "";
    let mut ps1: Vec<&str> = Vec::new();
    let mut ps2: Vec<&str> = Vec::new();
    for line in out.lines() {
        if let Some(name) = line.strip_prefix("@@") {
            close_ps(cur, &mut ps1, &mut ps2, &mut d);
            cur = name.trim();
            continue;
        }
        match cur {
            "t1" => {
                if d.t1.is_none() {
                    d.t1 = line.trim().parse().ok();
                }
            }
            "t2" => {
                if d.t2.is_none() {
                    d.t2 = line.trim().parse().ok();
                }
            }
            "ps1" => ps1.push(line),
            "ps2" => ps2.push(line),
            _ => {}
        }
    }
    close_ps(cur, &mut ps1, &mut ps2, &mut d);
    d
}

/// 收一个 ps 节：表头定布局后逐行解析（无表头 / 表头不认 → 整节丢弃）。
fn close_ps(cur: &str, ps1: &mut Vec<&str>, ps2: &mut Vec<&str>, d: &mut ProbeData) {
    match cur {
        "ps1" => d.ps1 = parse_ps_lines(ps1),
        "ps2" => d.ps2 = parse_ps_lines(ps2),
        _ => {}
    }
    ps1.clear();
    ps2.clear();
}

fn parse_ps_lines(lines: &[&str]) -> Option<PsSnap> {
    let mut it = lines.iter().filter(|l| !l.trim().is_empty());
    // 首个非空行必须是表头（未用 --no-headers）：PID 起头才认。
    let header: Vec<&str> = it.next()?.split_whitespace().collect();
    let layout = PsLayout::parse(&header)?;
    let args_tail = matches!(header.last(), Some(&"COMMAND") | Some(&"ARGS"));
    let rows = it
        .filter_map(|l| parse_ps_row(l, &layout, args_tail))
        .collect();
    Some(PsSnap { layout, args_tail, rows })
}

fn parse_ps_row(line: &str, lay: &PsLayout, args_tail: bool) -> Option<ProcRaw> {
    let t: Vec<&str> = line.split_whitespace().collect();
    // 命令列至少 1 token。
    if t.len() < lay.args + 1 {
        return None;
    }
    let at = |i: usize| t.get(i).copied().unwrap_or("");
    Some(ProcRaw {
        pid: at(lay.pid).parse().ok()?,
        ppid: lay.ppid.and_then(|i| at(i).parse().ok()),
        user: lay.user.map(|i| at(i).to_string()),
        pcpu: at(lay.pcpu).parse().unwrap_or(0.),
        pmem: at(lay.pmem).parse().unwrap_or(0.),
        rss_kb: at(lay.rss).parse().unwrap_or(0),
        nlwp: lay.nlwp.and_then(|i| at(i).parse().ok()),
        cputime_s: lay.cputime.and_then(|i| at(i).parse().ok()),
        elapsed_s: lay.elapsed.and_then(|i| parse_etime(at(i))),
        stat: at(lay.stat).to_string(),
        args: if args_tail {
            t[lay.args..].join(" ")
        } else {
            at(lay.args).to_string()
        },
    })
}

/// procps `ELAPSED` `[[dd-]hh:]mm:ss` → 秒。
fn parse_etime(s: &str) -> Option<f64> {
    let (days, time) = match s.split_once('-') {
        Some((d, t)) => (d.parse::<f64>().ok()?, t),
        None => (0., s),
    };
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    // 逆序加权：ss×1、mm×60、hh×3600。
    let secs = parts.iter().rev().enumerate().try_fold(0f64, |acc, (i, p)| {
        Some(acc + p.parse::<f64>().ok()? * 60f64.powi(i as i32))
    })?;
    Some(days * 86400. + secs)
}

// ---------- 面板状态（差分 + 排序 + 选中） ----------

/// 一行展示数据（差分产物；rows 已按当前排序有序）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProcRow {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub user: Option<String>,
    /// 瞬时 CPU%（双拍差分；差分不上回落 ps 平均值）。多核可超 100。
    pub cpu_pct: f32,
    /// true=差分瞬时值；false=回落 ps 生命周期平均（详情区标注口径）。
    pub cpu_instant: bool,
    pub mem_pct: f32,
    /// 字节。
    pub rss: u64,
    pub nlwp: Option<u32>,
    pub elapsed_s: Option<f64>,
    pub stat: String,
    pub args: String,
}

/// 排序键（与表头列一一对应）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ProcSortKey {
    Pid,
    Cpu,
    Mem,
    Rss,
    Cmd,
}

impl ProcSortKey {
    /// 自然方向：占用类数值列降序（高的在前），PID/命令列升序。
    fn natural_desc(self) -> bool {
        matches!(self, Self::Cpu | Self::Mem | Self::Rss)
    }
}

/// 当前排序（表头点击切换）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcSort {
    pub key: ProcSortKey,
    pub desc: bool,
}

impl Default for ProcSort {
    fn default() -> Self {
        Self { key: ProcSortKey::Cpu, desc: true }
    }
}

impl ProcSort {
    /// 表头点击：同列翻转方向；异列换列并取自然方向。
    pub(crate) fn toggle(&mut self, key: ProcSortKey) {
        if self.key == key {
            self.desc = !self.desc;
        } else {
            self.key = key;
            self.desc = key.natural_desc();
        }
    }
}

/// 一个已连接 tab 的进程面板状态。
pub(crate) struct ProcState {
    /// 最近一次探针错误（已有数据后失败保持旧值静默，不写 error）。
    pub error: Option<String>,
    /// 当前展示行（已按 [`ProcSort`] 排序）。
    pub rows: Vec<ProcRow>,
    pub sort: ProcSort,
    /// 选中的进程（详情区展示对象；按 pid 跨刷新保持）。
    pub selected: Option<u32>,
    /// 探针在飞（防 ticker 重复并发）。
    pub probing: bool,
    /// 上次探针发起时刻（节流口径 = 发起间隔）。
    pub last_probe_at: Option<Instant>,
}

impl ProcState {
    fn new() -> Self {
        Self {
            error: None,
            rows: Vec::new(),
            sort: ProcSort::default(),
            selected: None,
            probing: false,
            last_probe_at: None,
        }
    }

    /// 吸收一次探针：以第二拍为当前帧，双拍同 pid 差分 cputimes 得瞬时
    /// CPU%；重排展示行。探针期间退出的进程不在第二拍 → 自然消失（选中
    /// 的进程消失时清选中，详情区回空态）。
    pub(crate) fn ingest(&mut self, d: ProbeData) {
        self.error = None;
        let Some(s2) = d.ps2 else { return };
        // 差分窗口：远端时间戳 Δt（非正 / 离谱大 / 缺失 → 标称 1s）。
        let dt = match (d.t1, d.t2) {
            (Some(a), Some(b)) if (b - a) > 0.05 && (b - a) < 60. => b - a,
            _ => 1.,
        };
        // 第一拍同 pid 的累计 CPU 秒索引；两拍都带 CPUTIMES 才可差分。
        let ct1: HashMap<u32, f64> = if s2.layout.has_ct {
            d.ps1
                .as_ref()
                .filter(|s| s.layout.has_ct)
                .map(|s| {
                    s.rows
                        .iter()
                        .filter_map(|r| r.cputime_s.map(|ct| (r.pid, ct)))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let can_diff = s2.layout.has_ct && !ct1.is_empty();
        self.rows = s2
            .rows
            .iter()
            .map(|r| ProcRow {
                pid: r.pid,
                ppid: r.ppid,
                user: r.user.clone(),
                cpu_pct: match (can_diff, r.cputime_s, ct1.get(&r.pid)) {
                    (true, Some(ct2), Some(ct1v)) => {
                        ((ct2 - ct1v).max(0.) / dt * 100.) as f32
                    }
                    // 新进程（第一拍无）/ 无 cputimes / 时钟异常 → ps 平均值。
                    _ => r.pcpu,
                },
                cpu_instant: can_diff && r.cputime_s.is_some() && ct1.contains_key(&r.pid),
                mem_pct: r.pmem,
                rss: r.rss_kb.saturating_mul(1024),
                nlwp: r.nlwp,
                elapsed_s: r.elapsed_s,
                stat: r.stat.clone(),
                args: r.args.clone(),
            })
            .collect();
        if let Some(pid) = self.selected {
            if !self.rows.iter().any(|r| r.pid == pid) {
                self.selected = None;
            }
        }
        self.sort_rows();
    }

    /// 按当前排序重排（数据到达 / 切换排序键时调用）。
    pub(crate) fn sort_rows(&mut self) {
        let sort = self.sort;
        self.rows.sort_by(|a, b| {
            let ord = match sort.key {
                ProcSortKey::Pid => a.pid.cmp(&b.pid),
                ProcSortKey::Cpu => {
                    a.cpu_pct.partial_cmp(&b.cpu_pct).unwrap_or(Ordering::Equal)
                }
                ProcSortKey::Mem => {
                    a.mem_pct.partial_cmp(&b.mem_pct).unwrap_or(Ordering::Equal)
                }
                ProcSortKey::Rss => a.rss.cmp(&b.rss),
                // 大小写不敏感（目录习惯：小写系统进程不因大小写跳序）。
                ProcSortKey::Cmd => a
                    .args
                    .to_lowercase()
                    .cmp(&b.args.to_lowercase()),
            };
            (if sort.desc { ord.reverse() } else { ord })
                // 同值稳定次序：pid 升序（与方向无关）。
                .then_with(|| a.pid.cmp(&b.pid))
        });
    }
}

// ---------- 进程识别（本地规则库 + 外部搜索） ----------

/// 按命令行判断进程作用（本地规则库；未收录返回 None → 详情显示「未收录」，
/// 可点标题栏搜索按钮到浏览器识别——AI 识别为后续扩展入口）。
/// 匹配 = 命令首段的 basename 小写**前缀**（先具体后通用，顺序敏感）；
/// `[xxx]` 方括号开头特判为内核线程。
pub(crate) fn proc_kind(loc: crate::i18n::Locale, args: &str) -> Option<String> {
    use crate::i18n::{Tr, tl};
    if args.trim_start().starts_with('[') {
        return Some(tl(loc, Tr::PkKernelThread).into());
    }
    let base = args
        .split_whitespace()
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(':')
        .to_lowercase();
    // 顺序敏感：长名/具体在前（systemd-journald 先于 systemd）。
    const RULES: &[(&str, Tr)] = &[
        ("systemd-journald", Tr::PkSystemdJournald),
        ("systemd-udevd", Tr::PkSystemdUdevd),
        ("systemd", Tr::PkSystemd),
        ("kworker", Tr::PkKernelThread),
        ("ksoftirqd", Tr::PkKernelThread),
        ("kthread", Tr::PkKernelThread),
        ("migration", Tr::PkKernelThread),
        ("rcu", Tr::PkKernelThread),
        ("zswap", Tr::PkKernelThread),
        ("sshd", Tr::PkSshd),
        ("nginx", Tr::PkNginx),
        ("caddy", Tr::PkNginx),
        ("apache", Tr::PkNginx),
        ("mysqld", Tr::PkMysql),
        ("mariadbd", Tr::PkMysql),
        ("psql", Tr::PkPostgres),
        ("postgres", Tr::PkPostgres),
        ("redis", Tr::PkRedis),
        ("memcached", Tr::PkMemcached),
        ("mongod", Tr::PkPostgres),
        ("node", Tr::PkNode),
        ("python", Tr::PkPython),
        ("java", Tr::PkJava),
        ("dockerd", Tr::PkDocker),
        ("containerd", Tr::PkContainerd),
        ("docker", Tr::PkDocker),
        ("podman", Tr::PkDocker),
        ("bash", Tr::PkShell),
        ("zsh", Tr::PkShell),
        ("fish", Tr::PkShell),
        ("cron", Tr::PkCron),
        ("atd", Tr::PkCron),
        ("rsyslogd", Tr::PkSyslog),
        ("syslogd", Tr::PkSyslog),
        ("journald", Tr::PkSystemdJournald),
        ("prometheus", Tr::PkPrometheus),
        ("grafana", Tr::PkGrafana),
        ("dhclient", Tr::PkDhclient),
        ("systemd-networkd", Tr::PkSystemdUdevd),
        ("snapd", Tr::PkSnapd),
        ("apt", Tr::PkPackageManager),
        ("yum", Tr::PkPackageManager),
        ("dnf", Tr::PkPackageManager),
        ("vim", Tr::PkVim),
        ("nano", Tr::PkVim),
        ("sudo", Tr::PkSudo),
    ];
    RULES
        .iter()
        .find(|(key, _)| base.starts_with(key))
        .map(|(_, tr)| tl(loc, *tr).into())
}

/// ps STAT 首字母解读（R/S/D/Z/T），后缀标志保留原文。
fn stat_word(loc: crate::i18n::Locale, stat: &str) -> String {
    use crate::i18n::{Tr, tl};
    let word = match stat.chars().next() {
        Some('R') => tl(loc, Tr::ProcStatRunning),
        Some('S') => tl(loc, Tr::ProcStatSleeping),
        Some('D') => tl(loc, Tr::ProcStatUninterrupt),
        Some('Z') => tl(loc, Tr::ProcStatZombie),
        Some('T') => tl(loc, Tr::ProcStatStopped),
        _ => return stat.to_string(),
    };
    format!("{word} ({stat})")
}

/// 外部搜索引擎（进程详情搜索用；设置「通用 › 系统 › 搜索引擎」选择，
/// 持久化键 [`SETTING_SEARCH_ENGINE`]）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum SearchEngine {
    #[default]
    Google,
    Baidu,
    Bing,
}

/// 搜索引擎设置持久化键（值见 [`SearchEngine::as_str`]）。
pub(crate) const SETTING_SEARCH_ENGINE: &str = "app.search.engine";

impl SearchEngine {
    /// 从 DB 值解析（脏数据回退 Google）。
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "baidu" => Self::Baidu,
            "bing" => Self::Bing,
            _ => Self::Google,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Baidu => "baidu",
            Self::Bing => "bing",
        }
    }

    /// 设置 chips 迭代目录。
    pub(crate) const ALL: [Self; 3] = [Self::Google, Self::Baidu, Self::Bing];

    /// 显示名（专名，中英一致，不走 Tr）。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Baidu => "Baidu",
            Self::Bing => "Bing",
        }
    }

    /// 按所选引擎的 URL 规则组织搜索地址（查询参数名各家不同：
    /// google/bing 用 `q`，baidu 用 `wd`）。
    fn search_url(self, query: &str) -> String {
        let q = urlencode(query);
        match self {
            Self::Google => format!("https://www.google.com/search?q={q}"),
            Self::Baidu => format!("https://www.baidu.com/s?wd={q}"),
            Self::Bing => format!("https://www.bing.com/search?q={q}"),
        }
    }
}

/// URL 查询编码（保留字母数字与 `-_.`；进程名基本 ASCII，覆盖足够）。
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 用系统浏览器搜索（macOS `open` / Linux `xdg-open` / Windows `start`）。
fn open_web_search(engine: SearchEngine, query: &str) {
    let url = engine.search_url(query);
    let (cmd, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C".into(), "start".into(), url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(cmd).args(args).spawn();
}

// ---------- 布局常量 / 拖拽标记 ----------

/// 详情区高度下限 / 默认（px；上限随窗口高动态钳制）。
pub(crate) const DETAIL_MIN_H: f32 = 120.;
pub(crate) const DETAIL_DEFAULT_H: f32 = 176.;
/// 分割线高（px）。
const DETAIL_SPLIT_H: f32 = 5.;
/// 列表区保底（详情上限 = 面板高 − 标题 − 表头 − 分割线 − 此值）。
const LIST_MIN_H: f32 = 160.;
/// 详情区高度持久化键。
pub(crate) const SETTING_PROCS_DETAIL_H: &str = "procs.detail.h";

pub(crate) fn procs_detail_max_h(win_h: f32) -> f32 {
    (win_h - crate::dir_panel::DIR_TITLE_H - HDR_H - DETAIL_SPLIT_H - LIST_MIN_H).max(DETAIL_MIN_H)
}

/// 进程页分割线拖拽标记（锚点：鼠标 y、起始详情高；与 DirSplitDrag 分立
/// 防 on_drag_move 交叉触发）。
pub(crate) struct ProcSplitDrag(pub(crate) Arc<Mutex<Option<(f32, f32)>>>);

// ---------- 行为（AppUi 扩展） ----------

impl crate::app_ui::AppUi {
    /// ticker / 页面切换入口：面板可见且活跃 tab 在线时触发一次探针
    /// （在飞或距上次发起 <4s 跳过）。
    pub(crate) fn maybe_probe_procs(&mut self, cx: &mut Context<Self>) {
        if !self.right_open || self.right_page != RightPage::Processes {
            return;
        }
        let Some(tab) = self.active_live_tab() else { return };
        let id = tab.id;
        let st = self.proc_data.entry(id).or_insert_with(ProcState::new);
        if st.probing {
            return;
        }
        if let Some(t0) = st.last_probe_at {
            if t0.elapsed() < PROBE_MIN_INTERVAL {
                return;
            }
        }
        st.probing = true;
        st.last_probe_at = Some(Instant::now());
        self.procs_probe(id, cx);
    }

    /// 后台跑探针；双拍差分入状态并刷新。
    fn procs_probe(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(session) = self.live_session(tab_id) else {
            if let Some(st) = self.proc_data.get_mut(&tab_id) {
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
                let Some(st) = ui.proc_data.get_mut(&tab_id) else { return };
                st.probing = false;
                match out {
                    Ok(text) => st.ingest(parse_probe(&text)),
                    // 已有数据后偶发失败（超时/断连瞬间）保持旧值静默；
                    // 从未成功过才显示错误。
                    Err(e) => {
                        if st.rows.is_empty() {
                            st.error = Some(e);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 表头点击：切换排序（同列翻转 / 异列自然方向）并重排。
    pub(crate) fn procs_toggle_sort(&mut self, key: ProcSortKey, cx: &mut Context<Self>) {
        let Some(id) = self.active_live_tab().map(|t| t.id) else { return };
        let Some(st) = self.proc_data.get_mut(&id) else { return };
        st.sort.toggle(key);
        st.sort_rows();
        cx.notify();
    }

    /// 列表行点击：选中进程（详情区展示）。
    pub(crate) fn procs_select(&mut self, pid: u32, cx: &mut Context<Self>) {
        let Some(id) = self.active_live_tab().map(|t| t.id) else { return };
        let Some(st) = self.proc_data.get_mut(&id) else { return };
        if st.selected != Some(pid) {
            st.selected = Some(pid);
            cx.notify();
        }
    }

    /// 详情区高度分割线拖拽：鼠标上移 → 详情区变高（与侧栏/目录页分割线同向）；
    /// 高度持久化，窗口过矮时按上限钳制（持久值不动，恢复窗口还原）。
    fn on_procs_splitter_drag(
        &mut self,
        e: &gpui::DragMoveEvent<ProcSplitDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let y: f32 = e.event.position.y.into();
        let state = e.drag(cx).0.clone();
        let mut anchor = state.lock().expect("procs split drag lock");
        if !e.event.dragging() {
            *anchor = None;
            return;
        }
        let &mut (start_y, start_h) = anchor.get_or_insert((y, self.procs_detail_h));
        let win_h: f32 = window.viewport_size().height.into();
        let h = (start_h + (start_y - y)).clamp(DETAIL_MIN_H, procs_detail_max_h(win_h));
        if h != self.procs_detail_h {
            self.procs_detail_h = h;
            self.db.set_setting(SETTING_PROCS_DETAIL_H, &format!("{h}"));
            cx.notify();
        }
    }

    /// 外部搜索选中进程（浏览器打开；查询词随界面语言；AI 识别为后续扩展）。
    pub(crate) fn procs_open_search(&mut self, _cx: &mut Context<Self>) {
        let Some(st) = self.active_live_tab().and_then(|t| self.proc_data.get(&t.id)) else { return };
        let Some(pid) = st.selected else { return };
        let Some(row) = st.rows.iter().find(|r| r.pid == pid) else { return };
        let base = row
            .args
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(':');
        if base.is_empty() {
            return;
        }
        let q = match self.loc {
            crate::i18n::Locale::Zh => format!("linux {base} 进程 是什么"),
            crate::i18n::Locale::En => format!("what is linux {base} process"),
        };
        open_web_search(self.search_engine, &q);
    }

    /// 设置窗回调：切换外部搜索引擎（写库 + 内存态；搜索按需读取）。
    pub(crate) fn set_search_engine(&mut self, e: SearchEngine, cx: &mut Context<Self>) {
        if self.search_engine != e {
            self.search_engine = e;
            self.db.set_setting(SETTING_SEARCH_ENGINE, e.as_str());
        }
        cx.notify();
    }

    // ---------- 渲染 ----------

    /// 进程页右面板（上下两区：标题 + 排序表头 + 虚拟滚动列表 / 分割线 /
    /// 进程详细钉底 + 宽度拖柄）。
    pub(crate) fn render_procs_panel(&self, win_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        // 窗口过矮时钳生效值（持久值不动）。
        let detail_h = self.procs_detail_h.min(procs_detail_max_h(win_h));
        div()
            .id("right-panel-procs")
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
            .child(self.render_procs_title(cx))
            .child(self.render_procs_header(cx))
            .child(self.render_procs_area(win_h, detail_h, cx))
            .child(self.render_procs_splitter(detail_h, cx))
            .child(self.render_procs_detail(detail_h, cx))
            .child(self.render_right_width_handle(cx))
            .into_any_element()
    }

    /// 标题行：「进程 · 服务器名」。
    fn render_procs_title(&self, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let mut row = div()
            .id("procs-title")
            .flex()
            .items_center()
            .h(px(crate::dir_panel::DIR_TITLE_H))
            .px_3()
            .flex_none()
            .text_size(px(11.))
            .text_color(rgb(pal.dim))
            .child(div().child(crate::i18n::t(cx, crate::i18n::Tr::ProcTitle)));
        if let Some(tab) = self.active_live_tab() {
            let name: String = tab.title.chars().take(18).collect();
            row = row.child(div().text_color(rgb(pal.muted)).child(format!(" · {name}")));
        }
        row.into_any_element()
    }

    /// 排序表头：每列可点击（当前列亮色 + ▾/▴ 方向标），列宽与数据行共用
    /// 常量保证纵向对齐。
    fn render_procs_header(&self, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let sort = self
            .active_live_tab()
            .and_then(|t| self.proc_data.get(&t.id))
            .map(|st| st.sort)
            .unwrap_or_default();
        div()
            .id("procs-header")
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(HDR_H))
            .flex_none()
            .border_b_1()
            .border_color(rgb(pal.divider()))
            .child(header_col(
                sort,
                ProcSortKey::Pid,
                crate::i18n::Tr::ProcColPid,
                Some(COL_PID),
                false,
                &pal,
                cx,
            ))
            .child(header_col(
                sort,
                ProcSortKey::Cpu,
                crate::i18n::Tr::ProcColCpu,
                Some(COL_CPU),
                true,
                &pal,
                cx,
            ))
            .child(header_col(
                sort,
                ProcSortKey::Mem,
                crate::i18n::Tr::ProcColMem,
                Some(COL_MEM),
                true,
                &pal,
                cx,
            ))
            .child(header_col(
                sort,
                ProcSortKey::Rss,
                crate::i18n::Tr::ProcColRss,
                Some(COL_RSS),
                true,
                &pal,
                cx,
            ))
            .child(header_col(
                sort,
                ProcSortKey::Cmd,
                crate::i18n::Tr::ProcColCmd,
                None,
                false,
                &pal,
                cx,
            ))
            .into_any_element()
    }

    /// 列表区（上区）：四态（未连接 / 首探失败 / 首探在飞 / 虚拟滚动列表），
    /// 底部让位分割线 + 详情区。
    fn render_procs_area(&self, win_h: f32, detail_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let vp_h = (win_h - crate::dir_panel::DIR_TITLE_H - HDR_H - DETAIL_SPLIT_H - detail_h)
            .max(0.);
        let area = div()
            .id("procs-area")
            .absolute()
            .top(px(crate::dir_panel::DIR_TITLE_H + HDR_H))
            .bottom(px(DETAIL_SPLIT_H + detail_h))
            .left_0()
            .right_0();
        let live = self
            .active_live_tab()
            .and_then(|t| self.proc_data.get(&t.id).map(|st| (t.id, st)));
        let hint = |el: gpui::Stateful<gpui::Div>, color: u32, text: String| {
            el.flex()
                .items_center()
                .justify_center()
                .child(div().px_3().text_size(px(11.)).text_color(rgb(color)).child(text))
                .into_any_element()
        };
        match live {
            None => hint(
                area,
                pal.dim,
                crate::i18n::t(cx, crate::i18n::Tr::ProcEmptyHint).into(),
            ),
            Some((_, st)) if st.rows.is_empty() && st.error.is_some() => {
                hint(area, pal.error, st.error.clone().unwrap_or_default())
            }
            Some((_, st)) if st.rows.is_empty() => hint(
                area,
                pal.dim,
                crate::i18n::t(cx, crate::i18n::Tr::ProcNoData).into(),
            ),
            Some((_, st)) => {
                // —— 虚拟滚动窗口：只渲染可视行（顶部 spacer 撑偏移）——
                let content_h = st.rows.len() as f32 * ROW_H;
                let max_y = (content_h - vp_h).max(0.);
                let off = self.procs_scroll_y.clamp(0., max_y);
                let first = (off / ROW_H) as usize;
                let vis = ((vp_h / ROW_H).ceil() as usize).max(1) + 1;
                let last = (first + vis).min(st.rows.len());
                let content_w = (self.right_w - 14.).max(120.);
                let rows_el = div()
                    .id("procs-rows")
                    .w(px(content_w))
                    .h(px(vp_h))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    // 顶部 spacer = first 行被滚出视口后的**残余**高度
                    //（first×ROW_H − off ∈ [0, ROW_H)），使行 i 落在
                    // y = i×ROW_H − off——内容随偏移整体上移；若写成
                    // first×ROW_H，行 i 恒在 y = i×ROW_H，滚动时只见
                    // 顶部变空白、行不上移。
                    .child(div().flex_none().h(px((first as f32 * ROW_H - off).max(0.))))
                    .children(
                        st.rows[first..last]
                            .iter()
                            .enumerate()
                            .map(|(i, r)| proc_row(r, first + i, st.selected == Some(r.pid), &pal, cx)),
                    );
                // 回调里改 AppUi 自管偏移并 notify（TextArea 虚拟模式同款链路）。
                let entity = cx.entity();
                let entity2 = cx.entity();
                area.child(
                    ScrollArea::virtual_y(
                        self.procs_scroll.clone(),
                        rows_el,
                        off,
                        max_y,
                        vp_h,
                        vp_h,
                        move |top, _w, cx| {
                            entity.update(cx, |a, cx| {
                                a.procs_scroll_y = top.clamp(0., max_y);
                                cx.notify();
                            });
                        },
                    )
                    .on_scroll_delta(move |dy, _w, cx| {
                        // gpui 约定 dy 正值=向内容顶部；正偏移=滚入内容 → 反号。
                        entity2.update(cx, |a, cx| {
                            a.procs_scroll_y = (a.procs_scroll_y - dy).clamp(0., max_y);
                            cx.notify();
                        });
                    })
                    .theme(pal.kit()),
                )
                .into_any_element()
            }
        }
    }

    /// 分割线（钉在详情区顶缘；拖拽调列表/详情两区高度）。
    fn render_procs_splitter(&self, detail_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let drag = ProcSplitDrag(self.procs_split_drag.clone());
        div()
            .id("procs-splitter")
            .absolute()
            .bottom(px(detail_h))
            .left_0()
            .w_full()
            .h(px(DETAIL_SPLIT_H))
            .flex()
            .items_center()
            .cursor(CursorStyle::ResizeRow)
            .occlude()
            .child(div().h(px(1.)).w_full().bg(rgb(pal.divider())))
            .hover(|s| s.bg(rgb(pal.border)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseDownEvent, _w, _cx| {
                    let y: f32 = ev.position.y.into();
                    *this.procs_split_drag.lock().expect("procs split drag lock") =
                        Some((y, this.procs_detail_h));
                }),
            )
            .on_drag(drag, move |_, _, _, cx| {
                cx.new(|_| crate::sidebar::SplitterGhost)
            })
            .on_drag_move(cx.listener(
                |this, e: &gpui::DragMoveEvent<ProcSplitDrag>, w, cx| {
                    this.on_procs_splitter_drag(e, w, cx);
                },
            ))
            .into_any_element()
    }

    /// 进程详细（钉底）：未选中=提示；选中=完整字段 + 命令行 + 作用判断 +
    /// 外部搜索按钮（标题栏 tools 位）。内容超高走真实滚动。
    fn render_procs_detail(&self, detail_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let kit = pal.kit();
        let selected = self
            .active_live_tab()
            .and_then(|t| self.proc_data.get(&t.id))
            .and_then(|st| st.selected.and_then(|pid| st.rows.iter().find(|r| r.pid == pid).cloned()));
        // 标题行：icon + 「进程详细」 + 搜索按钮（选中时可用）。
        let head = div()
            .id("procs-detail-head")
            .flex()
            .items_center()
            .h(px(DETAIL_HEAD_H))
            .px_2()
            .flex_none()
            .gap_1()
            .child(crate::app_ui::svg_icon(crate::icons::INFO, 12., pal.dim))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(pal.muted))
                    .child(crate::i18n::t(cx, crate::i18n::Tr::ProcDetailTitle)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("procs-detail-search")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded_md()
                    .hover(|s| s.bg(rgb(pal.border)))
                    // svg 必须显式着色（父级 text_color 继承不进 svg）。
                    .child(crate::app_ui::svg_icon(crate::icons::SEARCH, 11., pal.muted))
                    .when(selected.is_some(), |d| {
                        d.on_click(cx.listener(|this, _ev: &gpui::ClickEvent, _w, cx| {
                            this.procs_open_search(cx);
                        }))
                        .tooltip(move |_, cx| {
                            ui_kit::base::tooltip::tip(
                                crate::i18n::t(cx, crate::i18n::Tr::ProcSearchTip),
                                kit,
                                cx,
                            )
                        })
                    }),
            );
        let body = match selected {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px_3()
                        .text_size(px(11.))
                        .text_color(rgb(pal.dim))
                        .child(crate::i18n::t(cx, crate::i18n::Tr::ProcDetailEmptyHint)),
                )
                .into_any_element(),
            Some(r) => {
                let content = div()
                    .id("procs-detail-body")
                    .w(px((self.right_w - 14.).max(120.)))
                    .py_1()
                    .pl_1()
                    .pr(px(2.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(detail_row(cx, crate::i18n::Tr::ProcDPid, r.pid.to_string(), &pal))
                    .child(detail_row(
                        cx,
                        crate::i18n::Tr::ProcDParent,
                        r.ppid.map(|p| p.to_string()).unwrap_or_else(|| "—".into()),
                        &pal,
                    ))
                    .child(detail_row(
                        cx,
                        crate::i18n::Tr::ProcDUser,
                        r.user.clone().unwrap_or_else(|| "—".into()),
                        &pal,
                    ))
                    .child(detail_row(cx, crate::i18n::Tr::ProcDStat, stat_word(self.loc, &r.stat), &pal))
                    .child(detail_row(
                        cx,
                        crate::i18n::Tr::ProcDThreads,
                        r.nlwp.map(|n| n.to_string()).unwrap_or_else(|| "—".into()),
                        &pal,
                    ))
                    .child(detail_row(
                        cx,
                        crate::i18n::Tr::ProcDElapsed,
                        r.elapsed_s
                            .map(|s| crate::sysinfo::fmt_uptime(self.loc, s))
                            .unwrap_or_else(|| "—".into()),
                        &pal,
                    ))
                    .child(detail_row(
                        cx,
                        crate::i18n::Tr::ProcDCpu,
                        {
                            let suffix = crate::i18n::tl(
                                self.loc,
                                if r.cpu_instant {
                                    crate::i18n::Tr::ProcCpuInstantSuffix
                                } else {
                                    crate::i18n::Tr::ProcCpuAvgSuffix
                                },
                            );
                            format!("{:.1}%{suffix}", r.cpu_pct)
                        },
                        &pal,
                    ))
                    .child(detail_row(
                        cx,
                        crate::i18n::Tr::ProcDMem,
                        format!("{:.1}%", r.mem_pct),
                        &pal,
                    ))
                    .child(detail_row(cx, crate::i18n::Tr::ProcDRss, theme::fmt_bytes(r.rss), &pal))
                    .child(detail_row(
                        cx,
                        crate::i18n::Tr::ProcDKind,
                        proc_kind(self.loc, &r.args)
                            .unwrap_or_else(|| crate::i18n::tl(self.loc, crate::i18n::Tr::PkUnknown).into()),
                        &pal,
                    ))
                    // 命令行：完整不截断（换行展示）。
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.dim))
                                    .child(crate::i18n::t(cx, crate::i18n::Tr::ProcDCmd)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.muted))
                                    .child(r.args),
                            ),
                    );
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        ScrollArea::new(self.procs_detail_scroll.clone(), content)
                            .theme(pal.kit()),
                    )
                    .into_any_element()
            }
        };
        div()
            .id("procs-detail")
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .h(px(detail_h))
            .flex_col()
            .border_t_1()
            .border_color(rgb(pal.divider()))
            .overflow_hidden()
            .child(head)
            .child(body)
            .into_any_element()
    }
}

// ---------- 渲染基元（无状态小件） ----------

/// 行高 / 表头高（px）。
const ROW_H: f32 = 22.;
const HDR_H: f32 = 24.;
/// 详情区标题行高（px）。
const DETAIL_HEAD_H: f32 = 26.;
/// 固定列宽（表头与数据行共用，保证纵向对齐）；命令列为弹性剩余。
const COL_PID: f32 = 42.;
const COL_CPU: f32 = 46.;
const COL_MEM: f32 = 46.;
const COL_RSS: f32 = 52.;
/// 命令列单行渲染的最大字符数（超长命令行防布局计算爆炸，渲染期再截断；
/// 完整命令行在详情区展示）。
const ARGS_MAX_CHARS: usize = 160;

/// 表头单元格：当前排序列亮色 + ▾/▴；hover 提亮；点击切换排序。
/// `width = None` 为弹性命令列。
fn header_col(
    sort: ProcSort,
    key: ProcSortKey,
    label: crate::i18n::Tr,
    width: Option<f32>,
    right: bool,
    pal: &PalFx,
    cx: &Context<crate::app_ui::AppUi>,
) -> AnyElement {
    let active = sort.key == key;
    let arrow = if active {
        if sort.desc {
            " ▾"
        } else {
            " ▴"
        }
    } else {
        ""
    };
    let mut d = div()
        .id(("procs-h", key as usize))
        .flex()
        .items_center()
        .flex_none()
        .text_size(px(10.))
        .text_color(rgb(if active { pal.muted } else { pal.dim }))
        .hover(|s| s.text_color(rgb(pal.text)))
        .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
            this.procs_toggle_sort(key, cx);
        }))
        .child(format!("{}{}", crate::i18n::t(cx, label), arrow));
    d = match width {
        Some(w) => d.w(px(w)),
        None => d.flex_1().min_w_0(),
    };
    if right {
        d = d.justify_end();
    }
    d.into_any_element()
}

/// 数据行：固定列与表头同宽同对齐；CPU% 按占用分级着色；命令列弹性截断；
/// 点击选中（详情区展示），选中行底色高亮。`idx` 为绝对行号。
fn proc_row(
    r: &ProcRow,
    idx: usize,
    selected: bool,
    pal: &PalFx,
    cx: &Context<crate::app_ui::AppUi>,
) -> AnyElement {
    let cpu_color = if r.cpu_pct >= 90. {
        pal.error
    } else if r.cpu_pct >= 50. {
        pal.warn
    } else {
        pal.muted
    };
    let args: String = r.args.chars().take(ARGS_MAX_CHARS).collect();
    let pid = r.pid;
    let mut d = div()
        .id(("procs-row", idx))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .w_full()
        .h(px(ROW_H))
        .flex_none()
        .text_size(px(10.5))
        .hover(|s| s.bg(rgb(pal.border)))
        .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
            this.procs_select(pid, cx);
        }))
        .child(
            div()
                .w(px(COL_PID))
                .flex_none()
                // PID 用 muted（与数值列同级）：行内主体是命令列（text），
                // 标识与数值退半档做层级。
                .text_color(rgb(if selected { pal.text } else { pal.muted }))
                .child(r.pid.to_string()),
        )
        .child(
            div()
                .w(px(COL_CPU))
                .flex_none()
                .flex()
                .justify_end()
                .text_color(rgb(cpu_color))
                .child(format!("{:.1}", r.cpu_pct)),
        )
        .child(
            div()
                .w(px(COL_MEM))
                .flex_none()
                .flex()
                .justify_end()
                .text_color(rgb(pal.muted))
                .child(format!("{:.1}", r.mem_pct)),
        )
        .child(
            div()
                .w(px(COL_RSS))
                .flex_none()
                .flex()
                .justify_end()
                .text_color(rgb(pal.muted))
                .child(theme::fmt_bytes(r.rss)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                // 命令列是行内主体：text 档与 tab/树的服务器名同级
                //（用户反馈 muted 档明显偏暗）。
                .text_color(rgb(pal.text))
                .child(args),
        );
    if selected {
        d = d.bg(rgb(pal.border));
    }
    d.into_any_element()
}

/// 详情字段行：label 左、值右对齐弹性截断（同 sysinfo 的 sys_row 形态）。
fn detail_row(cx: &gpui::App, label: crate::i18n::Tr, value: String, pal: &PalFx) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .w(px(60.))
                .flex_none()
                .text_size(px(10.5))
                .text_color(rgb(pal.dim))
                .child(crate::i18n::t(cx, label)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(10.5))
                .text_color(rgb(pal.muted))
                .flex()
                .justify_end()
                .child(value),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;

    /// 双拍全量（11 列）样例：两拍 dt=1.5s；pid42 双核满载、pid999 第二拍新起。
    fn probe_sample() -> String {
        concat!(
            "@@t1\n1000000000.000000000\n@@ps1\n",
            "  PID  PPID USER                %CPU %MEM    RSS NLWP CPUTIMES ELAPSED STAT COMMAND\n",
            "    1     0 root                 0.0  0.1   1234    1    10.00  2-03:04:05 Ss   /sbin/init splash\n",
            "   42     1 app                  2.5  3.2  56000    4   100.00     05:06:07 R    python3 server.py --port 8080 --verbose\n",
            "@@t2\n1000000001.500000000\n@@ps2\n",
            "  PID  PPID USER                %CPU %MEM    RSS NLWP CPUTIMES ELAPSED STAT COMMAND\n",
            "    1     0 root                 0.0  0.1   1234    1    11.00  2-03:04:06 Ss   /sbin/init splash\n",
            "   42     1 app                  2.5  3.2  56000    4   102.25     05:06:08 R    python3 server.py --port 8080 --verbose\n",
            "  999     2 root                 9.9  0.0    200    1     0.00     01:23 Ss   [kworker/u:2]\n",
            "@@end\n",
        )
        .into()
    }

    /// 全量解析：时间戳、11 列布局（表头驱动）、详情字段、args 内空格保真。
    #[test]
    fn parse_full_probe() {
        let d = parse_probe(&probe_sample());
        assert_eq!(d.t1, Some(1000000000.));
        assert_eq!(d.t2, Some(1000000001.5));
        let s2 = d.ps2.as_ref().unwrap();
        assert!(s2.layout.has_ct);
        assert_eq!(s2.rows.len(), 3);
        let r = &s2.rows[1];
        assert_eq!(r.pid, 42);
        assert_eq!(r.ppid, Some(1));
        assert_eq!(r.user.as_deref(), Some("app"));
        assert_eq!(r.pcpu, 2.5);
        assert_eq!(r.rss_kb, 56000);
        assert_eq!(r.nlwp, Some(4));
        assert_eq!(r.cputime_s, Some(102.25));
        // ELAPSED 05:06:08 → 18368s（第二拍比第一拍 +1s）。
        assert_eq!(r.elapsed_s, Some(5. * 3600. + 6. * 60. + 8.));
        assert_eq!(r.stat, "R");
        assert_eq!(r.args, "python3 server.py --port 8080 --verbose");
        // pid1 第二拍 2-03:04:06 → 183846s。
        assert_eq!(s2.rows[0].elapsed_s, Some(2. * 86400. + 3. * 3600. + 4. * 60. + 6.));
    }

    /// 降级链输出（7 列，无详情字段）：布局按表头判定，详情字段为 None。
    #[test]
    fn parse_fallback_probe() {
        let out = concat!(
            "@@t1\n1697000000.%N\n@@ps1\n",
            "  PID USER                  %CPU %MEM    RSS STAT COMMAND\n",
            "    1 root                  0.0  0.1   1234 Ss   /sbin/init\n",
            "@@t2\n1697000001.%N\n@@ps2\n",
            "  PID USER                  %CPU %MEM    RSS STAT COMMAND\n",
            "    1 root                  0.0  0.1   1234 Ss   /sbin/init\n",
            "@@end\n",
        );
        let d = parse_probe(out);
        // BSD date 不支持 %N → 字面输出，时间戳不认 → None。
        assert!(d.t1.is_none() && d.t2.is_none());
        let s = d.ps2.as_ref().unwrap();
        assert!(!s.layout.has_ct);
        assert_eq!(s.rows.len(), 1);
        let r = &s.rows[0];
        assert_eq!(r.cputime_s, None);
        assert_eq!(r.ppid, None);
        assert_eq!(r.nlwp, None);
        assert_eq!(r.elapsed_s, None);
        assert_eq!(r.stat, "Ss");
    }

    /// 噪声容错：ps 节无表头整节丢弃、坏行跳过、坏 pid 跳行。
    #[test]
    fn parse_tolerates_noise() {
        let out = concat!(
            "@@ps1\nps: unknown option\n@@ps2\n",
            "  PID  PPID USER                %CPU %MEM    RSS NLWP CPUTIMES ELAPSED STAT COMMAND\n",
            "  bad line\n",
            "  abc root                  0.0  0.0     10      1      0.00     01:23 Ss   short\n",
            "   10     1 root                 0.0  0.0     10      1      0.00     01:23 Ss   ok\n",
            "@@end\n",
        );
        let d = parse_probe(out);
        assert!(d.ps1.is_none(), "无表头的节整体丢弃");
        let s = d.ps2.as_ref().unwrap();
        assert_eq!(s.rows.len(), 1, "坏行与坏 pid 跳过");
        assert_eq!(s.rows[0].pid, 10);
    }

    /// 差分链路：瞬时 CPU%（单核 66.7% / 多核 150%）、新进程回落 pcpu、
    /// 退出进程消失、默认按 CPU 降序、选中进程消失即清选中。
    #[test]
    fn ingest_cpu_deltas() {
        let mut st = ProcState::new();
        st.selected = Some(42);
        st.ingest(parse_probe(&probe_sample()));
        // 默认排序 cpu desc：150.0 / 66.7 / 9.9。
        assert_eq!(st.rows.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![42, 1, 999]);
        assert!((st.rows[0].cpu_pct - 150.0).abs() < 0.01, "2.25s/1.5s 多核");
        assert!(st.rows[0].cpu_instant);
        assert!((st.rows[1].cpu_pct - 200. / 3.).abs() < 0.01, "1s/1.5s");
        assert!((st.rows[2].cpu_pct - 9.9).abs() < 0.001, "新进程回落 ps 平均值");
        assert!(!st.rows[2].cpu_instant, "新进程无差分");
        assert_eq!(st.rows[1].rss, 1234 * 1024);
        assert_eq!(st.error, None);
        assert_eq!(st.selected, Some(42), "选中进程仍在 → 保持");
        // 下一拍 42 消失 → 清选中。
        let no42 = probe_sample().replace(
            "   42     1 app                  2.5  3.2  56000    4   102.25     05:06:08 R    python3 server.py --port 8080 --verbose\n",
            "",
        );
        st.ingest(parse_probe(&no42));
        assert_eq!(st.selected, None, "选中进程退出 → 清选中");
    }

    /// 时间戳异常（Δt 非正）→ 回退 1s 窗口；第二拍缺失 → 保持旧数据。
    #[test]
    fn ingest_bad_clock_keeps_fallback() {
        let out = probe_sample().replace("1000000001.500000000", "999999999.0");
        let mut st = ProcState::new();
        st.ingest(parse_probe(&out));
        // dt 回退 1s：pid42 = 2.25/1*100。
        let r42 = st.rows.iter().find(|r| r.pid == 42).unwrap();
        assert!((r42.cpu_pct - 225.0).abs() < 0.01);

        let mut st2 = ProcState::new();
        st2.ingest(parse_probe(&probe_sample()));
        st2.ingest(parse_probe("@@ps1\n@@end\n"));
        assert_eq!(st2.rows.len(), 3, "无第二拍 → 保持旧数据不清空");
    }

    /// 排序规则：同列翻转、异列自然方向（数值降序/文本升序）、同值 pid 稳定。
    #[test]
    fn sort_rules() {
        let mut st = ProcState::new();
        st.ingest(parse_probe(&probe_sample()));
        // 同列再点：cpu desc → asc。
        st.sort.toggle(ProcSortKey::Cpu);
        st.sort_rows();
        assert_eq!(
            st.rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![999, 1, 42]
        );
        // 换列取自然方向：PID 升序。
        st.sort.toggle(ProcSortKey::Pid);
        st.sort_rows();
        assert_eq!(
            st.rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![1, 42, 999]
        );
        // 命令列升序（大小写不敏感）：/sbin/init < [kworker…] < python3…
        // （'/' 0x2F < '[' 0x5B < 'p'）。
        st.sort.toggle(ProcSortKey::Cmd);
        st.sort_rows();
        assert_eq!(
            st.rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![1, 999, 42]
        );
    }

    /// 同值稳定次序：两条同 CPU 的行按 pid 升序，与排序方向无关。
    #[test]
    fn sort_stable_by_pid() {
        let mut st = ProcState::new();
        st.rows = vec![
            ProcRow { pid: 7, ppid: None, user: None, cpu_pct: 5., cpu_instant: true, mem_pct: 0., rss: 1, nlwp: None, elapsed_s: None, stat: "S".into(), args: "b".into() },
            ProcRow { pid: 3, ppid: None, user: None, cpu_pct: 5., cpu_instant: true, mem_pct: 0., rss: 1, nlwp: None, elapsed_s: None, stat: "S".into(), args: "a".into() },
        ];
        st.sort_rows();
        assert_eq!(st.rows[0].pid, 3);
        st.sort.toggle(ProcSortKey::Cpu); // desc → asc，同值仍 pid 升序。
        st.sort_rows();
        assert_eq!(st.rows[0].pid, 3);
    }

    /// ELAPSED 解析：mm:ss / hh:mm:ss / dd-hh:mm:ss / 非法。
    #[test]
    fn etime_parsing() {
        assert_eq!(parse_etime("01:23"), Some(83.));
        assert_eq!(parse_etime("05:06:07"), Some(18367.));
        assert_eq!(parse_etime("1-02:03:04"), Some(93784.));
        assert_eq!(parse_etime("90"), None, "procps ELAPSED 必带冒号");
        assert_eq!(parse_etime("bad"), None);
        assert_eq!(parse_etime("1:2:3:4"), None);
    }

    /// 进程识别：内核线程 / 路径 basename / 冒号后缀 / 未收录。
    #[test]
    fn proc_kinds() {
        assert_eq!(proc_kind(Locale::Zh, "[kworker/u:2]").as_deref(), Some("内核线程"));
        assert_eq!(proc_kind(Locale::Zh, "nginx: worker process").as_deref(), Some("Web 服务器 / 反向代理"));
        assert_eq!(proc_kind(Locale::Zh, "python3 server.py").as_deref(), Some("Python 程序"));
        assert_eq!(proc_kind(Locale::En, "/usr/sbin/sshd -D").as_deref(), Some("SSH server"));
        assert_eq!(proc_kind(Locale::Zh, "containerd").as_deref(), Some("容器运行时"));
        assert_eq!(proc_kind(Locale::Zh, "totally-unknown-bin --x"), None);
    }

    /// STAT 首字母解读 + 原文保留。
    #[test]
    fn stat_words() {
        assert_eq!(stat_word(Locale::Zh, "R"), "运行 (R)");
        assert_eq!(stat_word(Locale::En, "Ssl"), "Sleeping (Ssl)");
        assert_eq!(stat_word(Locale::Zh, "Z"), "僵尸 (Z)");
        assert_eq!(stat_word(Locale::Zh, "?"), "?");
    }

    /// URL 编码：保留字母数字与 -_.，其余百分号转义。
    #[test]
    fn urlencoding() {
        assert_eq!(urlencode("nginx-2.0_worker"), "nginx-2.0_worker");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }

    /// 搜索引擎 URL 规则：google/bing 查询参数 q、baidu wd。
    #[test]
    fn search_engine_urls() {
        assert_eq!(
            SearchEngine::Google.search_url("what is nginx"),
            "https://www.google.com/search?q=what%20is%20nginx"
        );
        assert_eq!(
            SearchEngine::Baidu.search_url("what is nginx"),
            "https://www.baidu.com/s?wd=what%20is%20nginx"
        );
        assert_eq!(
            SearchEngine::Bing.search_url("nginx 进程"),
            "https://www.bing.com/search?q=nginx%20%E8%BF%9B%E7%A8%8B"
        );
        // 存取往返：脏数据回退 Google。
        assert_eq!(SearchEngine::parse("baidu"), SearchEngine::Baidu);
        assert_eq!(SearchEngine::parse("bing"), SearchEngine::Bing);
        assert_eq!(SearchEngine::parse("garbage"), SearchEngine::Google);
        assert_eq!(SearchEngine::Baidu.as_str(), "baidu");
    }

    /// 详情区高度上限钳制：窗口过矮时不低于下限，正常时 = 面板高 − 固定开销。
    #[test]
    fn detail_height_clamps() {
        let normal = procs_detail_max_h(800.);
        assert!((normal - (800. - 30. - HDR_H - 5. - 160.)).abs() < 0.01);
        // 极矮窗口 → 钳到下限。
        assert_eq!(procs_detail_max_h(100.), DETAIL_MIN_H);
    }
}
