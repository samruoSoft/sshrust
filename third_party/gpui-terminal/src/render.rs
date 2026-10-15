//! Terminal rendering module.
//!
//! This module provides [`TerminalRenderer`], which handles efficient rendering of
//! terminal content using GPUI's text and drawing systems.
//!
//! # Rendering Pipeline
//!
//! The renderer processes the terminal grid in several stages:
//!
//! ```text
//! Terminal Grid → Layout Phase → Paint Phase
//!                      │              │
//!                      ├─ Collect backgrounds
//!                      ├─ Batch text runs
//!                      │              │
//!                      │              ├─ Paint default background
//!                      │              ├─ Paint non-default backgrounds
//!                      │              ├─ Paint text characters
//!                      │              └─ Paint cursor
//! ```
//!
//! # Optimizations
//!
//! The renderer includes several optimizations to minimize draw calls:
//!
//! 1. **Background Merging**: Adjacent cells with the same background color are
//!    merged into single rectangles, reducing the number of quads to paint.
//!
//! 2. **Text Batching**: Adjacent cells with identical styling (color, bold, italic)
//!    are grouped into [`BatchedTextRun`]s for efficient text shaping.
//!
//! 3. **Default Background Skip**: Cells with the default background color don't
//!    generate separate background rectangles.
//!
//! 4. **Cell Measurement**: Font metrics are measured once using the 'M' character
//!    and cached for consistent cell dimensions.
//!
//! # Cell Dimensions
//!
//! Cell size is calculated from actual font metrics:
//!
//! - **Width**: Measured from shaped 'M' character (typically widest in monospace)
//! - **Height**: `(ascent + descent) × line_height_multiplier`
//!
//! The `line_height_multiplier` (default 1.2) adds extra vertical space to
//! accommodate tall glyphs from nerd fonts and other icon fonts.
//!
//! # Example
//!
//! ```ignore
//! use gpui::px;
//! use gpui_terminal::{ColorPalette, TerminalRenderer};
//!
//! let renderer = TerminalRenderer::new(
//!     "JetBrains Mono".to_string(),
//!     px(14.0),
//!     1.2,  // line height multiplier
//!     ColorPalette::default(),
//! );
//! ```

use crate::colors::ColorPalette;
use crate::event::GpuiEventProxy;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as AlacPoint};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui::{
    App, Bounds, Edges, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Point,
    SharedString, Size, TextRun, UnderlineStyle, Window, px, quad, transparent_black,
};

/// A batched run of text with consistent styling.
///
/// This struct groups adjacent terminal cells with identical visual attributes
/// to reduce the number of text rendering calls.
#[derive(Debug, Clone)]
pub struct BatchedTextRun {
    /// The text content to render
    pub text: String,

    /// Starting column position
    pub start_col: usize,

    /// Row position
    pub row: usize,

    /// Foreground color
    pub fg_color: Hsla,

    /// Background color
    pub bg_color: Hsla,

    /// Bold flag
    pub bold: bool,

    /// Italic flag
    pub italic: bool,

    /// Underline flag
    pub underline: bool,
}

/// Background rectangle to paint.
///
/// Represents a rectangular region with a solid color background.
#[derive(Debug, Clone)]
pub struct BackgroundRect {
    /// Starting column position
    pub start_col: usize,

    /// Ending column position (exclusive)
    pub end_col: usize,

    /// Row position
    pub row: usize,

    /// Background color
    pub color: Hsla,
}

impl BackgroundRect {
    /// Check if this rectangle can be merged with another.
    ///
    /// Two rectangles can be merged if they:
    /// - Are on the same row
    /// - Have the same color
    /// - Are horizontally adjacent
    fn can_merge_with(&self, other: &Self) -> bool {
        self.row == other.row && self.color == other.color && self.end_col == other.start_col
    }
}

/// 选区高亮色：半透明蓝，深/浅终端底色通用（叠在行背景之上、文字之下）。
const SELECTION_BG: Hsla = Hsla {
    h: 211.0 / 360.0,
    s: 0.6,
    l: 0.55,
    a: 0.38,
};

/// 文本选区端点。行用**绝对行号**（网格行 − `topmost_line()`）：屏幕滚动时
/// 内容的网格行会整体平移，绝对行号保持锚定同一行内容（历史环形淘汰时
/// 除外——此时选区轻微漂移，可接受）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelPoint {
    /// 绝对行号（从缓冲区顶部数起，不受回滚视口影响）。
    pub line: i32,
    /// 列号。
    pub col: usize,
}

/// 选区两端按 (行, 列) 归一化为升序。
pub fn normalize_sel(a: SelPoint, b: SelPoint) -> (SelPoint, SelPoint) {
    if (a.line, a.col) <= (b.line, b.col) {
        (a, b)
    } else {
        (b, a)
    }
}

/// Terminal renderer with font settings and cell dimensions.
///
/// This struct manages the rendering of terminal content, including text,
/// backgrounds, and cursor. It maintains font metrics and provides the
/// [`paint`](Self::paint) method for drawing the terminal grid.
///
/// # Font Metrics
///
/// Cell dimensions are calculated from actual font measurements via
/// [`measure_cell`](Self::measure_cell). This ensures accurate character
/// positioning regardless of the font used.
///
/// # Usage
///
/// The renderer is typically used internally by [`TerminalView`](crate::TerminalView),
/// but can also be used directly for custom rendering:
///
/// ```ignore
/// // Measure cell dimensions (call once per font change)
/// renderer.measure_cell(window);
///
/// // Paint the terminal grid
/// renderer.paint(bounds, padding, &term, window, cx);
/// ```
///
/// # Performance
///
/// For optimal performance:
/// - Call `measure_cell` only when font settings change
/// - The `paint` method is designed to be called every frame
/// - Background and text batching minimize GPU draw calls
#[derive(Clone)]
pub struct TerminalRenderer {
    /// Font family name (e.g. "Fira Code", "Menlo")
    pub font_family: String,

    /// Font size in pixels
    pub font_size: Pixels,

    /// Width of a single character cell
    pub cell_width: Pixels,

    /// Height of a single character cell (cell line height)
    pub cell_height: Pixels,

    /// Multiplier for line height to accommodate tall glyphs
    pub line_height_multiplier: f32,

    /// Color palette for resolving terminal colors
    pub palette: ColorPalette,

    /// 本地着色模板（客户端样式规则组开关）
    pub style: StyleTemplate,
}

impl TerminalRenderer {
    /// Creates a new terminal renderer with the given font settings and color palette.
    ///
    /// # Arguments
    ///
    /// * `font_family` - The name of the font family to use
    /// * `font_size` - The font size in pixels
    /// * `line_height_multiplier` - Multiplier for line height (e.g., 1.2 for 20% extra)
    /// * `palette` - The color palette to use for terminal colors
    ///
    /// # Returns
    ///
    /// A new `TerminalRenderer` instance with default cell dimensions.
    ///
    /// # Examples
    ///
    /// ```
    /// use gpui::px;
    /// use gpui_terminal::render::TerminalRenderer;
    /// use gpui_terminal::ColorPalette;
    ///
    /// let renderer = TerminalRenderer::new("Fira Code".to_string(), px(14.0), 1.2, ColorPalette::default());
    /// ```
    pub fn new(
        font_family: String,
        font_size: Pixels,
        line_height_multiplier: f32,
        palette: ColorPalette,
    ) -> Self {
        // Default cell dimensions - will be measured on first paint
        // Using 0.6 as approximate em-width ratio for monospace fonts
        let cell_width = font_size * 0.6;
        let cell_height = font_size * 1.4; // Line height with some spacing

        Self {
            font_family,
            font_size,
            cell_width,
            cell_height,
            line_height_multiplier,
            palette,
            style: StyleTemplate::default(),
        }
    }

    /// Measure cell dimensions based on actual font metrics.
    ///
    /// This method measures the actual width and height of characters
    /// using the GPUI text system.
    ///
    /// # Arguments
    ///
    /// * `window` - The GPUI window for text system access
    pub fn measure_cell(&mut self, window: &mut Window) {
        // Measure using a reference character (M is typically the widest)
        let font = Font {
            family: self.font_family.clone().into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };

        let text_run = TextRun {
            len: 1,
            font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        // Shape a single 'M' character to get its metrics
        let shaped = window
            .text_system()
            .shape_line("M".into(), self.font_size, &[text_run], None);

        // Get the width from the shaped line (accessed via Deref to LineLayout)
        if shaped.width > px(0.0) {
            self.cell_width = shaped.width;
        }

        // Calculate height from ascent + descent with multiplier for tall glyphs (nerd fonts, etc.)
        let line_height = shaped.ascent + shaped.descent;
        if line_height > px(0.0) {
            self.cell_height = line_height * self.line_height_multiplier;
        }
    }

    /// Layout cells into batched text runs and background rects for a single row.
    ///
    /// This method processes a row of terminal cells and groups adjacent cells
    /// with identical styling into batched runs. It also collects background
    /// rectangles that need to be painted.
    ///
    /// # Arguments
    ///
    /// * `row` - The row number
    /// * `cells` - Iterator over (column, Cell) pairs
    /// * `colors` - Terminal color configuration
    ///
    /// # Returns
    ///
    /// A tuple of `(backgrounds, text_runs)` where:
    /// - `backgrounds` is a vector of merged background rectangles
    /// - `text_runs` is a vector of batched text runs
    pub fn layout_row(
        &self,
        row: usize,
        cells: impl Iterator<Item = (usize, Cell)>,
        colors: &Colors,
    ) -> (Vec<BackgroundRect>, Vec<BatchedTextRun>) {
        let mut backgrounds = Vec::new();
        let mut text_runs = Vec::new();

        let mut current_run: Option<BatchedTextRun> = None;
        let mut current_bg: Option<BackgroundRect> = None;

        for (col, cell) in cells {
            // Skip wide character spacers
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            // Extract cell styling
            let fg_color = self.palette.resolve(cell.fg, colors);
            let bg_color = self.palette.resolve(cell.bg, colors);
            let bold = cell.flags.contains(Flags::BOLD);
            let italic = cell.flags.contains(Flags::ITALIC);
            let underline = cell.flags.contains(Flags::UNDERLINE);

            // Get the character (or space if empty)
            let ch = if cell.c == ' ' || cell.c == '\0' {
                ' '
            } else {
                cell.c
            };

            // Handle background rectangles
            if let Some(ref mut bg_rect) = current_bg {
                if bg_rect.color == bg_color && bg_rect.end_col == col {
                    // Extend current background
                    bg_rect.end_col = col + 1;
                } else {
                    // Save current background and start new one
                    backgrounds.push(bg_rect.clone());
                    current_bg = Some(BackgroundRect {
                        start_col: col,
                        end_col: col + 1,
                        row,
                        color: bg_color,
                    });
                }
            } else {
                // Start new background
                current_bg = Some(BackgroundRect {
                    start_col: col,
                    end_col: col + 1,
                    row,
                    color: bg_color,
                });
            }

            // Handle text runs
            if let Some(ref mut run) = current_run {
                if run.fg_color == fg_color
                    && run.bg_color == bg_color
                    && run.bold == bold
                    && run.italic == italic
                    && run.underline == underline
                {
                    // Extend current run
                    run.text.push(ch);
                } else {
                    // Save current run and start new one
                    text_runs.push(run.clone());
                    current_run = Some(BatchedTextRun {
                        text: ch.to_string(),
                        start_col: col,
                        row,
                        fg_color,
                        bg_color,
                        bold,
                        italic,
                        underline,
                    });
                }
            } else {
                // Start new run
                current_run = Some(BatchedTextRun {
                    text: ch.to_string(),
                    start_col: col,
                    row,
                    fg_color,
                    bg_color,
                    bold,
                    italic,
                    underline,
                });
            }
        }

        // Push final run and background
        if let Some(run) = current_run {
            text_runs.push(run);
        }
        if let Some(bg) = current_bg {
            backgrounds.push(bg);
        }

        // Merge adjacent backgrounds with same color
        let merged_backgrounds = self.merge_backgrounds(backgrounds);

        (merged_backgrounds, text_runs)
    }

    /// Merge adjacent background rects with same color.
    ///
    /// This optimization reduces the number of rectangles to paint by
    /// combining horizontally adjacent rectangles that share the same color.
    ///
    /// # Arguments
    ///
    /// * `rects` - Vector of background rectangles to merge
    ///
    /// # Returns
    ///
    /// A new vector with merged rectangles
    fn merge_backgrounds(&self, mut rects: Vec<BackgroundRect>) -> Vec<BackgroundRect> {
        if rects.is_empty() {
            return rects;
        }

        let mut merged = Vec::new();
        let mut current = rects.remove(0);

        for rect in rects {
            if current.can_merge_with(&rect) {
                current.end_col = rect.end_col;
            } else {
                merged.push(current);
                current = rect;
            }
        }

        merged.push(current);
        merged
    }

    /// Paint terminal content to the window.
    ///
    /// This is the main rendering method that draws the terminal grid,
    /// including backgrounds, text, and cursor.
    ///
    /// # Arguments
    ///
    /// * `bounds` - The bounding box to render within
    /// * `padding` - Padding around the terminal content
    /// * `term` - The terminal state
    /// * `selection` - Current selection (normalized ascending), if any
    /// * `window` - The GPUI window
    /// * `cx` - The application context
    pub fn paint(
        &self,
        bounds: Bounds<Pixels>,
        padding: Edges<Pixels>,
        term: &Term<GpuiEventProxy>,
        selection: Option<(SelPoint, SelPoint)>,
        window: &mut Window,
        _cx: &mut App,
    ) {
        // Get terminal dimensions
        let grid = term.grid();
        let num_lines = grid.screen_lines();
        let num_cols = grid.columns();
        let colors = term.colors();

        // Calculate default background color
        let default_bg = self.palette.resolve(
            Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background),
            colors,
        );

        // Paint default background (covers full bounds including padding)
        window.paint_quad(quad(
            bounds,
            px(0.0),
            default_bg,
            Edges::<Pixels>::default(),
            transparent_black(),
            Default::default(),
        ));

        // Calculate origin offset (content starts after padding)
        let origin = Point {
            x: bounds.origin.x + padding.left,
            y: bounds.origin.y + padding.top,
        };

        // Iterate over visible lines. Line(0) is the top of the visible screen
        // (negative lines are scrollback), so shift by display_offset to show
        // history when the viewport is scrolled up.
        let display_offset = grid.display_offset() as i32;
        // 选区 → 网格行范围（绝对行 + topmost），行循环内逐行画高亮。
        let topmost = grid.topmost_line().0;
        let sel = selection.map(|(s, e)| (s, e, s.line + topmost, e.line + topmost));
        for line_idx in 0..num_lines {
            let line = Line(line_idx as i32 - display_offset);

            // Collect cells for this line
            let mut cells: Vec<(usize, Cell)> = (0..num_cols)
                .map(|col_idx| {
                    let col = Column(col_idx);
                    let point = AlacPoint::new(line, col);
                    let cell = grid[point].clone();
                    (col_idx, cell)
                })
                .collect();

            // 客户端样式规则：识别纯文本模式就地改写前景色（见 restyle_line）。
            restyle_line(&mut cells, &self.style);

            // Layout the row for backgrounds
            let (backgrounds, _) = self.layout_row(line_idx, cells.iter().cloned(), colors);

            // Paint backgrounds
            for bg_rect in backgrounds {
                // Skip if it's the default background color
                if bg_rect.color == default_bg {
                    continue;
                }

                let x = origin.x + self.cell_width * (bg_rect.start_col as f32);
                let y = origin.y + self.cell_height * (bg_rect.row as f32);
                let width = self.cell_width * ((bg_rect.end_col - bg_rect.start_col) as f32);
                let height = self.cell_height;

                let rect_bounds = Bounds {
                    origin: Point { x, y },
                    size: Size { width, height },
                };

                window.paint_quad(quad(
                    rect_bounds,
                    px(0.0),
                    bg_rect.color,
                    Edges::<Pixels>::default(),
                    transparent_black(),
                    Default::default(),
                ));
            }

            // 选区高亮（行背景之上、文字之下）：与选区行区间相交的行，
            // 首行从起点列起、末行到终点列止，中间行整行。
            if let Some((s, e, gs, ge)) = sel {
                if (gs..=ge).contains(&line.0) {
                    let cs = if line.0 == gs { s.col } else { 0 };
                    let ce = if line.0 == ge { (e.col + 1).min(num_cols) } else { num_cols };
                    if ce > cs {
                        let x = origin.x + self.cell_width * cs as f32;
                        let y = origin.y + self.cell_height * line_idx as f32;
                        window.paint_quad(quad(
                            Bounds {
                                origin: Point { x, y },
                                size: Size {
                                    width: self.cell_width * (ce - cs) as f32,
                                    height: self.cell_height,
                                },
                            },
                            px(0.0),
                            SELECTION_BG,
                            Edges::<Pixels>::default(),
                            transparent_black(),
                            Default::default(),
                        ));
                    }
                }
            }

            // Calculate vertical offset to center text in cell
            // The multiplier adds extra height; we want to distribute it evenly top/bottom
            let base_height = self.cell_height / self.line_height_multiplier;
            let vertical_offset = (self.cell_height - base_height) / 2.0;

            // Paint each character individually at exact cell positions
            // This ensures perfect alignment for terminal emulation
            for (col_idx, cell) in cells {
                let ch = cell.c;

                // Skip empty cells (space or null)
                if ch == ' ' || ch == '\0' {
                    continue;
                }

                let x = origin.x + self.cell_width * (col_idx as f32);
                let y = origin.y + self.cell_height * (line_idx as f32) + vertical_offset;

                // Get cell colors
                let fg_color = self.palette.resolve(cell.fg, colors);

                // Get cell flags for styling
                let flags = cell.flags;
                let bold = flags.contains(alacritty_terminal::term::cell::Flags::BOLD);
                let italic = flags.contains(alacritty_terminal::term::cell::Flags::ITALIC);
                let underline = flags.contains(alacritty_terminal::term::cell::Flags::UNDERLINE);

                // Create font with styling
                let font = Font {
                    family: self.font_family.clone().into(),
                    features: FontFeatures::default(),
                    fallbacks: None,
                    weight: if bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if italic {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    },
                };

                // Create text run for this single character
                let char_str = ch.to_string();
                let text_run = TextRun {
                    len: char_str.len(),
                    font,
                    color: fg_color,
                    background_color: None,
                    underline: if underline {
                        Some(UnderlineStyle {
                            thickness: px(1.0),
                            color: Some(fg_color),
                            wavy: false,
                        })
                    } else {
                        None
                    },
                    strikethrough: None,
                };

                // Shape and paint the character
                let text: SharedString = char_str.into();
                let shaped_line =
                    window
                        .text_system()
                        .shape_line(text, self.font_size, &[text_run], None);

                // Paint at exact cell position (ignore errors)
                let _ = shaped_line.paint(Point { x, y }, self.cell_height, window, _cx);
            }
        }

        // Paint cursor; hide it when the viewport is scrolled far enough that
        // the cursor row (buffer line + display_offset) falls outside it.
        let cursor_point = grid.cursor.point;
        let cursor_viewport_line = cursor_point.line.0 + display_offset;
        if (0..num_lines as i32).contains(&cursor_viewport_line) {
            let cursor_x = origin.x + self.cell_width * (cursor_point.column.0 as f32);
            let cursor_y = origin.y + self.cell_height * (cursor_viewport_line as f32);

            let cursor_color = self.palette.resolve(
                Color::Named(alacritty_terminal::vte::ansi::NamedColor::Cursor),
                colors,
            );

            let cursor_bounds = Bounds {
                origin: Point {
                    x: cursor_x,
                    y: cursor_y,
                },
                size: Size {
                    width: self.cell_width,
                    height: self.cell_height,
                },
            };

            window.paint_quad(quad(
                cursor_bounds,
                px(0.0),
                cursor_color,
                Edges::<Pixels>::default(),
                transparent_black(),
                Default::default(),
            ));
        }
    }
}

// ===== 客户端样式规则（全局配色由 app 决定，与远端无关） =====
//
// 远端（root 的 bashrc、MOTD、sshd 登录横幅）常常一个颜色标记都不发，
// 而终端只能给"被标记"的内容上色。这里由 app 自定规则：在渲染层对识别出的
// 纯文本模式就地改写单元格前景色——零注入、零服务器改动、随主题即时变色。
//
// 规则按**类别**组织（[`STYLE_CATS`]，引擎匹配器固定），每类可独立配置
// 启用/颜色槽/加粗/下划线（[`RuleStyle`]），整套配置即 [`StyleTemplate`]，
// 可序列化（JSON）持久化为用户自定义模板，经 TerminalConfig.style 注入。

/// 规则类别清单（id, UI 显示名）。索引即 StyleTemplate.rules 下标。
pub const STYLE_CATS: &[(&str, &str)] = &[
    ("prompt_user", "提示符 · 用户@主机"),
    ("prompt_path", "提示符 · 路径"),
    ("url", "链接 URL"),
    ("last_login", "Last login 整行"),
    ("ip", "IP 地址"),
    ("date", "日期时间"),
    ("ls_perm", "ls · 权限串"),
    ("ls_size", "ls · 大小"),
    ("ls_meta", "ls · 属主/属组/链接数"),
    ("ls_dir", "ls · 目录名"),
    ("ls_exec", "ls · 可执行名"),
    ("ls_symlink", "ls · 符号链接名"),
    ("ls_archive", "ls · 压缩包名"),
    ("ls_image", "ls · 图片名"),
    ("ls_device", "ls · 设备/socket/管道名"),
];

/// 类别下标（内部使用，与 STYLE_CATS 对应）。
mod cat {
    pub const PROMPT_USER: usize = 0;
    pub const PROMPT_PATH: usize = 1;
    pub const URL: usize = 2;
    pub const LAST_LOGIN: usize = 3;
    pub const IP: usize = 4;
    pub const DATE: usize = 5;
    pub const LS_PERM: usize = 6;
    pub const LS_SIZE: usize = 7;
    pub const LS_META: usize = 8;
    pub const LS_DIR: usize = 9;
    pub const LS_EXEC: usize = 10;
    pub const LS_SYMLINK: usize = 11;
    pub const LS_ARCHIVE: usize = 12;
    pub const LS_IMAGE: usize = 13;
    pub const LS_DEVICE: usize = 14;
}

/// 颜色槽位表（UI 显示名, NamedColor）。RuleStyle.slot 为此表下标。
pub const STYLE_SLOTS: [(&str, NamedColor); 18] = [
    ("默认", NamedColor::Foreground),
    ("暗化", NamedColor::DimForeground),
    ("黑", NamedColor::Black),
    ("红", NamedColor::Red),
    ("绿", NamedColor::Green),
    ("黄", NamedColor::Yellow),
    ("蓝", NamedColor::Blue),
    ("洋红", NamedColor::Magenta),
    ("青", NamedColor::Cyan),
    ("白", NamedColor::White),
    ("亮黑", NamedColor::BrightBlack),
    ("亮红", NamedColor::BrightRed),
    ("亮绿", NamedColor::BrightGreen),
    ("亮黄", NamedColor::BrightYellow),
    ("亮蓝", NamedColor::BrightBlue),
    ("亮洋红", NamedColor::BrightMagenta),
    ("亮青", NamedColor::BrightCyan),
    ("亮白", NamedColor::BrightWhite),
];

/// 单条类别的样式配置（可序列化，存用户自定义模板）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuleStyle {
    pub enabled: bool,
    /// 颜色槽位下标（STYLE_SLOTS）。
    pub slot: u8,
    pub bold: bool,
    pub underline: bool,
}

impl RuleStyle {
    const fn off() -> Self {
        Self { enabled: false, slot: 0, bold: false, underline: false }
    }

    const fn on(slot: u8) -> Self {
        Self { enabled: true, slot, bold: false, underline: false }
    }

    const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
}

/// 一套本地着色模板：每个类别一条配置（顺序同 [`STYLE_CATS`]）。
/// 可 JSON 序列化持久化；内置 standard/minimal 两套由 [`StyleTemplate::standard`]
/// 与 [`StyleTemplate::minimal`] 提供，用户模板由 app 侧存取。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StyleTemplate {
    pub rules: Vec<RuleStyle>,
}

impl Default for StyleTemplate {
    fn default() -> Self {
        Self::standard()
    }
}

impl StyleTemplate {
    /// 标准：全量规则（绿提示符/蓝路径/青 URL/黄登录行/洋红 IP/亮蓝日期/
    /// 亮青权限/黄大小/暗化属主 + ls 名字按类型着色）。
    pub fn standard() -> Self {
        use cat::*;
        let mut rules = [RuleStyle::off(); STYLE_CATS.len()];
        rules[PROMPT_USER] = RuleStyle::on(4); // 绿
        rules[PROMPT_PATH] = RuleStyle::on(6); // 蓝
        rules[URL] = RuleStyle::on(8).underline(); // 青
        rules[LAST_LOGIN] = RuleStyle::on(5); // 黄
        rules[IP] = RuleStyle::on(7); // 洋红
        rules[DATE] = RuleStyle::on(14); // 亮蓝
        rules[LS_PERM] = RuleStyle::on(16); // 亮青
        rules[LS_SIZE] = RuleStyle::on(5); // 黄
        rules[LS_META] = RuleStyle::on(1); // 暗化
        rules[LS_DIR] = RuleStyle::on(6).bold(); // 蓝
        rules[LS_EXEC] = RuleStyle::on(4).bold(); // 绿
        rules[LS_SYMLINK] = RuleStyle::on(8); // 青
        rules[LS_ARCHIVE] = RuleStyle::on(3); // 红
        rules[LS_IMAGE] = RuleStyle::on(7); // 洋红
        rules[LS_DEVICE] = RuleStyle::on(5).bold(); // 黄
        Self { rules: rules.to_vec() }
    }

    /// 简洁：仅提示符 + URL + ls 名字着色，无字段级与杂项规则。
    pub fn minimal() -> Self {
        use cat::*;
        let mut rules = [RuleStyle::off(); STYLE_CATS.len()];
        rules[PROMPT_USER] = RuleStyle::on(4);
        rules[PROMPT_PATH] = RuleStyle::on(6);
        rules[URL] = RuleStyle::on(8).underline();
        rules[LS_DIR] = RuleStyle::on(6).bold();
        rules[LS_EXEC] = RuleStyle::on(4).bold();
        rules[LS_SYMLINK] = RuleStyle::on(8);
        Self { rules: rules.to_vec() }
    }

    /// 取某类别的生效样式；未启用或长度不符（脏数据）返回 None。
    fn rule(&self, cat_idx: usize) -> Option<(NamedColor, bool, bool)> {
        let r = self.rules.get(cat_idx)?;
        if !r.enabled {
            return None;
        }
        let slot = STYLE_SLOTS.get(r.slot as usize)?;
        Some((slot.1, r.bold, r.underline))
    }
}

/// 对一行单元格应用客户端样式规则（就地改写 fg / UNDERLINE / BOLD 标记）。
/// 后应用的类别覆盖先应用的（URL 优先级最高，其次 IP/日期）。
fn restyle_line(cells: &mut [(usize, Cell)], template: &StyleTemplate) {
    // chars 与 cells 的可着色字符一一对应（跳过宽字符占位），按下标映射回单元格。
    let mut chars: Vec<char> = Vec::with_capacity(cells.len());
    for (_, cell) in cells.iter() {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        chars.push(if cell.c == '\0' { ' ' } else { cell.c });
    }

    let mut overrides: Vec<(usize, usize, NamedColor, bool, bool)> = Vec::new();
    let push = |cat_idx: usize, range: (usize, usize), ov: &mut Vec<(usize, usize, NamedColor, bool, bool)>| {
        if let Some((color, bold, underline)) = template.rule(cat_idx) {
            ov.push((range.0, range.1, color, underline, bold));
        }
    };

    // 提示符
    if let Some((at_end, path_start, path_end)) = prompt_segments(&chars) {
        push(cat::PROMPT_USER, (0, at_end), &mut overrides);
        push(cat::PROMPT_PATH, (path_start, path_end), &mut overrides);
    }
    // ls -l 行
    if let Some(ls) = ls_line(&chars) {
        push(cat::LS_PERM, ls.perms, &mut overrides);
        push(cat::LS_SIZE, ls.size, &mut overrides);
        push(cat::LS_META, ls.links, &mut overrides);
        push(cat::LS_META, ls.owner, &mut overrides);
        push(cat::LS_META, ls.group, &mut overrides);
        push(cat::DATE, ls.date, &mut overrides);
        let name_cat = match ls.kind {
            LsKind::Dir => cat::LS_DIR,
            LsKind::Exec => cat::LS_EXEC,
            LsKind::Symlink => cat::LS_SYMLINK,
            LsKind::Archive => cat::LS_ARCHIVE,
            LsKind::Image => cat::LS_IMAGE,
            LsKind::SocketOrPipe | LsKind::Device => cat::LS_DEVICE,
            LsKind::Plain => usize::MAX, // 普通文件：着色交由默认前景
        };
        if name_cat != usize::MAX {
            push(name_cat, ls.name, &mut overrides);
        }
    }
    // 整行 / 通用模式（后写优先 → URL > IP > 日期 > 登录行）
    if starts_with_ascii(&chars, "Last login:") {
        push(cat::LAST_LOGIN, (0, chars.len()), &mut overrides);
    }
    for range in date_segments(&chars) {
        push(cat::DATE, range, &mut overrides);
    }
    for range in ip_segments(&chars) {
        push(cat::IP, range, &mut overrides);
    }
    for range in url_segments(&chars) {
        push(cat::URL, range, &mut overrides);
    }
    if overrides.is_empty() {
        return;
    }

    let mut fg_of: Vec<Option<NamedColor>> = vec![None; chars.len()];
    let mut ul_of = vec![false; chars.len()];
    let mut bold_of = vec![false; chars.len()];
    for (start, end, color, underline, bold) in overrides {
        for i in start.min(chars.len())..end.min(chars.len()) {
            fg_of[i] = Some(color);
            ul_of[i] = ul_of[i] || underline;
            bold_of[i] = bold_of[i] || bold;
        }
    }
    let mut ci = 0;
    for (_, cell) in cells.iter_mut() {
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if let Some(color) = fg_of[ci] {
            cell.fg = Color::Named(color);
            if ul_of[ci] {
                cell.flags.insert(Flags::UNDERLINE);
            }
            if bold_of[ci] {
                cell.flags.insert(Flags::BOLD);
            }
        }
        ci += 1;
    }
}

/// ASCII 前缀匹配（行内容按字符数组给出）。
fn starts_with_ascii(chars: &[char], prefix: &str) -> bool {
    let pb: Vec<char> = prefix.chars().collect();
    chars.len() >= pb.len() && chars[..pb.len()] == pb[..]
}

/// 解析经典 Debian 提示符 `user@host:path[$#]`（其后可跟空格与命令）。
///
/// 返回 `(user@host 结束位置, path 开始位置, path 结束位置)`，均为字符数组的
/// 半开区间；不匹配（无 `@`/`:`/`$#` 终结符、含空格的 user/host/path）返回 None。
fn prompt_segments(chars: &[char]) -> Option<(usize, usize, usize)> {
    let at = chars.iter().position(|&c| c == '@')?;
    let is_word = |c: char| c.is_ascii_graphic() && c != ':' && c != '@';
    if at == 0 || !chars[..at].iter().all(|&c| is_word(c)) {
        return None;
    }
    let colon = chars.iter().position(|&c| c == ':')?;
    if colon <= at + 1 {
        return None; // host 为空
    }
    if !chars[at + 1..colon].iter().all(|&c| is_word(c)) {
        return None;
    }
    let rest = &chars[colon + 1..];
    let term = rest.iter().position(|&c| c == '#' || c == '$')?;
    let path = &rest[..term];
    if !path.is_empty() && !path.iter().all(|&c| is_word(c)) {
        return None; // 路径含空格等：不是提示符
    }
    // 终结符之后必须是行尾或空白，防止把正文里的 `$`/`#` 误判进来。
    let after = rest.get(term + 1);
    match after {
        None => {}
        Some(&c) if c == ' ' => {}
        _ => return None,
    }
    Some((colon, colon + 1, colon + 1 + term))
}

/// 从提示符行文本提取工作目录（[`prompt_segments`] 的取值包装，
/// 供目录面板跟随控制台 cd 使用）。
///
/// 只接受 `~` 开头（`\w` 的 home 形态，展开由宿主按 home 处理）或 `/` 开头
/// 的路径；空路径 / 相对形态（自定义 `\W` 等）歧义大，不上报。
pub fn prompt_cwd(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let (_, ps, pe) = prompt_segments(&chars)?;
    if pe <= ps {
        return None;
    }
    let path: String = chars[ps..pe].iter().collect();
    (path.starts_with('~') || path.starts_with('/')).then_some(path)
}

/// 扫描行内所有 `http://` / `https://` URL 段（到下一个空白或行尾）。
fn url_segments(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let schemes = ["https://", "http://"];
    let mut i = 0;
    while i < chars.len() {
        let mut matched = None;
        for scheme in &schemes {
            if starts_with_ascii(&chars[i..], scheme) {
                matched = Some(scheme.len());
                break;
            }
        }
        if let Some(prefix_len) = matched {
            let mut j = i + prefix_len;
            while j < chars.len() && chars[j].is_ascii_graphic() {
                j += 1;
            }
            out.push((i, j));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// ls -l 条目类型（由权限串与名字扩展名判定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LsKind {
    Dir,
    Symlink,
    Exec,
    SocketOrPipe,
    Device,
    Archive,
    Image,
    Plain,
}

/// `ls -l` 单行的字段区间（字符下标，半开区间）。
struct LsLine {
    kind: LsKind,
    perms: (usize, usize),
    links: (usize, usize),
    owner: (usize, usize),
    group: (usize, usize),
    size: (usize, usize),
    /// 三个日期 token 的连续区间（含中间空格），如 `Oct 14 08:45`。
    date: (usize, usize),
    /// 名字到行尾（含符号链接的 ` -> target`）。
    name: (usize, usize),
}

/// 解析 `ls -l` 输出行：`perms links owner group size date(3 tokens) name…`。
/// 首字段必须是 10 位权限串（`dlbcps-` + 9 位 rwxstS-），否则不是 ls 行。
fn ls_line(chars: &[char]) -> Option<LsLine> {
    // 按空白切 token（记录区间），收集到第 9 个（名字 token）即可停止切分。
    let mut tokens: Vec<(usize, usize)> = Vec::with_capacity(9);
    let mut i = 0;
    let n = chars.len();
    while i < n && tokens.len() < 9 {
        while i < n && chars[i] == ' ' {
            i += 1;
        }
        if i >= n {
            break;
        }
        let start = i;
        while i < n && chars[i] != ' ' {
            i += 1;
        }
        tokens.push((start, i));
    }
    if tokens.len() < 9 {
        return None;
    }

    // 权限串校验
    let (p0, p1) = tokens[0];
    if p1 - p0 != 10 {
        return None;
    }
    let perm_chars = &chars[p0..p1];
    if !matches!(perm_chars[0], 'd' | 'l' | 'b' | 'c' | 's' | 'p' | '-') {
        return None;
    }
    if perm_chars[1..].iter().any(|&c| !matches!(c, 'r' | 'w' | 'x' | 's' | 'S' | 't' | 'T' | '-')) {
        return None;
    }

    // GNU 日期为 3 个 token（Oct 14 08:45 / Oct 14 2024）；名字从第 9 个 token 起连到行尾。
    let name_start = tokens[8].0;
    let name: Vec<char> = chars[name_start..].iter().copied().collect();

    let kind = match perm_chars[0] {
        'd' => LsKind::Dir,
        'l' => LsKind::Symlink,
        's' | 'p' => LsKind::SocketOrPipe,
        'b' | 'c' => LsKind::Device,
        _ => {
            // 三个执行位（user/group/other 的 x，s=SetUID 含执行）任一可执行
            let exec = [3usize, 6, 9].iter().any(|&i| matches!(perm_chars[i], 'x' | 's'));
            if exec {
                LsKind::Exec
            } else if has_archive_ext(&name) {
                LsKind::Archive
            } else if has_image_ext(&name) {
                LsKind::Image
            } else {
                LsKind::Plain
            }
        }
    };

    let date_start = tokens[5].0;
    let date_end = tokens[7].1;
    Some(LsLine {
        kind,
        perms: tokens[0],
        links: tokens[1],
        owner: tokens[2],
        group: tokens[3],
        size: tokens[4],
        date: (date_start, date_end),
        name: (name_start, n),
    })
}

/// 名字（可含 ` -> target`）是否以压缩包扩展名结尾。
fn has_archive_ext(name: &[char]) -> bool {
    let s: String = name.iter().collect();
    const EXTS: [&str; 10] =
        [".tar", ".tgz", ".gz", ".zip", ".xz", ".bz2", ".7z", ".rar", ".zst", ".iso"];
    EXTS.iter().any(|e| s.ends_with(e))
}

fn has_image_ext(name: &[char]) -> bool {
    let s: String = name.iter().collect();
    const EXTS: [&str; 6] = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"];
    EXTS.iter().any(|e| s.ends_with(e))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// 扫描行内 IPv4 地址段（四个点分十进制组，每组 ≤255，前后不能再是数字或点，
/// 避免把版本号 `1.2.3.4.5` 的前四段误判进来）。
fn ip_segments(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let dig = |i: usize| i < n && chars[i].is_ascii_digit();
    let mut i = 0;
    while i < n {
        if !dig(i) {
            i += 1;
            continue;
        }
        // 数字串起点；尝试解析 4 个「点分十进制组」
        let start = i;
        let mut j = i;
        let mut ok = true;
        for g in 0..4 {
            let gs = j;
            while dig(j) {
                j += 1;
            }
            let len = j - gs;
            let octet = chars[gs..j].iter().collect::<String>().parse::<u16>();
            if !(1..=3).contains(&len) || octet.map(|v| v > 255).unwrap_or(true) {
                ok = false;
                break;
            }
            if g < 3 {
                if j < n && chars[j] == '.' {
                    j += 1;
                } else {
                    ok = false;
                    break;
                }
            }
        }
        if ok
            && !(j < n && (chars[j].is_ascii_digit() || chars[j] == '.'))
            && !(start > 0 && (chars[start - 1].is_ascii_digit() || chars[start - 1] == '.'))
        {
            out.push((start, j));
            i = j;
        } else {
            i = start + 1;
        }
    }
    out
}

/// 扫描行内日期/时间段：`Oct 14 08:45`、`Oct 14 2026`、`2026-10-14`、
/// `2026-10-14 08:45:32`、以及独立的 `08:45:32`（带秒才独立匹配，避免
/// `16:9` 这类比例误报）。
fn date_segments(chars: &[char]) -> Vec<(usize, usize)> {
    let n = chars.len();
    let mut out = Vec::new();
    let token_at = |i: usize| -> Option<(usize, usize)> {
        // 从 i 向后取一个非空 token（要求 i 处不是空白）
        if i >= n || chars[i] == ' ' {
            return None;
        }
        let s = i;
        let mut e = i;
        while e < n && chars[e] != ' ' {
            e += 1;
        }
        Some((s, e))
    };
    let is_digits = |r: std::ops::Range<usize>| r.clone().count() > 0 && chars[r].iter().all(|c| c.is_ascii_digit());
    let is_time_hms = |t: &[char]| {
        // HH:MM:SS
        t.len() == 8 && t[0..2].iter().all(|c| c.is_ascii_digit()) && t[2] == ':' && t[3..5].iter().all(|c| c.is_ascii_digit()) && t[5] == ':' && t[6..8].iter().all(|c| c.is_ascii_digit())
    };
    let is_time_hm = |t: &[char]| {
        // HH:MM
        t.len() == 5 && t[0..2].iter().all(|c| c.is_ascii_digit()) && t[2] == ':' && t[3..5].iter().all(|c| c.is_ascii_digit())
    };

    let mut i = 0;
    while i < n {
        if chars[i] == ' ' {
            i += 1;
            continue;
        }
        let Some((s, e)) = token_at(i) else { break };
        let tok = &chars[s..e];
        let word = |k: usize| -> bool {
            // 独立单词边界：前后不是字母（避免匹配 forget 里的 get）
            let before_ok = s == 0 || !chars[s - 1].is_ascii_alphabetic();
            let after_ok = e >= n || !chars[e].is_ascii_alphabetic();
            tok.len() == k && before_ok && after_ok
        };

        let mut matched_len = None;
        // ① 月名日期：Oct 14 08:45 / Oct 14 2026
        if word(3) && tok.iter().all(|c| c.is_ascii_alphabetic()) {
            let name: String = tok.iter().collect();
            if MONTHS.contains(&name.as_str()) {
                if let Some((d1s, d1e)) = token_at(e + 1) {
                    if is_digits(d1s..d1e) && (d1e - d1s) <= 2 {
                        if let Some((t2s, t2e)) = token_at(d1e + 1) {
                            let t2 = &chars[t2s..t2e];
                            if is_time_hms(t2) || is_time_hm(t2) || (is_digits(t2s..t2e) && t2.len() == 4) {
                                matched_len = Some(t2e - s);
                            }
                        }
                    }
                }
            }
        }
        // ② ISO 日期：2026-10-14（可带时间 2026-10-14 08:45[:32]）
        if matched_len.is_none() && tok.len() == 10 && tok[4] == '-' && tok[7] == '-' && is_digits(s..s + 4) && is_digits(s + 5..s + 7) && is_digits(s + 8..s + 10) {
            let mut end = e;
            if let Some((t2s, t2e)) = token_at(e + 1) {
                let t2 = &chars[t2s..t2e];
                if is_time_hms(t2) || is_time_hm(t2) {
                    end = t2e;
                }
            }
            matched_len = Some(end - s);
        }
        // ③ 独立时间（带秒）
        if matched_len.is_none() && is_time_hms(tok) && word(8) {
            matched_len = Some(8);
        }

        if let Some(len) = matched_len {
            out.push((s, s + len));
            i = s + len;
        } else {
            i = e;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 粘贴字节封装（paste_bytes 在 view.rs，测试借住本模块——view.rs 内联
    /// 测试受宏展开递归限制）。
    #[test]
    fn paste_wraps_bracketed_mode() {
        let b = crate::view::paste_bytes("ls\n-la", true);
        assert_eq!(b, b"\x1b[200~ls\n-la\x1b[201~".to_vec());
        // CRLF / 裸 CR 统一为 \n（bracketed 内由 shell 自己解释）
        assert_eq!(
            crate::view::paste_bytes("a\r\nb\rc", true),
            b"\x1b[200~a\nb\nc\x1b[201~".to_vec()
        );
    }

    #[test]
    fn paste_plain_converts_newlines_to_cr() {
        assert_eq!(crate::view::paste_bytes("ls\n-la", false), b"ls\r-la".to_vec());
        assert_eq!(crate::view::paste_bytes("a\r\nb", false), b"a\rb".to_vec());
        assert_eq!(crate::view::paste_bytes("plain", false), b"plain".to_vec());
    }

    /// 选区端点归一化：按 (行, 列) 升序。
    #[test]
    fn selection_normalizes_order() {
        let a = SelPoint { line: 3, col: 5 };
        let b = SelPoint { line: 1, col: 9 };
        let (s, e) = normalize_sel(a, b);
        assert_eq!((s.line, s.col), (1, 9));
        assert_eq!((e.line, e.col), (3, 5));
        // 同行按列
        let (s, e) =
            normalize_sel(SelPoint { line: 2, col: 7 }, SelPoint { line: 2, col: 1 });
        assert_eq!((s.col, e.col), (1, 7));
        // 相等稳定
        let p = SelPoint { line: 0, col: 0 };
        assert_eq!(normalize_sel(p, p), (p, p));
    }

    fn chars_of(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// 由字符串构造一行单元格（默认前景，无标记），用于 restyle_line 端到端断言。
    fn cells_of(s: &str) -> Vec<(usize, Cell)> {
        s.chars().enumerate().map(|(i, c)| {
            let mut cell = Cell::default();
            cell.c = c;
            (i, cell)
        }).collect()
    }

    #[test]
    fn test_prompt_cwd_extraction() {
        use super::prompt_cwd;
        // 经典提示符（含已键入命令）：取路径，~ 形态原样上报。
        assert_eq!(prompt_cwd("root@ubu:~# "), Some("~".into()));
        assert_eq!(prompt_cwd("root@ubu:~/projects$ cargo build"), Some("~/projects".into()));
        assert_eq!(prompt_cwd("user@h:/var/log$ "), Some("/var/log".into()));
        // 空路径 / 非 prompt 行 / 相对路径形态：不上报。
        assert_eq!(prompt_cwd("root@ubu:$ "), None);
        assert_eq!(prompt_cwd("git@github.com:owner/repo.git"), None);
        assert_eq!(prompt_cwd("user@h:src$ "), None);
    }

    #[test]
    fn test_prompt_segments_classic() {
        // root 提示符 + 命令
        let chars = chars_of("root@iZbp1:~# ls -l");
        let (at_end, path_start, path_end) = prompt_segments(&chars).unwrap();
        assert_eq!(&chars[0..at_end].iter().collect::<String>(), "root@iZbp1");
        assert_eq!(&chars[path_start..path_end].iter().collect::<String>(), "~");
        // 裸提示符（光标行）
        let chars = chars_of("root@host:~#");
        assert!(prompt_segments(&chars).is_some());
        // 普通用户 $ 提示符 + 绝对路径
        let chars = chars_of("sam@my.host:/var/log$ tail -f x");
        let (at_end, path_start, path_end) = prompt_segments(&chars).unwrap();
        assert_eq!(&chars[0..at_end].iter().collect::<String>(), "sam@my.host");
        assert_eq!(&chars[path_start..path_end].iter().collect::<String>(), "/var/log");
    }

    #[test]
    fn test_prompt_segments_rejects_non_prompt() {
        // scp/rsync 风格：无 $/# 终结符
        assert!(prompt_segments(&chars_of("git@github.com:owner/repo.git")).is_none());
        // 正文里的 @ 与冒号
        assert!(prompt_segments(&chars_of("mail a@b: hello world")).is_none());
        // 路径含空格（提示符 \w 会转义空格，不会出现裸空格）
        assert!(prompt_segments(&chars_of("root@host:my dir/x# ")).is_none());
        // $ 后不是空白也不是行尾
        assert!(prompt_segments(&chars_of("cost$5 at x@y:z")).is_none());
    }

    #[test]
    fn test_url_segments() {
        let chars = chars_of("see https://ubuntu.com/esm and http://a.b/c done");
        let urls = url_segments(&chars);
        assert_eq!(urls.len(), 2);
        assert_eq!(&chars[urls[0].0..urls[0].1].iter().collect::<String>(), "https://ubuntu.com/esm");
        assert_eq!(&chars[urls[1].0..urls[1].1].iter().collect::<String>(), "http://a.b/c");
        // 行尾 URL 无空白也收口
        let chars = chars_of("go to https://ubuntu.com/workshop");
        let urls = url_segments(&chars);
        assert_eq!(urls, vec![(6, chars.len())]);
    }

    #[test]
    fn test_restyle_line_prompt_and_url() {
        let mut cells = cells_of("root@iZbp1:~# echo https://a.b");
        restyle_line(&mut cells, &StyleTemplate::standard());
        let fg = |s: &str| {
            cells[s.len() - 1].1.fg
        };
        assert_eq!(fg("root@iZbp1"), Color::Named(NamedColor::Green)); // user@host 绿
        assert_eq!(fg("root@iZbp1:"), Color::Named(NamedColor::Foreground)); // ':' 默认
        assert_eq!(fg("root@iZbp1:~"), Color::Named(NamedColor::Blue)); // path 蓝
        assert_eq!(fg("root@iZbp1:~#"), Color::Named(NamedColor::Foreground)); // '#' 默认
        assert_eq!(fg("root@iZbp1:~# echo "), Color::Named(NamedColor::Foreground)); // 命令默认
        // URL 青 + 下划线
        let url_start = "root@iZbp1:~# echo ".len();
        let last = cells.last().unwrap().1.clone();
        assert_eq!(last.fg, Color::Named(NamedColor::Cyan));
        assert!(last.flags.contains(Flags::UNDERLINE));
        assert_eq!(cells[url_start].1.fg, Color::Named(NamedColor::Cyan));
    }

    #[test]
    fn test_restyle_line_last_login_whole_line_yellow() {
        let mut cells = cells_of("Last login: Wed Oct 14 08:41:32 2026 from 1.2.3.4");
        restyle_line(&mut cells, &StyleTemplate::standard());
        // 整行黄打底，但 IP 段（优先级更高）为洋红
        let ip_start = "Last login: Wed Oct 14 08:41:32 2026 from ".len();
        assert_eq!(cells[ip_start].1.fg, Color::Named(NamedColor::Magenta));
        assert_eq!(cells[0].1.fg, Color::Named(NamedColor::Yellow));
        // 简洁模板：登录行/IP/日期规则均关闭
        let mut cells = cells_of("Last login: Wed Oct 14 08:41:32 2026 from 1.2.3.4");
        restyle_line(&mut cells, &StyleTemplate::minimal());
        assert!(cells.iter().all(|(_, c)| c.fg == Color::Named(NamedColor::Foreground)));
    }

    #[test]
    fn test_restyle_line_plain_text_untouched() {
        let mut cells = cells_of("System restart required");
        restyle_line(&mut cells, &StyleTemplate::standard());
        assert!(cells.iter().all(|(_, c)| c.fg == Color::Named(NamedColor::Foreground)));
    }

    /// ls -l 行：目录名蓝加粗、大小黄、属主暗化、权限串亮青、日期亮蓝。
    #[test]
    fn test_restyle_line_ls_long_format() {
        let line = "drwxr-xr-x  3 root root 4096 Oct 14 08:45 mydir";
        let mut cells = cells_of(line);
        restyle_line(&mut cells, &StyleTemplate::standard());
        let cell_at = |i: usize| cells[i].1.clone();
        // 名字 = 蓝加粗
        let name_start = line.len() - "mydir".len();
        let name_cell = cell_at(name_start);
        assert_eq!(name_cell.fg, Color::Named(NamedColor::Blue));
        assert!(name_cell.flags.contains(Flags::BOLD));
        // 权限串 = 亮青（含类型字符）
        assert_eq!(cell_at(0).fg, Color::Named(NamedColor::BrightCyan));
        assert_eq!(cell_at(9).fg, Color::Named(NamedColor::BrightCyan));
        // 大小 = 黄
        let size_start = line.find("4096").unwrap();
        assert_eq!(cell_at(size_start).fg, Color::Named(NamedColor::Yellow));
        // 属主 = 暗化
        let owner_start = line.find("root").unwrap();
        assert_eq!(cell_at(owner_start).fg, Color::Named(NamedColor::DimForeground));
        // 日期 = 亮蓝（日期类）
        let date_start = line.find("Oct").unwrap();
        assert_eq!(cell_at(date_start).fg, Color::Named(NamedColor::BrightBlue));
    }

    /// ls 条目类型分类：可执行/符号链接/压缩包/普通文件/socket。
    #[test]
    fn test_ls_kind_classification() {
        let check = |line: &str, want: LsKind| {
            let chars: Vec<char> = line.chars().collect();
            assert_eq!(ls_line(&chars).unwrap().kind, want, "line: {line}");
        };
        check("-rwxr-xr-x  1 root root  220 Oct 14 08:45 run.sh", LsKind::Exec);
        check("lrwxrwxrwx  1 root root    7 Oct 14 08:45 link -> target", LsKind::Symlink);
        check("-rw-r--r--  1 root root  12M Oct 14 08:45 pkg.tar.gz", LsKind::Archive);
        check("srwxr-xr-x  1 root root    0 Oct 14 08:45 sock", LsKind::SocketOrPipe);
        check("-rw-r--r--  1 root root  4.0K Oct 14 08:45 notes.txt", LsKind::Plain);
        check("-rw-r--r--  1 root root  4.0K Oct 14 08:45 pic.png", LsKind::Image);
        // 非 ls 行不解析
        let not_ls: Vec<char> = "total 12".chars().collect();
        assert!(ls_line(&not_ls).is_none());
        let not_ls: Vec<char> = "hello world foo bar baz qux quux corge grault".chars().collect();
        assert!(ls_line(&not_ls).is_none());
    }

    /// 模板规则可自定义：关掉目录名着色后 ls 行目录名回默认色，其余不变。
    #[test]
    fn test_style_template_custom_rules() {
        let line = "drwxr-xr-x  3 root root 4096 Oct 14 08:45 mydir";
        let mut tpl = StyleTemplate::standard();
        tpl.rules[cat::LS_DIR].enabled = false;
        let mut cells = cells_of(line);
        restyle_line(&mut cells, &tpl);
        let name_start = line.len() - "mydir".len();
        assert_eq!(cells[name_start].1.fg, Color::Named(NamedColor::Foreground));
        // 模板可 JSON 序列化（用户自定义模板持久化路径）
        let json = serde_json::to_string(&tpl).unwrap();
        let back: StyleTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(tpl, back);
    }

    /// IP 匹配：标准 IPv4 命中；版本号/超界段不误报。
    #[test]
    fn test_ip_segments() {
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("from 61.130.104.186 ok", vec!["61.130.104.186"]),
            ("a 1.2.3.4 b 10.0.0.1", vec!["1.2.3.4", "10.0.0.1"]),
            ("ver 1.2.3.4.5 no", vec![]),
            ("bad 300.1.1.1 no", vec![]),
            ("::1 and 127.0.0.1", vec!["127.0.0.1"]),
        ];
        for (line, want) in cases {
            let chars: Vec<char> = line.chars().collect();
            let got: Vec<String> = ip_segments(&chars)
                .into_iter()
                .map(|(s, e)| chars[s..e].iter().collect())
                .collect();
            assert_eq!(got, want, "line: {line}");
        }
    }

    /// 日期匹配：月名日期 / ISO 日期（含带时间）/ 独立 HH:MM:SS；HH:MM 单独不匹配。
    #[test]
    fn test_date_segments() {
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("at Oct 14 08:45 end", vec!["Oct 14 08:45"]),
            ("at Oct 14 2026 end", vec!["Oct 14 2026"]),
            ("on 2026-10-14 ok", vec!["2026-10-14"]),
            ("on 2026-10-14 08:45:32 ok", vec!["2026-10-14 08:45:32"]),
            ("time 08:45:32 only", vec!["08:45:32"]),
            ("ratio 16:9 no", vec![]),
            ("forget getting nothing", vec![]),
        ];
        for (line, want) in cases {
            let chars: Vec<char> = line.chars().collect();
            let got: Vec<String> = date_segments(&chars)
                .into_iter()
                .map(|(s, e)| chars[s..e].iter().collect())
                .collect();
            assert_eq!(got, want, "line: {line}");
        }
    }

    #[test]
    fn test_renderer_creation() {
        let renderer = TerminalRenderer::new(
            "Fira Code".to_string(),
            px(14.0),
            1.2,
            ColorPalette::default(),
        );
        assert_eq!(renderer.font_family, "Fira Code");
        assert_eq!(renderer.font_size, px(14.0));
        assert_eq!(renderer.line_height_multiplier, 1.2);
    }

    #[test]
    fn test_background_rect_merge() {
        let black = Hsla::black();

        let rect1 = BackgroundRect {
            start_col: 0,
            end_col: 5,
            row: 0,
            color: black,
        };

        let rect2 = BackgroundRect {
            start_col: 5,
            end_col: 10,
            row: 0,
            color: black,
        };

        assert!(rect1.can_merge_with(&rect2));

        let rect3 = BackgroundRect {
            start_col: 5,
            end_col: 10,
            row: 1,
            color: black,
        };

        assert!(!rect1.can_merge_with(&rect3));
    }

    #[test]
    fn test_merge_backgrounds() {
        let renderer = TerminalRenderer::new(
            "monospace".to_string(),
            px(14.0),
            1.2,
            ColorPalette::default(),
        );
        let black = Hsla::black();

        let rects = vec![
            BackgroundRect {
                start_col: 0,
                end_col: 5,
                row: 0,
                color: black,
            },
            BackgroundRect {
                start_col: 5,
                end_col: 10,
                row: 0,
                color: black,
            },
        ];

        let merged = renderer.merge_backgrounds(rects);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_col, 0);
        assert_eq!(merged[0].end_col, 10);
    }
}
