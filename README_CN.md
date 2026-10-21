# sshrust

[English](README.md) | 简体中文

Rust + iroh + [GPUI](https://github.com/longbridge/gpui) 打造的 SSH / P2P 服务器管理软件（类 Termius / XShell）。

现有 SSH 客户端总让人二选一：Termius、XShell 这类成熟的 GUI 工具按订阅收费、或仅限个人使用授权；
免费替代品则多为 Electron 套壳，为了画一个终端拖上整套 Chromium 运行时——还没连上服务器就先吃掉
150–300 MB 内存。sshrust 走了另一条路：全原生 Rust 技术栈（协议层 russh + P2P 层
[iroh](https://iroh.computer) + 界面层 GPUI），单个约 24 MB 的可执行文件、闲置约 86 MB 内存，
同时照样提供多标签终端、服务器分组管理、远程文件浏览与 SFTP 传输、实时系统监控、远程进程面板
与常用命令库——以及基于 iroh 的 P2P 连接：目标设备无公网 IP、免端口映射/DDNS，也能直连内网。

## 界面预览

### 主界面

macOS 全高侧栏 + 三栏布局：左侧「My Servers」服务器树（分组 / 状态指示点），中间多标签终端，
右侧可收缩工具面板；支持明暗主题与毛玻璃窗口。

![主界面](doc/home.png)

### 远程目录

跟随终端 `cd` 自动同步（OSC 7 + `ls -la` 懒加载），图标化文件树，底部 SFTP
文件传输区（上传/下载带进度），分割线可拖并持久化。

![远程目录](doc/directory.png)

### 系统信息

四张实时卡片：System（主机名 / 内核 / 时区 / 运行时间）、CPU（总占用 + 每核条 + 历史曲线）、
Memory（环形图 + 总量 / 已用 / 缓存 / 可用）、Network（上下行速率曲线 + 累计流量）。

![系统信息](doc/systemInfo.png)

### 进程面板

PID / CPU% / MEM% / RSS / 命令行列表，表头点击排序，点击行查看完整详情
（父进程 / 状态 / 线程数 / 运行时长等），内置规则库自动识别进程用途，并可跳转外部搜索。

![进程面板](doc/Processes.png)

## 体积与资源占用

macOS（Apple Silicon）release 构建实测：

| 指标 | 数值 |
| --- | --- |
| 可执行文件体积 | 约 24 MB，单文件自包含（含 P2P 协议栈），无捆绑运行时 |
| DMG 安装包 | 约 12 MB |
| 闲置内存 | 约 86 MB（活动监视器口径），长时间运行稳定 |

闲置内存中已包含 GPUI 拉起的 Metal GPU 驱动栈与字体 / 图标缓存，应用自身状态只占其中一小部分。
作为参照：Electron 空壳通常闲置 150–300 MB，同为 GPUI 技术栈的 Zed 闲置也在数百 MB 量级。

## 功能概览

- **会话管理**：服务器树（分组/拖拽/右键增删改/内联重命名）、密码或私钥认证
  （私钥内容直传 + 别名管理）；SQLite 持久化
- **终端**：多 tab 会话、tab 弹出/并回独立窗口、可收缩三栏、紧凑标题栏；
  连接过程实时分阶段日志（解析 → 握手 → 认证 → PTY），全程以别名标识
- **P2P 连接**：经 iroh 打通无公网设备的 SSH（relay 汇合 + QUIC 打洞，端到端加密）；
  链路指示（直连/中继/混合）+ 实测往返延迟 + 双方公网 IP 与运营商，中继卡死时
  自动重试打洞。relay/令牌在「设置 › 通用 › 网络」全局配置一次即可；目标机需运行
  [sshrust-agent](#p2p-与-sshrust-agent内网穿透无需公网和固定-ip)
- **右侧工具面板**：远程目录（OSC 7 跟随 cd + `ls -la` 懒加载 + **SFTP 文件传输**：
  右键/按钮上传下载、进度/速度/取消、同名覆盖确认）、
  系统信息（CPU/内存/网络/磁盘实时图表）、会话状态、进程面板（CPU% 差分采样、
  排序、详情、进程作用识别 + 外部搜索）
- **常用命令**：通用命令库 + 每服务器专用命令，双击/右键直写 PTY 运行到控制台
- **外观**：明暗主题、窗口透明度 + 毛玻璃、macOS 全高侧栏（交通灯落侧栏）
- **多语言**：中英双语，即时切换
- **节点图标**：内嵌 Lucide SVG（317 个；42 分类图标选择器 + 按名称关键词自动匹配）

## P2P 与 sshrust-agent（内网穿透、无需公网和固定 IP）

连接没有公网地址的设备（家里的树莓派/NAS、校园网内网主机、移动宽带下的小主机），sshrust 配套
**sshrust-agent**——目标机上的轻量桥接程序（**闭源**，预编译静态二进制 Linux `aarch64` / `x86_64`、
零依赖，见 [Releases](https://github.com/samruoSoft/sshrust/releases) 页面）。agent 以固定
NodeID（一对 ed25519 密钥的公钥，即设备身份）加入 iroh 网络，把每条入站 QUIC 流桥接到本机 sshd：

```
 你（客户端）                                       目标设备
┌──────────────┐     relay——汇合 + 兜底中继        ┌──────────────┐   回环   ┌──────┐
│    sshrust   │◄════════ TCP 443 上的 TLS ═══════►│ sshrust-agent│◄────────►│ sshd │
│              │◄─ ─ ─ QUIC/UDP 打洞直连 ─ ─ ─ ─ ►│ （systemd）  │          └──────┘
└──────────────┘        直连路径（优先）            └──────────────┘
```

SSH 层原样跑在该传输上：用户名/密码/私钥就是目标机自己的 SSH 凭据，终端、SFTP 传输、系统信息、
进程等全部面板在 P2P 会话下照常工作。流量在两端间端到端加密——无论用谁家的 relay，中继只见密文。
会话面板实时显示链路（**直连** / **中继** / **混合**＝直连已选中、relay 保留热备）、实测往返延迟
与两端公网出口 IP；起步走中继的会话会自动反复重试打洞，直连中途掉线也会无缝回落中继，SSH
会话不掉。

两种跑法，按部署自选其一：

### 方案 A —— n0 公共设施（零配置）

relay 处处留空（agent 不带 relay 环境变量安装、app 设置页留空）。汇合、兜底中继与地址发现全部
走 n0 免费公共服务，无需部署任何东西。两个注意点：

- n0 的 relay 是共享的且在海外：国内实测中继 RTT 约 350–550 ms（其他地区通常不慢）。
- iroh 公共默认发布的发现记录只含 relay 地址、不含直连 IP 候选——连接哪里都能通，但基本
  一直走中继。直连是本版本里自建模式的特性（见下方对比表）。

### 方案 B —— 自建中继（Docker）

中继需要一台有公网 IP 和域名的小 VPS，放行 **TCP 80/443 与 UDP 7842**——云安全组与主机防火墙
都要放。最容易被漏的是 UDP：被封时不会有任何报错，只是打洞能力悄悄退化。

`/opt/iroh-relay/iroh-relay.toml`：

```toml
# 顶层键必须写在任何 [节] 之前。
# 让端点经由你自己的 relay 探测公网地址（QUIC，UDP 7842），不必借用 n0 的镜子。
enable_quic_addr_discovery = true

[tls]
hostname  = "relay.example.com"
cert_mode = "LetsEncrypt"        # 自动签发与续期；需要 TCP 80 + 443
cert_dir  = "/certs"
contact   = "you@example.com"

[access]
# 只有持有此令牌的客户端可用（生成：openssl rand -hex 32）
shared_token = ["0123abcd…"]
```

```bash
docker run -d --name iroh-relay --restart unless-stopped \
  -p 80:80 -p 443:443 -p 7842:7842/udp \
  -v /opt/iroh-relay/iroh-relay.toml:/iroh-relay.toml:ro \
  -v /opt/iroh-relay/certs:/certs \
  n0computer/iroh-relay:v1.3.0 -c /iroh-relay.toml

curl https://relay.example.com/healthz    # 应返回 {"status":"ok",…}
```

说明：

- relay 必须自己对外提供 TLS。若 443 已被 nginx 占用，给 relay 单独端口
  （`https_bind_addr = "[::]:9443"`）再反代：`proxy_pass https://127.0.0.1:9443`——scheme
  必须是 https——并带上 WebSocket Upgrade 头；UDP 7842 不经反代、由客户端直连。
- 手头已有该主机证书（certbot / 面板签发）？改用 `cert_mode = "Reloading"` 配
  `manual_cert_path` / `manual_key_path`，relay 会在证书续期后热重载。
- 服务器 UDP 入站永久封死的话，去掉 `enable_quic_addr_discovery` 并给 agent 配
  `IROH_QAD_RELAY`（见下文）——sshrust 会自动借 n0 的 relay 做地址发现镜像。
- relay 与客户端版本不必严格一致——relay 间可互通，任意较新的 1.x 镜像均可。

### 部署 sshrust-agent（设备侧）

```bash
scp sshrust-agent pi@device:/tmp/ && ssh pi@device
sudo mv /tmp/sshrust-agent /usr/local/bin/sshrust-agent   # 别从 /tmp 安装（重启即被清空）
sudo chmod +x /usr/local/bin/sshrust-agent

ss -tlnp | grep ssh   # sshd 实际监听哪个端口？（agent 桥接的目标就是它）

# 方案 A（n0）：无需环境变量
sudo sshrust-agent install

# 方案 B（自建 relay——此处给出的环境变量会被捕获写进 systemd 单元）：
sudo env IROH_RELAY=https://relay.example.com \
         IROH_RELAY_TOKEN=0123abcd… \
         SSHRUST_AGENT_SSH_ADDR=127.0.0.1:2222 \
         sshrust-agent install
#   SSHRUST_AGENT_SSH_ADDR：仅当 sshd 不在 22 端口时需要（对照上面 ss 的结果）
#   IROH_QAD_RELAY=https://usw1-1.relay.n0.iroh.link：仅当自建 relay 的 UDP 7842
#     被封时需要——选两端都可达的 n0 节点，纯做地址发现

sudo sshrust-agent id           # 客户端要拨的 NodeID——重启不变
journalctl -u sshrust-agent -f  # 实时日志：版本、NodeID、QAD 探测结果
```

`install` 生成 systemd 单元（`Restart=always`、开机自启），把当前环境变量捕获进单元并
（重）启动服务。之后改 relay/令牌/sshd 端口＝带新环境变量重跑 `install`，或编辑
`/etc/systemd/system/sshrust-agent.service` 后 `systemctl restart sshrust-agent`。
身份密钥存于 `/var/lib/sshrust-agent/key`；`sshrust-agent getnewid` 轮换身份——旧 NodeID
立即作废，记得更新客户端里的服务器条目。`sudo sshrust-agent uninstall` 移除服务（密钥保留）。

### 客户端配置

1. （仅方案 B）设置 ‣ 通用 ‣ 网络——填一次 relay 地址与令牌，对所有 P2P 连接生效；留空即
   n0 模式。
2. 新建服务器 ‣ 连接方式选 **P2P**——粘贴 agent 的 NodeID（64 位十六进制）。用户名/密码/
   私钥填目标机自己的 SSH 凭据。
3. 连接。连接中的 tab 实时打印各阶段（iroh 端点 → relay 汇合 → 拨号 → QUIC 流 → SSH 握手 →
   认证 → PTY）；连上后，会话状态页持续显示链路、延迟与两端出口。

### n0 与自建中继的区别

|  | n0 公共（默认） | 自建中继 |
| --- | --- | --- |
| 部署 | 零 | 公网主机上一个 Docker 容器，约 10 分钟 |
| 中继延迟 | 共享海外节点：国内实测约 350–550 ms | 就近 VPS 几十 ms（实测约 50–60 ms） |
| 直连（打洞） | 基本走不上——公共默认发布的记录不含 IP 候选 | 可用——同省宽带实测约 10 ms，relay 保留热备 |
| 内容机密性 | 相同——端到端加密，中继只见密文 | 相同 |
| 元数据（谁连谁、时序） | n0 可见 | 留在你自己服务器上 |
| 访问控制 | 对所有人开放 | `shared_token`——只放行你的客户端 |
| 可用性 | 免费、尽力而为、无 SLA | 你的可用性你做主 |

几条值得知道的细节：

- 裸 NodeID 总得有被发现的途径，因此**两种模式**都会向 n0 公共 DNS 发布一条签名地址记录
  （NodeID → relay + 公网地址）——这是 iroh 的标准行为。sshrust 会把记录过滤到只剩 relay
  与公网可路由 IP，内网地址绝不发布。
- 配了自建 relay 并不会彻底断开 n0：n0 节点仍并入作地址发现镜像与备用中继，自建 relay 挂掉
  时连接依然存活。
- 直连与中继的差别主要体现在大流量吞吐——10 ms 与 50 ms 的交互式操作体感接近，但大文件
  SFTP 直连快得多。

## 项目结构

- `core/`（sshrust-core）：russh 会话抽象 + iroh P2P 传输 + 进程内 mock 服务器
- `app/`（sshrust-app）：GPUI 桌面应用（见上）
- `ui-kit/`：gpui 组件库（base 简单组件 / pro 复合组件 + 主题注入，可跨项目复用，
  功能记录见 `ui-kit/FEATURES.md`）
- `spike/`：交互式 CLI 客户端（core 链路调试用）
- `third_party/gpui-terminal`：终端渲染 fork（补滚轮回滚 + 滚动 API）

> `sshrust-agent`（目标侧桥接程序）为闭源组件，不在本仓库内；
> 预编译二进制见 [Releases](https://github.com/samruoSoft/sshrust/releases) 页面。

## 快速开始

```bash
cargo run -p sshrust-app                # GUI
cargo run -p sshrust-app -- --mock-demo=2   # 自动开 2 个 mock 终端（演示）
cargo run -p spike -- --mock            # CLI 直连内置 mock（test/test）
cargo test --workspace                  # 全部测试
```

## 技术栈

russh 0.64 · russh-sftp 3.0 · iroh 1.3（P2P 传输） · gpui 0.2.2 · gpui-terminal 0.1.0（fork） · tokio · rusqlite（bundled）

> 注：依赖 gpui 0.2.2（crates.io 锁版）。

## 打包

```bash
cargo build --release -p sshrust-app    # 产物 target/release/sshrust-app
# .app bundle + DMG 见 scripts/package.sh
```

## 许可证

本仓库源码采用 **Apache-2.0 + Commons Clause**：任何人可自由使用、复制、修改与再分发
（含公司内的日常使用），但**不得售卖本软件**——即以收费或其他对价向第三方提供本软件、
或价值主要来源于本软件的产品/服务。确切措辞见 [LICENSE](LICENSE)。

**sshrust-agent** 闭源：预编译二进制可随 sshrust 免费使用，不得再分发、售卖或用于提供
收费服务。需要商业授权请开 GitHub issue 联系。
