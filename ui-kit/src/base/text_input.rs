//! 文本输入框（单行，基础组件）：状态实体 `TextField` + 样式包装 `InputField`。
//! 多行输入用 [`crate::base::text_area::TextArea`]（独立组件）。
//!
//! ## 通用属性（InputField 包装器，builder 链式）
//! - 边框：`.bordered(bool)`（默认有；聚焦时自动高亮为强调色）
//! - 提示字：构造参数 placeholder
//! - 图标：`.leading(el)` / `.trailing(el)`（任意元素）及 `.leading_svg(path)` 快捷方式
//! - 尺寸：`.size(InputSize::Sm | Md)`
//! - 字体：`.mono(bool)` 等宽
//!
//! ## 输入类型（TextField 实体）
//! - `InputType::Text`：普通文本
//! - `InputType::Password`：掩码圆点显示
//! - `InputType::Number`：仅数字（提交/粘贴时过滤非数字字符）
//!
//! ## 组件事件（`cx.subscribe(&field, …)` 订阅）
//! - `TextFieldEvent::Change`：内容变化（用户输入；set_value 不触发）
//! - `TextFieldEvent::PressEnter`：回车（同时仍派发 SubmitInput action 供宿主全局处理）
//! - `TextFieldEvent::Focus` / `Blur`：焦点进入/离开
//!
//! ## 交互能力
//! 单击定位、拖拽选区、shift+点击扩展、双击选词、三击全选、
//! IME（中文输入）、复制粘贴剪切、超宽内容横向滚动跟随光标。
//!
//! 约定：key_context 为 "TextField"（宿主的 Esc 取消重命名等绑定依赖它）。

use std::ops::Range;

use gpui::{
    actions, AnyElement, App, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point,
    RenderOnce, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
    div, fill, hsla, point, prelude::*, px, rgb, relative, rgba, size,
};

use super::field_core::{word_range_at, FieldCore};

actions!(
    text_input,
    [
        Backspace, Delete, Left, Right, SelectLeft, SelectRight, SelectAll, Home, End,
        ShowCharacterPalette, Paste, Cut, Copy, SubmitInput,
    ]
);

/// 注册 TextField 所需的键位绑定（仅 "TextField" 上下文内生效）。
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextField")),
        KeyBinding::new("delete", Delete, Some("TextField")),
        KeyBinding::new("left", Left, Some("TextField")),
        KeyBinding::new("right", Right, Some("TextField")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextField")),
        KeyBinding::new("shift-right", SelectRight, Some("TextField")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextField")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextField")),
        KeyBinding::new("cmd-v", Paste, Some("TextField")),
        KeyBinding::new("cmd-c", Copy, Some("TextField")),
        KeyBinding::new("cmd-x", Cut, Some("TextField")),
        KeyBinding::new("home", Home, Some("TextField")),
        KeyBinding::new("end", End, Some("TextField")),
        KeyBinding::new("enter", SubmitInput, Some("TextField")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextField")),
    ]);
}

/// 输入类型：决定显示与输入控制。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InputType {
    #[default]
    Text,
    /// 掩码圆点显示。
    Password,
    /// 仅数字：提交/粘贴时过滤非数字字符。
    Number,
}

/// 组件事件（cx.subscribe 订阅）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextFieldEvent {
    /// 内容变化（用户输入路径；set_value 程序赋值不触发）。
    Change,
    /// 回车按下。
    PressEnter,
    /// 获得焦点。
    Focus,
    /// 失去焦点。
    Blur,
}

pub struct TextField {
    pub focus_handle: FocusHandle,
    core: FieldCore,
    placeholder: SharedString,
    /// 掩码显示（InputType::Password 推导）。
    pub mask: bool,
    input_type: InputType,
    /// 焦点状态缓存（render 时对比并发 Focus/Blur 事件）。
    had_focus: bool,
}

impl gpui::EventEmitter<TextFieldEvent> for TextField {}

impl TextField {
    pub fn new(placeholder: &str, input_type: InputType, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            core: FieldCore::new(input_type == InputType::Number),
            placeholder: placeholder.to_owned().into(),
            mask: input_type == InputType::Password,
            input_type,
            had_focus: false,
        }
    }

    pub fn value(&self) -> String {
        self.core.value()
    }

    /// 程序赋值（不触发 Change 事件）。
    pub fn set_value(&mut self, v: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.core.set_value(v);
        cx.notify();
    }

    /// 程序更新提示字（如运行时切换界面语言）。
    pub fn set_placeholder(&mut self, p: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = p.into();
        cx.notify();
    }

    pub fn input_type(&self) -> InputType {
        self.input_type
    }

    // —— 动作处理器（编辑内核 + 通知/事件） ——

    fn backspace(&mut self, _: &Backspace, _window: &mut Window, cx: &mut Context<Self>) {
        self.core.backspace();
        cx.emit(TextFieldEvent::Change);
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, _window: &mut Window, cx: &mut Context<Self>) {
        self.core.delete_forward();
        cx.emit(TextFieldEvent::Change);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.core.move_left();
        cx.notify();
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.core.move_right();
        cx.notify();
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.core.select_left();
        cx.notify();
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.core.select_right();
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.core.select_all();
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.core.move_to(0);
        cx.notify();
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.core.move_to(self.core.content.len());
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // 单行压平换行。
            self.core.replace_in_range(None, &text.replace('\n', " "));
            cx.emit(TextFieldEvent::Change);
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.core.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.core.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.core.replace_in_range(None, "");
            cx.emit(TextFieldEvent::Change);
            cx.notify();
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.index_for_mouse_position(event.position);
        match event.click_count {
            2 => {
                let r = word_range_at(&self.core.content, idx);
                self.core.set_selection(r);
                cx.notify();
            }
            3 => {
                // 单行：三击全选
                self.core.set_selection(0..self.core.content.len());
                cx.notify();
            }
            _ => {
                if event.modifiers.shift {
                    self.core.select_from_anchor(idx);
                } else {
                    self.core.selection_anchor = idx;
                    self.core.move_to(idx);
                }
                cx.notify();
            }
        }
    }

    /// 拖拽选择：左键按住移动时从锚点扩展选区；未按键的移动不触发。
    fn on_select_drag(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button == Some(MouseButton::Left) {
            self.core
                .select_from_anchor(self.index_for_mouse_position(event.position));
            cx.notify();
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.core.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.core.last_bounds.as_ref(), self.core.last_layout.as_ref())
        else {
            return 0;
        };
        line.closest_index_for_x(position.x - bounds.left() + self.core.scroll_x)
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.core.range_from_utf16(&range_utf16);
        actual_range.replace(self.core.range_to_utf16(&range));
        Some(self.core.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.core.range_to_utf16(&self.core.selected_range),
            reversed: self.core.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.core.marked_range.as_ref().map(|r| self.core.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.core.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.core.replace_in_range(range_utf16, new_text) {
            cx.emit(TextFieldEvent::Change);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.core
            .replace_and_mark(range_utf16, new_text, new_selected_range_utf16);
        cx.emit(TextFieldEvent::Change);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: gpui::Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        let last_layout = self.core.last_layout.as_ref()?;
        let range = self.core.range_from_utf16(&range_utf16);
        Some(gpui::Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start) - self.core.scroll_x,
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end) - self.core.scroll_x,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.core.last_bounds?;
        let last_layout = self.core.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(position.x - bounds.origin.x + self.core.scroll_x)?;
        Some(self.core.offset_to_utf16(utf8_index))
    }
}

// ---------- 渲染元素 ----------

struct TextFieldElement {
    input: Entity<TextField>,
}

struct PrepaintState {
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextFieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextFieldElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let selected_range = input.core.selected_range.clone();
        let cursor = input.core.cursor_offset();
        let marked_range = input.core.marked_range.clone();
        let style = window.text_style();

        // 掩码显示：光标/选区索引按真实内容长度换算，宽度以掩码字形近似。
        let content: SharedString = if input.mask && !input.core.content.is_empty() {
            "•".repeat(input.core.content.chars().count()).into()
        } else {
            input.core.content.clone()
        };
        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 1., 0.25))
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = marked_range.as_ref() {
            vec![
                TextRun { len: marked.start, ..run.clone() },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun { len: display_text.len() - marked.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        // —— 横向滚动跟随：光标移出可视区时滚动，钳在内容宽度内 ——
        let vw = bounds.size.width;
        let cursor_pos = line.x_for_index(cursor);
        let max_scroll = (line.width - vw + px(16.)).max(px(0.));
        let old_scroll = input.core.scroll_x;
        let mut scroll = old_scroll;
        if cursor_pos - scroll > vw - px(8.) {
            scroll = cursor_pos - vw + px(8.);
        }
        if cursor_pos < scroll {
            scroll = (cursor_pos - px(8.)).max(px(0.));
        }
        scroll = scroll.min(max_scroll).max(px(0.));
        let scroll_changed = scroll != old_scroll;
        if scroll_changed {
            self.input.update(cx, |input, cx| {
                input.core.scroll_x = scroll;
                cx.notify();
            });
        }
        let x0 = bounds.left() - scroll;

        let cursor_quad = if selected_range.is_empty() {
            Some(fill(
                gpui::Bounds::new(
                    point(x0 + cursor_pos, bounds.top()),
                    size(px(2.), bounds.bottom() - bounds.top()),
                ),
                gpui::blue(),
            ))
        } else {
            None
        };
        let selection = if selected_range.is_empty() {
            None
        } else {
            Some(fill(
                gpui::Bounds::from_corners(
                    point(x0 + line.x_for_index(selected_range.start), bounds.top()),
                    point(x0 + line.x_for_index(selected_range.end), bounds.bottom()),
                ),
                rgba(0x7aa2f780),
            ))
        };
        PrepaintState { line, cursor: cursor_quad, selection }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.clone();
        let scroll = self.input.read(cx).core.scroll_x;
        line.paint(
            point(bounds.origin.x - scroll, bounds.origin.y),
            window.line_height(),
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.core.last_layout = Some(line);
            input.core.last_layouts = vec![prepaint.line.clone()];
            input.core.last_line_height = window.line_height();
            input.core.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 焦点转移检测：发 Focus/Blur 事件（gpui 0.2.2 无 observe_focus，render 期收口）。
        let focused = self.focus_handle.is_focused(window);
        if focused != self.had_focus {
            self.had_focus = focused;
            cx.emit(if focused { TextFieldEvent::Focus } else { TextFieldEvent::Blur });
        }
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .key_context("TextField")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            // 回车：发 PressEnter 事件；不 stop_propagation，宿主的全局
            // SubmitInput 处理（如回车=连接）继续生效。
            .on_action(cx.listener(|_this, _: &SubmitInput, _w, cx: &mut Context<Self>| {
                cx.emit(TextFieldEvent::PressEnter);
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_select_drag))
            .overflow_hidden()
            .child(TextFieldElement { input: cx.entity() })
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------- InputField：标准化样式包装器 ----------

/// 输入框尺寸模板。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InputSize {
    /// 26px 高 / 12.5px 字号（紧凑位、内联重命名）。
    Sm,
    /// 30px 高 / 13px 字号（表单标准行，默认）。
    #[default]
    Md,
}

impl InputSize {
    fn height(self) -> f32 {
        match self {
            InputSize::Sm => 26.,
            InputSize::Md => 30.,
        }
    }

    fn font_size(self) -> f32 {
        match self {
            InputSize::Sm => 12.5,
            InputSize::Md => 13.,
        }
    }
}

/// TextField 的样式包装：边框 / 聚焦高亮 / 首尾图标 / 尺寸 / 等宽字体。
/// 逻辑与状态在 `Entity<TextField>` 上，本组件仅负责外观（RenderOnce 无状态）。
///
/// ```ignore
/// InputField::new(host_field)
///     .size(InputSize::Md)
///     .leading_svg("icons/server.svg")
/// ```
#[derive(IntoElement)]
pub struct InputField {
    input: Entity<TextField>,
    size: InputSize,
    bordered: bool,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    mono: bool,
    theme: crate::Theme,
}

impl InputField {
    pub fn new(input: Entity<TextField>) -> Self {
        Self {
            input,
            size: InputSize::Md,
            bordered: true,
            leading: None,
            trailing: None,
            mono: false,
            theme: crate::Theme::DARK,
        }
    }

    /// 尺寸模板。
    pub fn size(mut self, size: InputSize) -> Self {
        self.size = size;
        self
    }

    /// 边框开关（默认开；聚焦时边框自动高亮为强调色）。
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// 首部图标/元素。
    pub fn leading(mut self, el: impl Into<AnyElement>) -> Self {
        self.leading = Some(el.into());
        self
    }

    /// 尾部图标/元素。
    pub fn trailing(mut self, el: impl Into<AnyElement>) -> Self {
        self.trailing = Some(el.into());
        self
    }

    /// 首部 svg 图标（按 svg mask 渲染，弱化色着色）。
    pub fn leading_svg(self, path: impl Into<gpui::SharedString>) -> Self {
        let path = path.into();
        self.leading(
            gpui::svg()
                .path(path)
                .size(px(14.))
                .flex_none()
                .text_color(hsla(0., 0., 1., 0.45))
                .into_any_element(),
        )
    }

    /// 尾部 svg 图标。
    pub fn trailing_svg(self, path: impl Into<gpui::SharedString>) -> Self {
        let path = path.into();
        self.trailing(
            gpui::svg()
                .path(path)
                .size(px(14.))
                .flex_none()
                .text_color(hsla(0., 0., 1., 0.45))
                .into_any_element(),
        )
    }

    /// 等宽字体。
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// 注入配色。
    pub fn theme(mut self, theme: crate::Theme) -> Self {
        self.theme = theme;
        self
    }
}

impl RenderOnce for InputField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focused = self.input.read(cx).focus_handle.is_focused(window);
        let size = self.size;
        let theme = self.theme;

        let mut el = div()
            .id("input-field")
            .debug_selector(|| "input-field-box".to_string())
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .h(px(size.height()))
            .px_3()
            .rounded_md()
            .overflow_hidden()
            .text_size(px(size.font_size()))
            .children(self.leading)
            .child(self.input)
            .children(self.trailing);
        if self.mono {
            el = el.font_family("Menlo");
        }
        if self.bordered {
            el = el
                .border_1()
                .border_color(rgb(if focused { theme.accent } else { theme.border }));
        }
        el
    }
}
