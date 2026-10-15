//! base 系列：基础控件（按钮 / 表单基元 / 模态浮层 / 锚定下拉菜单 / 悬浮提示 /
//! 面板容器）。

pub mod button;
pub mod dropdown;
pub mod form;
pub mod modal;
pub mod panel;
pub mod text_area;
pub mod text_input;
pub mod tooltip;

pub(crate) mod field_core;

use gpui::prelude::*;

/// 拖拽会话需要的跟随视图（gpui 约定）；全透明 1px 使其不可见。
/// 库内组件（滚动条等）共用，宿主自定义拖拽也可复用。
pub struct DragGhost;

impl gpui::Render for DragGhost {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::div().size(gpui::px(1.)).opacity(0.)
    }
}
