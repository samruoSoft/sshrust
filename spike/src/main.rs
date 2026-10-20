//! Phase 0 spike：russh 交互式 SSH shell 客户端。
//!
//! 用法：
//!   spike <host> [-p port] -u <user> [--password <pw> | --key <path>]
//!   spike --mock   # 进程内启动 mock 服务器并连接（无需真实主机）
//!
//! 本地退出：Ctrl-Q。Ctrl-D / exit 交给远端处理。

mod keys;

use std::io::{BufRead, Read, Write};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use crossterm::{execute, terminal};

use sshrust_core::session::{open_session, AuthMethod, ConnectParams, SessionReader};
use sshrust_core::Transport;

#[derive(Parser, Debug)]
#[command(name = "spike", about = "russh interactive SSH shell (Phase 0 spike)")]
struct Args {
    host: Option<String>,
    #[arg(short, long, default_value_t = 22)]
    port: u16,
    #[arg(short = 'u', long)]
    user: Option<String>,
    #[arg(long)]
    password: Option<String>,
    #[arg(long)]
    key: Option<std::path::PathBuf>,
    /// 连接进程内 mock 服务器（test/test）
    #[arg(long)]
    mock: bool,
    /// 行模式（无 TTY 场景/脚本化测试）：不走 raw 模式，stdin 按行发送
    #[arg(long)]
    no_tty: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // mock 句柄必须存活到 main 结束（drop 即关停服务器）。
    let mock_server = if args.mock {
        Some(runtime.block_on(sshrust_core::mock::spawn())?)
    } else {
        None
    };

    let mut params = match (&args.host, &mock_server) {
        (Some(host), _) => build_params(&args, host.clone())?,
        (None, Some(server)) => ConnectParams {
            host: "127.0.0.1".into(),
            port: server.port,
            username: sshrust_core::mock::MOCK_USER.into(),
            auth: AuthMethod::Password(sshrust_core::mock::MOCK_PASS.into()),
            cols: 80,
            rows: 24,
            transport: Transport::Tcp,
        },
        (None, None) => bail!("缺少 <host>（或使用 --mock）"),
    };
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    params.cols = cols;
    params.rows = rows;

    // 连接阶段不在 raw 模式，保留正常输出与密码提示。
    eprintln!("connecting to {}:{} as {} ...", params.host, params.port, params.username);
    let opened = runtime.block_on(open_session(params)).context("open ssh session")?;
    eprintln!("connected. (本地退出: Ctrl-Q)");

    let session = opened.session;
    let mut reader = opened.reader;

    if args.no_tty {
        return run_line_mode(session, reader);
    }

    enable_raw_mode().context("raw mode")?;
    let _raw_guard = RawGuard;
    execute!(std::io::stdout(), EnterAlternateScreen)?;

    // 输入线程：crossterm 事件 → writer（Ctrl-Q 退出）
    std::thread::Builder::new()
        .name("term-in".into())
        .spawn({
            let session = session.clone();
            move || {
                let writer = session.writer();
                pump_input(writer, session)
            }
        })
        .context("spawn input thread")?;

    // 远端输出线程：reader(阻塞) → stdout；EOF 即会话结束。
    let output = std::thread::Builder::new()
        .name("term-out".into())
        .spawn(move || pump_output(&mut reader))
        .context("spawn output thread")?;

    // 只等远端关闭；输入线程可能在 read() 上阻塞，随进程退出回收。
    let _ = output.join();

    session.close();
    drop(_raw_guard); // 恢复终端
    Ok(())
}

fn build_params(args: &Args, host: String) -> Result<ConnectParams> {
    let user = args.user.clone().unwrap_or_else(|| "root".into());
    let auth = if let Some(path) = &args.key {
        // core 层认证只收私钥内容；文件读取留在 CLI 边界。
        let key = std::fs::read_to_string(path).with_context(|| format!("read key {}", path.display()))?;
        AuthMethod::Key { key, passphrase: None }
    } else if let Some(pw) = &args.password {
        AuthMethod::Password(pw.clone())
    } else {
        let pw = rpassword::prompt_password(format!("password for {user}: "))?;
        AuthMethod::Password(pw)
    };
    Ok(ConnectParams { host, port: args.port, username: user, auth, cols: 80, rows: 24, transport: Transport::Tcp })
}

fn pump_output(reader: &mut SessionReader) {
    let mut stdout = std::io::stdout();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stdout.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = stdout.flush();
            }
        }
    }
}

fn pump_input(mut writer: sshrust_core::SessionWriter, session: sshrust_core::Session) {
    loop {
        let Ok(ev) = crossterm::event::read() else { break };
        match ev {
            Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL, .. }) => {
                session.close();
                break;
            }
            Event::Key(key) => {
                if let Some(bytes) = keys::encode_key(&key) {
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                }
            }
            Event::Resize(cols, rows) => session.resize(cols, rows),
            _ => {}
        }
    }
}

/// 行模式：stdin 按行 → writer；stdout 持续打印远端输出；stdin EOF 后等远端
/// 排空输出（最多 5s），超时才强制关闭。
fn run_line_mode(session: sshrust_core::Session, mut reader: SessionReader) -> Result<()> {
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let output = std::thread::Builder::new()
        .name("term-out".into())
        .spawn(move || {
            let mut stdout = std::io::stdout();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if stdout.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = stdout.flush();
                    }
                }
            }
            let _ = done_tx.send(());
        })
        .context("spawn output thread")?;

    let mut writer = session.writer();
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        writer
            .write_all(format!("{line}\r").as_bytes())
            .context("write to remote")?;
    }
    // stdin 结束≠远端结束：给回包留时间，超时强制关。
    if done_rx.recv_timeout(Duration::from_secs(5)).is_err() {
        session.close();
    }
    let _ = output.join();
    Ok(())
}

/// RAII：任何退出路径都恢复 cooked 模式并离开备用屏幕。
struct RawGuard;

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}
