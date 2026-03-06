use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
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
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

fn write_connections(app: &AppHandle, connections: &Vec<SavedConnection>) -> Result<(), String> {
    let path = connections_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(connections).map_err(|e| e.to_string())?;
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}

/// 获取所有已保存的连接，按最近使用时间降序排列
#[tauri::command]
pub fn get_connections(app: AppHandle) -> Result<Vec<SavedConnection>, String> {
    let mut connections = load_connections(&app)?;
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
    let mut connections = load_connections(&app)?;
    let id = connection.id.clone();
    if let Some(existing) = connections.iter_mut().find(|c| c.id == id) {
        *existing = connection;
    } else {
        connections.push(connection);
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