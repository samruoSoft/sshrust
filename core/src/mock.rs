//! 进程内 russh mock 服务器：Phase 0 集成测试与 app 演示模式共用。
//!
//! 行为：test/test 密码认证 → pty → shell，可打印字符回显，
//! 回车时回 `echo: <line>`，`exit` 或 Ctrl-D 关闭连接。
//!
//! 目录面板链路支持：`cd <dir>` 更新模拟 cwd 并回发 OSC 7；
//! exec 通道支持 `pwd` / `ls -la -- <path>`（固化输出）与系统信息探针
//! （`@@` 分节命令，合成随时间波动的 /proc 计数器）。
//!
//! SFTP 支持：`sftp` 子系统通道交给 russh-sftp server（内存文件系统，
//! canned 只读层 + 动态写入层，跨连接共享——上传后 ls 可见、可下载回读）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use russh::keys::ssh_key::Algorithm;
use russh::server::{self, Auth, Session as ServerSession};
use russh::ChannelId;
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};

pub const MOCK_USER: &str = "test";
pub const MOCK_PASS: &str = "test";

const BANNER: &[u8] = b"Welcome to sshrust mock shell\r\n(user: test / password: test)\r\n> ";

/// mock 初始 cwd（登录即 home）。
const MOCK_HOME: &str = "/home/test";

pub struct MockServer {
    pub port: u16,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl MockServer {
    pub fn stop(self) {
        let _ = self.shutdown.send(true);
    }
}

/// 在 127.0.0.1 随机端口启动 mock 服务器（不阻塞）。
pub async fn spawn() -> anyhow::Result<MockServer> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();

    let key = russh::keys::PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    let config = Arc::new(server::Config {
        keys: vec![key],
        // 测试场景不做恒定时间延迟。
        auth_rejection_time: Duration::from_secs(0),
        ..Default::default()
    });

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let config = config.clone();
                        tokio::spawn(async move {
                            let handler = MockShellHandler::default();
                            match server::run_stream(config, stream, handler).await {
                                Ok(running) => {
                                    let _ = running.await;
                                }
                                Err(_) => {}
                            }
                        });
                    }
                    Err(_) => break,
                },
            }
        }
    });

    Ok(MockServer { port, shutdown: shutdown_tx })
}

/// 每连接一个 handler，持有 shell 行缓冲与通道表。
#[derive(Default)]
struct MockShellHandler {
    shell: Option<ShellState>,
    /// channel_open 时登记的通道对象：subsystem(sftp) 要消费它的 ChannelStream；
    /// shell/exec 通道在对应请求到达时移除（数据走 Handler::data 回调）。
    channels: HashMap<ChannelId, russh::Channel<russh::server::Msg>>,
    /// 已交给 russh-sftp server 的通道：Handler::data 需让路（数据经 ChannelStream 投递）。
    sftp_channels: HashSet<ChannelId>,
}

struct ShellState {
    line: Vec<u8>,
    /// 模拟 cwd：`cd` 移动，随提示符以 OSC 7 回发（目录面板同步链路）。
    cwd: String,
}

impl server::Handler for MockShellHandler {
    type Error = russh::Error;

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        // 登记通道对象：sftp 子系统要用它的 ChannelStream；
        // shell/exec 路径在各自请求到达时移除（数据走 data 回调）。
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == MOCK_USER && password == MOCK_PASS {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    /// 公钥认证：test 用户接受任意密钥（客户端解码链路的联调用）。
    async fn auth_publickey(
        &mut self,
        user: &str,
        _public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        if user == MOCK_USER {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        session.channel_success(channel)?;
        session.data(channel, BANNER)?;
        self.shell = Some(ShellState { line: Vec::new(), cwd: MOCK_HOME.into() });
        Ok(())
    }

    /// sftp 子系统：通道对象转 ChannelStream 交给 russh-sftp server（内部自起任务）。
    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            if let Some(ch) = self.channels.remove(&channel) {
                self.sftp_channels.insert(channel);
                session.channel_success(channel)?;
                russh_sftp::server::run(ch.into_stream(), MockSftpHandler::default()).await;
                return Ok(());
            }
        }
        session.channel_failure(channel)?;
        Ok(())
    }

    /// exec 通道：目录面板取 cwd / 清单用（pwd / ls -la -- <path>）。
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        session.channel_success(channel)?;
        let command = String::from_utf8_lossy(data).into_owned();
        session.data(channel, exec_output(&command).into_bytes())?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        // sftp 通道：数据同时投递给 ChannelStream 与本回调，让路避免被当 shell 输入。
        if self.sftp_channels.contains(&channel) {
            return Ok(());
        }
        let Some(shell) = &mut self.shell else {
            return Ok(());
        };
        for &b in data {
            match b {
                b'\r' | b'\n' => {
                    let line = String::from_utf8_lossy(&shell.line).into_owned();
                    shell.line.clear();
                    if line == "exit" {
                        session.data(channel, b"bye\r\n".to_vec())?;
                        session.eof(channel)?;
                        session.close(channel)?;
                        return Ok(());
                    }
                    // cd：移动模拟 cwd，提示符前回发 OSC 7（与注入的
                    // PROMPT_COMMAND 语义一致；其余命令 cwd 不变不回发）。
                    let resp = if let Some(arg) = line.strip_prefix("cd ") {
                        shell.cwd = resolve_dir(&shell.cwd, arg.trim());
                        format!("\x1b]7;file://mock{}\x1b\\> ", shell.cwd)
                    } else {
                        format!("echo: {line}\r\n> ")
                    };
                    session.data(channel, resp.into_bytes())?;
                }
                0x7f => {
                    // 退格：终端惯例 \b 空格 \b
                    if shell.line.pop().is_some() {
                        session.data(channel, &b"\x08 \x08"[..])?;
                    }
                }
                0x04 => {
                    // Ctrl-D
                    session.data(channel, &b"bye\r\n"[..])?;
                    session.eof(channel)?;
                    session.close(channel)?;
                    return Ok(());
                }
                0x20..=0x7e => {
                    shell.line.push(b);
                    session.data(channel, vec![b])?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// 供集成测试与 app 演示使用：返回 (端口, 用户名, 密码)。
pub async fn spawn_with_creds() -> anyhow::Result<(MockServer, &'static str, &'static str)> {
    let server = spawn().await?;
    Ok((server, MOCK_USER, MOCK_PASS))
}

// ---------- 目录面板演示链路（exec 固化输出 + cd 路径解析） ----------

/// exec 命令 → 固化输出（真实服务器上由 shell 执行；mock 只认 pwd / ls -la / 系统信息探针）。
fn exec_output(command: &str) -> String {
    let command = command.trim();
    if command == "pwd" {
        return format!("{MOCK_HOME}\n");
    }
    // 进程探针（@@ps1/@@ps2 双拍）：合成进程表，cputimes 随墙钟递增。
    if command.contains("@@ps1") {
        return procs_probe_output();
    }
    // 系统信息探针（分节标记 @@）：合成随时间变化的计数器，演示面板动起来。
    if command.contains("@@") {
        return probe_output();
    }
    // LC_ALL=C ls -la -- <path>
    let path = command.split("-- ").nth(1).map(str::trim).unwrap_or("");
    listing_for(path)
}

/// 系统信息探针的合成输出：/proc 计数器按当前时间生成（两次采样可差分出
/// 波动的 CPU% 与上下行速率），静态字段为一台 4 核 16G Ubuntu 的 plausible 值。
fn probe_output() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    // 首次调用锚定当前时刻：t 从 137 起步（uptime 好看），此后随墙钟单调递增。
    static T0: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut t0 = T0.load(Ordering::Relaxed);
    if t0 == 0 || t0 > now {
        T0.store(now, Ordering::Relaxed);
        t0 = now;
    }
    let t = (now - t0) as f64 + 137.;

    let hz = 100.;
    // 第 i 核占用率随时间正弦波动（3%~97%），总体取核 0 的波动叠加均值。
    let core_pct = |i: f64| (28. + 24. * (t / (7. + i) + i).sin()).clamp(3., 97.);
    let total_pct = core_pct(0.) * 0.6 + core_pct(2.) * 0.4;
    // 计数器（tick）：忙/闲按占比分摊，单调递增（速率恒正）。
    let counters = |pct: f64| -> (u64, u64) {
        let busy = t * hz * pct / 100.;
        let idle = t * hz * (100. - pct) / 100.;
        (busy as u64, idle as u64)
    };
    let (busy, idle) = counters(total_pct);

    let mut out = String::new();
    out.push_str("@@host\nmock-server\n@@ip\n127.0.0.1 172.17.0.2\n");
    out.push_str("@@os\nUbuntu 22.04.4 LTS\n@@kernel\n6.8.0-45-generic\n");
    out.push_str("@@tz\n/usr/share/zoneinfo/Asia/Shanghai\n@@tzoff\n+0800\n");
    out.push_str(&format!("@@uptime\n{t:.2} 999999.99\n"));
    out.push_str("@@cpumodel\n Mock Cloud CPU (4) @ 2.60GHz\n@@stat\n");
    out.push_str(&format!(
        "cpu  {:.0} {:.0} {:.0} {:.0} 300 100 90 20\n",
        busy * 72 / 100,
        busy * 3 / 100,
        busy * 25 / 100,
        idle - 300
    ));
    for i in 0..4u32 {
        let (b, idl) = counters(core_pct(i as f64));
        out.push_str(&format!(
            "cpu{i} {:.0} {:.0} {:.0} {:.0} 75 25 22 5\n",
            b * 72 / 100,
            b * 3 / 100,
            b * 25 / 100,
            idl - 75
        ));
    }
    out.push_str(concat!(
        "@@mem\n",
        "MemTotal:       15864708 kB\n",
        "MemFree:         613380 kB\n",
        "MemAvailable:   6029432 kB\n",
        "Buffers:         512000 kB\n",
        "Cached:         9216000 kB\n",
        "SReclaimable:    409600 kB\n",
    ));
    // 网速：下行 ~148 KB/s、上行 ~50 KB/s，叠加大周期正弦（导数为正保持单调）。
    let rx = 2_000_000. + t * 151_552. + 30_720. * (t / 5.).sin();
    let tx = 1_000_000. + t * 51_200. + 20_480. * (t / 4. + 2.).sin();
    out.push_str(concat!(
        "@@net\n",
        "Inter-|   Receive                                                |  Transmit\n",
        " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
    ));
    out.push_str(&format!(
        "    lo: 999999 999 0 0 0 0 0 0 999999 999 0 0 0 0 0 0\n  eth0: {rx:.0} 5210 0 0 0 0 0 0 {tx:.0} 3344 0 0 0 0 0 0\n",
    ));
    out.push_str("@@df\n/dev/vda1 42157981696 12863488000 27176161280 32% /\n@@end\n");
    out
}

/// 进程探针（`@@ps1`/`@@ps2` 双拍）的合成输出：每进程一个固定 CPU 速率
/// + 有界慢正弦，cputimes 随墙钟递增 → 客户端双拍差分出波动的瞬时 CPU%；
/// 两拍时间戳相差 1.5s（与 cputimes 增量同口径）。列布局与 app 侧全量
/// 探针一致（11 列，表头驱动解析）。
fn procs_probe_output() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    // (pid, ppid, user, pmem, rss_kb, nlwp, stat, args, cpu 速率%——多核可超 100)。
    const MOCK_PROCS: &[(u32, u32, &str, f32, u64, u32, &str, &str, f64)] = &[
        (1, 0, "root", 0.1, 12340, 1, "Ss", "/sbin/init splash", 0.3),
        (2, 0, "root", 0.0, 0, 1, "S", "[kthreadd]", 0.0),
        (14, 2, "root", 0.0, 0, 1, "I<", "[kworker/0:1H]", 0.1),
        (287, 1, "root", 0.2, 9876, 1, "Ss", "/lib/systemd/systemd-journald", 1.2),
        (331, 1, "root", 0.1, 5432, 1, "Ss", "/lib/systemd/systemd-udevd", 0.4),
        (512, 1, "root", 0.3, 22340, 1, "Ss", "/usr/sbin/sshd -D", 0.2),
        (640, 1, "root", 0.8, 41200, 8, "Ssl", "/usr/bin/dockerd -H fd://", 3.5),
        (655, 1, "root", 0.4, 18900, 9, "Ssl", "containerd", 1.1),
        (733, 1, "mysql", 12.6, 199800, 32, "Sl", "/usr/sbin/mysqld", 22.0),
        (811, 1, "www-data", 3.1, 52100, 1, "S", "nginx: worker process", 8.5),
        (812, 1, "www-data", 2.8, 51800, 1, "S", "nginx: worker process", 7.2),
        (920, 1, "root", 1.2, 30400, 3, "Ss", "python3 /opt/app/server.py --port 8080", 65.0),
        (997, 1, "deploy", 4.5, 71200, 12, "Sl", "node /srv/web/dist/server.js", 118.0),
        (1104, 512, "deploy", 0.5, 6100, 1, "Ss", "-bash", 0.0),
        (1188, 1104, "deploy", 0.3, 5400, 1, "R+", "ps -eo pid,ppid,user:20,pcpu,pmem,rss,nlwp,cputimes,etime,stat,args", 2.0),
        (1240, 1, "root", 0.6, 15600, 1, "Ss", "/usr/sbin/cron -f", 0.1),
        (1301, 1, "syslog", 0.2, 9800, 4, "Ss", "/usr/sbin/rsyslogd -n", 0.5),
        (1500, 1, "redis", 1.9, 28700, 6, "Ssl", "redis-server 127.0.0.1:6379", 4.4),
        (2049, 1, "root", 0.1, 4300, 1, "Ss", "/sbin/dhclient", 0.0),
        (2311, 1, "prometheus", 2.2, 66400, 14, "Sl", "/opt/prometheus/prometheus", 12.7),
        (2600, 1, "grafana", 5.8, 92100, 18, "Sl", "/usr/share/grafana/bin/grafana server", 9.3),
        (3155, 1, "root", 0.2, 7200, 1, "Ss", "/usr/sbin/atd -f", 0.0),
        (4200, 1104, "deploy", 0.9, 24600, 2, "SN", "python3 backup.py --daily", 3.1),
        (5150, 2, "root", 0.0, 0, 1, "I", "[kworker/u256:2]", 0.2),
        (7742, 1, "deploy", 6.3, 88300, 44, "Sl", "java -jar /opt/search/service.jar", 155.0),
        (8096, 512, "deploy", 0.4, 8200, 1, "Ss", "sshd: deploy@pts/0", 0.1),
        (8234, 512, "deploy", 0.4, 8100, 1, "Ss", "sshd: deploy@notty", 0.1),
        (9971, 1, "root", 0.1, 4100, 1, "Ss", "/lib/systemd/systemd --user", 0.1),
    ];
    // cputimes(i, t)：基准随序号错开 + 线性累积项（t·rate，严格单调、差分
    // 即 rate）+ 有界慢正弦（±40s 演示波动；波动只能加性——乘进线性项系数
    // 会被大 t 放大成百万秒级跳变，破坏单调性）。
    let cpu_at = |i: usize, t: f64| -> f64 {
        let (_, _, _, _, _, _, _, _, rate) = MOCK_PROCS[i];
        50. + i as f64 * 31. + t * rate + 40. * (t / (300. + i as f64)).sin()
    };
    // ELAPSED：每进程错开的固定运行时长（1.5s 采样间隔内在分钟级显示下
    // 不变，固定值即真实；不做随 t 递增以避免取模跨界跳变）。
    let etime_at = |i: usize| fmt_etime(i as f64 * 2637. + 137.);
    let snap = |t: f64| -> String {
        let mut s = String::from(
            "  PID  PPID USER                %CPU %MEM    RSS NLWP CPUTIMES ELAPSED STAT COMMAND\n",
        );
        for (i, (pid, ppid, user, pmem, rss, nlwp, stat, args, _)) in MOCK_PROCS.iter().enumerate() {
            // pcpu（生命周期平均）对差分无意义，给固定小值即可。
            s.push_str(&format!(
                "{pid:>5} {ppid:>5} {user:<20} {pcpu:>4} {pmem:>4} {rss:>7} {nlwp:>5} {ct:>10.2} {el:>12} {stat:<4} {args}\n",
                pcpu = 0.0,
                ct = cpu_at(i, t),
                el = etime_at(i),
            ));
        }
        s
    };
    format!(
        "@@t1\n{now}\n@@ps1\n{}@@t2\n{t2}\n@@ps2\n{}@@end\n",
        snap(now - 1.5),
        snap(now),
        t2 = now + 1.5,
    )
}

/// 秒 → procps ELAPSED 形态 `[[dd-]hh:]mm:ss`（合成输出用）。
fn fmt_etime(secs: f64) -> String {
    let s = secs.max(0.) as u64;
    let (d, r) = (s / 86400, s % 86400);
    let (h, r) = (r / 3600, r % 3600);
    let (m, ss) = (r / 60, r % 60);
    if d > 0 {
        format!("{d}-{h:02}:{m:02}:{ss:02}")
    } else if h > 0 {
        format!("{h:02}:{m:02}:{ss:02}")
    } else {
        format!("{m:02}:{ss:02}")
    }
}

// ---------- mock 文件系统（canned 只读层 + 动态写入层；ls 与 SFTP 同源） ----------

/// canned 目录项：名字 + 目录/符号链接标记（文件大小按内容长度）。
#[derive(Clone, Copy)]
struct CE {
    name: &'static str,
    dir: bool,
    link: Option<&'static str>,
}

const fn d(name: &'static str) -> CE {
    CE { name, dir: true, link: None }
}
const fn f(name: &'static str) -> CE {
    CE { name, dir: false, link: None }
}
const fn l(name: &'static str, target: &'static str) -> CE {
    CE { name, dir: false, link: Some(target) }
}

const ROOT_ENTRIES: &[CE] = &[
    d(".Recycle_bin"),
    l("bin", "usr/bin"),
    d("boot"),
    d("cdrom"),
    d("dev"),
    d("etc"),
    d("home"),
    l("lib", "usr/lib"),
    d("opt"),
    d("proc"),
    d("root"),
    d("run"),
    l("sbin", "usr/sbin"),
    d("srv"),
    d("sys"),
    d("tmp"),
    d("usr"),
    d("var"),
];
const HOME_ENTRIES: &[CE] = &[d("projects"), f(".bashrc"), f("notes.txt"), l("latest", "projects")];
const PROJECTS_ENTRIES: &[CE] = &[d("sshrust"), f("README.md")];
const SSHRUST_ENTRIES: &[CE] = &[f("Cargo.toml"), d("src")];

fn canned_dir(path: &str) -> Option<&'static [CE]> {
    match path {
        "/" => Some(ROOT_ENTRIES),
        MOCK_HOME => Some(HOME_ENTRIES),
        "/home/test/projects" => Some(PROJECTS_ENTRIES),
        "/home/test/projects/sshrust" => Some(SSHRUST_ENTRIES),
        _ => None,
    }
}

/// canned 文件内容（SFTP 下载演示与 stat 大小来源）。
fn canned_file(path: &str) -> Option<&'static str> {
    match path {
        "/home/test/notes.txt" => Some(concat!(
            "mock notes for sshrust download demo\n",
            "the quick brown fox jumps over the lazy dog\n",
            "0123456789 ABCDEF\n",
            "tail line for byte alignment\n",
        )),
        "/home/test/.bashrc" => Some(concat!(
            "# ~/.bashrc: sshrust mock shell init\n",
            "# 输入任意命令回显；cd 移动模拟目录并回发 OSC 7。\n",
            "export PS1='> '\n",
            "export TERM=xterm-256color\n",
            "alias ll='ls -alF'\n",
            "alias la='ls -A'\n",
            "alias l='ls -CF'\n",
        )),
        "/home/test/projects/README.md" => Some(concat!(
            "# sshrust mock project\n",
            "\n",
            "演示用固化 README：目录面板/SFTP 传输链路的下载目标。\n",
            "\n",
            "- 多标签终端\n",
            "- 远程目录同步\n",
            "- 系统信息监控\n",
            "- 进程面板\n",
            "- SFTP 文件传输\n",
        )),
        "/home/test/projects/sshrust/Cargo.toml" => Some(concat!(
            "[package]\n",
            "name = \"mock-demo\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2021\"\n",
            "\n",
            "[dependencies]\n",
            "russh = \"0.64.1\"\n",
            "tokio = { version = \"1\", features = [\"full\"] }\n",
        )),
        _ => None,
    }
}

/// 动态写入层：上传/写入的文件、mkdir 的目录、对 canned 的删除遮罩。
/// 全局共享（跨连接），与 T0 探针锚定同款模式——演示模式上传后 ls 可见。
#[derive(Default)]
struct DynFs {
    files: HashMap<String, Vec<u8>>,
    dirs: HashSet<String>,
    hidden: HashSet<String>,
}

static DYN_FS: std::sync::OnceLock<std::sync::Mutex<DynFs>> = std::sync::OnceLock::new();

fn dyn_fs() -> &'static std::sync::Mutex<DynFs> {
    DYN_FS.get_or_init(|| std::sync::Mutex::new(DynFs::default()))
}

fn join_path(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 目录的直接子项（canned 未遮罩 + 动态层）；非目录返回 None。
fn mock_entries(path: &str) -> Option<Vec<MockEntry>> {
    let dyns = dyn_fs().lock().unwrap();
    if canned_dir(path).is_none() && !dyns.dirs.contains(path) {
        return None;
    }
    let mut out = Vec::new();
    if let Some(ces) = canned_dir(path) {
        for ce in ces {
            let full = join_path(path, ce.name);
            if dyns.hidden.contains(&full) {
                continue;
            }
            let size = match ce.link {
                Some(t) => t.len() as u64,
                None if ce.dir => 4096,
                None => canned_file(&full).map(|s| s.len() as u64).unwrap_or(0),
            };
            out.push(MockEntry {
                name: ce.name.to_string(),
                dir: ce.dir,
                link: ce.link.map(str::to_string),
                size,
            });
        }
    }
    for (p, content) in &dyns.files {
        if parent_of(p).as_deref() == Some(path) {
            out.push(MockEntry {
                name: basename_of(p),
                dir: false,
                link: None,
                size: content.len() as u64,
            });
        }
    }
    for p in &dyns.dirs {
        if parent_of(p).as_deref() == Some(path) {
            out.push(MockEntry {
                name: basename_of(p),
                dir: true,
                link: None,
                size: 4096,
            });
        }
    }
    Some(out)
}

struct MockEntry {
    name: String,
    dir: bool,
    link: Option<String>,
    size: u64,
}

fn parent_of(path: &str) -> Option<String> {
    path.rsplit_once('/').map(|(p, _)| if p.is_empty() { "/".to_string() } else { p.to_string() })
}

fn basename_of(path: &str) -> String {
    path.rsplit_once('/').map(|(_, n)| n.to_string()).unwrap_or_else(|| path.to_string())
}

fn mock_read(path: &str) -> Option<Vec<u8>> {
    let dyns = dyn_fs().lock().unwrap();
    if dyns.hidden.contains(path) {
        return None;
    }
    if let Some(v) = dyns.files.get(path) {
        return Some(v.clone());
    }
    drop(dyns);
    canned_file(path).map(|s| s.as_bytes().to_vec())
}

fn mock_is_dir(path: &str) -> bool {
    if canned_dir(path).is_some() {
        return true;
    }
    dyn_fs().lock().unwrap().dirs.contains(path)
}

fn mock_link(path: &str) -> Option<String> {
    let name = basename_of(path);
    let parent = parent_of(path)?;
    canned_dir(&parent)?.iter().find(|ce| ce.name == name).and_then(|ce| ce.link.map(str::to_string))
}

fn mock_exists(path: &str) -> bool {
    mock_is_dir(path) || mock_read(path).is_some() || mock_link(path).is_some()
}

/// 固化的 `ls -la` 输出（列格式与真实 ls 一致，供 app 解析链路复用）；
/// 与 SFTP readdir 同源（mock_entries），上传的动态文件随取随见。
fn listing_for(path: &str) -> String {
    let Some(entries) = mock_entries(path) else {
        return "total 0\n".into();
    };
    let mut out = format!("total {}\n", entries.len() * 4);
    out.push_str("drwxr-xr-x 1 test test     4096 Oct 14 09:00 .\n");
    out.push_str("drwxr-xr-x 2 test test     4096 Oct 14 08:00 ..\n");
    for e in entries {
        let line = match &e.link {
            Some(t) => format!(
                "lrwxrwxrwx 1 test test {:>7} Oct 14 09:00 {} -> {}\n",
                t.len(),
                e.name,
                t
            ),
            None if e.dir => format!("drwxr-xr-x 1 test test {:>7} Oct 14 09:00 {}\n", e.size, e.name),
            None => format!("-rw-r--r-- 1 test test {:>7} Oct 14 09:00 {}\n", e.size, e.name),
        };
        out.push_str(&line);
    }
    out
}

// ---------- SFTP server（内存 FS 后端） ----------

/// 每条 sftp 子系统连接一个实例：句柄表 + mock FS 统一视图。
#[derive(Default)]
struct MockSftpHandler {
    /// handle → 打开的文件路径。
    files: HashMap<String, String>,
    /// handle → 目录路径（readdir 单批：首批返回、次批 EOF）。
    dirs: HashMap<String, String>,
    next: u32,
}

impl MockSftpHandler {
    fn ok(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        }
    }

    fn attrs_file(size: u64) -> FileAttributes {
        FileAttributes {
            size: Some(size),
            permissions: Some(0o100644),
            user: Some("test".to_string()),
            group: Some("test".to_string()),
            ..Default::default()
        }
    }

    fn attrs_dir() -> FileAttributes {
        FileAttributes {
            size: Some(4096),
            permissions: Some(0o040755),
            user: Some("test".to_string()),
            group: Some("test".to_string()),
            ..Default::default()
        }
    }

    fn attrs_link(target_len: usize) -> FileAttributes {
        FileAttributes {
            size: Some(target_len as u64),
            permissions: Some(0o120777),
            user: Some("test".to_string()),
            group: Some("test".to_string()),
            ..Default::default()
        }
    }

    fn attrs_of(path: &str) -> Option<FileAttributes> {
        if let Some(t) = mock_link(path) {
            return Some(Self::attrs_link(t.len()));
        }
        if mock_is_dir(path) {
            return Some(Self::attrs_dir());
        }
        mock_read(path).map(|v| Self::attrs_file(v.len() as u64))
    }
}

impl russh_sftp::server::Handler for MockSftpHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        if pflags.contains(OpenFlags::WRITE) {
            let mut dyns = dyn_fs().lock().unwrap();
            dyns.hidden.remove(&filename);
            let entry = dyns
                .files
                .entry(filename.clone())
                .or_insert_with(|| canned_file(&filename).map(|s| s.as_bytes().to_vec()).unwrap_or_default());
            if pflags.contains(OpenFlags::TRUNCATE) {
                entry.clear();
            }
        } else if !mock_exists(&filename) {
            return Err(StatusCode::NoSuchFile);
        }
        self.next += 1;
        let handle = format!("mock-file-{}", self.next);
        self.files.insert(handle.clone(), filename);
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(path) = self.files.get(&handle).cloned() else {
            return Err(StatusCode::Failure);
        };
        let data = mock_read(&path).ok_or(StatusCode::NoSuchFile)?;
        let off = offset as usize;
        if off >= data.len() {
            return Err(StatusCode::Eof);
        }
        let end = (off + len as usize).min(data.len());
        Ok(Data { id, data: data[off..end].to_vec() })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(path) = self.files.get(&handle).cloned() else {
            return Err(StatusCode::Failure);
        };
        let mut dyns = dyn_fs().lock().unwrap();
        dyns.hidden.remove(&path);
        let file = dyns
            .files
            .entry(path.clone())
            .or_insert_with(|| canned_file(&path).map(|s| s.as_bytes().to_vec()).unwrap_or_default());
        let off = offset as usize;
        let end = off + data.len();
        if file.len() < end {
            file.resize(end, 0);
        }
        file[off..end].copy_from_slice(&data);
        Ok(Self::ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.files.remove(&handle);
        self.dirs.remove(&handle);
        Ok(Self::ok(id))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(path) = self.files.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        let size = mock_read(path).map(|v| v.len()).unwrap_or(0);
        Ok(Attrs { id, attrs: Self::attrs_file(size as u64) })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        if !mock_is_dir(&path) {
            return Err(StatusCode::NoSuchFile);
        }
        self.next += 1;
        let handle = format!("mock-dir-{}", self.next);
        self.dirs.insert(handle.clone(), path);
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let Some(path) = self.dirs.remove(&handle) else {
            return Err(StatusCode::Failure);
        };
        let entries = mock_entries(&path).ok_or(StatusCode::NoSuchFile)?;
        let files = entries
            .into_iter()
            .map(|e| {
                let attrs = if let Some(t) = &e.link {
                    Self::attrs_link(t.len())
                } else if e.dir {
                    Self::attrs_dir()
                } else {
                    Self::attrs_file(e.size)
                };
                File::new(e.name, attrs)
            })
            .collect();
        Ok(Name { id, files })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let attrs = Self::attrs_of(&path).ok_or(StatusCode::NoSuchFile)?;
        Ok(Attrs { id, attrs })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let attrs = Self::attrs_of(&path).ok_or(StatusCode::NoSuchFile)?;
        Ok(Attrs { id, attrs })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name { id, files: vec![File::dummy(resolve_dir(MOCK_HOME, &path))] })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let mut dyns = dyn_fs().lock().unwrap();
        if dyns.files.remove(&filename).is_some() {
            return Ok(Self::ok(id));
        }
        if canned_file(&filename).is_some() || mock_link(&filename).is_some() {
            dyns.hidden.insert(filename);
            return Ok(Self::ok(id));
        }
        Err(StatusCode::NoSuchFile)
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        if mock_exists(&path) {
            return Err(StatusCode::Failure);
        }
        dyn_fs().lock().unwrap().dirs.insert(path);
        Ok(Self::ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        if dyn_fs().lock().unwrap().dirs.remove(&path) {
            Ok(Self::ok(id))
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let Some(content) = mock_read(&oldpath) else {
            return Err(StatusCode::NoSuchFile);
        };
        let mut dyns = dyn_fs().lock().unwrap();
        dyns.files.insert(newpath, content);
        if dyns.files.remove(&oldpath).is_none() {
            dyns.hidden.insert(oldpath);
        }
        Ok(Self::ok(id))
    }
}

/// 简化路径解析：绝对路径直接归一；相对路径按 base 拼接；处理 `..`/`.`。
fn resolve_dir(base: &str, target: &str) -> String {
    if target.is_empty() || target == "~" {
        return MOCK_HOME.into();
    }
    let segs: Vec<&str> = if target.starts_with('/') {
        target.split('/').collect()
    } else {
        base.split('/').chain(target.split('/')).collect()
    };
    let mut out: Vec<&str> = Vec::new();
    for p in segs.into_iter().filter(|p| !p.is_empty() && *p != ".") {
        if p == ".." {
            out.pop();
        } else {
            out.push(p);
        }
    }
    format!("/{}", out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dir_paths() {
        assert_eq!(resolve_dir("/home/test", "/etc"), "/etc");
        assert_eq!(resolve_dir("/home/test", "projects"), "/home/test/projects");
        assert_eq!(resolve_dir("/home/test/projects", ".."), "/home/test");
        assert_eq!(resolve_dir("/home/test", "../.."), "/");
        assert_eq!(resolve_dir("/home/test", "~"), "/home/test");
        assert_eq!(resolve_dir("/home/test", ""), "/home/test");
    }

    #[test]
    fn exec_output_pwd_and_ls() {
        assert_eq!(exec_output("pwd"), "/home/test\n");
        assert_eq!(exec_output("  pwd  "), "/home/test\n");
        let ls = exec_output("LC_ALL=C ls -la -- /home/test");
        assert!(ls.contains("latest -> projects"), "符号链接行保留：{ls}");
        assert!(ls.starts_with("total "), "total 行保留：{ls}");
        assert_eq!(exec_output("LC_ALL=C ls -la -- /nope"), "total 0\n");
    }

    /// 系统信息探针：分节齐全、lo 排除由解析侧保证、随时间单调（可差分）。
    #[test]
    fn exec_output_probe() {
        let p1 = exec_output("echo @@host; hostname");
        assert!(p1.contains("@@host\nmock-server\n"));
        assert!(p1.contains("eth0: "));
        // 两次调用计数器单调不减（差分不出负速率）。
        let rx = |s: &str| {
            s.lines()
                .find(|l| l.contains("eth0: "))
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u64>()
                .unwrap()
        };
        assert!(rx(&p1) > 0);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let p2 = exec_output("echo @@host; hostname");
        assert!(rx(&p2) >= rx(&p1), "eth0 rx 字节应随时间单调递增");
    }

    /// 进程探针：双拍分节齐全、时间戳 Δt=1.5、同行 cputimes 递增（可差分）、
    /// 11 列表头与详情字段齐备。
    #[test]
    fn exec_output_procs_probe() {
        let cmd = "echo @@t1; date +%s.%N\necho @@ps1; ps -eo pid,ppid,user:20,pcpu,pmem,rss,nlwp,cputimes,etime,stat,args";
        let out = exec_output(cmd);
        assert!(out.starts_with("@@t1\n"), "首节为 t1：{}", &out[..24]);
        assert!(out.contains("@@ps1\n") && out.contains("@@ps2\n") && out.contains("@@end"));
        // 时间戳差 1.5s。
        let t = |name: &str| {
            out.lines()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| w[0] == name)
                .unwrap()[1]
                .parse::<f64>()
                .unwrap()
        };
        let dt = t("@@t2") - t("@@t1");
        assert!((dt - 1.5).abs() < 1e-6, "两拍 Δt 应为 1.5s，实际 {dt}");
        // 按表头名找列（表头驱动）：java 进程（7742）第二拍 cputimes 更大。
        let ps1 = out.split("@@ps2\n").next().unwrap();
        let ps2 = out.split("@@ps2\n").nth(1).unwrap();
        let col_of = |snap: &str, name: &str| -> usize {
            snap.lines()
                .next()
                .unwrap()
                .split_whitespace()
                .position(|h| h == name)
                .unwrap_or_else(|| panic!("表头缺列 {name}"))
        };
        let ct_i = col_of(ps2, "CPUTIMES");
        let ppid_i = col_of(ps2, "PPID");
        let et_i = col_of(ps2, "ELAPSED");
        let field = |snap: &str, pid: u32, i: usize| -> String {
            snap.lines()
                .find(|l| l.split_whitespace().next() == Some(pid.to_string().as_str()))
                .unwrap()
                .split_whitespace()
                .nth(i)
                .unwrap()
                .to_string()
        };
        assert!(field(ps2, 7742, ct_i) > field(ps1, 7742, ct_i), "cputimes 双拍递增");
        assert_eq!(field(ps2, 8096, ppid_i), "512", "sshd 会话行父进程 512");
        assert!(
            field(ps2, 7742, et_i).contains(':'),
            "ELAPSED 带冒号：{}",
            field(ps2, 7742, et_i)
        );
    }
}
