//! 平台样式补丁（macOS）。
//!
//! gpui 的窗口模糊（`WindowBackgroundAppearance::Blurred`）底层是
//! NSVisualEffectView（material=Selection，tint 跟随**系统**外观）——系统处于
//! 浅色模式时，浅色背景 app 在窗口下方会被糊成**浅色**，深色主题的浅色文字
//! 就看不清了。[`set_blur_tint`] 按宿主主题接管这些视图的 appearance：
//! 深色主题 → VibrantDark（深色毛玻璃，底下再白也糊成深底）、亮色 → VibrantLight。
//!
//! 全局枚举 NSApp 全部窗口的模糊视图统一设置（幂等，可反复调用）；
//! gpui 在 开窗/重新开启模糊 时会重建视图，重建后需再调一次。

/// 给全部窗口的模糊视图（NSVisualEffectView）设置外观 tint。
/// `dark = true` → VibrantDark（深色毛玻璃）；false → VibrantLight。
#[cfg(target_os = "macos")]
pub fn set_blur_tint(dark: bool) {
    unsafe { mac::tint_all(dark) };
}

#[cfg(not(target_os = "macos"))]
pub fn set_blur_tint(_dark: bool) {}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::CString;

    use objc::msg_send;
    use objc::rc::autoreleasepool;
    use objc::runtime::Object;
    use objc::{class, sel, sel_impl};

    const VIBRANT_DARK: &str = "NSAppearanceNameVibrantDark";
    const VIBRANT_LIGHT: &str = "NSAppearanceNameVibrantLight";

    /// 枚举 NSApp 所有窗口 contentView 的一级子视图，把 NSVisualEffectView
    /// （含 gpui 的 BlurredView 子类）的 appearance 设为指定 vibrancy。
    /// 没有运行中的 NSApplication（如单测）时安全返回。
    pub unsafe fn tint_all(dark: bool) {
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            if app.is_null() {
                return;
            }
            let windows: *mut Object = msg_send![app, windows];
            if windows.is_null() {
                return;
            }
            let appearance = new_vibrancy_appearance(dark);
            let count: usize = msg_send![windows, count];
            for i in 0..count {
                let win: *mut Object = msg_send![windows, objectAtIndex: i];
                let content: *mut Object = msg_send![win, contentView];
                if content.is_null() {
                    continue;
                }
                let subs: *mut Object = msg_send![content, subviews];
                if subs.is_null() {
                    continue;
                }
                let n: usize = msg_send![subs, count];
                for j in 0..n {
                    let sub: *mut Object = msg_send![subs, objectAtIndex: j];
                    let is_effect: bool =
                        msg_send![sub, isKindOfClass: class!(NSVisualEffectView)];
                    if is_effect {
                        // 只设 appearance（毛玻璃明暗倾向）。**不要动 material**：
                        // gpui 默认的 Selection 材质才有背后采样模糊，实验证实
                        // 换成 HudWindow 等自渲染材质后模糊整个消失（HUD 材质不
                        // 采样背后内容，且其层背景会被 gpui 的 updateLayer 剥掉）。
                        // 深色可读性由表面色 0.92+压暗保证，不依赖模糊层着色。
                        let _: () = msg_send![sub, setAppearance: appearance];
                    }
                }
            }
        }
    }

    /// 构造 vibrancy NSAppearance（appearanceNamed 返回自动释放对象，
    /// 包池调用避免无池环境告警/泄漏——本函数调用频率极低，代价可忽略）。
    unsafe fn new_vibrancy_appearance(dark: bool) -> *mut Object {
        unsafe {
            autoreleasepool(|| {
                let name = CString::new(if dark { VIBRANT_DARK } else { VIBRANT_LIGHT })
                    .expect("vibrancy appearance name");
                let name: *mut Object =
                    msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
                msg_send![class!(NSAppearance), appearanceNamed: name]
            })
        }
    }
}
