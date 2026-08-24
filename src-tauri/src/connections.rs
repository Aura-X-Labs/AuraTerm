use crate::encryption::{
    self, CredentialStore, MasterPasswordState, StoredCredential, StoredJumpCredential,
};
use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager, State};
use std::collections::{HashMap, HashSet};

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
    pub auth_type: String, // "password" | "key" | "agent" | "none"
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_forwarding: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jump_hosts: Vec<SavedJumpHost>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_login_rules: Vec<SavedAutoLoginRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_connect_commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_name: Option<String>,
    /// Serial transport: `"local"` (the default when absent), `"rfc2217"` or
    /// `"raw-tcp"`. Network transports keep their endpoint in `host`/`port`
    /// above rather than growing a parallel pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_transport: Option<String>,
    /// RFC 2217 only: negotiate the option but adopt the server's existing port
    /// settings instead of pushing our own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt_server_params: Option<bool>,
    /// Network serial only: come back automatically when the link drops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_auto_reconnect: Option<bool>,
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
    /// Saved port-forwarding tunnels (SSH only). Stored in plaintext alongside
    /// connection metadata — these carry no secrets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tunnels: Vec<SavedTunnel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedJumpHost {
    pub id: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedAutoLoginRule {
    pub expect: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub case_sensitive: bool,
    #[serde(default = "default_expect_timeout", skip_serializing_if = "is_default_expect_timeout")]
    pub timeout_secs: u64,
}

fn default_ssh_port() -> u16 { 22 }
fn default_expect_timeout() -> u64 { 30 }
fn is_default_expect_timeout(value: &u64) -> bool { *value == 30 }

/// Persisted definition of a single port-forwarding tunnel. Mirrors the
/// frontend `TunnelConfig`; behaviour lives in `ssh::forwarding`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedTunnel {
    pub id: String,
    #[serde(rename = "type")]
    pub tunnel_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_address: Option<String>,
    pub bind_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_port: Option<u16>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_start: bool,
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
    connection.passphrase = None;
    for jump in &mut connection.jump_hosts {
        jump.password = None;
        jump.private_key = None;
        jump.passphrase = None;
    }
    for rule in &mut connection.auto_login_rules {
        rule.response = None;
    }
    connection.post_connect_commands.clear();
}

fn hydrate_connection_secrets(
    connection: &mut SavedConnection,
    credential_store: &CredentialStore,
) {
    if connection.protocol != "ssh" {
        connection.password = None;
        connection.private_key = None;
        connection.passphrase = None;
        return;
    }

    if connection.auth_type == "none" {
        connection.password = None;
        connection.private_key = None;
        connection.passphrase = None;
    }

    // 从凭据存储中查找并恢复凭据
    if let Some(stored) = credential_store
        .credentials
        .iter()
        .find(|c| c.connection_id == connection.id)
    {
        connection.password = if connection.auth_type == "password" {
            stored.password.clone()
        } else {
            None
        };
        connection.private_key = if connection.auth_type == "key" {
            stored.private_key.clone()
        } else {
            None
        };
        // Phase 2 stored key passphrases in the password field.
        connection.passphrase = if connection.auth_type == "key" {
            stored.passphrase.clone().or_else(|| stored.password.clone())
        } else {
            None
        };
        for jump in &mut connection.jump_hosts {
            if let Some(secret) = stored.jump_hosts.iter().find(|item| item.id == jump.id) {
                jump.password = secret.password.clone();
                jump.private_key = secret.private_key.clone();
                jump.passphrase = secret.passphrase.clone();
            }
        }
        for (rule, response) in connection
            .auto_login_rules
            .iter_mut()
            .zip(stored.auto_login_responses.iter())
        {
            rule.response = Some(response.clone());
        }
        connection.post_connect_commands = stored.post_connect_commands.clone();
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
                password: None,
                private_key: connection.private_key.clone(),
                passphrase: connection.passphrase.clone(),
                jump_hosts: collect_jump_credentials(connection),
                auto_login_responses: collect_auto_login_responses(connection),
                post_connect_commands: connection.post_connect_commands.clone(),
            });
        }
        "password" => {
            credential_store.credentials.push(StoredCredential {
                connection_id: connection.id.clone(),
                password: connection.password.clone(),
                private_key: None,
                passphrase: None,
                jump_hosts: collect_jump_credentials(connection),
                auto_login_responses: collect_auto_login_responses(connection),
                post_connect_commands: connection.post_connect_commands.clone(),
            });
        }
        _ => {
            credential_store.credentials.push(StoredCredential {
                connection_id: connection.id.clone(),
                password: None,
                private_key: None,
                passphrase: None,
                jump_hosts: collect_jump_credentials(connection),
                auto_login_responses: collect_auto_login_responses(connection),
                post_connect_commands: connection.post_connect_commands.clone(),
            });
            // "none" 认证方式，不存储凭据
        }
    }

    Ok(())
}

fn collect_jump_credentials(connection: &SavedConnection) -> Vec<StoredJumpCredential> {
    connection.jump_hosts.iter().map(|jump| StoredJumpCredential {
        id: jump.id.clone(),
        password: jump.password.clone(),
        private_key: jump.private_key.clone(),
        passphrase: jump.passphrase.clone(),
    }).collect()
}

fn collect_auto_login_responses(connection: &SavedConnection) -> Vec<String> {
    connection.auto_login_rules.iter()
        .map(|rule| rule.response.clone().unwrap_or_default())
        .collect()
}

fn connections_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    Ok(config_dir.join("connections.json"))
}

pub(crate) fn load_connections(app: &AppHandle) -> Result<Vec<SavedConnection>, String> {
    let path = connections_path(app)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let connections: Vec<SavedConnection> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(connections)
}

pub(crate) fn write_connections(app: &AppHandle, connections: &Vec<SavedConnection>) -> Result<(), String> {
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
                crate::warn_log!("[get_connections] credential store unreadable ({}), returning empty credentials", e);
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
            connection.passphrase = None;
            for jump in &mut connection.jump_hosts {
                jump.password = None;
                jump.private_key = None;
                jump.passphrase = None;
            }
            for rule in &mut connection.auto_login_rules {
                rule.response = None;
            }
            connection.post_connect_commands.clear();
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
            crate::warn_log!("[save_connection] credential store unreadable ({}), resetting", e);
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

/// 批量删除。一次性重写元数据与凭据库：前端逐条调用 `delete_connection` 会在
/// 主密码模式下触发 N 次 Argon2id 派生（每次写入都用新盐，绕开 KDF 缓存）。
#[tauri::command]
pub fn delete_connections(
    app: AppHandle,
    ids: Vec<String>,
    master_state: State<'_, MasterPasswordState>,
) -> Result<usize, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let doomed: HashSet<&str> = ids.iter().map(String::as_str).collect();

    let secret = encryption::resolve_secret(&app, &master_state)?;
    let mut credential_store = encryption::load_encrypted_credentials(&app, &secret)
        .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() });
    credential_store
        .credentials
        .retain(|credential| !doomed.contains(credential.connection_id.as_str()));
    encryption::save_encrypted_credentials(&app, &credential_store, &secret)?;

    let mut connections = load_connections(&app)?;
    let before = connections.len();
    connections.retain(|connection| !doomed.contains(connection.id.as_str()));
    let removed = before - connections.len();
    write_connections(&app, &connections)?;
    Ok(removed)
}

/// 批量移动分组。只改元数据、完全不触碰凭据库——因此主密码锁定时同样安全，
/// 而走 `save_connection` 重新保存会用前端持有的（已被剥空的）凭据覆盖密文。
#[tauri::command]
pub fn move_connections(
    app: AppHandle,
    ids: Vec<String>,
    group: Option<String>,
) -> Result<usize, String> {
    let target = group
        .map(|value| normalize_group_path(&value))
        .filter(|value| !value.is_empty());
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();

    let mut connections = load_connections(&app)?;
    let mut moved = 0;
    for connection in &mut connections {
        if !wanted.contains(connection.id.as_str()) {
            continue;
        }
        let current = connection
            .group
            .as_deref()
            .map(normalize_group_path)
            .filter(|value| !value.is_empty());
        if current == target {
            continue;
        }
        connection.group = target.clone();
        moved += 1;
    }
    if moved > 0 {
        write_connections(&app, &connections)?;
    }
    Ok(moved)
}

/// 重命名（或移动）一个分组，子分组随之跟随：`Prod` → `Production` 会把
/// `Prod/EU` 一并改成 `Production/EU`。`to` 为空表示解散该分组——其下的书签
/// 变为未分组，子分组提升一级。返回受影响的书签条数。
#[tauri::command]
pub fn rename_group(app: AppHandle, from: String, to: String) -> Result<usize, String> {
    let source = normalize_group_path(&from);
    if source.is_empty() {
        return Err("Group path is empty".to_string());
    }
    let target = normalize_group_path(&to);
    if source == target {
        return Ok(0);
    }
    let mut connections = load_connections(&app)?;
    let mut renamed = 0;
    for connection in &mut connections {
        let current = connection
            .group
            .as_deref()
            .map(normalize_group_path)
            .unwrap_or_default();
        let Some(next) = renamed_group_path(&current, &source, &target) else {
            continue;
        };
        connection.group = if next.is_empty() { None } else { Some(next) };
        renamed += 1;
    }
    if renamed > 0 {
        write_connections(&app, &connections)?;
    }
    Ok(renamed)
}

/// Where a bookmark currently in `current` lands when group `source` is renamed
/// to `target`. `None` means the bookmark is outside the renamed subtree; an
/// empty target dissolves the group (children are promoted one level).
fn renamed_group_path(current: &str, source: &str, target: &str) -> Option<String> {
    if current == source {
        return Some(target.to_string());
    }
    let rest = current.strip_prefix(&format!("{}/", source))?;
    Some(if target.is_empty() { rest.to_string() } else { format!("{}/{}", target, rest) })
}

/// 复制一条书签，连同它的凭据。返回新书签的 id。
#[tauri::command]
pub fn duplicate_connection(
    app: AppHandle,
    id: String,
    name: Option<String>,
    master_state: State<'_, MasterPasswordState>,
) -> Result<String, String> {
    let mut connections = load_connections(&app)?;
    let source = connections
        .iter()
        .find(|connection| connection.id == id)
        .ok_or_else(|| "Bookmark not found".to_string())?
        .clone();

    let new_id = uuid::Uuid::new_v4().to_string();
    let secret = encryption::resolve_secret(&app, &master_state)?;
    let mut credential_store = encryption::load_encrypted_credentials(&app, &secret)
        .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() });
    if let Some(mut existing) = credential_store
        .credentials
        .iter()
        .find(|credential| credential.connection_id == id)
        .cloned()
    {
        // StoredCredential is ZeroizeOnDrop, so its fields cannot be moved out
        // with struct-update syntax — retag the clone instead.
        existing.connection_id = new_id.clone();
        credential_store.credentials.push(existing);
        encryption::save_encrypted_credentials(&app, &credential_store, &secret)?;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64);
    let mut copy = source.clone();
    copy.id = new_id.clone();
    copy.name = name.unwrap_or_else(|| format!("{} copy", source.name));
    copy.created_at = now;
    copy.last_used = None;
    connections.push(copy);
    write_connections(&app, &connections)?;
    Ok(new_id)
}

/// AuraTerm 自有的书签交换格式（`export_bookmarks` 产出，`import_bookmarks`
/// 以 `format = "auraterm"` 读回）。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkExport {
    format: String,
    version: u32,
    #[serde(default)]
    exported_at: u64,
    #[serde(default)]
    include_secrets: bool,
    connections: Vec<SavedConnection>,
}

const BOOKMARK_EXPORT_FORMAT: &str = "auraterm-bookmarks";
const BOOKMARK_EXPORT_VERSION: u32 = 1;

/// 导出书签为 JSON 字符串。`ids` 为空表示导出全部。
///
/// 默认**不含**凭据：磁盘上的元数据本就不带密码/私钥。`include_secrets` 会把
/// 凭据注水进导出内容，因此要求凭据当前可访问（主密码已解锁）。
#[tauri::command]
pub fn export_bookmarks(
    app: AppHandle,
    ids: Option<Vec<String>>,
    include_secrets: bool,
    master_state: State<'_, MasterPasswordState>,
) -> Result<String, String> {
    let mut connections = load_connections(&app)?;
    if let Some(ids) = ids {
        let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
        connections.retain(|connection| wanted.contains(connection.id.as_str()));
    }

    if include_secrets {
        if !encryption::credentials_accessible(&app, &master_state) {
            return Err("Unlock the master password before exporting credentials".to_string());
        }
        let secret = encryption::resolve_secret(&app, &master_state)?;
        let credential_store = encryption::load_encrypted_credentials(&app, &secret)?;
        for connection in &mut connections {
            hydrate_connection_secrets(connection, &credential_store);
        }
    }

    let exported_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64);
    serde_json::to_string_pretty(&BookmarkExport {
        format: BOOKMARK_EXPORT_FORMAT.to_string(),
        version: BOOKMARK_EXPORT_VERSION,
        exported_at,
        include_secrets,
        connections,
    })
    .map_err(|e| e.to_string())
}

/// 解析 AuraTerm 导出文件。返回待导入的连接（id 一律重新分配，避免覆盖同机
/// 已存在的书签）。
fn parse_auraterm_export(content: &str, base_group: &str) -> Result<(Vec<SavedConnection>, Vec<String>), String> {
    let parsed: BookmarkExport = serde_json::from_str(content)
        .map_err(|e| format!("Not an AuraTerm bookmark export: {}", e))?;
    if parsed.format != BOOKMARK_EXPORT_FORMAT {
        return Err("Not an AuraTerm bookmark export".to_string());
    }
    if parsed.version > BOOKMARK_EXPORT_VERSION {
        return Err(format!(
            "This file was written by a newer AuraTerm (format v{})",
            parsed.version
        ));
    }

    let warnings = Vec::new();
    let entries = parsed
        .connections
        .into_iter()
        .map(|mut connection| {
            connection.id = uuid::Uuid::new_v4().to_string();
            connection.last_used = None;
            let own_group = connection
                .group
                .as_deref()
                .map(normalize_group_path)
                .unwrap_or_default();
            connection.group = match (base_group.is_empty(), own_group.is_empty()) {
                (true, true) => None,
                (true, false) => Some(own_group),
                (false, true) => Some(base_group.to_string()),
                (false, false) => Some(format!("{}/{}", base_group, own_group)),
            };
            connection
        })
        .collect();
    Ok((entries, warnings))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkImportResult {
    imported: usize,
    skipped: usize,
    warnings: Vec<String>,
}

#[tauri::command]
pub fn import_bookmarks(
    app: AppHandle,
    format: String,
    content: String,
    group: Option<String>,
    master_state: State<'_, MasterPasswordState>,
) -> Result<BookmarkImportResult, String> {
    let is_auraterm = format.eq_ignore_ascii_case("auraterm");
    // An AuraTerm export already carries its own folder layout, so an unset
    // target group means "keep it as it was" rather than a synthetic folder.
    let base_group = group
        .map(|value| normalize_group_path(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| if is_auraterm {
            String::new()
        } else {
            format!("Imported/{}", if format.eq_ignore_ascii_case("putty") { "PuTTY" } else { "OpenSSH" })
        });
    let (candidates, mut warnings) = if format.eq_ignore_ascii_case("putty") {
        parse_putty_registry(&content, &base_group)
    } else if format.eq_ignore_ascii_case("openssh") {
        parse_openssh_config(&content, &base_group)
    } else if is_auraterm {
        parse_auraterm_export(&content, &base_group)?
    } else {
        return Err("Unsupported bookmark import format".to_string());
    };

    // Credentials only travel in AuraTerm exports, and only when the store can
    // be written; otherwise the bookmarks still import, minus their secrets.
    let secret = if is_auraterm && candidates.iter().any(connection_carries_secrets) {
        if encryption::credentials_accessible(&app, &master_state) {
            Some(encryption::resolve_secret(&app, &master_state)?)
        } else {
            warnings.push("Credentials in this file were skipped: unlock the master password first".to_string());
            None
        }
    } else {
        None
    };
    let mut credential_store = match &secret {
        Some(secret) => Some(
            encryption::load_encrypted_credentials(&app, secret)
                .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() }),
        ),
        None => None,
    };

    let mut connections = load_connections(&app)?;
    let mut imported = 0;
    let mut skipped = 0;
    for mut candidate in candidates {
        if let Some(store) = credential_store.as_mut() {
            persist_connection_secrets(&candidate, store)?;
        }
        sanitize_connection_for_storage(&mut candidate);
        let duplicate = connections.iter().any(|existing| {
            existing.protocol == candidate.protocol
                && existing.host.eq_ignore_ascii_case(&candidate.host)
                && existing.port == candidate.port
                && existing.user == candidate.user
                && existing.name.eq_ignore_ascii_case(&candidate.name)
        });
        if duplicate {
            skipped += 1;
        } else {
            connections.push(candidate);
            imported += 1;
        }
    }
    if imported == 0 && skipped == 0 {
        warnings.push("No concrete hosts were found in the selected file".to_string());
    }
    if let (Some(secret), Some(store)) = (&secret, &credential_store) {
        encryption::save_encrypted_credentials(&app, store, secret)?;
    }
    write_connections(&app, &connections)?;
    Ok(BookmarkImportResult { imported, skipped, warnings })
}

/// Whether an imported entry brought any secret along that is worth persisting.
fn connection_carries_secrets(connection: &SavedConnection) -> bool {
    connection.password.is_some()
        || connection.private_key.is_some()
        || connection.passphrase.is_some()
        || !connection.post_connect_commands.is_empty()
        || connection.jump_hosts.iter().any(|jump| {
            jump.password.is_some() || jump.private_key.is_some() || jump.passphrase.is_some()
        })
        || connection.auto_login_rules.iter().any(|rule| rule.response.is_some())
}

fn normalize_group_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn imported_connection(name: String, group: String, protocol: &str, host: String, port: u16, user: String) -> SavedConnection {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64);
    SavedConnection {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        group: Some(group),
        log_path: None,
        protocol: protocol.to_string(),
        host,
        port,
        user,
        auth_type: if protocol == "ssh" { "agent".to_string() } else { "none".to_string() },
        password: None,
        private_key: None,
        passphrase: None,
        agent_forwarding: false,
        jump_hosts: Vec::new(),
        auto_login_rules: Vec::new(),
        post_connect_commands: Vec::new(),
        port_name: None,
        serial_transport: None,
        adopt_server_params: None,
        serial_auto_reconnect: None,
        baud_rate: None,
        data_bits: None,
        stop_bits: None,
        parity: None,
        flow_control: None,
        created_at: now,
        last_used: None,
        auto_reconnect: false,
        reconnect_type: default_reconnect_type(),
        tunnels: Vec::new(),
    }
}

fn parse_openssh_config(content: &str, base_group: &str) -> (Vec<SavedConnection>, Vec<String>) {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut global = HashMap::<String, String>::new();
    let mut aliases = Vec::<String>::new();
    let mut options = HashMap::<String, String>::new();

    let flush = |aliases: &mut Vec<String>, options: &mut HashMap<String, String>, entries: &mut Vec<SavedConnection>, warnings: &mut Vec<String>| {
        for alias in aliases.drain(..) {
            if alias.contains('*') || alias.contains('?') || alias.starts_with('!') {
                warnings.push(format!("Skipped wildcard Host entry: {alias}"));
                continue;
            }
            let host = options.get("hostname").cloned().unwrap_or_else(|| alias.clone());
            let port = options.get("port").and_then(|value| value.parse::<u16>().ok()).unwrap_or(22);
            let user = options.get("user").cloned().unwrap_or_default();
            let normalized_alias = normalize_group_path(&alias);
            let (name, group) = match normalized_alias.rsplit_once('/') {
                Some((folder, name)) => (name.to_string(), format!("{base_group}/{folder}")),
                None => (alias.clone(), base_group.to_string()),
            };
            let mut connection = imported_connection(name, group, "ssh", host, port, user);
            connection.agent_forwarding = options.get("forwardagent").is_some_and(|value| value.eq_ignore_ascii_case("yes"));
            if let Some(proxy) = options.get("proxyjump").and_then(|value| value.split(',').next()) {
                if let Some(jump) = parse_jump_host(proxy, &connection.user) {
                    connection.jump_hosts.push(jump);
                }
            }
            entries.push(connection);
        }
        options.clear();
    };

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = split_config_line(line) else { continue };
        let key = key.to_ascii_lowercase();
        if key == "host" {
            flush(&mut aliases, &mut options, &mut entries, &mut warnings);
            aliases = value.split_whitespace().map(str::to_string).collect();
            options = global.clone();
        } else if aliases.is_empty() {
            global.entry(key).or_insert(value.to_string());
        } else {
            options.entry(key).or_insert(value.to_string());
        }
    }
    flush(&mut aliases, &mut options, &mut entries, &mut warnings);
    (entries, warnings)
}

fn split_config_line(line: &str) -> Option<(&str, &str)> {
    if let Some((key, value)) = line.split_once('=') {
        return Some((key.trim(), value.trim().trim_matches('"')));
    }
    let index = line.find(char::is_whitespace)?;
    Some((line[..index].trim(), line[index..].trim().trim_matches('"')))
}

fn parse_jump_host(value: &str, default_user: &str) -> Option<SavedJumpHost> {
    if value.eq_ignore_ascii_case("none") || value.contains('%') {
        return None;
    }
    let (user, address) = value.rsplit_once('@').map_or((default_user, value), |(user, address)| (user, address));
    let (host, port) = address.rsplit_once(':')
        .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
        .unwrap_or((address, 22));
    (!host.trim().is_empty()).then(|| SavedJumpHost {
        id: uuid::Uuid::new_v4().to_string(),
        host: host.to_string(),
        port,
        user: user.to_string(),
        auth_type: "agent".to_string(),
        password: None,
        private_key: None,
        passphrase: None,
    })
}

fn parse_putty_registry(content: &str, base_group: &str) -> (Vec<SavedConnection>, Vec<String>) {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut session_name: Option<String> = None;
    let mut values = HashMap::<String, String>::new();

    let flush = |name: &mut Option<String>, values: &mut HashMap<String, String>, entries: &mut Vec<SavedConnection>| {
        let Some(raw_name) = name.take() else { return };
        let host = values.get("HostName").cloned().unwrap_or_default();
        if host.is_empty() || raw_name == "Default Settings" {
            values.clear();
            return;
        }
        let protocol = values.get("Protocol").map(String::as_str).unwrap_or("ssh");
        if protocol != "ssh" && protocol != "telnet" {
            values.clear();
            return;
        }
        let port = values.get("PortNumber")
            .and_then(|value| u16::from_str_radix(value.trim_start_matches("dword:"), 16).ok())
            .unwrap_or(if protocol == "telnet" { 23 } else { 22 });
        let decoded_name = decode_putty_name(&raw_name);
        let normalized = normalize_group_path(&decoded_name);
        let (display_name, group) = normalized.rsplit_once('/')
            .map(|(folder, name)| (name.to_string(), format!("{base_group}/{folder}")))
            .unwrap_or((decoded_name, base_group.to_string()));
        entries.push(imported_connection(
            display_name,
            group,
            protocol,
            host,
            port,
            values.get("UserName").cloned().unwrap_or_default(),
        ));
        values.clear();
    };

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            flush(&mut session_name, &mut values, &mut entries);
            let marker = "\\PuTTY\\Sessions\\";
            session_name = line.find(marker).map(|index| line[index + marker.len()..line.len() - 1].to_string());
            continue;
        }
        if session_name.is_none() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim_matches('"').to_string();
            let value = value.trim().trim_matches('"').replace("\\\\", "\\").replace("\\\"", "\"");
            values.insert(key, value);
        }
    }
    flush(&mut session_name, &mut values, &mut entries);
    if content.contains("PublicKeyFile") {
        warnings.push("PuTTY private-key paths were not imported; PPK conversion is required before use".to_string());
    }
    (entries, warnings)
}

fn decode_putty_name(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    output.push(byte);
                    index += 3;
                    continue;
                }
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

#[cfg(test)]
mod import_tests {
    use super::{parse_openssh_config, parse_putty_registry};

    #[test]
    fn imports_concrete_openssh_hosts_and_nested_aliases() {
        let config = r#"
            User global-user
            Host prod/web
              HostName web.internal
              Port 2222
              ProxyJump ops@bastion:2200
              ForwardAgent yes
            Host *.wildcard
              HostName ignored
        "#;
        let (entries, warnings) = parse_openssh_config(config, "Imported/OpenSSH");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "web");
        assert_eq!(entries[0].group.as_deref(), Some("Imported/OpenSSH/prod"));
        assert_eq!(entries[0].host, "web.internal");
        assert_eq!(entries[0].port, 2222);
        assert_eq!(entries[0].jump_hosts[0].host, "bastion");
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn imports_putty_registry_sessions() {
        let registry = r#"
            Windows Registry Editor Version 5.00
            [HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\Prod%20Router]
            "HostName"="10.0.0.1"
            "PortNumber"=dword:00000016
            "UserName"="admin"
            "Protocol"="ssh"
        "#;
        let (entries, warnings) = parse_putty_registry(registry, "Imported/PuTTY");
        assert!(warnings.is_empty());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Prod Router");
        assert_eq!(entries[0].port, 22);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A strict serde struct silently drops fields it does not declare, so a
    /// network serial bookmark would come back as a local port aimed at a
    /// label. Lock the round-trip down.
    #[test]
    fn network_serial_bookmark_survives_a_save_and_load() {
        let json = serde_json::json!({
            "id": "b1",
            "name": "Lab console",
            "protocol": "serial",
            "host": "10.0.0.5",
            "port": 2217,
            "portName": "10.0.0.5:2217",
            "serialTransport": "rfc2217",
            "adoptServerParams": true,
            "serialAutoReconnect": true,
            "baudRate": 115200,
            "dataBits": 8,
            "stopBits": 1,
            "parity": "none",
            "flowControl": "none",
            "createdAt": 1_700_000_000_u64,
        });

        let parsed: SavedConnection = serde_json::from_value(json).expect("parse");
        let reloaded: SavedConnection =
            serde_json::from_str(&serde_json::to_string(&parsed).expect("encode")).expect("decode");

        assert_eq!(reloaded.serial_transport.as_deref(), Some("rfc2217"));
        assert_eq!(reloaded.adopt_server_params, Some(true));
        assert_eq!(reloaded.serial_auto_reconnect, Some(true));
        assert_eq!(reloaded.host, "10.0.0.5");
        assert_eq!(reloaded.port, 2217);
    }

    /// Bookmarks written before network serial existed must still load.
    #[test]
    fn local_serial_bookmark_without_a_transport_still_loads() {
        let json = serde_json::json!({
            "id": "b2",
            "name": "USB console",
            "protocol": "serial",
            "portName": "/dev/ttyUSB0",
            "baudRate": 9600,
            "createdAt": 1_700_000_000_u64,
        });
        let parsed: SavedConnection = serde_json::from_value(json).expect("parse");
        assert_eq!(parsed.serial_transport, None);
        assert_eq!(parsed.port_name.as_deref(), Some("/dev/ttyUSB0"));
    }

    #[test]
    fn rename_carries_subfolders_and_leaves_others_alone() {
        assert_eq!(renamed_group_path("Prod", "Prod", "Production"), Some("Production".to_string()));
        assert_eq!(renamed_group_path("Prod/EU", "Prod", "Production"), Some("Production/EU".to_string()));
        assert_eq!(renamed_group_path("Prod/EU/Web", "Prod/EU", "Prod/APAC"), Some("Prod/APAC/Web".to_string()));
        assert_eq!(renamed_group_path("Production", "Prod", "Production"), None);
        assert_eq!(renamed_group_path("", "Prod", "Production"), None);
    }

    #[test]
    fn empty_target_dissolves_the_group() {
        assert_eq!(renamed_group_path("Lab", "Lab", ""), Some(String::new()));
        assert_eq!(renamed_group_path("Lab/Bench", "Lab", ""), Some("Bench".to_string()));
    }

    #[test]
    fn export_round_trips_through_the_importer() {
        let export = BookmarkExport {
            format: BOOKMARK_EXPORT_FORMAT.to_string(),
            version: BOOKMARK_EXPORT_VERSION,
            exported_at: 1,
            include_secrets: false,
            connections: vec![imported_connection(
                "web".to_string(),
                "Production/EU".to_string(),
                "ssh",
                "10.0.0.1".to_string(),
                22,
                "ops".to_string(),
            )],
        };
        let json = serde_json::to_string(&export).expect("serialize");

        let (entries, _) = parse_auraterm_export(&json, "").expect("parse");
        assert_eq!(entries[0].group.as_deref(), Some("Production/EU"));
        assert_ne!(entries[0].id, export.connections[0].id, "ids are reassigned on import");

        let (nested, _) = parse_auraterm_export(&json, "Restored").expect("parse");
        assert_eq!(nested[0].group.as_deref(), Some("Restored/Production/EU"));
    }

    #[test]
    fn foreign_files_are_rejected() {
        assert!(parse_auraterm_export("{\"format\":\"other\",\"version\":1,\"connections\":[]}", "").is_err());
        assert!(parse_auraterm_export("not json", "").is_err());
    }
}
