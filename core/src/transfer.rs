//! SFTP 文件传输：同一 SSH 连接上的 sftp 子系统通道（非独立 FTP 连接）。
//!
//! 架构与 [`crate::session`] 的 exec 通道同族：`Session::enqueue_transfer` 把请求
//! 发给 supervisor，supervisor 首次使用时启动 [`run_sftp`] 常驻任务（持有共享的
//! russh Handle），队列串行执行上传/下载。每笔传输返回 [`TransferTicket`]——
//! 状态放在 Arc 内的原子量/锁里，UI 侧轮询快照即可拿进度，取消也是无锁置位。
//!
//! 状态机按「块」驱动（一次 read+write 为一个块，任务循环每块之间处理新命令），
//! 避免把整个传输作为 future 放进 select! 被命令分支取消丢弃、重入时重复写入。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use russh::client::Handle;
use russh_sftp::client::fs::File as RemoteFile;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::session::ClientHandler;

/// 单块大小：进度粒度与往返开销的折中。
const CHUNK: usize = 64 * 1024;
/// SFTP 会话操作超时（防服务器半死后请求挂死）。
const SFTP_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferStatus {
    Queued,
    Running,
    Done,
    Error(String),
    Canceled,
}

/// 一笔传输的入队请求。`remote` 为远端绝对路径（含文件名）。
#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub direction: TransferDirection,
    pub local: PathBuf,
    pub remote: String,
}

/// UI 轮询用的进度快照。
#[derive(Debug, Clone)]
pub struct TransferSnapshot {
    pub direction: TransferDirection,
    pub file_name: String,
    pub local: String,
    pub remote: String,
    pub done: u64,
    pub total: u64,
    pub status: TransferStatus,
}

struct TransferInner {
    direction: TransferDirection,
    file_name: String,
    local: String,
    remote: String,
    done: AtomicU64,
    total: AtomicU64,
    status: Mutex<TransferStatus>,
    canceled: AtomicBool,
}

/// 传输票据：入队即得，克隆廉价；轮询 [`snapshot`] 看进度、[`cancel`] 取消。
///
/// [snapshot]: TransferTicket::snapshot
/// [cancel]: TransferTicket::cancel
#[derive(Clone)]
pub struct TransferTicket {
    inner: Arc<TransferInner>,
}

impl TransferTicket {
    fn new(req: &TransferRequest) -> Self {
        let file_name = match req.direction {
            // 上传以本地文件名展示；下载远端文件名即保存前的名字。
            TransferDirection::Upload => req
                .local
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            TransferDirection::Download => basename(&req.remote),
        };
        Self {
            inner: Arc::new(TransferInner {
                direction: req.direction,
                file_name,
                local: req.local.display().to_string(),
                remote: req.remote.clone(),
                done: AtomicU64::new(0),
                total: AtomicU64::new(0),
                status: Mutex::new(TransferStatus::Queued),
                canceled: AtomicBool::new(false),
            }),
        }
    }

    /// 请求取消：最多再传一块即停（状态转 `Canceled`）。
    pub fn cancel(&self) {
        self.inner.canceled.store(true, Ordering::Relaxed);
    }

    /// 票据稳定 id（Arc 地址；列表行内取消/移除定位用）。
    pub fn id(&self) -> u64 {
        Arc::as_ptr(&self.inner) as u64
    }

    pub fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            direction: self.inner.direction,
            file_name: self.inner.file_name.clone(),
            local: self.inner.local.clone(),
            remote: self.inner.remote.clone(),
            done: self.inner.done.load(Ordering::Relaxed),
            total: self.inner.total.load(Ordering::Relaxed),
            status: self.inner.status.lock().unwrap().clone(),
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            *self.inner.status.lock().unwrap(),
            TransferStatus::Done | TransferStatus::Error(_) | TransferStatus::Canceled
        )
    }

    fn set_status(&self, status: TransferStatus) {
        *self.inner.status.lock().unwrap() = status;
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// SFTP 任务命令（supervisor → run_sftp）。任务终止走 sender drop（supervise 退出）。
pub(crate) enum SftpCmd {
    Enqueue {
        req: TransferRequest,
        reply: oneshot::Sender<Result<TransferTicket, String>>,
    },
    /// 覆盖确认用：检查远端路径是否已存在。
    Exists {
        path: String,
        reply: oneshot::Sender<Result<bool, String>>,
    },
}

/// 正在执行的传输：票据 + 跨块存活的句柄与进度。
struct PendingTransfer {
    ticket: TransferTicket,
    local_path: PathBuf,
    remote_path: String,
    direction: TransferDirection,
    local_file: Option<tokio::fs::File>,
    remote_file: Option<RemoteFile>,
    buf: Vec<u8>,
    done: u64,
    /// 本地读到 EOF（上传）/远端读到 EOF（下载）：待收尾。
    draining: bool,
}

enum ChunkOutcome {
    More,
    Done,
}

impl PendingTransfer {
    fn new(ticket: TransferTicket, req: TransferRequest) -> Self {
        let (direction, local_path, remote_path) =
            (req.direction, req.local, req.remote);
        Self {
            ticket,
            local_path,
            remote_path,
            direction,
            local_file: None,
            remote_file: None,
            buf: vec![0u8; CHUNK],
            done: 0,
            draining: false,
        }
    }

    fn fail(&mut self, msg: String) {
        self.drop_handles();
        self.ticket.set_status(TransferStatus::Error(msg));
    }

    fn finish(&mut self) {
        self.drop_handles();
        if self.ticket.inner_canceled() {
            self.ticket.set_status(TransferStatus::Canceled);
        } else {
            self.ticket.set_status(TransferStatus::Done);
        }
    }

    fn drop_handles(&mut self) {
        // 句柄 Drop 即关闭（russh-sftp File 的 Drop 不等待 close 确认，可接受）。
        self.local_file = None;
        self.remote_file = None;
    }

    /// 推进一块：首次调用打开句柄；EOF 后收尾关闭。错误即整笔失败。
    async fn drive_chunk(&mut self, sftp: &SftpSession) -> Result<ChunkOutcome, String> {
        if self.ticket.inner_canceled() {
            return Ok(ChunkOutcome::Done); // finish() 会转 Canceled
        }
        // 首块：打开句柄 + 定总量。
        if self.remote_file.is_none() && self.local_file.is_none() {
            self.ticket.set_status(TransferStatus::Running);
            return self.open_handles(sftp).await.map(|_| ChunkOutcome::More);
        }
        if self.draining {
            return self.close_handles().await.map(|_| ChunkOutcome::Done);
        }
        match self.direction {
            TransferDirection::Upload => self.upload_chunk().await,
            TransferDirection::Download => self.download_chunk(sftp).await,
        }
    }

    async fn open_handles(&mut self, sftp: &SftpSession) -> Result<(), String> {
        match self.direction {
            TransferDirection::Upload => {
                let meta = tokio::fs::metadata(&self.local_path)
                    .await
                    .map_err(|e| format!("local: {e}"))?;
                self.ticket.inner_set_total(meta.len());
                let local = tokio::fs::File::open(&self.local_path)
                    .await
                    .map_err(|e| format!("local: {e}"))?;
                let remote = sftp.create(&self.remote_path).await.map_err(|e| format!("sftp: {e}"))?;
                self.local_file = Some(local);
                self.remote_file = Some(remote);
            }
            TransferDirection::Download => {
                let meta = sftp
                    .metadata(&self.remote_path)
                    .await
                    .map_err(|e| format!("sftp: {e}"))?;
                self.ticket.inner_set_total(meta.len());
                let remote = sftp.open(&self.remote_path).await.map_err(|e| format!("sftp: {e}"))?;
                let local = tokio::fs::File::create(&self.local_path)
                    .await
                    .map_err(|e| format!("local: {e}"))?;
                self.remote_file = Some(remote);
                self.local_file = Some(local);
            }
        }
        Ok(())
    }

    async fn upload_chunk(&mut self) -> Result<ChunkOutcome, String> {
        let local = self.local_file.as_mut().expect("local handle");
        let n = local.read(&mut self.buf).await.map_err(|e| format!("local: {e}"))?;
        if n == 0 {
            self.draining = true;
            return Ok(ChunkOutcome::More);
        }
        let remote = self.remote_file.as_mut().expect("remote handle");
        remote
            .write_all(&self.buf[..n])
            .await
            .map_err(|e| format!("sftp: {e}"))?;
        self.done += n as u64;
        self.ticket.inner_set_done(self.done);
        Ok(ChunkOutcome::More)
    }

    async fn download_chunk(&mut self, _sftp: &SftpSession) -> Result<ChunkOutcome, String> {
        let remote = self.remote_file.as_mut().expect("remote handle");
        let n = remote.read(&mut self.buf).await.map_err(|e| format!("sftp: {e}"))?;
        if n == 0 {
            self.draining = true;
            return Ok(ChunkOutcome::More);
        }
        let local = self.local_file.as_mut().expect("local handle");
        local
            .write_all(&self.buf[..n])
            .await
            .map_err(|e| format!("local: {e}"))?;
        local.flush().await.map_err(|e| format!("local: {e}"))?;
        self.done += n as u64;
        self.ticket.inner_set_done(self.done);
        Ok(ChunkOutcome::More)
    }

    async fn close_handles(&mut self) -> Result<(), String> {
        // 远端 close 等待全部 pending write 确认（丢弃则静默，可能丢写入）；
        // 本地句柄 Drop 即关闭（下载路径每块已 flush）。
        if let Some(remote) = self.remote_file.take() {
            remote.close().await.map_err(|e| format!("sftp close: {e}"))?;
        }
        self.local_file = None;
        Ok(())
    }
}

impl TransferTicket {
    fn inner_canceled(&self) -> bool {
        self.inner.canceled.load(Ordering::Relaxed)
    }
    fn inner_set_total(&self, v: u64) {
        self.inner.total.store(v, Ordering::Relaxed);
    }
    fn inner_set_done(&self, v: u64) {
        self.inner.done.store(v, Ordering::Relaxed);
    }
}

/// 打开（或复用）SFTP 会话：新通道 + sftp 子系统 + 协议握手。
async fn open_sftp_session(handle: &Handle<ClientHandler>) -> Result<SftpSession, String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("open channel: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("subsystem sftp: {e}（服务器可能未启用 SFTP）"))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("sftp init: {e}"))?;
    sftp.set_timeout(SFTP_TIMEOUT_SECS);
    Ok(sftp)
}

/// SFTP 常驻任务：串行队列 + 命令在块间处理；通道关闭时余项标记失败。
pub(crate) async fn run_sftp(
    handle: Arc<Handle<ClientHandler>>,
    mut rx: mpsc::UnboundedReceiver<SftpCmd>,
) {
    let mut sftp: Option<SftpSession> = None;
    let mut queue: VecDeque<PendingTransfer> = VecDeque::new();
    let mut current: Option<PendingTransfer> = None;
    let mut shutdown = false;

    while !shutdown {
        // 非阻塞收命令（块间处理：命令延迟 ≤ 一块）。
        loop {
            match rx.try_recv() {
                Ok(cmd) => {
                    if !handle_cmd(cmd, &handle, &mut sftp, &mut queue).await {
                        shutdown = true;
                        break;
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // supervise 结束（连接断开/会话关闭）：终止全部传输。
                    shutdown = true;
                    break;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
            }
        }
        if shutdown {
            break;
        }

        // 无当前任务则从队列取一条（懒开 SFTP 会话）。
        if current.is_none() {
            if let Some(mut p) = queue.pop_front() {
                if sftp.is_none() {
                    match open_sftp_session(&handle).await {
                        Ok(s) => sftp = Some(s),
                        Err(e) => {
                            p.fail(e);
                            continue;
                        }
                    }
                }
                current = Some(p);
            }
        }

        let Some(p) = current.as_mut() else {
            // 队列空且无任务：阻塞等命令（收到后当场处理，不可丢弃）。
            match rx.recv().await {
                Some(cmd) => {
                    if !handle_cmd(cmd, &handle, &mut sftp, &mut queue).await {
                        break;
                    }
                }
                None => break,
            }
            continue;
        };

        let step = match sftp.as_ref() {
            Some(s) => p.drive_chunk(s).await,
            None => Err("sftp session lost".to_string()),
        };
        match step {
            Ok(ChunkOutcome::More) => {}
            Ok(ChunkOutcome::Done) => {
                p.finish();
                current = None;
            }
            Err(e) => {
                p.fail(e);
                current = None;
            }
        }
    }

    for mut p in queue.drain(..) {
        p.fail("session closed".into());
    }
    if let Some(p) = current.as_mut() {
        p.fail("session closed".into());
    }
}

/// 处理一条命令；返回 false 表示应终止任务（发送端已断开）。
async fn handle_cmd(
    cmd: SftpCmd,
    handle: &Handle<ClientHandler>,
    sftp: &mut Option<SftpSession>,
    queue: &mut VecDeque<PendingTransfer>,
) -> bool {
    match cmd {
        SftpCmd::Enqueue { req, reply } => {
            let ticket = TransferTicket::new(&req);
            let _ = reply.send(Ok(ticket.clone()));
            queue.push_back(PendingTransfer::new(ticket, req));
        }
        SftpCmd::Exists { path, reply } => {
            if sftp.is_none() {
                match open_sftp_session(handle).await {
                    Ok(s) => *sftp = Some(s),
                    Err(e) => {
                        let _ = reply.send(Err(e));
                        return true;
                    }
                }
            }
            let res = match sftp.as_ref() {
                Some(s) => s.try_exists(&path).await.map_err(|e| e.to_string()),
                None => unreachable!(),
            };
            if res.is_err() {
                // 会话可能已坏：下次重开。
                *sftp = None;
            }
            let _ = reply.send(res);
        }
    }
    true
}
