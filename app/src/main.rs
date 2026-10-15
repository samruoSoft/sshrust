//! sshrust GUI 入口：窗口/键位/全局运行时。
//! 具体布局见 app_ui.rs（三栏 + 紧凑标题栏 + 多 Tab）。

mod app_ui;
mod assets;
mod db;
mod dir_panel;
mod form_command_edit;
mod form_folder_edit;
mod form_service_edit;
mod hosts;
mod i18n;
mod icon_picker;
mod icons;
mod popped;
mod procs;
mod settings_view;
mod sidebar;
mod sysinfo;
mod theme;

use std::sync::OnceLock;

use gpui::{
    App, AppContext, Application, Bounds, Entity, KeyBinding, TitlebarOptions, WindowBounds,
    WindowHandle, WindowOptions, actions, px, size,
};

use app_ui::{AppUi, CloseTab, NewTab, NextTab, PrevTab, ToggleLeft, ToggleRight};

actions!(main, [Quit]);

pub static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    RUNTIME.set(runtime).ok();

    // Dock 重开回调在 Application 构建期注册，而 AppUi 实体要到 run 回调里才
    // 创建——经共享槽转交（回调里取一份克隆）。
    let root_slot: std::rc::Rc<std::cell::RefCell<Option<Entity<AppUi>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let reopen_slot = root_slot.clone();

    let app = Application::new().with_assets(assets::EmbeddedAssets);
    // 红灯关掉全部窗口后进程驻留 Dock（macOS 惯例）：再点 Dock 图标时经
    // applicationShouldHandleReopen 重建主窗口——复用原 AppUi 实体，
    // 会话/终端/tab 状态原样恢复（连接不中断）。仍有窗口时仅激活。
    app.on_reopen(move |cx: &mut App| {
        let root = reopen_slot.borrow().clone();
        if let Some(root) = root {
            if cx.windows().is_empty() {
                open_main_window(&root, cx);
            }
        }
        cx.activate(true);
    });
    app.run(move |cx: &mut App| {
        ui_kit::base::text_input::bind_keys(cx);
        ui_kit::base::text_area::bind_keys(cx);
        ui_kit::pro::tree::bind_keys(cx);
        ui_kit::base::modal::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            // 表单窗口内 Esc = 放弃关闭（上下文限定，不影响树内重命名的 Esc）。
            KeyBinding::new("escape", form_service_edit::CancelForm, Some("form_service_edit")),
            KeyBinding::new("escape", form_command_edit::CancelCmdForm, Some("form_command_edit")),
            KeyBinding::new("escape", form_folder_edit::CancelFolderForm, Some("form_folder_edit")),
            KeyBinding::new("cmd-shift-]", NextTab, None),
            KeyBinding::new("cmd-shift-[", PrevTab, None),
            KeyBinding::new("cmd-b", ToggleLeft, None),
            KeyBinding::new("alt-cmd-b", ToggleRight, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let root_entity: Entity<AppUi> = cx.new(|cx| AppUi::new(cx));
        *root_slot.borrow_mut() = Some(root_entity.clone());
        let window = open_main_window(&root_entity, cx);

        window
            .update(cx, |_view, window, cx| {
                cx.activate(true);
                // --mock-demo[=N]：启动即自动开 N 个 mock 终端 tab（默认 1；演示/截图用）
                let demo_count = std::env::args()
                    .find(|a| a.starts_with("--mock-demo"))
                    .map(|a| a.strip_prefix("--mock-demo").unwrap_or("").to_string());
                if let Some(count) = demo_count {
                    let n: usize = count
                        .strip_prefix('=')
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1);
                    let root = root_entity.clone();
                    // window.update 已在更新该实体，延迟到下一周期再开演示 tab。
                    cx.spawn(async move |_this: gpui::WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
                        let _ = root.update(cx, |ui, cx| {
                            for _ in 0..n.max(1) {
                                ui.connect_mock(cx);
                            }
                        });
                    })
                    .detach();
                }
                let _ = window;
            })
            .ok();

        // 独立窗口被关闭（红绿灯等）→ 其中的 tab 收回主窗口（会话不断）。
        cx.on_window_closed({
            let root = root_entity.clone();
            move |cx: &mut App| {
                let _ = root.update(cx, |ui, cx| ui.reattach_dead_windows(cx));
            }
        })
        .detach();
    });
}

/// 主窗口打开流程（启动与 Dock 重开共用）：同样尺寸/标题栏/背景模式，
/// 复用传入的 AppUi 实体作窗口根（状态保留），并回写主窗句柄。
fn open_main_window(root: &Entity<AppUi>, cx: &mut App) -> WindowHandle<AppUi> {
    let bounds = Bounds::centered(None, size(px(1100.0), px(680.0)), cx);
    // 背景透明度：开窗时决定窗口背景模式（运行期变化由 AppUi::render 对账同步）。
    let bg_fx = theme::BgFx::load(&db::Db::open());
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(gpui::Size {
                    width: px(760.0),
                    height: px(420.0),
                }),
                // 紧凑标题栏：内容铺满整窗，tab 与交通灯同排。
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_background: bg_fx.window_appearance(),
                ..Default::default()
            },
            |_, _| root.clone(),
        )
        .expect("open main window");
    // 记住主窗口句柄：表单窗口里发起连接后要把主窗带到前台。
    let _ = root.update(cx, |ui: &mut AppUi, _| ui.main_window = Some(window.clone()));
    window
}
