# sshrust

[English](README.md) | 简体中文

Rust + [GPUI](https://github.com/longbridge/gpui) 打造的 SSH 服务器管理软件（类 Termius / XShell）。

现有 SSH 客户端总让人二选一：Termius、XShell 这类成熟的 GUI 工具按订阅收费、或仅限个人使用授权；
免费替代品则多为 Electron 套壳，为了画一个终端拖上整套 Chromium 运行时——还没连上服务器就先吃掉
150–300 MB 内存。sshrust 走了另一条路：全原生 Rust 技术栈（协议层 russh + 界面层 GPUI），
单个约 16 MB 的可执行文件、闲置约 86 MB 内存，同时照样提供多标签终端、服务器分组管理、
远程文件浏览、实时系统监控、远程进程面板与常用命令库。

## 界面预览

### 主界面

macOS 全高侧栏 + 三栏布局：左侧「My Servers」服务器树（分组 / 状态指示点），中间多标签终端，
右侧可收缩工具面板；支持明暗主题与毛玻璃窗口。

![主界面](doc/home.png)

### 远程目录

跟随终端 `cd` 自动同步（OSC 7 + `ls -la` 懒加载），图标化文件树，底部传输区，
分割线可拖并持久化。

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
| 可执行文件体积 | 约 16 MB，单文件自包含，无捆绑运行时 |
| 闲置内存 | 约 86 MB（活动监视器口径），长时间运行稳定 |

闲置内存中已包含 GPUI 拉起的 Metal GPU 驱动栈与字体 / 图标缓存，应用自身状态只占其中一小部分。
作为参照：Electron 空壳通常闲置 150–300 MB，同为 GPUI 技术栈的 Zed 闲置也在数百 MB 量级。

## 功能概览

- **会话管理**：服务器树（分组/拖拽/右键增删改/内联重命名）、密码或私钥认证
  （私钥内容直传 + 别名管理）；SQLite 持久化（`~/.sshrust/sshrust.db`，
  自动迁移旧 hosts.json）
- **终端**：多 tab 会话、tab 弹出/并回独立窗口、可收缩三栏、紧凑标题栏
- **右侧工具面板**：远程目录（OSC 7 跟随 cd + `ls -la` 懒加载 + 传输区）、
  系统信息（CPU/内存/网络/磁盘实时图表）、会话状态、进程面板（CPU% 差分采样、
  排序、详情、进程作用识别 + 外部搜索）
- **常用命令**：通用命令库 + 每服务器专用命令，双击/右键直写 PTY 运行到控制台
- **外观**：明暗主题、窗口透明度 + 毛玻璃、macOS 全高侧栏（交通灯落侧栏）
- **多语言**：中英双语，即时切换
- **节点图标**：内嵌 Lucide SVG（314 个；42 分类图标选择器 + 按名称关键词自动匹配）

## 项目结构

- `core/`（sshrust-core）：russh 会话抽象 + 进程内 mock 服务器
- `app/`（sshrust-app）：GPUI 桌面应用（见上）
- `ui-kit/`：gpui 组件库（base 简单组件 / pro 复合组件 + 主题注入，可跨项目复用，
  功能记录见 `ui-kit/FEATURES.md`）
- `spike/`：交互式 CLI 客户端（core 链路调试用）
- `third_party/gpui-terminal`：终端渲染 fork（补滚轮回滚 + 滚动 API）

## 快速开始

```bash
cargo run -p sshrust-app                # GUI
cargo run -p sshrust-app -- --mock-demo=2   # 自动开 2 个 mock 终端（演示）
cargo run -p spike -- --mock            # CLI 直连内置 mock（test/test）
cargo test --workspace                  # 全部测试
```

## 技术栈

russh 0.64 · gpui 0.2.2 · gpui-terminal 0.1.0（fork） · tokio · rusqlite（bundled）

> 注：依赖 gpui 0.2.2（crates.io 锁版）。

## 打包

```bash
cargo build --release -p sshrust-app    # 产物 target/release/sshrust-app
# .app bundle + DMG 见 scripts/package.sh
```
