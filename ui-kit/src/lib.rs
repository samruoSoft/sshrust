//! ui-kit：gpui 0.2.2 之上的自研组件库（本项目与后续项目共用）。
//!
//! 两大系列：
//! - [`base`]：基础控件——按钮、分段选择、标签行、模态浮层、锚定下拉菜单、
//!   文本输入框（单行 InputType/事件/图标/尺寸）、多行文本域（回车换行）、
//!   面板容器（标题栏/body/底栏三段式卡片，按需开启）。
//! - [`pro`]：复合控件——滚动容器（真实滚动 + 虚拟滚动两种模式）。
//!
//! 主题：组件不持色值，统一接收 [`Theme`]（12 个 u32 槽位，Copy），
//! 由宿主应用从自己的调色板转换（如 sshrust 的 `theme::Palette::kit()`）。
//!
//! 布局约定（gpui 0.2.2 实测坑，见各组件注释）：
//! - 嵌套列里 flex_1/percentage 高度不可靠，组件高度用显式像素；
//! - `on_drag_move` 按标记类型全局分发，多实例拖拽标记必须带 owner id 过滤；
//! - 浮层卡片本体不能挂 mouse-down 关闭（会吞掉内部按钮的 click 配对）。

pub mod base;
pub mod platform;
pub mod pro;
pub mod theme;

pub use theme::Theme;
