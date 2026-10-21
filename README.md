# sshrust

English | [简体中文](README_CN.md)

An SSH / P2P server management app (Termius / XShell-like) built with Rust + iroh +
[GPUI](https://github.com/longbridge/gpui).

Most SSH clients today force a trade-off. Polished GUI tools like Termius and XShell are
subscription-based or gated by personal-use-only licenses, while the free alternatives are
mostly Electron wrappers that drag a full Chromium runtime along just to draw a terminal —
easily 150–300 MB of RAM before you've even connected anywhere. sshrust takes a different
path: an all-native Rust stack (russh for the protocol, [iroh](https://iroh.computer) for
P2P, GPUI for the UI) that ships as a single ~24 MB executable and idles at ~86 MB of
memory, while still delivering multi-tab terminals, grouped server management, remote file
browsing with SFTP transfers, real-time system monitoring, a remote process panel, a
snippet library — and iroh-based P2P connections that reach devices inside a LAN even
without a public IP, with no port forwarding or DDNS required.

## Screenshots

### Main Window

macOS full-height sidebar with a three-pane layout: the "My Servers" tree (groups / status dots)
on the left, multi-tab terminals in the center, and a collapsible tool panel on the right;
light/dark themes and frosted-glass window supported.

![Main window](doc/home.png)

### Remote Directory

Follows `cd` in the terminal automatically (OSC 7 + `ls -la` lazy loading), icon-decorated file
tree, SFTP transfers pinned at the bottom (upload / download with progress) with a draggable,
persisted split line.

![Remote directory](doc/directory.png)

### System Info

Four live cards: System (hostname / kernel / timezone / uptime), CPU (total usage + per-core
bars + history chart), Memory (donut chart + total / used / cached / available), and Network
(upload & download rate curves + cumulative traffic).

![System info](doc/systemInfo.png)

### Processes

A PID / CPU% / MEM% / RSS / command list with sortable headers; click a row for full details
(parent process / state / threads / uptime, etc.). A built-in rule set recognizes what each
process is for, with one-click external search.

![Processes](doc/Processes.png)

## Footprint

Measured on macOS (Apple Silicon), release build:

| Metric | Value |
| --- | --- |
| Executable size | ~24 MB, single self-contained binary (P2P stack included) — no bundled runtime |
| DMG installer | ~12 MB |
| Idle memory | ~86 MB (Activity Monitor), stable over time |

The idle footprint already includes the Metal GPU driver stack and font / icon caches that
GPUI pulls in — the app's own state is only a small slice of it. For comparison: an empty
Electron shell typically sits at 150–300 MB, and Zed (also GPUI-based) idles at several
hundred MB.

## Features

- **Session management**: server tree (groups / drag & drop / context-menu CRUD / inline
  rename), password or private-key auth (key content pass-through + alias management);
  SQLite persistence
- **Terminal**: multi-tab sessions, pop out / merge back tabs into standalone windows,
  collapsible three-pane layout, compact title bar; connecting tabs show a live
  per-stage log (resolve → handshake → auth → PTY) and stay identified by alias
- **P2P connections**: reach machines behind NAT/firewall via iroh (relay rendezvous +
  QUIC hole punching, end-to-end encrypted); live path indicator (direct / relay / mixed)
  with measured round-trip latency, both endpoints' public IP & ISP, and automatic
  re-punching when stuck on relay. Configure once globally (Settings › General › Network);
  requires the [sshrust-agent](#p2p--sshrust-agent-nat-traversal-no-public-or-static-ip-needed) binary on the target machine
- **Right tool panel**: remote directory (follows `cd` via OSC 7 + `ls -la` lazy loading +
  **SFTP file transfer**: upload/download via context menu or toolbar button, progress /
  speed / cancel, overwrite confirmation), system info (real-time CPU / memory / network /
  disk charts), session status, and processes (CPU% differential sampling, sorting, details,
  process-purpose recognition + external search)
- **Snippets**: a global library plus per-server commands; double-click or the context menu
  writes them straight into the PTY
- **Appearance**: light/dark themes, window transparency + blur, macOS full-height sidebar
  (traffic lights sit in the sidebar)
- **Localization**: English & Chinese, switching instantly
- **Node icons**: embedded Lucide SVGs (317 icons; 42-category picker + keyword auto-matching)

## P2P & sshrust-agent (NAT traversal, no public or static IP needed)

To connect to a machine without a public address (home NAS, lab box behind campus NAT,
a Pi on mobile broadband), sshrust pairs with **sshrust-agent** — a tiny closed-source
bridge distributed as prebuilt static binaries (Linux `aarch64` / `x86_64`, zero
dependencies) from the [Releases](https://github.com/samruoSoft/sshrust/releases) page.
The agent joins the iroh network with a stable NodeID (an ed25519 public key) and
bridges every inbound QUIC stream to the local sshd:

```
 you                                              the device
┌──────────────┐    relay — rendezvous + fallback ┌──────────────┐  loopback ┌──────┐
│    sshrust   │◄════════ TLS over TCP 443 ══════►│ sshrust-agent│◄─────────►│ sshd │
│   (client)   │◄─ ─ ─ QUIC/UDP hole punch ─ ─ ─►│  (systemd)   │           └──────┘
└──────────────┘      direct path, preferred     └──────────────┘
```

The SSH layer rides on this transport unchanged: username / password / key are the
device's own SSH credentials, and every panel (terminal, SFTP transfer, system info,
processes) works over a P2P session. Traffic is end-to-end encrypted between the two
endpoints — whichever relay is in use only ever sees ciphertext. The session panel
tracks the live path (**Direct** / **Relay** / **Mixed** = direct selected with the
relay kept as hot standby), a measured round-trip latency, and both endpoints' public
egress IP; a session that starts relayed keeps re-attempting the hole punch
automatically, and a dropped direct path falls back to the relay without losing the
SSH session.

Two ways to run it — pick one per deployment:

### Option A — n0 public infrastructure (zero setup)

Leave the relay setting empty everywhere (agent installed without relay env vars,
app settings empty). Rendezvous, fallback relaying and address discovery all use
n0's free public services — nothing to deploy. Two caveats:

- n0's relays are shared and hosted overseas: from mainland China we measured
  ~350–550 ms relayed RTT (from elsewhere they are usually fast).
- With iroh's public defaults, published discovery records carry only the relay
  address and no direct IP candidates — connections work from anywhere, but they
  effectively stay relayed. Direct paths are a self-hosted-mode feature in this
  release (see the comparison below).

### Option B — self-hosting a relay (Docker)

A relay wants a small VPS with a public IP and a DNS name, with **TCP 80/443 and
UDP 7842** open — in the cloud security group *and* the host firewall. UDP is the one
everybody misses: when it is blocked nothing errors out, hole punching just quietly
degrades.

`/opt/iroh-relay/iroh-relay.toml`:

```toml
# Top-level keys must come before any [section].
# Lets endpoints learn their public address from *your* relay (QUIC, UDP 7842)
# instead of borrowing n0's mirrors.
enable_quic_addr_discovery = true

[tls]
hostname  = "relay.example.com"
cert_mode = "LetsEncrypt"        # issue & renew automatically; needs TCP 80 + 443
cert_dir  = "/certs"
contact   = "you@example.com"

[access]
# Only clients presenting this token may use the relay (generate: openssl rand -hex 32)
shared_token = ["0123abcd…"]
```

```bash
docker run -d --name iroh-relay --restart unless-stopped \
  -p 80:80 -p 443:443 -p 7842:7842/udp \
  -v /opt/iroh-relay/iroh-relay.toml:/iroh-relay.toml:ro \
  -v /opt/iroh-relay/certs:/certs \
  n0computer/iroh-relay:v1.3.0 -c /iroh-relay.toml

curl https://relay.example.com/healthz    # expect {"status":"ok",…}
```

Notes:

- The relay must serve TLS itself. If nginx already owns 443, give the relay its own
  port (`https_bind_addr = "[::]:9443"`) and reverse-proxy to it with
  `proxy_pass https://127.0.0.1:9443` — keep the `https://` scheme! — plus WebSocket
  upgrade headers; UDP 7842 bypasses the proxy entirely.
- Already holding certificates for that host (certbot, hosting panel)? Use
  `cert_mode = "Reloading"` with `manual_cert_path` / `manual_key_path`; the relay
  hot-reloads them on renewal.
- If the server's UDP inbound is permanently blocked, drop `enable_quic_addr_discovery`
  and set `IROH_QAD_RELAY` on the agent (see below) — sshrust borrows n0's relays as
  discovery mirrors automatically.
- Relay and client versions do not have to match exactly — relays are interoperable;
  any recent 1.x image is fine.

### Deploying sshrust-agent (device side)

```bash
scp sshrust-agent pi@device:/tmp/ && ssh pi@device
sudo mv /tmp/sshrust-agent /usr/local/bin/sshrust-agent   # never install from /tmp (wiped on boot)
sudo chmod +x /usr/local/bin/sshrust-agent

ss -tlnp | grep ssh   # which port does sshd actually listen on? (the agent bridges to it)

# Option A (n0): no env needed
sudo sshrust-agent install

# Option B (your relay — env given here is captured into the systemd unit):
sudo env IROH_RELAY=https://relay.example.com \
         IROH_RELAY_TOKEN=0123abcd… \
         SSHRUST_AGENT_SSH_ADDR=127.0.0.1:2222 \
         sshrust-agent install
#   SSHRUST_AGENT_SSH_ADDR: only if sshd is not on 22 (from the ss check above)
#   IROH_QAD_RELAY=https://usw1-1.relay.n0.iroh.link: only if your relay's UDP 7842
#     is blocked — an n0 relay reachable from both ends, used purely for discovery

sudo sshrust-agent id           # the NodeID you dial — stable across reboots
journalctl -u sshrust-agent -f  # live logs: version, NodeID, QAD probe results
```

`install` writes a systemd unit (`Restart=always`, boot autostart), captures the
current env into it and (re)starts the service. Changing relay / token / sshd port
later means re-running `install` with the new env, or editing
`/etc/systemd/system/sshrust-agent.service` followed by
`systemctl restart sshrust-agent`. The identity key lives in
`/var/lib/sshrust-agent/key`; `sshrust-agent getnewid` rotates it — the old NodeID
stops working immediately, so update your server entries. Remove the service with
`sudo sshrust-agent uninstall` (the key is kept).

### Configuring the client

1. *(Option B only)* Settings ‣ General ‣ Network — enter the relay URL and token
   once; they apply to all P2P connections. Leave empty for n0.
2. New server ‣ Connection: **P2P** — paste the agent's NodeID (64 hex characters).
   Username / password / key are the device's own SSH credentials.
3. Connect. The connecting tab logs every stage (iroh endpoint → relay rendezvous →
   dial → QUIC stream → SSH handshake → auth → PTY); once open, the session-status
   page tracks path, latency and both endpoints' egress.

### n0 vs self-hosted relay

|  | n0 public (default) | Self-hosted relay |
| --- | --- | --- |
| Setup | none | one Docker container on a public-IP host, ~10 min |
| Relayed latency | shared overseas relays: ~350–550 ms measured from China | tens of ms with a nearby VPS (~50–60 ms measured) |
| Direct path (hole punch) | effectively off — public defaults publish no IP candidates | on — ~10 ms measured between same-province broadband lines, relay kept as hot standby |
| Content confidentiality | identical — end-to-end encrypted, the relay sees only ciphertext | identical |
| Metadata (who↔who, timing) | visible to n0 | stays on your server |
| Access control | open to everyone | `shared_token` — only your clients |
| Availability | free, best-effort, no SLA | your uptime, your rules |

Fine print worth knowing:

- A bare NodeID must be discoverable somehow, so in **both** modes a signed address
  record (NodeID → relay + public address) is published to n0's public DNS — standard
  iroh behaviour. sshrust filters it down to the relay plus publicly routable IPs;
  LAN addresses are never published.
- Setting your own relay does not cut n0 off: n0's relays remain merged in as
  address-discovery mirrors and backup relays, so connections survive your own relay
  going down.
- Direct vs relayed mostly matters for bulk throughput — interactive typing feels
  similar at 10 ms and 50 ms, but large SFTP transfers are much faster over a direct
  path.

## Project Structure

- `core/` (sshrust-core): russh session abstraction, iroh P2P transport + in-process mock server
- `app/` (sshrust-app): the GPUI desktop app (see above)
- `ui-kit/`: gpui component library (base primitives / pro composites + theme injection,
  reusable across projects; feature notes in `ui-kit/FEATURES.md`)
- `spike/`: interactive CLI client (for exercising the core layer)
- `third_party/gpui-terminal`: terminal rendering fork (scroll-wheel fallback + scroll API)

> `sshrust-agent` (the target-side bridge) is closed-source and not part of this repository;
> prebuilt binaries live on the [Releases](https://github.com/samruoSoft/sshrust/releases) page.

## Getting Started

```bash
cargo run -p sshrust-app                # GUI
cargo run -p sshrust-app -- --mock-demo=2   # auto-open 2 mock terminals (demo)
cargo run -p spike -- --mock            # CLI against the built-in mock (test/test)
cargo test --workspace                  # all tests
```

## Tech Stack

russh 0.64 · russh-sftp 3.0 · iroh 1.3 (P2P transport) · gpui 0.2.2 · gpui-terminal 0.1.0 (fork) · tokio · rusqlite (bundled)

> Note: pinned to gpui 0.2.2 (crates.io version lock).

## Packaging

```bash
cargo build --release -p sshrust-app    # artifact: target/release/sshrust-app
# .app bundle + DMG: see scripts/package.sh
```

## License

The source code in this repository is licensed under **Apache-2.0 with the Commons
Clause**: you are free to use, copy, modify and redistribute it for any purpose —
including day-to-day use at a company — but you may **not Sell the Software**, i.e.
provide it, or a product/service whose value derives substantially from it, to third
parties for a fee or other consideration. See [LICENSE](LICENSE) for the exact wording.

**sshrust-agent** is closed-source. Its prebuilt binaries are free to use together with
sshrust and may not be redistributed, sold, or used to provide a paid service. For a
commercial license, open a GitHub issue.
