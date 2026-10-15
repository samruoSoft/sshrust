//! 表单基元：标签行（单行/块级）与分段选择器。

use gpui::{prelude::*, AnyElement, Context, IntoElement, Window, div, px, rgb};

use crate::Theme;

/// 单行标签行：`标签 [child]`，垂直居中（label 固定 72px 宽）。
pub fn labeled_row(label: &str, child: impl IntoElement, theme: Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(72.)).flex_none().text_color(rgb(theme.muted)).child(label.to_string()))
        .child(child)
        .into_any_element()
}

/// 块级标签行：标签贴首行（多行输入框等高子元素用）。
pub fn labeled_block(label: &str, child: impl IntoElement, theme: Theme) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(72.))
                .flex_none()
                .pt(px(7.))
                .text_color(rgb(theme.muted))
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(child))
        .into_any_element()
}

/// 分段选择器（如 密码/密钥 切换）：选中段强调底色，其余描边。
/// `options: &[(标签, 值)]`，值需 Copy + PartialEq。
pub fn segmented<T: 'static, V: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(&str, V)],
    selected: V,
    theme: Theme,
    cx: &Context<T>,
    on_select: impl Fn(&mut T, V, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let mut row = div().flex().gap_2();
    let on_select = std::rc::Rc::new(on_select);
    for (i, (label, value)) in options.iter().enumerate() {
        let value = *value;
        let is_sel = value == selected;
        let mut seg = div()
            .id((id, i))
            .px_3()
            .h(px(26.))
            .flex()
            .items_center()
            .rounded_sm()
            .text_size(px(13.))
            .child(label.to_string())
            .on_click({
                let on_select = on_select.clone();
                cx.listener(move |this: &mut T, _ev: &gpui::ClickEvent, window, cx| {
                    on_select(this, value, window, cx);
                })
            });
        seg = if is_sel {
            seg.bg(rgb(theme.accent)).text_color(rgb(theme.accent_dark))
        } else {
            seg.border_1()
                .border_color(rgb(theme.border))
                .text_color(rgb(theme.muted))
                .hover(|s| s.border_color(rgb(theme.accent)))
        };
        row = row.child(seg);
    }
    row.into_any_element()
}
