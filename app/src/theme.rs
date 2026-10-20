//! 主题（Palette）与通用样式辅助。
//!
//! - 界面主题 [`THEMES`]：侧栏顶部 ◐ 菜单切换，索引持久化 settings("theme")。
//! - 终端配色 [`TERM_THEMES`]：独立模板目录，与界面主题**不一一对应**。
//!   联动规则：界面主题系列（深/浅）翻转时，终端配色切到同系列上次记忆的选择
//!   （settings 键 `terminal.theme.dark` / `terminal.theme.light` 各自记忆）；
//!   系列内切换界面主题不影响终端配色。
//! UI 代码统一以 `pal.<字段>`（u32）取色，经 `rgb(pal.x)` 着色。
//!
//! ## 背景透明度（整个 app）
//!
//! [`BgFx`]（透明度 + 模糊）持久化在 settings（`app.bg.opacity` / `app.bg.blur`），
//! 运行态镜像为 [`BgGlobal`]（复刻 ThemeGlobal 模式）。取色统一走
//! [`PalFx`]（`theme::current` / `AppUi::theme` 的返回值）：bg/panel/card 三个
//! 表面色经 `pal.surface(pal.bg)` 叠加 alpha，其余颜色照旧 `rgb(pal.x)` 不透明。
//! 窗口背景模式（Opaque/Transparent/Blurred）在开窗时设置，已开窗口在各自
//! render 里对账 [`bg_fx`] 变化后调 `window.set_background_appearance`。

use crate::db::Db;
use gpui::WindowBackgroundAppearance;

/// 全局主题索引（gpui Global）：拖拽 ghost 等独立视图读取当前配色。
#[derive(Clone, Copy)]
pub struct ThemeGlobal(pub usize);

impl gpui::Global for ThemeGlobal {}

// ---------- 背景透明度 ----------

/// 深色系毛玻璃参数（可读性与透视兼顾的三件套）：
/// - alpha 80%：20% 透入（用户调优值）；
/// - 表面压暗 2/3：白底把背景抬到 ~99（0.68×(底色×2/3)+0.32×255）；
/// - **文字提亮 48%（向白）**：补偿被抬亮的背景——灰色次要文字提亮后在
///   最坏情况（纯白底）下对比度仍 ≥ 不透明模式基线的 95%；黑底时只升不降。
///   没有提亮，透入 >8% 时灰字必然看不清（物理上限）。
/// 亮色系：alpha 90%、不压色不提亮（白底深字，对比天然充足）。

/// settings 键（历史键 app.bg.opacity 已弃用）。
pub const KEY_BG_BLUR: &str = "app.bg.blur";

/// 背景效果设置（唯一可调项 = 模糊开关；不透明度经 [`BgFx::effective_opacity`] 推导）。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BgFx {
    /// 透明时叠加系统模糊（macOS 毛玻璃）。
    pub blur: bool,
}

pub const BG_FROST_ALPHA: f32 = 0.8;
pub const BG_FROST_DIM: f32 = 2.0 / 3.0;
pub const BG_FROST_LIFT: f32 = 0.48;
/// 亮色系的 alpha。
pub const BG_SOLID_ALPHA: f32 = 0.9;

/// 一套表面效果参数（由 模糊设置 × 主题系列 推导）。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SurfaceFx {
    /// 表面不透明度。
    pub alpha: f32,
    /// 表面压暗比例（1.0 = 原色）。
    pub dim: f32,
    /// 文字色提亮比例（向白，0 = 不提亮）。
    pub lift: f32,
}

#[cfg(test)]
impl SurfaceFx {
    /// 不透明基线（等效效果关闭；测试对照用）。
    pub const OPAQUE: SurfaceFx = SurfaceFx { alpha: 1.0, dim: 1.0, lift: 0.0 };
}

impl BgFx {
    /// 缺省（无设置/脏数据兜底）：模糊开。
    pub const DEFAULT: BgFx = BgFx { blur: true };

    /// 表面效果参数：深色系 = 毛玻璃三件套（高透 + 压暗 + 文字提亮，
    /// 模糊开关只影响窗口模式/纹理）；亮色系 = 90% 原色。
    pub fn fx(self, light: bool) -> SurfaceFx {
        if !light {
            SurfaceFx { alpha: BG_FROST_ALPHA, dim: BG_FROST_DIM, lift: BG_FROST_LIFT }
        } else {
            SurfaceFx { alpha: BG_SOLID_ALPHA, dim: 1.0, lift: 0.0 }
        }
    }

    /// 从 DB 读取。
    pub fn load(db: &Db) -> Self {
        let blur = db.get_setting(KEY_BG_BLUR).map(|v| v != "0").unwrap_or(true);
        Self { blur }
    }

    /// 落库。
    pub fn save(&self, db: &mut Db) {
        db.set_setting(KEY_BG_BLUR, if self.blur { "1" } else { "0" });
    }

    /// 窗口背景模式：模糊 → Blurred（毛玻璃）；关 → Transparent（清晰透视）。
    /// 不透明度只作用于表面色，不影响窗口模式。
    pub fn window_appearance(self) -> WindowBackgroundAppearance {
        if self.blur {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Transparent
        }
    }
}

/// 背景效果全局镜像：设置窗写入，各窗口 render 对账（颜色即时取、
/// 窗口背景模式变化时调 set_background_appearance）。
#[derive(Clone, Copy)]
pub struct BgGlobal(pub BgFx);

impl gpui::Global for BgGlobal {}

/// 当前背景效果（无 Global 回退缺省）。
pub fn bg_fx(cx: &gpui::App) -> BgFx {
    cx.try_global::<BgGlobal>().map(|g| g.0).unwrap_or(BgFx::DEFAULT)
}

/// 当前界面主题是否亮色系。
pub fn is_light(cx: &gpui::App) -> bool {
    let idx = cx
        .try_global::<ThemeGlobal>()
        .map(|g| g.0)
        .unwrap_or(0)
        .min(THEMES.len() - 1);
    THEMES[idx].1.is_light
}

/// 当前表面效果参数（模糊设置 × 主题系列推导；终端/弹窗表面统一取值）。
pub fn surface_fx(cx: &gpui::App) -> SurfaceFx {
    bg_fx(cx).fx(is_light(cx))
}

/// 带表面效果的取色包装：内部持一份**文字/描边已提亮**的 Palette 副本
/// （半透明档背景会被底色抬亮，灰字向白提亮补偿对比度），调用方经 Deref
/// 读到的 text/muted/dim/border/accent/status 等已是提亮值——全部既有
/// `rgb(pal.x)` 调用点零改动受益。表面色（bg/panel/card）保持原值，
/// 经 `pal.surface(pal.bg)` 压暗+加 alpha 着色。
pub struct PalFx {
    pal: Palette,
    /// 未提亮的基线配色（分割线取色用：隐约可见，不吃文字提亮）。
    base: Palette,
    a: u8,
    d: u8,
}

impl std::ops::Deref for PalFx {
    type Target = Palette;
    fn deref(&self) -> &Palette {
        &self.pal
    }
}

/// 单通道向白提亮。
fn lift_channel(c: u32, k: f32) -> u32 {
    ((c as f32 * (1.0 - k) + 255.0 * k).round() as u32).min(0xff)
}

fn lift_rgb(v: u32, k: f32) -> u32 {
    (lift_channel(v >> 16 & 0xff, k) << 16)
        | (lift_channel(v >> 8 & 0xff, k) << 8)
        | lift_channel(v & 0xff, k)
}

impl PalFx {
    /// 由基线配色 + 表面效果构造（文字提亮在构造期烘焙）。
    /// 不提亮：bg/panel/card（经 surface() 压暗）与 accent_dark
    /// （强调色按钮上的深字，强调底不变则不提）。
    pub fn build(p: &Palette, fx: SurfaceFx) -> Self {
        let k = fx.lift;
        // 主文字额外多提一档（对齐终端前景的"近白"清晰感——终端配色前景
        // 本就近白，界面主文字带色偏，在毛玻璃底上会显得发灰）。
        let k_text = if k > 0.0 { (k + 0.2).min(0.8) } else { 0.0 };
        // 次要文字（muted/dim：分组标题、路径、状态行、提示）紧随主文字
        // 再提一档——用户观感：只比主标题稍微暗一点（原基础提亮档偏灰）。
        let k_secondary = if k > 0.0 { (k + 0.12).min(0.8) } else { 0.0 };
        let mut pal = *p;
        if k > 0.0 {
            pal.text = lift_rgb(pal.text, k_text);
            pal.muted = lift_rgb(pal.muted, k_secondary);
            pal.dim = lift_rgb(pal.dim, k_secondary);
            pal.border = lift_rgb(pal.border, k);
            pal.accent = lift_rgb(pal.accent, k);
            pal.ok = lift_rgb(pal.ok, k);
            pal.warn = lift_rgb(pal.warn, k);
            pal.error = lift_rgb(pal.error, k);
        }
        Self {
            pal,
            base: *p,
            a: (fx.alpha * 255.0).round() as u8,
            d: (fx.dim * 255.0).round() as u8,
        }
    }

    /// 分割线颜色：取**未提亮**的基线 border（深色系为贴近底色的深灰，
    /// 毛玻璃底上只留一道隐约的痕——文字提亮不作用于线条，否则亮灰抢眼）。
    /// 亮色系同为基线 border（无提亮，值等价）。
    pub fn divider(&self) -> u32 {
        self.base.border
    }

    /// 表面色（bg/panel/card）着色：压暗（毛玻璃档）+ 叠加背景透明度。
    /// alpha=255 且不压暗时与 `rgb(v)` 等价，不透明路径零行为变化。
    pub fn surface(&self, v: u32) -> gpui::Rgba {
        let k = self.d as f32 / 255.0;
        let dim = |c: u32| ((c as f32 * k).round() as u32).min(0xff);
        let rgb = (dim(v >> 16 & 0xff) << 16) | (dim(v >> 8 & 0xff) << 8) | dim(v & 0xff);
        gpui::rgba((rgb << 8) | self.a as u32)
    }

    /// 转组件库主题（文字色已提亮、surface_alpha/surface_dim 一并注入；
    /// **border 槽位换成分割线基线色**——暗色系组件边框不吃提亮，与结构
    /// 分割线同色隐约；亮色系无提亮，值等价不变）。
    pub fn kit(&self) -> ui_kit::Theme {
        let mut t = self.pal.kit();
        t.border = self.divider();
        t.surface_alpha = self.a;
        t.surface_dim = self.d;
        t
    }
}

impl Palette {
    /// 转组件库主题（字段一一对应；is_light 为 app 特有，不入 kit）。
    pub fn kit(&self) -> ui_kit::Theme {
        ui_kit::Theme {
            bg: self.bg,
            panel: self.panel,
            card: self.card,
            border: self.border,
            text: self.text,
            muted: self.muted,
            dim: self.dim,
            accent: self.accent,
            accent_dark: self.accent_dark,
            ok: self.ok,
            warn: self.warn,
            error: self.error,
            surface_alpha: 0xff,
            surface_dim: 0xff,
        }
    }
}

/// 当前生效配色（未设置时回退第 0 套）+ 表面效果。
/// 返回 [`PalFx`]：`pal.x` 直读（文字已按需提亮），表面色经 `pal.surface(...)`。
pub fn current(cx: &gpui::App) -> PalFx {
    let idx = cx
        .try_global::<ThemeGlobal>()
        .map(|g| g.0)
        .unwrap_or(0)
        .min(THEMES.len() - 1);
    PalFx::build(&THEMES[idx].1, surface_fx(cx))
}

/// 一套界面配色（u32 RGB，无 alpha）。
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: u32,
    pub panel: u32,
    pub card: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub dim: u32,
    pub accent: u32,
    /// 强调色上的文字色（浅色主题为深字，深色主题为深底字）。
    pub accent_dark: u32,
    pub error: u32,
    pub ok: u32,
    pub warn: u32,
    /// 浅色系列标记：true = 白底系，与终端配色浅色系列联动。
    pub is_light: bool,
}

macro_rules! palette {
    ($name:literal, $light:expr, $bg:expr, $panel:expr, $card:expr, $border:expr, $text:expr,
     $muted:expr, $dim:expr, $accent:expr, $accent_dark:expr, $error:expr, $ok:expr, $warn:expr) => {
        (
            $name,
            Palette {
                bg: $bg,
                panel: $panel,
                card: $card,
                border: $border,
                text: $text,
                muted: $muted,
                dim: $dim,
                accent: $accent,
                accent_dark: $accent_dark,
                error: $error,
                ok: $ok,
                warn: $warn,
                is_light: $light,
            },
        )
    };
}

/// 终端配色系列：与界面主题的黑白系列同步。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TermSeries {
    Dark,
    Light,
}

impl TermSeries {
    /// 该系列的终端配色记忆键（settings 表）。
    pub fn setting_key(self) -> &'static str {
        match self {
            TermSeries::Dark => "terminal.theme.dark",
            TermSeries::Light => "terminal.theme.light",
        }
    }
}

/// 一套终端配色（u32 RGB）：前景/背景/光标 + 16 ANSI 色 + 所属系列。
///
/// 控制台里"不同对象不同颜色"（提示符、目录、可执行、时间戳……）来自远端
/// 程序输出的 ANSI 转义码，本模板决定各编号映射到的实际 RGB。
pub struct TermPalette {
    pub series: TermSeries,
    pub fg: u32,
    pub bg: u32,
    pub cursor: u32,
    /// 16 ANSI 色：0-7 常规（黑红绿黄蓝洋红青白），8-15 亮色（同序）。
    pub ansi: [u32; 16],
}

impl TermPalette {
    /// 转为 gpui-terminal 的调色板（注入 TerminalConfig.colors），表面效果
    /// 三件套与界面一致：默认背景 压暗+alpha（透窗）；前景/16 ANSI/光标
    /// 向白提亮（补偿被底色抬亮的背景，文字对比度不降）；ANSI 单元格底色
    /// 保持不透明；alpha=1.0+不压暗+不提亮 即原不透明语义。
    pub fn to_gpui_with_frost(
        &self,
        bg_alpha: f32,
        bg_dim: f32,
        fg_lift: f32,
    ) -> gpui_terminal::ColorPalette {
        let rgb = |v: u32| ((v >> 16) as u8, (v >> 8) as u8, v as u8);
        let dim = |c: u8| (c as f32 * bg_dim).round().min(255.0) as u8;
        let lift = |c: u8| (c as f32 * (1.0 - fg_lift) + 255.0 * fg_lift).round().min(255.0) as u8;
        let a = &self.ansi;
        let (r, g, b) = rgb(self.fg);
        let (r, g, b) = (lift(r), lift(g), lift(b));
        let mut p = gpui_terminal::ColorPalette::builder()
            .foreground(r, g, b);
        let (r, g, b) = rgb(self.bg);
        p = p.background(dim(r), dim(g), dim(b));
        let (r, g, b) = rgb(self.cursor);
        let (r, g, b) = (lift(r), lift(g), lift(b));
        p = p.cursor(r, g, b);
        const SLOTS: [(&str, usize); 16] = [
            ("black", 0), ("red", 1), ("green", 2), ("yellow", 3), ("blue", 4),
            ("magenta", 5), ("cyan", 6), ("white", 7), ("bright_black", 8),
            ("bright_red", 9), ("bright_green", 10), ("bright_yellow", 11),
            ("bright_blue", 12), ("bright_magenta", 13), ("bright_cyan", 14),
            ("bright_white", 15),
        ];
        for (name, idx) in SLOTS {
            let (r, g, b) = rgb(a[idx]);
            let (r, g, b) = (lift(r), lift(g), lift(b));
            p = match name {
                "black" => p.black(r, g, b),
                "red" => p.red(r, g, b),
                "green" => p.green(r, g, b),
                "yellow" => p.yellow(r, g, b),
                "blue" => p.blue(r, g, b),
                "magenta" => p.magenta(r, g, b),
                "cyan" => p.cyan(r, g, b),
                "white" => p.white(r, g, b),
                "bright_black" => p.bright_black(r, g, b),
                "bright_red" => p.bright_red(r, g, b),
                "bright_green" => p.bright_green(r, g, b),
                "bright_yellow" => p.bright_yellow(r, g, b),
                "bright_blue" => p.bright_blue(r, g, b),
                "bright_magenta" => p.bright_magenta(r, g, b),
                "bright_cyan" => p.bright_cyan(r, g, b),
                _ => p.bright_white(r, g, b),
            };
        }
        // alpha 必须在 background() 之后设置（后者整体覆写含 a 通道）。
        p.background_alpha(bg_alpha).build()
    }
}

macro_rules! term_palette {
    ($series:ident, $fg:expr, $bg:expr, $cursor:expr, $($c:expr),+ $(,)?) => {
        TermPalette {
            series: TermSeries::$series,
            fg: $fg,
            bg: $bg,
            cursor: $cursor,
            ansi: [$($c),+],
        }
    };
}

/// 内置终端配色模板目录（独立于界面主题；含深/浅两系列）。
///
/// 深色 7 套 + 浅色 4 套，均取自各配色官方发布值。
pub static TERM_THEMES: &[(&str, TermPalette)] = &[
    // Tokyo Night（官方终端配色）
    (
        "Tokyo Night",
        term_palette!(
            Dark, 0xc0caf5, 0x1a1b26, 0xc0caf5, 0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68,
            0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868, 0xff7a93, 0xb9f27c, 0xff9e64,
            0x7da6ff, 0xbb9af7, 0x0db9d7, 0xc0caf5,
        ),
    ),
    // Dracula（官方 dracula/dracula-theme 终端配色）
    (
        "Dracula",
        term_palette!(
            Dark, 0xf8f8f2, 0x282a36, 0xf8f8f2, 0x000000, 0xff5555, 0x50fa7b, 0xf1fa8c,
            0xbd93f9, 0xff79c6, 0x8be9fd, 0xbfbfbf, 0x4d4d4d, 0xff6e67, 0x5af78e, 0xf4f99d,
            0xcaa9fa, 0xff92d0, 0x9aedfe, 0xe6e6e6,
        ),
    ),
    // Nord（nordtheme 官方终端配色）
    (
        "Nord",
        term_palette!(
            Dark, 0xd8dee9, 0x2e3440, 0xd8dee9, 0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b,
            0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b,
            0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ),
    ),
    // Gruvbox Dark（morhetz 官方）
    (
        "Gruvbox Dark",
        term_palette!(
            Dark, 0xebdbb2, 0x282828, 0xebdbb2, 0x282828, 0xcc241d, 0x98971a, 0xd79921,
            0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f,
            0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ),
    ),
    // One Dark（iTerm2 one-dark）
    (
        "One Dark",
        term_palette!(
            Dark, 0xabb2bf, 0x282c34, 0xabb2bf, 0x282c34, 0xe06c75, 0x98c379, 0xe5c07b,
            0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b,
            0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
        ),
    ),
    // Catppuccin Mocha（官方终端配色）
    (
        "Catppuccin Mocha",
        term_palette!(
            Dark, 0xcdd6f4, 0x1e1e2e, 0xf5e0dc, 0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af,
            0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af,
            0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ),
    ),
    // Solarized Dark（官方；深浅两套共用 16 ANSI 色，仅底色/前景互换）
    (
        "Solarized Dark",
        term_palette!(
            Dark, 0x839496, 0x002b36, 0x93a1a1, 0x073642, 0xdc322f, 0x859900, 0xb58900,
            0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83,
            0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ),
    ),
    // One Light（Atom One Light 系 ANSI，白底）
    (
        "One Light",
        term_palette!(
            Light, 0x383a42, 0xffffff, 0x383a42, 0x383a42, 0xe45649, 0x50a14f, 0xc18401,
            0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7, 0xa0a1a7, 0xe45649, 0x50a14f, 0xc18401,
            0x4078f2, 0xa626a4, 0x0184bc, 0x383a42,
        ),
    ),
    // Solarized Light（官方）
    (
        "Solarized Light",
        term_palette!(
            Light, 0x657b83, 0xfdf6e3, 0x586e75, 0x073642, 0xdc322f, 0x859900, 0xb58900,
            0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83,
            0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ),
    ),
    // Catppuccin Latte（官方终端配色）
    (
        "Catppuccin Latte",
        term_palette!(
            Light, 0x4c4f69, 0xeff1f5, 0xdc8a78, 0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d,
            0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, 0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d,
            0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
        ),
    ),
    // Gruvbox Light（morhetz 官方 light 模式）
    (
        "Gruvbox Light",
        term_palette!(
            Light, 0x3c3836, 0xfbf1c7, 0x3c3836, 0xfbf1c7, 0x9d0006, 0x79740e, 0xb57614,
            0x076678, 0x8f3f71, 0x427b58, 0x7c6f64, 0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f,
            0x83a598, 0xd3869b, 0x8ec07c, 0x282828,
        ),
    ),
];

/// 按索引取终端配色（越界回退第 0 套）。
pub fn term_for(idx: usize) -> &'static TermPalette {
    &TERM_THEMES[idx.min(TERM_THEMES.len() - 1)].1
}

/// 系列默认终端配色索引（无记忆时首次使用）。
pub fn term_default(series: TermSeries) -> usize {
    TERM_THEMES
        .iter()
        .position(|(_, p)| p.series == series)
        .unwrap_or(0)
}

/// 界面主题所属系列（与终端配色系列同步的依据）。
pub fn ui_series(idx: usize) -> TermSeries {
    if THEMES[idx.min(THEMES.len() - 1)].1.is_light {
        TermSeries::Light
    } else {
        TermSeries::Dark
    }
}

/// 同系列的全部终端配色（菜单列出当前系列可选项）。
pub fn term_themes_in(
    series: TermSeries,
) -> impl Iterator<Item = (usize, &'static str, &'static TermPalette)> {
    TERM_THEMES
        .iter()
        .enumerate()
        .filter(move |(_, (_, p))| p.series == series)
        .map(|(i, (n, p))| (i, *n, p))
}

/// 内置界面主题（索引持久化；is_light 决定所属系列，驱动终端配色联动）。
pub static THEMES: &[(&str, Palette)] = &[
    palette!(
        "Tokyo Night", false, 0x1a1b26, 0x1f2335, 0x24283b, 0x414868, 0xc0caf5, 0x8189af,
        0x565f89, 0x7aa2f7, 0x1a1b26, 0xf7768e, 0x9ece6a, 0xe0af68
    ),
    palette!(
        "Nord", false, 0x2e3440, 0x3b4252, 0x434c5e, 0x4c566a, 0xeceff4, 0x9fa6b8,
        0x697389, 0x88c0d0, 0x2e3440, 0xbf616a, 0xa3be8c, 0xebcb8b
    ),
    palette!(
        "Gruvbox Dark", false, 0x282828, 0x32302f, 0x3c3836, 0x504945, 0xebdbb2, 0xbdae93,
        0x7c6f64, 0xfabd2f, 0x282828, 0xfb4934, 0xb8bb26, 0xfe8019
    ),
    palette!(
        "One Dark", false, 0x282c34, 0x21252b, 0x2c313a, 0x3e4451, 0xabb2bf, 0x828997,
        0x5c6370, 0x61afef, 0x282c34, 0xe06c75, 0x98c379, 0xe5c07b
    ),
    palette!(
        "Catppuccin Mocha", false, 0x1e1e2e, 0x181825, 0x313244, 0x45475a, 0xcdd6f4, 0xa6adc8,
        0x6c7086, 0x89b4fa, 0x1e1e2e, 0xf38ba8, 0xa6e3a1, 0xf9e2af
    ),
    palette!(
        "macOS Light", true, 0xffffff, 0xf0f0f5, 0xe4e4ea, 0xc7c7d1, 0x1d1d1f, 0x6e6e73,
        0xa0a0a8, 0x0a7cff, 0xffffff, 0xe0405e, 0x2ea043, 0xd29922
    ),
];

/// 标题栏高度：与 macOS 交通灯同排。
pub const TITLEBAR_H: f32 = 38.0;
/// macOS 交通灯占用的左侧安全宽度。
pub const TRAFFIC_LIGHTS_W: f32 = 78.0;

/// tab 条外边距（贴中部区两缘；tab 之间的 gap_1 不变）。
pub const TAB_STRIP_EDGE: f32 = 8.0;
/// tab 条溢出时左右移动按钮的宽度。
pub const TAB_ARROW_W: f32 = 22.0;
/// 点击移动按钮的单步滚动量（px）。
pub const TAB_SCROLL_STEP: f32 = 120.0;

/// 左侧服务器面板宽度 / 右侧状态面板默认宽度。
pub const LEFT_W: f32 = 232.0;
pub const RIGHT_W: f32 = 280.0;

/// 右缘全高工具栏宽度（常驻，承载左右面板开合按钮）。
pub const RIGHT_TOOLBAR_W: f32 = 40.0;

/// 侧栏宽度可拖拽范围（右缘拖柄）。
pub const SIDEBAR_MIN_W: f32 = 180.0;
pub const SIDEBAR_MAX_W: f32 = 420.0;

/// 右侧状态面板宽度可拖拽范围（左缘拖柄）。
pub const RIGHT_MIN_W: f32 = 200.0;
pub const RIGHT_MAX_W: f32 = 480.0;

/// 侧栏底部工具栏高度（定高贴底）。
pub const SIDEBAR_TOOLBAR_H: f32 = 38.0;
/// 树型区与快捷栏之间的分割线热区厚度（视觉线 1px，其余为拖拽命中区）。
pub const SIDEBAR_SPLITTER_H: f32 = 7.0;
/// 快捷栏高度范围与默认值（分割线可上下调整）。
/// 上限动态 = 窗口高 - 顶部让位 - 工具栏 - 分割线 - 树最小高（见 sidebar.rs）。
pub const SHORTCUT_MIN_H: f32 = 56.0;
pub const SHORTCUT_DEFAULT_H: f32 = 110.0;
/// 树区最小高度：固定值，不随窗口尺寸变化。
pub const TREE_MIN_H: f32 = 80.0;

/// 右键菜单宽度。
/// 菜单项高度。

/// 人性化字节量。
pub fn fmt_bytes(n: u64) -> String {
    let f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} K", f / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} M", f / 1024.0 / 1024.0)
    } else {
        format!("{:.1} G", f / 1024.0 / 1024.0 / 1024.0)
    }
}

/// 人性化时长（弹出窗会话状态用；中文分/秒，英文紧凑 m/s 记法）。
pub fn fmt_duration(loc: crate::i18n::Locale, secs: u64) -> String {
    if secs < 60 {
        crate::i18n::tlf(loc, crate::i18n::Tr::DurSecs, &[&secs.to_string()])
    } else if secs < 3600 {
        crate::i18n::tlf(loc, crate::i18n::Tr::DurMinSec, &[&(secs / 60).to_string(), &(secs % 60).to_string()])
    } else {
        crate::i18n::tlf(loc, crate::i18n::Tr::DurHourMin, &[&(secs / 3600).to_string(), &((secs % 3600) / 60).to_string()])
    }
}

/// 连接日志的相对时间戳：`+85ms` / `+1.4s`。
pub fn fmt_step_ms(ms: u32) -> String {
    if ms >= 1000 {
        format!("+{}.{:01}s", ms / 1000, (ms % 1000) / 100)
    } else {
        format!("+{ms}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表面参数推导：深色系（无论模糊开关）= 毛玻璃三件套；亮色系 = 90% 原色。
    #[test]
    fn bg_fx_dark_frost_params() {
        for blur in [true, false] {
            assert_eq!(
                BgFx { blur }.fx(false),
                SurfaceFx { alpha: BG_FROST_ALPHA, dim: BG_FROST_DIM, lift: BG_FROST_LIFT }
            );
            assert_eq!(
                BgFx { blur }.fx(true),
                SurfaceFx { alpha: BG_SOLID_ALPHA, dim: 1.0, lift: 0.0 }
            );
        }
    }

    /// 白底可读性（文字提亮补偿）：最坏情况（纯白底）下，提亮后的灰色文字
    /// 对比度不低于不透明模式基线的 95%（线性空间近似）。
    #[test]
    fn bg_fx_white_backdrop_readable() {
        let fx = BgFx { blur: true }.fx(false);
        let bg = 0x1a as f32 / 255.0; // Tokyo bg r 通道
        let dim_c = 0x56 as f32 / 255.0; // Tokyo dim（最暗的次要文字）r 通道
        let worst_bg = fx.alpha * (bg * fx.dim) + (1.0 - fx.alpha) * 1.0;
        let lifted = dim_c * (1.0 - fx.lift) + fx.lift;
        let lin = |c: f32| c * c; // gamma≈2 近似
        let plain = (lin(dim_c) + 0.05) / (lin(bg) + 0.05);
        let frosted = (lin(lifted) + 0.05) / (lin(worst_bg) + 0.05);
        assert!(
            frosted >= plain * 0.95,
            "提亮后对比 {frosted:.2} 低于基线 95%（{plain:.2}）"
        );
        // PalFx 烘焙验证：dim 被提亮、bg/panel/card/accent_dark 不动。
        let pal = PalFx::build(&THEMES[0].1, fx);
        assert!(pal.dim > THEMES[0].1.dim, "dim 应被提亮");
        // 主文字的"到白距离"缩短比例应大于次要文字（对齐终端近白前景；
        // 不能比绝对增量——提亮对暗色天然增益更大）。
        let dist_white = |v: u32| 255 - (v >> 16 & 0xff) as f32 as i32;
        let text_ratio =
            dist_white(pal.text) as f32 / dist_white(THEMES[0].1.text) as f32;
        assert!(
            text_ratio <= 1.0 - BG_FROST_LIFT - 0.15,
            "主文字提亮幅度应更高（ratio={text_ratio:.2}）"
        );
        assert_eq!(pal.bg, THEMES[0].1.bg, "表面色不被提亮（surface() 负责压暗）");
        assert_eq!(pal.accent_dark, THEMES[0].1.accent_dark);
        // 不透明档零变化。
        let opaque = PalFx::build(&THEMES[0].1, SurfaceFx::OPAQUE);
        assert_eq!(opaque.dim, THEMES[0].1.dim);
    }

    /// 表面换算与 surface() 着色：RGB 分量不被移位破坏、alpha/压暗正确注入。
    #[test]
    fn bg_fx_alpha_and_surface() {
        let frost = PalFx::build(
            &THEMES[0].1,
            SurfaceFx { alpha: BG_FROST_ALPHA, dim: BG_FROST_DIM, lift: 0.0 },
        );
        // 压暗机制：白 × 2/3 = 0xaa。
        let w = frost.surface(0xffffff);
        assert_eq!(w.r, 0xaa as f32 / 255.0);
        assert_eq!(frost.kit().surface_dim, (BG_FROST_DIM * 255.0) as u8);
        assert_eq!(frost.kit().surface_alpha, (BG_FROST_ALPHA * 255.0).round() as u8);
        // 不压暗路径：颜色原样、alpha 注入。
        let pal = PalFx::build(
            &THEMES[0].1,
            SurfaceFx { alpha: 0.5, dim: 1.0, lift: 0.0 },
        );
        let c = pal.surface(pal.bg);
        assert_eq!(c.a, 0x80 as f32 / 255.0);
        assert_eq!(c.r, 0x1a as f32 / 255.0);
    }

    /// 窗口背景模式：模糊→Blurred、关→Transparent（不透明度只管表面色）。
    #[test]
    fn bg_fx_window_appearance() {
        assert!(matches!(
            BgFx { blur: true }.window_appearance(),
            WindowBackgroundAppearance::Blurred
        ));
        assert!(matches!(
            BgFx { blur: false }.window_appearance(),
            WindowBackgroundAppearance::Transparent
        ));
    }

    /// 终端配色带三件套：背景压暗+alpha、前景提亮、不提亮即原语义。
    #[test]
    fn term_palette_bg_alpha() {
        let plain = term_for(0).to_gpui_with_frost(1.0, 1.0, 0.0);
        assert_eq!(plain.background().a, 1.0);
        assert_eq!(plain.foreground().l, term_for(0).to_gpui_with_frost(1.0, 1.0, 0.0).foreground().l);
        let frost = term_for(0).to_gpui_with_frost(0.75, 2.0 / 3.0, 0.35);
        assert!((frost.background().a - 0.75).abs() < 1e-6);
        // 压暗作用于默认背景（与 lift 无关，亮度等于显式压暗版）；
        // 提亮作用于前景（亮度上升）。
        let dimmed = term_for(0).to_gpui_with_frost(1.0, 2.0 / 3.0, 0.0);
        assert!((frost.background().l - dimmed.background().l).abs() < 1e-6);
        assert!(frost.background().l < plain.background().l);
        assert!(frost.foreground().l > plain.foreground().l);
    }

    /// 每个系列至少一套配色，且系列默认值确属该系列。
    #[test]
    fn term_series_nonempty() {
        for series in [TermSeries::Dark, TermSeries::Light] {
            let n = term_themes_in(series).count();
            assert!(n > 0, "{:?} 系列为空", series);
            let (idx, _, p) = term_themes_in(series).next().unwrap();
            assert_eq!(idx, term_default(series));
            assert_eq!(p.series, series);
        }
    }

    /// 系列标记必须与背景亮度一致（联动切换时终端不会黑底配白界面或反之）。
    #[test]
    fn term_series_matches_bg_luminance() {
        for (name, p) in TERM_THEMES {
            let l = p.to_gpui_with_frost(1.0, 1.0, 0.0).background().l;
            if p.series == TermSeries::Light {
                assert!(l > 0.5, "{name} 标记浅色但背景偏深");
            } else {
                assert!(l < 0.5, "{name} 标记深色但背景偏浅");
            }
        }
    }

    /// 界面主题系列标记同样校验（macOS Light 浅色、其余深色）。
    #[test]
    fn ui_series_flags_consistent() {
        let lights = THEMES.iter().filter(|(_, p)| p.is_light).count();
        assert_eq!(lights, 1, "应有且仅有一套浅色界面主题");
        assert_eq!(ui_series(THEMES.len() - 1), TermSeries::Light);
        assert_eq!(ui_series(0), TermSeries::Dark);
    }

    /// to_gpui 必须把 16 ANSI 全量映射进调色板（builder 会同步前 16 个扩展色）。
    #[test]
    fn to_gpui_maps_all_slots() {
        let p = term_for(0).to_gpui_with_frost(1.0, 1.0, 0.0);
        assert_eq!(p.ansi_colors().len(), 16);
        assert_eq!(p.extended_colors().len(), 256);
        // 黑色与亮黑不同色，防止宏参数错位后静默全同。
        assert_ne!(p.ansi_colors()[0], p.ansi_colors()[8]);
    }
}
