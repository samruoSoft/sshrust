//! P2P 会话网络探测：直连/中继链路 + 双方公网 IP 与运营商（会话状态页展示）。
//!
//! 三条通道各自独立、失败互不拖累（局部 Result 逐项落库）：
//! - 链路：core iroh `path_info`——活动传输地址快照（直连地址 / 中继 URL）；
//! - 本端：本机 `curl` 查询 ip-api.com（macOS 自带 curl，免引 HTTP 客户端依赖）；
//! - 对端：经 SSH exec 通道在远端执行同款查询——拿到的是远端真实出口。
//!
//! ip-api.com 免费接口限频 45 次/分，仅在连接建立时查一次，余量充足。

use serde::Deserialize;

use sshrust_core::session::Session;
use sshrust_core::PathInfo;

/// 一端的公网出口信息。
#[derive(Clone, Debug)]
pub(crate) struct IpInfo {
    pub ip: String,
    pub country: String,
    pub isp: String,
}

/// 一个 tab 的网络情况（iroh 会话连接成功后填充）。
/// 各字段 None = 检测中；Some(Err(..)) = 该项查询失败。
#[derive(Clone, Debug, Default)]
pub(crate) struct NetInfo {
    pub path: Option<Result<PathInfo, String>>,
    pub local: Option<Result<IpInfo, String>>,
    pub remote: Option<Result<IpInfo, String>>,
    /// 会话往返延迟（SSH exec 层实测，毫秒）——反映真实承流路径：
    /// 中继 ≈ 50ms 级、直连 ≈ 10ms 级。None = 尚未测得。
    pub rtt_ms: Option<u64>,
}

/// ip-api.com 返回的最小字段集。
#[derive(Deserialize)]
struct IpApi {
    #[serde(default)]
    status: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    isp: String,
}

/// 备用源 api.ip.sb/geoip（HTTPS）的最小字段集。
#[derive(Deserialize)]
struct IPSb {
    #[serde(default)]
    ip: String,
    #[serde(default)]
    country: String,
    /// 形如 "AS4134 CHINANET BACKBONE"，当运营商展示。
    #[serde(default)]
    organization: String,
}

/// 查询 URL（fields 精简到所需四项 + 状态位）。
const IP_API_URL: &str = "http://ip-api.com/json?fields=status,message,query,country,isp";
const IP_SB_URL: &str = "https://api.ip.sb/geoip";

/// 两种载荷任一解析成功即返回。
fn parse_ip_payload(text: &str) -> Result<IpInfo, String> {
    if let Ok(v) = serde_json::from_str::<IpApi>(text.trim()) {
        if v.status == "success" && !v.query.is_empty() {
            return Ok(IpInfo { ip: v.query, country: v.country, isp: v.isp });
        }
        if !v.message.is_empty() {
            return Err(v.message);
        }
    }
    if let Ok(v) = serde_json::from_str::<IPSb>(text.trim()) {
        if !v.ip.is_empty() {
            return Ok(IpInfo { ip: v.ip, country: v.country, isp: v.organization });
        }
    }
    Err(format!("unrecognized payload: {}", text.trim().chars().take(64).collect::<String>()))
}

/// 本端公网出口：本机 curl（阻塞式，须在 spawn_blocking 里跑）。
/// ip-api（http）国内链路偶发超时 → 失败走 api.ip.sb（https）兜底。
fn local_ip_info() -> Result<IpInfo, String> {
    for url in [IP_API_URL, IP_SB_URL] {
        let out = std::process::Command::new("curl")
            .args(["-s", "--max-time", "5", url])
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if let Ok(info) = parse_ip_payload(&String::from_utf8_lossy(&out.stdout)) {
            return Ok(info);
        }
    }
    Err("ip-api / ip.sb both failed".into())
}

/// 对端公网出口：SSH exec 远端查询（四级回退：ip-api curl/wget → ip.sb curl/wget；
/// URL 单引号防 shell 展开问号）。
async fn remote_ip_info(session: &Session) -> Result<IpInfo, String> {
    let cmd = format!(
        "curl -s --max-time 5 '{IP_API_URL}' || wget -qO- -T 5 '{IP_API_URL}' \
         || curl -s --max-time 5 '{IP_SB_URL}' || wget -qO- -T 5 '{IP_SB_URL}'"
    );
    let out = session
        .exec_output(cmd)
        .await
        .map_err(|e| format!("exec: {e}"))?;
    parse_ip_payload(&out)
}

/// 链路查询统一走 tokio 运行时（iroh 内部若依赖 reactor 上下文也安全）；
/// JoinHandle 在 gpui 执行器上 await 无 reactor 依赖。
async fn query_path(
    node: &str,
    relay: Option<&str>,
    token: Option<&str>,
) -> Result<PathInfo, String> {
    let (n, r, t) = (
        node.to_string(),
        relay.map(str::to_string),
        token.map(str::to_string),
    );
    super::RUNTIME
        .get()
        .expect("tokio runtime")
        .spawn(async move { sshrust_core::iroh::path_info(&n, r.as_deref(), t.as_deref()).await })
        .await
        .expect("join path query")
}

/// 中继定时重打：链路无直连时每 30s 后台重拨一次（重跑地址解析 + 打洞
/// 协商，并带上次成功的直连地址提示），最多 6 次——发现服务的记录存在
/// 发布时机窗口（TTL 30s / 无变化 5min 才重发），重拨是为了在记录恢复
/// 带地址的时刻自动抓住机会；出现直连即不再触发。
const REPUNCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const REPUNCH_MAX: u32 = 6;

/// 会话往返延迟：SSH exec 一个空命令的本地计时（含 sshd fork 开销，几毫秒级），
/// 近似网络往返——中继/直连差距（~50ms vs ~10ms）远大于该开销。
async fn session_rtt(session: &Session) -> Option<u64> {
    let t0 = std::time::Instant::now();
    session.exec_output(":").await.ok()?;
    Some(t0.elapsed().as_millis() as u64)
}

/// iroh 会话网络探测入口（连接成功后调用一次）：结果逐项写入 `AppUi.net_data`。
/// 链路 → 本端 → 对端顺序推进，先到先显示；随后**常驻轮询链路**（每 5s）——
/// iroh 是「先中继打通、后打洞升级直连」，一次性快照会把升级中的会话误标为中继。
pub(crate) async fn probe(
    tab_id: u64,
    node: String,
    relay: Option<String>,
    token: Option<String>,
    session: Session,
    this: gpui::WeakEntity<crate::app_ui::AppUi>,
    cx: &mut gpui::AsyncApp,
) {
    let mut net = NetInfo::default();

    net.path = Some(query_path(&node, relay.as_deref(), token.as_deref()).await);
    push(&this, cx, tab_id, &net).await;

    net.local = Some(
        super::RUNTIME
            .get()
            .expect("tokio runtime")
            .spawn_blocking(local_ip_info)
            .await
            .expect("join local net probe"),
    );
    push(&this, cx, tab_id, &net).await;

    net.remote = Some(remote_ip_info(&session).await);
    push(&this, cx, tab_id, &net).await;

    // 链路轮询：中继→直连的升级可能发生在连接后数秒乃至网络变化时。
    // 变化才推 UI；tab 关闭（net_data 条目被清）或连续查询失败即停。
    // 计时器必须用 gpui 的 background_executor：本任务跑在 gpui 执行器上，
    // tokio::time::sleep 需要 tokio reactor 上下文（Handle::current 直接 panic）。
    let mut failures = 0u32;
    let mut redials = 0u32;
    let mut last_redial = std::time::Instant::now();
    let mut round: u32 = 0;
    // 出口信息失败重试（ip-api 等公共接口偶发超时，一次失败不代表永远取不到）。
    let (mut local_retries, mut remote_retries) = (0u32, 0u32);
    loop {
        cx.background_executor()
            .timer(std::time::Duration::from_secs(5))
            .await;
        round += 1;
        // 每 4 轮（20s）实测一次会话往返延迟：随路径切换（中继↔直连）即时反映。
        if round % 4 == 0 {
            if let Some(ms) = session_rtt(&session).await {
                net.rtt_ms = Some(ms);
                if !push(&this, cx, tab_id, &net).await {
                    return;
                }
            }
        }
        // 每 6 轮（30s）重试失败的出口查询，各至多 5 次。
        if round % 6 == 0 {
            let mut updated = false;
            if matches!(net.local, Some(Err(_))) && local_retries < 5 {
                local_retries += 1;
                let r = super::RUNTIME
                    .get()
                    .expect("tokio runtime")
                    .spawn_blocking(local_ip_info)
                    .await
                    .expect("join local retry");
                if r.is_ok() {
                    net.local = Some(r);
                    updated = true;
                }
            }
            if matches!(net.remote, Some(Err(_))) && remote_retries < 5 {
                remote_retries += 1;
                if let Ok(r) = remote_ip_info(&session).await {
                    net.remote = Some(Ok(r));
                    updated = true;
                }
            }
            if updated && !push(&this, cx, tab_id, &net).await {
                return;
            }
        }
        match query_path(&node, relay.as_deref(), token.as_deref()).await {
            Ok(p) => {
                failures = 0;
                let changed = match net.path.as_ref() {
                    Some(Ok(old)) => *old != p,
                    _ => true,
                };
                if changed {
                    net.path = Some(Ok(p.clone()));
                    if !push(&this, cx, tab_id, &net).await {
                        return;
                    }
                }
                // 中继定时重打：无直连 → 到点重拨（丢弃式：只握手不开流，
                // agent 侧 sshd 桥不被惊动；失败静默，重试由轮询循环调度）。
                if p.direct.is_none()
                    && redials < REPUNCH_MAX
                    && last_redial.elapsed() >= REPUNCH_INTERVAL
                {
                    redials += 1;
                    last_redial = std::time::Instant::now();
                    let (n, r, t) = (node.clone(), relay.clone(), token.clone());
                    let _ = super::RUNTIME.get().expect("tokio runtime").spawn(async move {
                        let _ = sshrust_core::iroh::repunch(&n, r.as_deref(), t.as_deref()).await;
                    });
                }
            }
            Err(_) => {
                failures += 1;
                if failures >= 3 || !push(&this, cx, tab_id, &net).await {
                    return;
                }
            }
        }
    }
}

/// 写入 tab 的网络情况。返回 false = tab 已关闭（net_data 条目被清）或窗口已亡，
/// 调用方据此终止探测。
async fn push(
    this: &gpui::WeakEntity<crate::app_ui::AppUi>,
    cx: &mut gpui::AsyncApp,
    tab_id: u64,
    net: &NetInfo,
) -> bool {
    this.update(cx, |ui, cx| {
        if ui.net_data.contains_key(&tab_id) {
            ui.net_data.insert(tab_id, net.clone());
            cx.notify();
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ip_payload_two_sources() {
        // ip-api 载荷。
        let ok = parse_ip_payload(
            r#"{"status":"success","query":"1.2.3.4","country":"China","isp":"Chinanet"}"#,
        )
        .expect("valid ip-api payload");
        assert_eq!(ok.ip, "1.2.3.4");
        assert_eq!(ok.country, "China");
        assert_eq!(ok.isp, "Chinanet");

        // ip.sb 载荷（ip-api 状态缺失时按备用源解析）。
        let sb = parse_ip_payload(
            r#"{"ip":"5.6.7.8","country":"United States","organization":"AS3356 Lumen"}"#,
        )
        .expect("valid ip.sb payload");
        assert_eq!(sb.ip, "5.6.7.8");
        assert_eq!(sb.isp, "AS3356 Lumen");

        // ip-api 明确 fail 且 ip.sb 解析不出（无 ip 字段）→ Err 带服务端 message。
        let err = parse_ip_payload(r#"{"status":"fail","message":"reserved address"}"#);
        assert_eq!(err.unwrap_err(), "reserved address");

        // 非 JSON → Err。
        assert!(parse_ip_payload("<html>blocked</html>").is_err());
    }
}
