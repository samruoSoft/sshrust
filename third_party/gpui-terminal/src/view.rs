//! Main terminal view component for GPUI.
//!
//! This module provides [`TerminalView`], the primary component for embedding terminals
//! in GPUI applications. It manages:
//!
//! - **I/O Streams**: Accepts arbitrary [`Read`]/[`Write`]
//!   streams, allowing integration with any PTY implementation
//! - **Event Handling**: Keyboard and mouse input, with configurable callbacks
//! - **Rendering**: Efficient canvas-based rendering via [`TerminalRenderer`]
//! - **Configuration**: Font, colors, dimensions, and padding via [`TerminalConfig`]
//!
//! # Architecture
//!
//! The terminal uses a push-based async I/O architecture:
//!
//! 1. A background thread reads bytes from the PTY stdout in 4KB chunks
//! 2. Bytes are sent through a [flume](https://docs.rs/flume) channel to an async task
//! 3. The async task processes bytes through the VTE parser and calls `cx.notify()`
//! 4. GPUI repaints the terminal with the updated grid
//!
//! This approach ensures the terminal only wakes when data arrives, avoiding polling.
//!
//! # Thread Safety
//!
//! - [`TerminalView`] itself is not `Send` (it contains GPUI handles)
//! - The stdin writer is wrapped in `Arc<parking_lot::Mutex<>>` for thread-safe writes
//! - Callbacks ([`ResizeCallback`], [`KeyHandler`]) must be `Send + Sync`
//!
//! # Example
//!
//! ```ignore
//! use gpui::{Context, Edges, px};
//! use gpui_terminal::{ColorPalette, TerminalConfig, TerminalView};
//!
//! // In a GPUI window context:
//! let terminal = cx.new(|cx| {
//!     TerminalView::new(pty_writer, pty_reader, TerminalConfig::default(), cx)
//!         .with_resize_callback(move |cols, rows| {
//!             // Notify PTY of new dimensions
//!         })
//!         .with_exit_callback(|_, cx| {
//!             cx.quit();
//!         })
//! });
//!
//! // Focus the terminal to receive keyboard input
//! terminal.read(cx).focus_handle().focus(window);
//! ```

use crate::colors::ColorPalette;
use crate::event::{GpuiEventProxy, TerminalEvent};
use crate::input::keystroke_to_bytes;
use crate::osc7::Osc7Scanner;
use crate::render::{TerminalRenderer, normalize_sel, SelPoint};
use crate::terminal::TerminalState;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as AlacPoint};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use gpui::{Edges, *};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

/// 渲染闭包与鼠标处理之间的共享槽。paint 闭包拿不到 `&self`（只能捕获克隆），
/// 几何（bounds / 格子尺寸 / 回滚视口偏移）由 paint 每帧写回，鼠标事件据此
/// 做像素 → 单元格换算；选区双向读写（鼠标更新、paint 取快照绘制）。
struct TermShared {
    /// 最近一帧画布 bounds（窗口坐标）。
    bounds: Bounds<Pixels>,
    padding: Edges<Pixels>,
    cell_w: f32,
    cell_h: f32,
    /// 回滚视口偏移（0 = 跟随底部），与渲染换算同口径。
    display_offset: i32,
    /// 绝对行号换算基准（grid.topmost_line()）。
    topmost: i32,
    cols: usize,
    rows: usize,
    /// 拖选锚点（左键按下时设定；None = 不在拖选中）。
    anchor: Option<SelPoint>,
    /// 当前选区（归一化升序；None = 无）。
    selection: Option<(SelPoint, SelPoint)>,
}

impl Default for TermShared {
    fn default() -> Self {
        Self {
            bounds: Bounds::default(),
            padding: Edges::default(),
            cell_w: 0.,
            cell_h: 0.,
            display_offset: 0,
            topmost: 0,
            cols: 0,
            rows: 0,
            anchor: None,
            selection: None,
        }
    }
}

/// Configuration for terminal creation and runtime updates.
///
/// This struct defines the terminal's appearance and behavior, including
/// grid dimensions, font settings, scrollback buffer, and color scheme.
///
/// # Default Values
///
/// | Field | Default |
/// |-------|---------|
/// | `cols` | 80 |
/// | `rows` | 24 |
/// | `font_family` | "monospace" |
/// | `font_size` | 14px |
/// | `scrollback` | 10000 |
/// | `line_height_multiplier` | 1.2 |
/// | `padding` | 0px all sides |
/// | `colors` | Default palette |
///
/// # Example
///
/// ```ignore
/// use gpui::{Edges, px};
/// use gpui_terminal::{ColorPalette, TerminalConfig};
///
/// let config = TerminalConfig {
///     cols: 120,
///     rows: 40,
///     font_family: "JetBrains Mono".into(),
///     font_size: px(13.0),
///     scrollback: 50000,
///     line_height_multiplier: 1.1,
///     padding: Edges::all(px(10.0)),
///     colors: ColorPalette::builder()
///         .background(0x1a, 0x1a, 0x1a)
///         .foreground(0xe0, 0xe0, 0xe0)
///         .build(),
/// };
/// ```
///
/// # Runtime Updates
///
/// Configuration can be updated at runtime via [`TerminalView::update_config`].
/// This is useful for implementing features like dynamic font sizing:
///
/// ```ignore
/// terminal.update(cx, |terminal, cx| {
///     let mut config = terminal.config().clone();
///     config.font_size += px(1.0);
///     terminal.update_config(config, cx);
/// });
/// ```
#[derive(Clone, Debug)]
pub struct TerminalConfig {
    /// Number of columns (character width) in the terminal
    pub cols: usize,

    /// Number of rows (lines) in the terminal
    pub rows: usize,

    /// Font family name (e.g., "Fira Code", "JetBrains Mono")
    pub font_family: String,

    /// Font size in pixels
    pub font_size: Pixels,

    /// Maximum number of scrollback lines to keep in history
    pub scrollback: usize,

    /// Multiplier for line height to accommodate tall glyphs (e.g., nerd fonts)
    /// Default is 1.2 (20% extra height)
    pub line_height_multiplier: f32,

    /// Padding around the terminal content (top, right, bottom, left)
    /// The padding area renders with the terminal's background color
    pub padding: Edges<Pixels>,

    /// Color palette for terminal colors (16 ANSI colors, 256 extended colors,
    /// foreground, background, and cursor colors)
    pub colors: ColorPalette,

    /// 本地着色模板（客户端样式规则组开关，见 render::StyleTemplate）
    pub style: crate::render::StyleTemplate,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            font_family: "monospace".into(),
            font_size: px(14.0),
            scrollback: 10000,
            line_height_multiplier: 1.2,
            padding: Edges::all(px(0.0)),
            colors: ColorPalette::default(),
            style: crate::render::StyleTemplate::default(),
        }
    }
}

/// Callback type for PTY resize notifications.
///
/// This callback is invoked when the terminal grid dimensions change,
/// typically due to window resizing. The callback receives the new
/// column and row counts.
///
/// # Arguments
///
/// * `cols` - New number of columns (characters wide)
/// * `rows` - New number of rows (lines tall)
///
/// # Thread Safety
///
/// This callback must be `Send + Sync` as it may be called from the render thread.
///
/// # Example
///
/// ```ignore
/// use portable_pty::PtySize;
///
/// let pty = Arc::new(Mutex::new(pty_master));
/// let pty_clone = pty.clone();
///
/// terminal.with_resize_callback(move |cols, rows| {
///     pty_clone.lock().resize(PtySize {
///         cols: cols as u16,
///         rows: rows as u16,
///         pixel_width: 0,
///         pixel_height: 0,
///     }).ok();
/// });
/// ```
pub type ResizeCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

/// Callback type for key event interception.
///
/// This callback is invoked before the terminal processes a key event,
/// allowing you to intercept and handle specific key combinations.
///
/// # Arguments
///
/// * `event` - The key down event from GPUI
///
/// # Returns
///
/// * `true` - Consume the event (terminal will not process it)
/// * `false` - Let the terminal handle the event normally
///
/// # Thread Safety
///
/// This callback must be `Send + Sync`.
///
/// # Example
///
/// ```ignore
/// terminal.with_key_handler(|event| {
///     let keystroke = &event.keystroke;
///
///     // Intercept Ctrl++ for font size increase
///     if keystroke.modifiers.control && (keystroke.key == "+" || keystroke.key == "=") {
///         // Handle font size increase
///         return true; // Consume the event
///     }
///
///     // Intercept Ctrl+- for font size decrease
///     if keystroke.modifiers.control && keystroke.key == "-" {
///         // Handle font size decrease
///         return true;
///     }
///
///     false // Let terminal handle all other keys
/// });
/// ```
pub type KeyHandler = Box<dyn Fn(&KeyDownEvent) -> bool + Send + Sync>;

/// Callback for terminal bell events.
///
/// This callback is invoked when the terminal bell is triggered (BEL character,
/// ASCII 0x07), allowing you to play a sound or show a visual indicator.
///
/// # Arguments
///
/// * `window` - The GPUI window
/// * `cx` - The context for the TerminalView
///
/// # Example
///
/// ```ignore
/// terminal.with_bell_callback(|window, cx| {
///     // Option 1: Visual bell (flash the window or show an indicator)
///     // Option 2: Play a sound
///     // Option 3: Notify the user via system notification
/// });
/// ```
pub type BellCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;

/// Callback for terminal title changes.
///
/// This callback is invoked when the terminal title changes via escape sequences
/// (OSC 0, OSC 2), allowing you to update the window or tab title.
///
/// # Arguments
///
/// * `window` - The GPUI window
/// * `cx` - The context for the TerminalView
/// * `title` - The new title string
///
/// # Example
///
/// ```ignore
/// terminal.with_title_callback(|window, cx, title| {
///     // Update the window title
///     // Or update a tab label in a tabbed interface
///     println!("Terminal title changed to: {}", title);
/// });
/// ```
pub type TitleCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;

/// Callback for clipboard store requests.
///
/// This callback is invoked when the terminal wants to store data to the clipboard
/// via OSC 52 escape sequence. Applications like tmux and vim can use this to
/// copy text to the system clipboard.
///
/// # Arguments
///
/// * `window` - The GPUI window
/// * `cx` - The context for the TerminalView
/// * `text` - The text to store in the clipboard
///
/// # Example
///
/// ```ignore
/// use gpui_terminal::Clipboard;
///
/// terminal.with_clipboard_store_callback(|window, cx, text| {
///     if let Ok(mut clipboard) = Clipboard::new() {
///         clipboard.copy(text).ok();
///     }
/// });
/// ```
pub type ClipboardStoreCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;

/// Callback for terminal exit events.
///
/// This callback is invoked when the terminal process exits (e.g., shell exits,
/// process terminates). This is detected when the PTY reader reaches EOF.
///
/// # Arguments
///
/// * `window` - The GPUI window
/// * `cx` - The context for the TerminalView
///
/// # Example
///
/// ```ignore
/// terminal.with_exit_callback(|window, cx| {
///     // Option 1: Quit the application
///     cx.quit();
///
///     // Option 2: Close this terminal tab/pane
///     // terminal_manager.close_terminal(id);
///
///     // Option 3: Show an exit message
///     // show_notification("Terminal exited");
/// });
/// ```
pub type ExitCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;

/// Callback for working-directory changes (OSC 7).
///
/// Invoked in the event-loop context (reader task), not during render, when the
/// remote shell reports its cwd via `ESC ] 7 ; file://host/path ST` — typically
/// emitted by a PROMPT_COMMAND / precmd shell-integration hook.
///
/// # Arguments
///
/// * `cx` - The context for the TerminalView (host updates its own state from here)
/// * `path` - The new working directory (percent-decoded, absolute path)
pub type CwdCallback = Box<dyn Fn(&mut Context<TerminalView>, &str)>;

/// The main terminal view component for GPUI applications.
///
/// `TerminalView` is a GPUI entity that implements the [`Render`] trait,
/// providing a complete terminal emulator that can be embedded in any GPUI application.
///
/// # Responsibilities
///
/// - **Terminal State**: Manages the grid, cursor, and colors via [`TerminalState`]
/// - **I/O Streams**: Reads from PTY stdout and writes to PTY stdin
/// - **Event Handling**: Processes keyboard, mouse, and resize events
/// - **Rendering**: Paints text, backgrounds, and cursor via [`TerminalRenderer`]
/// - **Callbacks**: Dispatches events to user-provided callbacks
///
/// # Creating a Terminal
///
/// Use [`TerminalView::new`] within a GPUI entity context:
///
/// ```ignore
/// let terminal = cx.new(|cx| {
///     TerminalView::new(writer, reader, config, cx)
///         .with_resize_callback(resize_callback)
///         .with_exit_callback(|_, cx| cx.quit())
/// });
/// ```
///
/// # Focus
///
/// The terminal must be focused to receive keyboard input:
///
/// ```ignore
/// terminal.read(cx).focus_handle().focus(window);
/// ```
///
/// # Callbacks
///
/// Configure behavior through builder methods:
///
/// - [`with_resize_callback`](Self::with_resize_callback) - PTY size changes
/// - [`with_exit_callback`](Self::with_exit_callback) - Process exit
/// - [`with_key_handler`](Self::with_key_handler) - Key event interception
/// - [`with_bell_callback`](Self::with_bell_callback) - Terminal bell
/// - [`with_title_callback`](Self::with_title_callback) - Title changes
/// - [`with_clipboard_store_callback`](Self::with_clipboard_store_callback) - Clipboard writes
///
/// # Thread Safety
///
/// `TerminalView` is not `Send` as it contains GPUI handles. The stdin writer
/// is internally wrapped in `Arc<parking_lot::Mutex<>>` for safe concurrent access.
pub struct TerminalView {
    /// The terminal state managing the grid and VTE parser
    state: TerminalState,

    /// The renderer for drawing terminal content
    renderer: TerminalRenderer,

    /// Focus handle for keyboard event handling
    focus_handle: FocusHandle,

    /// Writer for sending input to the terminal process
    stdin_writer: Arc<parking_lot::Mutex<Box<dyn Write + Send>>>,

    /// Receiver for terminal events from the event proxy
    event_rx: mpsc::Receiver<TerminalEvent>,

    /// Configuration used to create this terminal
    config: TerminalConfig,

    /// Async task that reads bytes and notifies the view (push-based)
    #[allow(dead_code)]
    _reader_task: Task<()>,

    /// Callback to notify the PTY about size changes
    resize_callback: Option<Arc<ResizeCallback>>,

    /// Optional callback to intercept key events before terminal processing
    key_handler: Option<Arc<KeyHandler>>,

    /// Callback for terminal bell events
    bell_callback: Option<BellCallback>,

    /// Callback for terminal title changes
    title_callback: Option<TitleCallback>,

    /// Callback for clipboard store requests
    clipboard_store_callback: Option<ClipboardStoreCallback>,

    /// Callback for terminal exit events
    exit_callback: Option<ExitCallback>,

    /// OSC 7（cwd）旁路扫描器：跨块拼接序列、提取路径。
    osc7: Osc7Scanner,

    /// 远端最近上报的工作目录（OSC 7）。
    cwd: Option<String>,

    /// cwd 变化回调（宿主接手：刷新目录面板等）。
    cwd_callback: Option<CwdCallback>,

    /// 渲染/鼠标共享的几何与选区状态（见 [`TermShared`]）。
    shared: Arc<parking_lot::Mutex<TermShared>>,
}

impl TerminalView {
    /// Create a new terminal with provided I/O streams.
    ///
    /// This method initializes a new terminal emulator with the given stdin writer
    /// and stdout reader. It spawns a background task to read from stdout and
    /// process incoming bytes through the VTE parser.
    ///
    /// # Arguments
    ///
    /// * `stdin_writer` - Writer for sending input bytes to the terminal process
    /// * `stdout_reader` - Reader for receiving output bytes from the terminal process
    /// * `config` - Terminal configuration (dimensions, font, etc.)
    /// * `cx` - GPUI context for this view
    ///
    /// # Returns
    ///
    /// A new `TerminalView` instance ready to be rendered.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // In a GPUI window context:
    /// let terminal = cx.new(|cx| {
    ///     TerminalView::new(stdin_writer, stdout_reader, TerminalConfig::default(), cx)
    /// });
    /// ```
    pub fn new<W, R>(
        stdin_writer: W,
        stdout_reader: R,
        config: TerminalConfig,
        cx: &mut Context<Self>,
    ) -> Self
    where
        W: Write + Send + 'static,
        R: Read + Send + 'static,
    {
        // Create event channel for terminal events
        let (event_tx, event_rx) = mpsc::channel();

        // Clone event_tx for the reader task to send Exit event when PTY closes
        let exit_event_tx = event_tx.clone();

        // Create event proxy for alacritty
        let event_proxy = GpuiEventProxy::new(event_tx);

        // Create terminal state
        let state = TerminalState::new(config.cols, config.rows, event_proxy);

        // Create renderer with font settings and color palette
        let mut renderer = TerminalRenderer::new(
            config.font_family.clone(),
            config.font_size,
            config.line_height_multiplier,
            config.colors.clone(),
        );
        renderer.style = config.style.clone();

        // Create focus handle
        let focus_handle = cx.focus_handle();

        // Wrap stdin writer in Arc<Mutex> for thread-safe access
        let stdin_writer = Arc::new(parking_lot::Mutex::new(
            Box::new(stdin_writer) as Box<dyn Write + Send>
        ));

        // Create async channel for bytes (push-based notification)
        // Using flume instead of smol::channel because flume is executor-agnostic
        // and properly wakes GPUI's async executor when data arrives
        let (bytes_tx, bytes_rx) = flume::unbounded::<Vec<u8>>();

        // Spawn background thread to read from stdout
        // This thread sends bytes through the async channel
        thread::spawn(move || {
            Self::read_stdout_blocking(stdout_reader, bytes_tx);
        });

        // Spawn async task that awaits on the channel and notifies the view
        // This is push-based: the task blocks until bytes arrive, then immediately notifies
        let reader_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                // Wait for bytes from the background reader (blocks until data arrives)
                match bytes_rx.recv_async().await {
                    Ok(bytes) => {
                        // Process bytes and notify the view.
                        // 返回值：本批识别到的新 cwd（OSC 7 优先，光标行提示符
                        // 兜底；与上次相同则不回调）。
                        let result = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                            let from_osc = view.osc7.feed(&bytes);
                            view.state.process_bytes(&bytes);
                            let new_cwd = from_osc.or_else(|| view.detect_prompt_cwd());
                            let report = match (&view.cwd, &new_cwd) {
                                (Some(prev), Some(n)) if prev == n => None,
                                (_, n) => n.clone(),
                            };
                            if let Some(p) = &new_cwd {
                                view.cwd = Some(p.clone());
                            }
                            cx.notify();
                            report
                        });
                        if result.is_err() {
                            // View was dropped, exit
                            break;
                        }
                        // cwd 回调在实体更新之外触发（事件循环上下文，非渲染期）。
                        if let Ok(Some(path)) = result {
                            let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                                if let Some(ref callback) = view.cwd_callback {
                                    callback(cx, &path);
                                }
                            });
                        }
                    }
                    Err(_) => {
                        // Channel closed - PTY has finished, send Exit event
                        let _ = exit_event_tx.send(TerminalEvent::Exit);
                        // Notify view to process the Exit event
                        let _ = this.update(cx, |_view, cx: &mut Context<Self>| {
                            cx.notify();
                        });
                        break;
                    }
                }
            }
        });

        Self {
            state,
            renderer,
            focus_handle,
            stdin_writer,
            event_rx,
            config,
            _reader_task: reader_task,
            resize_callback: None,
            key_handler: None,
            bell_callback: None,
            title_callback: None,
            clipboard_store_callback: None,
            exit_callback: None,
            osc7: Osc7Scanner::new(),
            cwd: None,
            cwd_callback: None,
            shared: Arc::new(parking_lot::Mutex::new(TermShared::default())),
        }
    }

    /// 当前滚动状态：(display_offset, 历史最大偏移, 屏幕行数)。
    pub fn scroll_info(&self) -> (usize, usize, usize) {
        use alacritty_terminal::grid::Dimensions;
        self.state.with_term(|t| {
            let g = t.grid();
            (g.display_offset(), g.history_size(), g.screen_lines())
        })
    }

    /// 滚动到指定历史偏移（0 = 跟随底部）。
    pub fn scroll_to(&self, offset: usize) {
        use alacritty_terminal::grid::Dimensions;
        self.state.with_term_mut(|t| {
            let max = t.grid().history_size();
            t.grid_mut().scroll_display(alacritty_terminal::grid::Scroll::Top);
            // Top => history_size（最老），再用 Delta 收敛到目标
            let target = offset.min(max);
            let cur = t.grid().display_offset();
            let delta = target as i32 - cur as i32;
            if delta != 0 {
                t.grid_mut().scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
            }
        });
    }

    /// Set a callback to be invoked when the terminal is resized.
    ///
    /// This callback should resize the underlying PTY to match the new dimensions.
    /// The callback receives (cols, rows) as arguments.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called with (cols, rows) on resize
    pub fn with_resize_callback(
        mut self,
        callback: impl Fn(usize, usize) + Send + Sync + 'static,
    ) -> Self {
        self.resize_callback = Some(Arc::new(Box::new(callback)));
        self
    }

    /// Set a callback to intercept key events before terminal processing.
    ///
    /// The callback receives the key event and should return `true` to consume
    /// the event (prevent the terminal from processing it), or `false` to allow
    /// normal terminal processing.
    ///
    /// # Arguments
    ///
    /// * `handler` - A function that receives key events and returns whether to consume them
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_key_handler(|event| {
    ///     // Handle Ctrl++ to increase font size
    ///     if event.keystroke.modifiers.control && event.keystroke.key == "+" {
    ///         // Handle the event
    ///         return true; // Consume the event
    ///     }
    ///     false // Let terminal handle it
    /// })
    /// ```
    pub fn with_key_handler(
        mut self,
        handler: impl Fn(&KeyDownEvent) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.key_handler = Some(Arc::new(Box::new(handler)));
        self
    }

    /// Set a callback to be invoked when the terminal bell is triggered.
    ///
    /// The callback receives a mutable reference to the window and context,
    /// allowing you to play a sound or show a visual indicator.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called when the bell is triggered
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_bell_callback(|window, cx| {
    ///     // Play a sound or flash the screen
    /// })
    /// ```
    pub fn with_bell_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>) + 'static,
    ) -> Self {
        self.bell_callback = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when the terminal title changes.
    ///
    /// The callback receives a mutable reference to the window and context,
    /// along with the new title string.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called with the new title
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_title_callback(|window, cx, title| {
    ///     // Update window title or tab title
    /// })
    /// ```
    pub fn with_title_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>, &str) + 'static,
    ) -> Self {
        self.title_callback = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when the terminal wants to store data to the clipboard.
    ///
    /// The callback receives a mutable reference to the window and context,
    /// along with the text to store. This is typically triggered by OSC 52 escape sequences.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called with the text to store
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_clipboard_store_callback(|window, cx, text| {
    ///     // Store text to system clipboard
    /// })
    /// ```
    pub fn with_clipboard_store_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>, &str) + 'static,
    ) -> Self {
        self.clipboard_store_callback = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when the terminal process exits.
    ///
    /// The callback receives a mutable reference to the window and context,
    /// allowing you to close the terminal view or show an exit message.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called when the process exits
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_exit_callback(|window, cx| {
    ///     // Close the terminal tab or show exit message
    /// })
    /// ```
    pub fn with_exit_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>) + 'static,
    ) -> Self {
        self.exit_callback = Some(Box::new(callback));
        self
    }

    /// Set a callback to be invoked when the remote reports a new working
    /// directory via OSC 7 (`ESC ] 7 ; file://host/path ST`).
    ///
    /// The callback receives the view's context and the decoded absolute path.
    /// It runs in the event-loop context (reader task), not during render.
    pub fn with_cwd_callback(
        mut self,
        callback: impl Fn(&mut Context<TerminalView>, &str) + 'static,
    ) -> Self {
        self.cwd_callback = Some(Box::new(callback));
        self
    }

    /// 最近一次上报的工作目录（OSC 7 / 提示符识别；尚未识别则为 None）。
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// 客户端识别：光标行是提示符时提取其路径（`user@host:path$` 形态，
    /// `\w` 渲染为 `~`/`~/x`/`/abs`）。无 shell 集成时的默认 cwd 来源。
    fn detect_prompt_cwd(&self) -> Option<String> {
        use alacritty_terminal::grid::Dimensions;
        self.state.with_term(|t| {
            let g = t.grid();
            let cursor = g.cursor.point;
            let line = cursor.line;
            let cols = g.columns();
            let mut text = String::with_capacity(cols);
            for col_idx in 0..cols {
                let point = alacritty_terminal::index::Point::new(line, alacritty_terminal::index::Column(col_idx));
                let cell = &g[point];
                if cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
            crate::render::prompt_cwd(text.trim_end())
        })
    }

    /// Background thread that reads from stdout.
    ///
    /// This function runs in a background thread, continuously reading bytes
    /// from the stdout reader and sending them through the async channel.
    /// The async channel allows the main async task to be woken up immediately
    /// when data arrives (push-based).
    fn read_stdout_blocking<R: Read + Send + 'static>(
        mut stdout_reader: R,
        bytes_tx: flume::Sender<Vec<u8>>,
    ) {
        let mut buffer = [0u8; 4096];

        loop {
            match stdout_reader.read(&mut buffer) {
                Ok(0) => {
                    // EOF - channel will be dropped, signaling completion
                    break;
                }
                Ok(n) => {
                    // Send bytes to the async task
                    let bytes = buffer[..n].to_vec();
                    if bytes_tx.send(bytes).is_err() {
                        break; // Channel closed
                    }
                }
                Err(_) => {
                    // Read error
                    break;
                }
            }
        }
    }

    /// Handle keyboard input events.
    ///
    /// Converts GPUI keystrokes to terminal escape sequences and writes them
    /// to the stdin writer. If a key handler is set and returns true, the event
    /// is consumed and not sent to the terminal.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // Check if key handler wants to consume this event
        if let Some(ref handler) = self.key_handler
            && handler(event)
        {
            return; // Event consumed by handler
        }

        // ⌘（platform 修饰键，gpui 0.2.2 里 macOS cmd 位）组合不进终端：
        // 粘贴/复制在此按 macOS 惯例就地处理，其余 meta 组合一律吞掉——
        // 否则字面量字符会漏进 PTY（旧版 ⌘V 敲 "v"）。
        if event.keystroke.modifiers.platform {
            match event.keystroke.key.as_str() {
                "v" => self.paste_clipboard(cx),
                "c" => {
                    self.copy_selection(cx);
                }
                _ => {}
            }
            return;
        }

        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, self.state.mode()) {
            let mut writer = self.stdin_writer.lock();
            let _ = writer.write_all(&bytes);
            let _ = writer.flush();
            drop(writer);
            // 键入即清选区（与 iTerm2 / macOS Terminal 一致）。
            let mut sh = self.shared.lock();
            if sh.selection.take().is_some() || sh.anchor.take().is_some() {
                cx.notify();
            }
        }
    }

    /// 像素坐标 → 绝对行/列单元格（越界钳制进网格；几何未就绪返回 None）。
    fn cell_at(&self, position: Point<Pixels>) -> Option<SelPoint> {
        let sh = self.shared.lock();
        if sh.cell_w <= 0. || sh.cell_h <= 0. || sh.cols == 0 || sh.rows == 0 {
            return None;
        }
        let x: f32 = (position.x - sh.bounds.origin.x - sh.padding.left).into();
        let y: f32 = (position.y - sh.bounds.origin.y - sh.padding.top).into();
        let col = ((x / sh.cell_w).floor() as i32).clamp(0, sh.cols as i32 - 1) as usize;
        let row = ((y / sh.cell_h).floor() as i32).clamp(0, sh.rows as i32 - 1);
        // 渲染同款换算：可见行 row → 网格行 row − display_offset → 绝对行 − topmost。
        Some(SelPoint { line: row - sh.display_offset - sh.topmost, col })
    }

    /// Handle mouse down events: focus + 起始选区。
    /// 单击设锚点（拖动扩展）、双击选词、三击选整行。
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Request focus when clicking the terminal
        window.focus(&self.focus_handle);

        let Some(p) = self.cell_at(event.position) else { return };
        let range = match event.click_count {
            2 => self.expand_to_word(p),
            n if n >= 3 => self.expand_to_line(p),
            _ => (p, p),
        };
        let mut sh = self.shared.lock();
        sh.anchor = Some(range.0);
        sh.selection = (range.0 != range.1).then_some(normalize_sel(range.0, range.1));
        drop(sh);
        cx.notify();
    }

    /// 双击选词：以点击处为中心向两侧扩展到空白边界。
    fn expand_to_word(&self, p: SelPoint) -> (SelPoint, SelPoint) {
        self.state.with_term(|t| {
            let g = t.grid();
            let line = Line(p.line + g.topmost_line().0);
            let cols = g.columns();
            let is_word = |c: char| c != ' ' && c != '\0';
            let char_at = |col: usize| -> char {
                if col >= cols {
                    return ' ';
                }
                let cell = g[AlacPoint::new(line, Column(col))].clone();
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    ' '
                } else {
                    cell.c
                }
            };
            let mid = p.col.min(cols.saturating_sub(1));
            if !is_word(char_at(mid)) {
                return (p, p);
            }
            let mut s = mid;
            let mut e = mid;
            while s > 0 && is_word(char_at(s - 1)) {
                s -= 1;
            }
            while e + 1 < cols && is_word(char_at(e + 1)) {
                e += 1;
            }
            (SelPoint { line: p.line, col: s }, SelPoint { line: p.line, col: e })
        })
    }

    /// 三击选整行（0 列到末列）。
    fn expand_to_line(&self, p: SelPoint) -> (SelPoint, SelPoint) {
        let last = self.state.with_term(|t| t.grid().columns().saturating_sub(1));
        (SelPoint { line: p.line, col: 0 }, SelPoint { line: p.line, col: last })
    }

    /// Handle mouse up events: 结束拖选（选区保留，⌘C 复制）。
    fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.shared.lock().anchor = None;
    }

    /// Handle mouse move events: 左键按住时从锚点扩展选区。
    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 无按键的移动：顺带清掉失联的拖选状态（mouse_up 落在终端外收不到，
        // 靠这里自愈），其余不动。
        if event.pressed_button != Some(MouseButton::Left) {
            let mut sh = self.shared.lock();
            if sh.anchor.take().is_some() {
                cx.notify();
            }
            return;
        }
        let Some(cur) = self.cell_at(event.position) else { return };
        let mut sh = self.shared.lock();
        let Some(a) = sh.anchor else { return };
        let range = normalize_sel(a, cur);
        let changed = sh.selection != Some(range);
        sh.selection = Some(range);
        drop(sh);
        if changed {
            cx.notify();
        }
    }

    /// 提取当前选区文本：按行拼字符、跳过宽字符占位、去行尾空白、行间 \n；
    /// 已被历史淘汰的行跳过。空选区/空文本返回 None。
    fn selection_text(&self) -> Option<String> {
        let (s, e) = self.shared.lock().selection?;
        if s == e {
            return None;
        }
        let text = self.state.with_term(|t| {
            let g = t.grid();
            let top = g.topmost_line().0;
            let bottom = g.bottommost_line().0;
            let cols = g.columns();
            let mut lines: Vec<String> = Vec::new();
            for abs in s.line..=e.line {
                let line = abs + top;
                if line < top || line > bottom {
                    continue; // 行已被回滚历史淘汰
                }
                let cs = if abs == s.line { s.col } else { 0 };
                let ce = if abs == e.line { e.col.min(cols - 1) } else { cols - 1 };
                let mut row = String::new();
                for c in cs..=ce {
                    let cell = g[AlacPoint::new(Line(line), Column(c))].clone();
                    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        continue;
                    }
                    row.push(if cell.c == '\0' { ' ' } else { cell.c });
                }
                lines.push(row.trim_end().to_string());
            }
            lines.join("\n")
        });
        (!text.is_empty()).then_some(text)
    }

    /// ⌘C：复制当前选区到系统剪贴板；返回是否存在可复制的选区。
    fn copy_selection(&mut self, cx: &Context<Self>) -> bool {
        match self.selection_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    /// ⌘V：读系统剪贴板并粘贴进 PTY（按 bracketed paste 模式决定转义）。
    fn paste_clipboard(&mut self, cx: &Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if text.is_empty() {
                return;
            }
            let bytes =
                paste_bytes(&text, self.state.mode().contains(TermMode::BRACKETED_PASTE));
            let mut writer = self.stdin_writer.lock();
            let _ = writer.write_all(&bytes);
            let _ = writer.flush();
        }
    }

    /// Handle scroll events.
    ///
    /// Currently a placeholder for future scrollback support.
    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 回滚历史（alternate screen 如 vim 由应用自行处理鼠标，这里只滚 display offset）
        let cell_h: f32 = self.renderer.cell_height.into();
        let dy: f32 = event.delta.pixel_delta(px(14.)).y.into();
        if dy.abs() < 0.01 || cell_h <= 0. {
            return;
        }
        let lines = (dy.abs() / cell_h).ceil().max(1.0) as i32;
        // 与 gpui 内置滚动（scroll_offset += dy，正=向内容顶部）同号：
        // dy 正 = 看更早内容 = display_offset 增大。
        let delta = if dy > 0. { lines } else { -lines };
        self.state.with_term_mut(|t| {
            t.grid_mut().scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        });
        let _after = self.state.with_term(|t| t.grid().display_offset());
        cx.notify();
    }

    /// Process pending terminal events.
    ///
    /// This method drains all available events from the event receiver
    /// and handles them appropriately. Note: bytes are processed in the
    /// async reader task, not here.
    fn process_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Process terminal events (from alacritty event proxy)
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                TerminalEvent::Wakeup => {
                    // Terminal has new content - already handled by async task
                }
                TerminalEvent::Bell => {
                    if let Some(ref callback) = self.bell_callback {
                        callback(window, cx);
                    }
                }
                TerminalEvent::Title(title) => {
                    if let Some(ref callback) = self.title_callback {
                        callback(window, cx, &title);
                    }
                }
                TerminalEvent::ClipboardStore(text) => {
                    if let Some(ref callback) = self.clipboard_store_callback {
                        callback(window, cx, &text);
                    }
                }
                TerminalEvent::ClipboardLoad => {
                    // Terminal wants to load data from clipboard
                    // TODO: Implement clipboard integration
                }
                TerminalEvent::Exit => {
                    if let Some(ref callback) = self.exit_callback {
                        callback(window, cx);
                    }
                }
            }
        }
    }

    /// Get the current terminal dimensions.
    ///
    /// # Returns
    ///
    /// A tuple of (columns, rows).
    pub fn dimensions(&self) -> (usize, usize) {
        (self.state.cols(), self.state.rows())
    }

    /// Resize the terminal to new dimensions.
    ///
    /// This method should be called when the terminal view size changes.
    /// It updates the internal grid and notifies the terminal process of the new size.
    ///
    /// # Arguments
    ///
    /// * `cols` - New number of columns
    /// * `rows` - New number of rows
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.state.resize(cols, rows);
    }

    /// Get the current terminal configuration.
    ///
    /// # Returns
    ///
    /// A reference to the current configuration.
    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    /// Get the focus handle for this terminal view.
    ///
    /// # Returns
    ///
    /// A reference to the focus handle.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Update the terminal configuration.
    ///
    /// This method updates the terminal's configuration, including font settings,
    /// padding, and color palette. Changes take effect on the next render.
    ///
    /// # Arguments
    ///
    /// * `config` - The new configuration to apply
    /// * `cx` - The context for triggering a repaint
    pub fn update_config(&mut self, config: TerminalConfig, cx: &mut Context<Self>) {
        // Update renderer with new font settings and palette
        self.renderer.font_family = config.font_family.clone();
        self.renderer.font_size = config.font_size;
        self.renderer.line_height_multiplier = config.line_height_multiplier;
        self.renderer.palette = config.colors.clone();
        self.renderer.style = config.style.clone();

        // Store the new config
        self.config = config;

        // Trigger a repaint - cell dimensions will be recalculated via measure_cell()
        cx.notify();
    }

    /// Calculate terminal dimensions from pixel bounds and cell size.
    ///
    /// Helper method to determine how many columns and rows fit in the given bounds.
    #[allow(dead_code)]
    fn calculate_dimensions(&self, bounds: Bounds<Pixels>) -> (usize, usize) {
        let width_f32: f32 = bounds.size.width.into();
        let height_f32: f32 = bounds.size.height.into();
        let cell_width_f32: f32 = self.renderer.cell_width.into();
        let cell_height_f32: f32 = self.renderer.cell_height.into();

        let cols = ((width_f32 / cell_width_f32) as usize).max(1);
        let rows = ((height_f32 / cell_height_f32) as usize).max(1);
        (cols, rows)
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Process any pending events
        self.process_events(window, cx);

        // Get terminal state and renderer for rendering
        let state_arc = self.state.term_arc();
        let renderer = self.renderer.clone();
        let resize_callback = self.resize_callback.clone();
        let padding = self.config.padding;
        let shared = self.shared.clone();

        // 不在根 div 铺背景：painter 的 default_bg quad 覆盖全部 bounds（含
        // padding）且带 background_alpha（宿主窗口透明联动）；div 再铺一层会在
        // 半透明时双重叠加（alpha 复合）把窗口压得比设定值更不透明。
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                    canvas(
                    move |bounds, _window, _cx| bounds,
                    move |bounds, _, window, cx| {
                        use alacritty_terminal::grid::Dimensions;

                        // Measure actual cell dimensions from the font
                        let mut measured_renderer = renderer.clone();
                        measured_renderer.measure_cell(window);

                        // Calculate available space after padding
                        let available_width: f32 =
                            (bounds.size.width - padding.left - padding.right).into();
                        let available_height: f32 =
                            (bounds.size.height - padding.top - padding.bottom).into();
                        let cell_width_f32: f32 = measured_renderer.cell_width.into();
                        let cell_height_f32: f32 = measured_renderer.cell_height.into();

                        let cols = ((available_width / cell_width_f32) as usize).max(1);
                        let rows = ((available_height / cell_height_f32) as usize).max(1);

                        // Helper struct implementing Dimensions for resize
                        struct TermSize {
                            cols: usize,
                            rows: usize,
                        }
                        impl Dimensions for TermSize {
                            fn total_lines(&self) -> usize {
                                self.rows
                            }
                            fn screen_lines(&self) -> usize {
                                self.rows
                            }
                            fn columns(&self) -> usize {
                                self.cols
                            }
                            fn last_column(&self) -> alacritty_terminal::index::Column {
                                alacritty_terminal::index::Column(self.cols.saturating_sub(1))
                            }
                            fn bottommost_line(&self) -> alacritty_terminal::index::Line {
                                alacritty_terminal::index::Line(self.rows as i32 - 1)
                            }
                            fn topmost_line(&self) -> alacritty_terminal::index::Line {
                                alacritty_terminal::index::Line(0)
                            }
                        }

                        // Resize terminal if dimensions changed
                        let mut term = state_arc.lock();
                        let current_cols = term.columns();
                        let current_rows = term.screen_lines();
                        if cols != current_cols || rows != current_rows {
                            // Notify the PTY about the resize
                            if let Some(ref callback) = resize_callback {
                                callback(cols, rows);
                            }
                            term.resize(TermSize { cols, rows });
                        }

                        // 几何写回共享槽（鼠标事件换算用），并快照选区交给绘制。
                        let selection = {
                            let mut sh = shared.lock();
                            sh.bounds = bounds;
                            sh.padding = padding;
                            sh.cell_w = cell_width_f32;
                            sh.cell_h = cell_height_f32;
                            sh.display_offset = term.grid().display_offset() as i32;
                            sh.topmost = term.grid().topmost_line().0;
                            sh.cols = cols;
                            sh.rows = rows;
                            sh.selection
                        };

                        // Paint the terminal with measured dimensions
                        measured_renderer.paint(bounds, padding, &term, selection, window, cx);
                    },
                )
                .size_full(),
            )
    }
}

// Tests are omitted due to macro expansion issues with the test attribute
// in this configuration. Integration tests can be added separately.

/// 粘贴文本 → PTY 字节。bracketed paste 模式下包 `\x1b[200~…\x1b[201~`
/// （vim/tmux 把粘贴当一整块处理，不逐键触发缩进/映射）；否则裸文本、
/// 换行转 `\r`（回车即执行，符合终端粘贴惯例）。
/// 测试在 render.rs（view.rs 内联测试受宏展开递归限制，见上方原注释）。
pub(crate) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let t = text.replace("\r\n", "\n").replace('\r', "\n");
        format!("\x1b[200~{t}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}
