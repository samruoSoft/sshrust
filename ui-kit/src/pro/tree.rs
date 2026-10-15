//! 树组件（pro 系列）：自渲染实体 `TreeState`。
//!
//! ## 能力（已实现功能组件内封闭，事件对外）
//! - 行型：小节标题（Header）/ 可展开分组（Group）/ 叶节点（Leaf）；
//! - 展开/收起（点击分组行）、单选、双击激活（Activate）；
//! - 右键菜单事件（行/空白区分别携带 id 与坐标）；
//! - 内联重命名：`begin_rename(id)`，回车提交 Rename 事件、Esc 取消；
//! - 节点拖拽移动：拖起浮签跟随，droppable 行高亮，放下发 Move 事件
//!   （合法性校验由宿主负责）；
//! - 内嵌滚动：复用 [`crate::pro::scroll_area::ScrollArea`] 真实模式。
//!
//! ## 颜色（要求：图标/文字/底色可单独设置）
//! 每节点 `NodeColors { icon, text, bg }` 三槽独立覆写（常态生效）；
//! 选中态统一用主题选中样式（accent 底 + accent_dark 字）保证辨识，
//! 覆写不作用于选中态。
//!
//! ## 布局语义
//! 展开箭头画在**缩进区内**（gutter 槽位），不占行内容空间：同层叶/分组的
//! 图标与标题严格左对齐，内容起点 = 首列基准 + 层级 × 步长，逐级右移一个步长。
//! 标题（label）**优先保留**；行尾信息（suffix，如 IP:端口）占弹性余量、
//! 宽度不足时先收缩省略（与"别名收缩"的旧实现相反）；状态点在行尾信息前。
//!
//! ## 事件（`cx.subscribe(&tree, …)`）
//! `Select / Activate / Toggle / ContextMenu / Rename / Move`
//!
//! 数据由宿主 `set_nodes()` 推入（业务树自持，组件不落盘）。

use std::collections::HashSet;

use gpui::{
    actions, prelude::*, AnyElement, App, ClickEvent, Context, Element, Entity, EventEmitter,
    FocusHandle, GlobalElementId, Hsla, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, SharedString, ShapedLine, Styled, TextRun, Window, div,
    px, rgb,
};

use crate::base::text_input::{InputField, InputSize, TextField, TextFieldEvent};
use crate::pro::scroll_area::{ScrollArea, ScrollAreaState};
use crate::Theme;

actions!(tree, [TreeEscape]);

/// 展开箭头槽宽：箭头画在缩进区内（gutter），不占行内容空间。
const ARROW_W: f32 = 12.;
/// 行内相邻元素间距（body 的 gap_1）。
const ITEM_GAP: f32 = 4.;
/// 首列内容基准：箭头槽（槽宽 + 前后各一档间距）。
const BASE_INDENT: f32 = ARROW_W + 2. * ITEM_GAP;

/// 注册树组件键位（内联重命名的 Esc 取消；仅在树上下文内生效）。
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([gpui::KeyBinding::new(
        "escape",
        TreeEscape,
        Some("ui_kit_tree"),
    )]);
}

/// 行型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKind {
    /// 小节标题（不可选、不可拖、不参与事件）。
    Header,
    /// 可展开分组（可作为拖放目标，见 `droppable`）。
    Group,
    /// 叶节点。
    Leaf,
}

/// 节点级颜色覆写（None = 用主题默认；仅常态生效，选中态用主题选中样式）。
#[derive(Clone, Copy, Default, Debug)]
pub struct NodeColors {
    pub icon: Option<u32>,
    pub text: Option<u32>,
    pub bg: Option<u32>,
}

/// 行尾信息的溢出省略模式（同一 label 的两种显示效果）。
/// Tail：尾部省略（默认，`text_ellipsis`）；Middle：两头保留、中间 `…`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EllipsisMode {
    #[default]
    Tail,
    Middle,
}

/// 一个树节点（业务数据快照，宿主每次变更后整体推入）。
#[derive(Clone)]
pub struct TreeNodeData {
    pub id: usize,
    pub kind: RowKind,
    pub label: String,
    /// svg 资源路径（None = 不画图标）。
    pub icon: Option<gpui::SharedString>,
    /// 展开态图标（None = 沿用 icon；仅 Group 生效）。
    pub icon_open: Option<gpui::SharedString>,
    /// 行尾信息（宽度不足时收缩省略；如 IP:端口）。
    pub suffix: Option<String>,
    /// 行尾信息溢出省略模式（默认尾部省略）。
    pub suffix_ellipsis: EllipsisMode,
    /// 状态点颜色（行尾信息前的小圆点；None = 不画）。
    pub dot: Option<u32>,
    pub colors: NodeColors,
    pub children: Vec<TreeNodeData>,
    /// 初始展开（仅在组件首次渲染时播种；此后以内部状态为准）。
    pub expanded: bool,
    /// 可作为拖放目标（分组/根）。
    pub droppable: bool,
}

impl TreeNodeData {
    pub fn new(id: usize, kind: RowKind, label: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            label: label.into(),
            icon: None,
            icon_open: None,
            suffix: None,
            suffix_ellipsis: EllipsisMode::default(),
            dot: None,
            colors: NodeColors::default(),
            children: Vec::new(),
            expanded: false,
            droppable: false,
        }
    }

    pub fn leaf(id: usize, label: impl Into<String>) -> Self {
        Self::new(id, RowKind::Leaf, label)
    }

    pub fn group(id: usize, label: impl Into<String>) -> Self {
        let mut n = Self::new(id, RowKind::Group, label);
        n.droppable = true;
        n
    }

    pub fn header(id: usize, label: impl Into<String>) -> Self {
        Self::new(id, RowKind::Header, label)
    }
}

/// 组件事件。
#[derive(Clone, Debug)]
pub enum TreeEvent {
    /// 单击选中（Header 不发）。
    Select { id: usize },
    /// 双击激活（如连接服务器）。
    Activate { id: usize },
    /// 展开/收起变化。
    Toggle { id: usize, expanded: bool },
    /// 右键：Some(id)=行上，None=空白区；position 为窗口坐标（弹菜单用）。
    ContextMenu { id: Option<usize>, position: Point<Pixels> },
    /// 内联重命名提交（回车；Esc/失焦不提交）。
    Rename { id: usize, name: String },
    /// 拖拽移动请求（from → to 的 id；合法性由宿主校验）。
    Move { from: usize, to: usize },
}

/// 拖拽标记（值为节点 id）。
#[derive(Clone, Copy)]
struct TreeDrag(usize);

/// 拖拽跟随浮签：图标 + 标题。
struct TreeDragGhost {
    label: gpui::SharedString,
    icon: Option<gpui::SharedString>,
    theme: Theme,
}

impl Render for TreeDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut row = div()
            .id("tree-drag-ghost")
            .flex()
            .items_center()
            .gap_1()
            .h(px(24.))
            .px_2()
            .rounded_md()
            .bg(rgb(t.card))
            // 浮层不吃窗口模糊：底色不透明（自身 0.92 元素透明度保留）。
            .border_1()
            .border_color(rgb(t.accent))
            .shadow_sm()
            .opacity(0.92)
            .text_size(px(12.))
            .text_color(rgb(t.text));
        if let Some(icon) = self.icon.clone() {
            row = row.child(
                gpui::svg()
                    .path(icon)
                    .size(px(12.))
                    .flex_none()
                    .text_color(rgb(t.accent)),
            );
        }
        row.child(div().child(self.label.clone()))
    }
}

/// 中间省略（纯函数）：整体放得下则原样返回；否则保留头+尾字符、
/// 中间以 `…` 顶替，头部约占保留量的 2/3，从最大保留量往下找首个放得下的组合。
/// `width_of` 由调用方注入测宽（渲染时用 text_system，测试用伪测宽）。
pub fn middle_ellipsize(text: &str, avail: f32, width_of: impl Fn(&str) -> f32) -> String {
    if text.is_empty() || width_of(text) <= avail {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    for keep in (2..n).rev() {
        let head = keep * 2 / 3;
        let tail = keep - head;
        if head == 0 || tail == 0 {
            continue;
        }
        let mut candidate: String = chars[..head].iter().collect();
        candidate.push('…');
        candidate.extend(chars[n - tail..].iter());
        if width_of(&candidate) <= avail {
            return candidate;
        }
    }
    "…".to_string()
}

/// 扁平整行（纯函数，测试用）：按展开集收集可见行 (id, depth)。
pub fn flatten_visible(nodes: &[TreeNodeData], expanded: &HashSet<usize>) -> Vec<(usize, usize)> {
    fn walk(nodes: &[TreeNodeData], depth: usize, expanded: &HashSet<usize>, out: &mut Vec<(usize, usize)>) {
        for n in nodes {
            out.push((n.id, depth));
            if !n.children.is_empty() && expanded.contains(&n.id) {
                walk(&n.children, depth + 1, expanded, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, expanded, &mut out);
    out
}

/// 树状态实体（自渲染组件）。数据经 [`TreeState::set_nodes`] 推入。
struct MiddleLabelPrepaint {
    line: ShapedLine,
}

/// 中间省略 label（自绘元素）：布局期占 flex 弹性余量（flex_grow=1, min_w=0），
/// prepaint 拿到实际分配宽度后按 `middle_ellipsize` 截断，paint 右对齐绘制。
/// 仍是“一个 label”，只是显示效果为两头保留、中间 `…`。
struct MiddleLabel {
    text: SharedString,
    font_size: Pixels,
    color: Hsla,
    line_h: Pixels,
}

impl IntoElement for MiddleLabel {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for MiddleLabel {
    type RequestLayoutState = ();
    type PrepaintState = MiddleLabelPrepaint;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.flex_grow = 1.;
        style.flex_shrink = 1.;
        style.min_size.width = px(0.).into();
        style.size.height = self.line_h.into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> MiddleLabelPrepaint {
        let font = window.text_style().font();
        let shape = |t: &str| -> ShapedLine {
            let run = TextRun {
                len: t.len(),
                font: font.clone(),
                color: self.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(t.to_string().into(), self.font_size, &[run], None)
        };
        let avail = f32::from(bounds.size.width);
        let display = middle_ellipsize(&self.text, avail, |t| f32::from(shape(t).width));
        let line = shape(&display);
        MiddleLabelPrepaint { line }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut MiddleLabelPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        // 右对齐：与尾部省略模式的 justify_end 视觉一致。
        let x = bounds.origin.x + bounds.size.width - prepaint.line.width;
        prepaint
            .line
            .paint(gpui::point(x, bounds.origin.y), self.line_h, window, cx)
            .unwrap();
    }
}

pub struct TreeState {
    nodes: Vec<TreeNodeData>,
    expanded: HashSet<usize>,
    selected: Option<usize>,
    editing: Option<(usize, Entity<TextField>)>,
    scroll: Entity<ScrollAreaState>,
    theme: Theme,
    row_h: f32,
    indent: f32,
    /// 首次渲染播种初始展开后置位。
    seeded: bool,
}

impl EventEmitter<TreeEvent> for TreeState {}

impl TreeState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let scroll = cx.new(|_| ScrollAreaState::new(crate::pro::scroll_area::next_owner()));
        // 滚动条 hover/拖拽状态联动自身重绘，宿主无需关心。
        cx.observe(&scroll, |_, _, cx| cx.notify()).detach();
        Self {
            nodes: Vec::new(),
            expanded: HashSet::new(),
            selected: None,
            editing: None,
            scroll,
            theme: Theme::DARK,
            row_h: 26.,
            indent: 14.,
            seeded: false,
        }
    }

    /// 推入业务树数据（变更后调用；保留展开/选中状态）。
    pub fn set_nodes(&mut self, nodes: Vec<TreeNodeData>, cx: &mut Context<Self>) {
        self.nodes = nodes;
        cx.notify();
    }

    /// 覆写展开集（宿主持久化恢复用）。
    pub fn set_expanded(&mut self, ids: impl IntoIterator<Item = usize>, cx: &mut Context<Self>) {
        self.expanded = ids.into_iter().collect();
        self.seeded = true; // 宿主显式接管后不再播种默认值
        cx.notify();
    }

    /// 当前展开的节点 id 集（宿主持久化用）。
    pub fn expanded_ids(&self) -> Vec<usize> {
        self.expanded.iter().copied().collect()
    }

    pub fn is_expanded(&self, id: usize) -> bool {
        self.expanded.contains(&id)
    }

    /// 主动展开某节点（宿主控制动态出现的分组用；不影响其余展开状态）。
    pub fn expand(&mut self, id: usize, cx: &mut Context<Self>) {
        self.expanded.insert(id);
        cx.notify();
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn set_selected(&mut self, id: Option<usize>, cx: &mut Context<Self>) {
        self.selected = id;
        cx.notify();
    }

    /// 进入内联重命名（回车提交 Rename 事件；Esc 取消）。
    pub fn begin_rename(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(label) = self.find_node(id).map(|n| n.label.clone()) else { return };
        let field = cx.new(|cx| {
            let mut f = TextField::new("", crate::base::text_input::InputType::Text, cx);
            f.set_value(label, cx);
            f
        });
        // 回车 → 提交 Rename。
        cx.subscribe(&field, move |this: &mut TreeState, _, ev: &TextFieldEvent, cx| {
            if matches!(ev, TextFieldEvent::PressEnter) {
                if let Some((rid, field)) = this.editing.take() {
                    let name = field.read(cx).value().trim().to_string();
                    if !name.is_empty() {
                        cx.emit(TreeEvent::Rename { id: rid, name });
                    }
                    cx.notify();
                }
            }
        })
        .detach();
        let handle: FocusHandle = field.read(cx).focus_handle.clone();
        window.focus(&handle);
        self.editing = Some((id, field));
        cx.notify();
    }

    /// 注入配色。
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// 行高 / 缩进模板。
    pub fn set_metrics(&mut self, row_h: f32, indent: f32, cx: &mut Context<Self>) {
        self.row_h = row_h;
        self.indent = indent;
        cx.notify();
    }

    fn find_node(&self, id: usize) -> Option<&TreeNodeData> {
        fn walk(nodes: &[TreeNodeData], id: usize) -> Option<&TreeNodeData> {
            for n in nodes {
                if n.id == id {
                    return Some(n);
                }
                if let Some(found) = walk(&n.children, id) {
                    return Some(found);
                }
            }
            None
        }
        walk(&self.nodes, id)
    }
}

impl Render for TreeState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 首次渲染播种各节点默认展开。
        if !self.seeded {
            fn seed(nodes: &[TreeNodeData], out: &mut HashSet<usize>) {
                for n in nodes {
                    if n.expanded {
                        out.insert(n.id);
                    }
                    seed(&n.children, out);
                }
            }
            let mut s = HashSet::new();
            seed(&self.nodes, &mut s);
            self.expanded = s;
            self.seeded = true;
        }
        let theme = self.theme;
        let rows = self.render_rows(cx);
        div()
            .id("ui-kit-tree")
            .key_context("ui_kit_tree")
            .size_full()
            .on_action(cx.listener(|this, _: &TreeEscape, _w, cx| {
                if this.editing.take().is_some() {
                    cx.notify();
                }
            }))
            .child(
                ScrollArea::new(self.scroll.clone(), div().pt_1().children(rows).on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                        // 空白区右键（行内右键已 stop_propagation）。
                        let _ = this;
                        cx.emit(TreeEvent::ContextMenu { id: None, position: ev.position });
                    }),
                ))
                .theme(theme),
            )
    }
}

impl TreeState {
    fn render_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // 先不可变借用收集可见行快照，再逐行可变渲染（避开交叉借用）。
        fn collect(
            nodes: &[TreeNodeData],
            depth: usize,
            expanded: &HashSet<usize>,
            out: &mut Vec<(TreeNodeData, usize)>,
        ) {
            for n in nodes {
                out.push((n.clone(), depth));
                if n.kind == RowKind::Group && !n.children.is_empty() && expanded.contains(&n.id) {
                    collect(&n.children.clone(), depth + 1, expanded, out);
                }
            }
        }
        let mut visible: Vec<(TreeNodeData, usize)> = Vec::new();
        collect(&self.nodes, 0, &self.expanded, &mut visible);
        let mut out = Vec::new();
        for (node, depth) in visible {
            let (theme, row_h, indent) = (self.theme, self.row_h, self.indent);
            let selected = self.selected;
            let editing_id = self.editing.as_ref().map(|(id, _)| *id);
            out.push(self.render_row(&node, depth, theme, row_h, indent, selected, editing_id, cx));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &mut self,
        node: &TreeNodeData,
        depth: usize,
        theme: Theme,
        row_h: f32,
        indent: f32,
        selected: Option<usize>,
        editing_id: Option<usize>,
        cx: &Context<TreeState>,
    ) -> AnyElement {
        let id = node.id;
        let is_sel = selected == Some(id);
        let is_editing = editing_id == Some(id);
        // 内容起点（图标/标题）只由层级决定；首列基准已含箭头槽——
        // 箭头画在缩进区内，同层叶/分组不因有无箭头而错位。
        let indent_px = BASE_INDENT + depth as f32 * indent;

        // —— 小节标题行：小字弱化，不参与交互 ——
        if node.kind == RowKind::Header {
            return div()
                .id(("tree-header", id))
                .flex()
                .items_center()
                .h(px(24.))
                .px_3()
                .mt_1()
                .flex_none()
                .text_size(px(10.5))
                .text_color(rgb(node.colors.text.unwrap_or(theme.dim)))
                .child(node.label.clone())
                .into_any_element();
        }

        let ghost_label: gpui::SharedString = node.label.clone().into();
        let ghost_icon: Option<gpui::SharedString> = if node.kind == RowKind::Group {
            let open = self.expanded.contains(&id);
            let icon = if open { node.icon_open.clone().or(node.icon.clone()) } else { node.icon.clone() };
            icon
        } else {
            node.icon.clone()
        };

        let mut row = div()
            .id(("tree-node", id))
            .debug_selector(move || format!("tree-node-{id}"))
            .flex()
            .items_center()
            .gap_1()
            .cursor(gpui::CursorStyle::PointingHand)
            .h(px(if is_editing { row_h + 4. } else { row_h }))
            .pr_2()
            .mx_1()
            .rounded_md()
            .text_size(px(12.))
            .on_click(cx.listener(move |this: &mut TreeState, ev: &ClickEvent, window, cx| {
                if !ev.standard_click() {
                    return;
                }
                // 编辑中的行：点击（含点进输入框选光标）不触发选中/展开，
                // 否则分组行会把编辑框折叠掉——重命名"无法修改"即此。
                if this.editing.as_ref().is_some_and(|(eid, _)| *eid == id) {
                    return;
                }
                this.selected = Some(id);
                match this.find_node(id).map(|n| n.kind) {
                    Some(RowKind::Group) => {
                        let now_expanded = !this.expanded.contains(&id);
                        if now_expanded {
                            this.expanded.insert(id);
                        } else {
                            this.expanded.remove(&id);
                        }
                        cx.emit(TreeEvent::Toggle { id, expanded: now_expanded });
                    }
                    Some(RowKind::Leaf) if ev.click_count() >= 2 => {
                        cx.emit(TreeEvent::Activate { id });
                    }
                    _ => {}
                }
                let _ = window;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this: &mut TreeState, ev: &MouseDownEvent, _w, cx| {
                    this.selected = Some(id);
                    cx.emit(TreeEvent::ContextMenu { id: Some(id), position: ev.position });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_drag(TreeDrag(id), {
                let label = ghost_label.clone();
                let icon = ghost_icon.clone();
                let theme2 = theme;
                move |_, _, _, cx| {
                    cx.new(|_| TreeDragGhost { label: label.clone(), icon: icon.clone(), theme: theme2 })
                }
            });

        // —— 底色/文字：选中态统一主题样式；常态可用节点覆写 ——
        if is_sel {
            row = row.bg(rgb(theme.accent)).text_color(rgb(theme.accent_dark));
        } else {
            let text = node.colors.text.unwrap_or(theme.text);
            row = row.text_color(rgb(text));
            if let Some(bg) = node.colors.bg {
                row = row.bg(theme.surface(bg)).hover(|s| s.bg(theme.surface(bg)));
            } else {
                row = row.hover(|s| s.bg(theme.surface(theme.card)));
            }
        }

        // —— 行内容：箭头（分组）/ 图标 / 标题（优先保留）/ 状态点 / 行尾信息（收缩） ——
        let icon_color = if is_sel {
            theme.accent_dark
        } else {
            node.colors.icon.unwrap_or(theme.muted)
        };

        // 行内元素统一 16px 等高盒 + 垂直居中，消除图标/文字基线错位。
        let mut body = div().flex().flex_1().min_w_0().items_center().gap_1();
        if node.kind == RowKind::Group {
            body = body.font_weight(gpui::FontWeight::SEMIBOLD);
        }

        // —— 缩进区：占位 + 箭头（分组）。箭头落在缩进区最后一个槽位、
        // 不占行内容空间——同层叶/分组的内容起点严格一致。——
        let lead = if node.kind == RowKind::Group {
            indent_px - ARROW_W - 2. * ITEM_GAP
        } else {
            indent_px - ITEM_GAP
        };
        body = body.child(div().flex_none().w(px(lead)));
        if node.kind == RowKind::Group {
            let open = self.expanded.contains(&id);
            let arrow = if open { "▾" } else { "▸" };
            body = body.child(
                div()
                    .w(px(ARROW_W))
                    .h(px(16.))
                    .flex()
                    .items_center()
                    .flex_none()
                    .text_size(px(9.))
                    .line_height(px(16.))
                    .text_color(rgb(if is_sel { theme.accent_dark } else { theme.dim }))
                    .debug_selector(move || format!("tree-arrow-{id}"))
                    .child(arrow),
            );
        }

        // 图标：16px 盒内居中 + 右缘 4px（与 gap_1 合成 8px 的图标-标题间距）。
        let icon_path = ghost_icon; // 展开/收起图标已择定
        if let Some(path) = icon_path {
            body = body.child(
                div()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .mr_1()
                    .child(
                        gpui::svg()
                            .path(path)
                            .size(px(13.))
                            .text_color(rgb(icon_color)),
                    ),
            );
        }

        // 标题：flex_none 优先保留（宽度不足时收缩的是行尾信息）
        let title: AnyElement = if is_editing {
            let field = self
                .editing
                .as_ref()
                .and_then(|(fid, f)| (*fid == id).then(|| f.clone()));
            match field {
                Some(field) => div()
                    .flex_1()
                    .min_w_0()
                    .child(InputField::new(field).size(InputSize::Sm).theme(theme))
                    .into_any_element(),
                None => div().flex_1().into_any_element(),
            }
        } else {
            div()
                .flex_none()
                .max_w(px(160.))
                .h(px(16.))
                .flex()
                .items_center()
                .line_height(px(16.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .debug_selector(move || format!("tree-label-{id}"))
                .child(node.label.clone())
                .into_any_element()
        };
        body = body.child(title);

        // 状态点 + 行尾信息（弹性余量、右对齐、收缩省略）
        if !is_editing {
            if let Some(dot) = node.dot {
                body = body.child(
                    div()
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .flex_none()
                        .child(div().size(px(6.)).rounded_full().bg(rgb(dot))),
                );
            }
            if let Some(suffix) = node.suffix.clone() {
                // 同一 label 的两种溢出显示：Tail=尾部省略（默认）；Middle=两头保留、中间 …。
                let color = rgb(if is_sel { theme.accent_dark } else { theme.dim });
                let suffix_el: AnyElement = match node.suffix_ellipsis {
                    EllipsisMode::Tail => div()
                        .flex_1()
                        .min_w_0()
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .line_height(px(16.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(10.))
                        .text_color(color)
                        .child(suffix)
                        .into_any_element(),
                    EllipsisMode::Middle => MiddleLabel {
                        text: suffix.into(),
                        font_size: px(10.),
                        color: color.into(),
                        line_h: px(16.),
                    }
                    .into_any_element(),
                };
                body = body.child(suffix_el);
            } else {
                body = body.child(div().flex_1());
            }
        }

        row = row.child(body);

        // —— 拖放目标（分组且 droppable）——
        if node.kind == RowKind::Group && node.droppable {
            row = row
                .drag_over::<TreeDrag>(|s, _drag, _w, _cx| s.bg(gpui::rgba(0x7aa2f733)))
                .on_drop::<TreeDrag>(cx.listener(move |_this, drag: &TreeDrag, _w, cx| {
                    if drag.0 != id {
                        cx.emit(TreeEvent::Move { from: drag.0, to: id });
                    }
                }));
        }

        row.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: usize, children: Vec<TreeNodeData>) -> TreeNodeData {
        let mut node = TreeNodeData::group(id, format!("n{id}"));
        node.children = children;
        node
    }

    #[test]
    fn flatten_respects_expansion() {
        let forest = vec![
            n(1, vec![n(2, vec![]), n(3, vec![n(4, vec![])])]),
            n(5, vec![]),
        ];
        // 全收起：只看得到第一层
        let empty = HashSet::new();
        assert_eq!(flatten_visible(&forest, &empty), vec![(1, 0), (5, 0)]);
        // 展开 1：2、3 可见，4 仍藏
        let mut ex = HashSet::new();
        ex.insert(1);
        assert_eq!(flatten_visible(&forest, &ex), vec![(1, 0), (2, 1), (3, 1), (5, 0)]);
        // 展开 3：4 可见（深度 2）
        ex.insert(3);
        assert_eq!(
            flatten_visible(&forest, &ex),
            vec![(1, 0), (2, 1), (3, 1), (4, 2), (5, 0)]
        );
    }

    #[test]
    fn middle_ellipsize_keeps_both_ends() {
        // 伪测宽：每字符 1.0 宽、… 也按 1 字符计（len 宽）
        let w = |t: &str| t.chars().count() as f32;
        // 放得下：原样
        assert_eq!(middle_ellipsize("ab", 10., w), "ab");
        // 放不下：保留头尾、中间 …，总宽 <= avail，两端可见
        // n=19、avail=10 → keep=9（head=6 "121.40"、tail=3 "222"）
        let out = middle_ellipsize("121.40.227.110:2222", 10., w);
        assert_eq!(out, "121.40…222");
        // 头占 2/3：avail=6 → keep=5（head=3、tail=2）
        assert_eq!(middle_ellipsize("abcdefghi", 6., w), "abc…hi");
        // 极窄：只剩 …
        assert_eq!(middle_ellipsize("abcdef", 1., w), "…");
        // 空串原样
        assert_eq!(middle_ellipsize("", 5., w), "");
    }

    #[test]
    fn node_colors_default_all_none() {
        let c = NodeColors::default();
        assert!(c.icon.is_none() && c.text.is_none() && c.bg.is_none());
    }
}
