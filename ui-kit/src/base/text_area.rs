//! 多行文本域（基础组件）：状态实体 `TextArea` + 样式包装 `TextAreaField`。
//! 适用于私钥、命令脚本等内容型多行编辑。
//!
//! ## 通用属性（TextAreaField 包装器，builder 链式）
//! - 边框：`.bordered(bool)`（默认有；聚焦时自动高亮为强调色）
//! - 提示字：构造参数 placeholder（支持多行占位文本）
//! - 可视行数：构造参数 rows（高度 = 行高 × rows）
//! - 字体：默认等宽（内容型编辑惯例），`.mono(false)` 可关
//!
//! ## 组件事件（`cx.subscribe(&area, …)` 订阅）
//! - `TextAreaEvent::Change` / `Focus` / `Blur`
//!
//! ## 与 TextField 的语义差异
//! - **回车 = 插入换行**（不派发 SubmitInput，宿主表单的回车提交不受影响）；
//! - Home/End 按行首/行尾；三击选当前行（双击仍选词）；
//! - 粘贴保留换行；逐视觉行渲染/选区高亮、多行 IME 下划线；
//! - **自动换行（默认开）**：超宽行在词边界折行显示（超宽单词按字素硬折）；
//!   `set_wrap(false)` 关闭后长行保持一行、出现 X 轴滚动条可横向滚动；
//! - **自带滚动**：内容超出可视行数时滚轮/滚动条可用——内部复用
//!   [`crate::pro::scroll_area::ScrollArea`] 虚拟模式（滚轮→on_scroll_delta、
//!   滚动条拖拽→on_goto、光标行自动跟随进视口）。
//!
//! key_context 为 "TextArea"（编辑键位仅在本组件内生效）。

use std::ops::Range;

use gpui::{
    actions, App, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point,
    RenderOnce, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
    div, fill, hsla, point, prelude::*, px, rgb, relative, rgba, size,
};

use super::field_core::{
    line_of_offset, line_range_at, line_starts, word_range_at, FieldCore, NthOrLast,
};
use crate::pro::scroll_area::{ScrollArea, ScrollAreaState};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    text_area,
    [
        Backspace, Delete, Left, Right, SelectLeft, SelectRight, SelectAll, Home, End,
        ShowCharacterPalette, Paste, Cut, Copy, InsertNewline,
    ]
);

/// 注册 TextArea 所需的键位绑定（仅 "TextArea" 上下文内生效）。
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextArea")),
        KeyBinding::new("delete", Delete, Some("TextArea")),
        KeyBinding::new("left", Left, Some("TextArea")),
        KeyBinding::new("right", Right, Some("TextArea")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextArea")),
        KeyBinding::new("shift-right", SelectRight, Some("TextArea")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextArea")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextArea")),
        KeyBinding::new("cmd-v", Paste, Some("TextArea")),
        KeyBinding::new("cmd-c", Copy, Some("TextArea")),
        KeyBinding::new("cmd-x", Cut, Some("TextArea")),
        KeyBinding::new("home", Home, Some("TextArea")),
        KeyBinding::new("end", End, Some("TextArea")),
        // 回车 = 换行（区别于 TextField 的 SubmitInput）。
        KeyBinding::new("enter", InsertNewline, Some("TextArea")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextArea")),
    ]);
}

/// 组件事件（cx.subscribe 订阅）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAreaEvent {
    /// 内容变化（用户输入路径；set_value 程序赋值不触发）。
    Change,
    /// 获得焦点。
    Focus,
    /// 失去焦点。
    Blur,
}

pub struct TextArea {
    pub focus_handle: FocusHandle,
    core: FieldCore,
    placeholder: SharedString,
    /// 可视行数（视口高度）。
    rows: usize,
    /// 纵向滚动偏移（px，0=内容顶，正值=滚入内容）。
    scroll_y: Pixels,
    /// 最宽行像素宽（prepaint 测量，供横向滚动条参数；滞后一帧可接受）。
    content_w: Pixels,
    /// 视口像素宽（同上）。
    viewport_w: Pixels,
    /// 自动折行（默认开）：超出视口宽在词边界折行；关闭则长行横向滚动。
    wrap: bool,
    /// 各视觉行在 content 中的起始偏移（prepaint 写回，鼠标定位/滚动参数用）。
    last_row_starts: Vec<usize>,
    /// 上次跟随滚动时的光标偏移：光标不动不跟随，避免与用户拖拽滚动打架。
    last_cursor: usize,
    /// 内嵌滚动条状态（ScrollArea 虚拟模式）。
    scroll: Entity<ScrollAreaState>,
    /// 焦点状态缓存（render 时对比并发 Focus/Blur 事件）。
    had_focus: bool,
}

impl gpui::EventEmitter<TextAreaEvent> for TextArea {}

impl TextArea {
    /// 多行文本域；rows 为可视行数（≥2）。
    pub fn new(placeholder: &str, rows: usize, cx: &mut Context<Self>) -> Self {
        let scroll = cx.new(|_| ScrollAreaState::new(crate::pro::scroll_area::next_owner()));
        // 滚动条状态变化（hover/拖拽锚点）联动自身重绘，宿主无需关心。
        cx.observe(&scroll, |_, _, cx| cx.notify()).detach();
        Self {
            focus_handle: cx.focus_handle(),
            core: FieldCore::new(false),
            placeholder: placeholder.to_owned().into(),
            rows: rows.max(2),
            scroll_y: px(0.),
            content_w: px(0.),
            viewport_w: px(0.),
            wrap: true,
            last_row_starts: Vec::new(),
            last_cursor: 0,
            scroll,
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

    pub fn rows(&self) -> usize {
        self.rows.max(2)
    }

    /// 内嵌滚动状态（诊断/高级用途）。
    pub fn scroll_state(&self) -> &Entity<ScrollAreaState> {
        &self.scroll
    }

    /// 当前横向滚动偏移（px，0=最左）。
    pub fn scroll_x(&self) -> f32 {
        self.core.scroll_x.into()
    }

    /// 当前纵向滚动偏移（px，0=最顶）。
    pub fn scroll_y(&self) -> f32 {
        self.scroll_y.into()
    }

    /// 折行模式（默认开）：超出视口宽自动折行显示；关闭则长行保持一行、
    /// 出现 X 轴滚动条可横向滚动。
    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.wrap = wrap;
        if wrap {
            self.core.scroll_x = px(0.);
        }
        cx.notify();
    }

    // —— 动作处理器 ——

    fn backspace(&mut self, _: &Backspace, _window: &mut Window, cx: &mut Context<Self>) {
        self.core.backspace();
        cx.emit(TextAreaEvent::Change);
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, _window: &mut Window, cx: &mut Context<Self>) {
        self.core.delete_forward();
        cx.emit(TextAreaEvent::Change);
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

    /// Home：回当前行行首。
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let (row, _) = line_of_offset(&self.core.content, self.core.cursor_offset());
            let mut starts = line_starts(&self.core.content);
            starts.nth_or_last(row, self.core.content.len())
        };
        self.core.move_to(target);
        cx.notify();
    }

    /// End：到当前行行尾（不含换行符）。
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let (row, _) = line_of_offset(&self.core.content, self.core.cursor_offset());
            let mut starts = line_starts(&self.core.content);
            let start = starts.nth_or_last(row, self.core.content.len());
            self.core.content[start..]
                .find('\n')
                .map(|i| start + i)
                .unwrap_or(self.core.content.len())
        };
        self.core.move_to(target);
        cx.notify();
    }

    /// 回车 = 换行。
    fn insert_newline(&mut self, _: &InsertNewline, _window: &mut Window, cx: &mut Context<Self>) {
        self.core.replace_in_range(None, "\n");
        cx.emit(TextAreaEvent::Change);
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // 多行保留换行。
            self.core.replace_in_range(None, &text);
            cx.emit(TextAreaEvent::Change);
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
            cx.emit(TextAreaEvent::Change);
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
                // 三击选当前行
                let r = line_range_at(&self.core.content, idx);
                self.core.set_selection(r);
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
        let (Some(bounds), Some(_)) = (self.core.last_bounds.as_ref(), self.core.last_layouts.first())
        else {
            return 0;
        };
        // 滚动条热区让位：右缘 14px（Y 轴）、底缘 14px（X 轴），
        // 避免点滚动条时跳光标。
        if position.x > bounds.right() - px(14.) || position.y > bounds.bottom() - px(14.) {
            return self.core.cursor_offset();
        }
        // 越界 y 钳到框内（拖拽经过上下缘时不至于跳到行首）；
        // 视口 y + 滚动偏移 = 内容 y
        let y = position.y.clamp(bounds.top(), bounds.bottom() - px(14.)) + self.scroll_y;
        if self.core.last_layouts.is_empty() {
            return 0;
        }
        let lh = self.core.last_line_height;
        let row = (((y - bounds.top()) / lh).floor() as usize)
            .min(self.core.last_layouts.len() - 1);
        let row_start = self.last_row_starts.get(row).copied().unwrap_or(0);
        let in_row = self.core.last_layouts[row]
            .closest_index_for_x(position.x - bounds.left() + self.core.scroll_x);
        row_start + in_row
    }
}

impl EntityInputHandler for TextArea {
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
            cx.emit(TextAreaEvent::Change);
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
        cx.emit(TextAreaEvent::Change);
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

struct TextAreaElement {
    input: Entity<TextArea>,
}

/// 一个视觉行：折行模式下逻辑行会拆成多个视觉行。
struct VisualRow {
    /// 行文本在 content 中的字节区间 [start, end)。
    start: usize,
    end: usize,
    line: ShapedLine,
}

struct PrepaintState {
    rows: Vec<VisualRow>,
    cursor: Option<PaintQuad>,
    /// 选区高亮（逐视觉行一个 quad）。
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextAreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextAreaElement {
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
        let rows = self.input.read(cx).rows();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * rows as f32).into();
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
        let lh = window.line_height();

        let (display_text, text_color) = if input.core.content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 1., 0.25))
        } else {
            (input.core.content.clone(), style.color)
        };

        let run = TextRun {
            len: 0,
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = style.font_size.to_pixels(window.rem_size());

        // —— 视觉行构造 ——
        // wrap（默认）：逻辑行按视口宽在词边界折行（超宽单词按字素硬折）；
        // no-wrap：逻辑行即视觉行（长行横向滚动）。
        let vw = bounds.size.width;
        let budget = vw - px(2.);
        let starts = line_starts(&input.core.content).collect::<Vec<usize>>();

        // 段排版：与 IME marked_range 相交的段拆三段 run（中段下划线）。
        let shape_seg = |text: &str, seg_start: usize| -> ShapedLine {
            let runs: Vec<TextRun> = match &marked_range {
                Some(m) if m.start < seg_start + text.len() && m.end > seg_start => {
                    let s = m.start.max(seg_start) - seg_start;
                    let e = m.end.min(seg_start + text.len()) - seg_start;
                    vec![
                        TextRun { len: s, ..run.clone() },
                        TextRun {
                            len: e - s,
                            underline: Some(UnderlineStyle {
                                color: Some(run.color),
                                thickness: px(1.0),
                                wavy: false,
                            }),
                            ..run.clone()
                        },
                        TextRun { len: text.len() - e, ..run.clone() },
                    ]
                    .into_iter()
                    .filter(|run| run.len > 0)
                    .collect()
                }
                _ => vec![TextRun { len: text.len(), ..run.clone() }],
            };
            window
                .text_system()
                .shape_line(text.to_string().into(), font_size, &runs, None)
        };

        let mut rows: Vec<VisualRow> = Vec::new();
        for (li, lt) in display_text.split('\n').enumerate() {
            let ls = starts.get(li).copied().unwrap_or(input.core.content.len());
            let full = shape_seg(lt, ls);
            if !input.wrap || full.width <= budget {
                rows.push(VisualRow { start: ls, end: ls + lt.len(), line: full });
                continue;
            }
            // 折行：贪心取最后一个放得下的词边界；单词超宽按字素硬折。
            let mut seg = 0usize;
            while seg < lt.len() {
                let seg_x = full.x_for_index(seg);
                let mut best: Option<usize> = None;
                for (wi, word) in lt[seg..].split_word_bound_indices() {
                    let after = seg + wi + word.len();
                    if full.x_for_index(after) - seg_x <= budget {
                        best = Some(after);
                    } else {
                        break;
                    }
                }
                let cut = match best {
                    Some(b) if b > seg => b,
                    _ => {
                        let mut hard = seg;
                        for (gi, g) in lt[seg..].grapheme_indices(true) {
                            let after = seg + gi + g.len();
                            if full.x_for_index(after) - seg_x <= budget {
                                hard = after;
                            } else {
                                break;
                            }
                        }
                        if hard > seg { hard } else { lt.len() }
                    }
                };
                rows.push(VisualRow {
                    start: ls + seg,
                    end: ls + cut,
                    line: shape_seg(&lt[seg..cut], ls + seg),
                });
                seg = cut;
            }
        }

        // —— 光标 → 视觉行（行尾光标归属本行） ——
        let (crow, cursor_in_row) = {
            let mut r = rows.len().saturating_sub(1);
            let mut xin = 0usize;
            for (i, row) in rows.iter().enumerate() {
                if cursor >= row.start && cursor <= row.end {
                    r = i;
                    xin = cursor - row.start;
                    break;
                }
            }
            (r, xin)
        };
        let cursor_pos = rows[crow].line.x_for_index(cursor_in_row);

        // —— 光标跟随（仅当光标偏移变化时；用户拖拽滚动条不与之打架） ——
        let cursor_moved = cursor != input.last_cursor;
        let max_line_w = rows.iter().map(|r| r.line.width).max().unwrap_or_default();
        let content_w_changed = input.content_w != max_line_w || input.viewport_w != vw;
        let old_scroll = input.core.scroll_x;
        let input_sy = input.scroll_y;
        let mut scroll = old_scroll;
        let mut sy = input_sy;
        if cursor_moved {
            // 横向（wrap 模式恒为 0）
            let max_scroll = if input.wrap {
                px(0.)
            } else {
                (max_line_w - vw + px(16.)).max(px(0.))
            };
            if cursor_pos - scroll > vw - px(8.) {
                scroll = cursor_pos - vw + px(8.);
            }
            if cursor_pos < scroll {
                scroll = (cursor_pos - px(8.)).max(px(0.));
            }
            scroll = scroll.min(max_scroll).max(px(0.));

            // 纵向：光标行跟随进视口（视觉行数计高度）
            let vp = bounds.size.height;
            let max_y = (rows.len() as f32 * lh - vp).max(px(0.));
            let cursor_top = lh * crow as f32;
            if cursor_top < sy {
                sy = cursor_top;
            }
            if cursor_top + lh > sy + vp {
                sy = (cursor_top + lh - vp).max(px(0.));
            }
            sy = sy.min(max_y).max(px(0.));
        }
        let scroll_changed = scroll != old_scroll;
        let sy_changed = sy != input_sy;

        if scroll_changed || sy_changed || content_w_changed || cursor_moved {
            self.input.update(cx, |input, cx| {
                input.core.scroll_x = scroll;
                input.scroll_y = sy;
                input.content_w = max_line_w;
                input.viewport_w = vw;
                input.last_cursor = cursor;
                cx.notify();
            });
        }
        let x0 = bounds.left() - scroll;
        let y0 = bounds.top() - sy;

        // —— 光标 quad ——
        let cursor_quad = if selected_range.is_empty() {
            Some(fill(
                gpui::Bounds::new(
                    point(x0 + cursor_pos, y0 + lh * crow as f32),
                    size(px(2.), lh),
                ),
                gpui::blue(),
            ))
        } else {
            None
        };

        // —— 逐视觉行选区 ——
        let mut selections = Vec::new();
        if !selected_range.is_empty() {
            let (sel_start, sel_end) = (
                selected_range.start.min(selected_range.end),
                selected_range.start.max(selected_range.end),
            );
            for (ri, row) in rows.iter().enumerate() {
                let s = sel_start.max(row.start);
                let e = sel_end.min(row.end);
                if s < e {
                    let xs = row.line.x_for_index(s - row.start);
                    let xe = row.line.x_for_index(e - row.start);
                    let ry = y0 + lh * ri as f32;
                    selections.push(fill(
                        gpui::Bounds::from_corners(
                            point(x0 + xs, ry),
                            point(x0 + xe, ry + lh),
                        ),
                        rgba(0x7aa2f780),
                    ));
                }
                if row.start > sel_end {
                    break;
                }
            }
        }
        PrepaintState { rows, cursor: cursor_quad, selections }
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
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection)
        }
        let rows = std::mem::take(&mut prepaint.rows);
        let lh = window.line_height();
        let scroll = self.input.read(cx).core.scroll_x;
        let sy = self.input.read(cx).scroll_y;
        let y0 = bounds.origin.y - sy;
        for (i, row) in rows.iter().enumerate() {
            // 仅绘制与视口相交的视觉行（上下双向裁剪；留 0.5px 浮点余量）。
            let top_i = y0 + lh * i as f32;
            if top_i + lh < bounds.top() - px(0.5) {
                continue;
            }
            if top_i > bounds.bottom() + px(0.5) {
                break;
            }
            row.line
                .paint(point(bounds.origin.x - scroll, top_i), lh, window, cx)
                .unwrap();
        }

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        let row_starts: Vec<usize> = rows.iter().map(|r| r.start).collect();
        self.input.update(cx, |input, _cx| {
            input.core.last_layout = rows.first().map(|r| r.line.clone());
            input.core.last_layouts = rows.iter().map(|r| r.line.clone()).collect();
            input.core.last_line_height = lh;
            input.core.last_bounds = Some(bounds);
            input.last_row_starts = row_starts;
        });
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 焦点转移检测：发 Focus/Blur 事件。
        let focused = self.focus_handle.is_focused(window);
        if focused != self.had_focus {
            self.had_focus = focused;
            cx.emit(if focused { TextAreaEvent::Focus } else { TextAreaEvent::Blur });
        }
        // 纵向滚动参数：视口 = rows×行高；max = 内容高 − 视口。
        let lh: f32 = window.line_height().into();
        let vp = lh * self.rows() as f32;
        // 视觉行数（折行后），prepaint 测量、滞后一帧可接受。
        let row_count = self.last_row_starts.len().max(1);
        let max_y = (lh * row_count as f32 - vp).max(0.);
        let entity = cx.entity();
        let scroll_state = self.scroll.clone();

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .key_context("TextArea")
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
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_select_drag))
            // 自带滚动：ScrollArea 虚拟模式（滚轮/滚动条交互），
            // 内容偏移由本实体 scroll_y 自管（渲染期换算）。
            .child(
                ScrollArea::virtual_y(
                    scroll_state,
                    TextAreaElement { input: entity.clone() },
                    self.scroll_y.into(),
                    max_y,
                    vp,
                    vp,
                    // 滚动条拖拽：top 域偏移（0=内容顶）。
                    {
                        let entity = entity.clone();
                        move |top, _w, cx| {
                            entity.update(cx, |a, cx| {
                                a.scroll_y = px(top.clamp(0., max_y));
                                cx.notify();
                            });
                        }
                    },
                )
                .on_scroll_delta({
                    let entity = entity.clone();
                    move |dy, _w, cx| {
                        // gpui 约定 dy 正值=向内容顶部；scroll_y 正值=滚入内容 → 反号。
                        entity.update(cx, |a, cx| {
                            let cur: f32 = a.scroll_y.into();
                            a.scroll_y = px((cur - dy).clamp(0., max_y));
                            cx.notify();
                        });
                    }
                })
                // 横向滚动条：内容最宽行超出视口时出现（值滞后一帧，prepaint 测量）。
                .virtual_x(
                    self.core.scroll_x.into(),
                    {
                        let w: f32 = self.content_w.into();
                        let vp: f32 = self.viewport_w.into();
                        (w - vp).max(0.)
                    },
                    self.viewport_w.into(),
                    {
                        let entity = entity.clone();
                        move |left, _w, cx| {
                            entity.update(cx, |a, cx| {
                                a.core.scroll_x = px(left);
                                cx.notify();
                            });
                        }
                    },
                ),
            )
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------- TextAreaField：标准化样式包装器 ----------

/// TextArea 的样式包装：边框 / 聚焦高亮 / 等宽字体（默认开）。
/// 高度由实体构造时的 rows 决定。
#[derive(IntoElement)]
pub struct TextAreaField {
    input: Entity<TextArea>,
    bordered: bool,
    mono: bool,
    theme: crate::Theme,
}

impl TextAreaField {
    pub fn new(input: Entity<TextArea>) -> Self {
        Self {
            input,
            bordered: true,
            mono: true,
            theme: crate::Theme::DARK,
        }
    }

    /// 边框开关（默认开；聚焦时边框自动高亮为强调色）。
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// 等宽字体（内容型多行编辑默认开）。
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

impl RenderOnce for TextAreaField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focused = self.input.read(cx).focus_handle.is_focused(window);
        let theme = self.theme;

        // 高度由实体内容（rows×行高，元素在 11px 语境内自测）驱动，
        // 不在本层另行计算——外层字号不同会导致行高口径不一（曾出 26px 底部空隙）。
        let mut el = div()
            .id("text-area-field")
            .debug_selector(|| "text-area-field".to_string())
            .flex()
            .w_full()
            .px_2()
            .py(px(2.))
            .rounded_md()
            .text_size(px(11.))
            .child(self.input);
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
