//! Color palette for terminal emulator.
//!
//! This module provides [`ColorPalette`] and [`ColorPaletteBuilder`] for managing
//! terminal colors. It supports:
//!
//! - **16 ANSI colors**: The standard palette (colors 0-15)
//! - **256-color mode**: Extended palette with 6×6×6 RGB cube and grayscale ramp
//! - **True color (24-bit RGB)**: Direct RGB color specification
//!
//! # Force-Theme Quantization
//!
//! All colors are **forced through the palette**: 256-indexed colors (16-255)
//! and true-color (24-bit RGB) values are quantized to the closest palette
//! color at build/resolve time — chromatic colors snap to the nearest of the
//! 16 themed ANSI colors (black excluded, as it usually matches the
//! background), and grays are remapped onto a lightness ramp between the
//! theme's background and foreground. Runtime palette overrides sent by
//! programs (OSC 4/10/11) are ignored. The rendered result therefore depends
//! only on the configured palette, never on what the remote side emits.
//!
//! # Default ANSI Colors
//!
//! The default palette uses colors similar to common terminal emulators:
//!
//! | Index | Name | RGB |
//! |-------|------|-----|
//! | 0 | Black | `#000000` |
//! | 1 | Red | `#CC0000` |
//! | 2 | Green | `#4E9A06` |
//! | 3 | Yellow | `#C4A000` |
//! | 4 | Blue | `#3465A4` |
//! | 5 | Magenta | `#75507B` |
//! | 6 | Cyan | `#06989A` |
//! | 7 | White | `#D3D7CF` |
//! | 8 | Bright Black | `#555753` |
//! | 9 | Bright Red | `#EF2929` |
//! | 10 | Bright Green | `#8AE234` |
//! | 11 | Bright Yellow | `#FCE94F` |
//! | 12 | Bright Blue | `#729FCF` |
//! | 13 | Bright Magenta | `#AD7FA8` |
//! | 14 | Bright Cyan | `#34E2E2` |
//! | 15 | Bright White | `#EEEEEC` |
//!
//! # 256-Color Mode
//!
//! Colors 16-255 are calculated:
//!
//! - **16-231**: 6×6×6 RGB cube where each component is `0, 95, 135, 175, 215, 255`
//! - **232-255**: 24-step grayscale from `#080808` to `#EEEEEE`
//!
//! # Example
//!
//! ```
//! use gpui_terminal::ColorPalette;
//!
//! // Use default palette
//! let default = ColorPalette::default();
//!
//! // Or customize with builder
//! let custom = ColorPalette::builder()
//!     .background(0x1a, 0x1b, 0x26)  // Tokyo Night background
//!     .foreground(0xa9, 0xb1, 0xd6)  // Tokyo Night foreground
//!     .red(0xf7, 0x76, 0x8e)
//!     .green(0x9e, 0xce, 0x6a)
//!     .blue(0x7a, 0xa2, 0xf7)
//!     .build();
//! ```

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui::Hsla;

/// A color palette that maps ANSI colors to GPUI Hsla colors.
///
/// This struct maintains the 16-color ANSI palette, 256-color extended palette,
/// and special colors (foreground, background, cursor). It provides the
/// [`resolve`](Self::resolve) method to convert any terminal color to GPUI's
/// [`Hsla`] format for rendering.
///
/// # Color Resolution
///
/// The [`resolve`](Self::resolve) method handles all terminal color types:
///
/// 1. **Named colors** (0-15): Looked up in the ANSI palette
/// 2. **Special colors**: Foreground, Background, Cursor, Dim variants
/// 3. **Indexed colors** (16-255): Looked up in the extended palette
/// 4. **True colors**: Converted directly from RGB
///
/// # Creating a Palette
///
/// Use [`ColorPalette::default()`] for standard colors, or
/// [`ColorPalette::builder()`] for customization:
///
/// ```
/// use gpui_terminal::ColorPalette;
///
/// let palette = ColorPalette::builder()
///     .background(0x28, 0x28, 0x28)
///     .foreground(0xeb, 0xdb, 0xb2)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct ColorPalette {
    /// The 16 standard ANSI colors (black, red, green, yellow, blue, magenta, cyan, white,
    /// and their bright variants)
    ansi_colors: [Hsla; 16],

    /// Exact RGB values of the 16 ANSI colors, kept for quantization distance
    /// math (Hsla round-trips are lossy).
    ansi_rgb: [Rgb; 16],

    /// The 256-color palette (colors 16-255)
    /// Colors 0-15 are the standard ANSI colors
    /// Colors 16-255 are the standard reference values quantized through
    /// [`ColorPalette::themed`] (force-theme)
    extended_colors: [Hsla; 256],

    /// Default foreground color
    foreground: Hsla,

    /// Default background color
    background: Hsla,

    /// Default cursor color
    cursor: Hsla,
}

/// Standard reference RGB value of an extended (256-color) index.
///
/// Only defined for indices 16-255: the 6×6×6 cube (16-231) and the
/// 24-step grayscale ramp (232-255).
fn standard_indexed_rgb(idx: u8) -> Rgb {
    debug_assert!((16..=255).contains(&idx));
    match idx {
        232..=255 => {
            let gray = 8 + (idx - 232) * 10;
            Rgb { r: gray, g: gray, b: gray }
        }
        _ => {
            let i = (idx - 16) as u8;
            let component = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb {
                r: component(i / 36),
                g: component((i / 6) % 6),
                b: component(i % 6),
            }
        }
    }
}

/// Linear interpolation between two Hsla colors (components independently).
fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    Hsla {
        h: a.h + (b.h - a.h) * t,
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Saturation below which an incoming color is treated as a gray.
const GRAY_SATURATION_THRESHOLD: f32 = 0.12;

impl Default for ColorPalette {
    fn default() -> Self {
        // Standard ANSI colors (based on common terminal emulator defaults)
        let ansi_rgb = [
            // Normal colors
            Rgb { r: 0x00, g: 0x00, b: 0x00 }, // Black
            Rgb { r: 0xcc, g: 0x00, b: 0x00 }, // Red
            Rgb { r: 0x4e, g: 0x9a, b: 0x06 }, // Green
            Rgb { r: 0xc4, g: 0xa0, b: 0x00 }, // Yellow
            Rgb { r: 0x34, g: 0x65, b: 0xa4 }, // Blue
            Rgb { r: 0x75, g: 0x50, b: 0x7b }, // Magenta
            Rgb { r: 0x06, g: 0x98, b: 0x9a }, // Cyan
            Rgb { r: 0xd3, g: 0xd7, b: 0xcf }, // White
            // Bright colors
            Rgb { r: 0x55, g: 0x57, b: 0x53 }, // Bright Black (Gray)
            Rgb { r: 0xef, g: 0x29, b: 0x29 }, // Bright Red
            Rgb { r: 0x8a, g: 0xe2, b: 0x34 }, // Bright Green
            Rgb { r: 0xfc, g: 0xe9, b: 0x4f }, // Bright Yellow
            Rgb { r: 0x72, g: 0x9f, b: 0xcf }, // Bright Blue
            Rgb { r: 0xad, g: 0x7f, b: 0xa8 }, // Bright Magenta
            Rgb { r: 0x34, g: 0xe2, b: 0xe2 }, // Bright Cyan
            Rgb { r: 0xee, g: 0xee, b: 0xec }, // Bright White
        ];
        let ansi_colors = ansi_rgb.map(rgb_to_hsla);

        // Default terminal colors
        let foreground = rgb_to_hsla(Rgb {
            r: 0xd4,
            g: 0xd4,
            b: 0xd4,
        }); // Light gray
        let background = rgb_to_hsla(Rgb {
            r: 0x1e,
            g: 0x1e,
            b: 0x1e,
        }); // Dark gray
        let cursor = rgb_to_hsla(Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        }); // White

        let mut palette = Self {
            ansi_colors,
            ansi_rgb,
            extended_colors: [Hsla::default(); 256],
            foreground,
            background,
            cursor,
        };
        // First 16: the standard ANSI colors; 16-255: standard reference
        // values quantized through the force-theme mapping.
        palette.extended_colors[0..16].copy_from_slice(&ansi_colors);
        for idx in 16u8..=255 {
            palette.extended_colors[idx as usize] = palette.themed(standard_indexed_rgb(idx));
        }
        palette
    }
}

impl ColorPalette {
    /// Creates a new color palette with default colors.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new color palette builder for customizing colors.
    ///
    /// # Example
    ///
    /// ```
    /// use gpui_terminal::ColorPalette;
    ///
    /// let palette = ColorPalette::builder()
    ///     .background(0x16, 0x16, 0x17)
    ///     .foreground(0xC9, 0xC7, 0xCD)
    ///     .black(0x10, 0x10, 0x10)
    ///     .red(0xEF, 0xA6, 0xA2)
    ///     .build();
    /// ```
    pub fn builder() -> ColorPaletteBuilder {
        ColorPaletteBuilder::new()
    }

    /// Resolves a terminal color to a GPUI Hsla color.
    ///
    /// All color sources are forced through this palette (force-theme):
    /// true-color (24-bit RGB) and 256-indexed values are quantized via
    /// [`ColorPalette::themed`], and runtime palette overrides sent by
    /// programs (OSC 4/10/11, carried in `colors`) are ignored — the
    /// rendered colors depend only on the configured palette.
    ///
    /// # Arguments
    ///
    /// * `color` - The terminal color to resolve
    /// * `_colors` - Runtime color overrides from the terminal state; ignored
    ///
    /// # Returns
    ///
    /// The resolved Hsla color suitable for use with GPUI
    pub fn resolve(&self, color: Color, _colors: &Colors) -> Hsla {
        match color {
            Color::Named(named) => {
                // Handle different named color types
                let idx = named as usize;
                if idx < 16 {
                    // Standard ANSI colors (0-15)
                    self.ansi_colors[idx]
                } else {
                    // Special colors (Foreground, Background, Cursor, etc.)
                    match named {
                        NamedColor::Foreground => self.foreground,
                        NamedColor::Background => self.background,
                        NamedColor::Cursor => self.cursor,
                        NamedColor::DimForeground => {
                            // Dimmed version of foreground
                            let mut dim = self.foreground;
                            dim.l *= 0.7;
                            dim
                        }
                        NamedColor::BrightForeground => {
                            // Brighter version of foreground
                            let mut bright = self.foreground;
                            bright.l = (bright.l * 1.2).min(1.0);
                            bright
                        }
                        NamedColor::DimBlack
                        | NamedColor::DimRed
                        | NamedColor::DimGreen
                        | NamedColor::DimYellow
                        | NamedColor::DimBlue
                        | NamedColor::DimMagenta
                        | NamedColor::DimCyan
                        | NamedColor::DimWhite => {
                            // Dim variant - calculate base color index and dim it
                            let base_idx = match named {
                                NamedColor::DimBlack => 0,
                                NamedColor::DimRed => 1,
                                NamedColor::DimGreen => 2,
                                NamedColor::DimYellow => 3,
                                NamedColor::DimBlue => 4,
                                NamedColor::DimMagenta => 5,
                                NamedColor::DimCyan => 6,
                                NamedColor::DimWhite => 7,
                                _ => 7,
                            };
                            let mut dim = self.ansi_colors[base_idx];
                            dim.l *= 0.7;
                            dim
                        }
                        _ => self.foreground, // Fallback for any other special colors
                    }
                }
            }
            Color::Spec(rgb) => {
                // True color (24-bit RGB): quantized to the palette
                self.themed(rgb)
            }
            Color::Indexed(idx) => {
                // 256-color mode: 0-15 are the themed ANSI colors, 16-255 hold
                // the force-theme quantized reference values
                self.extended_colors[idx as usize]
            }
        }
    }

    /// Quantizes an arbitrary RGB color onto this palette (force-theme).
    ///
    /// - Chromatic colors snap to the nearest of the 16 themed ANSI colors by
    ///   weighted RGB distance. Black (index 0) is excluded from the
    ///   candidates: it usually matches the background, so snapping text to
    ///   it would make the glyph invisible.
    /// - Grays (saturation below [`GRAY_SATURATION_THRESHOLD`]) keep their
    ///   relative lightness but are remapped onto a ramp between the theme's
    ///   background and foreground, so shading structure (e.g. vim comment
    ///   hierarchies) survives while the tint follows the theme.
    pub fn themed(&self, rgb: Rgb) -> Hsla {
        let incoming = rgb_to_hsla(rgb);
        if incoming.s < GRAY_SATURATION_THRESHOLD {
            // 文字色必须不透明：background 可能带窗口透明 alpha，灰阶映射后钉回 1。
            let mut c = lerp_hsla(self.background, self.foreground, incoming.l.clamp(0.0, 1.0));
            c.a = 1.0;
            return c;
        }

        let mut best = self.ansi_colors[1];
        let mut best_dist = f32::MAX;
        for i in 1..16 {
            let cand = self.ansi_rgb[i];
            let dr = (rgb.r as f32 - cand.r as f32) * 2.0;
            let dg = (rgb.g as f32 - cand.g as f32) * 4.0;
            let db = (rgb.b as f32 - cand.b as f32) * 3.0;
            let dist = dr * dr + dg * dg + db * db;
            if dist < best_dist {
                best_dist = dist;
                best = self.ansi_colors[i];
            }
        }
        best
    }

    /// Gets a reference to the ANSI color palette.
    pub fn ansi_colors(&self) -> &[Hsla; 16] {
        &self.ansi_colors
    }

    /// Gets a reference to the full 256-color palette.
    pub fn extended_colors(&self) -> &[Hsla; 256] {
        &self.extended_colors
    }

    /// Gets the default foreground color.
    pub fn foreground(&self) -> Hsla {
        self.foreground
    }

    /// Gets the default background color.
    pub fn background(&self) -> Hsla {
        self.background
    }

    /// Gets the default cursor color.
    pub fn cursor(&self) -> Hsla {
        self.cursor
    }
}

/// Converts an RGB color to GPUI's Hsla color format.
///
/// # Arguments
///
/// * `rgb` - The RGB color to convert (each component is 0-255)
///
/// # Returns
///
/// The converted Hsla color with full opacity
fn rgb_to_hsla(rgb: Rgb) -> Hsla {
    // Normalize RGB values to 0.0-1.0 range
    let r = rgb.r as f32 / 255.0;
    let g = rgb.g as f32 / 255.0;
    let b = rgb.b as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    // Calculate lightness
    let l = (max + min) / 2.0;

    // Calculate saturation
    let s = if delta == 0.0 {
        0.0
    } else {
        delta / (1.0 - (2.0 * l - 1.0).abs())
    };

    // Calculate hue
    let h = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    // Normalize hue to 0.0-1.0 range (GPUI uses normalized values)
    let h = if h < 0.0 { h + 360.0 } else { h } / 360.0;

    Hsla {
        h,
        s,
        l,
        a: 1.0, // Full opacity
    }
}

/// Builder for creating a customized color palette.
///
/// Start with default colors and override specific ones as needed.
#[derive(Debug, Clone)]
pub struct ColorPaletteBuilder {
    palette: ColorPalette,
}

impl Default for ColorPaletteBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorPaletteBuilder {
    /// Creates a new builder with default colors.
    pub fn new() -> Self {
        Self {
            palette: ColorPalette::default(),
        }
    }

    /// Sets the background color.
    pub fn background(mut self, r: u8, g: u8, b: u8) -> Self {
        self.palette.background = rgb_to_hsla(Rgb { r, g, b });
        self
    }

    /// Sets the background alpha (host window translucency; 1.0 = opaque).
    /// Only the default background quad carries it — ANSI cell backgrounds,
    /// text and cursor stay opaque.
    pub fn background_alpha(mut self, a: f32) -> Self {
        self.palette.background.a = a.clamp(0.0, 1.0);
        self
    }

    /// Sets the foreground color.
    pub fn foreground(mut self, r: u8, g: u8, b: u8) -> Self {
        self.palette.foreground = rgb_to_hsla(Rgb { r, g, b });
        self
    }

    /// Sets the cursor color.
    pub fn cursor(mut self, r: u8, g: u8, b: u8) -> Self {
        self.palette.cursor = rgb_to_hsla(Rgb { r, g, b });
        self
    }

    /// Sets color 0 (black).
    pub fn black(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(0, r, g, b);
        self
    }

    /// Sets color 1 (red).
    pub fn red(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(1, r, g, b);
        self
    }

    /// Sets color 2 (green).
    pub fn green(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(2, r, g, b);
        self
    }

    /// Sets color 3 (yellow).
    pub fn yellow(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(3, r, g, b);
        self
    }

    /// Sets color 4 (blue).
    pub fn blue(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(4, r, g, b);
        self
    }

    /// Sets color 5 (magenta).
    pub fn magenta(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(5, r, g, b);
        self
    }

    /// Sets color 6 (cyan).
    pub fn cyan(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(6, r, g, b);
        self
    }

    /// Sets color 7 (white).
    pub fn white(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(7, r, g, b);
        self
    }

    /// Sets color 8 (bright black).
    pub fn bright_black(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(8, r, g, b);
        self
    }

    /// Sets color 9 (bright red).
    pub fn bright_red(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(9, r, g, b);
        self
    }

    /// Sets color 10 (bright green).
    pub fn bright_green(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(10, r, g, b);
        self
    }

    /// Sets color 11 (bright yellow).
    pub fn bright_yellow(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(11, r, g, b);
        self
    }

    /// Sets color 12 (bright blue).
    pub fn bright_blue(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(12, r, g, b);
        self
    }

    /// Sets color 13 (bright magenta).
    pub fn bright_magenta(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(13, r, g, b);
        self
    }

    /// Sets color 14 (bright cyan).
    pub fn bright_cyan(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(14, r, g, b);
        self
    }

    /// Sets color 15 (bright white).
    pub fn bright_white(mut self, r: u8, g: u8, b: u8) -> Self {
        self.set_ansi_color(15, r, g, b);
        self
    }

    /// Sets an ANSI color by index (0-15).
    fn set_ansi_color(&mut self, idx: usize, r: u8, g: u8, b: u8) {
        let rgb = Rgb { r, g, b };
        let color = rgb_to_hsla(rgb);
        self.palette.ansi_rgb[idx] = rgb;
        self.palette.ansi_colors[idx] = color;
        self.palette.extended_colors[idx] = color;
    }

    /// Builds the color palette.
    ///
    /// Recomputes the force-theme quantization of the extended (256-color)
    /// entries so they follow any ANSI/foreground/background overrides.
    pub fn build(self) -> ColorPalette {
        let mut palette = self.palette;
        for idx in 16u8..=255 {
            palette.extended_colors[idx as usize] = palette.themed(standard_indexed_rgb(idx));
        }
        palette
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Hsla, b: Hsla) -> bool {
        (a.h - b.h).abs() < 1e-3
            && (a.s - b.s).abs() < 1e-3
            && (a.l - b.l).abs() < 1e-3
            && (a.a - b.a).abs() < 1e-3
    }

    #[test]
    fn test_rgb_to_hsla_black() {
        let rgb = Rgb { r: 0, g: 0, b: 0 };
        let hsla = rgb_to_hsla(rgb);
        assert_eq!(hsla.l, 0.0);
        assert_eq!(hsla.s, 0.0);
        assert_eq!(hsla.a, 1.0);
    }

    #[test]
    fn test_rgb_to_hsla_white() {
        let rgb = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let hsla = rgb_to_hsla(rgb);
        assert_eq!(hsla.l, 1.0);
        assert_eq!(hsla.s, 0.0);
        assert_eq!(hsla.a, 1.0);
    }

    #[test]
    fn test_rgb_to_hsla_red() {
        let rgb = Rgb { r: 255, g: 0, b: 0 };
        let hsla = rgb_to_hsla(rgb);
        assert_eq!(hsla.h, 0.0);
        assert_eq!(hsla.s, 1.0);
        assert_eq!(hsla.a, 1.0);
    }

    #[test]
    fn test_color_palette_default() {
        let palette = ColorPalette::default();
        assert_eq!(palette.ansi_colors.len(), 16);
        assert_eq!(palette.extended_colors.len(), 256);
    }

    #[test]
    fn test_resolve_named_color() {
        use alacritty_terminal::vte::ansi::NamedColor;

        let palette = ColorPalette::new();
        let colors = Colors::default();
        let hsla = palette.resolve(Color::Named(NamedColor::Red), &colors);
        assert!(hsla.a > 0.0); // Should have some opacity
    }

    #[test]
    fn test_resolve_indexed_color() {
        let palette = ColorPalette::new();
        let colors = Colors::default();
        let hsla = palette.resolve(Color::Indexed(42), &colors);
        assert_eq!(hsla.a, 1.0);
    }

    #[test]
    fn test_resolve_spec_color() {
        let palette = ColorPalette::new();
        let colors = Colors::default();
        let rgb = Rgb {
            r: 128,
            g: 64,
            b: 192,
        };
        let hsla = palette.resolve(Color::Spec(rgb), &colors);
        assert_eq!(hsla.a, 1.0);
    }

    /// 真彩色必须量化到 16 主题 ANSI 之一，不允许原样直出。
    #[test]
    fn test_truecolor_quantized_to_theme_ansi() {
        let palette = ColorPalette::new();
        let colors = Colors::default();
        let resolved = palette.resolve(Color::Spec(Rgb { r: 255, g: 0, b: 0 }), &colors);
        assert!(
            palette.ansi_colors[1..].iter().any(|c| close(*c, resolved)),
            "truecolor red must map to one of the themed ANSI colors"
        );
        // 且不得是原始 #ff0000 的直出值
        assert!(!close(resolved, rgb_to_hsla(Rgb { r: 255, g: 0, b: 0 })));
    }

    /// 256 索引（16-255）与同值的真彩走同一量化结果，且随调色板变化。
    #[test]
    fn test_indexed_high_follows_truecolor_and_theme() {
        let colors = Colors::default();
        let p1 = ColorPalette::builder()
            .background(0x1a, 0x1b, 0x26)
            .foreground(0xc0, 0xca, 0xf5)
            .red(0xf7, 0x76, 0x8e)
            .build();
        let p2 = ColorPalette::builder()
            .background(0x28, 0x28, 0x28)
            .foreground(0xeb, 0xdb, 0xb2)
            .red(0xcc, 0x24, 0x1d)
            .build();
        // 立方体 196 号参考值恰为 (255,0,0)
        assert_eq!(standard_indexed_rgb(196), Rgb { r: 255, g: 0, b: 0 });
        let via_index = p1.resolve(Color::Indexed(196), &colors);
        let via_spec = p1.resolve(Color::Spec(Rgb { r: 255, g: 0, b: 0 }), &colors);
        assert!(close(via_index, via_spec));
        // 不同调色板给出不同结果（跟主题走）
        assert!(!close(via_index, p2.resolve(Color::Indexed(196), &colors)));
    }

    /// 灰阶按亮度映射进主题 bg→fg 区间：黑=bg、白=fg、中间取中点。
    #[test]
    fn test_gray_ramps_between_bg_and_fg() {
        let p = ColorPalette::builder()
            .background(0x1a, 0x1b, 0x26)
            .foreground(0xc0, 0xca, 0xf5)
            .build();
        let colors = Colors::default();
        let black = p.resolve(Color::Spec(Rgb { r: 0, g: 0, b: 0 }), &colors);
        let white = p.resolve(Color::Spec(Rgb { r: 255, g: 255, b: 255 }), &colors);
        let mid = p.resolve(Color::Spec(Rgb { r: 128, g: 128, b: 128 }), &colors);
        assert!(close(black, p.background()));
        assert!(close(white, p.foreground()));
        assert!(mid.l > p.background().l && mid.l < p.foreground().l);
    }

    /// 远端 OSC 运行时改调色板（Colors 覆盖）必须被忽略——颜色只归 app。
    #[test]
    fn test_runtime_overrides_ignored() {
        use alacritty_terminal::vte::ansi::NamedColor;

        let palette = ColorPalette::new();
        let mut colors = Colors::default();
        colors[NamedColor::Red] = Some(Rgb { r: 0x00, g: 0xff, b: 0x00 });
        let resolved = palette.resolve(Color::Named(NamedColor::Red), &colors);
        assert!(close(resolved, palette.ansi_colors[1]));
    }

    /// build() 后扩展色已按最终调色板量化（builder 覆盖 ANSI 后随候选变化）。
    #[test]
    fn test_builder_rebuilds_extended() {
        let p = ColorPalette::builder()
            .background(0x1a, 0x1b, 0x26)
            .foreground(0xc0, 0xca, 0xf5)
            .red(0xf7, 0x76, 0x8e)
            .build();
        let colors = Colors::default();
        let idx196 = p.resolve(Color::Indexed(196), &colors);
        // 196=(255,0,0) 应落到红色族（红或亮红），而不是绿/蓝
        assert!(
            close(idx196, p.ansi_colors[1]) || close(idx196, p.ansi_colors[9]),
            "cube red must snap to theme red family"
        );
    }
}
