//! 模态浮层：全屏遮罩 + 居中卡片 + Esc 关闭。
//!
//! 用法（宿主侧三件套）：
//! 1. 打开时 `ModalHandle::new(cx)` 存入宿主状态并 `window.focus(&handle.focus)`
//!    ——Esc 依赖焦点落在模态上下文内才派发；
//! 2. 渲染时把 [`overlay`] 挂在最顶层；
//! 3. `bind_keys(cx)` 注册 Esc（仅 `ui_kit_modal` 上下文内生效，不干扰终端 Esc）。
//!
//! 关闭路径：点遮罩空白 / Esc → `on_dismiss`；点卡片内按钮由 body 自行处理。
//! 卡片本体**不能**挂 mouse-down 关闭——按下即触发会把内部按钮的
//! click（down+up 配对）拆散，表现为“点了没反应”（实测踩坑）。

use gpui::{
    actions, prelude::*, AnyElement, App, Context, FocusHandle, KeyBinding, Window, div, px, rgb,
};

use crate::Theme;

actions!(ui_kit_modal, [ModalDismiss]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", ModalDismiss, Some("ui_kit_modal"))]);
}

/// 模态焦点句柄（Esc 派发的落点）。每个模态实例一个。
pub struct ModalHandle {
    pub focus: FocusHandle,
}

impl ModalHandle {
    pub fn new(cx: &mut App) -> Self {
        Self { focus: cx.focus_handle() }
    }
}

/// 渲染模态浮层。`body` 为卡片内容（宽 `width`，高度随内容）；
/// 点遮罩或按 Esc 调用 `on_dismiss`。
pub fn overlay<T: 'static>(
    handle: &ModalHandle,
    title: &str,
    width: f32,
    body: AnyElement,
    theme: Theme,
    cx: &Context<T>,
    on_dismiss: impl Fn(&mut T, &mut Window, &mut Context<T>) + Clone + 'static,
) -> AnyElement {
    let dismiss_click = on_dismiss.clone();
    let backdrop = div()
        .id("modal-backdrop")
        .absolute()
        .size_full()
        .bg(gpui::black().opacity(0.35))
        .occlude()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |this: &mut T, _ev: &gpui::MouseDownEvent, window, cx| {
                dismiss_click(this, window, cx);
            }),
        );

    let dismiss_esc = on_dismiss;
    let card = div()
        .id("modal-card")
        .key_context("ui_kit_modal")
        .track_focus(&handle.focus)
        // 卡片内任意按下都阻断冒泡：gpui 把鼠标事件分发给帧内全部监听器，
        // 兄弟层的遮罩 on_mouse_down 否则会先于 click 配对完成关闭模态，
        // 表现为"点卡片内按钮没反应、模态直接消失"。
        .on_mouse_down(gpui::MouseButton::Left, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .w(px(width))
        .p_6()
        .rounded_lg()
        .bg(rgb(theme.card))
        // 浮层不吃窗口模糊：底色不透明（同 dropdown 菜单）。
        .border_1()
        .border_color(rgb(theme.border))
        .shadow_lg()
        .on_action(cx.listener(move |this: &mut T, _: &ModalDismiss, window, cx| {
            dismiss_esc(this, window, cx);
        }))
        .child(
            div()
                .mb_4()
                .text_size(px(15.))
                .text_color(rgb(theme.text))
                .child(title.to_string()),
        )
        .child(body);

    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(backdrop)
        .child(card)
        .into_any_element()
}
