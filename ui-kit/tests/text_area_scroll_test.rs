//! TextArea 滚动交互的进程内回归测试：X/Y 滚动条拖拽必须真正滚动内容。
//! 复现用户报告：「X 轴能显示但拖不动，滚动条和内容都不动」。

use gpui::{div, prelude::*, px, IntoElement, MouseButton, ParentElement, Point, Render, Styled, TestAppContext, Window};

use ui_kit::base::text_area::{TextArea, TextAreaField};

struct Harness {
    area: gpui::Entity<TextArea>,
}

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let _ = cx;
        div()
            .id("root")
            .w(px(320.))
            .h(px(200.))
            .p(px(10.))
            .child(TextAreaField::new(self.area.clone()))
    }
}

fn center_of(b: gpui::Bounds<gpui::Pixels>) -> Point<gpui::Pixels> {
    Point::new(b.origin.x + b.size.width / 2., b.origin.y + b.size.height / 2.)
}

#[gpui::test]
fn x_thumb_drag_scrolls_content(cx: &mut TestAppContext) {
    let (harness, cx) = cx.add_window_view(|_w, cx| Harness {
        area: cx.new(|cx| {
            let mut a = TextArea::new("", 3, cx);
            // 300 字符连续"单词"（同 base64 形态），no-wrap 必横向溢出。
            a.set_value("A".repeat(300), cx);
            a.set_wrap(false, cx);
            a
        }),
    });
    cx.run_until_parked();

    // 悬停让滚动条可见
    let field = cx.debug_bounds("text-area-field").expect("field bounds");
    cx.simulate_mouse_move(center_of(field), None, Default::default());
    cx.run_until_parked();

    // set_value 把光标落在内容尾，自动跟随已滚到最右——向左拖验证能回滚。
    let before = cx.update(|_w, cx| harness.read(cx).area.read(cx).scroll_x());
    assert!(before > 100., "光标跟随应已横向滚离原点：before={before}");
    let thumb = cx.debug_bounds("scroll-thumb-x").expect("x thumb visible");
    let c = center_of(thumb);
    cx.simulate_mouse_down(c, MouseButton::Left, Default::default());
    // 拖拽会话在 Bubble 阶段建立，on_drag_move 只在 Capture 阶段派发——
    // 第一段 move 建会话，必须再来一段才触发滚动（真实鼠标天然多段）。
    cx.simulate_mouse_move(Point::new(c.x - px(40.), c.y), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_move(Point::new(c.x - px(80.), c.y), Some(MouseButton::Left), Default::default());
    cx.run_until_parked();
    let after = cx.update(|_w, cx| harness.read(cx).area.read(cx).scroll_x());
    cx.simulate_mouse_up(Point::new(c.x - px(80.), c.y), MouseButton::Left, Default::default());
    cx.run_until_parked();

    assert!(
        after < before - 1.,
        "X 滚动条拖拽应回滚横向偏移：before={before} after={after}"
    );
}

#[gpui::test]
fn y_thumb_drag_scrolls_content(cx: &mut TestAppContext) {
    let (harness, cx) = cx.add_window_view(|_w, cx| Harness {
        area: cx.new(|cx| {
            let mut a = TextArea::new("", 3, cx);
            // 100 行短行，纵向必溢出（wrap 默认开，行短不折）。
            let text: Vec<String> = (0..100).map(|i| format!("line-{i}")).collect();
            a.set_value(text.join("\n"), cx);
            a
        }),
    });
    cx.run_until_parked();

    let field = cx.debug_bounds("text-area-field").expect("field bounds");
    cx.simulate_mouse_move(center_of(field), None, Default::default());
    cx.run_until_parked();

    // 同上：光标在末行，已滚到底——向上拖验证能回滚。
    let before = cx.update(|_w, cx| harness.read(cx).area.read(cx).scroll_y());
    assert!(before > 100., "光标跟随应已纵向滚离原点：before={before}");
    let thumb = cx.debug_bounds("scroll-thumb-y").expect("y thumb visible");
    let c = center_of(thumb);
    cx.simulate_mouse_down(c, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(Point::new(c.x, c.y - px(40.)), Some(MouseButton::Left), Default::default());
    cx.simulate_mouse_move(Point::new(c.x, c.y - px(80.)), Some(MouseButton::Left), Default::default());
    cx.run_until_parked();
    let after = cx.update(|_w, cx| harness.read(cx).area.read(cx).scroll_y());
    cx.simulate_mouse_up(Point::new(c.x, c.y - px(80.)), MouseButton::Left, Default::default());
    cx.run_until_parked();

    assert!(
        after < before - 1.,
        "Y 滚动条拖拽应回滚纵向偏移：before={before} after={after}"
    );
}
