//! 按钮：函数式构造，监听器泛型于宿主 T。
//!
//! 四种样式 × 两档尺寸：
//! - Primary   强调底色（主操作：连接/确认）
//! - Secondary 描边（次操作：保存/测试）
//! - Ghost     无底无框，hover 才有底（工具栏/标题栏图标位）
//! - Danger    错误色描边（断开/删除）

use gpui::{
    prelude::*, AnyElement, ClickEvent, Context, ElementId, Entity, IntoElement,
    Window, div, px, rgb,
};

use crate::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    /// 26px：图标/紧凑位
    Sm,
    /// 32px：表单主行
    Md,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            ButtonSize::Sm => 26.,
            ButtonSize::Md => 32.,
        }
    }

    fn text_size(self) -> f32 {
        match self {
            ButtonSize::Sm => 13.,
            ButtonSize::Md => 13.,
        }
    }
}

/// 禁用/按 kind 着色的公共样式段。
fn styled_button(
    mut btn: gpui::Stateful<gpui::Div>,
    kind: ButtonKind,
    enabled: bool,
    theme: Theme,
) -> AnyElement {
    if !enabled {
        return btn.text_color(rgb(theme.dim)).into_any_element();
    }
    btn = match kind {
        ButtonKind::Primary => btn
            .bg(rgb(theme.accent))
            .text_color(rgb(theme.accent_dark))
            .hover(|s| s.bg(rgb(theme.accent)).opacity(0.9)),
        ButtonKind::Secondary => btn
            .border_1()
            .border_color(rgb(theme.border))
            .text_color(rgb(theme.muted))
            .hover(|s| s.border_color(rgb(theme.accent)).text_color(rgb(theme.text))),
        ButtonKind::Ghost => btn
            .text_color(rgb(theme.muted))
            .hover(|s| s.bg(theme.surface(theme.card)).text_color(rgb(theme.text))),
        ButtonKind::Danger => btn
            .border_1()
            .border_color(rgb(theme.border))
            .text_color(rgb(theme.error))
            .hover(|s| s.border_color(rgb(theme.error)).bg(rgb(theme.error)).text_color(rgb(theme.accent_dark))),
    };
    btn.into_any_element()
}

/// 构造一个按钮。`on_click` 仅在 enabled 时触发；禁用态呈弱化色。
pub fn button<T: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<gpui::SharedString>,
    kind: ButtonKind,
    size: ButtonSize,
    enabled: bool,
    theme: Theme,
    cx: &Context<T>,
    on_click: impl Fn(&mut T, &ClickEvent, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let btn = div()
        .id(id)
        .h(px(size.height()))
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(size.text_size()))
        .child(label.into())
        .on_click(cx.listener(move |this: &mut T, ev: &ClickEvent, window, cx| {
            if ev.standard_click() && enabled {
                on_click(this, ev, window, cx);
            }
        }));
    styled_button(btn, kind, enabled, theme)
}

/// 实体版按钮：无需处于宿主渲染上下文（对话框底部插槽等 App 级区域用），
/// 点击时在给定实体上执行 on_click（回调形参与 [`button`] 一致）；
/// App 级操作（如 closer.close）经 Context 的 Deref 完成。
pub fn button_for<T: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<gpui::SharedString>,
    kind: ButtonKind,
    size: ButtonSize,
    enabled: bool,
    theme: Theme,
    entity: Entity<T>,
    on_click: impl Fn(&mut T, &ClickEvent, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let btn = div()
        .id(id)
        .h(px(size.height()))
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(size.text_size()))
        .child(label.into())
        .on_click(move |ev: &ClickEvent, window, cx| {
            if ev.standard_click() && enabled {
                entity.update(cx, |t, cx| on_click(t, ev, window, cx));
            }
        });
    styled_button(btn, kind, enabled, theme)
}

/// 图标位按钮：Sm 尺寸 + 居中字形（文字图标或 svg 由宿主拼装成 label 前缀）。
pub fn icon_button<T: 'static>(
    id: impl Into<ElementId>,
    glyph: &str,
    active: bool,
    theme: Theme,
    cx: &Context<T>,
    on_click: impl Fn(&mut T, &ClickEvent, &mut Window, &mut Context<T>) + 'static,
) -> AnyElement {
    let mut btn = div()
        .id(id)
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(13.))
        .child(glyph.to_string())
        .on_click(cx.listener(move |this: &mut T, ev: &ClickEvent, window, cx| {
            if ev.standard_click() {
                on_click(this, ev, window, cx);
            }
        }));
    btn = if active {
        btn.text_color(rgb(theme.accent))
    } else {
        btn.text_color(rgb(theme.muted))
    };
    btn.hover(|s| s.bg(theme.surface(theme.card))).into_any_element()
}
