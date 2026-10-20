//! iroh 链路观测探针：连真实 sshrust-agent，逐路径打印 RTT / 承流状态 /
//! 对端地址（连接级视图，验证「打洞升级后流量是否真走直连」），并周期打印
//! socket 级路径快照与对端候选地址。
//!
//! ```sh
//! SSHRUST_IROH_NODE=<nodeid> SSHRUST_IROH_RELAY=<url> [SSHRUST_IROH_TOKEN=<t>] \
//!   ROUNDS=15 cargo run -p sshrust-core --example iroh_probe
//! ```
//! 连接不开流（agent 侧 sshd 桥零接触）。★承流 = 当前承载流量的路径（is_selected）。

use std::time::Duration;

#[tokio::main]
async fn main() {
    let node = std::env::var("SSHRUST_IROH_NODE").expect("set SSHRUST_IROH_NODE");
    let relay = std::env::var("SSHRUST_IROH_RELAY").ok();
    let token = std::env::var("SSHRUST_IROH_TOKEN").ok();
    let rounds: usize = std::env::var("ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);

    let short: String = node.chars().take(8).collect();
    println!("拨号 {short}…（relay={}）", relay.as_deref().unwrap_or("n0 默认"));
    let conn = sshrust_core::iroh::dial_debug(&node, relay.as_deref(), token.as_deref())
        .await
        .expect("dial agent");
    println!("已连接，逐路径观测（{} 轮 × 2s）\n", rounds);

    // 诊断一：本端 QAD/NAT 报告与自身 z32。
    if let Ok(Some(r)) = sshrust_core::iroh::local_net_report(relay.as_deref(), token.as_deref()).await {
        println!("[本端 net_report] {r}");
    }
    if let Ok(z) = sshrust_core::iroh::self_id_z32(relay.as_deref(), token.as_deref()).await {
        println!("[本端 z32] {z}");
    }

    for i in 1..=rounds {
        tokio::time::sleep(Duration::from_secs(2)).await;

        println!("[{:>5.1}s]", i as f32 * 2.0);
        for p in &conn.paths() {
            let sel = if p.is_selected() { "★承流" } else { "  备用" };
            let kind = if p.is_relay() { "relay" } else { "direct" };
            println!(
                "    {:>6.1}ms  {sel}  {kind:<5} → {}",
                p.rtt().as_secs_f64() * 1000.0,
                p.remote_addr()
            );
        }
        match sshrust_core::iroh::path_info(&node, relay.as_deref(), token.as_deref()).await {
            Ok(p) => println!(
                "    socket级: relayed={} relay={:?} direct={:?}",
                p.relayed,
                p.relay_url.as_deref().unwrap_or("-"),
                p.direct
            ),
            Err(e) => println!("    socket级: 查询失败 {e}"),
        }
        // 对端候选（首尾各打一次全量）。
        if i == 1 || i == rounds {
            match sshrust_core::iroh::remote_paths_verbose(&node, relay.as_deref(), token.as_deref()).await {
                Ok(paths) => {
                    for (addr, active) in paths {
                        println!("    候选 {addr} {}", if active { "[活动]" } else { "[未激活]" });
                    }
                }
                Err(e) => println!("    候选查询失败: {e}"),
            }
        }
    }
}
