//! iroh P2P 传输：经 relay 汇合 + QUIC 打洞直连目标端的 sshrust-agent，
//! 把 agent 桥接出来的一条双向 QUIC 流包装成 tokio AsyncRead/AsyncWrite，
//! 作为 russh 的传输载体（见 [`crate::session::open_session`]）。
//!
//! relay 配置两种形态：
//! - 未配置 → n0 公共设施（开箱即用）；
//! - 配置自建 relay（可带 token）→ 以其为主，n0 公共节点自动并入作
//!   QAD 地址探测与兜底中继（自建 relay 的 UDP 入站常被云平台截断）。

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, TransportAddr,
    endpoint::{RecvStream, SendStream, TransportAddrUsage, presets},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

/// 与 sshrust-agent 约定的 ALPN（改动需两端同步发布）。
pub const AGENT_ALPN: &[u8] = b"sshrust/ssh-bridge/1";

/// 一条已打通的 QUIC 双向流：russh 的 SSH 连接载体。
pub struct IrohStream {
    recv: RecvStream,
    send: SendStream,
}

impl AsyncRead for IrohStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // UFCS 显式选 tokio 版实现（noq 的流同时实现了 futures_io 版，错误类型不同）。
        AsyncRead::poll_read(Pin::new(&mut self.get_mut().recv), cx, buf)
    }
}

impl AsyncWrite for IrohStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().send), cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().send), cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().send), cx)
    }
}

/// 连接目标 agent 并打开一条双向流。`node_id` 为 64 位十六进制 NodeID。
pub async fn dial(
    node_id: &str,
    relay: Option<&str>,
    token: Option<&str>,
) -> Result<IrohStream, String> {
    dial_traced(node_id, relay, token, None).await
}

/// 丢弃式重拨（中继重打用）：只建立连接以触发地址解析与打洞协商，
/// **不打开流**——agent 的 sshd 桥不被惊动（其 accept_bi 随连接关闭直接返回），
/// 远端零噪音。路径状态按对端共享，打成的直连对既有会话连接同样可用。
pub async fn repunch(
    node_id: &str,
    relay: Option<&str>,
    token: Option<&str>,
) -> Result<(), String> {
    let id: EndpointId = node_id
        .trim()
        .parse()
        .map_err(|e| format!("NodeID 无效: {e}"))?;
    let ep = endpoint_for(relay, token).await?;
    ep.online().await;
    let mut addr = EndpointAddr::new(id);
    if let Some(url) = relay.map(str::trim).filter(|s| !s.is_empty()) {
        let relay_url: RelayUrl = url
            .parse()
            .map_err(|e| format!("relay 地址无效: {e}"))?;
        addr = addr.with_relay_url(relay_url);
    }
    if let Some(known) = KNOWN_DIRECT.lock().unwrap().get(node_id.trim()).copied() {
        addr = addr.with_ip_addr(known);
    }
    drop(
        ep.connect(addr, AGENT_ALPN)
            .await
            .map_err(|e| format!("iroh 连接失败: {e}"))?,
    );
    Ok(())
}

/// 诊断用：只建连接返回句柄（探针逐路径观测：RTT/承流状态/对端地址），
/// 不开流、不惊动 agent 的 sshd 桥。
pub async fn dial_debug(
    node_id: &str,
    relay: Option<&str>,
    token: Option<&str>,
) -> Result<iroh::endpoint::Connection, String> {
    let id: EndpointId = node_id
        .trim()
        .parse()
        .map_err(|e| format!("NodeID 无效: {e}"))?;
    let ep = endpoint_for(relay, token).await?;
    ep.online().await;
    let mut addr = EndpointAddr::new(id);
    if let Some(url) = relay.map(str::trim).filter(|s| !s.is_empty()) {
        let relay_url: RelayUrl = url
            .parse()
            .map_err(|e| format!("relay 地址无效: {e}"))?;
        addr = addr.with_relay_url(relay_url);
    }
    if let Some(known) = KNOWN_DIRECT.lock().unwrap().get(node_id.trim()).copied() {
        addr = addr.with_ip_addr(known);
    }
    ep.connect(addr, AGENT_ALPN)
        .await
        .map_err(|e| format!("iroh 连接失败: {e}"))
}

/// 上次观测到的对端直连地址（node_id → addr），重连时作为拨号提示带上。
/// 发现服务记录 TTL 仅 30s、无变化时 5 分钟才重发——新连接可能撞上
/// 「记录里还没有直连地址」的窗口而错失打洞；而锥形 NAT + 双方 endpoint
/// 常驻时，上次打洞成功的地址通常仍然有效（失效也只是多试一个候选，
/// relay 兜底不受影响）。
static KNOWN_DIRECT: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::net::SocketAddr>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// [`dial`] 的带进度版本：各阶段开始时经 `progress` 发出
/// [`crate::session::ConnectStep`]（接收端丢弃不影响流程）。
pub(crate) async fn dial_traced(
    node_id: &str,
    relay: Option<&str>,
    token: Option<&str>,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<crate::session::ConnectStep>>,
) -> Result<IrohStream, String> {
    let step = |s: crate::session::ConnectStep| {
        if let Some(tx) = progress {
            let _ = tx.send(s);
        }
    };
    let id: EndpointId = node_id
        .trim()
        .parse()
        .map_err(|e| format!("NodeID 无效: {e}"))?;
    step(crate::session::ConnectStep::IrohEndpoint);
    let ep = endpoint_for(relay, token).await?;
    // 等 home relay 连接就绪再拨号——不等待会与 relay 路径建立抢跑，裸 NodeID 无直连
    // 候选可试，最终 30s 超时（iroh-test dialer 的 online() 同款前置）。
    step(crate::session::ConnectStep::IrohOnline);
    ep.online().await;
    // 裸 NodeID 无处寻址：自定义 relay 时以其为汇合点（目标 agent 的 home relay）；
    // n0 模式（relay 未配置）则依赖 n0 DNS 发现。
    let mut addr = EndpointAddr::new(id);
    if let Some(url) = relay.map(str::trim).filter(|s| !s.is_empty()) {
        let relay_url: RelayUrl = url
            .parse()
            .map_err(|e| format!("relay 地址无效: {e}"))?;
        addr = addr.with_relay_url(relay_url);
    }
    // 上次打洞成功的直连地址作提示：绕过发现服务的发布时机问题。
    if let Some(known) = KNOWN_DIRECT.lock().unwrap().get(node_id.trim()).copied() {
        addr = addr.with_ip_addr(known);
    }
    step(crate::session::ConnectStep::IrohConnect);
    let conn = ep
        .connect(addr, AGENT_ALPN)
        .await
        .map_err(|e| format!("iroh 连接失败: {e}"))?;
    step(crate::session::ConnectStep::IrohStream);
    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(|e| format!("iroh 开流失败: {e}"))?;
    Ok(IrohStream { recv, send })
}

/// iroh 链路快照（成功连接后查询展示用；瞬时值，随时可能变化）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathInfo {
    /// 当前活动路径是否经 relay 中继。
    pub relayed: bool,
    /// 活动中继地址（经 relay 时）。
    pub relay_url: Option<String>,
    /// 活动直连地址（直连/打洞成功时；与 relay 可并存 = 混合路径）。
    pub direct: Option<String>,
}

/// 查询与目标 endpoint 当前活动传输路径（直连地址 / 中继 URL）。
/// 须在与目标建立连接后调用；复用 [`dial`] 的全局 endpoint 缓存。
pub async fn path_info(
    node_id: &str,
    relay: Option<&str>,
    token: Option<&str>,
) -> Result<PathInfo, String> {
    let id: EndpointId = node_id
        .trim()
        .parse()
        .map_err(|e| format!("NodeID 无效: {e}"))?;
    let ep = endpoint_for(relay, token).await?;
    let Some(info) = ep.remote_info(id).await else {
        return Err("对端链路信息不可用".into());
    };
    let mut path = PathInfo::default();
    for addr in info.addrs() {
        if !matches!(addr.usage(), TransportAddrUsage::Active) {
            continue;
        }
        match addr.addr() {
            TransportAddr::Relay(url) => {
                path.relayed = true;
                if path.relay_url.is_none() {
                    path.relay_url = Some(url.to_string());
                }
            }
            TransportAddr::Ip(sa) => {
                if path.direct.is_none() {
                    path.direct = Some(sa.to_string());
                }
            }
            _ => {}
        }
    }
    // 观测到活动直连地址 → 记入提示缓存（重连时带上，见 dial_traced）。
    if let Some(d) = &path.direct {
        if let Ok(sa) = d.parse() {
            KNOWN_DIRECT.lock().unwrap().insert(node_id.trim().to_string(), sa);
        }
    }
    Ok(path)
}

/// 对端全部已知传输地址（含未激活候选；诊断/探针用）。
/// 返回 (地址, 是否活动)——候选里有没有公网 IPv4 直接决定打洞能否进行。
pub async fn remote_paths_verbose(
    node_id: &str,
    relay: Option<&str>,
    token: Option<&str>,
) -> Result<Vec<(String, bool)>, String> {
    let id: EndpointId = node_id
        .trim()
        .parse()
        .map_err(|e| format!("NodeID 无效: {e}"))?;
    let ep = endpoint_for(relay, token).await?;
    let Some(info) = ep.remote_info(id).await else {
        return Err("对端链路信息不可用".into());
    };
    Ok(info
        .addrs()
        .map(|a| (a.addr().to_string(), matches!(a.usage(), TransportAddrUsage::Active)))
        .collect())
}

/// 本端 endpoint id 的 z32 编码（手工查 n0 pkarr 记录验证发布内容用）。
pub async fn self_id_z32(relay: Option<&str>, token: Option<&str>) -> Result<String, String> {
    let ep = endpoint_for(relay, token).await?;
    Ok(ep.id().to_z32())
}

/// 本端网络探测报告（QAD 结果；诊断/探针用）。`unstable-net-report` feature 门控。
pub async fn local_net_report(
    relay: Option<&str>,
    token: Option<&str>,
) -> Result<Option<String>, String> {
    use iroh::Watcher as _;
    let ep = endpoint_for(relay, token).await?;
    let r = ep.net_report().initialized().await;
    Ok(Some(format!(
        "udp_v4={} udp_v6={} global_v4={} global_v6={} mapping_varies_v4={}",
        r.udp_v4,
        r.udp_v6,
        r.global_v4.map(|s| s.to_string()).unwrap_or_else(|| "None".into()),
        r.global_v6.map(|s| s.to_string()).unwrap_or_else(|| "None".into()),
        r.mapping_varies_by_dest_ipv4
            .map(|v| v.to_string())
            .unwrap_or_else(|| "None".into()),
    )))
}

/// 全局 endpoint 缓存（iroh endpoint 启动含网络探测，代价高；同配置复用）。
/// relay 配置变化时重建。
static ENDPOINT: std::sync::LazyLock<Mutex<Option<(String, Arc<Endpoint>)>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

async fn endpoint_for(relay: Option<&str>, token: Option<&str>) -> Result<Arc<Endpoint>, String> {
    let cache_key = format!("{}|{}", relay.unwrap_or(""), token.is_some());
    let mut guard = ENDPOINT.lock().await;
    if let Some((key, ep)) = guard.as_ref() {
        if *key == cache_key {
            return Ok(ep.clone());
        }
    }
    let ep = build_endpoint(relay, token).await?;
    *guard = Some((cache_key, ep.clone()));
    Ok(ep)
}

async fn build_endpoint(relay: Option<&str>, token: Option<&str>) -> Result<Arc<Endpoint>, String> {
    let Some(relay) = relay.filter(|s| !s.trim().is_empty()) else {
        let ep = Endpoint::builder(presets::N0)
            .bind()
            .await
            .map_err(|e| format!("iroh endpoint 启动失败: {e}"))?;
        return Ok(Arc::new(ep));
    };
    let url: RelayUrl = relay
        .trim()
        .parse()
        .map_err(|e| format!("relay 地址无效: {e}"))?;
    let mut cfg = RelayConfig::from(url.clone());
    if let Some(t) = token.filter(|s| !s.trim().is_empty()) {
        cfg = cfg.with_auth_token(t.trim().to_string());
    }
    let map = RelayMap::empty();
    map.insert(url, Arc::new(cfg));
    // n0 公共节点整组并入：QAD 地址探测 + 兜底中继。
    for r in iroh::defaults::prod::default_relay_map().relays::<Vec<_>>() {
        map.insert(r.url.clone(), r);
    }
    // 地址发现（n0 DNS/pkarr）：iroh 1.3 的 relay 协议是纯转发、不交换节点地址，
    // 裸 NodeID 拨号靠 discovery 拿对端候选才能打洞。**坑：PkarrPublisher 默认
    // addr_filter = relay_only()——发布的记录只有 relay、不带任何 IP**（presets::N0
    // 同样如此），必须显式放开：relay + 公网可路由 IP（内网/回环不发布）。
    let ep = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(map))
        .address_lookup(
            iroh::address_lookup::PkarrPublisher::n0_dns().addr_filter(publish_addr_filter()),
        )
        .address_lookup(iroh::address_lookup::PkarrResolver::n0_dns())
        .address_lookup(iroh::address_lookup::DnsAddressLookup::n0_dns())
        .bind()
        .await
        .map_err(|e| format!("iroh endpoint 启动失败: {e}"))?;
    Ok(Arc::new(ep))
}

/// 发布过滤器：relay + 公网可路由 IP。内网/回环/链路本地地址发到公共 DNS
/// 既无用（跨网不可达）也泄内网拓扑，一律剔除。
fn publish_addr_filter() -> iroh::address_lookup::AddrFilter {
    use std::borrow::Cow;
    iroh::address_lookup::AddrFilter::new(|addrs: &Vec<TransportAddr>| {
        let kept: Vec<TransportAddr> = addrs
            .iter()
            .filter(|a| match a {
                TransportAddr::Relay(_) => true,
                TransportAddr::Ip(sa) => match sa.ip() {
                    std::net::IpAddr::V4(v4) => {
                        !(v4.is_private()
                            || v4.is_loopback()
                            || v4.is_link_local()
                            || v4.is_unspecified())
                    }
                    std::net::IpAddr::V6(v6) => {
                        !(v6.is_loopback()
                            || v6.is_unspecified()
                            || (v6.segments()[0] & 0xffc0) == 0xfe80)
                    }
                },
                _ => false,
            })
            .cloned()
            .collect();
        Cow::Owned(kept)
    })
}
