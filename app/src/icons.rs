//! 树形节点图标：内嵌 Lucide SVG（见 assets.rs），按名称关键词自动匹配。
//!
//! `node.icon` 为空 = 自动匹配；非空 = 用户在编辑表单/图标选择器里显式选择的图标名
//!（可选清单即 [`ICON_CATEGORIES`]）。

/// svg 元素使用的资产路径。
pub fn asset_path(icon: &str) -> String {
    format!("icons/{icon}.svg")
}

/// 官方分类（lucide.dev 全部 42 类）→ 精选图标名；图标选择器的分组数据源。
/// 成员由官方 tags/命名规则推导、每类精选上限 16 个，全部 svg 已内嵌。
pub const ICON_CATEGORIES: &[(&str, &[&str])] = &[
    ("Accessibility", &["activity", "book-open", "x", "cog", "ear", "eye", "nfc", "atom", "book", "cone", "hand", "earth", "panda", "regex", "trash"]),
    ("Accounts & access", &["key", "lock", "shield", "book-open", "pin", "tag", "bomb", "book", "tags", "user", "award", "gavel", "piano", "power", "users"]),
    ("Animals", &["key", "shield", "book-open", "x", "bug", "cat", "dog", "egg", "map", "nfc", "rat", "rss", "bell", "bird", "bone", "book"]),
    ("Arrows", &["activity", "hard-drive", "x", "book", "disc", "drum", "grip", "hand", "move", "redo", "undo", "brush", "minus", "trash", "book-x"]),
    ("Buildings", &["zap", "plug", "house", "printer", "book-open", "cog", "dam", "atom", "bolt", "book", "dock", "lamp", "tent", "blend"]),
    ("Charts", &["camera", "bug", "dot", "gpu", "flag", "type", "space", "rows-2", "rows-3", "rows-4", "pilcrow", "section"]),
    ("Communication", &["mail", "smartphone", "bot", "mic", "send", "inbox", "mails", "phone", "reply", "castle", "mail-x", "shrimp", "signal", "at-sign", "bot-off"]),
    ("Connectivity", &["wifi", "network", "tv", "rss", "cast", "flag", "link", "blend", "cable", "radio", "share", "link-2", "signal", "antenna", "contact", "forward"]),
    ("Cursors", &["rat", "hand", "lasso", "pointer", "asterisk", "keyboard", "signpost", "touchpad", "pointer-off", "keyboard-off", "lasso-select", "touchpad-off", "mouse-pointer", "spline-pointer", "mouse-pointer-2"]),
    ("Design", &["layers", "package", "mic", "pen", "crop", "scan", "blend", "brush", "frame", "gavel", "lasso", "eraser", "shapes"]),
    ("Coding & development", &["router", "server", "database", "terminal", "container", "hard-drive", "pi", "tv", "bug", "gpu", "code", "scan", "equal", "minus", "power", "regex", "circuit-board"]),
    ("Devices", &["cpu", "zap", "camera", "laptop", "router", "monitor", "printer", "activity", "hard-drive", "smartphone", "clapperboard", "tv", "eye", "gpu", "mic", "usb", "circuit-board"]),
    ("Emoji", &["usb", "cable", "heart", "sticker", "webhook", "grid-2x2", "grid-3x2", "hdmi-port", "heart-off", "land-plot", "scan-face", "thumbs-up", "book-heart", "grid-2x2-x"]),
    ("File icons", &["folder", "package", "book-open", "file-text", "folder-open", "book", "file", "files", "file-x", "scroll", "upload", "archive", "file-up", "folders", "bookmark"]),
    ("Finance", &["nfc", "euro", "coins", "vault", "wallet", "bitcoin", "percent", "receipt", "banknote", "currency", "landmark", "badge-cent", "badge-euro", "banknote-x", "hand-coins", "piggy-bank"]),
    ("Food & beverage", &["egg", "ham", "hop", "nut", "bean", "beef", "beer", "cake", "fish", "milk", "soup", "wine", "apple", "candy", "donut"]),
    ("Gaming", &["monitor", "book-open", "gamepad-2", "smartphone", "tv", "eye", "gpu", "usb", "cast", "club", "disc", "drum", "play", "dices", "heart", "piano"]),
    ("Home", &["house", "book-open", "bed", "book", "dock", "lamp", "sofa", "blinds", "book-a", "school"]),
    ("Layout", &["globe", "move", "earth", "fence", "shrub", "table", "cookie", "log-in", "repeat", "rows-2", "rows-3", "rows-4", "sprout", "tablet", "compass"]),
    ("Mail", &["mail", "rss", "send", "inbox", "mails", "reply", "space", "mail-x", "at-sign", "forward", "mailbox", "repeat-2"]),
    ("Mathematics", &["layers", "activity", "container", "x", "pi", "ham", "pin", "sun", "diff", "disc", "hash", "pill", "plus"]),
    ("Medical", &["activity", "dna", "bone", "pill", "apple", "brain", "cross", "salad", "carrot", "bandage", "dna-off", "syringe", "tablets"]),
    ("Multimedia", &["music", "camera", "monitor", "activity", "book-open", "hard-drive", "smartphone", "clapperboard", "tv", "ear", "eye", "gpu", "mic", "cast"]),
    ("Nature", &["cloud", "network", "activity", "book-open", "sun", "book", "leaf", "moon", "play", "rose", "soup", "star", "apple", "earth"]),
    ("Navigation & Places", &["router", "container", "map", "pin", "disc", "menu", "tent", "donut", "route", "shell", "shirt", "spool", "truck"]),
    ("Notifications", &["mail", "book-open", "rss", "tag", "bell", "book", "flag", "info", "send", "tags", "award", "mails", "siren", "book-a", "book-x"]),
    ("People", &["terminal", "axe", "baby", "drum", "mars", "rose", "user", "boxes", "flame", "ghost", "piano", "shell", "users", "venus", "blocks", "guitar"]),
    ("Photography", &["zap", "camera", "clapperboard", "gpu", "cctv", "crop", "film", "scan", "blend", "focus", "image", "video", "images", "search", "webcam"]),
    ("Science", &["book-open", "flask-conical", "dna", "tag", "atom", "book", "tags", "omega", "orbit", "beaker", "book-a", "magnet", "school"]),
    ("Seasons", &["sun", "leaf", "rose", "split", "wheat", "flower", "sprout", "sunset", "dessert", "eclipse", "rainbow", "sun-dim", "sunrise", "flower-2"]),
    ("Security", &["key", "lock", "shield", "pin", "bomb", "scan", "siren", "vault", "watch", "cookie", "cuboid", "magnet", "shapes", "webcam"]),
    ("Shapes", &["ban", "gem", "club", "cone", "play", "scan", "soup", "star", "crown", "power", "spade", "blocks", "circle", "radius", "shapes"]),
    ("Shopping", &["tag", "tags", "cable", "shirt", "store", "trash", "cuboid", "ticket", "barcode", "handbag", "luggage", "tickets", "backpack"]),
    ("Social", &["rss", "star", "vote", "heart", "share", "users", "user-x", "at-sign", "forward", "share-2", "sticker"]),
    ("Sports", &["activity", "bus", "egg", "bike", "play", "redo", "undo", "vote", "kayak", "medal", "redo-2", "target", "trophy"]),
    ("Sustainability", &["zap", "plug", "book-open", "container", "dam", "mic", "sun", "atom", "book", "cctv", "disc", "leaf", "wind", "apple", "cable"]),
    ("Text formatting", &["mail", "book-open", "dot", "rss", "bold", "book", "disc", "info", "type", "mails", "quote", "regex", "space", "book-a", "book-x"]),
    ("Time & calendar", &["x", "tv", "eye", "rss", "cake", "cctv", "rose", "ship", "clock", "ghost", "ruler", "timer", "watch", "copy-x", "sunset"]),
    ("Tools", &["plug", "house", "axe", "cog", "pin", "bolt", "info", "plus", "drill", "frame", "gavel", "hotel", "minus", "store"]),
    ("Transportation", &["activity", "container", "bus", "car", "gpu", "usb", "bike", "bomb", "club", "send", "ship", "cable", "cross", "heart"]),
    ("Travel", &["bus", "car", "map", "pin", "bike", "ship", "tent", "drone", "plane", "kanban", "locate"]),
    ("Weather", &["zap", "cloud", "router", "server", "sun", "wind", "split", "wheat", "blinds", "cloudy", "sunset", "dessert", "droplet", "eclipse"]),
];

pub const FOLDER: &str = "folder";
pub const FOLDER_OPEN: &str = "folder-open";
pub const MONITOR: &str = "monitor";
pub const CPU: &str = "cpu";
pub const LAYERS: &str = "layers";
pub const NETWORK: &str = "network";
pub const HARD_DRIVE: &str = "hard-drive";
pub const COG: &str = "cog";
pub const UPLOAD: &str = "upload";
pub const ACTIVITY: &str = "activity";
/// 进程列表（右面板进程页工具）。
pub const ROWS: &str = "rows-3";
/// 进程详情区标题行。
pub const INFO: &str = "info";
/// 进程详情区外部搜索按钮。
pub const SEARCH: &str = "search";
/// 传输区下载方向。
pub const DOWNLOAD: &str = "download";
/// 传输完成标记。
pub const CHECK: &str = "check";

/// 按节点名称猜测图标（中英文关键词，先命中先用）。
/// 空名/无命中回落到 fallback。
pub fn icon_for_name(name: &str, fallback: &str) -> String {
    let n = name.to_lowercase();
    const RULES: &[(&str, &str)] = &[
        // 容器
        ("docker", "container"),
        ("容器", "container"),
        ("k8s", "container"),
        ("kube", "container"),
        ("container", "container"),
        ("pod", "container"),
        // 服务器 / 云主机
        ("堡垒", "zap"),
        ("跳板", "zap"),
        ("服务器", "server"),
        ("主机", "server"),
        ("vps", "server"),
        ("ecs", "server"),
        ("cvm", "server"),
        ("轻量", "server"),
        ("server", "server"),
        ("host", "server"),
        // 硬件 / 板卡
        ("电路板", "circuit-board"),
        ("电路", "circuit-board"),
        ("板卡", "circuit-board"),
        ("主板", "circuit-board"),
        ("circuit", "circuit-board"),
        ("mainboard", "circuit-board"),
        // 数据库
        ("数据库", "database"),
        ("mysql", "database"),
        ("maria", "database"),
        ("postgres", "database"),
        ("pg", "database"),
        ("mongo", "database"),
        ("redis", "database"),
        ("memcach", "database"),
        ("clickhouse", "database"),
        ("sql", "database"),
        ("db", "database"),
        // 存储 / NAS
        ("nas", "hard-drive"),
        ("存储", "hard-drive"),
        ("硬盘", "hard-drive"),
        ("磁盘", "hard-drive"),
        ("备份", "hard-drive"),
        ("samba", "hard-drive"),
        ("nfs", "hard-drive"),
        ("storage", "hard-drive"),
        ("网盘", "cloud"),
        // 网络 / 安全
        ("路由", "router"),
        ("router", "router"),
        ("网关", "network"),
        ("gateway", "network"),
        ("交换机", "network"),
        ("负载", "layers"),
        ("防火墙", "shield"),
        ("firewall", "shield"),
        ("vpn", "lock"),
        ("代理", "network"),
        ("proxy", "network"),
        // Web
        ("www", "globe"),
        ("web", "globe"),
        ("http", "globe"),
        ("网站", "globe"),
        ("官网", "globe"),
        ("门户", "globe"),
        ("域名", "globe"),
        ("site", "globe"),
        ("blog", "globe"),
        // 开发
        ("git", "git-branch"),
        ("gogs", "git-branch"),
        ("gitea", "git-branch"),
        ("gitlab", "git-branch"),
        ("ci", "git-branch"),
        ("jenkins", "activity"),
        ("测试", "flask-conical"),
        ("test", "flask-conical"),
        ("开发", "terminal-square"),
        ("dev", "terminal-square"),
        ("build", "package"),
        ("编译", "package"),
        // 监控
        ("监控", "activity"),
        ("grafana", "activity"),
        ("prometheus", "activity"),
        ("zabbix", "activity"),
        ("alert", "activity"),
        ("告警", "activity"),
        ("日志", "file-text"),
        ("log", "file-text"),
        // 终端 / SSH
        ("终端", "terminal"),
        ("term", "terminal"),
        ("shell", "terminal"),
        ("ssh", "terminal"),
        ("cli", "terminal"),
        // 邮件 / 通讯
        ("邮件", "mail"),
        ("邮箱", "mail"),
        ("mail", "mail"),
        ("smtp", "mail"),
        // 影音娱乐
        ("音乐", "music"),
        ("music", "music"),
        ("歌", "music"),
        ("电影", "clapperboard"),
        ("视频", "clapperboard"),
        ("plex", "clapperboard"),
        ("jellyfin", "clapperboard"),
        ("emby", "clapperboard"),
        ("movie", "clapperboard"),
        ("游戏", "gamepad-2"),
        ("game", "gamepad-2"),
        ("mc ", "gamepad-2"),
        ("minecraft", "gamepad-2"),
        // 智能设备 / IoT
        ("摄像头", "camera"),
        ("摄像", "camera"),
        ("camera", "camera"),
        ("监控头", "camera"),
        ("打印机", "printer"),
        ("打印", "printer"),
        ("printer", "printer"),
        ("手机", "smartphone"),
        ("phone", "smartphone"),
        ("iot", "cpu"),
        ("家居", "house"),
        ("wifi", "wifi"),
        ("无线", "wifi"),
        // 密钥 / 证书
        ("密钥", "key"),
        ("证书", "key"),
        ("key", "key"),
        ("cert", "key"),
        ("ssl", "key"),
        ("vault", "key"),
        ("密码", "lock"),
        ("secret", "lock"),
        // 文档 / 知识
        ("文档", "book-open"),
        ("wiki", "book-open"),
        ("笔记", "book-open"),
        ("知识", "book-open"),
        ("book", "book-open"),
        ("主页", "house"),
        ("家", "house"),
        ("home", "house"),
        ("homelab", "house"),
        // 云
        ("阿里云", "cloud"),
        ("腾讯云", "cloud"),
        ("aws", "cloud"),
        ("azure", "cloud"),
        ("gcp", "cloud"),
        ("cloud", "cloud"),
        ("云", "cloud"),
    ];
    for (kw, icon) in RULES {
        if n.contains(kw) {
            return icon.to_string();
        }
    }
    fallback.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_matching() {
        assert_eq!(icon_for_name("MySQL 主库", "server"), "database");
        assert_eq!(icon_for_name("家用 NAS", "server"), "hard-drive");
        assert_eq!(icon_for_name("web-frontend", "server"), "globe");
        assert_eq!(icon_for_name("Docker 宿主机", "server"), "container");
        assert_eq!(icon_for_name("家庭路由器", "server"), "router");
        assert_eq!(icon_for_name("随便什么", "server"), "server");
    }
}
