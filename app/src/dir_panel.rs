//! 右侧目录面板：按连接的服务器显示其**当前目录**清单（树形，真实数据）。
//!
//! 同步链路（控制台 cd → 面板自动进入），**纯客户端识别、零注入**
//! （不向 shell 写任何字节，控制台不出现额外字符）：
//! 1. 终端 fork 双来源识别 cwd：服务器原生 shell 集成回发的 OSC 7
//!    （`ESC]7;file://host/path ST`，旁路扫描）优先；默认来源是
//!    **光标行提示符识别**（`user@host:path$` 的 `\w` 路径形态，
//!    与着色规则同一套严格匹配），经 cwd 回调进入
//!    [`AppUi::on_terminal_cwd`]（`~` 按 home 展开，home 来自初始 pwd）；
//! 2. 初始兜底：连接即 exec `pwd` 拿登录目录（home 与首次 cwd 一次到位）；
//! 3. cwd 变化 → `dir_fetch` 在独立 exec 通道跑 `LC_ALL=C ls -la -- <path>`
//!    解析为 [`DirEntry`] 清单（缓存于 `cache`，按路径复用）。
//!
//! 树组件复用 ui-kit `TreeState`：目录懒加载——展开未缓存目录时
//! `Toggle` 事件触发 `dir_fetch`，完成后再推节点（展开态按路径 id 保留）。
//!
//! **取数按页面可见性门控（零流量原则）**：只有右面板开且当前页为目录时才
//! 发起 pwd / ls（[`AppUi::dir_page_visible`]）；隐藏期间 cd 只记录 cwd 不取数，
//! 回页时 [`AppUi::maybe_fetch_dir`] 按需补取（cwd 未知→pwd；已知未缓存→ls）。
//!
//! 面板结构（右侧面板内；目录树在上、传输区钉底，中间分割线可拖——
//! 同侧栏「树/快捷栏」模式；原会话状态区已独立为右面板「会话状态」页）：
//! ┌────────────────┐
//! │ 目录 · mock    ↻│ ← 标题行（↻ 重取当前目录清单）
//! │ □ /root        │ ← 当前目录路径行
//! │ ▾ .config      │ ← 树：箭头 + 图标 + 名称（+ 链接目标 → xx）
//! │ ▸ bin   →usr/bin
//! ├────────────────┤ ← 分割线（拖拽调两区高度）
//! │ 文件传输        │ ← 传输区（功能后续开发，现空态提示）
//! └────────────────┘

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{prelude::*, px, rgb, AnyElement, ClickEvent, Context, CursorStyle, MouseButton, MouseDownEvent, Window, div};

use sshrust_core::{ConnectParams, Session};

use crate::app_ui::{RightPage, TabKind};
use crate::icons;
use crate::theme::PalFx;
use ui_kit::pro::tree::{TreeEvent, TreeNodeData};

// —— 面板几何（右面板内部绝对布局，见 app_ui.rs 的坑位说明） ——
pub(crate) const DIR_TITLE_H: f32 = 30.;
pub(crate) const DIR_PATH_H: f32 = 26.;
/// 目录区顶部总占位（标题行 + 路径行）。
pub(crate) const DIR_TOP: f32 = DIR_TITLE_H + DIR_PATH_H;
/// 树 / 传输区分割条高（同侧栏分割线）。
pub(crate) const DIR_SPLITTER_H: f32 = 5.;
/// 树区最小高度（传输区拖到最大时保底）。
const DIR_TREE_MIN_H: f32 = 120.;
/// 传输区最小高度。
pub(crate) const TRANSFER_MIN_H: f32 = 80.;
/// 传输区默认高度。
pub(crate) const TRANSFER_DEFAULT_H: f32 = 140.;
/// 传输区高度持久化键。
pub(crate) const SETTING_DIR_TRANSFER_H: &str = "dir.transfer_h";
/// 单次 exec（列目录/取 cwd/系统信息探针）超时。
pub(crate) const EXEC_TIMEOUT: Duration = Duration::from_secs(10);

/// 传输区高度上限（保证树区 ≥ DIR_TREE_MIN_H；panel_h 为右面板高）。
pub(crate) fn dir_transfer_max_h(panel_h: f32) -> f32 {
    (panel_h - DIR_TOP - DIR_SPLITTER_H - DIR_TREE_MIN_H).max(TRANSFER_MIN_H)
}

/// 目录页分割线拖拽标记。**必须与侧栏 `SplitDrag` 分立**：gpui 的
/// on_drag_move 按标记类型全局分发，共用类型会交叉触发；锚点随标记
/// 实例携带（鼠标 y、起始传输区高度）。
pub(crate) struct DirSplitDrag(pub(crate) Arc<Mutex<Option<(f32, f32)>>>);

// ---------- 数据模型 ----------

/// 一条目录项（`ls -la` 解析产物）。
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct DirEntry {
    pub name: String,
    /// 目录或指向目录的符号链接（可展开；`ls` 列取时跟随链接）。
    pub dir: bool,
    /// 符号链接目标（行尾显示「→ 目标」）。
    pub symlink: Option<String>,
}

/// 一个已连接 tab 的目录面板状态。
pub(crate) struct DirTree {
    /// 会话标识（host:port）。树 id 哈希前缀，多服务器同路径不串 id。
    pub host_key: String,
    /// 控制台当前目录（OSC 7 / 提示符识别 / 初始 pwd；None = 尚未得知）。
    pub cwd: Option<String>,
    /// 登录 home（初始 pwd；提示符 `~` 路径的展开基准）。
    pub home: Option<String>,
    /// path → 清单缓存（当前目录与已展开目录）。
    pub cache: HashMap<String, Vec<DirEntry>>,
    /// 取数中的路径（防重复并发请求）。
    pub loading: HashSet<String>,
    /// 当前目录取数错误（路径行下方提示；成功后清除）。
    pub error: Option<String>,
}

impl DirTree {
    fn new(host_key: String) -> Self {
        Self {
            host_key,
            cwd: None,
            home: None,
            cache: HashMap::new(),
            loading: HashSet::new(),
            error: None,
        }
    }
}

// ---------- ls 解析与命令构造 ----------

/// 构造列目录命令：C locale 固定列格式 + `--` 防路径歧义。
pub(crate) fn ls_cmd(path: &str) -> String {
    format!("LC_ALL=C ls -la -- {}", shell_quote(path))
}

/// shell 单引号包裹（内嵌单引号转义为 `'\''`）。
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 解析 `ls -la` 输出：跳过 total/./..；第 1 列定类型（d/l），
/// 名字取第 9 列起的原文（含空格）；符号链接按首个「 -> 」拆目标。
pub(crate) fn parse_ls(out: &str) -> Vec<DirEntry> {
    let mut entries = Vec::new();
    for line in out.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with("total ") {
            continue;
        }
        // 跳过前 8 个空白分隔的字段（perms links owner group size month day time）。
        let Some(name) = field_after(line, 8) else { continue };
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let perms = line.split_whitespace().next().unwrap_or("");
        let dir = perms.starts_with('d') || perms.starts_with('l');
        let (name, symlink) = match name.find(" -> ") {
            Some(i) => (name[..i].to_string(), Some(name[i + 4..].to_string())),
            None => (name.to_string(), None),
        };
        entries.push(DirEntry { name, dir, symlink });
    }
    entries
}

/// 取第 n 个空白分隔字段之后的整段原文（原样保留名字中的空格）。
fn field_after(line: &str, n: usize) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    for _ in 0..n {
        // 跳过当前字段
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        // 跳过字段间空白
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    line.get(i..)
}

/// 文件图标按扩展名粗分（压缩包 / 文本类 / 通用文件）。
fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "gz" | "zip" | "tar" | "xz" | "7z" | "bz2" => "archive",
        "txt" | "log" | "conf" | "md" | "json" | "yaml" | "yml" | "cfg" | "ini" | "toml"
        | "sh" | "py" | "rs" | "js" | "html" | "css" => "file-text",
        _ => "file",
    }
}

/// 全路径 → 稳定树 id（同会话内路径唯一即 id 唯一；展开态跨刷新保留）。
fn path_id(host_key: &str, path: &str) -> usize {
    let mut h = DefaultHasher::new();
    host_key.hash(&mut h);
    path.hash(&mut h);
    h.finish() as usize
}

/// 目录状态 → kit 树节点森林。目录=分组（黄文件夹），文件=叶子（灰），
/// 隐藏项（点前缀）文字再弱一档，符号链接行尾「→ 目标」；
/// 未缓存的目录（含根）在取数期间显示「…」占位。
/// `paths` 回填 id → 路径映射（展开事件懒加载用）。
pub(crate) fn dir_forest(
    st: &DirTree,
    pal: &PalFx,
    paths: &mut HashMap<usize, String>,
) -> Vec<TreeNodeData> {
    let Some(root) = st.cwd.clone() else {
        return Vec::new();
    };
    if !st.cache.contains_key(&root) {
        // 根清单未到位：取数中给「…」占位（否则为错误/中间态，树区另有提示）。
        if st.loading.contains(&root) {
            let mut ph = TreeNodeData::leaf(path_id(&st.host_key, &format!("{root}/…")), "…");
            ph.colors.text = Some(pal.dim);
            return vec![ph];
        }
        return Vec::new();
    }
    let mut out = Vec::new();
    build_level(st, &root, pal, paths, &mut out);
    out
}

fn build_level(
    st: &DirTree,
    dir_path: &str,
    pal: &PalFx,
    paths: &mut HashMap<usize, String>,
    out: &mut Vec<TreeNodeData>,
) {
    paths.insert(path_id(&st.host_key, dir_path), dir_path.to_string());
    let Some(entries) = st.cache.get(dir_path) else { return };
    for e in entries {
        let child_path = format!("{}/{}", dir_path.trim_end_matches('/'), e.name);
        let id = path_id(&st.host_key, &child_path);
        paths.insert(id, child_path.clone());
        let hidden = e.name.starts_with('.');
        let mut node = if e.dir {
            let mut g = TreeNodeData::group(id, e.name.clone());
            if st.cache.contains_key(&child_path) {
                build_level(st, &child_path, pal, paths, &mut g.children);
            } else if st.loading.contains(&child_path) {
                let mut ph = TreeNodeData::leaf(path_id(&st.host_key, &format!("{child_path}/…")), "…");
                ph.colors.text = Some(pal.dim);
                g.children = vec![ph];
            }
            g.icon = Some(icons::asset_path(icons::FOLDER).into());
            g.icon_open = Some(icons::asset_path(icons::FOLDER_OPEN).into());
            // 目录操作（上传/下载等）后续接线，先不做拖放目标。
            g.droppable = false;
            g
        } else {
            let mut l = TreeNodeData::leaf(id, e.name.clone());
            l.icon = Some(icons::asset_path(file_icon(&e.name)).into());
            l
        };
        if let Some(target) = &e.symlink {
            node.suffix = Some(format!("→ {target}"));
        }
        node.colors.icon = Some(if e.dir { pal.warn } else if hidden { pal.dim } else { pal.muted });
        if !e.dir || hidden {
            node.colors.text = Some(if hidden { pal.dim } else { pal.muted });
        }
        out.push(node);
    }
}

// ---------- 行为（AppUi 扩展） ----------

impl crate::app_ui::AppUi {
    /// 活跃 tab（仅 Live）：目录面板的数据归属。
    pub(crate) fn active_live_tab(&self) -> Option<&crate::app_ui::Tab> {
        self.tabs
            .iter()
            .find(|t| t.id == self.active_id && matches!(t.kind, TabKind::Live { .. }))
    }

    /// 指定 tab 的活跃会话句柄（弹出独立窗口的 tab 不在主窗口 tabs 里，不取）。
    pub(crate) fn live_session(&self, tab_id: u64) -> Option<Session> {
        self.tabs
            .iter()
            .find(|t| t.id == tab_id)
            .and_then(|t| match &t.kind {
                TabKind::Live { session, .. } => Some(session.clone()),
                _ => None,
            })
    }

    /// tab 建立会话后登记其目录面板状态（已存在则保留，如重连/弹出送回）。
    pub(crate) fn ensure_dir_data(&mut self, tab_id: u64, params: &ConnectParams) {
        self.dir_data
            .entry(tab_id)
            .or_insert_with(|| DirTree::new(format!("{}:{}", params.host, params.port)));
    }

    /// 目录页是否可见（右面板开且当前页为目录）。不可见期间不发起任何取数。
    pub(crate) fn dir_page_visible(&self) -> bool {
        self.right_open && self.right_page == RightPage::Dir
    }

    /// 目录页（重新）可见时同步取数并重建树：cwd 未知 → 初始 pwd（确立 home）；
    /// 已知但清单未缓存（隐藏期间 cd 过）→ 补取 ls。切 tab / 回页 / 展开面板时调用。
    pub(crate) fn maybe_fetch_dir(&mut self, cx: &mut Context<Self>) {
        if !self.dir_page_visible() {
            return;
        }
        enum Need {
            Pwd,
            Ls(String),
        }
        let need = self.active_live_tab().and_then(|tab| {
            let st = self.dir_data.get(&tab.id)?;
            match st.cwd.as_deref() {
                None => Some(Need::Pwd),
                Some(cwd)
                    if !st.cache.contains_key(cwd) && !st.loading.contains(cwd) =>
                {
                    Some(Need::Ls(cwd.to_string()))
                }
                _ => None,
            }
        });
        if let Some((id, need)) = self.active_live_tab().map(|t| t.id).zip(need) {
            match need {
                Need::Pwd => self.dir_fetch_cwd(id, cx),
                Need::Ls(path) => self.dir_fetch(id, path, cx),
            }
        }
        // 隐藏期间 cwd 变化只记录不重建，树实体可能停在旧根——这里统一对齐
        //（fetch 同步段已登记 loading，重建时占位/内容取值正确）。
        self.refresh_dir_tree(cx);
    }

    /// 控制台 cwd 变化（OSC 7 / 提示符识别 / 初始 pwd）→ 记录新目录并取其清单。
    /// 提示符的 `~` 形态按 home 展开；home 未知或非绝对路径不认。
    /// 页面可见时 cd 即取数+重建树根（与路径行同帧一致）：命中缓存直接显示，
    /// 未缓存先显示「…」占位，清单由 dir_fetch 异步补上；
    /// 页面隐藏时只记录 cwd 不取数（零流量），回页由 maybe_fetch_dir 补取。
    pub(crate) fn on_terminal_cwd(&mut self, tab_id: u64, path: &str, cx: &mut Context<Self>) {
        let resolved = if let Some(rest) = path.strip_prefix('~') {
            match self.dir_data.get(&tab_id).and_then(|st| st.home.clone()) {
                Some(h) if rest.is_empty() => Some(h),
                Some(h) if rest.starts_with('/') => {
                    Some(format!("{}{}", h.trim_end_matches('/'), rest))
                }
                _ => None, // ~otheruser/… 或 home 未知：等初始 pwd
            }
        } else {
            path.starts_with('/').then(|| path.to_string())
        };
        let Some(path) = resolved else { return };
        let changed = match self.dir_data.get_mut(&tab_id) {
            Some(st) if st.cwd.as_deref() != Some(path.as_str()) => {
                st.cwd = Some(path.clone());
                st.error = None;
                true
            }
            _ => false,
        };
        if changed {
            if self.dir_page_visible() {
                // 先 fetch（同步段登记 loading / 命中缓存）再重建树，
                // 保证重建时占位/内容二态取值正确。
                self.dir_fetch(tab_id, path, cx);
                self.refresh_dir_tree(cx);
            }
            // 隐藏页：只记录 cwd，不取数不重建（零流量），回页时 maybe_fetch_dir 补。
        }
    }

    /// 后台取目录清单（独立 exec 通道跑 ls）；结果回填缓存/错误并刷新树。
    pub(crate) fn dir_fetch(&mut self, tab_id: u64, path: String, cx: &mut Context<Self>) {
        let Some(session) = self.live_session(tab_id) else { return };
        let Some(st) = self.dir_data.get_mut(&tab_id) else { return };
        if st.cache.contains_key(&path) || st.loading.contains(&path) {
            return;
        }
        st.loading.insert(path.clone());
        let cmd = ls_cmd(&path);
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let ran = crate::RUNTIME
                .get()
                .expect("tokio runtime")
                .spawn(async move {
                    tokio::time::timeout(EXEC_TIMEOUT, session.exec_output(cmd)).await
                })
                .await;
            let out: Result<String, String> = match ran {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => Err("timeout".into()),
                Err(_) => Err("background task failed".into()),
            };
            let _ = this.update(cx, |ui, cx| {
                let is_cwd = ui
                    .dir_data
                    .get(&tab_id)
                    .is_some_and(|st| st.cwd.as_deref() == Some(path.as_str()));
                let Some(st) = ui.dir_data.get_mut(&tab_id) else { return };
                st.loading.remove(&path);
                match out {
                    Ok(text) => {
                        st.cache.insert(path.clone(), parse_ls(&text));
                        if is_cwd {
                            st.error = None;
                        }
                    }
                    Err(e) => {
                        if is_cwd {
                            st.error = Some(e);
                        }
                    }
                }
                ui.refresh_dir_tree(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 初始 cwd 兜底：exec `pwd` 拿登录目录——同时确立 home（提示符 `~`
    /// 路径的展开基准）与首个 cwd；后续 cd 由提示符识别 / OSC 7 跟随。
    pub(crate) fn dir_fetch_cwd(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(session) = self.live_session(tab_id) else { return };
        if !self.dir_data.contains_key(&tab_id) {
            return;
        }
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let ran = crate::RUNTIME
                .get()
                .expect("tokio runtime")
                .spawn(async move {
                    tokio::time::timeout(EXEC_TIMEOUT, session.exec_output("pwd")).await
                })
                .await;
            let path = match ran {
                Ok(Ok(Ok(p))) => Some(p.trim().to_string()).filter(|p| p.starts_with('/')),
                _ => None,
            };
            if let Some(p) = path {
                let _ = this.update(cx, |ui, cx| {
                    if let Some(st) = ui.dir_data.get_mut(&tab_id) {
                        st.home = Some(p.clone());
                    }
                    ui.on_terminal_cwd(tab_id, &p, cx);
                });
            }
        })
        .detach();
    }

    /// 目录树事件：展开未缓存目录 → 懒加载取清单（其余事件后续接线）。
    pub(crate) fn on_dir_tree_event(&mut self, ev: &TreeEvent, cx: &mut Context<Self>) {
        if let TreeEvent::Toggle { id, expanded: true } = ev {
            let Some(tab_id) = self.active_live_tab().map(|t| t.id) else { return };
            let Some(path) = self.dir_paths.get(id).cloned() else { return };
            let cached = self
                .dir_data
                .get(&tab_id)
                .is_some_and(|st| st.cache.contains_key(&path));
            if !cached {
                self.dir_fetch(tab_id, path, cx);
            }
        }
    }

    /// 把活跃 tab 的目录数据推入树组件（tab 切换/数据到达/主题变化后调用）。
    pub(crate) fn refresh_dir_tree(&mut self, cx: &mut Context<Self>) {
        let kit = self.theme().kit();
        let mut paths = HashMap::new();
        let forest = match self
            .active_live_tab()
            .and_then(|t| self.dir_data.get(&t.id).map(|st| (t, st)))
        {
            Some((_tab, st)) => dir_forest(st, &self.theme(), &mut paths),
            None => Vec::new(),
        };
        self.dir_paths = paths;
        self.dir_tree.update(cx, |t, cx| {
            t.set_theme(kit, cx);
            t.set_nodes(forest, cx);
        });
    }

    // ---------- 渲染 ----------

    /// 标题行：「目录 · 服务器名」+ 刷新按钮（重取当前目录清单）。
    pub(crate) fn render_dir_title(&self, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let mut row = div()
            .id("dir-title")
            .flex()
            .items_center()
            .h(px(DIR_TITLE_H))
            .px_3()
            .flex_none()
            .text_size(px(11.))
            .text_color(rgb(pal.dim))
            .child(div().child(crate::i18n::t(cx, crate::i18n::Tr::DirTitle)));
        if let Some(tab) = self.active_live_tab() {
            let name: String = tab.title.chars().take(18).collect();
            row = row.child(div().text_color(rgb(pal.muted)).child(format!(" · {name}")));
        }
        row.child(div().flex_1())
            .child(
                div()
                    .id("dir-refresh")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded_md()
                    .text_size(px(12.))
                    .text_color(rgb(pal.dim))
                    .hover(|s| s.bg(pal.surface(pal.card)).text_color(rgb(pal.text)))
                    .child("↻")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                        // 重取当前目录：清缓存后立即重建（根层回到「…」占位），
                        // 清单由取数完成回调补上。
                        if let Some(tab) = this.active_live_tab() {
                            let id = tab.id;
                            if let Some(st) = this.dir_data.get_mut(&id) {
                                if let Some(cwd) = st.cwd.clone() {
                                    st.cache.remove(&cwd);
                                    this.dir_fetch(id, cwd, cx);
                                    this.refresh_dir_tree(cx);
                                }
                            }
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// 路径行：文件夹图标 + 当前目录（未知显示 —）。
    pub(crate) fn render_dir_path(&self, _cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let cwd = self
            .active_live_tab()
            .and_then(|t| self.dir_data.get(&t.id))
            .and_then(|st| st.cwd.clone())
            .unwrap_or_else(|| "—".into());
        div()
            .id("dir-path")
            .flex()
            .items_center()
            .gap_1()
            .h(px(DIR_PATH_H))
            .px_3()
            .flex_none()
            .border_b_1()
            .border_color(rgb(pal.divider()))
            .child(
                gpui::svg()
                    .path(icons::asset_path(icons::FOLDER))
                    .size(px(12.))
                    .flex_none()
                    .text_color(rgb(pal.dim)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(10.5))
                    .text_color(rgb(pal.dim))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(cwd),
            )
            .into_any_element()
    }

    /// 树区四态：未连接（引导）/ 已连接未知 cwd（等待）/ 取数失败（错误）/ 树。
    /// `bottom` 为树区底部让位（传输区 + 分割条）。
    pub(crate) fn render_dir_area(&self, bottom: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let live = self
            .active_live_tab()
            .and_then(|t| self.dir_data.get(&t.id).map(|st| (t.id, st)));
        let area = div()
            .id("dir-area")
            .absolute()
            .top(px(DIR_TOP))
            .bottom(px(bottom))
            .left_0()
            .right_0();
        let hint = |el: gpui::Stateful<gpui::Div>, color: u32, text: String| {
            el.flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px_3()
                        .text_size(px(11.))
                        .text_color(rgb(color))
                        .child(text),
                )
                .into_any_element()
        };
        match live {
            Some((_id, st)) => match (&st.cwd, &st.error) {
                (None, _) => hint(area, pal.dim, crate::i18n::t(cx, crate::i18n::Tr::DirNoInfo).into()),
                (Some(_), Some(err)) => hint(area, pal.error, err.clone()),
                (Some(_), None) => area.child(self.dir_tree.clone()).into_any_element(),
            },
            None => {
                hint(area, pal.dim, crate::i18n::t(cx, crate::i18n::Tr::DirEmptyHint).into())
            }
        }
    }

    // ----- 树 / 传输区分割线 -----

    /// 分割线拖拽：鼠标上移 → 传输区变高（与侧栏树/快捷栏同向）；
    /// 高度持久化，窗口过矮时按上限钳制（持久值不动，恢复窗口还原）。
    fn on_dir_splitter_drag(
        &mut self,
        e: &gpui::DragMoveEvent<DirSplitDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let y: f32 = e.event.position.y.into();
        let state = e.drag(cx).0.clone();
        let mut anchor = state.lock().expect("dir split drag lock");
        if !e.event.dragging() {
            *anchor = None;
            return;
        }
        let &mut (start_y, start_h) = anchor.get_or_insert((y, self.dir_transfer_h));
        let win_h: f32 = window.viewport_size().height.into();
        let h = (start_h + (start_y - y)).clamp(TRANSFER_MIN_H, dir_transfer_max_h(win_h));
        if h != self.dir_transfer_h {
            self.dir_transfer_h = h;
            self.db.set_setting(SETTING_DIR_TRANSFER_H, &format!("{h}"));
            cx.notify();
        }
    }

    /// 分割线（钉在传输区顶缘；拖拽调树/传输两区高度）。
    pub(crate) fn render_dir_splitter(&self, transfer_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let drag = DirSplitDrag(self.dir_split_drag.clone());
        div()
            .id("dir-splitter")
            .absolute()
            .bottom(px(transfer_h))
            .left_0()
            .w_full()
            .h(px(DIR_SPLITTER_H))
            .flex()
            .items_center()
            .cursor(CursorStyle::ResizeRow)
            .occlude()
            .child(div().h(px(1.)).w_full().bg(rgb(pal.divider())))
            .hover(|s| s.bg(rgb(pal.border)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, _cx| {
                    let y: f32 = ev.position.y.into();
                    *this.dir_split_drag.lock().expect("dir split drag lock") =
                        Some((y, this.dir_transfer_h));
                }),
            )
            .on_drag(drag, move |_, _, _, cx| {
                cx.new(|_| crate::sidebar::SplitterGhost)
            })
            .on_drag_move(cx.listener(
                |this, e: &gpui::DragMoveEvent<DirSplitDrag>, w, cx| {
                    this.on_dir_splitter_drag(e, w, cx);
                },
            ))
            .into_any_element()
    }

    /// 传输区（钉底；标题行 + 空态提示——传输功能后续开发）。
    pub(crate) fn render_dir_transfer(&self, transfer_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        div()
            .id("dir-transfer")
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .h(px(transfer_h))
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .id("dir-transfer-head")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(DIR_TITLE_H))
                    .px_3()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(pal.dim))
                    .child(
                        gpui::svg()
                            .path(icons::asset_path(icons::UPLOAD))
                            .size(px(12.))
                            .flex_none()
                            .mr_1()
                            .text_color(rgb(pal.dim)),
                    )
                    .child(crate::i18n::t(cx, crate::i18n::Tr::TransferTitle)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_3()
                            .text_size(px(11.))
                            .text_color(rgb(pal.dim))
                            .child(crate::i18n::t(cx, crate::i18n::Tr::TransferNoTasks)),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{PalFx, THEMES};

    fn pal() -> PalFx {
        PalFx::build(&THEMES[0].1, crate::theme::SurfaceFx::OPAQUE)
    }

    /// ls 解析：目录/文件/符号链接/隐藏项/名字带空格/total 与 . .. 跳过。
    #[test]
    fn parse_ls_variants() {
        let out = "\
total 40
drwxr-xr-x  3 test test 4096 Oct 14 09:00 .
drwxr-xr-x  3 test test 4096 Oct 14 08:00 ..
drwxr-xr-x  2 test test 4096 Oct 14 09:00 projects
-rw-r--r--  1 test test  220 Oct  1 09:12 .bashrc
-rw-r--r--  1 test test  128 Sep 30 21:33 notes with space.txt
lrwxrwxrwx  1 test test    9 Oct 14 09:00 latest -> projects
lrwxrwxrwx  1 root root    7 Oct 14 08:00 bin -> usr/bin";
        let es = parse_ls(out);
        assert_eq!(
            es,
            vec![
                DirEntry { name: "projects".into(), dir: true, symlink: None },
                DirEntry { name: ".bashrc".into(), dir: false, symlink: None },
                DirEntry { name: "notes with space.txt".into(), dir: false, symlink: None },
                DirEntry { name: "latest".into(), dir: true, symlink: Some("projects".into()) },
                DirEntry { name: "bin".into(), dir: true, symlink: Some("usr/bin".into()) },
            ]
        );
        assert!(parse_ls("").is_empty());
        assert!(parse_ls("total 0\n").is_empty());
    }

    /// 命令构造：路径单引号包裹，内嵌引号转义。
    #[test]
    fn ls_cmd_quotes_path() {
        assert_eq!(ls_cmd("/home/t"), "LC_ALL=C ls -la -- '/home/t'");
        assert_eq!(ls_cmd("/a b"), "LC_ALL=C ls -la -- '/a b'");
        assert_eq!(ls_cmd("/it's"), "LC_ALL=C ls -la -- '/it'\\''s'");
    }

    /// 森林构建：缓存驱动嵌套、路径映射回填、未缓存目录子层为空、
    /// 取数中显示 … 占位、符号链接行尾 → 目标、目录不可拖放。
    #[test]
    fn forest_from_cache() {
        let mut st = DirTree::new("10.0.0.8:22".into());
        st.cwd = Some("/home/t".into());
        st.cache.insert(
            "/home/t".into(),
            vec![
                DirEntry { name: "projects".into(), dir: true, symlink: None },
                DirEntry { name: ".bashrc".into(), dir: false, symlink: None },
                DirEntry { name: "latest".into(), dir: true, symlink: Some("projects".into()) },
            ],
        );
        st.cache.insert(
            "/home/t/projects".into(),
            vec![DirEntry { name: "Cargo.toml".into(), dir: false, symlink: None }],
        );
        st.loading.insert("/home/t/latest".into());

        let mut paths = HashMap::new();
        let forest = dir_forest(&st, &pal(), &mut paths);

        let proj = forest.iter().find(|n| n.label == "projects").expect("projects");
        assert!(!proj.droppable);
        assert_eq!(proj.children.len(), 1);
        assert_eq!(proj.children[0].label, "Cargo.toml");

        let latest = forest.iter().find(|n| n.label == "latest").expect("latest");
        assert_eq!(latest.suffix.as_deref(), Some("→ projects"));
        assert_eq!(latest.children.len(), 1, "取数中显示 … 占位");
        assert_eq!(latest.children[0].label, "…");

        let bashrc = forest.iter().find(|n| n.label == ".bashrc").expect("隐藏文件");
        assert_eq!(bashrc.colors.text, Some(pal().dim));

        // 路径映射：根 + 各子路径（占位节点不入映射）。
        assert!(paths.values().any(|p| p == "/home/t"));
        assert!(paths.values().any(|p| p == "/home/t/projects"));
        assert!(paths.values().any(|p| p == "/home/t/latest"));
        assert!(!paths.values().any(|p| p.contains('…')));
    }

    /// 根清单未到位时的占位：取数中显示「…」，既非取数中也无缓存则空
    /// （错误态由面板提示行负责，不出假树）。
    #[test]
    fn forest_root_placeholder() {
        let mut st = DirTree::new("h:22".into());
        // cwd 未知：空。
        assert!(dir_forest(&st, &pal(), &mut HashMap::new()).is_empty());
        // cwd 已知、取数中：根层「…」占位。
        st.cwd = Some("/x".into());
        st.loading.insert("/x".into());
        let forest = dir_forest(&st, &pal(), &mut HashMap::new());
        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].label, "…");
        // cwd 已知、不在取数中也无缓存（错误/中间态）：空。
        st.loading.clear();
        assert!(dir_forest(&st, &pal(), &mut HashMap::new()).is_empty());
        // 有缓存：正常出树（同 forest_from_cache）。
        st.cache.insert("/x".into(), vec![DirEntry { name: "a".into(), dir: false, symlink: None }]);
        assert_eq!(dir_forest(&st, &pal(), &mut HashMap::new()).len(), 1);
    }

    /// 同路径不同会话 id 不同（多服务器互不串展开态）。
    #[test]
    fn ids_differ_across_hosts() {
        let mut a = DirTree::new("10.0.0.8:22".into());
        a.cwd = Some("/etc".into());
        a.cache.insert("/etc".into(), vec![DirEntry { name: "hosts".into(), dir: false, symlink: None }]);
        let mut b = DirTree::new("10.0.0.9:22".into());
        b.cwd = Some("/etc".into());
        b.cache.insert("/etc".into(), vec![DirEntry { name: "hosts".into(), dir: false, symlink: None }]);
        let mut pa = HashMap::new();
        let mut pb = HashMap::new();
        let fa = dir_forest(&a, &pal(), &mut pa);
        let fb = dir_forest(&b, &pal(), &mut pb);
        assert_ne!(fa[0].id, fb[0].id);
    }
}
