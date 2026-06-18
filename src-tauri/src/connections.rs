use crate::encryption::{self, CredentialStore, MasterPasswordState, StoredCredential};
use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager, State};

fn default_protocol() -> String {
    "ssh".to_string()
}

fn default_auth_type() -> String {
    "password".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String, // "password" | "key" | "none"
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baud_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_bits: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_bits: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow_control: Option<String>,
    pub created_at: u64,
    pub last_used: Option<u64>,
    /// Legacy compatibility field; reconnect behavior is driven by reconnect_type.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_reconnect: bool,
    /// SSH reconnect mode: "manual", "simple", "tmux", or "screen" (default: "manual")
    #[serde(default = "default_reconnect_type", skip_serializing_if = "is_default_reconnect_type")]
    pub reconnect_type: String,
}

fn default_reconnect_type() -> String {
    "manual".to_string()
}

fn is_default_reconnect_type(value: &str) -> bool {
    value == "manual"
}

fn sanitize_connection_for_storage(connection: &mut SavedConnection) {
    connection.password = None;
    connection.private_key = None;
}

fn hydrate_connection_secrets(
    connection: &mut SavedConnection,
    credential_store: &CredentialStore,
) {
    if connection.protocol != "ssh" {
        connection.password = None;
        connection.private_key = None;
        return;
    }

    if connection.auth_type == "none" {
        connection.password = None;
        connection.private_key = None;
        return;
    }

    // 从凭据存储中查找并恢复凭据
    if let Some(stored) = credential_store
        .credentials
        .iter()
        .find(|c| c.connection_id == connection.id)
    {
        connection.password = stored.password.clone();
        connection.private_key = if connection.auth_type == "key" {
            stored.private_key.clone()
        } else {
            None
        };
    }
}

fn persist_connection_secrets(
    connection: &SavedConnection,
    credential_store: &mut CredentialStore,
) -> Result<(), String> {
    if connection.protocol != "ssh" {
        // 移除非 SSH 协议的凭据
        credential_store
            .credentials
            .retain(|c| c.connection_id != connection.id);
        return Ok(());
    }

    // 移除旧的凭据记录（如果存在）
    credential_store
        .credentials
        .retain(|c| c.connection_id != connection.id);

    match connection.auth_type.as_str() {
        "key" => {
            credential_store.credentials.push(StoredCredential {
                connection_id: connection.id.clone(),
                password: connection.password.clone(),
                private_key: connection.private_key.clone(),
            });
        }
        "password" => {
            credential_store.credentials.push(StoredCredential {
                connection_id: connection.id.clone(),
                password: connection.password.clone(),
                private_key: None,
            });
        }
        _ => {
            // "none" 认证方式，不存储凭据
        }
    }

    Ok(())
}

fn connections_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    Ok(config_dir.join("connections.json"))
}

fn load_connections(app: &AppHandle) -> Result<Vec<SavedConnection>, String> {
    let path = connections_path(app)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let connections: Vec<SavedConnection> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(connections)
}

fn write_connections(app: &AppHandle, connections: &Vec<SavedConnection>) -> Result<(), String> {
    let path = connections_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(connections).map_err(|e| e.to_string())?;
    crate::util::write_atomic(&path, &content)?;
    Ok(())
}

/// 获取所有已保存的连接，按最近使用时间降序排列。
///
/// 主密码从 [`MasterPasswordState`] 读取（应用启动时通过 verify_master_password 解锁）。
/// 如果主密码未解锁，则返回连接元数据但不包含密码/私钥（凭据字段为 None），
/// 让前端可以展示书签列表，但发起连接前仍需提示用户解锁主密码。
#[tauri::command]
pub fn get_connections(
    app: AppHandle,
    master_state: State<'_, MasterPasswordState>,
) -> Result<Vec<SavedConnection>, String> {
    let mut connections = load_connections(&app)?;

    // 本地密钥模式恒可访问；密码模式需已解锁。否则返回空凭据的连接元数据。
    if encryption::credentials_accessible(&app, &master_state) {
        // If the credential store is unreadable (e.g. corrupted or wrong key),
        // degrade gracefully: return connection metadata without secrets rather
        // than propagating an error that would hide the whole bookmark list.
        let credential_store = encryption::resolve_secret(&app, &master_state)
            .and_then(|secret| encryption::load_encrypted_credentials(&app, &secret))
            .unwrap_or_else(|e| {
                eprintln!("[get_connections] credential store unreadable ({}), returning empty credentials", e);
                encryption::CredentialStore { credentials: Vec::new() }
            });
        for connection in &mut connections {
            hydrate_connection_secrets(connection, &credential_store);
        }
    } else {
        // 未解锁时清空敏感字段
        for connection in &mut connections {
            connection.password = None;
            connection.private_key = None;
        }
    }

    // 按 last_used 降序（未使用过的排最后）
    connections.sort_by(|a, b| {
        let a_ts = a.last_used.unwrap_or(a.created_at);
        let b_ts = b.last_used.unwrap_or(b.created_at);
        b_ts.cmp(&a_ts)
    });
    Ok(connections)
}

/// 新增或更新连接（同 id 则覆盖，否则追加）。
/// 凭据会被加密存储。
#[tauri::command]
pub fn save_connection(
    app: AppHandle,
    connection: SavedConnection,
    master_state: State<'_, MasterPasswordState>,
) -> Result<String, String> {
    let secret = encryption::resolve_secret(&app, &master_state)?;

    // 加载现有凭据存储；若解密失败（如主密码曾被重置导致密文不兼容），则从空存储开始，
    // 避免因遗留文件阻断所有新连接的保存。
    let mut credential_store = encryption::load_encrypted_credentials(&app, &secret)
        .unwrap_or_else(|e| {
            eprintln!("[save_connection] credential store unreadable ({}), resetting", e);
            encryption::CredentialStore { credentials: Vec::new() }
        });

    // 持久化凭据
    persist_connection_secrets(&connection, &mut credential_store)?;

    // 保存加密的凭据存储
    encryption::save_encrypted_credentials(&app, &credential_store, &secret)?;

    // 加载现有连接并保存元数据（凭据已从连接中移除）
    let mut connections = load_connections(&app)?;
    let id = connection.id.clone();
    let mut persisted_connection = connection;
    sanitize_connection_for_storage(&mut persisted_connection);

    if let Some(existing) = connections.iter_mut().find(|c| c.id == id) {
        *existing = persisted_connection;
    } else {
        connections.push(persisted_connection);
    }
    write_connections(&app, &connections)?;
    Ok(id)
}

/// 删除指定 id 的连接。
#[tauri::command]
pub fn delete_connection(
    app: AppHandle,
    id: String,
    master_state: State<'_, MasterPasswordState>,
) -> Result<(), String> {
    let secret = encryption::resolve_secret(&app, &master_state)?;

    // 从凭据存储中移除；若旧文件解密失败则从空存储开始（相当于凭据已丢失，继续删除连接元数据）
    let mut credential_store = encryption::load_encrypted_credentials(&app, &secret)
        .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() });
    credential_store
        .credentials
        .retain(|c| c.connection_id != id);
    encryption::save_encrypted_credentials(&app, &credential_store, &secret)?;

    // 从连接列表中移除
    let mut connections = load_connections(&app)?;
    connections.retain(|c| c.id != id);
    write_connections(&app, &connections)?;
    Ok(())
}

/// 更新连接的 last_used 时间戳（每次建立连接时调用）
#[tauri::command]
pub fn touch_connection(app: AppHandle, id: String, timestamp: u64) -> Result<(), String> {
    let mut connections = load_connections(&app)?;
    if let Some(conn) = connections.iter_mut().find(|c| c.id == id) {
        conn.last_used = Some(timestamp);
        write_connections(&app, &connections)?;
    }
    Ok(())
}
