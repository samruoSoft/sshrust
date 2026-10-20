//! 「关于 SSHRust」对话框内容：应用图标块 / 名称 / 版本 / 简介 / GitHub 主页链接。
//!
//! 宿主是 ui-kit DialogWindow（app_ui::open_about 打开，固定尺寸不可 resize）；
//! 红绿灯关闭即弃（无状态，重开重建）。

use gpui::{
    px, rgb, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div,
};

/// 图标块的深空底色（与 scripts/make_icon.py 应用图标同系：#1a1b26 → #24283b 渐变）。
const ICON_BG: u32 = 0x1a1b26;

pub struct AboutView;

impl AboutView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // 语言/主题系列/背景透明度变化即时重绘（独立窗口不挂 AppUi 根下）。
        cx.observe_global::<crate::i18n::I18nGlobal>(|_, cx| cx.notify()).detach();
        cx.observe_global::<crate::theme::BgGlobal>(|_, cx| cx.notify()).detach();
        cx.observe_global::<crate::theme::ThemeGlobal>(|_, cx| cx.notify()).detach();
        Self
    }
}

impl Render for AboutView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = crate::theme::current(cx);
        let loc = crate::i18n::current(cx);
        let t = |k: crate::i18n::Tr| crate::i18n::tl(loc, k);

        // 近似应用图标的色块：深空底 + 终端提示符 ">_"（真 icns 仅打进 .app 包）。
        let icon = div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(72.))
            .rounded_lg()
            .bg(rgb(ICON_BG))
            .border_1()
            .border_color(rgb(pal.border))
            .text_color(rgb(0xffffff))
            .font_family("Menlo")
            .text_size(px(22.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(">_");

        div()
            .id("about-root")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .px_6()
            .bg(pal.surface(pal.bg))
            .text_color(rgb(pal.text))
            .text_size(px(13.))
            .font_family("System UI")
            .child(icon)
            .child(
                div()
                    .mt_3()
                    .text_size(px(20.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("SSHRust"),
            )
            .child(
                div()
                    .text_color(rgb(pal.muted))
                    .child(format!("{} {}", t(crate::i18n::Tr::AboutVersion), env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .mt_2()
                    .text_color(rgb(pal.muted))
                    .child(t(crate::i18n::Tr::AboutTagline)),
            )
            .child(
                div()
                    .id("about-homepage")
                    .mt_3()
                    .text_size(px(12.5))
                    .text_color(rgb(pal.accent))
                    .hover(|s| s.underline())
                    .cursor(gpui::CursorStyle::PointingHand)
                    .child(t(crate::i18n::Tr::AboutHomepage))
                    .on_click(|_ev, _w, _cx| {
                        let _ = std::process::Command::new("open")
                            .arg("https://github.com/samruoSoft/sshrust")
                            .spawn();
                    }),
            )
    }
}
