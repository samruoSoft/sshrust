//! Tooltip：悬浮提示的**内容视图**（挂到 gpui 元素的 `.tooltip()` 上）。
//!
//! gpui 0.2.2 自带 tooltip 机制：悬停短暂延迟后在鼠标附近弹出、越窗缘自动
//! 翻转、按下/滚动即消失——延迟与定位由框架负责（鼠标锚点，无方位可选），
//! 本组件只负责内容样式：不透明卡片（浮层不吃透明，见 FEATURES.md 总则 3）。
//!
//! 用法（宿主侧）：
//! ```ignore
//! div().id("btn")
//!     .tooltip(move |_, cx| ui_kit::base::tooltip::tip(text, theme, cx))
//! ```
//! 文案在 builder 内取（每次弹出时执行），语言/主题切换后新弹的提示即时生效。

use gpui::{prelude::*, App, AnyView, Render, SharedString, Styled, div, px, rgb};

use crate::theme::Theme;

/// 提示内容视图（Entity 渲染；`tip()` 便捷产出 AnyView）。
pub struct Tooltip {
    text: SharedString,
    /// 可选行首图标（svg 资产路径）。
    icon: Option<SharedString>,
    text_size: f32,
    /// 内容最大宽度（长文案折行），px。
    max_w: f32,
    theme: Theme,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            icon: None,
            text_size: 11.,
            max_w: 280.,
            theme: Theme::DARK,
        }
    }

    /// 行首图标（svg 资产路径，随 muted 文字色）。
    pub fn icon(mut self, svg_path: impl Into<SharedString>) -> Self {
        self.icon = Some(svg_path.into());
        self
    }

    /// 文案字号（默认 11px；图标随字号 +1）。
    pub fn text_size(mut self, size: f32) -> Self {
        self.text_size = size;
        self
    }

    /// 最大宽度（默认 280px，超出折行）。
    pub fn max_w(mut self, w: f32) -> Self {
        self.max_w = w;
        self
    }

    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .id("kit-tooltip")
            .flex()
            .items_center()
            .gap_1_5()
            .px_2()
            .py_1()
            .rounded_md()
            .max_w(px(self.max_w))
            // 浮层不吃透明：整窗模糊/半透明下透底即透视底下文字，必须不透明。
            .bg(rgb(t.card))
            .border_1()
            .border_color(rgb(t.border))
            .shadow_sm()
            .text_size(px(self.text_size))
            .text_color(rgb(t.text))
            .children(self.icon.as_ref().map(|p| {
                gpui::svg()
                    .path(p.clone())
                    .size(px(self.text_size + 1.))
                    .flex_none()
                    .text_color(rgb(t.muted))
            }))
            .child(self.text.clone())
    }
}

/// 便捷入口：构造内容视图并转 `AnyView`（`.tooltip()` builder 的返回值）。
pub fn tip(text: impl Into<SharedString>, theme: Theme, cx: &mut App) -> AnyView {
    cx.new(|_| Tooltip::new(text).theme(theme)).into()
}
