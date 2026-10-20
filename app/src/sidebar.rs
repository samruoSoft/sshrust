//! macOS 风格侧栏（系统级工具框）：
//!
//! ┌────────────────┐
//! │ 我的服务器        │ ← 小节标题
//! │ ▾ 连接           │ ← 分组（可展开/收起，右键菜单）
//! │   ● server…     │ ← 服务器节点（双击连接，右键菜单）
//! │   ▸ dev         │ ← 文件夹（右键新增子项）
//! │ ▾ 常用命令       │
//! ├────────────────┤ ← 分割线（可上下拖拽）
//! │ 快捷栏（占位）     │
//! ├────────────────┤
//! │ （空，内容暂取消）  │ ← 底部工具栏（条带占位）
//! └────────────────┘
//!
//! 树本体用 ui-kit `TreeState` 组件（展开/选中/重命名/拖拽/滚动内聚），
//! 本文件负责：db ↔ 树数据构建（build_forest）、事件分发（on_tree_event）、
//! 右键菜单（kit dropdown）、分割线/宽度拖拽、底部工具栏。

use std::sync::{Arc, Mutex};

use gpui::{
    prelude::*, px, rgb, AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render,
    StatefulInteractiveElement, Styled, Window, div,
};

use sshrust_core::{AuthMethod, ConnectParams};

use crate::db::{Db, NewNode, NodeKind, ServerData, TreeNode, TreeScope, ROOT_COMMANDS, ROOT_SERVERS};
use crate::app_ui::TabKind;
use crate::hosts::StoredAuth;
use crate::icons;
use crate::theme::Palette;
use crate::theme::*;
use ui_kit::base::dropdown::{self as kit_menu, MenuEntry};
use ui_kit::pro::tree::{EllipsisMode, TreeEvent, TreeNodeData};

const SETTING_SHORTCUT_H: &str = "sidebar.shortcut_h";
const SETTING_EXPANDED: &str = "sidebar.expanded";
/// 树工具栏高度（钉在标题区与树之间）。
const TREE_TOOLBAR_H: f32 = 26.0;

// —— kit 树 id 约定（与 db 正数 id 空间隔离；事件回程映射用） ——
pub(crate) const KIT_HEADER_SERVERS: usize = 900_000_001;
pub(crate) const KIT_GROUP_SERVERS: usize = 910_000_001;
pub(crate) const KIT_HEADER_COMMANDS: usize = 900_000_002;
pub(crate) const KIT_GROUP_COMMANDS: usize = 910_000_002;
/// 「专用命令」分组（动态：连接后按当前服务器显示；映射到 server 节点 id）。
pub(crate) const KIT_GROUP_SERVER_CMDS: usize = 910_000_003;

// ---------- 状态模型 ----------

/// 侧栏选中项。
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SelectKey {
    Node(i64),
}

/// 菜单弹出目标，决定菜单项集合。
#[derive(Clone, PartialEq)]
pub enum MenuTarget {
    /// 工具栏「＋」的通用新增。
    New,
    GroupServers,
    GroupCommands,
    /// 「专用命令」分组（当前连接服务器的命令树根 = server 节点 id）。
    GroupServerCommands(i64),
    /// 「专用命令」分组未连接时的空态（新增入口禁用提示）。
    GroupServerCommandsIdle,
    Node(i64),
    /// 标签页（弹出独立窗口入口）。
    Tab(u64),
    /// 目录树·文件节点（参数 = 远端绝对路径）。
    DirFile(String),
    /// 目录树·目录节点（参数 = 远端绝对路径）。
    DirDir(String),
    /// 目录树空白处（参数 = 当前 cwd）。
    DirBlank(String),
}

pub struct ContextMenu {
    pub origin: Point<Pixels>,
    pub target: MenuTarget,
}

#[derive(Clone)]
enum MenuAction {
    NewServer(i64),
    NewFolder(i64),
    /// 新增命令（parent = ROOT_COMMANDS / server 节点 id / 其下 folder），走表单窗口。
    NewCommand(i64),
    /// 编辑命令（表单窗口）。
    EditCommand(i64),
    /// 编辑目录名称（单字段表单窗口）。
    EditFolder(i64),
    /// 运行到控制台。
    RunCommand(i64),
    Connect(i64),
    EditServer(i64),
    Delete(i64),
    OpenDataDir,
    PopTab(u64),
    /// 目录树·下载远端文件（参数 = 远端绝对路径）。
    DirDownload(String),
    /// 目录树·上传本地文件到远端目录（参数 = 远端目录绝对路径）。
    DirUpload(String),
}

/// 拖拽标记：gpui 的 on_drag_move 按标记类型过滤，同一类型的多条拖拽会互相触发，
/// 因此垂直分割线 / 宽度柄各用独立标记。
#[derive(Clone)]
struct SplitDrag(Arc<Mutex<Option<(f32, f32)>>>);
#[derive(Clone)]
struct WidthDrag(Arc<Mutex<Option<(f32, f32)>>>);

pub(crate) struct SplitterGhost;

impl Render for SplitterGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // 拖拽会话要求一个跟随视图；全透明的 1px 使其不可见。
        div().size(px(1.)).opacity(0.)
    }
}

// ---------- 树数据构建（db → kit TreeNodeData） ----------

/// 由 db 与在线状态构建整棵树（每次数据变更后整体推入组件）。
/// `server_cmds`：当前连接服务器（激活 Live tab 对应的 db 节点）→ 显示其专用命令组。
pub(crate) fn build_forest(
    db: &Db,
    pal: &Palette,
    is_live: &dyn Fn(&str, u16) -> bool,
    loc: crate::i18n::Locale,
    server_cmds: Option<(i64, String)>,
) -> Vec<TreeNodeData> {
    let mut forest = vec![
        TreeNodeData::header(KIT_HEADER_SERVERS, crate::i18n::tl(loc, crate::i18n::Tr::TreeMyServers)),
        kit_group(
            KIT_GROUP_SERVERS,
            crate::i18n::tl(loc, crate::i18n::Tr::TreeConnections),
            children_of(db, ROOT_SERVERS, pal, is_live),
        ),
        TreeNodeData::header(KIT_HEADER_COMMANDS, crate::i18n::tl(loc, crate::i18n::Tr::TreeCommands)),
        kit_group(
            KIT_GROUP_COMMANDS,
            crate::i18n::tl(loc, crate::i18n::Tr::TreeGeneralCommands),
            children_of(db, ROOT_COMMANDS, pal, is_live),
        ),
    ];
    // 专用命令组：常驻显示；按当前连接服务器自动加载（未连接为空态组）。
    let mut server_group = match &server_cmds {
        Some((sid, sname)) => kit_group(
            KIT_GROUP_SERVER_CMDS,
            &crate::i18n::tlf(loc, crate::i18n::Tr::TreeServerCommands, &[sname]),
            children_of(db, *sid, pal, is_live),
        ),
        None => kit_group(
            KIT_GROUP_SERVER_CMDS,
            crate::i18n::tl(loc, crate::i18n::Tr::TreeServerCommandsPlain),
            Vec::new(),
        ),
    };
    server_group.expanded = true;
    forest.push(server_group);
    forest
}

fn kit_group(id: usize, label: &str, children: Vec<TreeNodeData>) -> TreeNodeData {
    let mut g = TreeNodeData::group(id, label);
    g.children = children;
    g
}

fn children_of(
    db: &Db,
    parent: i64,
    pal: &Palette,
    is_live: &dyn Fn(&str, u16) -> bool,
) -> Vec<TreeNodeData> {
    db.children(parent)
        .iter()
        .map(|n| to_kit_node(db, n, pal, is_live))
        .collect()
}

fn to_kit_node(db: &Db, node: &TreeNode, pal: &Palette, is_live: &dyn Fn(&str, u16) -> bool) -> TreeNodeData {
    match &node.kind {
        NodeKind::Folder => {
            let mut g = TreeNodeData::group(node.id as usize, node.name.clone());
            g.children = children_of(db, node.id, pal, is_live);
            if node.icon.is_empty() {
                g.icon = Some(icons::asset_path(icons::FOLDER).into());
                g.icon_open = Some(icons::asset_path(icons::FOLDER_OPEN).into());
            } else {
                let p: gpui::SharedString = icons::asset_path(&node.icon).into();
                g.icon = Some(p.clone());
                g.icon_open = Some(p);
            }
            g
        }
        NodeKind::Server(data) => {
            let mut l = TreeNodeData::leaf(node.id as usize, node.name.clone());
            let icon_name = if node.icon.is_empty() {
                icons::icon_for_name(&node.name, "server")
            } else {
                node.icon.clone()
            };
            l.icon = Some(icons::asset_path(&icon_name).into());
            // 行尾（简洁模式）与第二行（详细模式）同源：IP=host:port；P2P=NodeID。
            let addr = match data.mode {
                crate::db::ConnectMode::Ip => format!("{}:{}", data.host, data.port),
                crate::db::ConnectMode::Iroh => data.host.clone(),
            };
            l.suffix = Some(addr.clone());
            // IP:端口 一头一尾都有信息量 → 溢出时两头保留、中间省略。
            l.suffix_ellipsis = EllipsisMode::Middle;
            // 详细模式：第二行地址 + 第一行配置徽章（登录后从系统信息捕获）。
            l.subtitle = Some(addr);
            if !data.spec.is_empty() {
                l.badge = Some(data.spec.clone());
            }
            // 已连接 = 激活行：状态点转绿 + 图标染绿 + 行底铺浅绿色调
            // （bg 经 kit surface() 叠加表面透明度；模糊模式下表面还会整体压暗，
            // 比例取 30% 保证暗色系一眼可辨——亮色系同为浅绿不刺眼）。
            if is_live(&data.host, data.port) {
                l.dot = Some(pal.ok);
                l.colors.icon = Some(pal.ok);
                l.colors.bg = Some(mix_rgb(pal.panel, pal.ok, 0.30));
            } else {
                l.dot = Some(pal.dim);
            }
            l
        }
        NodeKind::Command(data) => {
            let mut l = TreeNodeData::leaf(node.id as usize, node.name.clone());
            let icon_name = if node.icon.is_empty() {
                icons::icon_for_name(&node.name, "terminal-square")
            } else {
                node.icon.clone()
            };
            l.icon = Some(icons::asset_path(&icon_name).into());
            // 说明作行尾弱化信息（无说明则留空占位对齐）。
            if !data.description.is_empty() {
                l.suffix = Some(data.description.clone());
            }
            l
        }
    }
}

// ---------- id 映射（kit 树 id ↔ 业务键） ----------

fn db_id_for(kit_id: usize) -> Option<i64> {
    (kit_id > 0 && kit_id < KIT_HEADER_SERVERS).then_some(kit_id as i64)
}

fn select_key_for(kit_id: usize) -> Option<SelectKey> {
    db_id_for(kit_id).map(SelectKey::Node)
}

fn menu_target_for(kit_id: Option<usize>) -> MenuTarget {
    match kit_id {
        Some(KIT_GROUP_SERVERS) => MenuTarget::GroupServers,
        Some(KIT_GROUP_COMMANDS) => MenuTarget::GroupCommands,
        Some(id) if db_id_for(id).is_some() => MenuTarget::Node(id as i64),
        _ => MenuTarget::New,
    }
}

// ---------- 行为 ----------

impl crate::app_ui::AppUi {
    // ----- 菜单 -----

    pub(crate) fn open_menu(&mut self, target: MenuTarget, origin: Point<Pixels>, cx: &mut Context<Self>) {
        self.menu = Some(ContextMenu { origin, target });
        cx.notify();
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            MenuAction::NewServer(parent) => {
                // 在所选目录下新建：表单窗口（独立窗口）。
                self.open_server_form(parent, None, None, cx);
            }
            MenuAction::NewFolder(parent) => {
                self.create_named_node(parent, NodeKind::Folder, crate::i18n::tl(self.loc, crate::i18n::Tr::DefNewFolder), window, cx)
            }
            MenuAction::NewCommand(parent) => {
                // 命令有结构化字段（标题/说明/内容），走表单窗口。
                self.open_command_form(parent, None, cx);
            }
            MenuAction::EditCommand(id) => self.edit_command(id, cx),
            MenuAction::EditFolder(id) => self.edit_folder(id, cx),
            MenuAction::RunCommand(id) => self.run_command_node(id, cx),
            MenuAction::Connect(id) => self.connect_node(id, cx),
            MenuAction::PopTab(id) => self.pop_out_tab(id, window, cx),
            MenuAction::EditServer(id) => self.edit_server(id, cx),
            MenuAction::Delete(id) => self.delete_node(id, window, cx),
            MenuAction::OpenDataDir => open_data_dir(),
            // 目录树传输入口（会话取激活 Live tab；无会话时入队即报错显示在传输行）。
            MenuAction::DirDownload(path) => self.start_download(&path, cx),
            MenuAction::DirUpload(dir) => self.start_upload(&dir, cx),
        }
        cx.notify();
    }

    // ----- 树事件分发（kit Tree → 业务动作） -----

    pub(crate) fn on_tree_event(&mut self, ev: &TreeEvent, cx: &mut Context<Self>) {
        match ev {
            TreeEvent::Select { id } => {
                self.selected = select_key_for(*id);
                cx.notify();
            }
            TreeEvent::Activate { id } => {
                if let Some(db_id) = db_id_for(*id) {
                    match self.db.node(db_id).map(|n| &n.kind) {
                        // 双击命令 = 运行到控制台；服务器 = 连接。
                        Some(NodeKind::Command(_)) => self.run_command_node(db_id, cx),
                        Some(NodeKind::Server(_)) => self.connect_node(db_id, cx),
                        _ => {}
                    }
                }
            }
            TreeEvent::Toggle { .. } => self.persist_expanded(cx),
            // 内联重命名已取消（app 不再调 begin_rename）；组件能力保留，事件防御性忽略。
            TreeEvent::Rename { .. } => {}
            TreeEvent::ContextMenu { id, position } => {
                // 专用命令组哨兵 → 当前连接服务器的命令树根；未连接为空态组。
                let target = match id {
                    Some(KIT_GROUP_SERVER_CMDS) => match self.active_server_node() {
                        Some((sid, _)) => MenuTarget::GroupServerCommands(sid),
                        None => MenuTarget::GroupServerCommandsIdle,
                    },
                    other => menu_target_for(*other),
                };
                self.open_menu(target, *position, cx);
            }
            TreeEvent::Move { from, to } => {
                let parent = match *to {
                    KIT_GROUP_SERVERS => ROOT_SERVERS,
                    KIT_GROUP_COMMANDS => ROOT_COMMANDS,
                    // 专用命令组 = 当前服务器的命令树根。
                    KIT_GROUP_SERVER_CMDS => match self.active_server_node() {
                        Some((sid, _)) => sid,
                        None => return,
                    },
                    n => n as i64,
                };
                if let Some(from_id) = db_id_for(*from) {
                    self.move_node_to(from_id, parent, cx);
                    self.refresh_tree(cx);
                }
            }
        }
    }

    /// 展开集持久化（kit id → db id 空间，含分组哨兵）。
    fn persist_expanded(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .tree
            .read(cx)
            .expanded_ids()
            .into_iter()
            .filter_map(|kid| match kid {
                KIT_GROUP_SERVERS => Some(ROOT_SERVERS),
                KIT_GROUP_COMMANDS => Some(ROOT_COMMANDS),
                KIT_GROUP_SERVER_CMDS => Some(crate::db::ROOT_SERVER_CMDS),
                n if n > 0 && n < KIT_HEADER_SERVERS => Some(n as i64),
                _ => None,
            })
            .collect();
        if let Ok(json) = serde_json::to_string(&ids) {
            self.db.set_setting(SETTING_EXPANDED, &json);
        }
    }

    // ----- 树工具栏：简洁/详细（行模式）+ 全部展开/收缩 -----

    /// settings 键：树行模式（"1" = 详细两行）。
    const SETTING_TREE_DETAIL: &'static str = "sidebar.tree.detail";

    fn set_tree_detail(&mut self, on: bool, cx: &mut Context<Self>) {
        self.tree_detail = on;
        self.db.set_setting(Self::SETTING_TREE_DETAIL, if on { "1" } else { "0" });
        self.tree.update(cx, |t, cx| t.set_detail(on, cx));
        cx.notify();
    }

    fn tree_expand_all(&mut self, cx: &mut Context<Self>) {
        self.tree.update(cx, |t, cx| t.expand_all(cx));
        self.persist_expanded(cx);
        cx.notify();
    }

    fn tree_collapse_all(&mut self, cx: &mut Context<Self>) {
        self.tree.update(cx, |t, cx| t.collapse_all(cx));
        self.persist_expanded(cx);
        cx.notify();
    }

    /// 树上方工具栏：简洁/详细（选中态互斥）+ 全部展开/收缩。
    fn render_tree_toolbar(&self, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let loc = self.loc;
        let t = |k: crate::i18n::Tr| crate::i18n::tl(loc, k);
        // 图标必须显式传色（gpui svg 不吃父级 text_color）。
        let chip = |id: &'static str,
                    icon: &str,
                    label: &str,
                    active: bool,
                    cx: &Context<Self>,
                    f: fn(&mut Self, &mut Context<Self>)| {
            let fg = if active { pal.accent_dark } else { pal.muted };
            div()
                .id(id)
                .h(px(18.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(3.))
                .flex_none()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(10.5))
                .bg(if active { rgb(pal.accent) } else { pal.surface(pal.card) })
                .text_color(rgb(fg))
                .hover(|s| s.bg(rgb(pal.border)))
                .child(
                    gpui::svg()
                        .path(icons::asset_path(icon))
                        .size(px(11.))
                        .flex_none()
                        .text_color(rgb(fg)),
                )
                .child(label.to_string())
                .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| f(this, cx)))
                .into_any_element()
        };
        div()
            .id("sidebar-tree-toolbar")
            .absolute()
            .top(px(TITLEBAR_H))
            .left_0()
            .w_full()
            .h(px(TREE_TOOLBAR_H))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .bg(pal.surface(pal.panel))
            .border_b_1()
            .border_color(rgb(pal.divider()))
            .child(chip(
                "tree-mode-compact",
                "rows-2",
                t(crate::i18n::Tr::TreeCompact),
                !self.tree_detail,
                cx,
                |this, cx| this.set_tree_detail(false, cx),
            ))
            .child(chip(
                "tree-mode-detail",
                "rows-4",
                t(crate::i18n::Tr::TreeDetail),
                self.tree_detail,
                cx,
                |this, cx| this.set_tree_detail(true, cx),
            ))
            .child(div().flex_1())
            .child(chip(
                "tree-expand-all",
                "plus",
                t(crate::i18n::Tr::TreeExpandAll),
                false,
                cx,
                |this, cx| this.tree_expand_all(cx),
            ))
            .child(chip(
                "tree-collapse-all",
                "minus",
                t(crate::i18n::Tr::TreeCollapseAll),
                false,
                cx,
                |this, cx| this.tree_collapse_all(cx),
            ))
            .into_any_element()
    }

    // ----- 节点 CRUD 入口 -----

    /// 新建节点：落库、刷树、进入组件内联重命名。
    fn create_named_node(
        &mut self,
        parent: i64,
        kind: NodeKind,
        default_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node = self.db.insert(NewNode { parent, kind, name: default_name.into() });
        self.refresh_tree(cx);
        self.selected = Some(SelectKey::Node(node.id));
        self.tree.update(cx, |t, cx| {
            t.set_selected(Some(node.id as usize), cx);
            t.begin_rename(node.id as usize, window, cx);
        });
    }

    fn delete_node(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node) = self.db.node(id) else { return };
        // kit 模态确认（替代原生 window.prompt）：Esc/点遮罩取消。
        let modal = ui_kit::base::modal::ModalHandle::new(cx);
        window.focus(&modal.focus);
        self.confirm_delete = Some(crate::app_ui::ConfirmDelete {
            modal,
            id,
            name: node.name.clone(),
        });
        cx.notify();
    }

    /// 执行删除确认模态的「删除」。
    pub(crate) fn perform_delete(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.confirm_delete.take() {
            self.db.delete(c.id);
            if self.selected == Some(SelectKey::Node(c.id)) {
                self.selected = None;
            }
            self.refresh_tree(cx);
            cx.notify();
        }
    }

    // ----- 连接 / 编辑 -----

    pub(crate) fn connect_node(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(node) = self.db.node(id) else { return };
        let NodeKind::Server(data) = node.kind.clone() else { return };
        let node_name = node.name.clone();
        let title = if node_name.trim().is_empty() { data.host.clone() } else { node_name.clone() };
        let icon = if node.icon.is_empty() {
            crate::icons::icon_for_name(&node_name, "server")
        } else {
            node.icon.clone()
        };
        match server_params(&self.db, &data) {
            Some(params) => self.open_connecting_tab(title, params, Some(icon), cx),
            None => {
                // 缺口令/密钥：打开表单窗口补齐（编辑态，保存即更新该节点）。
                let msg = crate::i18n::t(cx, crate::i18n::Tr::NeedCredential).to_string();
                let load = Some((id, node_name, data, node.icon.clone()));
                self.open_server_form(ROOT_SERVERS, load, Some(msg), cx);
            }
        }
    }

    fn edit_server(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(node) = self.db.node(id) else { return };
        let NodeKind::Server(data) = node.kind.clone() else { return };
        let node_name = node.name.clone();
        let load = Some((id, node_name, data, node.icon.clone()));
        self.open_server_form(ROOT_SERVERS, load, None, cx);
    }

    /// 编辑命令（表单窗口；parent 维持原节点所在树）。
    fn edit_command(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(node) = self.db.node(id) else { return };
        let NodeKind::Command(data) = node.kind.clone() else { return };
        let load = Some((id, node.name.clone(), data));
        self.open_command_form(node.parent, load, cx);
    }

    /// 编辑目录名称（单字段表单窗口）。
    fn edit_folder(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(node) = self.db.node(id) else { return };
        let name = node.name.clone();
        self.open_folder_form(id, name, cx);
    }

    /// 拖拽移动节点到新父目录（拒绝自身/子孙/跨树目标）。
    /// 合法域 = 三棵树互不相通（服务器树 / 通用命令树 / 每服务器一棵专用命令树），
    /// 按**父节点**判定成员身份（server 节点自身是专用命令树根，不算服务器树成员）。
    pub(crate) fn move_node_to(&mut self, id: i64, new_parent: i64, cx: &mut Context<Self>) {
        if id == new_parent || self.db.under_root(new_parent, id) {
            return;
        }
        if self.db.node(id).is_some_and(|n| n.parent == new_parent) {
            return;
        }
        let same_tree = match (self.db.node(id).map(|n| n.parent), self.db.tree_scope(new_parent)) {
            (Some(from_parent), Some(to_scope)) => self.db.tree_scope(from_parent) == Some(to_scope),
            _ => false,
        };
        if !same_tree {
            return;
        }
        self.db.move_node(id, new_parent);
        self.selected = Some(SelectKey::Node(id));
        cx.notify();
    }

    // ----- 分割线拖拽 -----

    fn on_splitter_drag(
        &mut self,
        e: &gpui::DragMoveEvent<SplitDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let y: f32 = e.event.position.y.into();
        let state = e.drag(cx).0.clone();
        let mut anchor = state.lock().expect("split drag lock");
        if !e.event.dragging() {
            *anchor = None;
            return;
        }
        let &mut (start_y, start_h) = anchor.get_or_insert((y, self.shortcut_h));
        // 方向：鼠标上移 → 分割线跟随上移 → 快捷栏变高（VSCode 底栏惯例）。
        let max_h = self.shortcut_max(window);
        let h = (start_h + (start_y - y)).clamp(SHORTCUT_MIN_H, max_h);
        if h != self.shortcut_h {
            self.shortcut_h = h;
            self.db.set_setting(SETTING_SHORTCUT_H, &format!("{h}"));
            cx.notify();
        }
    }

    // ---------- 渲染 ----------

    /// 快捷栏在当前窗口下的可用上限（保证树区 ≥ TREE_MIN_H，不产生负高度）。
    fn shortcut_max(&self, window: &Window) -> f32 {
        let vh: f32 = window.viewport_size().height.into();
        (vh - TITLEBAR_H - SIDEBAR_TOOLBAR_H - SIDEBAR_SPLITTER_H - TREE_MIN_H).max(SHORTCUT_MIN_H)
    }

    pub(crate) fn render_sidebar(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        // 布局说明：gpui 0.2.2 嵌套列布局中 flex_1 不可靠，侧栏内部全部绝对定位。
        let eff_h = self.shortcut_h.min(self.shortcut_max(window));
        let inset_bottom = SIDEBAR_TOOLBAR_H + eff_h;
        let width_handle = self.render_width_handle(cx);
        let win_h: f32 = window.viewport_size().height.into();
        div()
            .id("left-panel")
            .relative()
            .w(px(self.sidebar_w))
            .h(px(win_h))
            .bg(pal.surface(pal.panel))
            .border_r_1()
            .border_color(rgb(pal.divider()))
            .overflow_hidden()
            // macOS 全高侧栏：顶部为交通灯让位区 + 折叠/主题按钮
            .child(
                div()
                    .id("sidebar-top-zone")
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(TITLEBAR_H))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(pal.divider()))
                    .child(div().w(px(TRAFFIC_LIGHTS_W)).flex_none().h_full())
                    // 主题入口已移入设置窗「通用 › 系统 › 界面主题」（原 ◐ 按钮取消）。
                    .child(
                        div()
                            .id("sb-collapse")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(26.))
                            .rounded_md()
                            .bg(pal.surface(pal.card))
                            .border_1()
                            .border_color(rgb(pal.border))
                            .shadow_sm()
                            .text_size(px(13.))
                            .text_color(rgb(pal.muted))
                            .hover(|s| s.bg(rgb(pal.border)).text_color(rgb(pal.text)))
                            .child("◧")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                this.left_open = false;
                                cx.notify();
                            })),
                    ),
            )
            // 树工具栏：简洁/详细 + 展开/收缩（钉在标题区下）
            .child(self.render_tree_toolbar(cx))
            // 树：kit TreeState 组件（自带滚动/重命名/拖拽）
            .child(
                div()
                    .id("sidebar-tree")
                    .absolute()
                    .top(px(TITLEBAR_H + TREE_TOOLBAR_H))
                    .bottom(px(inset_bottom + SIDEBAR_SPLITTER_H))
                    .left_0()
                    .w_full()
                    .child(self.tree.clone()),
            )
            // 分割线：贴在快捷栏顶边
            .child(self.render_splitter(cx, inset_bottom))
            // 快捷栏：贴在工具栏顶边
            .child(self.render_shortcut_bar(SIDEBAR_TOOLBAR_H, eff_h))
            // 工具栏：钉死面板底部
            .child(self.render_sidebar_toolbar(cx))
            // 右缘宽度拖柄（最上层）
            .children(width_handle)
            .into_any_element()
    }

    /// 侧栏右缘宽度拖柄：按下锚定（鼠标 x、起始宽度），拖动实时改宽并持久化。
    fn render_width_handle(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let drag = WidthDrag(self.width_drag.clone());
        Some(
            div()
                .id("sidebar-width-handle")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(5.))
                .cursor(CursorStyle::ResizeLeftRight)
                .occlude()
                .hover(|s| s.bg(gpui::rgba(0x7aa2f744)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, _w, _cx| {
                        let x: f32 = ev.position.x.into();
                        *this.width_drag.lock().expect("width drag lock") = Some((x, this.sidebar_w));
                    }),
                )
                .on_drag(drag, move |_, _, _, cx| cx.new(|_| SplitterGhost))
                .on_drag_move(cx.listener(|this, e: &gpui::DragMoveEvent<WidthDrag>, _w, cx| {
                    this.on_width_drag(e, cx);
                }))
                .into_any_element(),
        )
    }

    fn on_width_drag(&mut self, e: &gpui::DragMoveEvent<WidthDrag>, cx: &mut Context<Self>) {
        if !e.event.dragging() {
            *self.width_drag.lock().expect("width drag lock") = None;
            return;
        }
        let x: f32 = e.event.position.x.into();
        let state = e.drag(cx).0.clone();
        let mut anchor = state.lock().expect("width drag lock");
        let &mut (start_x, start_w) = anchor.get_or_insert((x, self.sidebar_w));
        let w = (start_w + (x - start_x)).clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
        if w != self.sidebar_w {
            self.sidebar_w = w;
            self.db.set_setting("sidebar.width", &format!("{w}"));
            cx.notify();
        }
    }

    fn render_splitter(&self, cx: &Context<Self>, bottom: f32) -> AnyElement {
        let pal = self.theme();
        let drag = SplitDrag(self.split_drag.clone());
        div()
            .id("sidebar-splitter")
            .absolute()
            .bottom(px(bottom))
            .left_0()
            .w_full()
            .h(px(SIDEBAR_SPLITTER_H))
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
                    *this.split_drag.lock().expect("split drag lock") = Some((y, this.shortcut_h));
                }),
            )
            .on_drag(drag, move |_, _, _, cx| cx.new(|_| SplitterGhost))
            .on_drag_move(cx.listener(|this, e: &gpui::DragMoveEvent<SplitDrag>, w, cx| {
                this.on_splitter_drag(e, w, cx);
            }))
            .into_any_element()
    }

    /// 快捷栏：定高占位容器，底边钉在工具栏顶，高度由分割线拖拽决定。
    fn render_shortcut_bar(&self, bottom: f32, h: f32) -> AnyElement {
        let pal = self.theme();
        div()
            .id("sidebar-shortcuts")
            .absolute()
            .bottom(px(bottom))
            .left_0()
            .w_full()
            .h(px(h))
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px_3()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(rgb(pal.dim))
                    .child(crate::i18n::tl(self.loc, crate::i18n::Tr::Shortcuts)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(pal.dim))
                            .child(crate::i18n::tl(self.loc, crate::i18n::Tr::ShortcutEmpty)),
                    ),
            )
            .into_any_element()
    }

    // ----- 底部工具栏 -----

    /// 底部工具栏：内容**暂全部取消**（＋/⏱︎/★/▶ 移除，条带保留占位——
    /// 快捷栏/分割线的布局锚点不动，后续重设计再放内容）。
    fn render_sidebar_toolbar(&self, _cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        div()
            .id("sidebar-toolbar")
            .absolute()
            .bottom_0()
            .left_0()
            .h(px(SIDEBAR_TOOLBAR_H))
            .w_full()
            .flex()
            .items_center()
            .px_2()
            .bg(pal.surface(pal.bg))
            .border_t_1()
            .border_color(rgb(pal.divider()))
            .occlude()
            // 左下角常驻版本号（关于对话框之外的日常可见位）。
            .child(
                div()
                    .text_size(px(9.5))
                    .text_color(rgb(pal.dim))
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
            .into_any_element()
    }

    // ----- 右键菜单浮层（kit dropdown） -----

    pub(crate) fn render_menu_overlay(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let entries = self.menu_entries(&menu.target);
        // 带色块预览的配色菜单加宽：长名 + 预览条并排不挤压。
        let has_preview = entries
            .iter()
            .any(|e| matches!(e, MenuEntry::Item { trailing: Some(_), .. }));
        let menu_w = if has_preview { kit_menu::MENU_W + 40. } else { kit_menu::MENU_W };
        kit_menu::overlay(
            entries,
            menu.origin,
            menu_w,
            self.theme().kit(),
            window,
            cx,
            |this, action, window, cx| this.run_menu_action(action, window, cx),
            |this, _window, cx| this.close_menu(cx),
        )
    }

    fn menu_entries(&self, target: &MenuTarget) -> Vec<MenuEntry<MenuAction>> {
        // 带图标（svg 资源路径 = icons::asset_path）
        let ic = crate::icons::asset_path;
        let loc = self.loc;
        let tr = |k: crate::i18n::Tr| crate::i18n::tl(loc, k);
        let iitem =
            |icon: &str, label: &str, action: MenuAction| kit_menu::icon_item(&ic(icon), label, action);
        let idanger =
            |icon: &str, label: &str, action: MenuAction| kit_menu::icon_danger_item(&ic(icon), label, action);
        let sep = MenuEntry::Sep;

        match target {
            MenuTarget::New => vec![
                iitem("server", tr(crate::i18n::Tr::MenuNewServer), MenuAction::NewServer(ROOT_SERVERS)),
                iitem("terminal-square", tr(crate::i18n::Tr::MenuNewCommand), MenuAction::NewCommand(ROOT_COMMANDS)),
                iitem("folder-plus", tr(crate::i18n::Tr::MenuNewFolder), MenuAction::NewFolder(ROOT_SERVERS)),
            ],
            MenuTarget::GroupServers => vec![
                iitem("server", tr(crate::i18n::Tr::MenuNewServer), MenuAction::NewServer(ROOT_SERVERS)),
                iitem("folder-plus", tr(crate::i18n::Tr::MenuNewFolder), MenuAction::NewFolder(ROOT_SERVERS)),
            ],
            MenuTarget::GroupCommands => vec![
                iitem("terminal-square", tr(crate::i18n::Tr::MenuNewCommand), MenuAction::NewCommand(ROOT_COMMANDS)),
                iitem("folder-plus", tr(crate::i18n::Tr::MenuNewFolder), MenuAction::NewFolder(ROOT_COMMANDS)),
            ],
            // 专用命令组：新增的命令/目录挂在当前连接服务器节点下。
            MenuTarget::GroupServerCommands(sid) => vec![
                iitem("terminal-square", tr(crate::i18n::Tr::MenuNewCommand), MenuAction::NewCommand(*sid)),
                iitem("folder-plus", tr(crate::i18n::Tr::MenuNewFolder), MenuAction::NewFolder(*sid)),
            ],
            // 专用命令组空态：未连接无处挂载，只给禁用提示。
            MenuTarget::GroupServerCommandsIdle => vec![kit_menu::icon_disabled_item(
                &ic("terminal-square"),
                tr(crate::i18n::Tr::MenuNewCommandDisabled),
                MenuAction::OpenDataDir,
            )],
            MenuTarget::Tab(id) => {
                let live = self
                    .tabs
                    .iter()
                    .any(|t| t.id == *id && matches!(t.kind, TabKind::Live { .. }));
                if live {
                    vec![iitem("external-link", tr(crate::i18n::Tr::MenuPopTab), MenuAction::PopTab(*id))]
                } else {
                    vec![kit_menu::icon_disabled_item(
                        &ic("external-link"),
                        tr(crate::i18n::Tr::MenuPopTabDisabled),
                        MenuAction::OpenDataDir,
                    )]
                }
            }
            MenuTarget::Node(id) => match self.db.node(*id).map(|n| (&n.kind, n.id)) {
                Some((NodeKind::Server(_), nid)) => vec![
                    iitem("play", tr(crate::i18n::Tr::MenuConnect), MenuAction::Connect(nid)),
                    iitem("pencil", tr(crate::i18n::Tr::MenuEdit), MenuAction::EditServer(nid)),
                    sep,
                    idanger("trash-2", tr(crate::i18n::Tr::Delete), MenuAction::Delete(nid)),
                ],
                // 文件夹菜单按所在树分流：服务器树给「新增服务器」，命令树（通用/专用）给「新增命令」。
                // 内联重命名已取消，目录改名走「编辑…」表单。
                Some((NodeKind::Folder, nid)) => {
                    let scope = self.db.tree_scope(self.db.node(nid).map(|n| n.parent).unwrap_or(ROOT_SERVERS));
                    let mut v = Vec::new();
                    match scope {
                        Some(TreeScope::Servers) => {
                            v.push(iitem("server", tr(crate::i18n::Tr::MenuNewServer), MenuAction::NewServer(nid)));
                        }
                        Some(TreeScope::GeneralCommands) | Some(TreeScope::ServerCommands(_)) => {
                            v.push(iitem("terminal-square", tr(crate::i18n::Tr::MenuNewCommand), MenuAction::NewCommand(nid)));
                        }
                        None => {}
                    }
                    v.push(iitem("folder-plus", tr(crate::i18n::Tr::MenuNewSubfolder), MenuAction::NewFolder(nid)));
                    v.push(iitem("pencil", tr(crate::i18n::Tr::MenuEdit), MenuAction::EditFolder(nid)));
                    v.push(sep);
                    v.push(idanger("trash-2", tr(crate::i18n::Tr::Delete), MenuAction::Delete(nid)));
                    v
                }
                Some((NodeKind::Command(_), nid)) => vec![
                    iitem("play", tr(crate::i18n::Tr::MenuRunConsole), MenuAction::RunCommand(nid)),
                    iitem("pencil", tr(crate::i18n::Tr::MenuEdit), MenuAction::EditCommand(nid)),
                    sep,
                    idanger("trash-2", tr(crate::i18n::Tr::Delete), MenuAction::Delete(nid)),
                ],
                None => vec![],
            },
            // 目录树传输入口：文件 → 下载；目录/空白 → 上传到该目录。
            MenuTarget::DirFile(_) => vec![iitem(
                "download",
                tr(crate::i18n::Tr::MenuDownload),
                MenuAction::DirDownload(match target {
                    MenuTarget::DirFile(p) => p.clone(),
                    _ => unreachable!(),
                }),
            )],
            MenuTarget::DirDir(_) | MenuTarget::DirBlank(_) => vec![iitem(
                "upload",
                tr(crate::i18n::Tr::MenuUploadHere),
                MenuAction::DirUpload(match target {
                    MenuTarget::DirDir(p) | MenuTarget::DirBlank(p) => p.clone(),
                    _ => unreachable!(),
                }),
            )],
        }
    }
}

// ---------- 辅助 ----------

/// 两色线性插值（0xRRGGBB；t=0 → a，t=1 → b）。激活行底色调用。
fn mix_rgb(a: u32, b: u32, t: f32) -> u32 {
    let ch = |x: u32, s: u32| ((x >> s) & 0xff) as f32;
    let mix_ch = |s: u32| {
        let v = ch(a, s) + (ch(b, s) - ch(a, s)) * t;
        (v.round() as u32).min(0xff) << s
    };
    mix_ch(16) | mix_ch(8) | mix_ch(0)
}

/// 服务器数据 → 连接参数；口令/密钥缺失时返回 None 由调用方处理。
/// iroh 模式的 relay/令牌取全局设置（设置 › 通用 › 网络）。
pub(crate) fn server_params(db: &crate::db::Db, data: &ServerData) -> Option<ConnectParams> {
    let auth = match &data.auth {
        StoredAuth::Password(Some(pw)) => AuthMethod::Password(pw.clone()),
        StoredAuth::Password(None) => return None,
        StoredAuth::Key { key, passphrase } => AuthMethod::Key {
            key: key.clone(),
            passphrase: passphrase.clone(),
        },
    };
    let transport = match data.mode {
        crate::db::ConnectMode::Ip => sshrust_core::Transport::Tcp,
        crate::db::ConnectMode::Iroh => {
            let (relay, token) = crate::db::iroh_global(db);
            sshrust_core::Transport::Iroh { relay, token }
        }
    };
    Some(ConnectParams {
        host: data.host.clone(),
        port: data.port,
        username: data.username.clone(),
        auth,
        cols: 100,
        rows: 30,
        transport,
    })
}

fn open_data_dir() {
    let mut p = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()));
    p.push(".sshrust");
    let _ = std::process::Command::new("open").arg(p).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_id_mappings() {
        assert_eq!(db_id_for(5), Some(5));
        assert_eq!(db_id_for(KIT_HEADER_SERVERS), None);
        assert_eq!(db_id_for(KIT_GROUP_SERVERS), None);
        assert!(matches!(menu_target_for(Some(3)), MenuTarget::Node(3)));
        assert!(matches!(menu_target_for(Some(KIT_GROUP_SERVERS)), MenuTarget::GroupServers));
        assert!(matches!(menu_target_for(None), MenuTarget::New));
    }

    #[test]
    fn mix_rgb_interpolates() {
        assert_eq!(mix_rgb(0x1a1b26, 0x1a1b26, 0.13), 0x1a1b26, "同色混合不变");
        assert_eq!(mix_rgb(0x000000, 0xffffff, 0.), 0x000000);
        assert_eq!(mix_rgb(0x000000, 0xffffff, 1.), 0xffffff);
        assert_eq!(mix_rgb(0x101010, 0xf0f0f0, 0.5), 0x808080, "中点取半");
    }
}

// ---------- 进程内流程回归（沙箱 HOME，不触真实库） ----------

#[cfg(test)]
mod flow_tests {
    use super::*;
    use crate::app_ui::AppUi;
    use crate::hosts::StoredAuth;
    use gpui::TestAppContext;

    fn sandbox_home() {
        // 每个测试独立目录（并行运行互不踩库；同一进程内 pid 相同需再分段）。
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sshrust_flow_{}_{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Rust 2024: set_var 为 unsafe。
        unsafe { std::env::set_var("HOME", dir) };
    }

    /// 删除：菜单 → 确认模态 → perform_delete，级联子节点。
    #[gpui::test]
    fn delete_menu_modal_flow(cx: &mut TestAppContext) {
        sandbox_home();
        let (ui, cx) = cx.add_window_view(|_w, cx| AppUi::new(cx));
        cx.run_until_parked();

        let (fid, sid) = cx.update(|_w, app| ui.update(app, |u, _| {
            let folder = u.db.insert(NewNode { parent: ROOT_SERVERS, kind: NodeKind::Folder, name: "TF".into() });
            let server = u.db.insert(NewNode {
                parent: folder.id,
                kind: NodeKind::Server(ServerData { host: "1.2.3.4".into(), port: 22, username: "u".into(), auth: StoredAuth::Password(None), mode: crate::db::ConnectMode::Ip, iroh_relay: String::new(), iroh_token: String::new(), spec: String::new() }),
                name: "TS".into(),
            });
            (folder.id, server.id)
        }));

        // 菜单删除 → 弹确认
        cx.update(|w, app| {
            ui.update(app, |u, cx| u.run_menu_action(MenuAction::Delete(fid), w, cx))
        });
        cx.run_until_parked();
        let modal_up = cx.update(|_w, app| ui.update(app, |u, _| u.confirm_delete.is_some()));
        assert!(modal_up, "点菜单「删除」后应弹确认模态");

        // 确认 → 级联删除 + 刷新
        cx.update(|_w, app| ui.update(app, |u, cx| u.perform_delete(cx)));
        cx.run_until_parked();
        let gone = cx.update(|_w, app| {
            ui.update(app, |u, _| u.db.node(fid).is_none() && u.db.node(sid).is_none())
        });
        assert!(gone, "确认后节点与子节点应一并删除");
    }

    /// 删除点击级全链路：菜单点「删除」→ 模态点「删除」→ 节点级联消失。
    #[gpui::test]
    fn delete_click_through(cx: &mut TestAppContext) {
        sandbox_home();
        let (ui, cx) = cx.add_window_view(|_w, cx| AppUi::new(cx));
        cx.run_until_parked();

        let fid = cx.update(|_w, app| {
            ui.update(app, |u, cx| {
                let folder = u.db.insert(NewNode { parent: ROOT_SERVERS, kind: NodeKind::Folder, name: "TF".into() });
                u.db.insert(NewNode {
                    parent: folder.id,
                    kind: NodeKind::Server(ServerData { host: "1.2.3.4".into(), port: 22, username: "u".into(), auth: StoredAuth::Password(None), mode: crate::db::ConnectMode::Ip, iroh_relay: String::new(), iroh_token: String::new(), spec: String::new() }),
                    name: "TS".into(),
                });
                u.refresh_tree(cx);
                folder.id
            })
        });
        cx.run_until_parked();

        // 右键目标：直接开菜单（ContextMenu 事件逻辑另有覆盖）
        cx.update(|_w, app| {
            ui.update(app, |u, cx| {
                u.open_menu(MenuTarget::Node(fid), gpui::Point::new(gpui::px(40.), gpui::px(60.)), cx)
            })
        });
        cx.run_until_parked();

        // 文件夹（servers 根下）菜单项序：0 新增服务器 / 1 新增子文件夹 / 2 重命名 / 3 分隔 / 4 删除
        let del = cx.debug_bounds("menu-item-4").expect("菜单「删除」项可见");
        let c = gpui::Point::new(del.origin.x + del.size.width / 2., del.origin.y + del.size.height / 2.);
        cx.simulate_mouse_move(c, None, Default::default());
        cx.simulate_mouse_down(c, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_up(c, gpui::MouseButton::Left, Default::default());
        cx.run_until_parked();

        let modal_up = cx.update(|_w, app| ui.update(app, |u, _| u.confirm_delete.is_some()));
        assert!(modal_up, "点菜单「删除」应弹确认模态");

        let btn = cx.debug_bounds("confirm-delete-btn").expect("模态「删除」按钮可见");
        let c = gpui::Point::new(btn.origin.x + btn.size.width / 2., btn.origin.y + btn.size.height / 2.);
        let _ = &btn;
        cx.simulate_mouse_move(c, None, Default::default());
        cx.simulate_mouse_down(c, gpui::MouseButton::Left, Default::default());
        cx.simulate_mouse_up(c, gpui::MouseButton::Left, Default::default());
        cx.run_until_parked();
        let gone = cx.update(|_w, app| ui.update(app, |u, _| u.db.node(fid).is_none()));
        assert!(gone, "模态确认后节点应被删除");
    }

    /// 目录改名（内联重命名已取消）：菜单「编辑…」→ 表单窗口 → persist 落库。
    #[gpui::test]
    fn folder_edit_form_flow(cx: &mut TestAppContext) {
        sandbox_home();
        let (ui, cx) = cx.add_window_view(|_w, cx| AppUi::new(cx));
        cx.run_until_parked();

        let fid = cx.update(|_w, app| ui.update(app, |u, _| {
            u.db.insert(NewNode { parent: ROOT_SERVERS, kind: NodeKind::Folder, name: "TF".into() }).id
        }));

        // 菜单「编辑…」→ 打开目录表单窗口。
        cx.update(|w, app| {
            ui.update(app, |u, cx| u.run_menu_action(MenuAction::EditFolder(fid), w, cx))
        });
        cx.run_until_parked();
        let form_open = cx.update(|_w, app| ui.update(app, |u, _| u.folder_form_win.is_some()));
        assert!(form_open, "点目录「编辑…」应打开表单窗口");

        // 表单保存 → 就地改名（id/parent 不变）。
        cx.update(|_w, app| {
            ui.update(app, |u, cx| {
                u.persist_folder_form(
                    crate::form_folder_edit::FolderFormOutput { id: fid, name: "改名后".into() },
                    cx,
                )
            })
        });
        cx.run_until_parked();
        let renamed = cx.update(|_w, app| {
            ui.update(app, |u, _| {
                u.db.node(fid).map(|n| n.name == "改名后" && n.parent == ROOT_SERVERS).unwrap_or(false)
            })
        });
        assert!(renamed, "目录表单保存应就地改名且不动 parent");
    }

    /// 常用命令：表单产物落库（通用/专用两种挂载点）+ 说明字段回读 +
    /// 无终端运行出浮条 + 跨树拖拽被拒（三棵树互不相通）。
    #[gpui::test]
    fn command_crud_scope_and_run_notice(cx: &mut TestAppContext) {
        sandbox_home();
        let (ui, cx) = cx.add_window_view(|_w, cx| AppUi::new(cx));
        cx.run_until_parked();

        let sid = cx.update(|_w, app| {
            ui.update(app, |u, _| {
                u.db.insert(NewNode {
                    parent: ROOT_SERVERS,
                    kind: NodeKind::Server(ServerData { host: "1.2.3.4".into(), port: 22, username: "u".into(), auth: StoredAuth::Password(None), mode: crate::db::ConnectMode::Ip, iroh_relay: String::new(), iroh_token: String::new(), spec: String::new() }),
                    name: "TS".into(),
                })
                .id
            })
        });

        let persist = |parent: i64, title: &str, desc: &str, cmd: &str| {
            crate::form_command_edit::CommandFormOutput {
                editing: None,
                parent,
                title: title.into(),
                description: desc.into(),
                command: cmd.into(),
            }
        };

        // 通用命令：落库到通用树根，说明/命令回读一致。
        cx.update(|_w, app| {
            ui.update(app, |u, cx| {
                u.persist_command_form(persist(ROOT_COMMANDS, "磁盘", "查看空间", "df -h"), cx)
            })
        });
        // 专用命令：落库到服务器节点下（专用命令树根）。
        cx.update(|_w, app| {
            ui.update(app, |u, cx| u.persist_command_form(persist(sid, "负载", "uptime", "uptime"), cx))
        });
        cx.run_until_parked();

        let (gid, scid) = cx.update(|_w, app| {
            ui.update(app, |u, _| {
                let gid = u.db.children(ROOT_COMMANDS).iter().find(|n| n.name == "磁盘").map(|n| n.id).expect("通用命令已落库");
                let scid = u.db.children(sid).iter().find(|n| n.name == "负载").map(|n| n.id).expect("专用命令挂服务器节点下");
                let NodeKind::Command(d) = &u.db.node(gid).unwrap().kind else { panic!("command") };
                assert_eq!((d.command.as_str(), d.description.as_str()), ("df -h", "查看空间"));
                (gid, scid)
            })
        });

        // 跨树拖拽：通用命令 → 服务器节点（专用根）、专用命令 → 通用根，均拒绝。
        cx.update(|_w, app| {
            ui.update(app, |u, cx| {
                u.move_node_to(gid, sid, cx);
                u.move_node_to(scid, ROOT_COMMANDS, cx);
            })
        });
        let kept = cx.update(|_w, app| {
            ui.update(app, |u, _| {
                u.db.node(gid).unwrap().parent == ROOT_COMMANDS && u.db.node(scid).unwrap().parent == sid
            })
        });
        assert!(kept, "命令不得跨树移动（通用 ↔ 专用 ↔ 服务器树互不相通）");

        // 无打开的控制台：运行命令 → 顶部浮条提示（双击 Activate 同链路不炸）。
        cx.update(|_w, app| ui.update(app, |u, cx| u.run_command_node(gid, cx)));
        cx.run_until_parked();
        let noticed = cx.update(|_w, app| ui.update(app, |u, _| u.notice.is_some()));
        assert!(noticed, "无 Live 终端时运行命令应出浮条提示");
    }
}
