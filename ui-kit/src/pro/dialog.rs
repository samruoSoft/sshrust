//! DialogWindow：模态风格对话框窗口（kit 级独立组件）。
//!
//! - **置顶（"模式"效果，app 内）**：`stay_on_top`（默认开）在 app 激活期间浮于
//!   本 app 全部窗口之上；切到其他 app 时回落普通层级、可被其窗口遮挡
//!   （macOS 窗口 level 跨 app 全局，静态 Floating 级会盖住其他 app，故按激活态
//!   动态调级）。注意这是视觉模态——不阻塞其他窗口输入（gpui 无 app-modal API）。
//! - **按钮可选**：默认仅显示关闭按钮；`minimizable` 开最小化、`zoomable` 开最大化
//!   （`resizable=false` 时系统会自动禁用最大化）。
//! - **尺寸**：`resizable`（默认开）允许手动 resize + `min_*` 最小尺寸约束；
//!   内容自适应由内容视图负责（宿主把整个窗口交给内容 Entity 渲染）。
//! - **事件回调**：`open_dialog_listening` 把内容实体的事件转发给回调
//!   （如选择类对话框「选中即关」），回调里可用 `DialogCloser::close(cx)` 收掉窗口。
//!
//! 内容是任意 `Entity<T: Render>`：表单、选择器、确认框都可作为对话框内容复用；
//! 无事件的对话框用 `open_dialog`（E 默认 `NoDialogEvent`）。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    prelude::*, px, AnyElement, App, AppContext, Bounds, Context, Entity, EventEmitter,
    ParentElement, Render, SharedString, Size, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, rgb,
};

/// 无事件对话框的占位事件（永不发出）。
pub enum NoDialogEvent {}

/// 对话框规格（创建时一次性应用）。
#[derive(Clone, Debug)]
pub struct DialogSpec {
    /// 窗口标题（同时用于系统标题栏）。
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
    /// 允许手动 resize（默认 true）。
    pub resizable: bool,
    /// 显示最小化按钮（默认 false）。
    pub minimizable: bool,
    /// 显示最大化按钮（默认 false；resizable=false 时自动失效）。
    pub zoomable: bool,
    /// app 激活期间置于本 app 全部窗口之上（默认 true，"模式窗口"效果）；
    /// 切到其他 app 时回落普通层级，不遮挡其他 app 的窗口。
    pub stay_on_top: bool,
    /// 底部工具栏（footer 插槽）高度；仅在带 footer 打开时使用。
    pub footer_height: f32,
    /// 表面透明度 0.0-1.0（None = 不透明窗口，默认）。宿主做全局窗口透明时注入：
    /// 窗口背景按 [`Self::surface_blur`] 走 Blurred/Transparent，内容表面色的
    /// alpha 由所传 [`crate::Theme`] 的 `surface_alpha` 决定。开窗时快照生效。
    pub surface_alpha: Option<f32>,
    /// 表面透明时是否叠加系统模糊（毛玻璃；仅 surface_alpha < 1.0 时有意义）。
    pub surface_blur: bool,
}

impl Default for DialogSpec {
    fn default() -> Self {
        Self {
            title: "对话框".into(),
            width: 460.,
            height: 420.,
            min_width: 360.,
            min_height: 240.,
            resizable: true,
            minimizable: false,
            zoomable: false,
            stay_on_top: true,
            footer_height: 48.,
            surface_alpha: None,
            surface_blur: true,
        }
    }
}

impl DialogSpec {
    /// 由表面设置推导窗口背景模式：不透明 → Opaque；透明+模糊 → Blurred；
    /// 透明不模糊 → Transparent。
    pub fn window_background(&self) -> WindowBackgroundAppearance {
        match self.surface_alpha {
            Some(a) if a < 1.0 => {
                if self.surface_blur {
                    WindowBackgroundAppearance::Blurred
                } else {
                    WindowBackgroundAppearance::Transparent
                }
            }
            _ => WindowBackgroundAppearance::Opaque,
        }
    }
}

/// 关闭句柄：回调内 `close(cx)` 收掉对话框（窗口句柄在窗口创建后填充）。
pub struct DialogCloser<T: 'static, E: 'static = NoDialogEvent>(
    Rc<RefCell<Option<WindowHandle<DialogHost<T, E>>>>>,
);

impl<T: 'static, E: 'static> Clone for DialogCloser<T, E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: 'static, E: 'static> Default for DialogCloser<T, E> {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(None)))
    }
}

impl<T: Render, E: 'static> DialogCloser<T, E> {
    /// 关闭对话框。经 `cx.defer` 延迟执行：close 常被该窗口自己的事件回调
    /// （footer 按钮/订阅转发）同步调用，直接 `WindowHandle::update` 会对
    /// 正在分发事件的窗口重入借用，gpui 在 extern "C" 边界 panic 即崩溃。
    pub fn close(&self, cx: &mut App) {
        let win = self.0.borrow().clone();
        if let Some(win) = win {
            cx.defer(move |cx| {
                let _ = win.update(cx, |_, window, _| window.remove_window());
            });
        }
    }
}

/// 底部工具栏插槽：**引用 dialog 时**给定内容与回调——闭包每帧以
/// (内容实体, 关闭句柄, App) 重建元素（状态/文案实时），按钮用
/// `button_for` 组装（App 级回调，可调实体方法或 closer.close）。
pub struct DialogFooter<T: 'static, E: 'static = NoDialogEvent>(
    Rc<dyn Fn(&Entity<T>, &DialogCloser<T, E>, &App) -> AnyElement>,
);

impl<T: 'static, E: 'static> Clone for DialogFooter<T, E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: 'static, E: 'static> DialogFooter<T, E> {
    pub fn new(
        render: impl Fn(&Entity<T>, &DialogCloser<T, E>, &App) -> AnyElement + 'static,
    ) -> Self {
        Self(Rc::new(render))
    }
}

/// 对话框宿主：根视图，把窗口交给内容 Entity 渲染；有 footer 时
/// 内容区钉在 [0, 视口高-footer_h]、footer 栏钉底（项目布局约定：显式像素）。
pub struct DialogHost<T: 'static, E: 'static = NoDialogEvent> {
    content: Entity<T>,
    footer: Option<DialogFooter<T, E>>,
    footer_h: f32,
    theme: crate::Theme,
    closer: DialogCloser<T, E>,
    /// E 只参与窗口/句柄的类型区分（事件已转发到回调），占位保持类型。
    _event: std::marker::PhantomData<fn(&E)>,
}

impl<T: Render, E: 'static> DialogHost<T, E> {
    /// 内容实体（复用已开对话框时重载内容状态用）。
    pub fn content(&self) -> Entity<T> {
        self.content.clone()
    }
}

impl<T: Render, E: 'static> Render for DialogHost<T, E> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().id("dialog-host").size_full().relative();
        let Some(footer) = &self.footer else {
            // 无 footer：全幅交给内容；自适应/滚动/内边距由内容视图决定。
            return root.child(self.content.clone()).into_any_element();
        };
        let h: f32 = window.viewport_size().height.into();
        let content_h = (h - self.footer_h).max(0.);
        root
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(content_h))
                    .overflow_hidden()
                    .child(self.content.clone()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(content_h))
                    .left_0()
                    .w_full()
                    .h(px(self.footer_h))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .bg(self.theme.surface(self.theme.panel))
                    .border_t_1()
                    .border_color(rgb(self.theme.border))
                    .child(footer.render(&self.content, &self.closer, cx)),
            )
            .into_any_element()
    }
}

impl<T: 'static, E: 'static> DialogFooter<T, E> {
    fn render(&self, content: &Entity<T>, closer: &DialogCloser<T, E>, cx: &App) -> AnyElement {
        (self.0)(content, closer, cx)
    }
}

/// 已开对话框的句柄。
pub struct DialogHandle<T: 'static, E: 'static = NoDialogEvent> {
    pub window: WindowHandle<DialogHost<T, E>>,
    pub host: Entity<DialogHost<T, E>>,
    closer: DialogCloser<T, E>,
}

impl<T: Render, E: 'static> DialogHandle<T, E> {
    /// 窗口是否仍存活（红绿灯关闭后即失效）。
    pub fn is_open(&self, cx: &mut App) -> bool {
        self.window.update(cx, |_, _, _| ()).is_ok()
    }

    /// 带到前台并激活（复用已开对话框时）。
    pub fn activate(&self, cx: &mut App) {
        let _ = self.window.update(cx, |_, window, _| window.activate_window());
    }

    /// 以编程方式关闭（等效红绿灯）。
    pub fn close(&self, cx: &mut App) {
        self.closer.close(cx);
    }

    /// 内容实体。
    pub fn content(&self, cx: &App) -> Entity<T> {
        self.host.read(cx).content()
    }
}

/// 打开无事件、无 footer 的对话框（窗体主题默认深色）。
pub fn open_dialog<T: Render>(content: Entity<T>, spec: DialogSpec, cx: &mut App) -> DialogHandle<T> {
    open_dialog_themed(content, spec, crate::Theme::DARK, cx)
}

/// 打开无事件、无 footer 的对话框，窗体主题可指定
/// （跟随宿主 app 当前主题时用，避免弹出窗固定深色）。
pub fn open_dialog_themed<T: Render>(
    content: Entity<T>,
    spec: DialogSpec,
    theme: crate::Theme,
    cx: &mut App,
) -> DialogHandle<T> {
    let closer = DialogCloser::default();
    let host = cx.new(|_cx| DialogHost {
        content,
        footer: None,
        footer_h: spec.footer_height,
        theme,
        closer: closer.clone(),
        _event: std::marker::PhantomData,
    });
    let window = create_window(host.clone(), &spec, cx);
    *closer.0.borrow_mut() = Some(window.clone());
    DialogHandle { window, host, closer }
}

/// 打开带事件回调的对话框：内容实体每发一次 E，回调收到
/// （事件, 关闭句柄, App）；选择类对话框在回调里取值并 `close`。
pub fn open_dialog_listening<T, E>(
    content: Entity<T>,
    spec: DialogSpec,
    cx: &mut App,
    on_event: impl Fn(&E, &DialogCloser<T, E>, &mut App) + 'static,
) -> DialogHandle<T, E>
where
    T: Render + EventEmitter<E>,
    E: 'static,
{
    open_dialog_ex(content, spec, None, crate::Theme::DARK, Some(Rc::new(on_event)), cx)
}

/// 全功能版：footer 底部工具栏（插槽 + 主题）+ 可选事件回调。
/// 按钮等元素在 footer 闭包里用 `button_for` 组装（App 级回调）。
pub fn open_dialog_ex<T, E>(
    content: Entity<T>,
    spec: DialogSpec,
    footer: Option<DialogFooter<T, E>>,
    theme: crate::Theme,
    on_event: Option<Rc<dyn Fn(&E, &DialogCloser<T, E>, &mut App)>>,
    cx: &mut App,
) -> DialogHandle<T, E>
where
    T: Render + EventEmitter<E>,
    E: 'static,
{
    let closer = DialogCloser::default();
    let host = cx.new(|cx| {
        if let Some(cb) = on_event.clone() {
            let closer2 = closer.clone();
            cx.subscribe(&content, move |_this, _entity, ev: &E, cx: &mut Context<DialogHost<T, E>>| {
                cb(ev, &closer2, cx);
            })
            .detach();
        }
        DialogHost {
            content,
            footer,
            footer_h: spec.footer_height,
            theme,
            closer: closer.clone(),
            _event: std::marker::PhantomData,
        }
    });
    let window = create_window(host.clone(), &spec, cx);
    *closer.0.borrow_mut() = Some(window.clone());
    DialogHandle { window, host, closer }
}

fn create_window<T: Render, E: 'static>(
    host: Entity<DialogHost<T, E>>,
    spec: &DialogSpec,
    cx: &mut App,
) -> WindowHandle<DialogHost<T, E>> {
    let title: SharedString = spec.title.clone().into();
    let bounds = Bounds::centered(None, gpui::size(px(spec.width), px(spec.height)), cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(Size {
                    width: px(spec.min_width),
                    height: px(spec.min_height),
                }),
                titlebar: Some(TitlebarOptions {
                    title: Some(title),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                kind: WindowKind::Normal,
                is_movable: true,
                is_resizable: spec.resizable,
                is_minimizable: spec.minimizable,
                window_background: spec.window_background(),
                ..Default::default()
            },
            |_window, _cx| host,
        )
        .expect("open dialog window");
    // 置顶/隐藏最大化按钮是 gpui WindowOptions 覆盖不到的，创建后补样式。
    // 此时对话框刚以 focus:true 创建，即 NSApp.keyWindow（以标题校验防误伤）。
    #[cfg(target_os = "macos")]
    let _ = handle.update(cx, |_, _, _| mac::apply_dialog_style(spec));
    handle
}

/// macOS 对话框样式补丁：app 内置顶与最大化按钮隐藏。
/// 直接作用于 keyWindow 且标题一致才生效，失败静默（样式可选不强求）。
///
/// **app 内置顶**：macOS 窗口 level 是跨 app 的全局层级，静态设 Floating(3)
/// 会浮在所有 app 之上（盖住其他应用，不合"模态对话框"语义）。改为按激活态
/// 动态调级：app 激活时 floating（浮于本 app 全部窗口之上），失活时回落
/// normal（其他 app 的窗口可正常遮挡它）。窗口关窗时从注册表注销防悬垂指针。
#[cfg(target_os = "macos")]
mod mac {
    use std::ptr;
    use std::sync::{Mutex, OnceLock};

    use super::DialogSpec;
    use objc::{class, declare::ClassDecl, msg_send, sel, sel_impl};
    use objc::runtime::{Class, Object, Sel};

    // AppKit 通知名常量（框架导出的 C 符号，链接期解析）。
    unsafe extern "C" {
        static NSApplicationDidBecomeActiveNotification: *mut Object;
        static NSApplicationDidResignActiveNotification: *mut Object;
        static NSWindowWillCloseNotification: *mut Object;
    }

    const LEVEL_NORMAL: i64 = 0; // NSNormalWindowLevel
    const LEVEL_FLOATING: i64 = 3; // kCGFloatingWindowLevel

    /// 参与层级跟随的对话框窗口（通知回调与本模块调用均在主线程，
    /// Send 包装仅为满足 static 约束）。
    #[derive(Clone, Copy, PartialEq)]
    struct WindowPtr(*mut Object);
    unsafe impl Send for WindowPtr {}

    static WINDOWS: Mutex<Vec<WindowPtr>> = Mutex::new(Vec::new());

    unsafe fn set_all_levels(level: i64) {
        for &w in WINDOWS.lock().unwrap().iter() {
            let _: () = msg_send![w.0, setLevel: level];
        }
    }

    // 观察者方法：激活/失活整体调级；对话框关窗时注销该窗口的观察与注册表项。
    extern "C" fn app_did_become_active(_: &Object, _: Sel, _: *mut Object) {
        unsafe { set_all_levels(LEVEL_FLOATING) };
    }
    extern "C" fn app_did_resign_active(_: &Object, _: Sel, _: *mut Object) {
        unsafe { set_all_levels(LEVEL_NORMAL) };
    }
    extern "C" fn dialog_window_will_close(this: &Object, _: Sel, note: *mut Object) {
        unsafe {
            let win: *mut Object = msg_send![note, object];
            let name: *mut Object = msg_send![note, name];
            let center: *mut Object = msg_send![class!(NSNotificationCenter), defaultCenter];
            let this = this as *const Object as *mut Object;
            let _: () = msg_send![center, removeObserver: this name: name object: win];
            WINDOWS.lock().unwrap().retain(|w| w.0 != win);
        }
    }

    fn observer_class() -> &'static Class {
        static CLASS: OnceLock<&'static Class> = OnceLock::new();
        *CLASS.get_or_init(|| unsafe {
            let mut decl = ClassDecl::new("UiKitDialogLevelObserver", class!(NSObject))
                .expect("注册 UiKitDialogLevelObserver");
            decl.add_method(
                sel!(appDidBecomeActive:),
                app_did_become_active as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(appDidResignActive:),
                app_did_resign_active as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(dialogWindowWillClose:),
                dialog_window_will_close as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.register()
        })
    }

    /// 全局观察者单例（usize 存指针以便入 static）；激活/失活观察永不注销。
    fn observer() -> *mut Object {
        static OBSERVER: OnceLock<usize> = OnceLock::new();
        *OBSERVER.get_or_init(|| unsafe {
            let nil: *mut Object = ptr::null_mut();
            let observer: *mut Object = msg_send![observer_class(), new];
            let center: *mut Object = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center,
                addObserver: observer selector: sel!(appDidBecomeActive:)
                name: NSApplicationDidBecomeActiveNotification object: nil];
            let _: () = msg_send![center,
                addObserver: observer selector: sel!(appDidResignActive:)
                name: NSApplicationDidResignActiveNotification object: nil];
            observer as usize
        }) as *mut Object
    }

    /// 登记一个对话框窗口进入"app 内置顶"跟随：立即抬到 floating（创建时
    /// app 必然激活——对话框以 focus:true 打开），并挂 willClose 注销。
    unsafe fn register_app_scoped_floating(win: *mut Object) {
        unsafe {
            let _: () = msg_send![win, setLevel: LEVEL_FLOATING];
            let observer = observer();
            let center: *mut Object = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center,
                addObserver: observer selector: sel!(dialogWindowWillClose:)
                name: NSWindowWillCloseNotification object: win];
            WINDOWS.lock().unwrap().push(WindowPtr(win));
        }
    }

    pub(super) fn apply_dialog_style(spec: &DialogSpec) {
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            if app.is_null() {
                return;
            }
            let key: *mut Object = msg_send![app, keyWindow];
            if key.is_null() {
                return;
            }
            // 标题校验：keyWindow 必须就是刚创建的对话框。
            let title: *mut Object = msg_send![key, title];
            let utf8: *const std::os::raw::c_char = msg_send![title, UTF8String];
            if utf8.is_null() {
                return;
            }
            let got = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
            if got != spec.title {
                return;
            }
            if spec.stay_on_top {
                register_app_scoped_floating(key);
            }
            // NSWindowZoomButton = 2：resizable 但不要最大化按钮时隐藏。
            if !spec.zoomable && spec.resizable {
                let btn: *mut Object = msg_send![key, standardWindowButton: 2i64];
                if !btn.is_null() {
                    let _: () = msg_send![btn, setHidden: objc::runtime::YES];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_defaults_are_dialog_style() {
        let s = DialogSpec::default();
        assert!(s.resizable, "对话框默认可 resize");
        assert!(!s.minimizable, "默认无最小化按钮");
        assert!(!s.zoomable, "默认无最大化按钮");
        assert!(s.stay_on_top, "默认置顶（模式窗口）");
    }
}
