//! 认证信息模型 + 旧版 hosts.json 兼容读取。
//! 持久化已迁移 SQLite（见 db.rs）；此文件仅保留数据结构与迁移解析。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum StoredAuth {
    /// 明文口令（本地库文件 0600；后续换 keyring）
    Password(Option<String>),
    /// 私钥文本内容（PEM / OpenSSH 格式）；旧版存文件路径，由 db 层迁移为内容。
    Key { key: String, passphrase: Option<String> },
}

/// 旧版 ~/.sshrust/hosts.json 的条目（仅迁移用）。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HostEntry {
    pub id: u64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: StoredAuth,
}

/// 旧版主机清单路径（迁移后保留不删）。
pub fn legacy_path() -> PathBuf {
    let mut p = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()));
    p.push(".sshrust");
    p.push("hosts.json");
    p
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::Path::new(&home).join(rest);
        }
    }
    PathBuf::from(path)
}
