//! iroh P2P 传输端到端验证：连真实 sshrust-agent（桥接到目标机 sshd）。
//!
//! 默认 ignore（依赖外部网络与部署）。显式运行：
//! ```sh
//! SSHRUST_IROH_NODE=<nodeid> SSHRUST_IROH_RELAY=<url> [SSHRUST_IROH_TOKEN=<t>] \
//!   cargo test -p sshrust-core --test iroh_e2e -- --ignored --nocapture
//! ```
//! 判定思路：不需要真实登录凭据——用一个必然错误的密码走完整流程，
//! 得到 `AuthFailed` 即证明 iroh 连接 + QUIC 流 + SSH 版本交换 + 密钥交换 +
//! 认证请求全部经传输层抵达真实 sshd。

use sshrust_core::{AuthMethod, ConnectParams, SessionError, Transport};

#[test]
#[ignore]
fn iroh_ssh_handshake_reaches_auth() {
    let node = std::env::var("SSHRUST_IROH_NODE").expect("set SSHRUST_IROH_NODE");
    let relay = std::env::var("SSHRUST_IROH_RELAY").ok();
    let token = std::env::var("SSHRUST_IROH_TOKEN").ok();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    // 进度事件与链路查询一并验证（通道 unbounded，无需并发排水）。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut steps = Vec::new();
    let (result, path) = rt.block_on(async {
        let params = ConnectParams {
            host: node.clone(),
            port: 0,
            username: "sshrust_e2e_probe".into(),
            auth: AuthMethod::Password("definitely-wrong-password".into()),
            cols: 80,
            rows: 24,
            transport: Transport::Iroh { relay, token },
        };
        let result = match sshrust_core::session::open_session_traced(params, Some(tx)).await {
            // 错误凭据却连上 = 测试环境异常（也视为失败）。
            Ok(opened) => {
                opened.session.close();
                Err(SessionError::Iroh("unexpected auth success with wrong password".into()))
            }
            Err(e) => Ok(e),
        };
        while let Ok(s) = rx.try_recv() {
            steps.push(s);
        }
        // 链路快照：iroh 层连接已建立（认证失败不影响传输层），应能取到路径。
        let path = sshrust_core::iroh::path_info(
            &node,
            std::env::var("SSHRUST_IROH_RELAY").ok().as_deref(),
            std::env::var("SSHRUST_IROH_TOKEN").ok().as_deref(),
        )
        .await;
        (result, path)
    });

    // 进度事件：实际流程的**前缀**（在哪个阶段断掉由错误类型判定——
    // 完整成功链是 iroh 四步 → SSH 握手 → 认证）。
    let full = [
        sshrust_core::ConnectStep::IrohEndpoint,
        sshrust_core::ConnectStep::IrohOnline,
        sshrust_core::ConnectStep::IrohConnect,
        sshrust_core::ConnectStep::IrohStream,
        sshrust_core::ConnectStep::SshHandshake,
        sshrust_core::ConnectStep::SshAuth,
    ];
    assert!(
        steps.len() <= full.len() && steps.iter().zip(full.iter()).all(|(a, b)| a == b),
        "进度事件应是完整流程的前缀，实际：{steps:?}"
    );

    // 内层 Err = 错误凭据却连上（测试环境异常）；正常应得到 AuthFailed。
    let result = result.expect("open_session should return AuthFailed, not succeed");

    match result {
        SessionError::AuthFailed(user) => {
            println!("iroh E2E 通：SSH 全协议栈抵达 sshd，认证被拒（用户 {user}）——预期行为");
        }
        other => panic!("iroh 传输未走通，错误：{other}"),
    }

    // 链路快照：打印直连/中继状态（网络环境差异大，仅验证可查询且路径自洽）。
    match path {
        Ok(p) => {
            println!(
                "iroh 链路：relayed={} relay={:?} direct={:?}",
                p.relayed, p.relay_url, p.direct
            );
            assert!(
                p.relayed || p.direct.is_some(),
                "路径快照至少应有一条活动路径"
            );
        }
        Err(e) => panic!("path_info 查询失败：{e}"),
    }
}
