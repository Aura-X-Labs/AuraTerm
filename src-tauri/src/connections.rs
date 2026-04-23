use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use tauri::{AppHandle, Manager};

const KEYRING_SERVICE: &str = "auraterm";

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

fn password_account(connection_id: &str) -> String {
    format!("connection:{connection_id}:password")
}

fn private_key_account(connection_id: &str) -> String {
    format!("connection:{connection_id}:private-key")
}

fn secure_entry(account: &str) -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, account)
        .map_err(|error| format!("Failed to access secure storage: {error}"))
}

fn secure_store(account: &str, value: Option<&str>) -> Result<(), String> {
    let entry = secure_entry(account)?;
    match value {
        Some(secret) if !secret.is_empty() => entry
            .set_password(secret)
            .map_err(|error| format!("Failed to save credential in secure storage: {error}")),
        _ => {
            let _ = entry.delete_credential();
            Ok(())
        }
    }
}

fn secure_load(account: &str) -> Option<String> {
    let entry = secure_entry(account).ok()?;
    entry.get_password().ok()
}

fn secure_delete(account: &str) {
    if let Ok(entry) = secure_entry(account) {
        let _ = entry.delete_credential();
    }
}

fn sanitize_connection_for_storage(connection: &mut SavedConnection) {
    connection.password = None;
    connection.private_key = None;
}

fn hydrate_connection_secrets(connection: &mut SavedConnection) {
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

    connection.password = secure_load(&password_account(&connection.id));
    connection.private_key = if connection.auth_type == "key" {
        secure_load(&private_key_account(&connection.id))
    } else {
        None
    };
}

fn persist_connection_secrets(connection: &SavedConnection) -> Result<(), String> {
    if connection.protocol != "ssh" {
        secure_delete(&password_account(&connection.id));
        secure_delete(&private_key_account(&connection.id));
        return Ok(());
    }

    match connection.auth_type.as_str() {
        "key" => {
            secure_store(&private_key_account(&connection.id), connection.private_key.as_deref())?;
            secure_store(&password_account(&connection.id), connection.password.as_deref())
        }
        "password" => {
            secure_store(&password_account(&connection.id), connection.password.as_deref())?;
            secure_store(&private_key_account(&connection.id), None)
        }
        _ => {
            secure_store(&password_account(&connection.id), None)?;
            secure_store(&private_key_account(&connection.id), None)
        }
    }
}

fn migrate_legacy_plaintext_secrets(connections: &mut [SavedConnection]) -> Result<bool, String> {
    let mut migrated = false;

    for connection in connections.iter_mut() {
        if connection.protocol != "ssh" {
            if connection.password.take().is_some() || connection.private_key.take().is_some() {
                migrated = true;
            }
            continue;
        }

        if connection.password.as_deref().is_some() {
            secure_store(&password_account(&connection.id), connection.password.as_deref())?;
            migrated = true;
        }

        if connection.private_key.as_deref().is_some() {
            secure_store(
                &private_key_account(&connection.id),
                connection.private_key.as_deref(),
            )?;
            migrated = true;
        }

        sanitize_connection_for_storage(connection);
    }

    Ok(migrated)
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
    let mut connections: Vec<SavedConnection> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    if migrate_legacy_plaintext_secrets(&mut connections)? {
        write_connections(app, &connections)?;
    }

    Ok(connections)
}

fn write_file_atomic(path: &std::path::Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid connections file path".to_string())?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Invalid connections file name".to_string())?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp_path = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));

    let mut tmp_file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp_path)
        .map_err(|error| format!("Failed to open temp connections file: {error}"))?;

    tmp_file
        .write_all(content.as_bytes())
        .map_err(|error| format!("Failed to write temp connections file: {error}"))?;
    tmp_file
        .sync_all()
        .map_err(|error| format!("Failed to flush temp connections file: {error}"))?;

    drop(tmp_file);

    fs::rename(&tmp_path, path)
        .map_err(|error| {
            let _ = fs::remove_file(&tmp_path);
            format!("Failed to atomically replace connections file: {error}")
        })?;

    Ok(())
}

fn write_connections(app: &AppHandle, connections: &Vec<SavedConnection>) -> Result<(), String> {
    let path = connections_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(connections).map_err(|e| e.to_string())?;
    write_file_atomic(&path, &content)?;
    Ok(())
}

/// 获取所有已保存的连接，按最近使用时间降序排列
#[tauri::command]
pub fn get_connections(app: AppHandle) -> Result<Vec<SavedConnection>, String> {
    let mut connections = load_connections(&app)?;
    for connection in &mut connections {
        hydrate_connection_secrets(connection);
    }

    // 按 last_used 降序（未使用过的排最后）
    connections.sort_by(|a, b| {
        let a_ts = a.last_used.unwrap_or(a.created_at);
        let b_ts = b.last_used.unwrap_or(b.created_at);
        b_ts.cmp(&a_ts)
    });
    Ok(connections)
}

/// 新增或更新连接（同 id 则覆盖，否则追加）
#[tauri::command]
pub fn save_connection(app: AppHandle, connection: SavedConnection) -> Result<String, String> {
    persist_connection_secrets(&connection)?;

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

/// 删除指定 id 的连接
#[tauri::command]
pub fn delete_connection(app: AppHandle, id: String) -> Result<(), String> {
    let mut connections = load_connections(&app)?;
    connections.retain(|c| c.id != id);
    write_connections(&app, &connections)?;
    secure_delete(&password_account(&id));
    secure_delete(&private_key_account(&id));
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