//! 标签页弹出的独立终端窗口。
//!
//! 弹出 = 「移动」：tab（终端实体 + 会话）从主窗口移除挂到本视图，同一时刻只在
//! 一个窗口渲染；关闭本窗口（红绿灯 / ⌘W / 「并入主窗口」）都把 tab 送回主窗口，
//! 会话不断、状态不丢。弹出期间会话断开则转 Failed 收回。
//!
//! 可行性依据：gpui 0.2.2 的 FocusHandle 由 App 级注册表分配（track_focus 在各
//! 窗口 paint 时注册），实体跨窗口移动安全，无需改动终端 fork。

use gpui::{
    prelude::*, px, rgb, Context, Entity, IntoElement, ParentElement, Render, Styled,
    WeakEntity, Window, WindowHandle, div,
};

use sshrust_core::ConnectParams;

use crate::app_ui::{CloseTab, Tab, TabKind};
use crate::icons;
use crate::theme;
use crate::theme::{fmt_bytes, fmt_duration};
use ui_kit::pro::scroll_area;

/// 主窗口登记的一条弹出窗口。
#[derive(Clone)]
pub(crate) struct PoppedWin {
    pub window: WindowHandle<TerminalStandalone>,
    pub entity: Entity<TerminalStandalone>,
    /// 会话连接参数（host_is_live / 收回判定用；随登记增删）。
    pub params: ConnectParams,
}

pub struct TerminalStandalone {
    pub(crate) tab: Option<Tab>,
    main: WeakEntity<crate::app_ui::AppUi>,
    scroll: Entity<scroll_area::ScrollAreaState>,
    /// 会话已断开的转 Failed 材料（收回主窗口时生效）。
    pub(crate) closed: Option<(ConnectParams, String)>,
    /// 已应用到本窗口的背景效果（render 对账，变化时调 set_background_appearance）。
    applied_fx: theme::BgFx,
}

impl TerminalStandalone {
    pub fn new(
        tab: Tab,
        main: WeakEntity<crate::app_ui::AppUi>,
        scroll_owner: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroll = cx.new(|_| scroll_area::ScrollAreaState::new(scroll_owner));
        // 滚动条显隐/锚点在 ScrollAreaState 里，变化要联动本窗口重绘。
        cx.observe(&scroll, |_, _, cx| cx.notify()).detach();
        // 语言切换：独立窗口即时重绘。
        cx.observe_global::<crate::i18n::I18nGlobal>(|_, cx| cx.notify()).detach();
        // 背景透明度：即时重绘（窗口背景模式在 render 里对账）。
        cx.observe_global::<theme::BgGlobal>(|_, cx| cx.notify()).detach();
        // 主题切换（系列翻转改变有效透明度）：即时重绘。
        cx.observe_global::<theme::ThemeGlobal>(|_, cx| cx.notify()).detach();
        Self { tab: Some(tab), main, scroll, closed: None, applied_fx: theme::bg_fx(cx) }
    }

    pub(crate) fn live_params(&self) -> Option<ConnectParams> {
        self.tab.as_ref().and_then(|t| match &t.kind {
            TabKind::Live { session, .. } => Some(session.params.clone()),
            _ => None,
        })
    }

    /// 并回主窗口（⌘W / 按钮 / 断开后调用）：tab 送回主窗口，然后关本窗。
    pub(crate) fn merge_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tab.take();
        let closed = self.closed.take();
        if let (Some(main), Some(tab)) = (self.main.upgrade(), tab) {
            let _ = main.update(cx, |ui, cx| ui.reaccept_tab(tab, closed, cx));
        }
        window.remove_window();
    }

    fn disconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = &self.tab {
            if let TabKind::Live { session, .. } = &tab.kind {
                let params = session.params.clone();
                session.close();
                self.closed =
                    Some((params, crate::i18n::t(cx, crate::i18n::Tr::ReasonManualDisconnect).to_string()));
            }
        }
        self.merge_back(window, cx);
    }
}

impl Render for TerminalStandalone {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 背景效果对账：开窗时的 WindowOptions 只管初值，运行期变化在此同步。
        let fx = theme::bg_fx(cx);
        if fx != self.applied_fx {
            self.applied_fx = fx;
            window.set_background_appearance(fx.window_appearance());
            // 重开模糊会重建 NSVisualEffectView，需按主题系列重新 tint。
            ui_kit::platform::set_blur_tint(!theme::is_light(cx));
        }
        let pal = theme::current(cx);
        let sz = window.viewport_size();
        let (w, h): (f32, f32) = (sz.width.into(), sz.height.into());

        // 根不铺底色：Live 时终端默认背景自带 alpha，再铺会二次压暗；
        // 顶栏/状态栏各自铺表面色，防御分支（会话结束）才整窗铺底。
        let base = div()
            .id("standalone-root")
            .size_full()
            .text_color(rgb(pal.text))
            // ⌘W = 非破坏性并回主窗口（与红绿灯关窗一致）。
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.merge_back(window, cx);
            }));

        let Some(tab) = self.tab.as_ref() else {
            return base.bg(pal.surface(pal.bg)).into_any_element();
        };
        match &tab.kind {
            TabKind::Live { term, session, opened_at } => {
                let term = term.clone();
                let term_handle = term.read(cx).focus_handle().clone();
                if !term_handle.is_focused(window) {
                    window.focus(&term_handle);
                }
                let tab_id = tab.id;
                let (down, up) = session.stats.bytes();
                let target = format!("{}:{}", session.params.host, session.params.port);
                let user = session.params.username.clone();

                // 与主窗口同款：终端区不用嵌套 flex_1（0.2.2 高度塌 0），绝对定位。
                let toolbar_h = 30.;
                let status_h = 26.;
                let term_h = (h - toolbar_h - status_h).max(0.);
                let (offset, max_hist, screen_rows) = term.read(cx).scroll_info();
                let goto = cx.listener({
                    let term = term.clone();
                    move |_this, top_off: &f32, _w, cx| {
                        let (_, max_hist, _) = term.read(cx).scroll_info();
                        let target =
                            (max_hist as f32 - top_off).round().clamp(0., max_hist as f32);
                        term.update(cx, |t, _| t.scroll_to(target as usize));
                        cx.notify();
                    }
                });

                let leading = match &tab.icon {
                    Some(icon_name) => gpui::svg()
                        .path(icons::asset_path(icon_name))
                        .size(px(13.))
                        .flex_none()
                        .text_color(rgb(pal.ok))
                        .into_any_element(),
                    None => div().size(px(7.)).flex_none().rounded_full().bg(rgb(pal.ok))
                        .into_any_element(),
                };

                // 顶栏：标题 + 目标 + [并入主窗口] [断开]
                let toolbar = div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(w))
                    .h(px(toolbar_h))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .bg(pal.surface(pal.panel))
                    .border_b_1()
                    .border_color(rgb(pal.divider()))
                    .child(leading)
                    .child(
                        div()
                            .text_size(px(12.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(tab.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(pal.muted))
                            .child(format!("{target} · {user}")),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(("btn-merge-back", tab_id as usize))
                            .px_2()
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .text_size(px(11.))
                            .text_color(rgb(pal.accent))
                            .border_1()
                            .border_color(rgb(pal.border))
                            .hover(|s| s.border_color(rgb(pal.accent)))
                            .child(crate::i18n::t(cx, crate::i18n::Tr::MergeBack))
                            .on_click(cx.listener(|this, _ev: &gpui::ClickEvent, window, cx| {
                                this.merge_back(window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("btn-pop-disconnect", tab_id as usize))
                            .px_2()
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .text_size(px(11.))
                            .text_color(rgb(pal.error))
                            .border_1()
                            .border_color(rgb(pal.border))
                            .hover(|s| s.border_color(rgb(pal.error)))
                            .child(crate::i18n::t(cx, crate::i18n::Tr::Disconnect))
                            .on_click(cx.listener(
                                |this, _ev: &gpui::ClickEvent, window, cx| {
                                    this.disconnect(window, cx);
                                },
                            )),
                    );

                let term_area = div()
                    .absolute()
                    .top(px(toolbar_h))
                    .left_0()
                    .w(px(w))
                    .h(px(term_h))
                    .child(
                        scroll_area::ScrollArea::virtual_y(
                            self.scroll.clone(),
                            term,
                            (max_hist - offset) as f32,
                            max_hist as f32,
                            screen_rows as f32,
                            term_h,
                            move |top: f32, w, cx| goto(&top, w, cx),
                        )
                        .theme(pal.kit()),
                    );

                let status = div()
                    .absolute()
                    .top(px((h - status_h).max(0.)))
                    .left_0()
                    .w(px(w))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .h(px(status_h))
                    .bg(pal.surface(pal.panel))
                    .border_t_1()
                    .border_color(rgb(pal.divider()))
                    .child(div().text_size(px(11.)).text_color(rgb(pal.ok)).child("●"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(pal.muted))
                            // 控制台下方状态栏展示别名（顶栏另有 host:port 详情）。
                            .child(format!("{} · {}", tab.title, user)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(pal.dim))
                            .child(format!(
                                "↓ {} ↑ {} · {}",
                                fmt_bytes(down),
                                fmt_bytes(up),
                                fmt_duration(crate::i18n::current(cx), opened_at.elapsed().as_secs())
                            )),
                    );

                base.child(toolbar).child(term_area).child(status).into_any_element()
            }
            // 只弹出 Live tab；此分支仅防御（断开瞬间的短暂窗口期）。
            _ => base
                .bg(pal.surface(pal.bg))
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(rgb(pal.muted)).child(crate::i18n::t(cx, crate::i18n::Tr::SessionEnded)))
                .into_any_element(),
        }
    }
}
