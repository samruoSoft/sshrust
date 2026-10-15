//! 锚定下拉/右键菜单浮层：通用条目模型 + 纯函数几何 + 渲染。
//!
//! 条目 `MenuEntry<A>` 泛型于动作值，宿主用自己的枚举；
//! Header/Sep 为纯展示。`trailing` 可挂尾部元素（如终端配色色块预览）。
//!
//! 已知坑（沿用侧栏菜单实测结论）：
//! - 面板上不能挂 mouse-down 关闭（吞内部 click 配对）；
//!   关闭路径：点项 → 宿主回调内关；点外部 → backdrop。
//! - 高度按条目类型估算（Item26 / Header22 / Sep9），弹出位置钳制在窗口内。

use gpui::{prelude::*, AnyElement, Context, IntoElement, Point, Pixels, Window, div, px, rgb};

use crate::Theme;

/// 菜单条目。
pub enum MenuEntry<A> {
    /// 不可点的小节标题。
    Header(String),
    Item {
        label: String,
        action: A,
        danger: bool,
        enabled: bool,
        /// 行首图标（svg 资源路径，如 "icons/trash-2.svg"；None = 无）。
        icon: Option<String>,
        /// 行尾附加元素（色块预览等）。
        trailing: Option<AnyElement>,
    },
    Sep,
}

/// 便捷构造。
pub fn item<A>(label: &str, action: A) -> MenuEntry<A> {
    MenuEntry::Item { label: label.into(), action, danger: false, enabled: true, icon: None, trailing: None }
}

pub fn danger_item<A>(label: &str, action: A) -> MenuEntry<A> {
    MenuEntry::Item { label: label.into(), action, danger: true, enabled: true, icon: None, trailing: None }
}

pub fn disabled_item<A>(label: &str) -> MenuEntry<A>
where
    A: Default,
{
    MenuEntry::Item { label: label.into(), action: A::default(), danger: false, enabled: false, icon: None, trailing: None }
}

/// 带行首图标的条目（icon 为 svg 资源路径）。
pub fn icon_item<A>(icon: &str, label: &str, action: A) -> MenuEntry<A> {
    MenuEntry::Item { label: label.into(), action, danger: false, enabled: true, icon: Some(icon.into()), trailing: None }
}

pub fn icon_danger_item<A>(icon: &str, label: &str, action: A) -> MenuEntry<A> {
    MenuEntry::Item { label: label.into(), action, danger: true, enabled: true, icon: Some(icon.into()), trailing: None }
}

pub fn icon_disabled_item<A>(icon: &str, label: &str, action: A) -> MenuEntry<A> {
    MenuEntry::Item { label: label.into(), action, danger: false, enabled: false, icon: Some(icon.into()), trailing: None }
}

pub const MENU_ITEM_H: f32 = 26.0;
pub const MENU_HEADER_H: f32 = 22.0;
pub const MENU_SEP_H: f32 = 9.0;
pub const MENU_W: f32 = 192.0;

/// 估算菜单高度（纯函数，测试用）。
pub fn menu_height<A>(entries: &[MenuEntry<A>]) -> f32 {
    entries
        .iter()
        .map(|e| match e {
            MenuEntry::Item { .. } => MENU_ITEM_H,
            MenuEntry::Header(_) => MENU_HEADER_H,
            MenuEntry::Sep => MENU_SEP_H,
        })
        .sum::<f32>()
        + 8.
}

/// 弹出位置钳制在窗口内（纯函数，测试用）。返回 (left, top)。
pub fn clamp_origin(origin: Point<Pixels>, vw: f32, vh: f32, menu_w: f32, menu_h: f32) -> (f32, f32) {
    let (ox, oy): (f32, f32) = (origin.x.into(), origin.y.into());
    let top = oy.clamp(0., (vh - menu_h - 4.).max(0.));
    let left = ox.clamp(0., (vw - menu_w - 4.).max(0.));
    (left, top)
}

/// 渲染菜单浮层（backdrop + 面板）。`origin` 为期望弹出点（鼠标位置）。
/// 点击可用条目 → `on_select`（宿主在回调里自行关闭菜单）；
/// 点击外部空白 → `on_dismiss`。
///
/// 返回 None 表示条目为空（调用方免渲染）。
pub fn overlay<T: 'static, A: Clone + 'static>(
    entries: Vec<MenuEntry<A>>,
    origin: Point<Pixels>,
    menu_w: f32,
    theme: Theme,
    window: &Window,
    cx: &Context<T>,
    on_select: impl Fn(&mut T, A, &mut Window, &mut Context<T>) + 'static,
    on_dismiss: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> Option<AnyElement> {
    if entries.is_empty() {
        return None;
    }
    let on_select = std::rc::Rc::new(on_select);
    let vh: f32 = window.viewport_size().height.into();
    let vw: f32 = window.viewport_size().width.into();
    let menu_h = menu_height(&entries);
    let (left, top) = clamp_origin(origin, vw, vh, menu_w, menu_h);

    let backdrop = div()
        .id("menu-backdrop")
        .absolute()
        .size_full()
        .occlude()
        .on_any_mouse_down(cx.listener(move |this: &mut T, _ev: &gpui::MouseDownEvent, window, cx| {
            on_dismiss(this, window, cx);
        }));

    let mut items: Vec<AnyElement> = Vec::new();
    for (i, entry) in entries.into_iter().enumerate() {
        match entry {
            MenuEntry::Sep => items.push(
                div()
                    .id(("menu-sep", i))
                    .h(px(1.))
                    .my_1()
                    .mx_2()
                    .bg(rgb(theme.border))
                    .into_any_element(),
            ),
            MenuEntry::Header(text) => items.push(
                div()
                    .id(("menu-header", i))
                    .flex()
                    .items_center()
                    .h(px(MENU_HEADER_H))
                    .px_3()
                    .text_size(px(10.5))
                    .text_color(rgb(theme.muted))
                    .child(text)
                    .into_any_element(),
            ),
            MenuEntry::Item { label, action, danger, enabled, icon, trailing } => {
                let icon_el = icon.as_ref().map(|path| {
                    // 不显式着色：继承行文字色（hover 随行翻转）。
                    gpui::svg().path(path.clone()).size(px(13.)).flex_none().into_any_element()
                });
                let mut base = div()
                    .id(("menu-item", i))
                    .debug_selector(move || format!("menu-item-{i}"))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(MENU_ITEM_H))
                    .px_3()
                    .text_size(px(12.5));
                if let Some(ic) = icon_el {
                    base = base.child(ic);
                }
                base = base.child(label.clone());
                if let Some(t) = trailing {
                    base = base.child(div().flex_1()).child(t);
                }
                let item = if enabled {
                    base.text_color(rgb(if danger { theme.error } else { theme.text }))
                        .hover(|s| {
                            if danger {
                                s.bg(rgb(theme.error)).text_color(rgb(theme.accent_dark))
                            } else {
                                s.bg(rgb(theme.accent)).text_color(rgb(theme.accent_dark))
                            }
                        })
                        .on_click({
                            let on_select = on_select.clone();
                            cx.listener(move |this: &mut T, _ev: &gpui::ClickEvent, window, cx| {
                                on_select(this, action.clone(), window, cx);
                            })
                        })
                } else {
                    base.text_color(rgb(theme.dim))
                };
                items.push(item.into_any_element());
            }
        }
    }

    let panel = div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(menu_w))
        .py_1()
        .rounded_md()
        .bg(rgb(theme.card))
        // 浮层不吃窗口模糊（NSVisualEffectView 是整窗级）：底色必须不透明，
        // 否则菜单透出底下的文字，和「直接透明」一样不可读。
        .border_1()
        .border_color(rgb(theme.border))
        .occlude()
        .children(items);

    Some(
        div()
            .absolute()
            .size_full()
            .child(backdrop)
            .child(panel)
            .into_any_element(),
    )
}
