//! 新增/编辑常用命令的弹出独立窗口。
//!
//! 字段：标题（input）/ 说明（input）/ 命令内容（textarea）；
//! 底部工具栏为 footer 插槽（保存 / 取消），保存经 WeakEntity<AppUi> 回主窗落库，
//! 关闭（红绿灯/⌘W/Esc）即放弃，无副作用。与 form_service_edit 同构。

use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, WeakEntity, Window,
    actions, div, prelude::*, px, rgb,
};

use crate::app_ui::AppUi;
use crate::db::CommandData;
use crate::theme;
use ui_kit::base::form as kit_form;
use ui_kit::base::text_area::{TextArea, TextAreaField};
use ui_kit::base::text_input::{InputField, InputSize, InputType, TextField};

actions!(form_command_edit, [CancelCmdForm]);

/// 保存成功（表单窗口自杀闭信号；内容已回主窗落库）。
pub enum CommandFormEvent {
    Saved,
}

/// 保存时交给主窗口落库的表单产物。
pub(crate) struct CommandFormOutput {
    pub editing: Option<i64>,
    pub parent: i64,
    pub title: String,
    pub description: String,
    pub command: String,
}

/// 打开表单时的初始数据：None = 新增；Some((节点 id, 名称, 数据)) = 编辑。
pub type CmdFormLoad = Option<(i64, String, CommandData)>;

pub struct CommandEditForm {
    main: WeakEntity<AppUi>,
    editing: Option<i64>,
    parent: i64,
    /// 内容滚动（窗口矮于表单自然高度时兜底；kit scroll_area 真实模式）。
    scroll: Entity<ui_kit::pro::scroll_area::ScrollAreaState>,
    title_f: Entity<TextField>,
    desc_f: Entity<TextField>,
    cmd_f: Entity<TextArea>,
    error: Option<String>,
}

impl CommandEditForm {
    pub fn new(parent: i64, load: CmdFormLoad, main: WeakEntity<AppUi>, cx: &mut Context<Self>) -> Self {
        let scroll = cx.new(|_| {
            ui_kit::pro::scroll_area::ScrollAreaState::new(ui_kit::pro::scroll_area::next_owner())
        });
        cx.observe(&scroll, |_, _, cx| cx.notify()).detach();
        // 语言切换：占位字即时更新。
        cx.observe_global::<crate::i18n::I18nGlobal>(|this, cx| {
            let ph_title = crate::i18n::t(cx, crate::i18n::Tr::PhCmdTitle).to_string();
            let ph_desc = crate::i18n::t(cx, crate::i18n::Tr::PhCmdDesc).to_string();
            let ph_cmd = crate::i18n::t(cx, crate::i18n::Tr::PhCmdContent).to_string();
            this.title_f.update(cx, |f, cx| f.set_placeholder(ph_title, cx));
            this.desc_f.update(cx, |f, cx| f.set_placeholder(ph_desc, cx));
            this.cmd_f.update(cx, |f, cx| f.set_placeholder(ph_cmd, cx));
            cx.notify();
        })
        .detach();
        // 背景透明度/主题变化：即时重绘（窗口背景模式为开窗快照，重开生效）。
        cx.observe_global::<crate::theme::BgGlobal>(|_, cx| cx.notify()).detach();
        cx.observe_global::<crate::theme::ThemeGlobal>(|_, cx| cx.notify()).detach();
        let this = Self {
            main,
            editing: None,
            parent,
            scroll,
            title_f: cx.new(|cx| {
                TextField::new(crate::i18n::t(cx, crate::i18n::Tr::PhCmdTitle), InputType::Text, cx)
            }),
            desc_f: cx.new(|cx| {
                TextField::new(crate::i18n::t(cx, crate::i18n::Tr::PhCmdDesc), InputType::Text, cx)
            }),
            cmd_f: cx.new(|cx| {
                TextArea::new(crate::i18n::t(cx, crate::i18n::Tr::PhCmdContent), 8, cx)
            }),
            error: None,
        };
        let mut this = this;
        this.apply_load(load, cx);
        this
    }

    /// 载入/重置（复用已开窗口时也走这里）。
    fn apply_load(&mut self, load: CmdFormLoad, cx: &mut Context<Self>) {
        self.error = None;
        match load {
            Some((id, name, data)) => {
                self.editing = Some(id);
                self.title_f.update(cx, |f, cx| f.set_value(name, cx));
                self.desc_f.update(cx, |f, cx| f.set_value(data.description, cx));
                self.cmd_f.update(cx, |f, cx| f.set_value(data.command, cx));
            }
            None => {
                self.editing = None;
                self.title_f.update(cx, |f, cx| f.set_value("", cx));
                self.desc_f.update(cx, |f, cx| f.set_value("", cx));
                self.cmd_f.update(cx, |f, cx| f.set_value("", cx));
            }
        }
        cx.notify();
    }

    /// 复用已开窗口时重载状态（新增/编辑切换）。
    pub fn load_for(&mut self, parent: i64, load: CmdFormLoad, cx: &mut Context<Self>) {
        self.parent = parent;
        self.apply_load(load, cx);
    }

    /// footer「保存」：校验 → 回主窗落库 → 发 Saved（dialog 回调里关窗）。
    pub fn save(&mut self, cx: &mut Context<Self>) {
        let Some(out) = self.collect(cx) else {
            cx.notify();
            return;
        };
        if let Some(main) = self.main.upgrade() {
            let _ = main.update(cx, |ui, cx| ui.persist_command_form(out, cx));
        }
        cx.emit(CommandFormEvent::Saved);
    }

    fn collect(&mut self, cx: &gpui::App) -> Option<CommandFormOutput> {
        let title = self.title_f.read(cx).value().trim().to_string();
        if title.is_empty() {
            self.error = Some(crate::i18n::t(cx, crate::i18n::Tr::ErrCmdTitleEmpty).into());
            return None;
        }
        let command = self.cmd_f.read(cx).value().trim().to_string();
        if command.is_empty() {
            self.error = Some(crate::i18n::t(cx, crate::i18n::Tr::ErrCmdContentEmpty).into());
            return None;
        }
        let description = self.desc_f.read(cx).value().trim().to_string();
        self.error = None;
        Some(CommandFormOutput {
            editing: self.editing,
            parent: self.parent,
            title,
            description,
            command,
        })
    }
}

impl EventEmitter<CommandFormEvent> for CommandEditForm {}

impl Render for CommandEditForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::current(cx);
        let kit = pal.kit();
        let tr = |k: crate::i18n::Tr| crate::i18n::t(cx, k);
        let field =
            |f: &Entity<TextField>| InputField::new(f.clone()).size(InputSize::Md).theme(kit);

        let body = div()
            .id("cmd-form-body")
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .min_h(gpui::relative(1.))
            .p_6()
            .child(kit_form::labeled_row(
                tr(crate::i18n::Tr::LabelCmdTitle),
                field(&self.title_f),
                kit,
            ))
            .child(kit_form::labeled_row(
                tr(crate::i18n::Tr::LabelCmdDesc),
                field(&self.desc_f),
                kit,
            ))
            .child(kit_form::labeled_block(
                tr(crate::i18n::Tr::LabelCmdContent),
                TextAreaField::new(self.cmd_f.clone()).theme(kit),
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
            .id("cmd-form-window-root")
            .key_context("form_command_edit")
            .size_full()
            .flex()
            .bg(pal.surface(pal.bg))
            // 独立窗口不挂 AppUi 根下：颜色/字号必须自带，否则回落默认黑字。
            .text_color(rgb(pal.text))
            .text_size(px(13.))
            .font_family("System UI")
            .on_action(cx.listener(|_this, _: &CancelCmdForm, window, _cx| {
                window.remove_window();
            }))
            .on_action(cx.listener(|_this, _: &crate::app_ui::CloseTab, window, _cx| {
                window.remove_window();
            }))
            .child(ui_kit::pro::scroll_area::ScrollArea::new(self.scroll.clone(), body).theme(kit))
    }
}
