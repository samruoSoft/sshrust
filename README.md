# sshrust

English | [简体中文](README_CN.md)

An SSH server management app (Termius / XShell-like) built with Rust + [GPUI](https://github.com/longbridge/gpui).

Most SSH clients today force a trade-off. Polished GUI tools like Termius and XShell are
subscription-based or gated by personal-use-only licenses, while the free alternatives are
mostly Electron wrappers that drag a full Chromium runtime along just to draw a terminal —
easily 150–300 MB of RAM before you've even connected anywhere. sshrust takes a different
path: an all-native Rust stack (russh for the protocol, GPUI for the UI) that ships as a
single ~24 MB executable and idles at ~86 MB of memory, while still delivering multi-tab
terminals, grouped server management, remote file browsing, real-time system monitoring,
a remote process panel, a snippet library — and NAT-traversing P2P connections to reach
machines that have no public address.

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
  SQLite persistence (`~/.sshrust/sshrust.db`, auto-migrates the legacy hosts.json)
- **Terminal**: multi-tab sessions, pop out / merge back tabs into standalone windows,
  collapsible three-pane layout, compact title bar; connecting tabs show a live
  per-stage log (resolve → handshake → auth → PTY) and stay identified by alias
- **P2P connections**: reach machines behind NAT/firewall via iroh (relay rendezvous +
  QUIC hole punching, end-to-end encrypted); live path indicator (direct / relay / mixed)
  with measured round-trip latency, both endpoints' public IP & ISP, and automatic
  re-punching when stuck on relay. Configure once globally (Settings › General › Network);
  requires the [sshrust-agent](#p2p--sshrust-agent) binary on the target machine
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

## P2P & sshrust-agent

To connect to a machine without a public address (home NAS, lab box behind campus NAT,
cloud-free environments), sshrust pairs with **sshrust-agent** — a tiny bridge that runs
on the target machine, joins the iroh network (relay rendezvous + QUIC hole punching) and
forwards inbound streams to the local sshd.

- The agent is **closed-source** and distributed as prebuilt static binaries
  (Linux `aarch64` / `x86_64`, zero dependencies) from the
  [Releases](https://github.com/samruoSoft/sshrust/releases) page.
- Install on the target machine:

  ```bash
  # pick the binary for the arch, then:
  sudo mv sshrust-agent /usr/local/bin/sshrust-agent
  sudo env IROH_RELAY=https://your-relay IROH_RELAY_TOKEN=...        /usr/local/bin/sshrust-agent install   # systemd service, enabled at boot
  sudo sshrust-agent id                       # prints the fixed NodeID
  ```

- In the app: *Settings › General › Network* sets the relay/token once (leave empty to use
  the free public n0 infrastructure), then add a server in **P2P** mode with its NodeID.
- Security: traffic is end-to-end encrypted between the two endpoints (identity = the
  NodeID's key pair); relays only ever see ciphertext and metadata.

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
