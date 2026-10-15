//! SSH 会话管理：连接、认证、PTY shell 通道、双向字节流、窗口尺寸同步。
//!
//! 对外暴露 [`Session`]（廉价克隆的句柄）与阻塞式的 [`SessionReader`]/[`SessionWriter`]，
//! 便于对接 CLI 的 stdin/stdout 或 gpui_terminal 的 Read/Write 约定。
//! reader/events 通道在 [`open_session`] 内预创建，避免“连接建立后、订阅前”丢字节。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use russh::client::{self, Handle};
use russh::keys::PrivateKeyWithHashAlg;

const TERM: &str = "xterm-256color";

#[derive(Debug, Clone)]
pub enum AuthMethod {
    Password(String),
    /// `key` 为私钥文本内容（PEM / OpenSSH 格式），不是文件路径。
    Key { key: String, passphrase: Option<String> },
}

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("connect {host}:{port}: {source}")]
    Connect { host: String, port: u16, source: russh::Error },
    #[error("auth failed for {0}")]
    AuthFailed(String),
    #[error("parse private key: {source}")]
    ParseKey { source: russh::keys::Error },
    #[error("ssh: {0}")]
    Ssh(#[from] russh::Error),
}

/// 会话侧发给 supervisor 的指令。
enum Cmd {
    Write(Vec<u8>),
    Resize(u16, u16),
    Shutdown,
    /// 在独立通道执行一次性命令（目录清单等），结果经 oneshot 回传。
    Exec { cmd: String, reply: oneshot::Sender<Result<String, String>> },
}

/// 会话生命周期事件。
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Closed { reason: String },
}

/// 双向字节计数（右侧状态面板展示用）。
#[derive(Clone, Default)]
pub struct SessionStats {
    inner: std::sync::Arc<StatsInner>,
}

#[derive(Default)]
struct StatsInner {
    bytes_in: std::sync::atomic::AtomicU64,
    bytes_out: std::sync::atomic::AtomicU64,
}

impl SessionStats {
    /// (下行, 上行)：in = 远端→本地，out = 本地→远端。
    pub fn bytes(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (
            self.inner.bytes_in.load(Relaxed),
            self.inner.bytes_out.load(Relaxed),
        )
    }

    fn add_in(&self, n: usize) {
        self.inner.bytes_in.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
    }

    fn add_out(&self, n: usize) {
        self.inner.bytes_out.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

/// open_session 的完整产物：句柄 + 预接好的读流/事件流。
pub struct OpenedSession {
    pub session: Session,
    pub reader: SessionReader,
    pub events: mpsc::UnboundedReceiver<SessionEvent>,
}

struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Phase 1: 无条件信任；Phase 2 引入 known_hosts/指纹确认。
        Ok(true)
    }
}

/// 一条已建立的 SSH shell 会话。克隆廉价；Drop 句柄不主动关连接。
#[derive(Clone)]
pub struct Session {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    pub params: ConnectParams,
    pub stats: SessionStats,
}

impl Session {
    /// 同步写入（键盘输入路径），转发给 tokio supervisor。
    pub fn writer(&self) -> SessionWriter {
        SessionWriter {
            cmd_tx: self.cmd_tx.clone(),
            stats: self.stats.clone(),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.cmd_tx.send(Cmd::Resize(cols, rows));
    }

    /// 在同一连接上开独立 exec 通道执行一次性命令，返回其 stdout。
    /// 退出码非 0、通道失败或会话已关 → Err（错误信息适合直接展示）。
    pub async fn exec_output(&self, cmd: impl Into<String>) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Exec { cmd: cmd.into(), reply: tx })
            .map_err(|_| "session closed".to_string())?;
        rx.await.map_err(|_| "session closed".to_string())?
    }

    pub fn close(&self) {
        let _ = self.cmd_tx.send(Cmd::Shutdown);
    }
}

/// 阻塞式读取远端输出：由 gpui_terminal 的后台线程或 CLI 输出循环调用。
pub struct SessionReader {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
}

impl std::io::Read for SessionReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.rx.recv() {
            Ok(chunk) => {
                let n = chunk.len().min(buf.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                // 终端场景下单块远小于常用读缓冲；装不下时丢弃余量并留痕。
                if n < chunk.len() {
                    eprintln!("[sshrust] reader chunk truncated ({}/{})", n, chunk.len());
                }
                Ok(n)
            }
            Err(_) => Ok(0), // 所有 sender 释放 = EOF
        }
    }
}

/// 同步写入：转发给 tokio supervisor。
pub struct SessionWriter {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    stats: SessionStats,
}

impl std::io::Write for SessionWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.cmd_tx
            .send(Cmd::Write(buf.to_vec()))
            .map_err(|_| std::io::Error::other("session closed"))?;
        self.stats.add_out(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 建立 TCP + SSH 连接，完成认证，打开 pty shell 通道，启动 supervisor。
pub async fn open_session(params: ConnectParams) -> Result<OpenedSession, SessionError> {
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(600)),
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });

    let mut handle: Handle<ClientHandler> = client::connect(
        config,
        (params.host.as_str(), params.port),
        ClientHandler,
    )
    .await
    .map_err(|e| SessionError::Connect {
        host: params.host.clone(),
        port: params.port,
        source: e,
    })?;

    let auth_result = match &params.auth {
        AuthMethod::Password(pw) => {
            handle.authenticate_password(&params.username, pw).await?
        }
        AuthMethod::Key { key, passphrase } => {
            let key = russh::keys::decode_secret_key(key, passphrase.as_deref())
                .map_err(|e| SessionError::ParseKey { source: e })?;
            let key_with_hash = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            handle
                .authenticate_publickey(&params.username, key_with_hash)
                .await?
        }
    };
    if !auth_result.success() {
        return Err(SessionError::AuthFailed(params.username.clone()));
    }

    let channel = handle.channel_open_session().await?;
    channel
        .request_pty(false, TERM, params.cols as u32, params.rows as u32, 0, 0, &[])
        .await?;
    channel.request_shell(true).await?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (data_tx, data_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let stats = SessionStats::default();

    // handle 必须存活到会话结束：交给 supervisor 持有（exec 通道复用，Arc 共享）。
    let handle = Arc::new(handle);
    tokio::spawn(supervise(handle, channel, cmd_rx, data_tx, events_tx.clone(), stats.clone()));

    Ok(OpenedSession {
        session: Session { cmd_tx, params, stats },
        reader: SessionReader { rx: data_rx },
        events: events_rx,
    })
}

/// 独占通道所有权的监督任务：读远端 → 分发；收指令 → 写远端/改尺寸/关闭。
async fn supervise(
    handle: Arc<Handle<ClientHandler>>,
    mut channel: russh::Channel<russh::client::Msg>,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    data_tx: std::sync::mpsc::Sender<Vec<u8>>,
    events_tx: mpsc::UnboundedSender<SessionEvent>,
    stats: SessionStats,
) {
    let closed = |reason: &str| {
        let _ = events_tx.send(SessionEvent::Closed { reason: reason.to_string() });
    };

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                None | Some(Cmd::Shutdown) => break,
                Some(Cmd::Write(bytes)) => {
                    if channel.data_bytes(bytes).await.is_err() {
                        break;
                    }
                }
                Some(Cmd::Resize(cols, rows)) => {
                    let _ = channel.window_change(cols as u32, rows as u32, 0, 0).await;
                }
                // 一次性命令不占用 shell 通道：另开 exec 通道后台执行，结果回传。
                Some(Cmd::Exec { cmd, reply }) => {
                    let handle = handle.clone();
                    tokio::spawn(async move {
                        let _ = reply.send(run_exec(&handle, &cmd).await);
                    });
                }
            },
            msg = channel.wait() => match msg {
                Some(russh::ChannelMsg::Data { data }) => {
                    stats.add_in(data.len());
                    // 读端已释放（如 UI 已切换）时 send 失败；继续排空远端即可。
                    let _ = data_tx.send(data.to_vec());
                }
                Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                    let _ = data_tx.send(data.to_vec());
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                    closed(&format!("remote exited ({exit_status})"));
                    let _ = channel.eof().await;
                    break;
                }
                Some(russh::ChannelMsg::Eof) => {
                    closed("remote closed");
                    break;
                }
                Some(_) => {}
                None => break,
            },
        }
    }

    closed("closed");
}

/// 独立 exec 通道跑一次性命令：收集 stdout/stderr 至 EOF，按退出码归一结果。
async fn run_exec(handle: &Handle<ClientHandler>, cmd: &str) -> Result<String, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("open channel: {e}"))?;
    channel
        .exec(true, cmd)
        .await
        .map_err(|e| format!("exec: {e}"))?;

    let mut out = String::new();
    let mut err = String::new();
    let mut exit: Option<u32> = None;
    // 循环到 wait()=None（通道彻底关闭）：真机的 ExitStatus 在 Eof 之后才到，
    // 在 Eof/Close 提前 break 会漏掉退出码（官方 client_exec_simple 同款写法）。
    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { data } => {
                out.push_str(&String::from_utf8_lossy(&data));
            }
            russh::ChannelMsg::ExtendedData { data, .. } => {
                err.push_str(&String::from_utf8_lossy(&data));
            }
            russh::ChannelMsg::ExitStatus { exit_status } => {
                exit = Some(exit_status);
                let _ = channel.eof().await;
            }
            _ => {}
        }
    }
    let err_msg = || -> String {
        let t = err.trim();
        if t.is_empty() { "no output".into() } else { t.to_string() }
    };
    match exit {
        Some(0) => Ok(out),
        Some(code) => Err(if err.trim().is_empty() { format!("exit {code}") } else { err_msg() }),
        None => Err(err_msg()),
    }
}
