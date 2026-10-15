//! Panel：三段式卡片容器（标题栏 + body + 底部工具栏，RenderOnce 无状态）。
//!
//! 三段按需拼装：给出 icon / title / tools 任一即渲染标题栏（图标 → 标题 →
//! 弹性空隙 → 右侧工具按钮区）；给出 footer 即渲染底部工具栏；body 为主体
//! 内容插槽（不给则面板无内容区）。标题栏/底栏高度、内边距、字号由
//! [`PanelSize`] 模板决定；交互元素（按钮等）由宿主经插槽注入，组件不带状态。
//! 背景默认**无色**（不铺底，透出宿主背景）；需要底色时 `.bg(色槽)` 经
//! `theme.surface()` 着色（随宿主透明度/模糊档同步）。
//!
//! ```ignore
//! Panel::new()
//!     .size(PanelSize::Sm)
//!     .icon("icons/cpu.svg")   // svg 资产路径，宿主 AssetSource 解析
//!     .title("CPU")
//!     .tools(pct_badge)        // 右侧工具区（任意元素；值徽标亦可）
//!     .body(rows)
//!     .footer(action_bar)
//!     .theme(theme)
//! ```

use gpui::{prelude::*, AnyElement, App, RenderOnce, SharedString, Window, div, px, rgb};

use crate::Theme;

/// 尺寸模板：标题栏/底栏高度、内边距、图标与标题字号。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelSize {
    /// 紧凑（信息密集面板，如系统信息分区）：标题栏 26、底栏 30、内边距 10。
    Sm,
    /// 常规（表单/通用卡片）：标题栏 32、底栏 38、内边距 14。
    Md,
}

impl PanelSize {
    fn header_h(self) -> f32 {
        match self {
            Self::Sm => 26.,
            Self::Md => 32.,
        }
    }

    fn footer_h(self) -> f32 {
        match self {
            Self::Sm => 30.,
            Self::Md => 38.,
        }
    }

    fn pad(self) -> f32 {
        match self {
            Self::Sm => 10.,
            Self::Md => 14.,
        }
    }

    fn icon_size(self) -> f32 {
        match self {
            Self::Sm => 12.,
            Self::Md => 14.,
        }
    }

    fn title_size(self) -> f32 {
        match self {
            Self::Sm => 11.,
            Self::Md => 12.5,
        }
    }

    /// body 水平可用宽的总内耗（边框 1×2 + 内边距×2），宿主算内容宽用。
    pub fn content_inset(self) -> f32 {
        2. + self.pad() * 2.
    }
}

/// 三段式面板容器（无状态外观件）。
#[derive(IntoElement)]
pub struct Panel {
    size: PanelSize,
    icon: Option<SharedString>,
    title: Option<SharedString>,
    tools: Option<AnyElement>,
    body: Option<AnyElement>,
    footer: Option<AnyElement>,
    /// 背景表面色（None = 无色：不铺底，透出宿主背景）。
    bg: Option<u32>,
    flush_body: bool,
    bordered: bool,
    theme: Theme,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            size: PanelSize::Md,
            icon: None,
            title: None,
            tools: None,
            body: None,
            footer: None,
            bg: None,
            flush_body: false,
            bordered: true,
            theme: Theme::DARK,
        }
    }

    /// 尺寸模板（默认 Md）。
    pub fn size(mut self, size: PanelSize) -> Self {
        self.size = size;
        self
    }

    /// 标题栏图标（svg 资产路径，宿主 AssetSource 解析；随主题弱化色着色）。
    pub fn icon(mut self, path: impl Into<SharedString>) -> Self {
        self.icon = Some(path.into());
        self
    }

    /// 标题文本（溢出尾部省略）。
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// 标题栏右侧工具按钮区（任意元素；按钮自带监听，值徽标亦可放此槽）。
    pub fn tools(mut self, el: impl IntoElement) -> Self {
        self.tools = Some(el.into_any_element());
        self
    }

    /// 主体内容插槽（不给则无内容区）。
    pub fn body(mut self, el: impl IntoElement) -> Self {
        self.body = Some(el.into_any_element());
        self
    }

    /// 底部工具栏插槽（不给则无底栏；顶边框与标题栏底边框分隔）。
    pub fn footer(mut self, el: impl IntoElement) -> Self {
        self.footer = Some(el.into_any_element());
        self
    }

    /// 背景表面色：经 `theme.surface()` 着色，随宿主的背景透明度/模糊档
    /// 同步（毛玻璃档半透明透入、不透明档实色）。**默认不给 = 无色**
    /// （面板不铺底，透出宿主背景，仅靠边框分隔）。
    pub fn bg(mut self, color: u32) -> Self {
        self.bg = Some(color);
        self
    }

    /// body 满铺：不加内边距，内容贴卡片内缘（图表/列表全出血用）。
    pub fn flush_body(mut self) -> Self {
        self.flush_body = true;
        self
    }

    /// 边框开关（默认开）。
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// 注入配色。
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Panel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self { size, icon, title, tools, body, footer, bg, flush_body, bordered, theme } = self;

        let mut el = div()
            .flex()
            .flex_col()
            .w_full()
            .min_h_0()
            .overflow_hidden()
            .rounded_md();
        if let Some(color) = bg {
            el = el.bg(theme.surface(color));
        }
        if bordered {
            el = el.border_1().border_color(rgb(theme.border));
        }

        // 标题栏：icon / title / tools 任一给出才渲染。solo（无 body 且无
        // footer）时省去底边框，避免与卡片外框叠成双线。
        if icon.is_some() || title.is_some() || tools.is_some() {
            let solo = body.is_none() && footer.is_none();
            let mut head = div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(size.header_h()))
                .px(px(size.pad()));
            if !solo {
                head = head.border_b_1().border_color(rgb(theme.border));
            }
            if let Some(path) = icon {
                head = head.child(
                    gpui::svg()
                        .path(path)
                        .size(px(size.icon_size()))
                        .flex_none()
                        // 图标自带四周默认间隔（mr_1）：与标题的距离即此间隔，
                        // 标题左侧不加额外 gap。
                        .mr_1()
                        .text_color(rgb(theme.dim)),
                );
            }
            if let Some(t) = title {
                head = head.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(size.title_size()))
                        .text_color(rgb(theme.muted))
                        .child(t),
                );
            }
            head = head.child(div().flex_1());
            if let Some(t) = tools {
                head = head.child(t);
            }
            el = el.child(head);
        }

        // body：包一层弹性容器（面板定高时占满余量、自然高度时随内容）。
        if let Some(body) = body {
            let mut wrap = div().flex().flex_col().flex_1().min_h_0();
            if !flush_body {
                wrap = wrap.p(px(size.pad()));
            }
            el = el.child(wrap.child(body));
        }

        // 底部工具栏。
        if let Some(footer) = footer {
            el = el.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .h(px(size.footer_h()))
                    .px(px(size.pad()))
                    .border_t_1()
                    .border_color(rgb(theme.border))
                    .child(footer),
            );
        }
        el
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内耗口径：边框 1×2 + 内边距×2（宿主按此算 body 可用宽，改动须同步）。
    #[test]
    fn content_inset_math() {
        assert_eq!(PanelSize::Sm.content_inset(), 22.);
        assert_eq!(PanelSize::Md.content_inset(), 30.);
    }
}
