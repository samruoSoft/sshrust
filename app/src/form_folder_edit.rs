//! 编辑目录（文件夹）名称的弹出独立窗口：单字段 input + footer 保存/取消。
//!
//! 树形内联重命名已全部取消：服务器/命令的改名走各自编辑表单，
//! 目录的「编辑…」走本窗口（Esc/红绿灯/取消即放弃，无副作用）。

use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, WeakEntity, Window,
    actions, div, prelude::*, px, rgb,
};

use crate::app_ui::AppUi;
use crate::theme;
use ui_kit::base::form as kit_form;
use ui_kit::base::text_input::{InputField, InputSize, InputType, TextField};

actions!(form_folder_edit, [CancelFolderForm]);

/// 保存成功（表单窗口自杀闭信号；内容已回主窗落库）。
pub enum FolderFormEvent {
    Saved,
}

/// 保存时交给主窗口落库的表单产物。
pub(crate) struct FolderFormOutput {
    pub id: i64,
    pub name: String,
}

pub struct FolderEditForm {
    main: WeakEntity<AppUi>,
    id: i64,
    name_f: Entity<TextField>,
    error: Option<String>,
}

impl FolderEditForm {
    pub fn new(id: i64, name: String, main: WeakEntity<AppUi>, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<crate::i18n::I18nGlobal>(|this, cx| {
            let ph = crate::i18n::t(cx, crate::i18n::Tr::PhFolderName).to_string();
            this.name_f.update(cx, |f, cx| f.set_placeholder(ph, cx));
            cx.notify();
        })
        .detach();
        // 背景透明度/主题变化：即时重绘（窗口背景模式为开窗快照，重开生效）。
        cx.observe_global::<crate::theme::BgGlobal>(|_, cx| cx.notify()).detach();
        cx.observe_global::<crate::theme::ThemeGlobal>(|_, cx| cx.notify()).detach();
        let this = Self {
            main,
            id,
            name_f: cx.new(|cx| {
                TextField::new(crate::i18n::t(cx, crate::i18n::Tr::PhFolderName), InputType::Text, cx)
            }),
            error: None,
        };
        this.name_f.update(cx, |f, cx| f.set_value(name, cx));
        this
    }

    /// 复用已开窗口时重载状态。
    pub fn load_for(&mut self, id: i64, name: String, cx: &mut Context<Self>) {
        self.id = id;
        self.error = None;
        self.name_f.update(cx, |f, cx| f.set_value(name, cx));
        cx.notify();
    }

    /// footer「保存」：校验 → 回主窗改名 → 发 Saved（dialog 回调里关窗）。
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name_f.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some(crate::i18n::t(cx, crate::i18n::Tr::ErrFolderNameEmpty).into());
            cx.notify();
            return;
        }
        self.error = None;
        let out = FolderFormOutput { id: self.id, name };
        if let Some(main) = self.main.upgrade() {
            let _ = main.update(cx, |ui, cx| ui.persist_folder_form(out, cx));
        }
        cx.emit(FolderFormEvent::Saved);
    }
}

impl EventEmitter<FolderFormEvent> for FolderEditForm {}

impl Render for FolderEditForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::current(cx);
        let kit = pal.kit();

        let body = div()
            .id("folder-form-body")
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .min_h(gpui::relative(1.))
            .p_6()
            .child(kit_form::labeled_row(
                crate::i18n::t(cx, crate::i18n::Tr::LabelFolderName),
                InputField::new(self.name_f.clone()).size(InputSize::Md).theme(kit),
                kit,
            ))
            .children(self.error.clone().map(|msg| {
                div()
                    .text_color(rgb(pal.error))
                    .text_size(px(12.))
                    .child(msg)
                    .into_any_element()
            }))
            .child(div().flex_1());

        div()
            .id("folder-form-window-root")
            .key_context("form_folder_edit")
            .size_full()
            .flex()
            .bg(pal.surface(pal.bg))
            // 独立窗口不挂 AppUi 根下：颜色/字号必须自带，否则回落默认黑字。
            .text_color(rgb(pal.text))
            .text_size(px(13.))
            .font_family("System UI")
            .on_action(cx.listener(|_this, _: &CancelFolderForm, window, _cx| {
                window.remove_window();
            }))
            .on_action(cx.listener(|_this, _: &crate::app_ui::CloseTab, window, _cx| {
                window.remove_window();
            }))
            .child(body)
    }
}
