//! 侧栏树持久化：SQLite（~/.sshrust/sshrust.db）。
//!
//! 表结构：
//! - tree_node(id, parent_id, kind, name, sort, data) —— data 为按类型区分的 JSON；
//!   parent_id 指向父节点，两个固定根哨兵不落库：ROOT_SERVERS / ROOT_COMMANDS。
//! - settings(key, value) —— 界面状态（分割线高度、展开状态等）。
//!
//! 首次启动时若库为空且存在旧版 hosts.json，自动迁移为「连接」下的服务器节点。

use std::path::PathBuf;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::hosts::{HostEntry, StoredAuth};

/// 「连接」分组根哨兵（不落库，AUTOINCREMENT 从 1 开始，负数不会冲突）。
pub const ROOT_SERVERS: i64 = -1;
/// 「常用命令」分组根哨兵。
pub const ROOT_COMMANDS: i64 = -2;
/// 「专用命令」分组哨兵（组常驻；内容按当前连接服务器动态加载，
/// 仅用于展开集持久化的 id 空间映射，不对应任何落库节点）。
pub const ROOT_SERVER_CMDS: i64 = -3;

/// 连接方式：IP 直连（host:port）或 iroh P2P（host 存 NodeID，port 恒 0）。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectMode {
    #[default]
    Ip,
    Iroh,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ServerData {
    /// Ip 模式 = 主机名/IP；Iroh 模式 = 目标 agent 的 NodeID（64 hex）。
    pub host: String,
    /// Iroh 模式未用（恒 0）。
    pub port: u16,
    pub username: String,
    pub auth: StoredAuth,
    #[serde(default)]
    pub mode: ConnectMode,
    /// Iroh：自建 relay URL（空 = n0 公共设施）。
    #[serde(default)]
    pub iroh_relay: String,
    /// Iroh：relay 访问令牌（无鉴权的 relay 留空）。
    #[serde(default)]
    pub iroh_token: String,
    /// 机器配置摘要（如 "2C 4G 60G"；登录后从系统信息捕获，展示在树行徽章）。
    #[serde(default)]
    pub spec: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct CommandData {
    pub command: String,
    /// 说明（可选；树行尾弱化显示）。
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Folder,
    Server(ServerData),
    Command(CommandData),
}

impl NodeKind {
    fn as_str(&self) -> &'static str {
        match self {
            NodeKind::Folder => "folder",
            NodeKind::Server(_) => "server",
            NodeKind::Command(_) => "command",
        }
    }

    fn from_db(kind: &str, data: &str) -> NodeKind {
        match kind {
            "server" => NodeKind::Server(serde_json::from_str(data).unwrap_or(ServerData {
                host: String::new(),
                port: 22,
                username: String::new(),
                auth: StoredAuth::Password(None),
                mode: ConnectMode::Ip,
                iroh_relay: String::new(),
                iroh_token: String::new(),
                spec: String::new(),
            })),
            "command" => NodeKind::Command(serde_json::from_str(data).unwrap_or_default()),
            _ => NodeKind::Folder,
        }
    }
}

/// 节点/根所在的树（三棵互不相通的树，拖拽合法域判定用）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeScope {
    /// 服务器树（ROOT_SERVERS 及其下的 folder/server）。
    Servers,
    /// 通用命令树（ROOT_COMMANDS 及其下的 folder/command）。
    GeneralCommands,
    /// 某服务器的专用命令树（服务器节点自身 = 树根，专用命令挂其下）。
    ServerCommands(i64),
}

#[derive(Clone, Debug)]
pub struct TreeNode {
    pub id: i64,
    pub parent: i64,
    pub kind: NodeKind,
    pub name: String,
    /// 显式图标名；空 = 按名称自动匹配（icons.rs）。
    pub icon: String,
    /// 排序值（当前按插入顺序递增，预留给拖拽排序）。
    #[allow(dead_code)]
    pub sort: i64,
}

/// 新建节点的入参。
pub struct NewNode {
    pub parent: i64,
    pub kind: NodeKind,
    pub name: String,
}

pub struct Db {
    conn: Connection,
    /// 内存中的全量节点缓存；变更走 write-through，规模小，直接全量重读也行。
    pub nodes: Vec<TreeNode>,
}

impl Db {
    pub fn open() -> Db {
        let path = Self::db_path();
        let conn = Connection::open(&path).expect("open sshrust.db");
        #[cfg(unix)]
        if let Some(dir) = path.parent() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let mut db = Self::with_conn(conn);
        db.migrate_legacy_hosts();
        db.migrate_legacy_key_paths();
        db.reload();
        db
    }

    fn db_path() -> PathBuf {
        let mut p = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()));
        p.push(".sshrust");
        let _ = std::fs::create_dir_all(&p);
        p.push("sshrust.db");
        p
    }

    fn with_conn(conn: Connection) -> Db {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tree_node (
                 id        INTEGER PRIMARY KEY AUTOINCREMENT,
                 parent_id INTEGER NOT NULL,
                 kind      TEXT    NOT NULL,
                 name      TEXT    NOT NULL,
                 sort      INTEGER NOT NULL DEFAULT 0,
                 data      TEXT    NOT NULL DEFAULT '{}'
             );
             CREATE INDEX IF NOT EXISTS idx_tree_node_parent ON tree_node(parent_id);
             CREATE TABLE IF NOT EXISTS settings (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS style_template (
                 id   INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL,
                 data TEXT NOT NULL
             );",
        )
        .expect("init sshrust.db schema");
        // 旧库升级：图标列（空串 = 按名称自动匹配）
        let _ = conn.execute("ALTER TABLE tree_node ADD COLUMN icon TEXT NOT NULL DEFAULT ''", []);
        let mut db = Db { conn, nodes: Vec::new() };
        db.reload();
        db
    }

    /// 首次启动：hosts.json → 「连接」下的服务器节点（旧文件保留不动）。
    fn migrate_legacy_hosts(&mut self) {
        let empty: bool = self
            .conn
            .query_row("SELECT COUNT(*) = 0 FROM tree_node", [], |r| r.get(0))
            .unwrap_or(true);
        if !empty {
            return;
        }
        let Ok(text) = std::fs::read_to_string(crate::hosts::legacy_path()) else {
            return;
        };
        let Ok(entries) = serde_json::from_str::<Vec<HostEntry>>(&text) else {
            return;
        };
        for e in entries {
            let data = ServerData {
                host: e.host,
                port: e.port,
                username: e.username,
                auth: e.auth,
                mode: ConnectMode::Ip,
                iroh_relay: String::new(),
                iroh_token: String::new(),
                spec: String::new(),
            };
            let _ = self.insert_raw(NewNode {
                parent: ROOT_SERVERS,
                name: e.name,
                kind: NodeKind::Server(data),
            });
        }
    }

    /// 旧版密钥认证存的是文件路径（StoredAuth::Key{path}）；新模型只存内容。
    /// 把 path 指向的文件内容读进来重写 data；读不到时保留原值（连接时报解析错误）。
    fn migrate_legacy_key_paths(&mut self) {
        let rows: Vec<(i64, String)> = {
            let mut stmt = match self
                .conn
                .prepare("SELECT id, data FROM tree_node WHERE kind = 'server'")
            {
                Ok(s) => s,
                Err(_) => return,
            };
            match stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => return,
            }
        };
        for (id, data) in rows {
            let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&data) else { continue };
            let Some(auth) = value.get_mut("auth").and_then(|a| a.as_object_mut()) else { continue };
            let Some(key_obj) = auth.get_mut("Key").and_then(|k| k.as_object_mut()) else { continue };
            let Some(path) = key_obj.get("path").and_then(|p| p.as_str()) else { continue };
            let path = path.to_string();
            let content = std::fs::read_to_string(crate::hosts::expand_tilde(&path))
                .ok()
                .filter(|c| c.contains("PRIVATE KEY"))
                .unwrap_or(path);
            key_obj.remove("path");
            key_obj.insert("key".into(), serde_json::Value::String(content));
            if let Ok(new_data) = serde_json::to_string(&value) {
                let _ = self.conn.execute(
                    "UPDATE tree_node SET data = ?2 WHERE id = ?1",
                    rusqlite::params![id, new_data],
                );
            }
        }
    }

    fn reload(&mut self) {
        let mut stmt = self
            .conn
            .prepare("SELECT id, parent_id, kind, name, sort, data, icon FROM tree_node ORDER BY sort, id")
            .expect("prepare tree_node select");
        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let parent: i64 = row.get(1)?;
                let kind: String = row.get(2)?;
                let name: String = row.get(3)?;
                let sort: i64 = row.get(4)?;
                let data: String = row.get(5)?;
                let icon: String = row.get(6).unwrap_or_default();
                Ok(TreeNode { id, parent, kind: NodeKind::from_db(&kind, &data), name, icon, sort })
            })
            .expect("query tree_node");
        self.nodes = rows.filter_map(|r| r.ok()).collect();
    }

    /// 子节点列表（按 sort、id 排序，reload 已保证顺序）。
    pub fn children(&self, parent: i64) -> Vec<&TreeNode> {
        self.nodes.iter().filter(|n| n.parent == parent).collect()
    }

    pub fn node(&self, id: i64) -> Option<&TreeNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// 节点所在子树是否挂在某个根下（沿 parent 上溯）。
    pub fn under_root(&self, id: i64, root: i64) -> bool {
        let mut cur = id;
        loop {
            if cur == root {
                return true;
            }
            match self.node(cur) {
                Some(n) if n.parent != cur => cur = n.parent,
                _ => return false,
            }
        }
    }

    /// 按 host:port 找服务器节点（专用命令组定位当前连接服务器的命令树）。
    pub fn server_by_addr(&self, host: &str, port: u16) -> Option<&TreeNode> {
        self.nodes.iter().find(|n| match &n.kind {
            NodeKind::Server(d) => d.host == host && d.port == port,
            _ => false,
        })
    }

    /// 节点归属的命令树：沿 parent 上溯遇到的第一个 Server 节点即其「专用命令根」。
    pub fn owner_server(&self, id: i64) -> Option<i64> {
        let mut cur = id;
        loop {
            match self.node(cur) {
                Some(n) if n.parent != cur => {
                    if matches!(self.node(n.parent).map(|p| &p.kind), Some(NodeKind::Server(_))) {
                        return Some(n.parent);
                    }
                    cur = n.parent;
                }
                _ => return None,
            }
        }
    }

    /// 一个节点/根哨兵所在的树；不存在返回 None。
    /// 注意 Server 节点自身按「专用命令树根」归类——移动校验应传**父节点**判定成员身份。
    pub fn tree_scope(&self, of: i64) -> Option<TreeScope> {
        match of {
            ROOT_SERVERS => Some(TreeScope::Servers),
            ROOT_COMMANDS => Some(TreeScope::GeneralCommands),
            _ => {
                let n = self.node(of)?;
                if matches!(n.kind, NodeKind::Server(_)) {
                    return Some(TreeScope::ServerCommands(of));
                }
                if let Some(sid) = self.owner_server(of) {
                    Some(TreeScope::ServerCommands(sid))
                } else if self.under_root(of, ROOT_COMMANDS) {
                    Some(TreeScope::GeneralCommands)
                } else {
                    Some(TreeScope::Servers)
                }
            }
        }
    }

    pub fn insert(&mut self, new: NewNode) -> TreeNode {
        let node = self.insert_raw(new);
        self.reload();
        self.node(node.id).expect("just inserted").clone()
    }

    fn insert_raw(&mut self, new: NewNode) -> TreeNode {
        let sort: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(sort), 0) + 1 FROM tree_node WHERE parent_id = ?1",
                [new.parent],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let data = kind_payload(&new.kind);
        self.conn
            .execute(
                "INSERT INTO tree_node (parent_id, kind, name, sort, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![new.parent, new.kind.as_str(), new.name, sort, data],
            )
            .expect("insert tree_node");
        let id = self.conn.last_insert_rowid();
        TreeNode { id, parent: new.parent, kind: new.kind, name: new.name, icon: String::new(), sort }
    }

    /// 就地更新节点的名称与类型数据（「编辑服务器」保存走这里，而非新增）。
    /// 不改 parent/sort/icon —— 移动用 move_node，图标用 set_node_icon。
    pub fn update_node(&mut self, id: i64, name: &str, kind: NodeKind) {
        let data = kind_payload(&kind);
        self.conn
            .execute(
                "UPDATE tree_node SET name = ?2, kind = ?3, data = ?4 WHERE id = ?1",
                rusqlite::params![id, name, kind.as_str(), data],
            )
            .expect("update tree_node");
        self.reload();
    }

    pub fn rename(&mut self, id: i64, name: &str) {
        self.conn
            .execute("UPDATE tree_node SET name = ?2 WHERE id = ?1", rusqlite::params![id, name])
            .expect("rename tree_node");
        self.reload();
    }

    /// 设置节点显式图标（空串恢复为自动匹配）。图标选择器确认回调写入。
    pub fn set_node_icon(&mut self, id: i64, icon: &str) {
        let _ = self.conn.execute(
            "UPDATE tree_node SET icon = ?2 WHERE id = ?1",
            rusqlite::params![id, icon],
        );
        self.reload();
    }

    /// 把节点移动到新父目录下（排到末尾）。调用方负责校验目标不是自身子孙。
    pub fn move_node(&mut self, id: i64, new_parent: i64) {
        let sort: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(sort), 0) + 1 FROM tree_node WHERE parent_id = ?1",
                [new_parent],
                |r| r.get(0),
            )
            .unwrap_or(1);
        let _ = self.conn.execute(
            "UPDATE tree_node SET parent_id = ?2, sort = ?3 WHERE id = ?1",
            rusqlite::params![id, new_parent, sort],
        );
        self.reload();
    }

    /// 递归删除节点及其子孙。
    pub fn delete(&mut self, id: i64) {
        let mut queue = vec![id];
        while let Some(cur) = queue.pop() {
            queue.extend(self.nodes.iter().filter(|n| n.parent == cur).map(|n| n.id));
            let _ = self.conn.execute("DELETE FROM tree_node WHERE id = ?1", [cur]);
        }
        self.reload();
    }

    // ---------- settings ----------

    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .ok()
    }

    pub fn set_setting(&mut self, key: &str, value: &str) {
        let _ = self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        );
    }

    // ---------- 用户自定义着色模板 ----------

    /// 全部用户模板（id, 名称, JSON 数据）。
    pub fn list_style_templates(&self) -> Vec<(i64, String, String)> {
        let Ok(mut stmt) =
            self.conn.prepare("SELECT id, name, data FROM style_template ORDER BY id")
        else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok).collect()
    }

    /// 新建模板，返回 id。
    pub fn insert_style_template(&mut self, name: &str, data: &str) -> i64 {
        let _ = self.conn.execute(
            "INSERT INTO style_template (name, data) VALUES (?1, ?2)",
            rusqlite::params![name, data],
        );
        self.conn.last_insert_rowid()
    }

    pub fn update_style_template(&mut self, id: i64, name: &str, data: &str) {
        let _ = self.conn.execute(
            "UPDATE style_template SET name = ?2, data = ?3 WHERE id = ?1",
            rusqlite::params![id, name, data],
        );
    }

    pub fn delete_style_template(&mut self, id: i64) {
        let _ = self.conn.execute("DELETE FROM style_template WHERE id = ?1", rusqlite::params![id]);
    }

    /// 取指定用户模板的数据。
    pub fn style_template_data(&self, id: i64) -> Option<String> {
        self.conn
            .query_row("SELECT data FROM style_template WHERE id = ?1", [id], |r| r.get(0))
            .ok()
    }
}

// ---------- iroh 全局配置（P2P 中继；原按服务器配置已迁全局） ----------

/// settings 键：全局 iroh relay URL（空/缺省 = n0 公共设施）。
pub const SETTING_IROH_RELAY: &str = "iroh.relay";
/// settings 键：全局 iroh relay 访问令牌（无鉴权 relay 留空）。
pub const SETTING_IROH_TOKEN: &str = "iroh.token";
/// 迁移完成标记（旧版按服务器存的 relay/令牌 → 全局，只跑一次）。
const SETTING_IROH_MIGRATED: &str = "iroh.global.migrated";

/// 全局 iroh relay/令牌（空串 → None = n0 公共设施）。
pub fn iroh_global(db: &Db) -> (Option<String>, Option<String>) {
    let pick = |key: &str| {
        db.get_setting(key)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    (pick(SETTING_IROH_RELAY), pick(SETTING_IROH_TOKEN))
}

/// 旧版 relay/令牌按服务器节点存放；迁到全局设置（首个非空值，只跑一次，
/// 已有全局值不覆盖）。AppUi 启动时调用。
pub fn migrate_iroh_to_global(db: &mut Db) {
    if db.get_setting(SETTING_IROH_MIGRATED).as_deref() == Some("1") {
        return;
    }
    let mut relay: Option<String> = None;
    let mut token: Option<String> = None;
    for n in &db.nodes {
        if let NodeKind::Server(d) = &n.kind {
            if relay.is_none() && !d.iroh_relay.trim().is_empty() {
                relay = Some(d.iroh_relay.trim().to_string());
            }
            if token.is_none() && !d.iroh_token.trim().is_empty() {
                token = Some(d.iroh_token.trim().to_string());
            }
        }
    }
    if let Some(r) = relay {
        db.set_setting(SETTING_IROH_RELAY, &r);
    }
    if let Some(t) = token {
        db.set_setting(SETTING_IROH_TOKEN, &t);
    }
    db.set_setting(SETTING_IROH_MIGRATED, "1");
}

/// 节点类型 → data 列的 JSON 载荷。
fn kind_payload(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Folder => "{}".to_string(),
        NodeKind::Server(d) => serde_json::to_string(d).unwrap_or_else(|_| "{}".into()),
        NodeKind::Command(d) => serde_json::to_string(d).unwrap_or_else(|_| "{}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Db {
        Db::with_conn(Connection::open_in_memory().expect("memory db"))
    }

    /// 用户自定义着色模板：插入/更新/读取/删除闭环。
    #[test]
    fn style_template_crud() {
        let mut db = mem_db();
        let tpl = gpui_terminal::StyleTemplate::standard();
        let data = serde_json::to_string(&tpl).unwrap();

        let id1 = db.insert_style_template("我的模板", &data);
        let id2 = db.insert_style_template("另一套", &data);
        assert_ne!(id1, id2);
        assert_eq!(db.list_style_templates().len(), 2);

        db.update_style_template(id1, "改名", &data);
        assert!(db.list_style_templates().iter().any(|(id, name, _)| *id == id1 && name == "改名"));

        let got: gpui_terminal::StyleTemplate =
            serde_json::from_str(&db.style_template_data(id1).unwrap()).unwrap();
        assert_eq!(got, tpl);

        db.delete_style_template(id1);
        assert!(db.style_template_data(id1).is_none());
        assert_eq!(db.list_style_templates().len(), 1);
        // 幂等：删除不存在的不报错。
        db.delete_style_template(id1);
    }

    #[test]
    fn insert_query_delete() {
        let mut db = mem_db();
        let a = db.insert(NewNode { parent: ROOT_SERVERS, kind: NodeKind::Folder, name: "dev".into() });
        let b = db.insert(NewNode {
            parent: a.id,
            kind: NodeKind::Server(ServerData {
                host: "10.0.0.1".into(),
                port: 22,
                username: "root".into(),
                auth: StoredAuth::Password(None),
                mode: ConnectMode::Ip,
                iroh_relay: String::new(),
                iroh_token: String::new(),
                spec: String::new(),
            }),
            name: "web".into(),
        });
        assert_eq!(db.children(ROOT_SERVERS).len(), 1);
        assert_eq!(db.children(a.id)[0].name, "web");
        assert!(db.under_root(b.id, ROOT_SERVERS));
        assert!(!db.under_root(b.id, ROOT_COMMANDS));

        db.delete(a.id);
        assert!(db.children(ROOT_SERVERS).is_empty());
        assert!(db.node(b.id).is_none());
    }

    #[test]
    fn settings_roundtrip() {
        let mut db = mem_db();
        assert!(db.get_setting("k").is_none());
        db.set_setting("k", "110");
        db.set_setting("k", "120");
        assert_eq!(db.get_setting("k").as_deref(), Some("120"));
    }

    /// relay/令牌全局化迁移：首个非空服务器值迁入全局；标记置位后不再重复跑
    ///（用户随后清空全局不会被旧值回填）。
    #[test]
    fn iroh_settings_migrate_once() {
        let mut db = mem_db();
        db.insert(NewNode {
            parent: ROOT_SERVERS,
            kind: NodeKind::Server(ServerData {
                host: "a".repeat(64),
                port: 0,
                username: "u".into(),
                auth: StoredAuth::Password(None),
                mode: ConnectMode::Iroh,
                iroh_relay: "https://relay.example.com".into(),
                iroh_token: "tok".into(),
                spec: String::new(),
            }),
            name: "p2p".into(),
        });
        migrate_iroh_to_global(&mut db);
        assert_eq!(
            iroh_global(&db),
            (
                Some("https://relay.example.com".to_string()),
                Some("tok".to_string())
            )
        );
        // 标记已置位：清空全局后重跑不回填。
        db.set_setting(SETTING_IROH_RELAY, "");
        db.set_setting(SETTING_IROH_TOKEN, "");
        migrate_iroh_to_global(&mut db);
        assert_eq!(iroh_global(&db), (None, None));
    }

    fn sample_tree(db: &mut Db) -> (i64, i64, i64) {
        // 连接/─web(server)；通用命令/─cmd1；web 下挂专用命令 scmd1。
        let server = db.insert(NewNode {
            parent: ROOT_SERVERS,
            kind: NodeKind::Server(ServerData {
                host: "10.0.0.1".into(),
                port: 22,
                username: "root".into(),
                auth: StoredAuth::Password(None),
                mode: ConnectMode::Ip,
                iroh_relay: String::new(),
                iroh_token: String::new(),
                spec: String::new(),
            }),
            name: "web".into(),
        });
        let general = db.insert(NewNode {
            parent: ROOT_COMMANDS,
            kind: NodeKind::Command(CommandData { command: "df -h".into(), description: String::new() }),
            name: "disk".into(),
        });
        let special = db.insert(NewNode {
            parent: server.id,
            kind: NodeKind::Command(CommandData { command: "uptime".into(), description: "负载".into() }),
            name: "up".into(),
        });
        (server.id, general.id, special.id)
    }

    /// 三棵树的归属判定：服务器树 / 通用命令树 / 每服务器一棵专用命令树。
    #[test]
    fn tree_scope_partitions() {
        let mut db = mem_db();
        let (sid, gid, scid) = sample_tree(&mut db);
        assert_eq!(db.tree_scope(ROOT_SERVERS), Some(TreeScope::Servers));
        assert_eq!(db.tree_scope(ROOT_COMMANDS), Some(TreeScope::GeneralCommands));
        // Server 节点自身 = 专用命令树根。
        assert_eq!(db.tree_scope(sid), Some(TreeScope::ServerCommands(sid)));
        // 成员按父节点判定：通用命令 → 通用树；专用命令 → 对应服务器。
        assert_eq!(db.tree_scope(gid), Some(TreeScope::GeneralCommands));
        assert_eq!(db.tree_scope(scid), Some(TreeScope::ServerCommands(sid)));
        // 服务器树成员与另一服务器的专用命令树互不相通。
        let other = db.insert(NewNode {
            parent: ROOT_SERVERS,
            kind: NodeKind::Server(ServerData {
                host: "10.0.0.2".into(),
                port: 22,
                username: "root".into(),
                auth: StoredAuth::Password(None),
                mode: ConnectMode::Ip,
                iroh_relay: String::new(),
                iroh_token: String::new(),
                spec: String::new(),
            }),
            name: "db".into(),
        });
        assert_ne!(db.tree_scope(other.id), db.tree_scope(sid));
        assert_eq!(db.server_by_addr("10.0.0.1", 22).map(|n| n.id), Some(sid));
        assert!(db.server_by_addr("10.0.0.9", 22).is_none());
    }

    /// 专用命令挂服务器节点下：删除服务器级联删命令；说明字段随 data 落库回读。
    #[test]
    fn server_commands_cascade_and_description() {
        let mut db = mem_db();
        let (sid, _, scid) = sample_tree(&mut db);
        db.delete(sid);
        assert!(db.node(scid).is_none(), "删服务器应级联删其专用命令");

        let (sid2, _, _) = sample_tree(&mut db);
        let node = db.node(sid2).unwrap();
        let NodeKind::Server(d) = &node.kind else { panic!("server") };
        let found = db.server_by_addr(&d.host, d.port).unwrap();
        let _ = found;
        // 说明字段 roundtrip：改写后回读。
        let cmd = db.children(sid2)[0].clone();
        let NodeKind::Command(cd) = cmd.kind else { panic!("command") };
        assert_eq!(cd.description, "负载");
        assert_eq!(cd.command, "uptime");
    }

    #[test]
    fn update_node_replaces_in_place() {
        let mut db = mem_db();
        let node = db.insert(NewNode {
            parent: ROOT_SERVERS,
            kind: NodeKind::Server(ServerData {
                host: "10.0.0.1".into(),
                port: 22,
                username: "root".into(),
                auth: StoredAuth::Password(None),
                mode: ConnectMode::Ip,
                iroh_relay: String::new(),
                iroh_token: String::new(),
                spec: String::new(),
            }),
            name: "old".into(),
        });
        db.update_node(
            node.id,
            "alias",
            NodeKind::Server(ServerData {
                host: "10.0.0.2".into(),
                port: 2222,
                username: "admin".into(),
                auth: StoredAuth::Key { key: "-----BEGIN OPENSSH PRIVATE KEY-----".into(), passphrase: None },
                mode: ConnectMode::Ip,
                iroh_relay: String::new(),
                iroh_token: String::new(),
                spec: String::new(),
            }),
        );
        // 仍是同一节点（数量不变、id 不变、parent/sort 保留），内容已替换。
        assert_eq!(db.children(ROOT_SERVERS).len(), 1);
        let updated = db.node(node.id).expect("node kept");
        assert_eq!(updated.name, "alias");
        let NodeKind::Server(d) = &updated.kind else { panic!("kind kept") };
        assert_eq!(d.host, "10.0.0.2");
        assert_eq!(d.port, 2222);
        assert_eq!(
            d.auth,
            StoredAuth::Key { key: "-----BEGIN OPENSSH PRIVATE KEY-----".into(), passphrase: None }
        );
        assert_eq!(d.mode, ConnectMode::Ip);
    }

    /// 旧版 Key{path} 数据：能读到文件时迁移为内容，读不到时保留原值不丢节点。
    #[test]
    fn migrate_legacy_key_paths_reads_file_content() {
        let mut db = mem_db();
        let key_file = std::env::temp_dir().join("sshrust_test_key.pem");
        std::fs::write(&key_file, "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n")
            .expect("write temp key");
        let legacy = format!(
            r#"{{"host":"h","port":22,"username":"u","auth":{{"Key":{{"path":"{}","passphrase":null}}}}}}"#,
            key_file.display()
        );
        db.conn
            .execute(
                "INSERT INTO tree_node (parent_id, kind, name, sort, data) VALUES (?1, 'server', 'n', 1, ?2)",
                rusqlite::params![ROOT_SERVERS, legacy],
            )
            .expect("insert legacy row");

        db.migrate_legacy_key_paths();
        db.reload();
        let node = db.node(1).expect("node kept");
        let NodeKind::Server(d) = &node.kind else { panic!("server") };
        match &d.auth {
            StoredAuth::Key { key, .. } => assert!(key.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")),
            other => panic!("auth should stay Key, got {other:?}"),
        }

        // 指向不存在文件的路径：保留原字符串（连接时报解析错误），不静默变 Password。
        db.conn
            .execute(
                "INSERT INTO tree_node (parent_id, kind, name, sort, data) VALUES (?1, 'server', 'm', 2, ?2)",
                rusqlite::params![ROOT_SERVERS, r#"{"host":"h2","port":22,"username":"u","auth":{"Key":{"path":"/nonexistent/key","passphrase":null}}}"#],
            )
            .expect("insert legacy row 2");
        db.migrate_legacy_key_paths();
        db.reload();
        let node2 = db.node(2).expect("node kept");
        let NodeKind::Server(d2) = &node2.kind else { panic!("server 2") };
        assert_eq!(
            d2.auth,
            StoredAuth::Key { key: "/nonexistent/key".into(), passphrase: None }
        );
        let _ = std::fs::remove_file(&key_file);
    }
}
