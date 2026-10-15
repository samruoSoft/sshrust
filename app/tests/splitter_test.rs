//! 分割线拖拽与侧栏布局的进程内确定性回归测试。
//!
//! Harness 镜像真实侧栏的绝对定位布局（gpui 0.2.2 嵌套列布局 flex_1 不可靠，
//! 真实代码已改为：工具栏 bottom=0 / 快捷栏 bottom=工具栏高 / 分割线 bottom=工具栏高+快捷栏高 /
//! 树 top=38 bottom=分割线之上），以及同一套按下锚定的拖拽逻辑。验证：
//! 1. 拖动分割线只调整快捷栏高度（底边不动、顶边上移/下移）；
//! 2. 底部工具栏 bounds 在任何情况下纹丝不动且贴住面板底；
//! 3. 未命中分割线的拖动无副作用；
//! 4. 树内容溢出（60 行）时布局不受影响。

use std::sync::{Arc, Mutex};

use gpui::{
    div, prelude::*, px, rgb, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Point, Render, StatefulInteractiveElement, Styled, TestAppContext, Window,
};

const SPLITTER_H: f32 = 7.0;
const TOOLBAR_H: f32 = 38.0;
const TITLEBAR_H: f32 = 38.0;
const ROOT_H: f32 = 600.0;

/// 与 sidebar.rs 相同的拖拽锚点机制；三类拖拽各用独立标记，
/// gpui 的 on_drag_move 按类型过滤——同类型会互相触发（真实踩坑）。
#[derive(Clone)]
struct SplitDrag(Arc<Mutex<Option<(f32, f32)>>>);
#[derive(Clone)]
struct WidthDrag(Arc<Mutex<Option<(f32, f32)>>>);

struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.))
    }
}

struct Harness {
    shortcut_h: f32,
    sidebar_w: f32,
    width_anchor: Arc<Mutex<Option<(f32, f32)>>>,
    tree_rows: usize,
    anchor: Arc<Mutex<Option<(f32, f32)>>>,
    downs: u32,
    drag_moves: u32,
}

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drag = SplitDrag(self.anchor.clone());
        let inset_bottom = TOOLBAR_H + self.shortcut_h;

        let rows: Vec<gpui::AnyElement> = (0..self.tree_rows)
            .map(|i| {
                div()
                    .id(("row", i))
                    .flex_none()
                    .h(px(26.))
                    .w_full()
                    .child(format!("row {i}"))
                    .into_any_element()
            })
            .collect();

        // ── 镜像真实结构：root(row, 显式高) = [left-panel(absolute 布局), main-column] ──
        div()
            .id("root")
            .w_full()
            .h(px(ROOT_H))
            .flex()
            .child(
                div()
                    .id("left-panel")
                    .debug_selector(|| "left-panel".to_string())
                    .w(px(self.sidebar_w))
                    .flex_none()
                    .bg(rgb(0x1f2335))
                    .border_r_1()
                    .border_color(rgb(0x414868))
                    .overflow_hidden()
                    .relative()
                    // 顶部让位区
                    .child(
                        div()
                            .id("top-zone")
                            .absolute()
                            .top_0()
                            .left_0()
                            .w_full()
                            .h(px(TITLEBAR_H))
                            .border_b_1(),
                    )
                    // 树
                    .child(
                        div()
                            .id("sidebar-tree")
                            .absolute()
                            .top(px(TITLEBAR_H))
                            .bottom(px(inset_bottom + SPLITTER_H))
                            .left_0()
                            .w_full()
                            .flex_col()
                            .pt_1()
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    // 分割线（按下锚定 + on_drag/on_drag_move，与真实代码一致）
                    .child(
                        div()
                            .id("sidebar-splitter")
                            .debug_selector(|| "splitter".to_string())
                            .absolute()
                            .bottom(px(inset_bottom))
                            .left_0()
                            .w_full()
                            .h(px(SPLITTER_H))
                            .flex()
                            .items_center()
                            .cursor(gpui::CursorStyle::ResizeRow)
                            .occlude()
                            .child(div().h(px(1.)).w_full().bg(rgb(0x414868)))
                            .hover(|s| s.bg(rgb(0x363c58)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &gpui::MouseDownEvent, _w, _cx| {
                                    this.downs += 1;
                                    let y: f32 = ev.position.y.into();
                                    *this.anchor.lock().unwrap() = Some((y, this.shortcut_h));
                                }),
                            )
                            .on_drag(drag, move |_, _, _, cx| cx.new(|_| DragGhost))
                            .on_drag_move(cx.listener(
                                |this, e: &gpui::DragMoveEvent<SplitDrag>, _w, cx| {
                                    this.drag_moves += 1;
                                    let y: f32 = e.event.position.y.into();
                                    let state = e.drag(cx).0.clone();
                                    let mut anchor = state.lock().unwrap();
                                    if !e.event.dragging() {
                                        *anchor = None;
                                        return;
                                    }
                                    let &mut (start_y, start_h) =
                                        anchor.get_or_insert((y, this.shortcut_h));
                                    // 镜像真实代码的动态上限：树保底 80
                                    let max_h = (ROOT_H - TITLEBAR_H - TOOLBAR_H - SPLITTER_H - 80.).max(56.);
                                    let h = (start_h + (start_y - y)).clamp(56., max_h);
                                    this.shortcut_h = h;
                                    cx.notify();
                                },
                            )),
                    )
                    // 快捷栏（bottom = 工具栏高，与真实代码一致）
                    .child(
                        div()
                            .id("sidebar-shortcuts")
                            .debug_selector(|| "shortcut".to_string())
                            .absolute()
                            .bottom(px(TOOLBAR_H))
                            .left_0()
                            .w_full()
                            .h(px(self.shortcut_h))
                            .flex_col()
                            .overflow_hidden(),
                    )
                    // 工具栏（钉底）
                    .child(
                        div()
                            .id("sidebar-toolbar")
                            .debug_selector(|| "toolbar".to_string())
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .h(px(TOOLBAR_H))
                            .w_full()
                            .flex()
                            .items_center()
                            .px_2()
                            .bg(rgb(0x1b1e2e))
                            .border_t_1(),
                    )
                    // 右缘宽度拖柄（独立 WidthDrag 标记）
                    .child({
                        let drag = WidthDrag(self.width_anchor.clone());
                        div()
                            .id("sidebar-width-handle")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .w(px(5.))
                            .cursor(gpui::CursorStyle::ResizeLeftRight)
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, ev: &gpui::MouseDownEvent, _w, _cx| {
                                    let x: f32 = ev.position.x.into();
                                    *this.width_anchor.lock().unwrap() = Some((x, this.sidebar_w));
                                }),
                            )
                            .on_drag(drag, move |_, _, _, cx| cx.new(|_| DragGhost))
                            .on_drag_move(cx.listener(
                                |this, e: &gpui::DragMoveEvent<WidthDrag>, _w, cx| {
                                    if !e.event.dragging() {
                                        *this.width_anchor.lock().unwrap() = None;
                                        return;
                                    }
                                    let x: f32 = e.event.position.x.into();
                                    let st = e.drag(cx).0.clone();
                                    let mut a = st.lock().unwrap();
                                    let &mut (sx, sw) = a.get_or_insert((x, this.sidebar_w));
                                    this.sidebar_w = (sw + (x - sx)).clamp(180., 420.);
                                    cx.notify();
                                },
                            ))
                    }),
            )
            .child(
                div()
                    .id("main-column")
                    .flex_1()
                    .flex_col()
                    .overflow_hidden()
                    .child(div().id("titlebar").flex_none().h(px(38.)).border_b_1())
                    .child(div().id("body").flex_1().bg(rgb(0x1a1b26))),
            )
    }
}

fn setup(
    cx: &mut TestAppContext,
    tree_rows: usize,
) -> (gpui::Entity<Harness>, &mut gpui::VisualTestContext) {
    let (harness, cx) = cx.add_window_view(|_window, _cx| Harness {
        shortcut_h: 110.,
        sidebar_w: 232.,
        width_anchor: Arc::new(Mutex::new(None)),
        tree_rows,
        anchor: Arc::new(Mutex::new(None)),
        downs: 0,
        drag_moves: 0,
    });
    cx.run_until_parked();
    (harness, cx)
}

/// 分割线中心 = 快捷栏顶边 - SPLITTER_H/2（用 debug_bounds 实测，不做理论推算）。
fn splitter_center_y(cx: &mut gpui::VisualTestContext) -> f32 {
    let sp = cx.debug_bounds("splitter").expect("splitter bounds");
    let top: f32 = sp.origin.y.into();
    top + SPLITTER_H / 2.
}

fn assert_toolbar_pinned(cx: &mut gpui::VisualTestContext, label: &str) {
    let tb = cx.debug_bounds("toolbar").expect("toolbar bounds");
    let bottom: f32 = (tb.origin.y + tb.size.height).into();
    assert!(
        (ROOT_H - bottom).abs() < 0.5,
        "{label}: 工具栏必须贴住面板底部({ROOT_H})，实际 {bottom}"
    );
}

fn drag(cx: &mut gpui::VisualTestContext, from: f32, by: f32) {
    let p = |y: f32| Point::new(px(50.), px(y));
    cx.simulate_mouse_move(p(from), None, Default::default());
    cx.simulate_mouse_down(p(from), MouseButton::Left, Default::default());
    cx.simulate_mouse_move(p(from + by / 2.), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_move(p(from + by), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_up(p(from + by), MouseButton::Left, Default::default());
    cx.run_until_parked();
}

#[gpui::test]
fn drag_splitter_resizes_shortcut_and_pins_toolbar(cx: &mut TestAppContext) {
    let (harness, cx) = setup(cx, 8);

    assert_toolbar_pinned(cx, "初始");
    let y0 = splitter_center_y(cx);
    let toolbar_before = cx.debug_bounds("toolbar").expect("toolbar bounds");
    let shortcut_before = cx.debug_bounds("shortcut").expect("shortcut bounds");

    drag(cx, y0, 30.);

    let (downs, drag_moves, shortcut_h) = cx.update(|_w, cx| {
        let s = harness.read(cx);
        (s.downs, s.drag_moves, s.shortcut_h)
    });
    assert!(downs >= 1, "分割线应收到 mouse down");
    assert!(drag_moves >= 1, "拖拽会话应产生 drag move");
    assert!(
        (shortcut_h - 80.).abs() < 0.5,
        "向下拖 30：快捷栏应从 110 降到 ~80，实际 {shortcut_h}"
    );

    let toolbar_after = cx.debug_bounds("toolbar").expect("toolbar bounds");
    assert_eq!(toolbar_before, toolbar_after, "拖动分割线时工具栏不允许移动");
    assert_toolbar_pinned(cx, "拖动后");

    let shortcut_after = cx.debug_bounds("shortcut").expect("shortcut bounds");
    let (h_before, h_after): (f32, f32) =
        (shortcut_before.size.height.into(), shortcut_after.size.height.into());
    assert!((h_before - h_after - 30.).abs() < 0.5, "快捷栏应变矮 30");
    // 底边不动、顶边上移
    let (b_before, b_after): (f32, f32) = (
        (shortcut_before.origin.y + shortcut_before.size.height).into(),
        (shortcut_after.origin.y + shortcut_after.size.height).into(),
    );
    assert!((b_before - b_after).abs() < 0.5, "快捷栏底边不允许移动");
}

/// 回归：独立拖拽标记——拖宽度柄不影响快捷栏高度，拖分割线不影响侧栏宽度。
#[gpui::test]
fn drag_markers_do_not_cross_trigger(cx: &mut TestAppContext) {
    let (harness, cx) = setup(cx, 8);

    // 1) 拖宽度柄右移 40：宽度变化，快捷栏高不变
    let panel = cx.debug_bounds("left-panel").expect("panel bounds");
    let panel_x: f32 = panel.origin.x.into();
    let panel_w: f32 = panel.size.width.into();
    let hx = panel_x + panel_w - 2.;
    let p = |x: f32, y: f32| Point::new(px(x), px(y));
    cx.simulate_mouse_move(p(hx, 300.), None, Default::default());
    cx.simulate_mouse_down(p(hx, 300.), MouseButton::Left, Default::default());
    cx.simulate_mouse_move(p(hx + 20., 300.), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_move(p(hx + 40., 300.), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_up(p(hx + 40., 300.), MouseButton::Left, Default::default());
    cx.run_until_parked();

    let (w, h) = cx.update(|_w, cx| {
        let s = harness.read(cx);
        (s.sidebar_w, s.shortcut_h)
    });
    assert!((w - 272.).abs() < 0.5, "宽度应 232→272，实际 {w}");
    assert!((h - 110.).abs() < 0.5, "拖宽度柄时快捷栏高不允许变化，实际 {h}");

    // 2) 拖分割线：快捷栏变化，宽度不变
    let y0 = splitter_center_y(cx);
    drag(cx, y0, 30.);
    let (w, h) = cx.update(|_w, cx| {
        let s = harness.read(cx);
        (s.sidebar_w, s.shortcut_h)
    });
    assert!((h - 80.).abs() < 0.5, "向下拖 30：快捷栏应 110→80，实际 {h}");
    assert!((w - 272.).abs() < 0.5, "拖分割线时侧栏宽不允许变化，实际 {w}");
}

#[gpui::test]
fn drag_off_splitter_changes_nothing(cx: &mut TestAppContext) {
    let (harness, cx) = setup(cx, 8);

    let y0 = splitter_center_y(cx);
    let toolbar_before = cx.debug_bounds("toolbar").expect("toolbar bounds");

    // 点在快捷栏内部（分割线下方 20px）拖动
    drag(cx, y0 + 20., 30.);

    let (downs, drag_moves, shortcut_h) = cx.update(|_w, cx| {
        let s = harness.read(cx);
        (s.downs, s.drag_moves, s.shortcut_h)
    });
    assert_eq!(downs, 0, "快捷栏不应收到分割线的 mouse down");
    assert_eq!(drag_moves, 0, "不应产生拖拽会话");
    assert!((shortcut_h - 110.).abs() < 0.5, "高度不应变化");
    assert_eq!(toolbar_before, cx.debug_bounds("toolbar").unwrap());
}

/// 极限拖拽：快捷栏上限动态（树保底 TREE_MIN=80），不再受静态上限限制。
#[gpui::test]
fn extreme_drag_keeps_tree_minimum(cx: &mut TestAppContext) {
    let (harness, cx) = setup(cx, 8);
    assert_toolbar_pinned(cx, "初始");

    let y0 = splitter_center_y(cx);
    let toolbar_before = cx.debug_bounds("toolbar").expect("toolbar bounds");
    // 用力向上拖 400：110+400=510 → 钳到 437（600-163-80）
    drag(cx, y0, -400.);

    let shortcut_h = cx.update(|_w, cx| harness.read(cx).shortcut_h);
    assert!((shortcut_h - 437.).abs() < 0.5, "快捷栏应钳到动态上限 437，实际 {shortcut_h}");

    // 树高 = 600-38-38-7-437 = 80 = 最小值（以快捷栏顶边反推）
    let shortcut = cx.debug_bounds("shortcut").expect("shortcut bounds");
    let shortcut_top: f32 = shortcut.origin.y.into();
    let tree_h = shortcut_top - SPLITTER_H - TITLEBAR_H;
    assert!((tree_h - 80.).abs() < 0.5, "树区应保底 80，实际 {tree_h}");
    assert_eq!(toolbar_before, cx.debug_bounds("toolbar").unwrap(), "极限拖拽工具栏仍不许动");
}

/// 树内容超过面板空间（60 行 × 26px ≈ 1560px）：布局与工具栏不受任何影响。
#[gpui::test]
fn toolbar_stays_pinned_with_overflowing_tree(cx: &mut TestAppContext) {
    let (harness, cx) = setup(cx, 60);

    assert_toolbar_pinned(cx, "树溢出时");
    let y0 = splitter_center_y(cx);
    let toolbar_before = cx.debug_bounds("toolbar").expect("toolbar bounds");

    drag(cx, y0, -40.);

    let shortcut_h = cx.update(|_w, cx| harness.read(cx).shortcut_h);
    assert!(
        (shortcut_h - 150.).abs() < 0.5,
        "向上拖 40：快捷栏应从 110 涨到 ~150，实际 {shortcut_h}"
    );
    assert_eq!(
        toolbar_before,
        cx.debug_bounds("toolbar").unwrap(),
        "树溢出+拖动后工具栏仍不允许移动"
    );
    assert_toolbar_pinned(cx, "树溢出+拖动后");
}

/// 侧栏右缘宽度拖柄：拖动改变侧栏宽度（与真实代码同一套按下锚定机制）。
#[gpui::test]
fn drag_width_handle_resizes_sidebar(cx: &mut TestAppContext) {
    use gpui::div;
    use std::sync::{Arc, Mutex};

    struct W {
        width: f32,
        anchor: Arc<Mutex<Option<(f32, f32)>>>,
    }
    impl Render for W {
        fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let drag = SplitDrag(self.anchor.clone());
            div()
                .id("root")
                .w_full()
                .h(px(400.))
                .flex()
                .child(
                    div()
                        .id("panel")
                        .debug_selector(|| "panel".to_string())
                        .w(px(self.width))
                        .flex_none()
                        .relative()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("width-handle")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(5.))
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, ev: &gpui::MouseDownEvent, _w, _cx| {
                                        let x: f32 = ev.position.x.into();
                                        *this.anchor.lock().unwrap() = Some((x, this.width));
                                    }),
                                )
                                .on_drag(drag, move |_, _, _, cx| cx.new(|_| DragGhost))
                                .on_drag_move(cx.listener(
                                    |this, e: &gpui::DragMoveEvent<SplitDrag>, _w, cx| {
                                        if !e.event.dragging() {
                                            *this.anchor.lock().unwrap() = None;
                                            return;
                                        }
                                        let x: f32 = e.event.position.x.into();
                                        let st = e.drag(cx).0.clone();
                                        let mut a = st.lock().unwrap();
                                        let &mut (sx, sw) = a.get_or_insert((x, this.width));
                                        this.width = (sw + (x - sx)).clamp(180., 420.);
                                        cx.notify();
                                    },
                                )),
                        ),
                )
                .child(div().id("main").flex_1().bg(rgb(0x1a1b26)))
        }
    }

    let (entity, cx) = cx.add_window_view(|_w, _cx| W {
        width: 232.,
        anchor: Arc::new(Mutex::new(None)),
    });
    cx.run_until_parked();

    let panel_before = cx.debug_bounds("panel").expect("panel");
    let panel_x: f32 = panel_before.origin.x.into();
    let handle_x = panel_x + 232. - 2.;

    let p = |x: f32, y: f32| Point::new(px(x), px(y));
    cx.simulate_mouse_move(p(handle_x, 200.), None, Default::default());
    cx.simulate_mouse_down(p(handle_x, 200.), MouseButton::Left, Default::default());
    cx.simulate_mouse_move(p(handle_x + 20., 200.), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_move(p(handle_x + 40., 200.), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_up(p(handle_x + 40., 200.), MouseButton::Left, Default::default());
    cx.run_until_parked();

    let width = cx.update(|_w, cx| entity.read(cx).width);
    assert!((width - 272.).abs() < 0.5, "拖右 40：侧栏应 232→272，实际 {width}");
    let panel_after = cx.debug_bounds("panel").expect("panel");
    let (w_before, w_after): (f32, f32) = (panel_before.size.width.into(), panel_after.size.width.into());
    assert!((w_after - w_before - 40.).abs() < 0.5, "面板实际宽度应 +40");
}

/// 右面板左缘宽度柄：向左拖变宽（与侧栏方向相反）；且与侧栏右缘柄
/// 互不串扰（真实代码两者标记类型分立，此处镜像验证同一机制）。
#[gpui::test]
fn drag_handle_resizes_right_panel_no_cross_trigger(cx: &mut TestAppContext) {
    use std::sync::{Arc, Mutex};

    const RIGHT_TOOLBAR_W: f32 = 40.;

    #[derive(Clone)]
    struct LeftWidth(Arc<Mutex<Option<(f32, f32)>>>);
    #[derive(Clone)]
    struct RightWidth(Arc<Mutex<Option<(f32, f32)>>>);

    struct R {
        sidebar_w: f32,
        right_w: f32,
        left_anchor: Arc<Mutex<Option<(f32, f32)>>>,
        right_anchor: Arc<Mutex<Option<(f32, f32)>>>,
    }

    impl Render for R {
        fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let ldrag = LeftWidth(self.left_anchor.clone());
            let rdrag = RightWidth(self.right_anchor.clone());
            div()
                .id("root")
                .w_full()
                .h(px(400.))
                .flex()
                // 侧栏（右缘柄：拖右变宽）
                .child(
                    div()
                        .id("left-panel")
                        .debug_selector(|| "left-panel".to_string())
                        .w(px(self.sidebar_w))
                        .flex_none()
                        .relative()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("sidebar-width-handle")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(5.))
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, ev: &gpui::MouseDownEvent, _w, _cx| {
                                        let x: f32 = ev.position.x.into();
                                        *this.left_anchor.lock().unwrap() = Some((x, this.sidebar_w));
                                    }),
                                )
                                .on_drag(ldrag, move |_, _, _, cx| cx.new(|_| DragGhost))
                                .on_drag_move(cx.listener(
                                    |this, e: &gpui::DragMoveEvent<LeftWidth>, _w, cx| {
                                        if !e.event.dragging() {
                                            *this.left_anchor.lock().unwrap() = None;
                                            return;
                                        }
                                        let x: f32 = e.event.position.x.into();
                                        let st = e.drag(cx).0.clone();
                                        let mut a = st.lock().unwrap();
                                        let &mut (sx, sw) = a.get_or_insert((x, this.sidebar_w));
                                        this.sidebar_w = (sw + (x - sx)).clamp(180., 420.);
                                        cx.notify();
                                    },
                                )),
                        ),
                )
                .child(div().id("main").flex_1())
                // 右面板（左缘柄：拖左变宽，方向与侧栏相反）
                .child(
                    div()
                        .id("right-panel")
                        .debug_selector(|| "right-panel".to_string())
                        .w(px(self.right_w))
                        .flex_none()
                        .relative()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("right-width-handle")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(5.))
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, ev: &gpui::MouseDownEvent, _w, _cx| {
                                        let x: f32 = ev.position.x.into();
                                        *this.right_anchor.lock().unwrap() = Some((x, this.right_w));
                                    }),
                                )
                                .on_drag(rdrag, move |_, _, _, cx| cx.new(|_| DragGhost))
                                .on_drag_move(cx.listener(
                                    |this, e: &gpui::DragMoveEvent<RightWidth>, _w, cx| {
                                        if !e.event.dragging() {
                                            *this.right_anchor.lock().unwrap() = None;
                                            return;
                                        }
                                        let x: f32 = e.event.position.x.into();
                                        let st = e.drag(cx).0.clone();
                                        let mut a = st.lock().unwrap();
                                        let &mut (sx, sw) = a.get_or_insert((x, this.right_w));
                                        this.right_w = (sw + (sx - x)).clamp(200., 480.);
                                        cx.notify();
                                    },
                                )),
                        ),
                )
                // 右缘全高工具栏（常驻）
                .child(
                    div()
                        .id("right-toolbar")
                        .debug_selector(|| "right-toolbar".to_string())
                        .w(px(RIGHT_TOOLBAR_W))
                        .flex_none()
                        .h_full(),
                )
        }
    }

    let (entity, cx) = cx.add_window_view(|_w, _cx| R {
        sidebar_w: 232.,
        right_w: 280.,
        left_anchor: Arc::new(Mutex::new(None)),
        right_anchor: Arc::new(Mutex::new(None)),
    });
    cx.run_until_parked();

    let drag_x = |cx: &mut gpui::VisualTestContext, from: f32, by: f32| {
        // 与现有 drag() 一致：两段 move 才会建立拖拽会话
        let p = |x: f32| Point::new(px(x), px(200.));
        cx.simulate_mouse_move(p(from), None, Default::default());
        cx.simulate_mouse_down(p(from), MouseButton::Left, Default::default());
        cx.simulate_mouse_move(p(from + by / 2.), Some(MouseButton::Left), Default::default());
        cx.simulate_mouse_move(p(from + by), Some(MouseButton::Left), Default::default());
        cx.simulate_mouse_up(p(from + by), MouseButton::Left, Default::default());
        cx.run_until_parked();
    };

    let panel_before = cx.debug_bounds("right-panel").expect("right-panel");
    let toolbar_before = cx.debug_bounds("right-toolbar").expect("right-toolbar");
    let panel_x: f32 = panel_before.origin.x.into();
    let handle_x = panel_x + 2.;

    // 1) 右面板左缘柄向左拖 40：面板变宽，侧栏不动，工具栏钉在右缘不动
    drag_x(cx, handle_x, -40.);
    let (sw, rw) = cx.update(|_w, cx| {
        let s = entity.read(cx);
        (s.sidebar_w, s.right_w)
    });
    assert!((rw - 320.).abs() < 0.5, "向左拖 40：右面板应 280→320，实际 {rw}");
    assert!((sw - 232.).abs() < 0.5, "拖右面板柄时侧栏宽不允许变化，实际 {sw}");
    let panel_after = cx.debug_bounds("right-panel").expect("right-panel");
    let (w_before, w_after): (f32, f32) =
        (panel_before.size.width.into(), panel_after.size.width.into());
    assert!((w_after - w_before - 40.).abs() < 0.5, "右面板实际宽度应 +40");
    assert_eq!(toolbar_before, cx.debug_bounds("right-toolbar").unwrap(), "右面板变宽时工具栏钉右缘不动");

    // 2) 侧栏右缘柄向右拖 20：侧栏变宽，右面板不动（标记分立不串扰）
    let left = cx.debug_bounds("left-panel").expect("left-panel");
    let left_right: f32 = (left.origin.x + left.size.width).into();
    let sx = left_right - 2.;
    drag_x(cx, sx, 20.);
    let (sw, rw) = cx.update(|_w, cx| {
        let s = entity.read(cx);
        (s.sidebar_w, s.right_w)
    });
    assert!((sw - 252.).abs() < 0.5, "拖右 20：侧栏应 232→252，实际 {sw}");
    assert!((rw - 320.).abs() < 0.5, "拖侧栏柄时右面板宽不允许变化，实际 {rw}");

    // 3) 极限：右面板左缘柄用力向右拖 → 钳到下限 200
    let panel = cx.debug_bounds("right-panel").expect("right-panel");
    let panel_x: f32 = panel.origin.x.into();
    let handle_x = panel_x + 2.;
    drag_x(cx, handle_x, 500.);
    let rw = cx.update(|_w, cx| entity.read(cx).right_w);
    assert!((rw - 200.).abs() < 0.5, "向右极限拖：右面板应钳到 200，实际 {rw}");
}
