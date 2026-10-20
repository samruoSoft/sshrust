//! 图标选择对话框内容：按 lucide 官方分类分组的图标网格。
//!
//! 单选（点击选中、双击即确认）；「自动」格 = 清除显式图标、回按名称匹配。
//! 确认经 `IconPickerEvent` 发给对话框的事件回调（open_dialog_ex 路径），
//! 取消/关闭由 footer 的取消按钮或红绿灯完成。

use gpui::{prelude::*, px, rgb, Context, Entity, EventEmitter, IntoElement, ParentElement,
    Render, Styled, Window, div};

use ui_kit::pro::scroll_area;

use crate::icons::{ICON_CATEGORIES, asset_path};
use crate::theme;

pub enum IconPickerEvent {
    /// 回到按名称自动匹配（清除显式图标）。
    Auto,
    /// 使用指定图标（lucide 名称）。
    Picked(String),
}

pub struct IconPicker {
    /// 当前选中（None = 「自动」格被选中）。
    selected: Option<String>,
    scroll: Entity<scroll_area::ScrollAreaState>,
}

impl IconPicker {
    pub fn new(initial: Option<String>, cx: &mut Context<Self>) -> Self {
        let scroll = cx.new(|_| scroll_area::ScrollAreaState::new(scroll_area::next_owner()));
        cx.observe(&scroll, |_, _, cx| cx.notify()).detach();
        // 语言切换：「自动」「推荐」等格内文案即时重绘。
        cx.observe_global::<crate::i18n::I18nGlobal>(|_, cx| cx.notify()).detach();
        // 背景透明度/主题变化：即时重绘（窗口背景模式为开窗快照，重开生效）。
        cx.observe_global::<crate::theme::BgGlobal>(|_, cx| cx.notify()).detach();
        cx.observe_global::<crate::theme::ThemeGlobal>(|_, cx| cx.notify()).detach();
        Self { selected: initial, scroll }
    }

    /// footer「确定」：按当前选中发事件（由对话框回调关窗并写回）。
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(name) => cx.emit(IconPickerEvent::Picked(name)),
            None => cx.emit(IconPickerEvent::Auto),
        }
    }

    /// 单元格：图标格或「自动」格；选中描边高亮；单击选中、双击确认。
    /// `ci` = 分类序号（进元素 id / debug 选择器）。
    ///
    /// **元素 id 必须全局唯一**：同一图标名会出现在多个分类，若只用图标名做 id，
    /// 同名格子会共享 gpui 的 InteractiveElementState（pending_mouse_down），
    /// 鼠标抬起时最先绘制的同名格子会在 Capture 阶段把共享按下状态丢弃，
    /// 其余同名格子全部收不到点击。因此 id 带上所属分类序号。
    fn cell(&self, ci: usize, name: Option<&'static str>, cx: &Context<Self>) -> gpui::AnyElement {
        let kit = theme::current(cx).kit();
        let is_sel = match name {
            Some(n) => self.selected.as_deref() == Some(n),
            None => self.selected.is_none(),
        };
        let inner = match name {
            Some(n) => gpui::svg()
                .path(asset_path(n))
                .size(px(18.))
                .text_color(rgb(if is_sel { kit.accent } else { kit.text }))
                .into_any_element(),
            None => div()
                .text_size(px(9.))
                .text_color(rgb(if is_sel { kit.accent } else { kit.dim }))
                .child(crate::i18n::t(cx, crate::i18n::Tr::Auto))
                .into_any_element(),
        };
        let id: gpui::ElementId = match name {
            Some(n) => gpui::SharedString::from(format!("{ci}-{n}")).into(),
            None => "icon-auto".into(),
        };
        div()
            .id(id)
            .debug_selector(move || match name {
                Some(n) => format!("icon-cell-{ci}-{n}"),
                None => "icon-cell-auto".to_string(),
            })
            .w(px(56.))
            .h(px(42.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(rgb(if is_sel { kit.accent } else { kit.border }))
            .bg(kit.surface(if is_sel { kit.card } else { kit.panel }))
            .hover(|s| s.border_color(rgb(kit.accent)))
            .child(inner)
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _w, cx| {
                if !ev.standard_click() {
                    return;
                }
                match name {
                    Some(n) => this.selected = Some(n.to_string()),
                    None => this.selected = None,
                }
                if ev.click_count() >= 2 {
                    this.confirm(cx);
                } else {
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// 测试用：当前选中的图标名（None = 「自动」）。
    #[cfg(test)]
    fn selected_icon(&self) -> Option<&str> {
        self.selected.as_deref()
    }
}

impl EventEmitter<IconPickerEvent> for IconPicker {}

impl Render for IconPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::current(cx);
        let kit = pal.kit();
        // 列数随窗口宽自适应；分类内按行铺格。
        let w: f32 = window.viewport_size().width.into();
        let cols = ((((w - 48.) / 64.).floor() as usize).max(4)).min(10);

        let mut sections: Vec<gpui::AnyElement> = Vec::new();
        // 「自动」先行（含语义：未显式选图标时按名称匹配）。
        sections.push(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(pal.dim))
                        .child(crate::i18n::t(cx, crate::i18n::Tr::Recommended)),
                )
                .child(div().flex().gap_2().child(self.cell(0, None, cx)))
                .into_any_element(),
        );
        for (ci, (title, names)) in ICON_CATEGORIES.iter().enumerate() {
            let mut rows: Vec<gpui::AnyElement> = Vec::new();
            for chunk in names.chunks(cols) {
                let mut row = div().flex().gap_2();
                for n in chunk {
                    row = row.child(self.cell(ci, Some(n), cx));
                }
                rows.push(row.into_any_element());
            }
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(pal.dim))
                            .child(format!(
                                "{} · {}",
                                crate::i18n::t(cx, crate::i18n::Tr::IconCat(title)),
                                names.len()
                            )),
                    )
                    .children(rows)
                    .into_any_element(),
            );
        }

        div()
            .id("icon-picker-root")
            .size_full()
            .bg(pal.surface(pal.bg))
            .text_color(rgb(pal.text))
            .text_size(px(13.))
            .font_family("System UI")
            .child(
                scroll_area::ScrollArea::new(
                    self.scroll.clone(),
                    div()
                        .w_full()
                        .min_h(gpui::relative(1.))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .p_4()
                        .children(sections),
                )
                .theme(kit),
            )
    }
}

// ---------- 回归：同名图标跨分类的每个格子都必须可点 ----------

#[cfg(test)]
mod tests {
    use super::*;

    /// 同一图标名出现在多个分类时（如 book-open 同时在 Accessibility(0) 与
    /// Accounts(1)），曾因格子共享元素 id（gpui InteractiveElementState 按
    /// GlobalElementId 存取）导致后绘制的同名格子收不到点击；
    /// 修复 = 格子 id 带分类序号。
    ///
    /// 注意断言顺序：必须**先点后绘制的格子**——若先点先绘制者把 selected 设为
    /// book-open，再点死格不改变值，断言会空过。
    #[gpui::test]
    fn duplicate_icon_cells_all_clickable(cx: &mut gpui::TestAppContext) {
        let (picker, cx) = cx.add_window_view(|_w, cx| IconPicker::new(None, cx));
        cx.run_until_parked();

        let click = |cx: &mut gpui::VisualTestContext, selector: &'static str| {
            let b = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} 应已渲染"));
            let c = gpui::Point::new(b.origin.x + b.size.width / 2., b.origin.y + b.size.height / 2.);
            cx.simulate_mouse_move(c, None, Default::default());
            cx.simulate_mouse_down(c, gpui::MouseButton::Left, Default::default());
            cx.simulate_mouse_up(c, gpui::MouseButton::Left, Default::default());
            cx.run_until_parked();
        };
        let selected = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_w, app| picker.update(app, |p, _| p.selected_icon().map(str::to_string)))
        };

        // 1) 后绘制的同名格子（Accounts 的 book-open）必须可点。
        click(cx, "icon-cell-1-book-open");
        assert_eq!(
            selected(cx).as_deref(),
            Some("book-open"),
            "后绘制的同名格子点击后应选中 book-open"
        );

        // 2)「自动」格可点回 None（顺带重置，避免下一步空过）。
        click(cx, "icon-cell-auto");
        assert_eq!(selected(cx), None, "自动格点击后应清除显式选择");

        // 3) 先绘制的同名格子（Accessibility 的 book-open）仍可点。
        click(cx, "icon-cell-0-book-open");
        assert_eq!(
            selected(cx).as_deref(),
            Some("book-open"),
            "先绘制的同名格子点击后应选中 book-open"
        );
    }
}
