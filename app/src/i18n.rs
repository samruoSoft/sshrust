//! 全局多语言（简体中文 / English）。
//!
//! 机制：[`Tr`] 枚举收全 app 的 UI 文案，`text()` 的单 match 内嵌双分支，
//! 编译期强制双语完备（新增变体不补译文无法编译）——零运行时开销、零资源加载。
//! 生效链路复刻主题切换：设置值持久化在 DB（`app.language`），启动时解析成
//! [`Locale`] 注入 [`I18nGlobal`]；切换时设置窗写库 + `set_global`，所有
//! `observe_global` 的视图（主窗/弹出终端窗/表单/图标选择器）即时重渲染。
//!
//! 取文案入口：
//! - 渲染/有 cx 路径：[`t`] / [`tf`]（读 Global，无 Global 回退中文——单测确定性）
//! - 纯函数路径（菜单/树构建/测试）：[`tl`] / [`tlf`]（显式传 Locale）
//!
//! 带参数的文案用 `{0}`/`{1}` 占位（如 `删除「{0}」？`），经 `tf`/`tlf` 填充。
//! 用户数据（节点名、模板名、「XX 副本」等）按创建时语言持久化，不随切换回翻。

use gpui::App;

/// settings 键：界面语言（值见 [`Lang::as_str`]，缺省 auto 跟随系统）。
pub const KEY_LANG: &str = "app.language";

/// 语言设置存储值。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    /// 跟随系统（缺省）。
    #[default]
    Auto,
    Zh,
    En,
}

impl Lang {
    /// 从 DB 值解析（脏数据回退 Auto）。
    pub fn parse(s: &str) -> Self {
        match s {
            "zh" => Lang::Zh,
            "en" => Lang::En,
            _ => Lang::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Auto => "auto",
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }
}

/// 已解析语言（渲染取文案用；Auto 在启动/切换时经 [`detect`] 归结到此）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Locale {
    Zh,
    En,
}

/// 语言全局镜像（复刻 `ThemeGlobal` 模式）。
pub struct I18nGlobal(pub Locale);
impl gpui::Global for I18nGlobal {}

/// 系统语言检测（启动/切换 auto 时调用；zh* → 中文，否则英文）。
/// macOS GUI 进程 LANG 环境变量不可靠，必须走系统 API。
pub fn detect() -> Locale {
    match sys_locale::get_locale() {
        Some(l) if l.to_ascii_lowercase().starts_with("zh") => Locale::Zh,
        _ => Locale::En,
    }
}

pub fn resolve(lang: Lang) -> Locale {
    match lang {
        Lang::Auto => detect(),
        Lang::Zh => Locale::Zh,
        Lang::En => Locale::En,
    }
}

/// 当前语言：读 Global，缺失回退中文。
/// 单测/无 AppUi 场景下中文断言不随运行机器语言漂移。
pub fn current(cx: &App) -> Locale {
    cx.try_global::<I18nGlobal>().map(|g| g.0).unwrap_or(Locale::Zh)
}

/// 翻译 key：全 app 的 UI 文案目录（带占位参数的见各条目内文）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tr {
    // ---- 通用按钮/短词 ----
    Cancel,
    Ok,
    Delete,
    Disconnect,
    Close,
    /// 图标「自动」格（按名称匹配）。
    Auto,
    /// 图标选择器「推荐」分组标题。
    Recommended,
    /// 图标选择器分类标题（lucide 官方 42 类名 → 本地化；英文即官方名原样）。
    IconCat(&'static str),

    // ---- 通用时长（弹出窗会话状态等） ----
    /// 「{0} 秒」
    DurSecs,
    /// 「{0} 分 {1} 秒」
    DurMinSec,
    /// 「{0} 小时 {1} 分」
    DurHourMin,

    // ---- app_ui：tab 状态 / 连接视图 / 删除确认 / 会话面板 ----
    TabConnecting,
    TabConnected,
    TabDisconnected,
    EditServerTitle,
    NewServerTitle,
    /// 会话关闭原因（连接中断）。
    ReasonConnClosed,
    /// 会话关闭原因（用户手动断开）。
    ReasonManualDisconnect,
    DeleteConfirmBody,
    /// 「删除「{0}」？」
    DeleteConfirmTitle,
    /// 「正在连接 {0} …」
    ConnectingTo,
    /// 连接视图副标题（协议流程；技术词保留英文）。
    ConnectingSteps,
    /// 「{0} 连接失败」
    ConnFailed,
    Reconnect,
    /// 右侧目录面板标题（「目录 · 服务器名」的前半）。
    DirTitle,
    /// 右面板·会话状态页标题。
    SessionStatus,
    /// 会话状态页空态（无任何会话行）。
    SessionEmptyHint,
    /// 目录面板内传输区标题（传输功能后续开发）。
    TransferTitle,
    /// 传输区空态。
    TransferNoTasks,
    /// 目录面板空态提示（无已连接会话时）。
    DirEmptyHint,
    /// 目录面板已连接但 cwd 未知（等 OSC 7 / pwd）。
    DirNoInfo,

    // ---- 右缘工具栏：工具按钮（右面板切页）与常驻按钮的 tooltip ----
    /// 工具·目录浏览。
    TipToolDir,
    /// 工具·系统信息。
    TipToolSys,
    /// 工具·会话状态。
    TipToolSessions,
    /// 工具·进程列表。
    TipToolProcs,
    /// 左侧栏开合。
    TipToggleSidebar,
    /// 设置。
    TipSettings,

    // ---- 系统信息面板 ----
    SysTitle,
    SysEmptyHint,
    SysNoData,
    SysSecSystem,
    SysSecCpu,
    SysSecMem,
    SysSecNet,
    SysSecDisk,
    SysTimezone,
    SysUptime,
    SysHostname,
    SysKernel,
    SysCpuModel,
    SysMemTotal,
    SysMemUsed,
    SysMemCache,
    SysMemAvail,
    /// 网络方向：下行（远端→本地）。
    SysNetDown,
    /// 网络方向：上行（本地→远端）。
    SysNetUp,
    SysDiskTotal,
    SysDiskUsed,
    /// 「{0}/s」速率后缀。
    RateSuffix,

    // ---- 进程面板 ----
    ProcTitle,
    ProcEmptyHint,
    ProcNoData,
    ProcColPid,
    ProcColCpu,
    ProcColMem,
    ProcColRss,
    ProcColCmd,
    /// 详情区标题。
    ProcDetailTitle,
    /// 详情区空态。
    ProcDetailEmptyHint,
    /// 详情区搜索按钮 tooltip。
    ProcSearchTip,
    ProcDPid,
    ProcDParent,
    ProcDUser,
    ProcDStat,
    ProcDThreads,
    ProcDElapsed,
    ProcDCpu,
    ProcDMem,
    ProcDRss,
    /// 作用说明行标签。
    ProcDKind,
    ProcDCmd,
    /// CPU 口径后缀（瞬时）。
    ProcCpuInstantSuffix,
    /// CPU 口径后缀（平均）。
    ProcCpuAvgSuffix,
    ProcStatRunning,
    ProcStatSleeping,
    ProcStatUninterrupt,
    ProcStatZombie,
    ProcStatStopped,
    // ---- 进程识别（本地规则库 proc_kind） ----
    PkUnknown,
    PkKernelThread,
    PkSystemd,
    PkSystemdJournald,
    PkSystemdUdevd,
    PkSshd,
    PkNginx,
    PkMysql,
    PkPostgres,
    PkRedis,
    PkMemcached,
    PkNode,
    PkPython,
    PkJava,
    PkDocker,
    PkContainerd,
    PkShell,
    PkCron,
    PkSyslog,
    PkPrometheus,
    PkGrafana,
    PkDhclient,
    PkSnapd,
    PkPackageManager,
    PkVim,
    PkSudo,

    // ---- sidebar：树分组 / 右键菜单 / 快捷栏 ----
    TreeMyServers,
    TreeConnections,
    TreeCommands,
    MenuNewServer,
    MenuNewCommand,
    MenuNewFolder,
    MenuNewSubfolder,
    MenuConnect,
    /// 「编辑…」
    MenuEdit,
    MenuPopTab,
    MenuPopTabDisabled,
    MenuUiTheme,
    /// 新建文件夹节点的默认名（用户数据，创建时语言）。
    DefNewFolder,
    /// 缺口令/密钥时打开表单的引导提示。
    NeedCredential,
    Shortcuts,
    ShortcutEmpty,

    // ---- 常用命令：树分组 / 菜单 / 命令表单 / 运行到控制台 ----
    /// 「通用命令」分组标签（不限服务器，常驻显示）。
    TreeGeneralCommands,
    /// 「专用命令 · {0}」分组标签（{0}=服务器名；组常驻，按当前连接加载）。
    TreeServerCommands,
    /// 「专用命令」分组标签（未连接时的空态）。
    TreeServerCommandsPlain,
    /// 命令节点右键「运行到控制台」。
    MenuRunConsole,
    /// 专用命令组未连接时的「新增命令」禁用项。
    MenuNewCommandDisabled,
    /// 目录（文件夹）编辑窗口标题。
    EditFolderTitle,
    /// 目录表单字段：名称。
    LabelFolderName,
    PhFolderName,
    ErrFolderNameEmpty,
    /// 命令表单窗口标题（编辑）。
    EditCommandTitle,
    /// 命令表单窗口标题（新增）。
    NewCommandTitle,
    /// 命令表单字段：标题。
    LabelCmdTitle,
    /// 命令表单字段：说明。
    LabelCmdDesc,
    /// 命令表单字段：命令内容。
    LabelCmdContent,
    PhCmdTitle,
    PhCmdDesc,
    PhCmdContent,
    ErrCmdTitleEmpty,
    ErrCmdContentEmpty,
    /// 运行命令但无打开的控制台时的浮条提示。
    NoticeNoConsole,
    /// 运行命令但命令内容为空时的浮条提示。
    NoticeCmdEmpty,
    /// 通用「保存」按钮（命令表单 footer）。
    BtnSave,

    // ---- 服务器表单 ----
    PhAlias,
    PhKey,
    IconPickerTitle,
    FormHint,
    /// 「正在测试 {0} …」
    Testing,
    TestTimeout,
    /// 「连接测试成功（{0}）」
    TestOk,
    /// 「连接测试失败：{0}」
    TestFail,
    ErrPortInvalid,
    ErrHostEmpty,
    ErrPortRange,
    ErrUserEmpty,
    ErrKeyMissing,
    LabelAlias,
    LabelHost,
    LabelPort,
    LabelUsername,
    LabelAuth,
    LabelPassword,
    LabelPrivateKey,
    AuthPassword,
    AuthKey,
    BtnTesting,
    BtnTest,
    BtnSaveEdit,
    BtnSaveNew,

    // ---- 设置窗 ----
    SettingsTitle,
    NavAppearance,
    NavConsole,
    /// 设置导航分组「通用」。
    NavGeneral,
    /// 「通用」分组条目：系统（详情页含 语言/窗口/界面主题）。
    NavSystem,
    /// 系统页「窗口」节标题。
    SecWindow,
    SecBgBlur,
    /// 系统页「搜索引擎」节标题。
    SecSearchEngine,
    /// 搜索引擎节提示行。
    SearchEngineHint,
    /// 开关 chips：开。
    ToggleOn,
    /// 开关 chips：关。
    ToggleOff,
    BgBlurHint,
    /// 内置着色模板「标准」名。
    TplStandard,
    /// 内置着色模板「简洁」名。
    TplMinimal,
    /// 内置模板名兜底（不可达防御）。
    TplFallback,
    /// 新模板基础名（用户数据，创建时语言）。
    TplNewBase,
    /// 「{0} 副本」：编辑内置模板时自动转用户副本的命名（用户数据）。
    CopySuffix,
    BtnTplNew,
    BtnTplSaveAs,
    SecLanguage,
    SecFont,
    SecFontSize,
    SecTermPalette,
    SecStyleTemplates,
    TplHint,
    /// 语言选项：跟随系统。
    LangAuto,
    /// 语言选项/显示名：简体中文（两种语言下同名）。
    LangZh,
    /// 语言选项/显示名：English（两种语言下同名）。
    LangEn,
    /// 「当前语言：{0}」
    LangCurrent,
    PhTplName,

    // ---- 弹出独立窗口 ----
    MergeBack,
    SessionEnded,

    // ---- 着色规则类别（third_party STYLE_CATS 显示名的 app 侧双语映射） ----
    CatPromptUser,
    CatPromptPath,
    CatUrl,
    CatLastLogin,
    CatIp,
    CatDate,
    CatLsPerm,
    CatLsSize,
    CatLsMeta,
    CatLsDir,
    CatLsExec,
    CatLsSymlink,
    CatLsArchive,
    CatLsImage,
    CatLsDevice,
}

/// 图标选择器分类名的中文译名（英文即 lucide 官方名，原样返回）。
/// 与 `icons::ICON_CATEGORIES` 的标题一一对应（测试锁定全覆盖）。
fn icon_cat_zh(name: &str) -> &'static str {
    match name {
        "Accessibility" => "无障碍",
        "Accounts & access" => "账户与访问",
        "Animals" => "动物",
        "Arrows" => "箭头",
        "Buildings" => "建筑",
        "Charts" => "图表",
        "Communication" => "通信",
        "Connectivity" => "连接",
        "Cursors" => "光标",
        "Design" => "设计",
        "Coding & development" => "编程开发",
        "Devices" => "设备",
        "Emoji" => "表情",
        "File icons" => "文件图标",
        "Finance" => "金融",
        "Food & beverage" => "餐饮",
        "Gaming" => "游戏",
        "Home" => "家居",
        "Layout" => "布局",
        "Mail" => "邮件",
        "Mathematics" => "数学",
        "Medical" => "医疗",
        "Multimedia" => "多媒体",
        "Nature" => "自然",
        "Navigation & Places" => "导航与地点",
        "Notifications" => "通知",
        "People" => "人物",
        "Photography" => "摄影",
        "Science" => "科学",
        "Seasons" => "季节",
        "Security" => "安全",
        "Shapes" => "形状",
        "Shopping" => "购物",
        "Social" => "社交",
        "Sports" => "运动",
        "Sustainability" => "可持续",
        "Text formatting" => "文字排版",
        "Time & calendar" => "时间与日历",
        "Tools" => "工具",
        "Transportation" => "交通",
        "Travel" => "旅行",
        "Weather" => "天气",
        _ => "其他",
    }
}

/// 双语文案内联选择（仅 [`Tr::text`] 内部使用；须定义在使用点之前）。
macro_rules! m {
    ($loc:expr, $zh:expr, $en:expr) => {
        match $loc {
            Locale::Zh => $zh,
            Locale::En => $en,
        }
    };
}

impl Tr {
    /// 双语文案（编译期强制每个变体两种语言都有译文）。
    /// 带 `{0}` 占位的条目必须经 [`tf`]/[`tlf`] 取用。
    pub fn text(self, loc: Locale) -> &'static str {
        match self {
            Tr::Cancel => m!(loc, "取消", "Cancel"),
            Tr::Ok => m!(loc, "确定", "OK"),
            Tr::Delete => m!(loc, "删除", "Delete"),
            Tr::Disconnect => m!(loc, "断开", "Disconnect"),
            Tr::Close => m!(loc, "关闭", "Close"),
            Tr::Auto => m!(loc, "自动", "Auto"),
            Tr::Recommended => m!(loc, "推荐", "Recommended"),
            Tr::IconCat(name) => {
                let zh = icon_cat_zh(name);
                m!(loc, zh, name)
            }

            Tr::DurSecs => m!(loc, "{0} 秒", "{0}s"),
            Tr::DurMinSec => m!(loc, "{0} 分 {1} 秒", "{0}m {1}s"),
            Tr::DurHourMin => m!(loc, "{0} 小时 {1} 分", "{0}h {1}m"),

            Tr::TabConnecting => m!(loc, "连接中", "Connecting"),
            Tr::TabConnected => m!(loc, "已连接", "Connected"),
            Tr::TabDisconnected => m!(loc, "已断开", "Disconnected"),
            Tr::EditServerTitle => m!(loc, "编辑服务器", "Edit Server"),
            Tr::NewServerTitle => m!(loc, "新增服务器", "New Server"),
            Tr::ReasonConnClosed => m!(loc, "连接已断开", "Connection closed"),
            Tr::ReasonManualDisconnect => m!(loc, "手动断开", "Disconnected manually"),
            Tr::DeleteConfirmBody => m!(
                loc,
                "将同时删除其中的子项，且不可恢复。",
                "This will also delete its children. This cannot be undone."
            ),
            Tr::DeleteConfirmTitle => m!(loc, "删除「{0}」？", "Delete \"{0}\"?"),
            Tr::ConnectingTo => m!(loc, "正在连接 {0} …", "Connecting to {0} …"),
            Tr::ConnectingSteps => m!(loc, "russh：握手 → 认证 → PTY → Shell", "russh: handshake → auth → pty → shell"),
            Tr::ConnFailed => m!(loc, "{0} 连接失败", "Failed to connect to {0}"),
            Tr::Reconnect => m!(loc, "重连", "Reconnect"),
            Tr::DirTitle => m!(loc, "目录", "Directory"),
            Tr::SessionStatus => m!(loc, "会话状态", "Session Status"),
            Tr::SessionEmptyHint => m!(loc, "暂无会话", "No sessions"),
            Tr::TransferTitle => m!(loc, "文件传输", "File Transfer"),
            Tr::TransferNoTasks => m!(loc, "暂无传输任务", "No transfers"),
            Tr::DirEmptyHint => m!(
                loc,
                "连接服务器后在此浏览其目录",
                "Connect a server to browse its directory"
            ),
            Tr::DirNoInfo => m!(loc, "等待目录信息…", "Waiting for directory info…"),

            Tr::TipToolDir => m!(loc, "目录", "Directory"),
            Tr::TipToolSys => m!(loc, "系统信息", "System Info"),
            Tr::TipToolSessions => m!(loc, "会话状态", "Session Status"),
            Tr::TipToolProcs => m!(loc, "进程列表", "Processes"),
            Tr::TipToggleSidebar => m!(loc, "显示/隐藏侧栏", "Toggle Sidebar"),
            Tr::TipSettings => m!(loc, "设置", "Settings"),

            Tr::SysTitle => m!(loc, "系统信息", "System Info"),
            Tr::SysEmptyHint => m!(
                loc,
                "连接服务器后在此查看其系统信息",
                "Connect a server to view its system info"
            ),
            Tr::SysNoData => m!(loc, "正在读取系统信息…", "Reading system info…"),
            Tr::SysSecSystem => m!(loc, "系统", "System"),
            Tr::SysSecCpu => m!(loc, "CPU", "CPU"),
            Tr::SysSecMem => m!(loc, "内存", "Memory"),
            Tr::SysSecNet => m!(loc, "网络", "Network"),
            Tr::SysSecDisk => m!(loc, "磁盘", "Disk"),
            Tr::SysTimezone => m!(loc, "时区", "Timezone"),
            Tr::SysUptime => m!(loc, "运行时间", "Uptime"),
            Tr::SysHostname => m!(loc, "主机名", "Hostname"),
            Tr::SysKernel => m!(loc, "内核", "Kernel"),
            Tr::SysCpuModel => m!(loc, "处理器", "Processor"),
            Tr::SysMemTotal => m!(loc, "总内存", "Total"),
            Tr::SysMemUsed => m!(loc, "已用", "Used"),
            Tr::SysMemCache => m!(loc, "缓存", "Cache"),
            Tr::SysMemAvail => m!(loc, "可用", "Available"),
            Tr::SysNetDown => m!(loc, "下行", "Download"),
            Tr::SysNetUp => m!(loc, "上行", "Upload"),
            Tr::SysDiskTotal => m!(loc, "总计", "Total"),
            Tr::SysDiskUsed => m!(loc, "已用", "Used"),
            Tr::RateSuffix => m!(loc, "{0}/s", "{0}/s"),

            Tr::ProcTitle => m!(loc, "进程", "Processes"),
            Tr::ProcEmptyHint => m!(loc, "连接服务器后查看进程", "Connect to a server to view processes"),
            Tr::ProcNoData => m!(loc, "正在获取进程…", "Fetching processes…"),
            Tr::ProcColPid => m!(loc, "PID", "PID"),
            Tr::ProcColCpu => m!(loc, "CPU%", "CPU%"),
            Tr::ProcColMem => m!(loc, "内存%", "MEM%"),
            Tr::ProcColRss => m!(loc, "内存", "RSS"),
            Tr::ProcColCmd => m!(loc, "命令", "Command"),

            Tr::ProcDetailTitle => m!(loc, "进程详细", "Process Details"),
            Tr::ProcDetailEmptyHint => m!(loc, "点击列表中的进程查看详情", "Click a process in the list to see details"),
            Tr::ProcSearchTip => m!(loc, "用浏览器搜索此进程", "Search this process in your browser"),
            Tr::ProcDPid => m!(loc, "PID", "PID"),
            Tr::ProcDParent => m!(loc, "父进程", "Parent"),
            Tr::ProcDUser => m!(loc, "用户", "User"),
            Tr::ProcDStat => m!(loc, "状态", "State"),
            Tr::ProcDThreads => m!(loc, "线程", "Threads"),
            Tr::ProcDElapsed => m!(loc, "运行时长", "Uptime"),
            Tr::ProcDCpu => m!(loc, "CPU", "CPU"),
            Tr::ProcDMem => m!(loc, "内存", "Memory"),
            Tr::ProcDRss => m!(loc, "常驻内存", "Resident"),
            Tr::ProcDKind => m!(loc, "说明", "Description"),
            Tr::ProcDCmd => m!(loc, "命令行", "Command line"),
            Tr::ProcCpuInstantSuffix => m!(loc, "（瞬时）", " (instant)"),
            Tr::ProcCpuAvgSuffix => m!(loc, "（平均）", " (average)"),
            Tr::ProcStatRunning => m!(loc, "运行", "Running"),
            Tr::ProcStatSleeping => m!(loc, "睡眠", "Sleeping"),
            Tr::ProcStatUninterrupt => m!(loc, "不可中断睡眠", "Uninterruptible"),
            Tr::ProcStatZombie => m!(loc, "僵尸", "Zombie"),
            Tr::ProcStatStopped => m!(loc, "已停止", "Stopped"),

            Tr::PkUnknown => m!(loc, "未收录", "Not in local catalog"),
            Tr::PkKernelThread => m!(loc, "内核线程", "Kernel thread"),
            Tr::PkSystemd => m!(loc, "系统初始化与服务管理器（PID 1）", "System init & service manager (PID 1)"),
            Tr::PkSystemdJournald => m!(loc, "系统日志服务", "System journal service"),
            Tr::PkSystemdUdevd => m!(loc, "设备管理守护进程", "Device manager daemon"),
            Tr::PkSshd => m!(loc, "SSH 服务端", "SSH server"),
            Tr::PkNginx => m!(loc, "Web 服务器 / 反向代理", "Web server / reverse proxy"),
            Tr::PkMysql => m!(loc, "MySQL / MariaDB 数据库", "MySQL / MariaDB database"),
            Tr::PkPostgres => m!(loc, "PostgreSQL 数据库", "PostgreSQL database"),
            Tr::PkRedis => m!(loc, "Redis 内存缓存", "Redis in-memory cache"),
            Tr::PkMemcached => m!(loc, "Memcached 内存缓存", "Memcached cache"),
            Tr::PkNode => m!(loc, "Node.js 应用", "Node.js application"),
            Tr::PkPython => m!(loc, "Python 程序", "Python program"),
            Tr::PkJava => m!(loc, "Java 虚拟机应用", "Java (JVM) application"),
            Tr::PkDocker => m!(loc, "Docker 容器引擎", "Docker container engine"),
            Tr::PkContainerd => m!(loc, "容器运行时", "Container runtime"),
            Tr::PkShell => m!(loc, "交互 Shell", "Interactive shell"),
            Tr::PkCron => m!(loc, "定时任务守护进程", "Cron scheduler"),
            Tr::PkSyslog => m!(loc, "系统日志守护进程", "System log daemon"),
            Tr::PkPrometheus => m!(loc, "Prometheus 监控", "Prometheus monitoring"),
            Tr::PkGrafana => m!(loc, "Grafana 可视化面板", "Grafana dashboards"),
            Tr::PkDhclient => m!(loc, "DHCP 客户端", "DHCP client"),
            Tr::PkSnapd => m!(loc, "Snap 包管理守护进程", "Snap package daemon"),
            Tr::PkPackageManager => m!(loc, "包管理器", "Package manager"),
            Tr::PkVim => m!(loc, "文本编辑器", "Text editor"),
            Tr::PkSudo => m!(loc, "提权执行", "Privileged execution"),

            Tr::TreeMyServers => m!(loc, "我的服务器", "My Servers"),
            Tr::TreeConnections => m!(loc, "连接", "Connections"),
            Tr::TreeCommands => m!(loc, "常用命令", "Commands"),
            Tr::MenuNewServer => m!(loc, "新增服务器", "New Server"),
            Tr::MenuNewCommand => m!(loc, "新增命令", "New Command"),
            Tr::MenuNewFolder => m!(loc, "新增文件夹", "New Folder"),
            Tr::MenuNewSubfolder => m!(loc, "新增子文件夹", "New Subfolder"),
            Tr::MenuConnect => m!(loc, "连接", "Connect"),
            Tr::MenuEdit => m!(loc, "编辑…", "Edit…"),
            Tr::MenuPopTab => m!(loc, "弹出独立窗口", "Pop Out to Window"),
            Tr::MenuPopTabDisabled => m!(
                loc,
                "弹出独立窗口（连接后可用）",
                "Pop Out to Window (available after connect)"
            ),
            Tr::MenuUiTheme => m!(loc, "界面主题", "UI Theme"),
            Tr::DefNewFolder => m!(loc, "新建文件夹", "New Folder"),
            Tr::NeedCredential => m!(
                loc,
                "该主机未保存密码，请在表单中输入后连接",
                "No password saved for this host. Enter it in the form to connect."
            ),
            Tr::Shortcuts => m!(loc, "快捷方式", "Shortcuts"),
            Tr::ShortcutEmpty => m!(
                loc,
                "拖拽服务器到这里收藏（即将支持）",
                "Drag servers here to favorite (coming soon)"
            ),

            Tr::TreeGeneralCommands => m!(loc, "通用命令", "General"),
            Tr::TreeServerCommands => m!(loc, "专用命令 · {0}", "Server · {0}"),
            Tr::TreeServerCommandsPlain => m!(loc, "专用命令", "Server"),
            Tr::MenuRunConsole => m!(loc, "运行到控制台", "Run in Console"),
            Tr::MenuNewCommandDisabled => m!(
                loc,
                "新增命令（连接后可用）",
                "New Command (available after connect)"
            ),
            Tr::EditFolderTitle => m!(loc, "编辑目录", "Edit Folder"),
            Tr::LabelFolderName => m!(loc, "名称", "Name"),
            Tr::PhFolderName => m!(loc, "如：运维工具", "e.g. Ops tools"),
            Tr::ErrFolderNameEmpty => m!(loc, "名称不能为空", "Name is required"),
            Tr::EditCommandTitle => m!(loc, "编辑命令", "Edit Command"),
            Tr::NewCommandTitle => m!(loc, "新增命令", "New Command"),
            Tr::LabelCmdTitle => m!(loc, "标题", "Title"),
            Tr::LabelCmdDesc => m!(loc, "说明", "Description"),
            Tr::LabelCmdContent => m!(loc, "命令内容", "Command"),
            Tr::PhCmdTitle => m!(loc, "如：查看磁盘空间", "e.g. Disk usage"),
            Tr::PhCmdDesc => m!(loc, "可选备注", "Optional note"),
            Tr::PhCmdContent => m!(loc, "df -h", "df -h"),
            Tr::ErrCmdTitleEmpty => m!(loc, "标题不能为空", "Title is required"),
            Tr::ErrCmdContentEmpty => m!(loc, "命令内容不能为空", "Command is required"),
            Tr::NoticeNoConsole => m!(
                loc,
                "没有打开的控制台，请先连接服务器",
                "No console open. Connect to a server first."
            ),
            Tr::NoticeCmdEmpty => m!(loc, "命令内容为空", "The command is empty"),
            Tr::BtnSave => m!(loc, "保存", "Save"),

            Tr::PhAlias => m!(loc, "可选，作为侧栏标题", "Optional; used as the sidebar title"),
            Tr::PhKey => m!(
                loc,
                "-----BEGIN OPENSSH PRIVATE KEY-----\n…（直接粘贴私钥内容）",
                "-----BEGIN OPENSSH PRIVATE KEY-----\n…(paste private key content here)"
            ),
            Tr::IconPickerTitle => m!(loc, "选择图标", "Choose Icon"),
            Tr::FormHint => m!(
                loc,
                "别名显示为侧栏树标题；密钥认证直接粘贴私钥内容（~/.sshrust/sshrust.db）",
                "The alias shows as the sidebar title; for key auth paste the private key directly (~/.sshrust/sshrust.db)"
            ),
            Tr::Testing => m!(loc, "正在测试 {0} …", "Testing {0} …"),
            Tr::TestTimeout => m!(loc, "连接超时（12 秒）", "Connection timed out (12s)"),
            Tr::TestOk => m!(loc, "连接测试成功（{0}）", "Test connection succeeded ({0})"),
            Tr::TestFail => m!(loc, "连接测试失败：{0}", "Test connection failed: {0}"),
            Tr::ErrPortInvalid => m!(loc, "端口无效", "Invalid port"),
            Tr::ErrHostEmpty => m!(loc, "主机不能为空", "Host is required"),
            Tr::ErrPortRange => m!(
                loc,
                "端口必须是 1-65535 的数字",
                "Port must be a number between 1 and 65535"
            ),
            Tr::ErrUserEmpty => m!(loc, "用户名不能为空", "Username is required"),
            Tr::ErrKeyMissing => m!(
                loc,
                "密钥认证请粘贴私钥内容（PEM / OpenSSH 格式）",
                "Paste a private key (PEM / OpenSSH format) for key auth"
            ),
            Tr::LabelAlias => m!(loc, "别名", "Alias"),
            Tr::LabelHost => m!(loc, "主机", "Host"),
            Tr::LabelPort => m!(loc, "端口", "Port"),
            Tr::LabelUsername => m!(loc, "用户名", "Username"),
            Tr::LabelAuth => m!(loc, "认证", "Auth"),
            Tr::LabelPassword => m!(loc, "密码", "Password"),
            Tr::LabelPrivateKey => m!(loc, "私钥", "Private Key"),
            Tr::AuthPassword => m!(loc, "密码", "Password"),
            Tr::AuthKey => m!(loc, "密钥", "Key"),
            Tr::BtnTesting => m!(loc, "测试中…", "Testing…"),
            Tr::BtnTest => m!(loc, "测试", "Test"),
            Tr::BtnSaveEdit => m!(loc, "保存修改", "Save Changes"),
            Tr::BtnSaveNew => m!(loc, "保存主机", "Save Host"),

            Tr::SettingsTitle => m!(loc, "设置", "Settings"),
            Tr::NavAppearance => m!(loc, "外观", "Appearance"),
            Tr::NavConsole => m!(loc, "控制台", "Console"),
            Tr::NavGeneral => m!(loc, "通用", "General"),
            Tr::NavSystem => m!(loc, "系统", "System"),
            Tr::SecWindow => m!(loc, "窗口", "Window"),
            Tr::SecBgBlur => m!(loc, "背景模糊", "Background Blur"),
            Tr::SecSearchEngine => m!(loc, "搜索引擎", "Search Engine"),
            Tr::SearchEngineHint => m!(
                loc,
                "进程详情「外部搜索」使用的搜索引擎；改动即时生效。",
                "Search engine used by the external search in process details. Takes effect immediately."
            ),
            Tr::ToggleOn => m!(loc, "开", "On"),
            Tr::ToggleOff => m!(loc, "关", "Off"),
            Tr::BgBlurHint => m!(
                loc,
                "模糊窗口背后的内容，提高半透明时的可读性；作用于全部窗口，改动即时生效。",
                "Blurs what's behind the window for readability while translucent. Applies to every window, takes effect immediately."
            ),
            Tr::TplStandard => m!(loc, "标准", "Standard"),
            Tr::TplMinimal => m!(loc, "简洁", "Minimal"),
            Tr::TplFallback => m!(loc, "模板", "Template"),
            Tr::TplNewBase => m!(loc, "新模板", "New Template"),
            Tr::CopySuffix => m!(loc, "{0} 副本", "{0} copy"),
            Tr::BtnTplNew => m!(loc, "＋ 新建", "+ New"),
            Tr::BtnTplSaveAs => m!(loc, "另存为", "Save As"),
            Tr::SecLanguage => m!(loc, "语言", "Language"),
            Tr::SecFont => m!(loc, "字体", "Font"),
            Tr::SecFontSize => m!(loc, "字号", "Size"),
            Tr::SecTermPalette => m!(loc, "终端配色", "Terminal Palette"),
            Tr::SecStyleTemplates => m!(loc, "着色模板", "Style Templates"),
            Tr::TplHint => m!(
                loc,
                "内置模板不可修改：调整任一规则会自动保存为「副本」；改动即时生效并持久化。字体为全局设置，不随模板变化。",
                "Built-in templates are read-only: editing any rule automatically saves a copy. Changes take effect immediately and persist. Font is a global setting, independent of templates."
            ),
            Tr::LangAuto => m!(loc, "跟随系统", "Follow System"),
            Tr::LangZh => m!(loc, "简体中文", "简体中文"),
            Tr::LangEn => m!(loc, "English", "English"),
            Tr::LangCurrent => m!(loc, "当前语言：{0}", "Current language: {0}"),
            Tr::PhTplName => m!(loc, "模板名", "Template name"),

            Tr::MergeBack => m!(loc, "并入主窗口", "Merge to Main Window"),
            Tr::SessionEnded => m!(loc, "会话已结束", "Session ended"),

            Tr::CatPromptUser => m!(loc, "提示符 · 用户@主机", "Prompt · user@host"),
            Tr::CatPromptPath => m!(loc, "提示符 · 路径", "Prompt · path"),
            Tr::CatUrl => m!(loc, "链接 URL", "URL link"),
            Tr::CatLastLogin => m!(loc, "Last login 整行", "Last login line"),
            Tr::CatIp => m!(loc, "IP 地址", "IP address"),
            Tr::CatDate => m!(loc, "日期时间", "Date & time"),
            Tr::CatLsPerm => m!(loc, "ls · 权限串", "ls · permissions"),
            Tr::CatLsSize => m!(loc, "ls · 大小", "ls · size"),
            Tr::CatLsMeta => m!(loc, "ls · 属主/属组/链接数", "ls · owner/group/links"),
            Tr::CatLsDir => m!(loc, "ls · 目录名", "ls · directory"),
            Tr::CatLsExec => m!(loc, "ls · 可执行名", "ls · executable"),
            Tr::CatLsSymlink => m!(loc, "ls · 符号链接名", "ls · symlink"),
            Tr::CatLsArchive => m!(loc, "ls · 压缩包名", "ls · archive"),
            Tr::CatLsImage => m!(loc, "ls · 图片名", "ls · image"),
            Tr::CatLsDevice => m!(loc, "ls · 设备/socket/管道名", "ls · device/socket/fifo"),
        }
    }
}

/// 即时翻译（渲染/有 cx 路径；读 Global，缺失回退中文）。
pub fn t(cx: &App, key: Tr) -> &'static str {
    tl(current(cx), key)
}

/// 纯函数翻译（菜单/树构建/测试等显式传 Locale 的路径）。
pub fn tl(loc: Locale, key: Tr) -> &'static str {
    key.text(loc)
}

/// 即时翻译并填充 `{0}`/`{1}` 占位。
pub fn tf(cx: &App, key: Tr, args: &[&str]) -> String {
    tlf(current(cx), key, args)
}

/// 纯函数翻译并填充占位。
pub fn tlf(loc: Locale, key: Tr, args: &[&str]) -> String {
    fill(key.text(loc), args)
}

/// `{n}` 占位顺序替换（n 为 args 下标；非法下标替换为空串）。
fn fill(tpl: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(tpl.len() + 16);
    let mut rest = tpl;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(rel) = rest[i..].find('}') else { break };
        let end = i + rel;
        let idx: usize = rest[i + 1..end].parse().unwrap_or(usize::MAX);
        out.push_str(args.get(idx).copied().unwrap_or(""));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// 着色规则类别 id（third_party `STYLE_CATS` 的稳定 id）→ 双语显示名。
/// third_party 里带的中文显示名不直接使用，翻译收敛在 app 侧。
pub fn style_cat(id: &str) -> Tr {
    match id {
        "prompt_user" => Tr::CatPromptUser,
        "prompt_path" => Tr::CatPromptPath,
        "url" => Tr::CatUrl,
        "last_login" => Tr::CatLastLogin,
        "ip" => Tr::CatIp,
        "date" => Tr::CatDate,
        "ls_perm" => Tr::CatLsPerm,
        "ls_size" => Tr::CatLsSize,
        "ls_meta" => Tr::CatLsMeta,
        "ls_dir" => Tr::CatLsDir,
        "ls_exec" => Tr::CatLsExec,
        "ls_symlink" => Tr::CatLsSymlink,
        "ls_archive" => Tr::CatLsArchive,
        "ls_image" => Tr::CatLsImage,
        "ls_device" => Tr::CatLsDevice,
        _ => Tr::CatPromptUser,
    }
}

/// 语言设置项的选项表（设置窗 chips 数据源；顺序即显示顺序）。
pub const LANG_OPTIONS: &[Lang] = &[Lang::Auto, Lang::Zh, Lang::En];

/// 语言选项的显示名（Auto 显示「跟随系统」，具体语言用本名）。
pub fn lang_label(lang: Lang, loc: Locale) -> &'static str {
    match lang {
        Lang::Auto => tl(loc, Tr::LangAuto),
        Lang::Zh => tl(loc, Tr::LangZh),
        Lang::En => tl(loc, Tr::LangEn),
    }
}

/// 已解析语言的显示名（「当前语言：…」用）。
pub fn locale_label(loc: Locale) -> &'static str {
    match loc {
        Locale::Zh => tl(loc, Tr::LangZh),
        Locale::En => tl(loc, Tr::LangEn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_categories_all_translated() {
        // 每个图标分类标题都必须有中文译名（未命中兜底「其他」即失败），
        // 防止新增分类漏翻。
        for (title, _) in crate::icons::ICON_CATEGORIES {
            let zh = Tr::IconCat(title).text(Locale::Zh);
            assert_ne!(zh, "其他", "图标分类「{title}」缺中文译名");
            assert_eq!(Tr::IconCat(title).text(Locale::En), *title);
        }
    }

    #[test]
    fn lang_round_trip() {
        for lang in LANG_OPTIONS {
            assert_eq!(Lang::parse(lang.as_str()), *lang);
        }
        assert_eq!(Lang::parse(""), Lang::Auto);
        assert_eq!(Lang::parse("脏数据"), Lang::Auto);
    }

    #[test]
    fn resolve_explicit() {
        assert_eq!(resolve(Lang::Zh), Locale::Zh);
        assert_eq!(resolve(Lang::En), Locale::En);
    }

    #[test]
    fn fill_placeholders() {
        assert_eq!(fill("删除「{0}」？", &["web"]), "删除「web」？");
        assert_eq!(fill("{0} 连接失败", &["1.2.3.4:22"]), "1.2.3.4:22 连接失败");
        assert_eq!(fill("无参数文案", &[]), "无参数文案");
        assert_eq!(fill("越界 {9}", &["a"]), "越界 ");
        // 非法占位（非数字下标）按空串处理，不 panic。
        assert_eq!(fill("{x}", &["a"]), "");
    }

    #[test]
    fn formatted_keys_bilingual() {
        assert_eq!(tlf(Locale::Zh, Tr::ConnFailed, &["h:1"]), "h:1 连接失败");
        assert_eq!(tlf(Locale::En, Tr::ConnFailed, &["h:1"]), "Failed to connect to h:1");
        assert_eq!(tlf(Locale::En, Tr::CopySuffix, &["Standard"]), "Standard copy");
    }
}
