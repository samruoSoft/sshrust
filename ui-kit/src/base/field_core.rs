//! 输入组件共享内核：内容/选区/IME 标记/边界/UTF-16 换算等纯编辑状态。
//! `TextField`（单行）与 `TextArea`（多行）共用；本模块对库外不可见。

use std::ops::Range;

use gpui::{px, Bounds, Pixels, ShapedLine, SharedString};
use unicode_segmentation::UnicodeSegmentation;

/// 数字过滤（InputType::Number 用）：仅保留 ASCII 数字。
pub(crate) fn numeric_only(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// 字节偏移 → (所在行号, 行内字节偏移)。偏移可以等于 content.len()。
pub(crate) fn line_of_offset(content: &str, offset: usize) -> (usize, usize) {
    let mut line = 0;
    let mut line_start = 0;
    for (i, b) in content.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    (line, offset - line_start)
}

/// 各行起始字节偏移的迭代器（第 0 行恒为 0）。
pub(crate) fn line_starts(content: &str) -> impl Iterator<Item = usize> + Clone + '_ {
    std::iter::once(0).chain(content.match_indices('\n').map(|(i, _)| i + 1))
}

/// 扩展方法：取第 row 行起始偏移；row 越界回落到 fallback。
pub(crate) trait NthOrLast {
    fn nth_or_last(&mut self, row: usize, fallback: usize) -> usize;
}

impl<I: Iterator<Item = usize>> NthOrLast for I {
    fn nth_or_last(&mut self, row: usize, fallback: usize) -> usize {
        let mut last = fallback;
        for (i, v) in self.enumerate() {
            if i == row {
                return v;
            }
            last = v;
        }
        last
    }
}

/// 第 row 行的字节长度（不含换行符）。
#[allow(dead_code)] // 目前仅测试使用；保留供后续行级操作
pub(crate) fn line_len_at(content: &str, row: usize) -> usize {
    content.split('\n').nth(row).map(|l| l.len()).unwrap_or(0)
}

/// offset 所在行（不含换行符）的字节区间（三击选行用）。
pub(crate) fn line_range_at(content: &str, offset: usize) -> Range<usize> {
    let (row, _) = line_of_offset(content, offset);
    let mut starts = line_starts(content);
    let start = starts.nth_or_last(row, content.len());
    let end = content[start..].find('\n').map(|i| start + i).unwrap_or(content.len());
    start..end
}

/// offset 所在词的字节区间（双击选词用，unicode 词边界）；
/// 落在边界/空白上时选该空白段，找不到返回空区间。
pub(crate) fn word_range_at(content: &str, offset: usize) -> Range<usize> {
    let segs: Vec<(usize, usize)> = content
        .split_word_bound_indices()
        .map(|(i, s)| (i, i + s.len()))
        .collect();
    segs.iter()
        .find(|(s, e)| *s <= offset && offset < *e)
        .or_else(|| segs.iter().find(|(s, _)| *s == offset))
        .map(|(s, e)| *s..*e)
        .unwrap_or(offset..offset)
}

/// 单个输入组件的编辑状态（无 gpui Context 依赖，纯数据操作）。
pub(crate) struct FieldCore {
    pub content: SharedString,
    pub selected_range: Range<usize>,
    pub selection_reversed: bool,
    /// 鼠标拖选锚点（按下处；拖动/shift+点击由此扩展）。
    pub selection_anchor: usize,
    /// IME 组装区（marked range，字节偏移）。
    pub marked_range: Option<Range<usize>>,
    /// 数字输入过滤开关（TextField::Number 用）。
    pub numeric: bool,
    /// 超宽内容横向滚动偏移（0=最左）。
    pub scroll_x: Pixels,
    // —— 渲染期缓存（鼠标定位/IME bounds 用） ——
    pub last_layout: Option<ShapedLine>,
    pub last_layouts: Vec<ShapedLine>,
    pub last_line_height: Pixels,
    pub last_bounds: Option<Bounds<Pixels>>,
}

impl FieldCore {
    pub fn new(numeric: bool) -> Self {
        Self {
            content: "".into(),
            selected_range: 0..0,
            selection_reversed: false,
            selection_anchor: 0,
            marked_range: None,
            numeric,
            scroll_x: px(0.),
            last_layout: None,
            last_layouts: Vec::new(),
            last_line_height: px(16.),
            last_bounds: None,
        }
    }

    pub fn value(&self) -> String {
        self.content.to_string()
    }

    /// 程序赋值：光标落尾、清 IME 标记与横向滚动；不产生事件。
    pub fn set_value(&mut self, v: impl Into<SharedString>) {
        self.content = v.into();
        self.selected_range = self.content.len()..self.content.len();
        self.marked_range = None;
        self.scroll_x = px(0.);
    }

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub fn move_to(&mut self, offset: usize) {
        self.selected_range = offset..offset;
    }

    pub fn select_to(&mut self, offset: usize) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
    }

    pub fn select_from_anchor(&mut self, offset: usize) {
        let anchor = self.selection_anchor;
        self.selection_reversed = offset < anchor;
        self.selected_range = anchor.min(offset)..anchor.max(offset);
    }

    /// 直接设定选区（双击/三击路径），锚点放选区起点。
    pub fn set_selection(&mut self, r: Range<usize>) {
        self.selection_anchor = r.start;
        self.selection_reversed = false;
        self.selected_range = r;
    }

    // —— 编辑原语 ——

    /// 提交路径的内容替换（UTF-16 可选区间；None 时 marked/选区回退）。
    /// Number 类型过滤非数字。返回是否真的改动了内容。
    pub fn replace_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str) -> bool {
        let filtered: String = if self.numeric {
            numeric_only(new_text)
        } else {
            new_text.to_string()
        };
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        if range.start > self.content.len() || range.end > self.content.len() {
            return false;
        }
        let changed = range.start != range.end || !filtered.is_empty();
        if !changed {
            return false;
        }
        self.content = (self.content[0..range.start].to_owned()
            + &filtered
            + &self.content[range.end..])
            .into();
        self.selected_range =
            range.start + filtered.len()..range.start + filtered.len();
        self.marked_range = None;
        true
    }

    /// IME 组装路径：替换并标记（组件发 Change 事件）。
    pub fn replace_and_mark(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_utf16: Option<Range<usize>>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        if range.start > self.content.len() || range.end > self.content.len() {
            return;
        }
        self.content = (self.content[0..range.start].to_owned()
            + new_text
            + &self.content[range.end..])
            .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| r.start + range.start..r.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
    }

    /// 删除键语义：无选区时吃掉光标前一个字素。
    pub fn backspace(&mut self) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()));
        }
        self.replace_in_range(None, "");
    }

    /// 前进删除：无选区时吃掉光标后一个字素。
    pub fn delete_forward(&mut self) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.selected_range.end));
        }
        self.replace_in_range(None, "");
    }

    pub fn move_left(&mut self) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()));
        } else {
            self.move_to(self.selected_range.start);
        }
    }

    pub fn move_right(&mut self) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end));
        } else {
            self.move_to(self.selected_range.end);
        }
    }

    pub fn select_left(&mut self) {
        self.select_to(self.previous_boundary(self.cursor_offset()));
    }

    pub fn select_right(&mut self) {
        self.select_to(self.next_boundary(self.cursor_offset()));
    }

    pub fn select_all(&mut self) {
        self.move_to(0);
        self.select_to(self.content.len());
    }

    /// 选中文本（copy/cut 用）。
    pub fn selected_text(&self) -> Option<String> {
        if self.selected_range.is_empty() {
            None
        } else {
            Some(self.content[self.selected_range.clone()].to_string())
        }
    }

    // —— 字素边界（UTF-8 字节偏移） ——

    pub fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    // —— UTF-16 换算（IME/系统剪贴板坐标） ——

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_only_filters_digits() {
        assert_eq!(numeric_only("12a3-4x"), "1234");
        assert_eq!(numeric_only("端口"), "");
        assert_eq!(numeric_only(""), "");
    }

    #[test]
    fn line_helpers() {
        let c = "ab\ncd\nef";
        assert_eq!(line_of_offset(c, 0), (0, 0));
        assert_eq!(line_of_offset(c, 3), (1, 0));
        assert_eq!(line_of_offset(c, 8), (2, 2));
        assert_eq!(line_starts(c).collect::<Vec<_>>(), vec![0, 3, 6]);
        assert_eq!(line_len_at(c, 1), 2);
        assert_eq!(line_range_at(c, 4), 3..5);
    }

    #[test]
    fn word_range_selects_word_and_boundaries() {
        let c = "hello world";
        assert_eq!(word_range_at(c, 1), 0..5);
        assert_eq!(word_range_at(c, 5), 5..6);
        assert_eq!(word_range_at(c, 7), 6..11);
        assert_eq!(word_range_at(c, 11), 11..11);
        let zh = "服务器列表";
        let r = word_range_at(zh, 2);
        assert!(r.start <= 2 && 2 < r.end, "zh word range covers offset: {r:?}");
    }

    #[test]
    fn core_edit_and_select() {
        let mut core = FieldCore::new(false);
        core.replace_in_range(None, "héllo");
        assert_eq!(core.value(), "héllo");
        core.select_all();
        assert_eq!(core.selected_text().as_deref(), Some("héllo"));
        core.replace_in_range(None, "hi");
        assert_eq!(core.value(), "hi");
        // 数字过滤
        let mut num = FieldCore::new(true);
        num.replace_in_range(None, "8a0b8");
        assert_eq!(num.value(), "808");
        // 拖选
        num.selection_anchor = 0;
        num.select_from_anchor(3);
        assert_eq!(num.selected_range, 0..3);
    }
}
