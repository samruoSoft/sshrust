//! 组件库主题：12 个颜色槽位（0xRRGGBB），Copy 语义。
//! 宿主应用负责从自己的调色板转换；组件只读不存。

/// 组件配色。字段名与 sshrust 的 `theme::Palette` 一致，转换是纯字段拷贝。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// 内容区底色
    pub bg: u32,
    /// 面板/侧栏底色
    pub panel: u32,
    /// 卡片/浮层底色
    pub card: u32,
    /// 描边
    pub border: u32,
    /// 主文本
    pub text: u32,
    /// 次要文本
    pub muted: u32,
    /// 弱化文本（禁用/提示）
    pub dim: u32,
    /// 强调色（选中/主按钮底）
    pub accent: u32,
    /// 强调色上的文本
    pub accent_dark: u32,
    /// 成功
    pub ok: u32,
    /// 警告
    pub warn: u32,
    /// 错误/危险
    pub error: u32,
    /// 表面（bg/panel/card）透明度 0-255：宿主窗口做整体透明时注入，
    /// 组件经 [`Theme::surface`] 把三个表面色转为带 alpha 的 Hsla；
    /// 文本/描边/强调色不受影响（保持不透明保证可读性）。
    pub surface_alpha: u8,
    /// 表面压暗 0-255 刻度（255=原色）：深色毛玻璃档用（配合 surface_alpha，
    /// 任何底色都被盖成深色底保证文字可读）。
    pub surface_dim: u8,
}

impl Theme {
    /// 深色默认（Tokyo Night 系），宿主未接调色板时的兜底。
    pub const DARK: Theme = Theme {
        bg: 0x1a1b26,
        panel: 0x1f2335,
        card: 0x24283b,
        border: 0x414868,
        text: 0xc0caf5,
        muted: 0x8189af,
        dim: 0x565f89,
        accent: 0x7aa2f7,
        accent_dark: 0x1a1b26,
        ok: 0x9ece6a,
        warn: 0xe0af68,
        error: 0xf7768e,
        surface_alpha: 0xff,
        surface_dim: 0xff,
    };

    /// 表面色（bg/panel/card）着色入口：压暗（[`Theme::surface_dim`]）+
    /// 叠加 [`Theme::surface_alpha`]。alpha=255 且不压暗时与 `rgb(v)` 完全
    /// 等价，不透明路径零行为变化。
    pub fn surface(&self, v: u32) -> gpui::Rgba {
        let k = self.surface_dim as f32 / 255.0;
        let dim = |c: u32| ((c as f32 * k).round() as u32).min(0xff);
        let rgb = (dim(v >> 16 & 0xff) << 16) | (dim(v >> 8 & 0xff) << 8) | dim(v & 0xff);
        gpui::rgba((rgb << 8) | self.surface_alpha as u32)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::DARK
    }
}
