//! Phase 0 端到端验证：进程内 mock 服务器 + 真实 SSH 协议栈。
//! 覆盖：拒绝错误密码、正确认证、pty+shell 建立、双向字节流、echo 语义、
//! 远端退出事件、窗口尺寸变更。

use std::io::{Read, Write};
use std::time::Duration;

use tokio::sync::mpsc;

use sshrust_core::session::{open_session, AuthMethod, ConnectParams, SessionError};
use sshrust_core::{mock, SessionEvent};

fn params(port: u16, auth: AuthMethod) -> ConnectParams {
    ConnectParams {
        host: "127.0.0.1".into(),
        port,
        username: mock::MOCK_USER.into(),
        auth,
        cols: 80,
        rows: 24,
    }
}

/// 把阻塞 SessionReader 泵到异步通道，供测试逐段断言。
struct OutputPump {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

impl OutputPump {
    fn spawn(mut reader: sshrust_core::SessionReader) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("test-pump".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .expect("spawn pump");
        Self { rx }
    }

    /// 累计读取直到出现 needle，返回累计文本。
    async fn until(&mut self, needle: &[u8], timeout: Duration) -> anyhow::Result<String> {
        let mut acc = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("timeout waiting for {:?}; accumulated: {:?}", needle, String::from_utf8_lossy(&acc));
            }
            let chunk = tokio::time::timeout_at(deadline, self.rx.recv()).await;
            match chunk {
                Ok(Some(bytes)) => {
                    acc.extend_from_slice(&bytes);
                    if contains(&acc, needle) {
                        return Ok(String::from_utf8_lossy(&acc).into_owned());
                    }
                }
                Ok(None) => anyhow::bail!("stream closed; accumulated: {:?}", String::from_utf8_lossy(&acc)),
                Err(_) => anyhow::bail!("timeout waiting for {:?}; accumulated: {:?}", needle, String::from_utf8_lossy(&acc)),
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn e2e_mock_shell_roundtrip() {
    let server = mock::spawn().await.expect("spawn mock server");

    // 1) 错误密码必须被拒。
    let wrong = open_session(params(server.port, AuthMethod::Password("wrong".into()))).await;
    match wrong {
        Err(SessionError::AuthFailed(_)) => {}
        Err(e) => panic!("expected AuthFailed, got err: {e}"),
        Ok(_) => panic!("expected AuthFailed, got Ok"),
    }

    // 2) 正确认证 + pty + shell。
    let opened = open_session(params(server.port, AuthMethod::Password(mock::MOCK_PASS.into())))
        .await
        .expect("open session");
    let session = opened.session;
    let mut writer = session.writer();
    let mut pump = OutputPump::spawn(opened.reader);

    let banner = pump.until(b"> ", Duration::from_secs(5)).await.expect("read banner");
    assert!(banner.contains("Welcome to sshrust mock shell"), "banner: {banner:?}");

    // 3) 输入回显 + echo 行。
    writer.write_all(b"hello ssh\r").unwrap();
    let echoed = pump.until(b"> ", Duration::from_secs(5)).await.expect("read echo");
    assert!(echoed.contains("hello ssh"), "echo line: {echoed:?}");
    assert!(echoed.contains("echo: hello ssh"), "echo resp: {echoed:?}");

    // 4) 窗口尺寸变更不破坏会话。
    session.resize(120, 40);
    writer.write_all(b"after resize\r").unwrap();
    let after = pump.until(b"> ", Duration::from_secs(5)).await.expect("read after resize");
    assert!(after.contains("echo: after resize"), "after resize: {after:?}");

    // 5) exit → 收到 Closed 事件。
    writer.write_all(b"exit\r").unwrap();
    let mut events = opened.events;
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Some(SessionEvent::Closed { .. }) => break "closed",
                None => break "channel-gone",
            }
        }
    })
    .await
    .expect("close event timeout");
    assert_eq!(closed, "closed");

    server.stop();
}

/// exec 通道 + 目录同步链路：pwd / ls -la 固化输出；cd 回发 OSC 7。
#[tokio::test]
async fn e2e_exec_and_cwd_osc7() {
    let server = mock::spawn().await.expect("spawn mock server");
    let opened = open_session(params(server.port, AuthMethod::Password(mock::MOCK_PASS.into())))
        .await
        .expect("open session");
    let session = opened.session;
    let mut writer = session.writer();
    let mut pump = OutputPump::spawn(opened.reader);
    let _ = pump.until(b"> ", Duration::from_secs(5)).await.expect("read banner");

    // 1) 一次性命令走独立 exec 通道：pwd / ls。
    let pwd = session.exec_output("pwd").await.expect("exec pwd");
    assert_eq!(pwd.trim(), "/home/test");
    let ls = session.exec_output("LC_ALL=C ls -la -- /home/test").await.expect("exec ls");
    assert!(ls.contains("latest -> projects"), "ls 固化输出：{ls:?}");

    // 2) shell 里 cd → 回发 OSC 7（file://mock + 新路径）。
    writer.write_all(b"cd /\r").unwrap();
    let resp = pump.until(b"> ", Duration::from_secs(5)).await.expect("read cd resp");
    assert!(resp.contains("\x1b]7;file://mock/\x1b\\"), "OSC 7 回发：{resp:?}");

    // 3) exec 不影响 shell 通道（会话仍可用）。
    writer.write_all(b"still alive\r").unwrap();
    let after = pump.until(b"> ", Duration::from_secs(5)).await.expect("shell still works");
    assert!(after.contains("echo: still alive"), "after exec: {after:?}");

    session.close();
    server.stop();
}

/// 密钥认证（私钥内容直传，非文件路径）：OpenSSH 文本 → 解码 → 公钥认证 → shell。
#[tokio::test]
async fn e2e_key_content_auth() {
    let server = mock::spawn().await.expect("spawn mock server");

    let key_text = russh::keys::PrivateKey::random(
        &mut rand::rng(),
        russh::keys::ssh_key::Algorithm::Ed25519,
    )
    .expect("generate key")
    .to_openssh(russh::keys::ssh_key::LineEnding::LF)
    .expect("encode openssh")
    .to_string();
    assert!(key_text.contains("PRIVATE KEY"));

    let opened = open_session(params(server.port, AuthMethod::Key { key: key_text, passphrase: None }))
        .await
        .expect("open session with pasted key content");
    let mut pump = OutputPump::spawn(opened.reader);
    let banner = pump.until(b"> ", Duration::from_secs(5)).await.expect("read banner");
    assert!(banner.contains("Welcome to sshrust mock shell"), "banner: {banner:?}");

    opened.session.close();
    server.stop();
}

/// 非法私钥文本：连接前即失败，报解析错误而非认证错误。
#[tokio::test]
async fn e2e_invalid_key_content_fails_to_parse() {
    let server = mock::spawn().await.expect("spawn mock server");
    let err = open_session(params(server.port, AuthMethod::Key { key: "not a key".into(), passphrase: None })).await;
    match err {
        Err(SessionError::ParseKey { .. }) => {}
        Err(e) => panic!("expected ParseKey, got: {e}"),
        Ok(_) => panic!("expected ParseKey, got Ok"),
    }
    server.stop();
}
