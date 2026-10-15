# sshrust

English | [简体中文](README_CN.md)

An SSH server management app (Termius / XShell-like) built with Rust + [GPUI](https://github.com/longbridge/gpui).

Most SSH clients today force a trade-off. Polished GUI tools like Termius and XShell are
subscription-based or gated by personal-use-only licenses, while the free alternatives are
mostly Electron wrappers that drag a full Chromium runtime along just to draw a terminal —
easily 150–300 MB of RAM before you've even connected anywhere. sshrust takes a different
path: an all-native Rust stack (russh for the protocol, GPUI for the UI) that ships as a
single ~16 MB executable and idles at ~86 MB of memory, while still delivering multi-tab
terminals, grouped server management, remote file browsing, real-time system monitoring,
a remote process panel and a snippet library.

## Screenshots

### Main Window

macOS full-height sidebar with a three-pane layout: the "My Servers" tree (groups / status dots)
on the left, multi-tab terminals in the center, and a collapsible tool panel on the right;
light/dark themes and frosted-glass window supported.

![Main window](doc/home.png)

### Remote Directory

Follows `cd` in the terminal automatically (OSC 7 + `ls -la` lazy loading), icon-decorated file
tree, transfer area pinned at the bottom with a draggable, persisted split line.

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
| Executable size | ~16 MB, single self-contained binary — no bundled runtime |
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
  collapsible three-pane layout, compact title bar
- **Right tool panel**: remote directory (follows `cd` via OSC 7 + `ls -la` lazy loading +
  transfer area), system info (real-time CPU / memory / network / disk charts), session
  status, and processes (CPU% differential sampling, sorting, details, process-purpose
  recognition + external search)
- **Snippets**: a global library plus per-server commands; double-click or the context menu
  writes them straight into the PTY
- **Appearance**: light/dark themes, window transparency + blur, macOS full-height sidebar
  (traffic lights sit in the sidebar)
- **Localization**: English & Chinese, switching instantly
- **Node icons**: embedded Lucide SVGs (314 icons; 42-category picker + keyword auto-matching)

## Project Structure

- `core/` (sshrust-core): russh session abstraction + in-process mock server
- `app/` (sshrust-app): the GPUI desktop app (see above)
- `ui-kit/`: gpui component library (base primitives / pro composites + theme injection,
  reusable across projects; feature notes in `ui-kit/FEATURES.md`)
- `spike/`: interactive CLI client (for exercising the core layer)
- `third_party/gpui-terminal`: terminal rendering fork (scroll-wheel fallback + scroll API)

## Getting Started

```bash
cargo run -p sshrust-app                # GUI
cargo run -p sshrust-app -- --mock-demo=2   # auto-open 2 mock terminals (demo)
cargo run -p spike -- --mock            # CLI against the built-in mock (test/test)
cargo test --workspace                  # all tests
```

## Tech Stack

russh 0.64 · gpui 0.2.2 · gpui-terminal 0.1.0 (fork) · tokio · rusqlite (bundled)

> Note: pinned to gpui 0.2.2 (crates.io version lock).

## Packaging

```bash
cargo build --release -p sshrust-app    # artifact: target/release/sshrust-app
# .app bundle + DMG: see scripts/package.sh
```
