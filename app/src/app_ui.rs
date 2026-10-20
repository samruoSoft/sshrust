//! AppUi：三栏布局 + 紧凑标题栏（tab 与交通灯同排）+ 多会话 Tab + 右缘全高工具栏。
//!
//! 布局：
//! ┌──────────────────────────────────────────────┬────┐
//! │ 🚦🚦🚦 [tab1][tab2]      空白(拖拽)            │ ◧  │
//! ├──────────┬────────────────────────┬──────────┤ ◨  │ ← 右缘全高
//! │ 服务器列表 │   中间：Tab 内容(终端)   │ 文件传输 │    │   工具栏(常驻)
//! │ (可收缩)  │   连接表单 / 终端 / 错误  │ (可收缩) │    │
//! └──────────┴────────────────────────┴──────────┴────┘
//! 新增连接不走 tab 条「＋」（已移除）：入口在左侧树（右键菜单/底部工具栏）与 ⌘T。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    actions, px, rgb, AnyElement, AppContext, Bounds, ClickEvent, Context, CursorStyle,
    Entity, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render, SharedString,
    Styled, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div, prelude::*,
};

use gpui_terminal::{TerminalConfig, TerminalView};

use sshrust_core::session::open_session_traced;
use sshrust_core::{
    mock, AuthMethod, ConnectParams, ConnectStep, Session, SessionEvent, Transport,
};

use crate::db::{CommandData, Db, NewNode, NodeKind, ROOT_SERVERS, ServerData};
use crate::icons;
use ui_kit::pro::scroll_area::{self, ScrollArea};
use crate::sidebar::{build_forest, SelectKey, KIT_GROUP_COMMANDS, KIT_GROUP_SERVERS, KIT_GROUP_SERVER_CMDS};
use ui_kit::pro::tree::TreeState;
use ui_kit::base::button::{self as kit_button, ButtonKind, ButtonSize};
use crate::theme::*;
use crate::theme;

actions!(
    app_ui,
    [
        NewTab, CloseTab, NextTab, PrevTab, ToggleLeft, ToggleRight, ShowAbout, ShowSettings
    ]
);

/// 原生菜单栏（macOS NSMenu，经 `App::set_menus` 注册）。
/// 首个菜单 = 应用菜单：关于 / 设置（⌘,）/ 退出（⌘Q，快捷键显示由 keymap 匹配）。
/// 语言切换时由 I18nGlobal 观察者按新 locale 重建。
pub(crate) fn app_menus(loc: crate::i18n::Locale) -> Vec<gpui::Menu> {
    let t = |k: crate::i18n::Tr| crate::i18n::tl(loc, k);
    vec![gpui::Menu {
        name: "SSHRust".into(),
        items: vec![
            gpui::MenuItem::action(t(crate::i18n::Tr::MenuAbout), ShowAbout),
            gpui::MenuItem::separator(),
            gpui::MenuItem::action(t(crate::i18n::Tr::MenuSettings), ShowSettings),
            gpui::MenuItem::separator(),
            // Quit 定义于 main.rs（crate 根），全局监听 cx.quit()。
            gpui::MenuItem::action(t(crate::i18n::Tr::MenuQuit), crate::Quit),
        ],
    }]
}

/// 右面板宽度拖拽标记。gpui 的 on_drag_move 按标记类型过滤，
/// 与侧栏的 WidthDrag 分立，两条宽度拖拽才不会互相触发。
#[derive(Clone)]
struct RightWidthDrag(Arc<Mutex<Option<(f32, f32)>>>);

/// 右面板当前页（右缘工具栏的工具按钮选择；点击激活中的工具 = 收起面板）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RightPage {
    /// 目录浏览（目录树整页；会话状态区已迁至文件传输页）。
    Dir,
    /// 系统信息（系统/CPU/内存/网络/磁盘）。**默认页**。
    SysInfo,
    /// 进程列表（全量进程，表头排序，CPU% 双拍差分）。
    Processes,
    /// 会话状态（按 tab 列连接状态/流量/时长）。
    Sessions,
}

pub enum TabKind {
    Connecting { params: ConnectParams },
    Live {
        term: Entity<TerminalView>,
        session: Session,
        opened_at: Instant,
    },
    Failed { params: ConnectParams, error: String },
}

pub struct Tab {
    pub id: u64,
    pub title: String,
    /// 服务器图标名（与侧栏节点一致）；None = 回退状态圆点。
    pub icon: Option<String>,
    pub kind: TabKind,
}

impl Tab {
    fn state_dot(&self, pal: &theme::Palette) -> u32 {
        match &self.kind {
            TabKind::Connecting { .. } => pal.warn,
            TabKind::Live { .. } => pal.ok,
            TabKind::Failed { .. } => pal.error,
        }
    }

    fn state_text(&self, pal: &theme::Palette, loc: crate::i18n::Locale) -> (u32, &'static str) {
        match &self.kind {
            TabKind::Connecting { .. } => (pal.warn, crate::i18n::tl(loc, crate::i18n::Tr::TabConnecting)),
            TabKind::Live { .. } => (pal.ok, crate::i18n::tl(loc, crate::i18n::Tr::TabConnected)),
            TabKind::Failed { .. } => (pal.error, crate::i18n::tl(loc, crate::i18n::Tr::TabDisconnected)),
        }
    }
}

pub struct AppUi {
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active_id: u64,
    next_tab_id: u64,
    /// tab 最近使用序（最新在前）：关闭激活 tab 时回跳到最近使用的剩余 tab。
    tab_mru: Vec<u64>,
    /// tab 条横向滚动偏移（px，0 = 最左；溢出时经 ‹ › 按钮移动）。
    pub(crate) tab_scroll_x: f32,
    /// 上次渲染算出的滚动上限（内容宽 − 视口宽；箭头点击钳制用）。
    pub(crate) tab_scroll_max: f32,
    /// 上次已滚入视野的激活 tab id：只在变化时定位（否则每帧重绘都会把
    /// 用户手动滚开的位置拽回激活 tab——状态面板 1s 心跳就会触发）。
    tab_scroll_focus: u64,
    /// 已弹出为独立窗口的 tab（登记窗口句柄/实体/连接参数；关窗时收回）。
    popped: HashMap<u64, crate::popped::PoppedWin>,

    /// 侧栏树 + 界面状态持久化（SQLite）。
    pub(crate) db: Db,
    /// 侧栏树组件（kit Tree；展开/选中/重命名/拖拽内聚，事件订阅分发）。
    pub(crate) tree: Entity<TreeState>,
    /// 侧栏当前选中项（树 Select 事件的镜像，▶ 连接等入口用）。
    pub(crate) selected: Option<SelectKey>,
    /// 快捷栏高度（分割线拖拽调整，持久化）。
    pub(crate) shortcut_h: f32,
    /// 分割线拖拽锚点（鼠标 y、起始高度；每次拖拽开始时重置）。
    pub(crate) split_drag: Arc<Mutex<Option<(f32, f32)>>>,
    /// 打开中的右键/下拉菜单浮层。
    pub(crate) menu: Option<crate::sidebar::ContextMenu>,
    /// 终端滚动状态（虚拟滚动模式；同屏只显示一个终端）。
    pub(crate) term_scroll_state: Entity<scroll_area::ScrollAreaState>,
    /// 右侧目录面板：每个已连接 tab 一份当前目录状态（cwd + 清单缓存）。
    pub(crate) dir_data: HashMap<u64, crate::dir_panel::DirTree>,
    /// 目录面板树组件（kit Tree；展开懒加载/滚动内聚）。
    pub(crate) dir_tree: Entity<TreeState>,
    /// 目录树 id → (路径, 是否目录) 映射（展开懒加载/右键菜单定位用，随 refresh_dir_tree 重建）。
    pub(crate) dir_paths: HashMap<usize, (String, bool)>,
    /// 目录页传输区：每个 tab 一份传输任务列表（SFTP 上传/下载）。
    pub(crate) transfer_data: HashMap<u64, Vec<crate::transfer::TransferItem>>,
    /// 传输区列表滚动状态（真实滚动模式）。
    pub(crate) transfer_scroll: Entity<scroll_area::ScrollAreaState>,
    /// 上传覆盖确认模态（目标目录已有同名文件）。
    pub(crate) confirm_overwrite: Option<crate::transfer::ConfirmOverwrite>,
    /// 连接过程日志（按 tab）：(距发起连接的毫秒, 阶段)。失败页/连接中页展示。
    pub(crate) connect_log: HashMap<u64, Vec<(u32, ConnectStep)>>,
    /// P2P 会话网络情况（按 tab；iroh 连接成功后由 netinfo::probe 填充）。
    pub(crate) net_data: HashMap<u64, crate::netinfo::NetInfo>,
    /// 系统信息面板：每个已连接 tab 一份（探针差分 + 历史）。
    pub(crate) sys_data: HashMap<u64, crate::sysinfo::SysInfo>,
    /// 系统信息页滚动状态（真实滚动模式）。
    pub(crate) sys_scroll: Entity<scroll_area::ScrollAreaState>,
    /// 进程面板：每个已连接 tab 一份（双拍差分 + 排序视图 + 选中详情）。
    pub(crate) proc_data: HashMap<u64, crate::procs::ProcState>,
    /// 进程页滚动条状态（虚拟滚动模式；偏移由 procs_scroll_y 自管）。
    pub(crate) procs_scroll: Entity<scroll_area::ScrollAreaState>,
    /// 进程页虚拟滚动偏移（px，0=内容顶部；渲染期换算可视行窗口）。
    pub(crate) procs_scroll_y: f32,
    /// 进程页详情区高度（分割线拖拽调整，持久化 procs.detail.h）。
    pub(crate) procs_detail_h: f32,
    /// 进程页分割线拖拽锚点（鼠标 y、起始详情高）。
    pub(crate) procs_split_drag: Arc<Mutex<Option<(f32, f32)>>>,
    /// 进程页详情区滚动状态（真实滚动模式）。
    pub(crate) procs_detail_scroll: Entity<scroll_area::ScrollAreaState>,
    /// 外部搜索引擎（进程详情搜索；设置「通用 › 系统」选择，持久化）。
    pub(crate) search_engine: crate::procs::SearchEngine,
    /// 会话状态页滚动状态（真实滚动模式）。
    pub(crate) sessions_scroll: Entity<scroll_area::ScrollAreaState>,
    /// 右面板当前页（见 [`RightPage`]）。
    pub(crate) right_page: RightPage,
    /// 目录页传输区高度（分割线拖拽调整，持久化）。
    pub(crate) dir_transfer_h: f32,
    /// 目录页分割线拖拽锚点（鼠标 y、起始传输区高度）。
    pub(crate) dir_split_drag: Arc<Mutex<Option<(f32, f32)>>>,
    /// 侧栏宽度（右缘可拖拽调整，持久化）。
    pub(crate) sidebar_w: f32,
    /// 服务器树行模式：true = 详细两行（别名+徽章 / 地址），false = 简单单行。
    pub(crate) tree_detail: bool,
    /// 侧栏宽度拖拽锚点（鼠标 x、起始宽度）。
    pub(crate) width_drag: Arc<Mutex<Option<(f32, f32)>>>,
    /// 右侧状态面板宽度（左缘可拖拽调整，持久化）。
    pub(crate) right_w: f32,
    /// 右侧面板宽度拖拽锚点（鼠标 x、起始宽度）。
    pub(crate) right_width_drag: Arc<Mutex<Option<(f32, f32)>>>,
    /// 当前主题索引（theme::THEMES），持久化于 settings。
    pub(crate) theme_idx: usize,
    /// 当前终端配色索引（theme::TERM_THEMES，独立目录）。
    /// 界面主题系列翻转时联动切换；深/浅各自记忆（terminal.theme.dark/.light）。
    pub(crate) term_theme_idx: usize,
        /// 本地着色模板：当前生效规则集（来源与编辑在设置窗 settings_view 中管理）。
    pub(crate) term_style: gpui_terminal::StyleTemplate,
    /// 终端字体（全局设置，不随模板）：家族 + 字号（设置窗「外观→控制台」）。
    pub(crate) term_font_family: String,
    pub(crate) term_font_size: f32,

    pub(crate) left_open: bool,
    pub(crate) right_open: bool,
    /// 内置 mock 服务器（懒启动，多会话复用同一实例；保活）。
    pub(crate) mock: Option<mock::MockServer>,

    /// 服务器表单对话框窗口（DialogWindow；同时一个，再开时复用并激活）。
    pub(crate) form_win: Option<ui_kit::pro::dialog::DialogHandle<crate::form_service_edit::ServiceEditForm>>,
    /// 命令表单对话框窗口（新增/编辑命令；同时一个，再开时复用并激活）。
    pub(crate) cmd_form_win:
        Option<ui_kit::pro::dialog::DialogHandle<crate::form_command_edit::CommandEditForm, crate::form_command_edit::CommandFormEvent>>,
    /// 目录名称表单对话框窗口（同时一个，再开时复用并激活）。
    pub(crate) folder_form_win:
        Option<ui_kit::pro::dialog::DialogHandle<crate::form_folder_edit::FolderEditForm, crate::form_folder_edit::FolderFormEvent>>,
    /// 「关于」对话框窗口（同时一个，再开时复用并激活）。
    pub(crate) about_win: Option<ui_kit::pro::dialog::DialogHandle<crate::about::AboutView>>,
    /// 主窗口自身句柄（表单窗口里发起连接后把主窗带到前台）。
    pub(crate) main_window: Option<gpui::WindowHandle<AppUi>>,
    /// 删除确认模态（替代原生 window.prompt）。
    pub(crate) confirm_delete: Option<ConfirmDelete>,
    /// 顶部浮条提示（运行命令无终端等；超时自动消失）。
    pub(crate) notice: Option<(String, Instant)>,
    /// 专用命令组当前对应的 server 节点（换服务器时组自动展开一次）。
    server_cmds_prev: Option<i64>,
    /// 已解析语言（树/菜单等无 cx 构建路径的取词依据；随语言切换更新）。
    pub(crate) loc: crate::i18n::Locale,
    /// 表面效果缓存（BgGlobal+ThemeGlobal 镜像；theme() 无 cx 取色用）。
    bg_params: theme::SurfaceFx,
    /// 已应用到主窗口的背景效果（render 对账，变化时调 set_background_appearance）。
    applied_fx: theme::BgFx,
    /// 已应用的模糊 tint（深/亮系；render 对账。取反哨兵初始化，首帧强制应用一次）。
    applied_tint: bool,
}

/// 删除确认模态的状态（句柄 + 目标节点）。
pub(crate) struct ConfirmDelete {
    pub modal: ui_kit::base::modal::ModalHandle,
    pub id: i64,
    pub name: String,
}

impl AppUi {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut db = Db::open();
        // relay/令牌全局化：旧版按服务器存的值迁入全局设置（一次性）。
        crate::db::migrate_iroh_to_global(&mut db);
        let theme_idx: usize = db
            .get_setting("theme")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
            .min(theme::THEMES.len() - 1);
        // 界面语言：设置值（缺省 auto）→ 解析（auto 检测系统语言）。
        let loc = crate::i18n::resolve(crate::i18n::Lang::parse(
            &db.get_setting(crate::i18n::KEY_LANG).unwrap_or_default(),
        ));
        // 终端配色独立于界面主题：取界面系列对应记忆值，无记忆用系列默认。
        let series = theme::ui_series(theme_idx);
        let term_theme_idx: usize = db
            .get_setting(series.setting_key())
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&i| theme::term_for(i).series == series)
            .unwrap_or_else(|| theme::term_default(series));
        // 本地着色模板（设置窗管理：内置/用户自定义）
        let term_style = crate::settings_view::load_style(&db);
        // 终端字体（全局，不随模板）
        let term_font_family = db.get_setting("terminal.font.family").unwrap_or_else(|| "Menlo".into());
        let term_font_size = crate::settings_view::normalize_font_size(
            db.get_setting("terminal.font.size").and_then(|v| v.parse::<f32>().ok()),
        );
        let bg_fx = theme::BgFx::load(&db);
        let sidebar_w = db
            .get_setting("sidebar.width")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(LEFT_W)
            .clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
        let right_w = db
            .get_setting("right.width")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(RIGHT_W)
            .clamp(RIGHT_MIN_W, RIGHT_MAX_W);
        let shortcut_h = db
            .get_setting("sidebar.shortcut_h")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(SHORTCUT_DEFAULT_H)
            .max(SHORTCUT_MIN_H);
        let dir_transfer_h = db
            .get_setting(crate::dir_panel::SETTING_DIR_TRANSFER_H)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(crate::dir_panel::TRANSFER_DEFAULT_H)
            .max(crate::dir_panel::TRANSFER_MIN_H);
        // 持久化展开集 → kit 树 id（分组哨兵映射）
        let expanded_kit: Vec<usize> = db
            .get_setting("sidebar.expanded")
            .and_then(|v| serde_json::from_str::<Vec<i64>>(&v).ok())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| match *id {
                        ROOT_SERVERS => Some(KIT_GROUP_SERVERS),
                        crate::db::ROOT_COMMANDS => Some(KIT_GROUP_COMMANDS),
                        crate::db::ROOT_SERVER_CMDS => Some(KIT_GROUP_SERVER_CMDS),
                        n if n > 0 => Some(n as usize),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![KIT_GROUP_SERVERS, KIT_GROUP_COMMANDS, KIT_GROUP_SERVER_CMDS]
            });

        // 树组件：事件订阅分发（选中/双击连接/展开持久化/右键菜单/重命名/拖拽）
        let tree = cx.new(|cx| TreeState::new(cx));
        cx.subscribe(&tree, |this, _, ev, cx| this.on_tree_event(ev, cx)).detach();
        tree.update(cx, |t, cx| t.set_expanded(expanded_kit, cx));
        // 树行模式（简洁/详细，持久化；缺省详细）。
        let tree_detail = db
            .get_setting("sidebar.tree.detail")
            .map(|v| v != "0")
            .unwrap_or(true);
        tree.update(cx, |t, cx| t.set_detail(tree_detail, cx));

        // 终端滚动状态（虚拟滚动模式；同屏只显示一个终端）。
        let term_scroll_state = cx.new(|_| scroll_area::ScrollAreaState::new(2));
        // 系统信息页滚动（真实滚动模式；owner 避开 1 tree / 2 term）。
        let sys_scroll = cx.new(|_| scroll_area::ScrollAreaState::new(3));
        // 文件传输页滚动（真实滚动模式；owner 4）。
        let sessions_scroll = cx.new(|_| scroll_area::ScrollAreaState::new(4));
        // 进程页滚动条（虚拟滚动模式；owner 5；偏移在 procs_scroll_y 自管）。
        let procs_scroll = cx.new(|_| scroll_area::ScrollAreaState::new(5));
        // 进程页详情区滚动（真实滚动模式；owner 6）。
        let procs_detail_scroll = cx.new(|_| scroll_area::ScrollAreaState::new(6));
        let transfer_scroll = cx.new(|_| scroll_area::ScrollAreaState::new(7));
        // 进程页详情区高度（分割线拖拽，持久化）。
        let procs_detail_h = db
            .get_setting(crate::procs::SETTING_PROCS_DETAIL_H)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(crate::procs::DETAIL_DEFAULT_H)
            .max(crate::procs::DETAIL_MIN_H);
        // 外部搜索引擎（进程详情搜索，持久化）。
        let search_engine = crate::procs::SearchEngine::parse(
            &db.get_setting(crate::procs::SETTING_SEARCH_ENGINE).unwrap_or_default(),
        );
        // 目录面板树（kit Tree；行距较侧栏更紧凑的文件树密度）。
        let dir_tree = cx.new(|cx| {
            let mut t = TreeState::new(cx);
            t.set_metrics(24., 12., cx);
            t
        });
        // 目录树事件：展开未缓存目录 → 懒加载清单。
        cx.subscribe(&dir_tree, |this, _, ev, cx| this.on_dir_tree_event(ev, cx)).detach();
        cx.observe(&term_scroll_state, |_, _, cx| cx.notify()).detach();
        // 真实模式滚轮更新的是 scroll state 自身，观察它驱动本窗重画滚动条。
        cx.observe(&sys_scroll, |_, _, cx| cx.notify()).detach();
        cx.observe(&sessions_scroll, |_, _, cx| cx.notify()).detach();
        cx.observe(&procs_scroll, |_, _, cx| cx.notify()).detach();
        cx.observe(&procs_detail_scroll, |_, _, cx| cx.notify()).detach();
        cx.observe(&transfer_scroll, |_, _, cx| cx.notify()).detach();

        let this = Self {
            tabs: Vec::new(),
            active_id: 0,
            next_tab_id: 1,
            tab_mru: Vec::new(),
            tab_scroll_x: 0.,
            tab_scroll_max: 0.,
            tab_scroll_focus: 0,
            popped: HashMap::new(),
            db,
            tree,
            selected: None,
            shortcut_h,
            split_drag: Arc::new(Mutex::new(None)),
            menu: None,
            term_scroll_state,
            dir_data: HashMap::new(),
            transfer_data: HashMap::new(),
            transfer_scroll,
            confirm_overwrite: None,
            connect_log: HashMap::new(),
            net_data: HashMap::new(),
            dir_tree,
            dir_paths: HashMap::new(),
            sys_data: HashMap::new(),
            sys_scroll,
            proc_data: HashMap::new(),
            procs_scroll,
            procs_scroll_y: 0.,
            procs_detail_h,
            procs_split_drag: Arc::new(Mutex::new(None)),
            procs_detail_scroll,
            search_engine,
            sessions_scroll,
            right_page: RightPage::SysInfo,
            dir_transfer_h,
            dir_split_drag: Arc::new(Mutex::new(None)),
            sidebar_w,
            tree_detail,
            width_drag: Arc::new(Mutex::new(None)),
            right_w,
            right_width_drag: Arc::new(Mutex::new(None)),
            form_win: None,
            cmd_form_win: None,
            folder_form_win: None,
            about_win: None,
            main_window: None,
            notice: None,
            server_cmds_prev: None,
            theme_idx,
            term_theme_idx,
            term_style,
            term_font_family,
            term_font_size,
            left_open: true,
            right_open: true,
            mock: None,
            confirm_delete: None,
            loc,
            bg_params: bg_fx.fx(theme::ui_series(theme_idx) == theme::TermSeries::Light),
            applied_fx: bg_fx,
            applied_tint: theme::ui_series(theme_idx) != theme::TermSeries::Dark,
        };
        // 初始：空状态（不预建「新建连接」tab）；⌘T / ＋ / 双击服务器随时新建。
        cx.set_global(theme::ThemeGlobal(theme_idx));
        // 背景透明度全局镜像：各窗口（含弹出窗/对话框）render 时读取对账。
        cx.set_global(theme::BgGlobal(bg_fx));
        cx.observe_global::<theme::BgGlobal>(|this, cx| {
            this.bg_params = theme::surface_fx(cx);
            // 已连接终端的默认背景 alpha 在 colors 里，必须推新配置
            // （新建连接走 terminal_config 自动带上，这里管存量）。
            this.refresh_term_theme(cx);
            cx.notify();
        })
        .detach();
        // 主题系列翻转同样改变有效不透明度（亮色系保可读档）：刷新表面 alpha
        // 与已连接终端的背景 alpha（TermPalette 的 alpha 经 refresh_term_theme 推入）。
        // 树组件是独立 Entity、渲染用的是缓存注入的 kit Theme，切主题必须
        // 重建推新——否则侧栏/目录树文字停留在旧系列（亮色系下仍是黑系的浅色字）。
        cx.observe_global::<theme::ThemeGlobal>(|this, cx| {
            this.bg_params = theme::surface_fx(cx);
            this.refresh_term_theme(cx);
            this.refresh_tree(cx);
            this.refresh_dir_tree(cx);
            cx.notify();
        })
        .detach();
        // 语言全局镜像 + 观察自身：设置窗切语言 set_global 后本窗（及本实体的
        // 渲染路径）即时更新树/菜单文案。
        cx.set_global(crate::i18n::I18nGlobal(loc));
        cx.observe_global::<crate::i18n::I18nGlobal>(|this, cx| {
            this.loc = crate::i18n::current(cx);
            // 原生菜单栏文案随语言即时重建（set_menus 幂等重建 NSMenu）。
            cx.set_menus(app_menus(this.loc));
            this.refresh_tree(cx);
            this.refresh_dir_tree(cx);
            cx.notify();
        })
        .detach();
        let mut ui = this;
        ui.refresh_tree(cx);
        ui.refresh_dir_tree(cx);
        // 状态面板的 1s 刷新（打开时才有意义，代价可忽略）。
        ui.spawn_status_ticker(cx);
        ui
    }

    /// 当前激活 Live tab 对应的 db 服务器节点（专用命令组的数据源）。
    /// 表单直连/mock 等未落库的主机返回 None（专用组不显示）。
    pub(crate) fn active_server_node(&self) -> Option<(i64, String)> {
        let tab = self.tabs.iter().find(|t| t.id == self.active_id)?;
        let TabKind::Live { session, .. } = &tab.kind else { return None };
        let node = self.db.server_by_addr(&session.params.host, session.params.port)?;
        let title = if node.name.trim().is_empty() {
            match &node.kind {
                NodeKind::Server(d) => d.host.clone(),
                _ => node.name.clone(),
            }
        } else {
            node.name.clone()
        };
        Some((node.id, title))
    }

    /// 由 db + 连接态重建树数据并推入组件（任何节点/在线态变更后调用）。
    pub(crate) fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let server_cmds = self.active_server_node();
        let forest = build_forest(
            &self.db,
            &self.theme(),
            &|h, p| self.host_is_live(h, p),
            self.loc,
            server_cmds.clone(),
        );
        let kit = self.theme().kit();
        // 从未连接到有连接：专用命令组获得内容时自动展开一次
        // （组本身常驻，用户手动收起后换服务器不再强制展开）。
        let server_appeared =
            server_cmds.is_some() && self.server_cmds_prev.is_none();
        self.server_cmds_prev = server_cmds.as_ref().map(|(sid, _)| *sid);
        self.tree.update(cx, |t, cx| {
            t.set_theme(kit, cx);
            if server_appeared {
                t.expand(KIT_GROUP_SERVER_CMDS, cx);
            }
            t.set_nodes(forest, cx);
        });
    }

    /// 顶部浮条提示（约 3.5 秒后自动消失）。
    pub(crate) fn show_notice(&mut self, msg: String, cx: &mut Context<Self>) {
        self.notice = Some((msg, Instant::now()));
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            cx.background_executor().timer(std::time::Duration::from_millis(3500)).await;
            let _ = this.update(cx, |ui, cx| {
                // 只清除未被后续提示覆盖的那一条。
                if ui.notice.as_ref().is_some_and(|(_, at)| at.elapsed() >= std::time::Duration::from_millis(3400)) {
                    ui.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---------- Tab 管理 ----------

    // ---------- 服务器表单（独立窗口） ----------

    /// 打开新增/编辑服务器表单窗口（已开则复用载入并激活）。
    /// error：打开时预设的错误提示（缺口令连接时引导输入）。
    pub(crate) fn open_server_form(
        &mut self,
        parent: i64,
        load: crate::form_service_edit::FormLoad,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let handle = match self.form_win.take() {
            Some(h) if h.is_open(cx) => h,
            _ => {
                let main = cx.entity().downgrade();
                let entity =
                    cx.new(|cx| crate::form_service_edit::ServiceEditForm::new(parent, load.clone(), main, cx));
                let spec = ui_kit::pro::dialog::DialogSpec {
                    title: crate::i18n::t(
                        cx,
                        if load.is_some() {
                            crate::i18n::Tr::EditServerTitle
                        } else {
                            crate::i18n::Tr::NewServerTitle
                        },
                    )
                    .to_string(),
                    width: 480.,
                    height: 500.,
                    min_width: 440.,
                    min_height: 400.,
                    surface_alpha: Some(theme::surface_fx(cx).alpha),
                    surface_blur: theme::bg_fx(cx).blur,
                    ..Default::default()
                };
                // 弹出窗主题跟随 app 当前主题（非硬编码深色）。
                ui_kit::pro::dialog::open_dialog_themed(
                    entity,
                    spec,
                    crate::theme::current(cx).kit(),
                    cx,
                )
            }
        };
        // 开窗建立的模糊视图需要按当前主题系列 tint（复用已开窗时幂等无害）。
        ui_kit::platform::set_blur_tint(!theme::is_light(cx));
        // 复用已开窗口重载内容；新开时重载等值无副作用。
        handle.content(cx).update(cx, |f, cx| f.load_for(parent, load, cx));
        handle.activate(cx);
        if let Some(msg) = error {
            handle.content(cx).update(cx, |f, cx| f.preset_error(msg, cx));
        }
        self.form_win = Some(handle);
    }

    /// 表单保存落库（编辑就地更新，新增插到目标父目录；树中选中作落点反馈）。
    pub(crate) fn persist_server_form(
        &mut self,
        out: crate::form_service_edit::ServerFormOutput,
        cx: &mut Context<Self>,
    ) {
        let icon = out.icon.clone();
        // relay/令牌已迁全局设置（db::iroh_global）；ServerData 的旧字段仅作兼容保留。
        let data = ServerData {
            host: out.host,
            port: out.port,
            username: out.username,
            auth: out.auth,
            mode: out.mode,
            iroh_relay: String::new(),
            iroh_token: String::new(),
            spec: String::new(),
        };
        let node_id = match out.editing {
            Some(id) => {
                self.db.update_node(id, &out.name, NodeKind::Server(data));
                id
            }
            None => self
                .db
                .insert(NewNode {
                    parent: out.parent,
                    name: out.name,
                    kind: NodeKind::Server(data),
                })
                .id,
        };
        // 图标显式落库（None = 清回按名称自动匹配）。
        self.db.set_node_icon(node_id, icon.as_deref().unwrap_or(""));
        self.selected = Some(SelectKey::Node(node_id));
        self.refresh_tree(cx);
        cx.notify();
    }

    /// 主窗口带到前台（表单窗口里发起连接后调用）。
    pub(crate) fn activate_main_window(&mut self, cx: &mut Context<Self>) {
        if let Some(wh) = self.main_window.clone() {
            let _ = wh.update(cx, |_, window, _| window.activate_window());
        }
    }

    // ---------- 命令表单（独立窗口） ----------

    /// 打开新增/编辑命令表单窗口（已开则复用载入并激活）。
    /// footer：保存（校验 → 回本窗落库）+ 取消（关窗即放弃）。
    pub(crate) fn open_command_form(
        &mut self,
        parent: i64,
        load: crate::form_command_edit::CmdFormLoad,
        cx: &mut Context<Self>,
    ) {
        let handle = match self.cmd_form_win.take() {
            Some(h) if h.is_open(cx) => h,
            _ => {
                let main = cx.entity().downgrade();
                let entity =
                    cx.new(|cx| crate::form_command_edit::CommandEditForm::new(parent, load.clone(), main, cx));
                let spec = ui_kit::pro::dialog::DialogSpec {
                    title: crate::i18n::t(
                        cx,
                        if load.is_some() {
                            crate::i18n::Tr::EditCommandTitle
                        } else {
                            crate::i18n::Tr::NewCommandTitle
                        },
                    )
                    .to_string(),
                    width: 480.,
                    height: 460.,
                    min_width: 420.,
                    min_height: 340.,
                    surface_alpha: Some(theme::surface_fx(cx).alpha),
                    surface_blur: theme::bg_fx(cx).blur,
                    ..Default::default()
                };
                // 底部工具栏：取消 | 弹性空隙 | 保存（需求定式：保存/取消）。
                let footer = ui_kit::pro::dialog::DialogFooter::new(
                    move |p: &Entity<crate::form_command_edit::CommandEditForm>,
                          closer: &ui_kit::pro::dialog::DialogCloser<
                          crate::form_command_edit::CommandEditForm,
                          crate::form_command_edit::CommandFormEvent,
                      >,
                          cx: &gpui::App| {
                        let kit = crate::theme::current(cx).kit();
                        let cancel_closer = closer.clone();
                        div()
                            .flex()
                            .gap_3()
                            .w_full()
                            .child(kit_button::button_for(
                                "cmd-form-cancel",
                                crate::i18n::t(cx, crate::i18n::Tr::Cancel),
                                ButtonKind::Secondary,
                                ButtonSize::Md,
                                true,
                                kit,
                                p.clone(),
                                move |_p, _e, _w, cx| cancel_closer.close(cx),
                            ))
                            .child(div().flex_1())
                            .child(kit_button::button_for(
                                "cmd-form-save",
                                crate::i18n::t(cx, crate::i18n::Tr::BtnSave),
                                ButtonKind::Primary,
                                ButtonSize::Md,
                                true,
                                kit,
                                p.clone(),
                                move |p, _e, _w, cx| p.save(cx),
                            ))
                            .into_any_element()
                    },
                );
                // 保存成功（Saved 事件）→ 关窗。
                ui_kit::pro::dialog::open_dialog_ex(
                    entity,
                    spec,
                    Some(footer),
                    crate::theme::current(cx).kit(),
                    Some(std::rc::Rc::new(
                        |_ev: &crate::form_command_edit::CommandFormEvent,
                          closer: &ui_kit::pro::dialog::DialogCloser<
                          crate::form_command_edit::CommandEditForm,
                          crate::form_command_edit::CommandFormEvent,
                      >,
                          cx: &mut gpui::App| {
                            closer.close(cx);
                        },
                    )),
                    cx,
                )
            }
        };
        // 开窗建立的模糊视图需要按当前主题系列 tint（复用已开窗时幂等无害）。
        ui_kit::platform::set_blur_tint(!theme::is_light(cx));
        // 复用已开窗口重载内容；新开时重载等值无副作用。
        handle.content(cx).update(cx, |f, cx| f.load_for(parent, load, cx));
        handle.activate(cx);
        self.cmd_form_win = Some(handle);
    }

    /// 命令表单保存落库（编辑就地更新，新增插到目标父目录；树中选中作落点反馈）。
    pub(crate) fn persist_command_form(
        &mut self,
        out: crate::form_command_edit::CommandFormOutput,
        cx: &mut Context<Self>,
    ) {
        let data = CommandData {
            command: out.command,
            description: out.description,
        };
        let node_id = match out.editing {
            Some(id) => {
                self.db.update_node(id, &out.title, NodeKind::Command(data));
                id
            }
            None => self
                .db
                .insert(NewNode {
                    parent: out.parent,
                    name: out.title,
                    kind: NodeKind::Command(data),
                })
                .id,
        };
        self.selected = Some(SelectKey::Node(node_id));
        self.refresh_tree(cx);
        cx.notify();
    }

    // ---------- 目录名称表单（独立窗口） ----------

    /// 打开目录编辑表单窗口（单字段名称 + footer 保存/取消；已开则复用载入）。
    pub(crate) fn open_folder_form(&mut self, id: i64, name: String, cx: &mut Context<Self>) {
        let handle = match self.folder_form_win.take() {
            Some(h) if h.is_open(cx) => h,
            _ => {
                let main = cx.entity().downgrade();
                let entity =
                    cx.new(|cx| crate::form_folder_edit::FolderEditForm::new(id, name.clone(), main, cx));
                let spec = ui_kit::pro::dialog::DialogSpec {
                    title: crate::i18n::t(cx, crate::i18n::Tr::EditFolderTitle).to_string(),
                    width: 380.,
                    height: 190.,
                    min_width: 320.,
                    min_height: 160.,
                    surface_alpha: Some(theme::surface_fx(cx).alpha),
                    surface_blur: theme::bg_fx(cx).blur,
                    ..Default::default()
                };
                let footer = ui_kit::pro::dialog::DialogFooter::new(
                    move |p: &Entity<crate::form_folder_edit::FolderEditForm>,
                          closer: &ui_kit::pro::dialog::DialogCloser<
                          crate::form_folder_edit::FolderEditForm,
                          crate::form_folder_edit::FolderFormEvent,
                      >,
                          cx: &gpui::App| {
                        let kit = crate::theme::current(cx).kit();
                        let cancel_closer = closer.clone();
                        div()
                            .flex()
                            .gap_3()
                            .w_full()
                            .child(kit_button::button_for(
                                "folder-form-cancel",
                                crate::i18n::t(cx, crate::i18n::Tr::Cancel),
                                ButtonKind::Secondary,
                                ButtonSize::Md,
                                true,
                                kit,
                                p.clone(),
                                move |_p, _e, _w, cx| cancel_closer.close(cx),
                            ))
                            .child(div().flex_1())
                            .child(kit_button::button_for(
                                "folder-form-save",
                                crate::i18n::t(cx, crate::i18n::Tr::BtnSave),
                                ButtonKind::Primary,
                                ButtonSize::Md,
                                true,
                                kit,
                                p.clone(),
                                move |p, _e, _w, cx| p.save(cx),
                            ))
                            .into_any_element()
                    },
                );
                ui_kit::pro::dialog::open_dialog_ex(
                    entity,
                    spec,
                    Some(footer),
                    crate::theme::current(cx).kit(),
                    Some(std::rc::Rc::new(
                        |_ev: &crate::form_folder_edit::FolderFormEvent,
                          closer: &ui_kit::pro::dialog::DialogCloser<
                          crate::form_folder_edit::FolderEditForm,
                          crate::form_folder_edit::FolderFormEvent,
                      >,
                          cx: &mut gpui::App| {
                            closer.close(cx);
                        },
                    )),
                    cx,
                )
            }
        };
        // 开窗建立的模糊视图需要按当前主题系列 tint（复用已开窗时幂等无害）。
        ui_kit::platform::set_blur_tint(!theme::is_light(cx));
        handle.content(cx).update(cx, |f, cx| f.load_for(id, name, cx));
        handle.activate(cx);
        self.folder_form_win = Some(handle);
    }

    /// 目录表单保存落库（就地改名，不动 parent/sort/icon）。
    pub(crate) fn persist_folder_form(
        &mut self,
        out: crate::form_folder_edit::FolderFormOutput,
        cx: &mut Context<Self>,
    ) {
        self.db.rename(out.id, &out.name);
        self.selected = Some(SelectKey::Node(out.id));
        self.refresh_tree(cx);
        cx.notify();
    }

    /// 运行命令到控制台：写入当前激活 Live tab 的 PTY（active 优先，
    /// 其次主窗口任一 Live tab / 弹出窗口的 Live tab，并前置对应窗口）。
    pub(crate) fn run_command_node(&mut self, id: i64, cx: &mut Context<Self>) {        let Some(node) = self.db.node(id) else { return };
        let NodeKind::Command(data) = &node.kind else { return };
        let cmd = data.command.trim().to_string();
        if cmd.is_empty() {
            let msg = crate::i18n::t(cx, crate::i18n::Tr::NoticeCmdEmpty).to_string();
            self.show_notice(msg, cx);
            return;
        }
        use std::io::Write as _;
        // 目标终端三级回退：激活 Live → 主窗口首个 Live → 弹出窗口首个 Live。
        let mut target = self.tabs.iter().position(|t| {
            t.id == self.active_id && matches!(t.kind, TabKind::Live { .. })
        });
        if target.is_none() {
            target = self.tabs.iter().position(|t| matches!(t.kind, TabKind::Live { .. }));
        }
        if let Some(idx) = target {
            let tab = &self.tabs[idx];
            let TabKind::Live { session, .. } = &tab.kind else { unreachable!() };
            let _ = session.writer().write_all(format!("{cmd}\n").as_bytes());
            // 切到该 tab（用户视角 = 命令落在正看着的终端里）。
            self.active_id = tab.id;
            self.refresh_tree(cx);
            cx.notify();
            return;
        }
        // 弹出独立窗口的终端：取第一个仍在线的，前置其窗口后写入。
        for pw in self.popped.values() {
            if let Some(tab) = pw.entity.read(cx).tab.as_ref() {
                if let TabKind::Live { session, .. } = &tab.kind {
                    let _ = session.writer().write_all(format!("{cmd}\n").as_bytes());
                    let _ = pw.window.update(cx, |_, window, _| window.activate_window());
                    return;
                }
            }
        }
        let msg = crate::i18n::t(cx, crate::i18n::Tr::NoticeNoConsole).to_string();
        self.show_notice(msg, cx);
    }

    pub(crate) fn open_connecting_tab(
        &mut self,
        title: String,
        params: ConnectParams,
        icon: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // 去重：该主机已有连接中/已连接的 tab 则直接跳转，避免重复开会话
        if let Some(existing) = self.tabs.iter().find(|t| match &t.kind {
            TabKind::Live { session, .. } => {
                session.params.host == params.host && session.params.port == params.port
            }
            TabKind::Connecting { params: p } => p.host == params.host && p.port == params.port,
            _ => false,
        }) {
            self.activate_tab(existing.id, cx);
            return;
        }
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab {
            id: tab_id,
            title,
            icon,
            kind: TabKind::Connecting { params },
        });
        self.activate_tab(tab_id, cx);
        self.refresh_tree(cx);
        self.connect_tab(tab_id, cx);
    }

    /// 终端配置：Menlo 默认字体 + 终端配色模板（独立于界面主题）+ 本地着色模板 + 四边 4px 内边距。
    fn terminal_config(&self, cx: &gpui::App) -> TerminalConfig {
        TerminalConfig {
            font_family: self.term_font_family.clone(),
            font_size: px(self.term_font_size),
            padding: gpui::Edges::all(px(4.0)),
            colors: {
                let fx = theme::surface_fx(cx);
                crate::theme::term_for(self.term_theme_idx)
                    .to_gpui_with_frost(fx.alpha, fx.dim, fx.lift)
            },
            style: self.term_style.clone(),
            ..Default::default()
        }
    }

    /// 配色/着色模板/字体变化后，把新外观推入所有已打开的 Live 终端。
    pub(crate) fn refresh_term_theme(&mut self, cx: &mut Context<Self>) {
        let colors = {
            let fx = theme::surface_fx(cx);
            crate::theme::term_for(self.term_theme_idx).to_gpui_with_frost(fx.alpha, fx.dim, fx.lift)
        };
        let style = self.term_style.clone();
        let family = self.term_font_family.clone();
        let size = px(self.term_font_size);
        // 收集主窗口与弹出独立窗口的全部 Live 终端（弹出窗的终端实体随 tab
        // 移动，不推进去的话切主题/透明度时它们保持旧外观）。
        let mut terms: Vec<Entity<TerminalView>> = Vec::new();
        for tab in &self.tabs {
            if let TabKind::Live { term, .. } = &tab.kind {
                terms.push(term.clone());
            }
        }
        for pw in self.popped.values() {
            if let Some(tab) = pw.entity.read(cx).tab.as_ref() {
                if let TabKind::Live { term, .. } = &tab.kind {
                    terms.push(term.clone());
                }
            }
        }
        for term in terms {
            term.update(cx, |t, cx| {
                let mut cfg = t.config().clone();
                cfg.colors = colors.clone();
                cfg.style = style.clone();
                cfg.font_family = family.clone();
                cfg.font_size = size;
                t.update_config(cfg, cx);
            });
        }
    }

    /// 设置窗外观变更事件：从 DB 重载配色/着色模板/字体并推入已开终端。
    pub(crate) fn reload_terminal_appearance(&mut self, cx: &mut Context<Self>) {
        let series = theme::ui_series(self.theme_idx);
        if let Some(idx) = self
            .db
            .get_setting(series.setting_key())
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&i| theme::term_for(i).series == series)
        {
            self.term_theme_idx = idx;
        }
        self.term_style = crate::settings_view::load_style(&self.db);
        if let Some(f) = self.db.get_setting("terminal.font.family") {
            if !f.is_empty() {
                self.term_font_family = f;
            }
        }
        self.term_font_size = crate::settings_view::normalize_font_size(
            self.db.get_setting("terminal.font.size").and_then(|v| v.parse::<f32>().ok()),
        );
        self.refresh_term_theme(cx);
        cx.notify();
    }

    /// 打开 app 设置弹出窗（终端配色 + 着色模板；后续设置项在此扩展）。
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        crate::settings_view::open_settings(&cx.entity(), cx);
    }

    /// 打开「关于」对话框（固定尺寸；已开时复用并带到前台）。
    pub(crate) fn open_about(&mut self, cx: &mut Context<Self>) {
        if let Some(h) = self.about_win.take() {
            if h.is_open(cx) {
                h.activate(cx);
                self.about_win = Some(h);
                return;
            }
        }
        let view = cx.new(|cx| crate::about::AboutView::new(cx));
        let spec = ui_kit::pro::dialog::DialogSpec {
            title: crate::i18n::t(cx, crate::i18n::Tr::MenuAbout).to_string(),
            width: 360.,
            height: 300.,
            min_width: 360.,
            min_height: 300.,
            resizable: false,
            surface_alpha: Some(theme::surface_fx(cx).alpha),
            surface_blur: theme::bg_fx(cx).blur,
            ..Default::default()
        };
        let handle =
            ui_kit::pro::dialog::open_dialog_themed(view, spec, theme::current(cx).kit(), cx);
        // 开窗建立的模糊视图需要按当前主题系列 tint（幂等）。
        ui_kit::platform::set_blur_tint(!theme::is_light(cx));
        self.about_win = Some(handle);
    }

    /// 切换界面主题。系列（深/浅）翻转时终端配色联动到该系列记忆值；
    /// 系列内切换不动终端配色——两套配置独立、仅黑白系列同步。
    pub(crate) fn set_ui_theme(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.theme_idx = idx.min(theme::THEMES.len() - 1);
        self.db.set_setting("theme", &format!("{}", self.theme_idx));
        let series = theme::ui_series(self.theme_idx);
        if theme::term_for(self.term_theme_idx).series != series {
            self.term_theme_idx = self
                .db
                .get_setting(series.setting_key())
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|&i| theme::term_for(i).series == series)
                .unwrap_or_else(|| theme::term_default(series));
            // 终端配色/背景 alpha 刷新由下方 set_global(ThemeGlobal) 的观察者统一执行。
        }
        cx.set_global(theme::ThemeGlobal(self.theme_idx));
        self.refresh_tree(cx);
        self.refresh_dir_tree(cx);
        cx.notify();
    }

    /// 后台执行连接；成功将 tab 置为 Live，失败置为 Failed。订阅关闭事件。
    /// 连接各阶段经 [`ConnectStep`] 实时汇入 `connect_log`（连接中/失败页展示）。
    fn connect_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else { return };
        let TabKind::Connecting { params } = &tab.kind else { return };
        let params = params.clone();
        let title = tab.title.clone();

        self.connect_log.insert(tab_id, Vec::new());
        cx.spawn(async move |this: WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
            let (step_tx, mut step_rx) = tokio::sync::mpsc::unbounded_channel::<ConnectStep>();
            let started = std::time::Instant::now();
            // cx 以参数传入（闭包不持有借用），后续 this.update 不与之冲突。
            let record = |step: ConnectStep, cx: &mut gpui::AsyncApp| {
                let ms = started.elapsed().as_millis() as u32;
                let _ = this.update(cx, |ui: &mut AppUi, cx: &mut Context<AppUi>| {
                    ui.connect_log.entry(tab_id).or_default().push((ms, step));
                    cx.notify();
                });
            };
            let mut task = super::RUNTIME
                .get()
                .expect("tokio runtime")
                .spawn(open_session_traced(params.clone(), Some(step_tx)));
            // 进度事件与连接结果并发汇流：步骤实时落日志，通道关闭即收尾。
            let result = loop {
                tokio::select! {
                    step = step_rx.recv() => match step {
                        Some(step) => record(step, cx),
                        None => break task.await.expect("join open_session"),
                    },
                    res = &mut task => break res.expect("join open_session"),
                }
            };
            while let Ok(step) = step_rx.try_recv() {
                record(step, cx);
            }

            let _ = this.update(cx, |ui: &mut AppUi, cx: &mut Context<AppUi>| {
                let config = ui.terminal_config(cx);
                let Some(tab) = ui.tabs.iter_mut().find(|t| t.id == tab_id) else {
                    // tab 已被用户关闭：立即释放会话。
                    if let Ok(opened) = result {
                        opened.session.close();
                    }
                    return;
                };
                match result {
                    Ok(opened) => {
                        let session = opened.session;
                        let writer = session.writer();
                        let reader = opened.reader;
                        let stats = session.stats.clone();

                        let resize_session = session.clone();
                        // cwd 跟随（OSC 7 / 提示符识别，fork 侧双来源）→ 目录面板；
                        // 回调经弱引用回宿主，弹出窗口里的终端同样生效。
                        let cwd_app = cx.entity().downgrade();
                        let cwd_tab = tab_id;
                        let term = cx.new(|cx| {
                            TerminalView::new(writer, reader, config, cx)
                                .with_resize_callback(move |cols, rows| {
                                    resize_session.resize(cols as u16, rows as u16)
                                })
                                .with_cwd_callback(move |cx, path| {
                                    let _ = cwd_app.update(cx, |ui, cx| {
                                        ui.on_terminal_cwd(cwd_tab, path, cx)
                                    });
                                })
                        });

                        // 会话关闭 → tab 转 Failed。
                        let mut events = opened.events;
                        cx.spawn(async move |this2: WeakEntity<AppUi>, cx2: &mut gpui::AsyncApp| {
                            while let Some(ev) = events.recv().await {
                                if matches!(ev, SessionEvent::Closed { .. }) {
                                    let _ = this2.update(cx2, |ui, cx| {
                                        ui.mark_tab_closed(tab_id, crate::i18n::t(cx, crate::i18n::Tr::ReasonConnClosed), cx);
                                    });
                                    break;
                                }
                            }
                        })
                        .detach();

                        // P2P 网络探测材料先取好（tab 借用在后续 ui.* 调用前结束）。
                        let probe_args = match params.transport.clone() {
                            Transport::Iroh { relay, token } => {
                                Some((params.host.clone(), relay, token, session.clone()))
                            }
                            Transport::Tcp => None,
                        };
                        tab.kind = TabKind::Live {
                            term,
                            session,
                            opened_at: Instant::now(),
                        };
                        // 目录面板：登记会话状态；初始 pwd/清单只在目录页可见时
                        // 取（零流量原则），隐藏时回页由 maybe_fetch_dir 补。
                        ui.ensure_dir_data(tab_id, &params);
                        ui.maybe_fetch_dir(cx);
                        let _ = stats;
                        let _ = title;
                        // 连接成功即收掉过程日志；P2P 会话另起网络探测（链路/双端出口）。
                        // 先登记空条目：probe 只更新已存在条目，tab 关闭清条目即探测自停。
                        ui.connect_log.remove(&tab_id);
                        if let Some((node, relay, token, probe_session)) = probe_args {
                            ui.net_data.entry(tab_id).or_default();
                            cx.spawn(async move |this3, cx3| {
                                crate::netinfo::probe(
                                    tab_id, node, relay, token, probe_session, this3, cx3,
                                )
                                .await;
                            })
                            .detach();
                        }
                    }
                    Err(e) => {
                        // 失败保留过程日志（失败页展示，便于定位卡在哪个阶段）。
                        tab.kind = TabKind::Failed { params, error: e.to_string() };
                    }
                }
                ui.refresh_dir_tree(cx);
                // 连接态变化：服务器行圆点与专用命令组随之更新。
                ui.refresh_tree(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 把 Live tab 弹出为独立窗口（移动语义：主窗口 tab 移除，关窗时送回）。
    pub(crate) fn pop_out_tab(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        if !matches!(self.tabs[idx].kind, TabKind::Live { .. }) {
            return;
        }
        let tab = self.tabs.remove(idx);
        if self.active_id == id {
            // 弹出的是激活 tab：回最近使用的剩余 tab（无记录回第一个）。
            let next = self
                .tab_mru
                .iter()
                .find(|&&x| x != id && self.tabs.iter().any(|t| t.id == x))
                .copied()
                .or_else(|| self.tabs.first().map(|t| t.id));
            if let Some(n) = next {
                self.activate_tab(n, cx);
            } else {
                self.active_id = 0;
            }
        }
        self.tab_mru.retain(|&x| x != id);
        let tab_id = tab.id;
        let params = match &tab.kind {
            TabKind::Live { session, .. } => session.params.clone(),
            _ => unreachable!("上面已过滤"),
        };
        let title: SharedString = tab.title.clone().into();
        let bounds = Bounds::centered(None, gpui::size(px(920.), px(600.)), cx);
        let bg_appearance = theme::bg_fx(cx).window_appearance();
        let main = cx.entity().downgrade();
        // 滚动条 owner 避开主窗口的 1（tree）/2（term）。
        let entity = cx.new(|cx| {
            crate::popped::TerminalStandalone::new(tab, main, 100 + tab_id as usize, cx)
        });
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(gpui::Size {
                        width: px(480.),
                        height: px(320.),
                    }),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title),
                        appears_transparent: false,
                        traffic_light_position: None,
                    }),
                    window_background: bg_appearance,
                    ..Default::default()
                },
                |_window, _cx| entity.clone(),
            )
            .expect("open pop-out window");
        // 开窗建立的模糊视图需要按当前主题系列 tint（幂等，全局枚举）。
        ui_kit::platform::set_blur_tint(!theme::is_light(cx));
        self.popped.insert(
            tab_id,
            crate::popped::PoppedWin { window: handle, entity, params },
        );
        self.refresh_tree(cx);
        self.maybe_fetch_dir(cx);
        cx.notify();
    }

    /// 独立窗口的 tab 送回主窗口（并入主窗口 / 关窗回调统一入口）。
    pub(crate) fn reaccept_tab(
        &mut self,
        mut tab: Tab,
        closed: Option<(ConnectParams, String)>,
        cx: &mut Context<Self>,
    ) {
        let id = tab.id;
        if let Some((params, error)) = closed {
            tab.kind = TabKind::Failed { params, error };
        }
        // 送回的会话仍在线则补登记目录数据；弹出期间 cwd 可能已变（OSC 7 持续
        // 上报）而清单从未取过——目录页可见时由 maybe_fetch_dir 补取并重建树。
        if let TabKind::Live { session, .. } = &tab.kind {
            let p = session.params.clone();
            self.ensure_dir_data(id, &p);
        }
        self.popped.remove(&id);
        self.tabs.push(tab);
        self.active_id = id;
        self.refresh_tree(cx);
        self.maybe_fetch_dir(cx);
        cx.notify();
    }

    /// 独立窗口被关闭（红绿灯等）后收回其中的 tab。
    /// 由 main 的 on_window_closed 回调驱动；merge_back 路径已先摘除登记，不会重复。
    pub(crate) fn reattach_dead_windows(&mut self, cx: &mut Context<Self>) {
        let dead: Vec<u64> = self
            .popped
            .iter()
            .filter(|(_, pw)| pw.window.update(cx, |_, _, _| ()).is_err())
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            if let Some(pw) = self.popped.remove(&id) {
                let (tab, closed) = pw.entity.update(cx, |s, _| (s.tab.take(), s.closed.take()));
                if let Some(tab) = tab {
                    self.reaccept_tab(tab, closed, cx);
                }
            }
        }
    }

    fn mark_tab_closed(&mut self, tab_id: u64, reason: &str, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            match &tab.kind {
                TabKind::Live { session, .. } => {
                    let params = session.params.clone();
                    tab.kind = TabKind::Failed { params, error: reason.to_string() };
                    self.refresh_tree(cx);
                    self.refresh_dir_tree(cx);
                    cx.notify();
                }
                _ => {}
            }
            return;
        }
        // 弹出窗口中的会话断开：记录原因并关窗，on_window_closed 回调随即转 Failed 收回。
        if let Some(pw) = self.popped.get(&tab_id).cloned() {
            if let Some(params) = pw.entity.read(cx).live_params() {
                let _ = pw
                    .entity
                    .update(cx, |s, _| s.closed = Some((params, reason.to_string())));
            }
            let _ = pw.window.update(cx, |_, window, _| window.remove_window());
        }
    }

    pub fn close_active_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_by_id(self.active_id, cx);
    }

    fn close_tab_by_id(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            if let TabKind::Live { session, .. } = &self.tabs[pos].kind {
                session.close();
            }
            self.tabs.remove(pos);
            // 目录/系统信息/传输数据随会话释放（tab id 不复用，直接删）。
            self.dir_data.remove(&id);
            self.sys_data.remove(&id);
            self.connect_log.remove(&id);
            self.net_data.remove(&id);
            if let Some(items) = self.transfer_data.remove(&id) {
                for it in items {
                    it.ticket.cancel();
                }
            }
            if self.tabs.is_empty() {
                // 允许空：中间显示空态提示
                cx.notify();
            } else if self.active_id == id {
                // 回最近使用的剩余 tab（无记录回邻近位）。
                let next = self
                    .tab_mru
                    .iter()
                    .find(|&&x| x != id && self.tabs.iter().any(|t| t.id == x))
                    .copied()
                    .unwrap_or_else(|| self.tabs[pos.min(self.tabs.len() - 1)].id);
                self.activate_tab(next, cx);
            }
            self.tab_mru.retain(|&x| x != id);
            self.maybe_fetch_dir(cx);
            // 在线圆点 / 专用命令组随连接消失。
            self.refresh_tree(cx);
            cx.notify();
        }
    }

    /// ⌘T：打开新增服务器表单窗口（表单已从 tab 迁到独立窗口）。
    pub fn new_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.open_server_form(ROOT_SERVERS, None, None, cx);
    }

    fn cycle_tab(&mut self, dir: i32, cx: &mut Context<Self>) {
        if self.tabs.len() < 2 {
            return;
        }
        let pos = self.tabs.iter().position(|t| t.id == self.active_id).unwrap_or(0);
        let next = (pos as i32 + dir).rem_euclid(self.tabs.len() as i32) as usize;
        self.activate_tab(self.tabs[next].id, cx);
        self.refresh_tree(cx);
        cx.notify();
    }

    pub fn toggle_left(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.left_open = !self.left_open;
        cx.notify();
    }

    pub fn toggle_right(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.right_open = !self.right_open;
        // 展开后按当前页立即取数；收起期间各页零取数（ticker/回页入口再续）。
        if self.right_open {
            match self.right_page {
                RightPage::Dir => self.maybe_fetch_dir(cx),
                RightPage::SysInfo => self.maybe_probe_sysinfo(cx),
                RightPage::Processes => self.maybe_probe_procs(cx),
                RightPage::Sessions => {}
            }
        }
        cx.notify();
    }

    /// 工具按钮选择右面板页：点激活中的工具 = 收起面板；点其它 = 切页并展开。
    /// 切到对应页立即取数，不等 ticker / 后续交互。
    pub(crate) fn select_right_page(&mut self, page: RightPage, cx: &mut Context<Self>) {
        if self.right_open && self.right_page == page {
            self.right_open = false;
        } else {
            self.right_open = true;
            self.right_page = page;
            match page {
                RightPage::Dir => self.maybe_fetch_dir(cx),
                RightPage::SysInfo => self.maybe_probe_sysinfo(cx),
                RightPage::Processes => self.maybe_probe_procs(cx),
                RightPage::Sessions => {}
            }
        }
        cx.notify();
    }

    fn spawn_status_ticker(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let Ok(()) = this.update(cx, |ui, cx| {
                    if ui.right_open {
                        // 系统信息页可见时驱动探针（在飞自动跳过）。
                        ui.maybe_probe_sysinfo(cx);
                        // 进程页探针自带 sleep 1，maybe 内另按 4s 节流。
                        ui.maybe_probe_procs(cx);
                        // 传输区：速度差分 + 完成回执（上传完刷新目录树）。
                        ui.tick_transfers(cx);
                        cx.notify();
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();
    }

    // ---------- 连接入口 ----------

    pub fn connect_mock(&mut self, cx: &mut Context<Self>) {
        // mock 服务器懒启动，多 tab 复用。
        if let Some(server) = &self.mock {
            self.open_connecting_tab("mock".into(), mock_params(server.port), Some("server".into()), cx);
            return;
        }
        cx.spawn(async move |this: WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
            let spawned = super::RUNTIME
                .get()
                .expect("tokio runtime")
                .spawn(mock::spawn())
                .await
                .expect("join mock spawn");
            let _ = this.update(cx, |ui: &mut AppUi, cx: &mut Context<AppUi>| match spawned {
                Ok(server) => {
                    ui.open_connecting_tab(
                        "mock".into(),
                        mock_params(server.port),
                        Some("server".into()),
                        cx,
                    );
                    ui.mock = Some(server);
                }
                Err(e) => {
                    // mock 失败只打日志（表单已是独立窗口，无处回显）。
                    let _ = (ui, cx);
                    eprintln!("mock 启动失败: {e}");
                }
            });
        })
        .detach();
    }

    /// 当前配色（含表面透明度）：`pal.x` 直读，表面色经 `pal.surface(pal.bg)`。
    pub(crate) fn theme(&self) -> theme::PalFx {
        theme::PalFx::build(
            &theme::THEMES[self.theme_idx.min(theme::THEMES.len() - 1)].1,
            self.bg_params,
        )
    }

    /// 中间区可用宽度：窗口宽 − 侧栏 − 右面板 − 右缘工具栏（收起的面板计 0）。
    /// render/render_center/render_form 三处共用，保证扣减口径一致。
    pub(crate) fn center_w(&self, win_w: f32) -> f32 {
        let sb = if self.left_open { self.sidebar_w } else { 0.0 };
        let rp = if self.right_open { self.right_w } else { 0.0 };
        (win_w - sb - rp - RIGHT_TOOLBAR_W).max(0.)
    }

    // ---------- 渲染 ----------

    /// 顶部居中浮条提示（show_notice 触发；超时自动消失，纯展示不拦截交互）。
    fn render_notice(&self, win_w: f32) -> Option<AnyElement> {
        let (msg, at) = self.notice.as_ref()?;
        if at.elapsed() >= std::time::Duration::from_millis(3500) {
            return None;
        }
        let pal = self.theme();
        let kit = pal.kit();
        Some(
            div()
                .id("app-notice-layer")
                .absolute()
                .top(px(TITLEBAR_H + 8.))
                .left_0()
                .w(px(win_w))
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("app-notice")
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(30.))
                        .px_4()
                        .rounded_md()
                        // 浮层不吃整窗模糊：底色不透明。
                        .bg(rgb(kit.panel))
                        .border_1()
                        .border_color(rgb(kit.border))
                        .shadow_md()
                        .text_size(px(12.))
                        .text_color(rgb(pal.text))
                        .child(svg_icon(icons::INFO, 14., pal.warn))
                        .child(div().child(msg.clone())),
                )
                .into_any_element(),
        )
    }

    /// 删除确认模态（kit Modal）：取消/Esc/点遮罩关闭，删除执行 perform_delete。
    fn render_confirm_modal(&self, cx: &Context<Self>) -> Option<AnyElement> {        let c = self.confirm_delete.as_ref()?;
        let kit = self.theme().kit();
        let body = div()
            .flex()
            .flex_col()
            .gap_5()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(self.theme().muted))
                            .child(crate::i18n::t(cx, crate::i18n::Tr::DeleteConfirmBody)),
                    )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .justify_end()
                    .child(kit_button::button(
                        "confirm-cancel",
                        crate::i18n::t(cx, crate::i18n::Tr::Cancel),
                        ButtonKind::Secondary,
                        ButtonSize::Md,
                        true,
                        kit,
                        cx,
                        |this, _ev, _window, cx| {
                            this.confirm_delete = None;
                            cx.notify();
                        },
                    ))
                    .child(
                        div()
                            .id("confirm-delete-wrap")
                            .debug_selector(|| "confirm-delete-btn".to_string())
                            .child(kit_button::button(
                                "confirm-delete",
                                crate::i18n::t(cx, crate::i18n::Tr::Delete),
                                ButtonKind::Danger,
                                ButtonSize::Md,
                                true,
                                kit,
                                cx,
                                |this, _ev, _window, cx| this.perform_delete(cx),
                            )),
                    ),
            );
        Some(ui_kit::base::modal::overlay(
            &c.modal,
            &crate::i18n::tf(cx, crate::i18n::Tr::DeleteConfirmTitle, &[&c.name]),
            340.,
            body.into_any_element(),
            kit,
            cx,
            |this, _window, cx| {
                this.confirm_delete = None;
                cx.notify();
            },
        ))
    }

    /// tab 条溢出布局：逐 tab 测自然宽（标题按渲染同款字体 shape，其余为固定
    /// 件 = px_3×2 + gap_1×2 + 前缀图标/圆点 + 关闭钮 + 激活边框余量），判定
    /// 是否溢出、维护滚动上限，并在激活 tab 变化时把它滚入视野。
    /// 返回是否溢出（决定 ‹ › 按钮与视口裁切是否登场）。
    fn layout_tabs(&mut self, window: &Window, main_w: f32) -> bool {
        let mut font = window.text_style().font();
        font.family = "System UI".into();
        // 与 tab_button 同口径：标题 14 字截断 + 12.5px。
        let widths: Vec<f32> = self
            .tabs
            .iter()
            .map(|t| {
                let title: String = t.title.chars().take(14).collect();
                let run = gpui::TextRun {
                    len: title.len(),
                    font: font.clone(),
                    color: Default::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let text_w = f32::from(
                    window
                        .text_system()
                        .shape_line(title.into(), px(12.5), &[run], None)
                        .width,
                );
                let lead = if t.icon.is_some() { 13. } else { 7. };
                // 测宽略偏松无害（滚动范围稍宽）；偏紧会让绝对定位 strip 估宽
                // 不足、末位 tab 露不全，故向上取整再加边框余量。
                (24. + 8. + lead + text_w.ceil() + 16. + 2.).min(190.)
            })
            .collect();
        const GAP: f32 = 4.;
        let content_w =
            widths.iter().sum::<f32>() + GAP * self.tabs.len().saturating_sub(1) as f32;
        // 侧栏收起时标题栏头部占位 = 交通灯让位 + 展开按钮。
        let left_inset = if self.left_open { 0. } else { TRAFFIC_LIGHTS_W + 26. };
        let avail = (main_w - left_inset - 2. * TAB_STRIP_EDGE).max(0.);
        let overflow = content_w > avail;
        let view_w = if overflow { (avail - 2. * TAB_ARROW_W).max(0.) } else { avail };
        self.tab_scroll_max = (content_w - view_w).max(0.);
        if !overflow {
            self.tab_scroll_x = 0.;
            // 未溢出也刷新聚焦标记：避免陈旧标记在「溢出刚发生」时误判跳滚。
            self.tab_scroll_focus = self.active_id;
        } else {
            if self.tab_scroll_focus != self.active_id {
                self.tab_scroll_focus = self.active_id;
                let mut x = 0.;
                for (t, w) in self.tabs.iter().zip(&widths) {
                    if t.id == self.active_id {
                        if x < self.tab_scroll_x {
                            self.tab_scroll_x = x;
                        }
                        if x + w > self.tab_scroll_x + view_w {
                            self.tab_scroll_x = x + w - view_w;
                        }
                        break;
                    }
                    x += w + GAP;
                }
            }
            self.tab_scroll_x = self.tab_scroll_x.clamp(0., self.tab_scroll_max);
        }
        overflow
    }

    fn tab_button(&self, tab: &Tab, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let id = tab.id;
        let active = id == self.active_id;
        let dot = tab.state_dot(&pal);
        let title: String = tab.title.chars().take(14).collect();
        let mut btn = div()
            .id(("tab", id as usize))
            .flex()
            .items_center()
            .gap_1()
            // 标题栏总高 38 不变：tab 高 30 + 顶部留 8，底边与标题栏底边齐平。
            // flex_none：溢出时由视口裁切 + ‹ › 滚动，tab 自身永不压缩。
            .h(px(30.))
            .mt(px(8.))
            .flex_none()
            .px_3()
            .max_w(px(190.))
            .overflow_hidden()
            .text_size(px(12.5))
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                this.activate_tab(id, cx);
                // 专用命令组跟随激活 tab 的服务器。
                this.refresh_tree(cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                    this.menu = Some(crate::sidebar::ContextMenu {
                        origin: ev.position,
                        target: crate::sidebar::MenuTarget::Tab(id),
                    });
                    cx.notify();
                }),
            );
        if active {
            // 浏览器式 tab：上圆角 + 三边描边 + 内容区同色，与下方控制台连成一体
            btn = btn
                .bg(pal.surface(pal.bg))
                .text_color(rgb(pal.text))
                .border_t_1()
                .border_l_1()
                .border_r_1()
                .border_color(rgb(pal.border))
                .rounded_tl(px(8.))
                .rounded_tr(px(8.));
        } else {
            btn = btn.text_color(rgb(pal.muted)).hover(|s| s.bg(pal.surface(pal.card)).rounded_tl(px(8.)).rounded_tr(px(8.)));
        }
        // 前缀图标与侧栏节点一致；状态以图标着色表达（无图标回退圆点）
        let leading = match &tab.icon {
            Some(icon_name) => gpui::svg()
                .path(icons::asset_path(icon_name))
                .size(px(13.))
                .flex_none()
                .text_color(rgb(dot))
                .into_any_element(),
            None => div().size(px(7.)).flex_none().rounded_full().bg(rgb(dot)).into_any_element(),
        };
        btn.child(leading)
            .child(div().child(title))
            .child(
                div()
                    .id(("tab-close", id as usize))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .rounded_sm()
                    .text_color(rgb(pal.dim))
                    .text_size(px(11.))
                    .hover(|s| s.bg(rgb(pal.border)).text_color(rgb(pal.text)))
                    .child("×")
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        // 阻止冒泡：父级 tab 按钮的激活会把已关闭的 tab 重新置为激活。
                        cx.stop_propagation();
                        this.close_tab_by_id(id, cx);
                    })),
            )
            .into_any_element()
    }


    fn render_titlebar(&self, tabs_overflow: bool, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let tabs: Vec<AnyElement> = self.tabs.iter().map(|t| self.tab_button(t, cx)).collect();
        // tab 条视口：相对定位裁切容器，内部 strip 绝对定位按 -tab_scroll_x
        // 平移——tab 保持自然宽（flex_none 不收缩），溢出部分被视口裁掉。
        let viewport = div()
            .id("tab-strip-viewport")
            .relative()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .id("tab-strip-inner")
                    .absolute()
                    .top_0()
                    .left(px(-self.tab_scroll_x))
                    .h_full()
                    .flex()
                    .items_end()
                    .gap_1()
                    .children(tabs),
            );
        // tab 条容器吃满标题栏剩余宽度：两侧 TAB_STRIP_EDGE 外边距（tab 之间
        // gap_1 不变）；溢出时视口两侧出 ‹ › 移动按钮。右侧剩余空白（未来窗口
        // 拖拽区）归入本容器；面板开合按钮在右缘全高工具栏。
        let mut strip = div()
            .id("tab-strip")
            .flex()
            .items_end()
            .flex_1()
            .h_full()
            .pl(px(TAB_STRIP_EDGE))
            .pr(px(TAB_STRIP_EDGE))
            .overflow_hidden();
        strip = if tabs_overflow {
            let (at_left, at_right) = (
                self.tab_scroll_x <= 0.5,
                self.tab_scroll_x >= self.tab_scroll_max - 0.5,
            );
            strip
                .child(self.tab_scroll_arrow(true, at_left, cx))
                .child(viewport)
                .child(self.tab_scroll_arrow(false, at_right, cx))
        } else {
            strip.child(viewport)
        };

        div()
            .flex()
            .items_end()
            .h(px(TITLEBAR_H))
            .bg(pal.surface(pal.panel))
            .border_b_1()
            .border_color(rgb(pal.divider()))
            .flex_none()
            // 侧栏收起时为 macOS 交通灯留位，并在其后放「展开侧栏」按钮；
            // 侧栏打开时交通灯落在侧栏顶部，tab 从中间区左缘开始。
            // 按钮在 h_full 包装内垂直居中——与侧栏打开态的顶部区同几何，
            // 开合切换位置零跳动。
            .children((!self.left_open).then(|| {
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .h_full()
                    .child(div().w(px(TRAFFIC_LIGHTS_W)).flex_none().h_full())
                    .child(
                        div()
                            .id("tb-expand-left")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(26.))
                            .rounded_md()
                            .bg(pal.surface(pal.card))
                            .border_1()
                            .border_color(rgb(pal.border))
                            .shadow_sm()
                            .text_size(px(13.))
                            .text_color(rgb(pal.muted))
                            .hover(|s| s.bg(rgb(pal.border)).text_color(rgb(pal.text)))
                            .child("◧")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                this.left_open = true;
                                cx.notify();
                            })),
                    )
            }))
            // tab 条（新增入口在左侧树与 ⌘T，tab 条不再放「＋」）
            .child(strip)
            .into_any_element()
    }

    /// tab 条溢出时的左右移动按钮（到达边界呈禁用态；svg 必须显式传色，
    /// 见 [`svg_icon`] 注释）。
    fn tab_scroll_arrow(&self, left: bool, disabled: bool, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let icon = if left { "chevron-left" } else { "chevron-right" };
        let glyph = |color: u32| {
            gpui::svg()
                .path(icons::asset_path(icon))
                .size(px(13.))
                .flex_none()
                .text_color(rgb(color))
        };
        let mut btn = div()
            .id(if left { "tab-scroll-left" } else { "tab-scroll-right" })
            .flex()
            .items_center()
            .justify_center()
            .w(px(TAB_ARROW_W))
            .h_full()
            .flex_none();
        if disabled {
            btn = btn.child(glyph(pal.dim));
        } else {
            let dir = if left { -1.0 } else { 1.0 };
            btn = btn
                .cursor(CursorStyle::PointingHand)
                .hover(|s| s.bg(pal.surface(pal.card)))
                .child(glyph(pal.muted))
                .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                    this.tab_scroll_x = (this.tab_scroll_x + dir * TAB_SCROLL_STEP)
                        .clamp(0., this.tab_scroll_max);
                    cx.notify();
                }));
        }
        btn.into_any_element()
    }

    pub(crate) fn host_is_live(&self, host: &str, port: u16) -> bool {
        let in_tabs = self.tabs.iter().any(|t| match &t.kind {
            TabKind::Live { session, .. } => {
                session.params.host == host && session.params.port == port
            }
            _ => false,
        });
        in_tabs
            || self
                .popped
                .values()
                .any(|pw| pw.params.host == host && pw.params.port == port)
    }

    fn render_right(&self, win_h: f32, cx: &Context<Self>) -> AnyElement {
        match self.right_page {
            RightPage::Dir => self.render_right_dir(win_h, cx),
            RightPage::SysInfo => self.render_sys_panel(win_h, cx),
            RightPage::Processes => self.render_procs_panel(win_h, cx),
            RightPage::Sessions => self.render_sessions_panel(win_h, cx),
        }
    }

    /// 右面板·目录页（标题行 + 路径行 + 树/传输区，中间分割线可拖——
    /// 同侧栏「树/快捷栏」模式；会话状态独立成页见 [`Self::render_sessions_panel`]）。
    fn render_right_dir(&self, win_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        // 传输区有效高度（窗口过矮时钳制，持久值不动）；树区底部让位。
        let transfer_h =
            self.dir_transfer_h.min(crate::dir_panel::dir_transfer_max_h(win_h));
        div()
            .id("right-panel")
            .absolute()
            .top_0()
            // 钉在右缘工具栏左侧（工具栏常驻占最右 RIGHT_TOOLBAR_W）
            .right(px(RIGHT_TOOLBAR_W))
            .w(px(self.right_w))
            .h(px(win_h))
            .flex_col()
            .bg(pal.surface(pal.panel))
            .border_l_1()
            .border_color(rgb(pal.divider()))
            .overflow_hidden()
            .child(self.render_dir_title(cx))
            .child(self.render_dir_path(cx))
            .child(
                self.render_dir_area(transfer_h + crate::dir_panel::DIR_SPLITTER_H, cx),
            )
            .child(self.render_dir_splitter(transfer_h, cx))
            .child(self.render_dir_transfer(transfer_h, cx))
            // 左缘宽度拖柄（最上层）
            .child(self.render_right_width_handle(cx))
            .into_any_element()
    }

    /// 右面板·会话状态页：按 tab 列连接状态/流量/时长；结构与系统信息页
    /// 同构：标题行 + 滚动内容 + 宽度拖柄。
    fn render_sessions_panel(&self, win_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let mut rows: Vec<AnyElement> = Vec::new();
        for tab in &self.tabs {
            let (color, state) = tab.state_text(&pal, self.loc);
            let (down, up, dur) = match &tab.kind {
                TabKind::Live { session, opened_at, .. } => {
                    let (i, o) = session.stats.bytes();
                    (fmt_bytes(i), fmt_bytes(o), fmt_duration(self.loc, opened_at.elapsed().as_secs()))
                }
                _ => ("—".into(), "—".into(), "—".into()),
            };
            let active = tab.id == self.active_id;
            let stat_tab_id = tab.id;
            let mut row = div()
                    .id(("stat", tab.id as usize))
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .cursor_pointer()
                    .border_color(rgb(pal.divider()))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                        this.activate_tab(stat_tab_id, cx);
                    }));
            if active {
                row = row.bg(rgb(pal.border));
            }
            let mut row = row
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(rgb(color)))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(pal.text))
                                    .child(tab.title.chars().take(16).collect::<String>()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.dim))
                                    .flex()
                                    .justify_end()
                                    .child(state),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(10.5))
                            .text_color(rgb(pal.muted))
                            .child(format!("↓ {down}"))
                            .child(format!("↑ {up}"))
                            .child(format!("⏱ {dur}")),
                    );
            // P2P 会话：链路（直连/中继）+ 双端公网出口。
            if let TabKind::Live { session, .. } = &tab.kind {
                if matches!(session.params.transport, Transport::Iroh { .. }) {
                    for line in self.net_rows(tab.id) {
                        row = row.child(line);
                    }
                }
            }
            rows.push(row.into_any_element());
        }
        let content_w = (self.right_w - 14.).max(120.);
        let area = div()
            .id("session-area")
            .absolute()
            .top(px(crate::dir_panel::DIR_TITLE_H))
            .bottom_0()
            .left_0()
            .right_0();
        let body = if rows.is_empty() {
            area.flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px_3()
                        .text_size(px(11.))
                        .text_color(rgb(pal.dim))
                        .child(crate::i18n::t(cx, crate::i18n::Tr::SessionEmptyHint)),
                )
                .into_any_element()
        } else {
            area.child(
                ScrollArea::new(
                    self.sessions_scroll.clone(),
                    div()
                        .id("session-rows")
                        .w(px(content_w))
                        .flex()
                        .flex_col()
                        .children(rows),
                )
                .theme(pal.kit()),
            )
            .into_any_element()
        };
        div()
            .id("right-panel-sessions")
            .absolute()
            .top_0()
            .right(px(RIGHT_TOOLBAR_W))
            .w(px(self.right_w))
            .h(px(win_h))
            .flex_col()
            .bg(pal.surface(pal.panel))
            .border_l_1()
            .border_color(rgb(pal.divider()))
            .overflow_hidden()
            .child(self.render_sessions_title(cx))
            .child(body)
            .child(self.render_right_width_handle(cx))
            .into_any_element()
    }

    /// 会话状态页标题行：「会话状态 · 服务器名」。
    fn render_sessions_title(&self, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let mut row = div()
            .id("session-title")
            .flex()
            .items_center()
            .h(px(crate::dir_panel::DIR_TITLE_H))
            .px_3()
            .flex_none()
            .text_size(px(11.))
            .text_color(rgb(pal.dim))
            .child(div().child(crate::i18n::t(cx, crate::i18n::Tr::SessionStatus)));
        if let Some(tab) = self.active_live_tab() {
            let name: String = tab.title.chars().take(18).collect();
            row = row.child(div().text_color(rgb(pal.muted)).child(format!(" · {name}")));
        }
        row.into_any_element()
    }

    /// 右缘全高工具栏：上半是右面板的工具按钮（页面切换；点击激活中的工具收起面板），
    /// 钉底是左栏开合与设置。全部带多语言 tooltip（系统文字一律走 i18n）。
    fn render_right_toolbar(&self, win_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let dir_active = self.right_open && self.right_page == RightPage::Dir;
        let sys_active = self.right_open && self.right_page == RightPage::SysInfo;
        div()
            .id("right-toolbar")
            .absolute()
            .top_0()
            .right_0()
            .w(px(RIGHT_TOOLBAR_W))
            .h(px(win_h))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(6.))
            .gap_1()
            .bg(pal.surface(pal.panel))
            .border_l_1()
            .border_color(rgb(pal.divider()))
            .child(self.toolbar_button(
                "rt-dir",
                |fg: u32| svg_icon(if dir_active { icons::FOLDER_OPEN } else { icons::FOLDER }, 15., fg),
                dir_active,
                crate::i18n::Tr::TipToolDir,
                cx,
                |this, _w, cx| this.select_right_page(RightPage::Dir, cx),
            ))
            .child(self.toolbar_button(
                "rt-sys",
                |fg: u32| svg_icon(icons::MONITOR, 15., fg),
                sys_active,
                crate::i18n::Tr::TipToolSys,
                cx,
                |this, _w, cx| this.select_right_page(RightPage::SysInfo, cx),
            ))
            .child(self.toolbar_button(
                "rt-sessions",
                |fg: u32| svg_icon(icons::ACTIVITY, 15., fg),
                self.right_open && self.right_page == RightPage::Sessions,
                crate::i18n::Tr::TipToolSessions,
                cx,
                |this, _w, cx| this.select_right_page(RightPage::Sessions, cx),
            ))
            .child(self.toolbar_button(
                "rt-procs",
                |fg: u32| svg_icon(icons::ROWS, 15., fg),
                self.right_open && self.right_page == RightPage::Processes,
                crate::i18n::Tr::TipToolProcs,
                cx,
                |this, _w, cx| this.select_right_page(RightPage::Processes, cx),
            ))
            .child(div().flex_1())
            .child(self.toolbar_button(
                "rt-toggle-left",
                |fg: u32| div().text_size(px(15.)).text_color(rgb(fg)).child("◧").into_any_element(),
                self.left_open,
                crate::i18n::Tr::TipToggleSidebar,
                cx,
                |this, w, cx| this.toggle_left(w, cx),
            ))
            .child(self.toolbar_button(
                "rt-settings",
                |fg: u32| svg_icon(icons::COG, 15., fg),
                false,
                crate::i18n::Tr::TipSettings,
                cx,
                |this, _w, cx| this.open_settings(cx),
            ))
            .into_any_element()
    }

    /// 工具栏按钮：内容构建器（收前景色——svg 必须显式着色，见 svg_icon）+
    /// 激活态 + 多语言 tooltip + 点击。激活 = 强调色 + 卡片底；未激活 hover 浮起。
    fn toolbar_button(
        &self,
        id: &'static str,
        content: impl FnOnce(u32) -> AnyElement,
        active: bool,
        tip: crate::i18n::Tr,
        cx: &Context<Self>,
        f: impl Fn(&mut AppUi, &mut Window, &mut Context<AppUi>) + 'static,
    ) -> AnyElement {
        let pal = self.theme();
        let kit = pal.kit();
        let fg = if active { pal.accent } else { pal.muted };
        let mut b = div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(30.))
            .rounded_md()
            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| f(this, window, cx)))
            // tooltip builder 每次弹出时执行：语言/主题切换后的新提示即时生效。
            .tooltip(move |_, cx| {
                ui_kit::base::tooltip::tip(crate::i18n::t(cx, tip), kit, cx)
            });
        b = if active {
            b.text_color(rgb(pal.accent)).bg(pal.surface(pal.card))
        } else {
            b.text_color(rgb(pal.muted)).hover(|s| s.bg(pal.surface(pal.card)))
        };
        b.child(content(fg)).into_any_element()
    }

    /// 右面板左缘宽度拖柄：按下锚定（鼠标 x、当前宽度），向左拖变宽，持久化 right.width。
    pub(crate) fn render_right_width_handle(&self, cx: &Context<Self>) -> AnyElement {
        let drag = RightWidthDrag(self.right_width_drag.clone());
        div()
            .id("right-width-handle")
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(5.))
            .cursor(CursorStyle::ResizeLeftRight)
            .occlude()
            .hover(|s| s.bg(gpui::rgba(0x7aa2f744)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _w, _cx| {
                    let x: f32 = ev.position.x.into();
                    *this.right_width_drag.lock().expect("right width drag lock") = Some((x, this.right_w));
                }),
            )
            .on_drag(drag, move |_, _, _, cx| cx.new(|_| crate::sidebar::SplitterGhost))
            .on_drag_move(cx.listener(|this, e: &gpui::DragMoveEvent<RightWidthDrag>, _w, cx| {
                this.on_right_width_drag(e, cx);
            }))
            .into_any_element()
    }

    fn on_right_width_drag(&mut self, e: &gpui::DragMoveEvent<RightWidthDrag>, cx: &mut Context<Self>) {
        if !e.event.dragging() {
            *self.right_width_drag.lock().expect("right width drag lock") = None;
            return;
        }
        let x: f32 = e.event.position.x.into();
        let state = e.drag(cx).0.clone();
        let mut anchor = state.lock().expect("right width drag lock");
        let &mut (start_x, start_w) = anchor.get_or_insert((x, self.right_w));
        // 左缘往左拖 = 面板变宽（与侧栏右缘拖柄方向相反）
        let w = (start_w + (start_x - x)).clamp(RIGHT_MIN_W, RIGHT_MAX_W);
        if w != self.right_w {
            self.right_w = w;
            self.db.set_setting("right.width", &format!("{w}"));
            cx.notify();
        }
    }

    fn render_center(&self, window: &mut Window, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let Some(tab) = self.tabs.iter().find(|t| t.id == self.active_id) else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(pal.surface(pal.bg))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(pal.dim))
                        .child(crate::i18n::t(cx, crate::i18n::Tr::EmptyTabsHint)),
                )
                .into_any_element();
        };
        // 像素级：center 区域显式宽高（不再依赖 stretch/percentage 解析）
        let (win_w, win_h): (f32, f32) = {
            let sz = window.viewport_size();
            (sz.width.into(), sz.height.into())
        };
        let center_w = self.center_w(win_w);
        let center_h = (win_h - TITLEBAR_H).max(0.);
        match &tab.kind {
            TabKind::Connecting { params } => {
                // 展示名 = 别名（tab.title 建立时已按 别名>host>NodeID前8 位解析）。
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(center_w))
                    .h(px(center_h))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .items_center()
                    .justify_center()
                    .bg(pal.surface(pal.bg))
                    .child(div().text_size(px(15.)).child(crate::i18n::tf(cx, crate::i18n::Tr::ConnectingTo, &[&tab.title])))
                    .child(self.connect_log_box(tab.id, params))
                    .into_any_element()
            }
            TabKind::Live { term, session, opened_at } => {
                let term_handle = term.read(cx).focus_handle().clone();
                if !term_handle.is_focused(window) {
                    window.focus(&term_handle);
                }
                let (down, up) = session.stats.bytes();
                let user = session.params.username.clone();
                let tab_id = tab.id;
                // 链路标签（仅 P2P）：直连/中继/混合，探测完成前显示「检测中」。
                let path_chip = self.net_path_chip(tab.id, session, cx);
                // 注意：终端区不能用嵌套列里的 flex_1（gpui 0.2.2 实测高度塌 0，
                // 终端空白），改为绝对定位：终端填 [0, 状态栏高]，状态栏钉底。
                let term_h = (center_h - 26.).max(0.);
                // 虚拟滚动：终端内容由 grid 的 display_offset 管理，
                // ScrollArea 只画滚动条并把拖拽换算成 scroll_to（0=最老）。
                let (offset, max_hist, screen_rows) = term.read(cx).scroll_info();
                let goto = cx.listener({
                    let term = term.clone();
                    move |_this, top_off: &f32, _w, cx| {
                        let (_, max_hist, _) = term.read(cx).scroll_info();
                        let target =
                            (max_hist as f32 - top_off).round().clamp(0., max_hist as f32);
                        term.update(cx, |t, _| t.scroll_to(target as usize));
                        cx.notify();
                    }
                });
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(center_w))
                    .h(px(center_h))
                    // 不铺底色：终端默认背景自带 alpha，再铺一层表面色会二次压暗
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(center_w))
                            .h(px(term_h))
                            .child(scroll_area::ScrollArea::virtual_y(
                                self.term_scroll_state.clone(),
                                term.clone(),
                                (max_hist - offset) as f32,
                                max_hist as f32,
                                screen_rows as f32,
                                term_h,
                                move |top: f32, w, cx| goto(&top, w, cx),
                            )
                            .theme(self.theme().kit())),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px((center_h - 26.).max(0.)))
                            .left_0()
                            .w(px(center_w))
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .h(px(26.))
                            .bg(pal.surface(pal.panel))
                            .border_t_1()
                            .border_color(rgb(pal.divider()))
                    .child(div().text_size(px(11.)).text_color(rgb(pal.ok)).child("●"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(pal.muted))
                            .child(format!("{} · {}", tab.title, user)),
                    )
                    .children(path_chip)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(pal.dim))
                            .child(format!(
                                "↓ {} ↑ {} · {}",
                                fmt_bytes(down),
                                fmt_bytes(up),
                                fmt_duration(self.loc, opened_at.elapsed().as_secs())
                            )),
                    )
                    .child(div().flex_1())
                    .children(
                        // 会话往返延迟（实测；随承流路径变化）：≤30ms 绿 / ≤80ms 黄 / 更高红。
                        self.net_data
                            .get(&tab.id)
                            .and_then(|n| n.rtt_ms)
                            .map(|ms| {
                                let color = if ms <= 30 {
                                    pal.ok
                                } else if ms <= 80 {
                                    pal.warn
                                } else {
                                    pal.error
                                };
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(color))
                                    .child(format!("{ms} ms"))
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .id(("btn-tab-disconnect", tab_id as usize))
                                    .px_2()
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .rounded_sm()
                                    .text_size(px(11.))
                                    .text_color(rgb(pal.error))
                                    .border_1()
                                    .border_color(rgb(pal.border))
                                    .hover(|s| s.border_color(rgb(pal.error)))
                                    .child(crate::i18n::t(cx, crate::i18n::Tr::Disconnect))
                                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                        this.mark_tab_closed(tab_id, crate::i18n::t(cx, crate::i18n::Tr::ReasonManualDisconnect), cx);
                                        if let Some(t) = this.tabs.iter().find(|t| t.id == tab_id) {
                                            if let TabKind::Live { session, .. } = &t.kind {
                                                session.close();
                                            }
                                        }
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            TabKind::Failed { params, error } => {
                let tab_id = tab.id;
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(center_w))
                    .h(px(center_h))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .bg(pal.surface(pal.bg))
                    .child(div().text_size(px(15.)).text_color(rgb(pal.error)).child(crate::i18n::tf(cx, crate::i18n::Tr::ConnFailed, &[&tab.title])))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(pal.muted))
                            .max_w(px(480.))
                            .child(error.clone()),
                    )
                    // 过程日志随失败保留——定位卡在哪个阶段。
                    .child(self.connect_log_box(tab_id, params))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .id(("btn-retry", tab_id as usize))
                                    .h(px(30.))
                                    .px_5()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .bg(rgb(pal.accent))
                                    .text_color(rgb(pal.accent_dark))
                                    .text_size(px(13.))
                                    .hover(|s| s.bg(rgb(pal.accent)))
                                    .child(crate::i18n::t(cx, crate::i18n::Tr::Reconnect))
                                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                        this.retry_tab(tab_id, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id(("btn-close-failed", tab_id as usize))
                                    .h(px(30.))
                                    .px_5()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(pal.border))
                                    .text_color(rgb(pal.muted))
                                    .text_size(px(13.))
                                    .hover(|s| s.border_color(rgb(pal.error)))
                                    .child(crate::i18n::t(cx, crate::i18n::Tr::Close))
                                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                        this.close_tab_by_id(tab_id, cx);
                                    })),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    // ---------- 连接过程日志 / P2P 网络情况（渲染辅助） ----------

    /// 阶段文案（带目标/认证方式参数；语言实时取）。
    fn connect_step_text(&self, params: &ConnectParams, step: ConnectStep) -> String {
        use crate::i18n::Tr;
        let loc = self.loc;
        let t = |k: Tr| crate::i18n::tl(loc, k).to_string();
        let tf = |k: Tr, args: &[&str]| crate::i18n::tlf(loc, k, args);
        match step {
            ConnectStep::TcpDial => {
                tf(Tr::StepTcp, &[&format!("{}:{}", params.host, params.port)])
            }
            ConnectStep::IrohEndpoint => t(Tr::StepIrohEndpoint),
            ConnectStep::IrohOnline => t(Tr::StepIrohOnline),
            ConnectStep::IrohConnect => {
                let short: String = params.host.chars().take(8).collect();
                tf(Tr::StepIrohConnect, &[&short])
            }
            ConnectStep::IrohStream => t(Tr::StepIrohStream),
            ConnectStep::SshHandshake => t(Tr::StepSshHandshake),
            ConnectStep::SshAuth => {
                let how = match &params.auth {
                    AuthMethod::Password(_) => t(Tr::AuthPassword),
                    AuthMethod::Key { .. } => t(Tr::AuthKey),
                };
                tf(Tr::StepSshAuth, &[&how])
            }
            ConnectStep::SshChannel => t(Tr::StepSshChannel),
            ConnectStep::SshPty => t(Tr::StepSshPty),
            ConnectStep::Done => t(Tr::StepDone),
        }
    }

    /// 连接过程日志框（连接中/失败页共用）：等宽多行，新行自底增长。
    /// 无日志时返回空元素（占位不影响布局）。
    fn connect_log_box(&self, tab_id: u64, params: &ConnectParams) -> AnyElement {
        let pal = self.theme();
        let Some(steps) = self.connect_log.get(&tab_id) else {
            return div().into_any_element();
        };
        if steps.is_empty() {
            return div().into_any_element();
        }
        let mut lines = div().flex().flex_col().gap(px(2.));
        for (ms, step) in steps {
            lines = lines.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .text_color(rgb(pal.dim))
                            .child(fmt_step_ms(*ms)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(match step {
                                ConnectStep::Done => pal.ok,
                                _ => pal.muted,
                            }))
                            .child(self.connect_step_text(params, *step)),
                    ),
            );
        }
        div()
            .id(gpui::SharedString::from(format!("conn-log-{tab_id}")))
            .w(px(430.))
            .max_h(px(230.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .justify_end()
            .rounded_md()
            .border_1()
            .border_color(rgb(pal.divider()))
            .bg(pal.surface(pal.panel))
            .p_3()
            .font_family("Menlo")
            .text_size(px(11.))
            .child(lines)
            .into_any_element()
    }

    /// 状态栏链路标签（仅 P2P 会话）：直连/中继/混合；探测中显示「网络检测中…」。
    /// 悬停 tooltip 解释当前模式的含义（多语言即时生效）。
    fn net_path_chip(
        &self,
        tab_id: u64,
        session: &Session,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !matches!(session.params.transport, Transport::Iroh { .. }) {
            return None;
        }
        let pal = self.theme();
        let kit = pal.kit();
        let t = |k: crate::i18n::Tr| crate::i18n::t(cx, k);
        let (label, color, tip) = match self.net_data.get(&tab_id).and_then(|n| n.path.as_ref()) {
            Some(Ok(p)) => {
                let (label, tip_key) = if p.direct.is_some() && p.relayed {
                    (t(crate::i18n::Tr::NetMixed), crate::i18n::Tr::NetTipMixed)
                } else if p.direct.is_some() {
                    (t(crate::i18n::Tr::NetDirect), crate::i18n::Tr::NetTipDirect)
                } else {
                    (t(crate::i18n::Tr::NetRelay), crate::i18n::Tr::NetTipRelay)
                };
                let color = if p.direct.is_some() && p.relayed {
                    pal.accent
                } else if p.direct.is_some() {
                    pal.ok
                } else {
                    pal.warn
                };
                (label, color, Some(tip_key))
            }
            Some(Err(_)) => (t(crate::i18n::Tr::NetUnknown), pal.dim, None),
            None => (t(crate::i18n::Tr::NetDetecting), pal.dim, None),
        };
        let mut chip = div().id(("net-path-chip", tab_id as usize)).text_size(px(11.)).text_color(rgb(color));
        if let Some(tip_key) = tip {
            chip = chip.tooltip(move |_, cx| {
                ui_kit::base::tooltip::tip(crate::i18n::t(cx, tip_key), kit, cx)
            });
        }
        Some(chip.child(label).into_any_element())
    }

    /// 会话状态页·P2P 网络行（链路 / 本端 / 对端；逐项显示探测进度）。
    fn net_rows(&self, tab_id: u64) -> Vec<AnyElement> {
        use crate::i18n::Tr;
        let pal = self.theme();
        let loc = self.loc;
        let t = |k: Tr| crate::i18n::tl(loc, k);
        let net = match self.net_data.get(&tab_id) {
            Some(n) => n,
            None => return Vec::new(),
        };
        let mut rows = Vec::new();
        let line = |label: &str, value: String, color: u32| {
            div()
                .flex()
                .justify_between()
                .text_size(px(10.5))
                .child(div().text_color(rgb(pal.dim)).child(label.to_string()))
                .child(div().text_color(rgb(color)).child(value))
                .into_any_element()
        };
        // 链路（悬停 tooltip 解释当前模式含义）
        let (value, color, tip_key) = match net.path.as_ref() {
            Some(Ok(p)) => {
                let mut parts: Vec<String> = Vec::new();
                if let Some(d) = &p.direct {
                    parts.push(format!("{} {}", t(Tr::NetDirect), d));
                }
                if p.relayed {
                    parts.push(format!("{} {}", t(Tr::NetRelay), p.relay_url.as_deref().unwrap_or("")));
                }
                let tip_key = if p.direct.is_some() && p.relayed {
                    Tr::NetTipMixed
                } else if p.direct.is_some() {
                    Tr::NetTipDirect
                } else {
                    Tr::NetTipRelay
                };
                (parts.join(" + "), pal.text, Some(tip_key))
            }
            Some(Err(_)) => (t(Tr::NetDetectFail).to_string(), pal.dim, None),
            None => (t(Tr::NetDetecting).to_string(), pal.dim, None),
        };
        let mut path_row = div()
            .id(("net-path-row", tab_id as usize))
            .flex()
            .justify_between()
            .text_size(px(10.5))
            .child(div().text_color(rgb(pal.dim)).child(t(Tr::NetPath)))
            .child(div().text_color(rgb(color)).child(value));
        if let Some(k) = tip_key {
            let kit = pal.kit();
            path_row = path_row.tooltip(move |_, cx| {
                ui_kit::base::tooltip::tip(crate::i18n::t(cx, k), kit, cx)
            });
        }
        rows.push(path_row.into_any_element());
        // 本端 / 对端
        for (label, item) in [
            (t(Tr::NetLocal), &net.local),
            (t(Tr::NetRemote), &net.remote),
        ] {
            let (value, color) = match item {
                Some(Ok(i)) => (format!("{} · {} · {}", i.ip, i.country, i.isp), pal.muted),
                Some(Err(_)) => (t(Tr::NetDetectFail).to_string(), pal.dim),
                None => (t(Tr::NetDetecting).to_string(), pal.dim),
            };
            rows.push(line(&label, value, color));
        }
        // 会话往返延迟（实测）：中继 ≈ 50ms 级 / 直连 ≈ 10ms 级，切换即时反映。
        if let Some(ms) = net.rtt_ms {
            rows.push(line(&t(Tr::NetRtt), format!("{ms} ms"), pal.muted));
        }
        rows
    }

    /// 从系统信息捕获机器配置摘要（"2C 4G 60G"）写回服务器节点。
    /// sysinfo 探针每次成功 ingest 后调用；与库内值一致时跳过（不刷树）。
    pub(crate) fn capture_server_spec(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some((cores, mem_total, disk_total)) = self.sys_data.get(&tab_id).and_then(|st| {
            let s = st.stat.as_ref()?;
            let l = st.live.as_ref()?;
            Some((s.cores, l.mem_total, l.disk_total))
        }) else {
            return;
        };
        // 按 1000 进制（GB）取整：与硬件标称一致（df/meminfo 的 GiB 换算会偏小）。
        let gb = |b: u64| (b as f64 / 1e9).round() as u64;
        let spec = format!("{}C {}G {}G", cores.max(1), gb(mem_total), gb(disk_total));
        let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else { return };
        let TabKind::Live { session, .. } = &tab.kind else { return };
        let params = &session.params;
        let Some(node) = self.db.server_by_addr(&params.host, params.port).cloned() else { return };
        let NodeKind::Server(mut data) = node.kind else { return };
        if data.spec == spec {
            return;
        }
        data.spec = spec;
        self.db.update_node(node.id, &node.name, NodeKind::Server(data));
        self.refresh_tree(cx);
    }

    /// 激活 tab：置激活 + 记录最近使用序（MRU）+ 目录补取。
    /// 需要联动树的调用方（tab 点击/新连接）随后自行 refresh_tree。
    fn activate_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active_id = id;
        self.tab_mru.retain(|&x| x != id);
        self.tab_mru.insert(0, id);
        self.maybe_fetch_dir(cx);
        cx.notify();
    }

    pub fn retry_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            let params = match &tab.kind {
                TabKind::Failed { params, .. } | TabKind::Connecting { params } => params.clone(),
                _ => return,
            };
            tab.kind = TabKind::Connecting { params };
            self.refresh_dir_tree(cx);
            cx.notify();
            self.connect_tab(tab_id, cx);
        }
    }


}

// Div 便捷：条件样式放这里，避免装饰性 trait 扩散。

/// lucide 图标元素（svg 资产）。**必须显式传色**：gpui 0.2.2 的 Svg 只在自身
/// `style.text.color` 存在时绘制（svg.rs `zip(style.text.color)`），父级 div 的
/// text_color 只进文本样式栈、继承不进来——不传色的 svg 静默不画。
pub(crate) fn svg_icon(name: &str, size: f32, color: u32) -> AnyElement {
    gpui::svg()
        .path(icons::asset_path(name))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
        .into_any_element()
}

fn mock_params(port: u16) -> ConnectParams {
    ConnectParams {
        host: "127.0.0.1".into(),
        port,
        username: mock::MOCK_USER.into(),
        auth: AuthMethod::Password(mock::MOCK_PASS.into()),
        cols: 100,
        rows: 30,
        transport: Transport::Tcp,
    }
}

impl Render for AppUi {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 背景效果对账：模糊/主题系列变化 → 同步主窗口背景模式 + 全局模糊 tint
        // （开窗时的 WindowOptions 只管初值；tint 枚举全部窗口的模糊视图——
        // gpui 的模糊默认跟系统外观，浅色系统+浅色底 app 会糊成浅色，深色主题
        // 文字看不清，故按主题系列接管为 VibrantDark/VibrantLight）。
        let fx = theme::bg_fx(cx);
        let tint_dark = !theme::is_light(cx);
        if fx != self.applied_fx || tint_dark != self.applied_tint {
            self.applied_fx = fx;
            self.applied_tint = tint_dark;
            window.set_background_appearance(fx.window_appearance());
            ui_kit::platform::set_blur_tint(tint_dark);
        }
        let pal = self.theme();
        // 像素级布局：gpui 0.2.2 的 stretch/percentage/flex_1 高度链不可靠
        // （终端容器实测塌 0），所有顶层区域用显式像素尺寸钉死。
        let (win_w, win_h): (f32, f32) = {
            let sz = window.viewport_size();
            (sz.width.into(), sz.height.into())
        };
        let main_w = self.center_w(win_w);
        // tab 条溢出布局（测宽/钳制/激活 tab 滚入视野）需 &mut + Window，渲染前算好。
        let tabs_overflow = self.layout_tabs(window, main_w);

        let titlebar = self.render_titlebar(tabs_overflow, cx);
        let left = self.left_open.then(|| self.render_sidebar(window, cx));
        let center = self.render_center(window, cx);
        let right = self.right_open.then(|| self.render_right(win_h, cx));
        let right_toolbar = self.render_right_toolbar(win_h, cx);
        let overlay = self.render_menu_overlay(window, cx);
        let confirm = self.render_confirm_modal(cx);
        let overwrite = self.render_overwrite_modal(cx);
        let notice = self.render_notice(win_w);

        div()
            .id("root")
            .size_full()
            .relative()
            .flex()
            // 根不铺底色：侧栏/标题栏/终端各自铺**单层**表面色——根再铺一层
            // 会与面板色双层复合（50%×50%=75%），与单层的中部不一致且压暗模糊。
            .text_color(rgb(pal.text))
            .text_size(px(13.))
            .font_family("System UI")
            .on_action(cx.listener(|this, _: &NewTab, w, cx| this.new_tab(w, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, w, cx| this.close_active_tab(w, cx)))
            .on_action(cx.listener(|this, _: &NextTab, _w, cx| this.cycle_tab(1, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, _w, cx| this.cycle_tab(-1, cx)))
            .on_action(cx.listener(|this, _: &ToggleLeft, w, cx| this.toggle_left(w, cx)))
            .on_action(cx.listener(|this, _: &ToggleRight, w, cx| this.toggle_right(w, cx)))
            .children(left)
            .child(
                div()
                    .id("main-column")
                    // 文档流内排在侧栏之后（x=侧栏宽），同时作为 body 等
                    // absolute 子元素的定位上下文；不可改成 absolute（会脱流
                    // 落到 x=0 盖住侧栏）。
                    .relative()
                    .w(px(main_w))
                    .h(px(win_h))
                    .overflow_hidden()
                    .child(titlebar)
                    // body 高度链：flex_1 在"父高度为拉伸解析值"时会塌 0（终端容器
                    // 实测 bounds=598x0），改绝对定位钉死 [标题栏底, 窗口底]。
                    .child(
                        div()
                            .id("body")
                            .absolute()
                            .top(px(TITLEBAR_H))
                            .left_0()
                            .w(px(main_w))
                            .h(px((win_h - TITLEBAR_H).max(0.)))
                            .overflow_hidden()
                            .child(center),
                    ),
            )
            // 右侧状态面板：root 层绝对定位，钉右缘工具栏左侧（main_w 已扣除其宽度）
            .children(right)
            // 右缘全高工具栏：常驻（面板收起时也提供开合入口）
            .child(right_toolbar)
            .children(overlay)
            .children(confirm)
            .children(overwrite)
            .children(notice)
    }
}
