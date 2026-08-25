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
    /// Set on bookmarks that arrived in a share bundle; see [`BookmarkOrigin`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<BookmarkOrigin>,
}

/// Where a bookmark came from when it was not created here.
///
/// `entry_id` is the *sharer's* connection id and stays the same across
/// re-exports of that bookmark, so importing an updated share can recognise
/// entries it already holds. The local `id` cannot serve that purpose: it is
/// reassigned on every import (see [`parse_auraterm_export`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkOrigin {
    /// The share bundle this entry was packed into.
    pub bundle_id: String,
    /// Stable per-bookmark identity within that bundle.
    pub entry_id: String,
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

/// Strip everything that is meaningless, misleading or unsafe once a bookmark
/// leaves this machine, and stamp it with its share identity.
///
/// This is deliberately *not* what [`export_bookmarks`] does: a backup of your
/// own bookmarks should keep its log paths and (opt-in) its credentials. A
/// share is the other case — the recipient gets the knowledge of *how to reach*
/// these hosts, never the identity used to reach them, and never a local path
/// off the sharer's disk.
fn sanitize_connection_for_sharing(connection: &mut SavedConnection, bundle_id: &str) {
    connection.origin = Some(BookmarkOrigin {
        bundle_id: bundle_id.to_string(),
        entry_id: connection.id.clone(),
    });
    // The importer reassigns ids anyway; carrying ours across adds nothing.
    connection.id = String::new();
    // A path on our disk, naming our user and our directory layout.
    connection.log_path = None;
    // Our habits, not theirs.
    connection.last_used = None;
    // Passwords, private keys, auto-login responses and post-connect commands.
    sanitize_connection_for_storage(connection);
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
    /// Present on group shares, absent on plain backups. Optional on purpose:
    /// the format version stays at 1 so clients that predate sharing can still
    /// read the file (serde drops fields it does not know).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share: Option<ShareMeta>,
    connections: Vec<SavedConnection>,
}

/// Describes a shared group: what it was called, who packed it, and the
/// subfolders that would otherwise vanish because they hold no bookmark.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMeta {
    /// Always `"group"` today; leaves room for other share units later.
    #[serde(default = "default_share_kind")]
    pub kind: String,
    /// The shared group's own path on the sharer's machine (`Prod/EU`). Used as
    /// the default landing group, never forced on the importer.
    pub root_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub bundle_id: String,
    /// Bookmark-free subfolders, relative to the shared root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub empty_groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
}

fn default_share_kind() -> String {
    "group".to_string()
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

    serde_json::to_string_pretty(&BookmarkExport {
        format: BOOKMARK_EXPORT_FORMAT.to_string(),
        version: BOOKMARK_EXPORT_VERSION,
        exported_at: now_millis(),
        include_secrets,
        share: None,
        connections,
    })
    .map_err(|e| e.to_string())
}

/// 把一个分组子树打包成**分享包**。
///
/// 与 [`export_bookmarks`] 的三点不同：
/// 1. 分组路径按根**相对化**（`Prod/EU/Web` → `Web`），接收端因此可以把它挂到
///    任意位置，也看不到分享者根以上的目录结构；
/// 2. 空子分组随包出行（见 [`shared_empty_groups`]），否则接收端会丢掉分享者
///    刻意建出来的层级；
/// 3. 每条都过一遍 [`sanitize_connection_for_sharing`]：凭据、日志路径、使用
///    时间一律不外带。分享包**永远不含凭据**，所以没有 `include_secrets` 开关。
///
/// `explicit_groups` 是前端的 `settings.bookmarkGroups`（用户显式创建过的分组，
/// 可能一条书签都没有）。
#[tauri::command]
pub fn export_group_bookmarks(
    app: AppHandle,
    root: String,
    explicit_groups: Vec<String>,
    label: Option<String>,
    note: Option<String>,
) -> Result<String, String> {
    build_share_bundle(&app, root, explicit_groups, label, note)
}

/// The bundle itself, without the Tauri command wrapper — `bookmark_share.rs`
/// encrypts this same JSON instead of writing it to a file.
pub(crate) fn build_share_bundle(
    app: &AppHandle,
    root: String,
    explicit_groups: Vec<String>,
    label: Option<String>,
    note: Option<String>,
) -> Result<String, String> {
    let root = normalize_group_path(&root);
    if root.is_empty() {
        return Err("Pick a group to share".to_string());
    }

    let bundle_id = uuid::Uuid::new_v4().to_string();
    let connections: Vec<SavedConnection> = load_connections(app)?
        .into_iter()
        .filter_map(|mut connection| {
            // `None` = outside the shared subtree, so this also does the filtering.
            let relative =
                relative_group_path(connection.group.as_deref().unwrap_or_default(), &root)?;
            connection.group = (!relative.is_empty()).then_some(relative);
            sanitize_connection_for_sharing(&mut connection, &bundle_id);
            Some(connection)
        })
        .collect();

    let empty_groups = shared_empty_groups(&explicit_groups, &root, &connections);
    if connections.is_empty() && empty_groups.is_empty() {
        return Err(format!("Group \"{}\" holds no bookmarks", root));
    }

    let trimmed = |value: Option<String>| {
        value.and_then(|value| {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        })
    };
    serde_json::to_string_pretty(&BookmarkExport {
        format: BOOKMARK_EXPORT_FORMAT.to_string(),
        version: BOOKMARK_EXPORT_VERSION,
        exported_at: now_millis(),
        include_secrets: false,
        share: Some(ShareMeta {
            kind: default_share_kind(),
            root_name: root,
            label: trimmed(label),
            note: trimmed(note),
            bundle_id,
            empty_groups,
            source_label: None,
        }),
        connections,
    })
    .map_err(|e| e.to_string())
}

/// A bookmark's group path relative to the shared root: `Prod/EU/Web` under
/// `Prod/EU` becomes `Web`, the root itself becomes `""`, and anything outside
/// the subtree yields `None` — the same subtree arithmetic a group rename does,
/// with the root as the source and an empty target.
fn relative_group_path(path: &str, root: &str) -> Option<String> {
    renamed_group_path(&normalize_group_path(path), root, "")
}

/// The subfolders a share has to spell out because no bookmark in it rebuilds
/// them. Paths come in absolute and go out relative to `root`; `packed` must
/// already carry relative groups.
fn shared_empty_groups(
    explicit_groups: &[String],
    root: &str,
    packed: &[SavedConnection],
) -> Vec<String> {
    let mut groups: Vec<String> = explicit_groups
        .iter()
        .filter_map(|group| relative_group_path(group, root))
        .filter(|group| !group.is_empty())
        .filter(|group| {
            let prefix = format!("{}/", group);
            !packed.iter().any(|connection| {
                let current = connection.group.as_deref().unwrap_or_default();
                current == group || current.starts_with(&prefix)
            })
        })
        .collect();
    groups.sort();
    groups.dedup();
    groups
}

/// 一次导入的解析结果。
struct ParsedImport {
    entries: Vec<SavedConnection>,
    warnings: Vec<String>,
    /// 需要显式建出来的空分组（绝对路径）。
    groups: Vec<String>,
    /// 分享包元数据；普通备份与外部格式为 None。
    share: Option<ShareMeta>,
}

/// 解析 AuraTerm 导出文件。id 一律重新分配，避免覆盖同机已存在的书签；`origin`
/// 原样保留，它才是跨机器稳定的身份。
fn parse_auraterm_export(content: &str, base_group: &str) -> Result<ParsedImport, String> {
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

    // 分享包里的路径是相对于「被分享的那个分组」的，所以在用户没有指定落点时，
    // 它落在该分组自己的名字下——与普通备份「保持文件中原有的分组」是同一个意思，
    // 只是备份里的路径本来就是绝对的。
    let share = parsed.share;
    let root = share
        .as_ref()
        .map(|share| normalize_group_path(&share.root_name))
        .unwrap_or_default();
    let base_group = if base_group.is_empty() { root.as_str() } else { base_group };

    let groups = share
        .as_ref()
        .map(|share| {
            share
                .empty_groups
                .iter()
                .filter_map(|group| {
                    let group = normalize_group_path(group);
                    if group.is_empty() {
                        return None;
                    }
                    Some(if base_group.is_empty() {
                        group
                    } else {
                        format!("{}/{}", base_group, group)
                    })
                })
                .collect()
        })
        .unwrap_or_default();

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
    Ok(ParsedImport { entries, warnings, groups, share })
}

/// 落点分组：用户指定的优先；留空时 AuraTerm 包沿用包内分组（分享包即它自己的
/// 名字，见 [`parse_auraterm_export`]），其它格式落到 `Imported/<格式>`。
fn import_base_group(format: &str, group: Option<&str>) -> String {
    if let Some(explicit) = group
        .map(normalize_group_path)
        .filter(|value| !value.is_empty())
    {
        return explicit;
    }
    if format.eq_ignore_ascii_case("auraterm") {
        String::new()
    } else {
        format!(
            "Imported/{}",
            if format.eq_ignore_ascii_case("putty") { "PuTTY" } else { "OpenSSH" }
        )
    }
}

fn parse_import(format: &str, content: &str, group: Option<&str>) -> Result<ParsedImport, String> {
    let base_group = import_base_group(format, group);
    if format.eq_ignore_ascii_case("putty") {
        let (entries, warnings) = parse_putty_registry(content, &base_group);
        Ok(ParsedImport { entries, warnings, groups: Vec::new(), share: None })
    } else if format.eq_ignore_ascii_case("openssh") {
        let (entries, warnings) = parse_openssh_config(content, &base_group);
        Ok(ParsedImport { entries, warnings, groups: Vec::new(), share: None })
    } else if format.eq_ignore_ascii_case("auraterm") {
        parse_auraterm_export(content, &base_group)
    } else {
        Err("Unsupported bookmark import format".to_string())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkImportResult {
    imported: usize,
    /// 命中本地已有条目并被覆盖的数量（拓扑更新，凭据与使用记录保留）。
    updated: usize,
    skipped: usize,
    warnings: Vec<String>,
    /// Bookmark-free subfolders the share asked for, as absolute paths. The
    /// frontend merges them into `settings.bookmarkGroups`, which is where
    /// groups that hold nothing yet are kept alive (they cannot be derived
    /// from `connections.json`).
    created_groups: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// 导入：预览 → 决策 → 落盘
//
// 早先 `import_bookmarks` 把「解析 / 去重 / 写盘」焊在一次调用里：用户点完文件，
// 下一秒东西已经在库里了，只回一个计数。分享场景下导入的是**外部输入**，落盘前
// 必须能看清楚里面有什么、和本地哪些条目冲突、有没有会自动执行的字段。
// ─────────────────────────────────────────────────────────────────────────────

pub const ACTION_ADD: &str = "add";
pub const ACTION_UPDATE: &str = "update";
pub const ACTION_SKIP: &str = "skip";

/// 后端缓存的待落盘导入。原文留在 Rust 侧，前端只拿展示用摘要——可能含凭据的
/// 载荷不必在 JS 与 Rust 之间来回搬。
struct CachedImport {
    format: String,
    content: String,
    created_at: u64,
}

/// 同时保留的预览份数。一个用户一次只看一份，多留几份只是为了容忍「开着预览
/// 又去点了别的文件」。
const MAX_CACHED_IMPORTS: usize = 4;

#[derive(Default)]
pub struct ImportPlanState(std::sync::Mutex<HashMap<String, CachedImport>>);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlanEntry {
    /// 在解析结果中的下标；前端原样回传，作为决策的键。
    index: usize,
    name: String,
    /// 落盘后的分组（已含落点前缀）；None = 未分组。
    group: Option<String>,
    protocol: String,
    /// 展示用目标串：`user@host:port`，串口则是设备与波特率。
    target: String,
    /// [`ACTION_ADD`] / [`ACTION_UPDATE`] / [`ACTION_SKIP`] 之一。
    disposition: String,
    /// 命中的本地书签名（update / skip 时有值）。
    matched_name: Option<String>,
    /// 命中依据：`"origin"`（同一分享包的同一条）或 `"endpoint"`（同一台机器）。
    matched_by: Option<String>,
}

/// 外部载荷里会**自动执行或自动发送**的东西。数的是条目数，不是字段数——
/// 提示要回答的是「这份文件里有多少条书签会替我干事」。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRisks {
    post_connect_commands: usize,
    auto_login_responses: usize,
    jump_host_credentials: usize,
    passwords: usize,
    private_keys: usize,
}

impl ImportRisks {
    fn survey(entries: &[SavedConnection]) -> Self {
        let mut risks = Self::default();
        for entry in entries {
            if !entry.post_connect_commands.is_empty() {
                risks.post_connect_commands += 1;
            }
            if entry.auto_login_rules.iter().any(|rule| rule.response.is_some()) {
                risks.auto_login_responses += 1;
            }
            if entry.jump_hosts.iter().any(|jump| {
                jump.password.is_some() || jump.private_key.is_some() || jump.passphrase.is_some()
            }) {
                risks.jump_host_credentials += 1;
            }
            if entry.password.is_some() {
                risks.passwords += 1;
            }
            if entry.private_key.is_some() {
                risks.private_keys += 1;
            }
        }
        risks
    }
}

/// 用户对外部载荷的信任决定。两项都默认关闭：剥离是默认动作，保留要显式勾。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTrust {
    /// 保留登录后命令与自动登录响应——前者连上就执行，后者按提示自动发送
    /// （而「响应」通常就是密码）。
    allow_commands: bool,
    /// 保留文件里带的密码、私钥与跳板机凭据。
    allow_credentials: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDecision {
    index: usize,
    action: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    plan_id: String,
    format: String,
    /// 生效的落点分组（空串 = 沿用包内分组）。
    group: String,
    share: Option<ShareMeta>,
    entries: Vec<ImportPlanEntry>,
    empty_groups: Vec<String>,
    risks: ImportRisks,
    warnings: Vec<String>,
}

/// 同一台机器、同一个登录身份。串口没有 host/user，比的是设备路径。
fn same_endpoint(left: &SavedConnection, right: &SavedConnection) -> bool {
    if left.protocol != right.protocol {
        return false;
    }
    if left.protocol == "serial" {
        return left.port_name.is_some() && left.port_name == right.port_name;
    }
    left.host.eq_ignore_ascii_case(&right.host) && left.port == right.port && left.user == right.user
}

/// 待导入条目与本地已有书签的关系，按优先级回落。返回 (本地下标, 依据)。
///
/// 一级用 `origin.entry_id`：它是分享者的连接 id，同一条书签多次导出保持不变，
/// 因此「同一分享包的同一条」能被认出来并更新，而不是每次导入翻一倍。本机 `id`
/// 做不到这件事——它在导入时一律重发。
fn match_existing(
    candidate: &SavedConnection,
    existing: &[SavedConnection],
) -> Option<(usize, &'static str)> {
    if let Some(origin) = &candidate.origin {
        if let Some(index) = existing.iter().position(|item| {
            item.origin
                .as_ref()
                .is_some_and(|other| other.entry_id == origin.entry_id)
        }) {
            return Some((index, "origin"));
        }
    }
    existing
        .iter()
        .position(|item| same_endpoint(item, candidate))
        .map(|index| (index, "endpoint"))
}

/// 默认处置：认得出的更新，撞机器的跳过，其余新增。
fn default_action(matched: Option<(usize, &'static str)>) -> &'static str {
    match matched {
        Some((_, "origin")) => ACTION_UPDATE,
        Some(_) => ACTION_SKIP,
        None => ACTION_ADD,
    }
}

/// 展示用目标串，与前端 `connectionTarget()` 同义。
fn display_target(connection: &SavedConnection) -> String {
    match connection.protocol.as_str() {
        "serial" | "rfc2217" | "raw-tcp" => format!(
            "{} @ {}",
            connection.port_name.as_deref().unwrap_or("serial"),
            connection.baud_rate.unwrap_or(9600)
        ),
        "telnet" => format!("{}:{}", connection.host, connection.port),
        _ => format!("{}@{}:{}", connection.user, connection.host, connection.port),
    }
}

/// 默认剥掉外部载荷里会自动执行或自动发送的一切。
fn apply_trust(connection: &mut SavedConnection, trust: &ImportTrust) {
    if !trust.allow_commands {
        connection.post_connect_commands.clear();
        for rule in &mut connection.auto_login_rules {
            rule.response = None;
        }
    }
    if !trust.allow_credentials {
        connection.password = None;
        connection.private_key = None;
        connection.passphrase = None;
        for jump in &mut connection.jump_hosts {
            jump.password = None;
            jump.private_key = None;
            jump.passphrase = None;
        }
    }
}

/// 同一分组里重名时补 `(2)`、`(3)`……，免得列表里出现两条一模一样的名字。
fn unique_name(name: &str, group: Option<&str>, connections: &[SavedConnection]) -> String {
    let taken = |candidate: &str| {
        connections.iter().any(|item| {
            item.name.eq_ignore_ascii_case(candidate)
                && item.group.as_deref().unwrap_or_default() == group.unwrap_or_default()
        })
    };
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|suffix| format!("{} ({})", name, suffix))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_string())
}

fn build_plan(app: &AppHandle, plan_id: String, format: &str, content: &str, group: Option<&str>) -> Result<ImportPlan, String> {
    let parsed = parse_import(format, content, group)?;
    let existing = load_connections(app)?;
    let entries = parsed
        .entries
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let matched = match_existing(candidate, &existing);
            ImportPlanEntry {
                index,
                name: candidate.name.clone(),
                group: candidate.group.clone(),
                protocol: candidate.protocol.clone(),
                target: display_target(candidate),
                disposition: default_action(matched).to_string(),
                matched_name: matched.map(|(position, _)| existing[position].name.clone()),
                matched_by: matched.map(|(_, reason)| reason.to_string()),
            }
        })
        .collect();
    Ok(ImportPlan {
        plan_id,
        format: format.to_string(),
        group: import_base_group(format, group),
        share: parsed.share,
        entries,
        empty_groups: parsed.groups,
        risks: ImportRisks::survey(&parsed.entries),
        warnings: parsed.warnings,
    })
}

/// 解析一份待导入的载荷并算出落盘计划——**不写任何东西**。
#[tauri::command]
pub fn preview_bookmark_import(
    app: AppHandle,
    format: String,
    content: String,
    group: Option<String>,
    plans: State<'_, ImportPlanState>,
) -> Result<ImportPlan, String> {
    let plan_id = uuid::Uuid::new_v4().to_string();
    let plan = build_plan(&app, plan_id.clone(), &format, &content, group.as_deref())?;

    let mut cache = plans.0.lock().map_err(|_| "Import plan cache is poisoned".to_string())?;
    cache.insert(plan_id, CachedImport { format, content, created_at: now_millis() });
    while cache.len() > MAX_CACHED_IMPORTS {
        let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, cached)| cached.created_at)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        cache.remove(&oldest);
    }
    Ok(plan)
}

/// 换一个落点分组重算计划。载荷已经在后端，不必让前端再发一遍。
#[tauri::command]
pub fn retarget_bookmark_import(
    app: AppHandle,
    plan_id: String,
    group: Option<String>,
    plans: State<'_, ImportPlanState>,
) -> Result<ImportPlan, String> {
    let (format, content) = {
        let cache = plans.0.lock().map_err(|_| "Import plan cache is poisoned".to_string())?;
        let cached = cache.get(&plan_id).ok_or("This import preview has expired; pick the file again")?;
        (cached.format.clone(), cached.content.clone())
    };
    build_plan(&app, plan_id, &format, &content, group.as_deref())
}

/// 丢弃一份预览（用户取消）。缓存里躺着的可能是带凭据的明文，别留着。
#[tauri::command]
pub fn discard_bookmark_import(plan_id: String, plans: State<'_, ImportPlanState>) -> Result<(), String> {
    let mut cache = plans.0.lock().map_err(|_| "Import plan cache is poisoned".to_string())?;
    cache.remove(&plan_id);
    Ok(())
}

/// 按用户的逐条决策落盘。
#[tauri::command]
pub fn apply_bookmark_import(
    app: AppHandle,
    plan_id: String,
    group: Option<String>,
    decisions: Vec<ImportDecision>,
    trust: ImportTrust,
    master_state: State<'_, MasterPasswordState>,
    plans: State<'_, ImportPlanState>,
) -> Result<BookmarkImportResult, String> {
    let (format, content) = {
        let cache = plans.0.lock().map_err(|_| "Import plan cache is poisoned".to_string())?;
        let cached = cache.get(&plan_id).ok_or("This import preview has expired; pick the file again")?;
        (cached.format.clone(), cached.content.clone())
    };
    let parsed = parse_import(&format, &content, group.as_deref())?;
    let decisions: HashMap<usize, String> = decisions
        .into_iter()
        .map(|decision| (decision.index, decision.action))
        .collect();
    let result = apply_import(&app, &master_state, parsed, &decisions, trust)?;

    if let Ok(mut cache) = plans.0.lock() {
        cache.remove(&plan_id);
    }
    Ok(result)
}

fn apply_import(
    app: &AppHandle,
    master_state: &State<'_, MasterPasswordState>,
    parsed: ParsedImport,
    decisions: &HashMap<usize, String>,
    trust: ImportTrust,
) -> Result<BookmarkImportResult, String> {
    let ParsedImport { mut entries, mut warnings, groups, .. } = parsed;
    for candidate in &mut entries {
        apply_trust(candidate, &trust);
    }

    // 凭据只在 AuraTerm 包里出现，且需要凭据库可写；否则书签照常导入，只是不带凭据。
    let secret = if entries.iter().any(connection_carries_secrets) {
        if encryption::credentials_accessible(app, master_state) {
            Some(encryption::resolve_secret(app, master_state)?)
        } else {
            warnings.push("Credentials in this file were skipped: unlock the master password first".to_string());
            None
        }
    } else {
        None
    };
    let mut credential_store = match &secret {
        Some(secret) => Some(
            encryption::load_encrypted_credentials(app, secret)
                .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() }),
        ),
        None => None,
    };

    let mut connections = load_connections(app)?;
    let mut imported = 0;
    let mut updated = 0;
    let mut skipped = 0;
    for (index, mut candidate) in entries.into_iter().enumerate() {
        let matched = match_existing(&candidate, &connections);
        let action = decisions
            .get(&index)
            .map(String::as_str)
            .unwrap_or_else(|| default_action(matched));

        // 「更新」到一半发现本地那条已经没了（另一个窗口删掉了？）——退化为新增，
        // 比静默丢弃诚实。
        let position = match (action, matched) {
            (ACTION_SKIP, _) => {
                skipped += 1;
                continue;
            }
            (ACTION_UPDATE, Some((position, _))) => Some(position),
            _ => None,
        };

        // 只有载荷真的带了凭据才动凭据库：更新一条不带凭据的书签**不能**清掉本地
        // 已存的密码（凭据库以连接 id 为键，而更新沿用本地 id）。
        let carries_secrets = connection_carries_secrets(&candidate);

        match position {
            Some(position) => {
                let existing = &connections[position];
                candidate.id = existing.id.clone();
                candidate.created_at = existing.created_at;
                candidate.last_used = existing.last_used;
                if carries_secrets {
                    if let Some(store) = credential_store.as_mut() {
                        persist_connection_secrets(&candidate, store)?;
                    }
                }
                sanitize_connection_for_storage(&mut candidate);
                connections[position] = candidate;
                updated += 1;
            }
            None => {
                candidate.name = unique_name(&candidate.name, candidate.group.as_deref(), &connections);
                if carries_secrets {
                    if let Some(store) = credential_store.as_mut() {
                        persist_connection_secrets(&candidate, store)?;
                    }
                }
                sanitize_connection_for_storage(&mut candidate);
                connections.push(candidate);
                imported += 1;
            }
        }
    }
    if imported == 0 && updated == 0 && skipped == 0 {
        warnings.push("No concrete hosts were found in the selected file".to_string());
    }
    if let (Some(secret), Some(store)) = (&secret, &credential_store) {
        encryption::save_encrypted_credentials(app, store, secret)?;
    }
    write_connections(app, &connections)?;
    Ok(BookmarkImportResult { imported, updated, skipped, warnings, created_groups: groups })
}

/// 无预览的一步导入，保留给旧调用点。等价于「按默认处置全部接受，且信任载荷」
/// ——信任是为了不改变既有备份恢复的行为（带凭据的自备份仍然照常还原）。
/// 分享场景请走 [`preview_bookmark_import`] + [`apply_bookmark_import`]。
#[tauri::command]
pub fn import_bookmarks(
    app: AppHandle,
    format: String,
    content: String,
    group: Option<String>,
    master_state: State<'_, MasterPasswordState>,
) -> Result<BookmarkImportResult, String> {
    let parsed = parse_import(&format, &content, group.as_deref())?;
    apply_import(
        &app,
        &master_state,
        parsed,
        &HashMap::new(),
        ImportTrust { allow_commands: true, allow_credentials: true },
    )
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

/// Milliseconds since the Unix epoch, 0 if the clock is before it.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
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
    let now = now_millis();
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
        origin: None,
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
    /// network serial bookmark would come back missing everything that makes it
    /// one. Lock the round-trip down.
    #[test]
    fn network_serial_bookmark_survives_a_save_and_load() {
        let json = serde_json::json!({
            "id": "b1",
            "name": "Lab console",
            "protocol": "rfc2217",
            "host": "10.0.0.5",
            "port": 2217,
            "portName": "10.0.0.5:2217",
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

        assert_eq!(reloaded.protocol, "rfc2217");
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
        assert_eq!(parsed.protocol, "serial");
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
            share: None,
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

        let parsed = parse_auraterm_export(&json, "").expect("parse");
        assert_eq!(parsed.entries[0].group.as_deref(), Some("Production/EU"));
        assert_ne!(parsed.entries[0].id, export.connections[0].id, "ids are reassigned on import");
        assert!(parsed.groups.is_empty(), "a plain backup carries no explicit groups");

        let nested = parse_auraterm_export(&json, "Restored").expect("parse");
        assert_eq!(nested.entries[0].group.as_deref(), Some("Restored/Production/EU"));
    }

    /// Builds the share bundle `export_group_bookmarks` would write for group
    /// `root`, without needing an `AppHandle`.
    fn share_bundle(root: &str, groups: &[&str], empty_groups: &[&str]) -> String {
        let connections = groups
            .iter()
            .enumerate()
            .map(|(index, group)| {
                let mut connection = imported_connection(
                    format!("host-{}", index),
                    group.to_string(),
                    "ssh",
                    format!("10.0.0.{}", index),
                    22,
                    "ops".to_string(),
                );
                connection.id = format!("entry-{}", index);
                sanitize_connection_for_sharing(&mut connection, "bundle-1");
                connection
            })
            .collect();
        serde_json::to_string(&BookmarkExport {
            format: BOOKMARK_EXPORT_FORMAT.to_string(),
            version: BOOKMARK_EXPORT_VERSION,
            exported_at: 1,
            include_secrets: false,
            share: Some(ShareMeta {
                kind: default_share_kind(),
                root_name: root.to_string(),
                label: None,
                note: None,
                bundle_id: "bundle-1".to_string(),
                empty_groups: empty_groups.iter().map(|group| group.to_string()).collect(),
                source_label: None,
            }),
            connections,
        })
        .expect("serialize")
    }

    #[test]
    fn relative_paths_are_the_root_rename_with_an_empty_target() {
        assert_eq!(relative_group_path("Prod/EU/Web", "Prod/EU"), Some("Web".to_string()));
        assert_eq!(relative_group_path("Prod/EU", "Prod/EU"), Some(String::new()));
        assert_eq!(relative_group_path("Prod/EUR", "Prod/EU"), None, "prefix, not a parent");
        assert_eq!(relative_group_path("Lab", "Prod/EU"), None);
        // Separator and whitespace noise is normalised before the comparison.
        assert_eq!(relative_group_path("Prod\\EU\\Web ", "Prod/EU"), Some("Web".to_string()));
    }

    #[test]
    fn only_subfolders_no_bookmark_rebuilds_travel_with_the_share() {
        let packed = vec![
            imported_connection("a".into(), "Web".into(), "ssh", "h".into(), 22, "u".into()),
            imported_connection("b".into(), "DB/Replicas".into(), "ssh", "h".into(), 22, "u".into()),
        ];
        let explicit = [
            "Prod/EU/Web".to_string(),      // a bookmark sits here
            "Prod/EU/DB".to_string(),       // rebuilt from DB/Replicas
            "Prod/EU/Staging".to_string(),  // genuinely empty -> must travel
            "Prod/EU".to_string(),          // the root itself
            "Lab".to_string(),              // outside the subtree
        ];
        assert_eq!(
            shared_empty_groups(&explicit, "Prod/EU", &packed),
            vec!["Staging".to_string()],
        );
    }

    #[test]
    fn a_share_lands_under_its_own_name_and_keeps_empty_subfolders() {
        let json = share_bundle("Prod/EU", &["Web", ""], &["Staging/Canary"]);

        // No target group: the bundle recreates the group it was cut from.
        let parsed = parse_auraterm_export(&json, "").expect("parse");
        assert_eq!(parsed.entries[0].group.as_deref(), Some("Prod/EU/Web"));
        assert_eq!(parsed.entries[1].group.as_deref(), Some("Prod/EU"));
        assert_eq!(parsed.groups, vec!["Prod/EU/Staging/Canary".to_string()]);

        // A target group wins, and the sharer's parent folders never appear.
        let parsed = parse_auraterm_export(&json, "From Bill").expect("parse");
        assert_eq!(parsed.entries[0].group.as_deref(), Some("From Bill/Web"));
        assert_eq!(parsed.entries[1].group.as_deref(), Some("From Bill"));
        assert_eq!(parsed.groups, vec!["From Bill/Staging/Canary".to_string()]);
    }

    #[test]
    fn sharing_strips_local_and_secret_fields_but_keeps_a_stable_identity() {
        let mut connection = imported_connection(
            "web".into(), "Web".into(), "ssh", "10.0.0.1".into(), 22, "ops".into(),
        );
        connection.id = "local-uuid".to_string();
        connection.log_path = Some("/Users/bill/logs/web.log".to_string());
        connection.last_used = Some(1_700_000_000_000);
        connection.password = Some("hunter2".to_string());
        connection.post_connect_commands = vec!["sudo -i".to_string()];
        connection.auto_login_rules = vec![SavedAutoLoginRule {
            expect: "assword:".to_string(),
            response: Some("hunter2".to_string()),
            case_sensitive: false,
            timeout_secs: default_expect_timeout(),
        }];

        sanitize_connection_for_sharing(&mut connection, "bundle-1");

        assert_eq!(connection.id, "");
        assert_eq!(connection.log_path, None);
        assert_eq!(connection.last_used, None);
        assert_eq!(connection.password, None);
        assert!(connection.post_connect_commands.is_empty());
        assert_eq!(connection.auto_login_rules[0].response, None);
        assert_eq!(connection.auto_login_rules[0].expect, "assword:", "topology survives");
        let origin = connection.origin.expect("origin is stamped");
        assert_eq!(origin.bundle_id, "bundle-1");
        assert_eq!(origin.entry_id, "local-uuid", "identity outlives the reassigned id");
    }

    #[test]
    fn the_origin_stamp_survives_the_id_being_reassigned() {
        let json = share_bundle("Prod", &["Web"], &[]);
        let parsed = parse_auraterm_export(&json, "").expect("parse");
        let origin = parsed.entries[0].origin.as_ref().expect("origin survives the import");
        assert_eq!(origin.entry_id, "entry-0", "the sharer's identity is what stays put");
        assert_eq!(origin.bundle_id, "bundle-1");
        assert!(!parsed.entries[0].id.is_empty(), "a fresh local id is handed out");
    }

    /// The whole reason `version` stays at 1 and `share` is optional: a client
    /// built before sharing existed must still be able to open a share bundle.
    /// If this test has to change, older AuraTerm installs just lost the file.
    #[test]
    fn a_share_still_opens_in_a_client_that_predates_sharing() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct LegacyExport {
            format: String,
            version: u32,
            connections: Vec<LegacyConnection>,
        }
        // No `origin`, no `share`: exactly the fields v0.3.3 knew about.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct LegacyConnection {
            id: String,
            name: String,
            #[serde(default)]
            group: Option<String>,
            host: String,
        }

        let json = share_bundle("Prod/EU", &["Web"], &["Staging"]);
        let legacy: LegacyExport = serde_json::from_str(&json).expect("old clients still parse it");
        assert_eq!(legacy.format, BOOKMARK_EXPORT_FORMAT);
        assert_eq!(legacy.version, 1, "bumping this locks older clients out of every share");
        assert_eq!(legacy.connections[0].name, "host-0");
        assert_eq!(legacy.connections[0].host, "10.0.0.0");
        assert_eq!(legacy.connections[0].id, "", "cleared, and reassigned by any importer");
        // Relative paths are what makes the fallback correct rather than merely
        // parseable: an old importer prefixes its own target group and gets the
        // subtree in the right shape, only without the share's name.
        assert_eq!(legacy.connections[0].group.as_deref(), Some("Web"));
    }

    // ── 导入：匹配、处置与信任闸门 ────────────────────────────────────────────

    fn ssh(name: &str, host: &str, user: &str) -> SavedConnection {
        imported_connection(
            name.to_string(), String::new(), "ssh", host.to_string(), 22, user.to_string(),
        )
    }

    #[test]
    fn the_share_identity_outranks_the_endpoint_match() {
        let mut local = ssh("prod-web", "10.0.0.1", "ops");
        local.origin = Some(BookmarkOrigin {
            bundle_id: "bundle-1".to_string(),
            entry_id: "entry-1".to_string(),
        });
        // A second local bookmark pointing at the same machine, without an origin.
        let plain = ssh("prod-web-copy", "10.0.0.1", "ops");
        let existing = vec![plain, local];

        // The incoming entry moved to a new host but kept its share identity:
        // it must still resolve to the bookmark it updates, not to the endpoint.
        let mut candidate = ssh("prod-web", "10.0.0.9", "ops");
        candidate.origin = Some(BookmarkOrigin {
            bundle_id: "bundle-2".to_string(),
            entry_id: "entry-1".to_string(),
        });
        assert_eq!(match_existing(&candidate, &existing), Some((1, "origin")));
        assert_eq!(default_action(match_existing(&candidate, &existing)), ACTION_UPDATE);
    }

    #[test]
    fn an_unknown_bookmark_on_a_known_machine_defaults_to_skip() {
        let existing = vec![ssh("prod-web", "10.0.0.1", "ops")];
        // Renaming is not enough to make it a different bookmark — the old key
        // included the name, so this used to import as a second copy.
        let candidate = ssh("web-1", "10.0.0.1", "ops");
        assert_eq!(match_existing(&candidate, &existing), Some((0, "endpoint")));
        assert_eq!(default_action(match_existing(&candidate, &existing)), ACTION_SKIP);

        let elsewhere = ssh("prod-web", "10.0.0.2", "ops");
        assert_eq!(match_existing(&elsewhere, &existing), None);
        assert_eq!(default_action(None), ACTION_ADD);
    }

    #[test]
    fn serial_bookmarks_match_on_the_device_not_the_empty_host() {
        let mut left = imported_connection(
            "uart".into(), String::new(), "serial", String::new(), 0, String::new(),
        );
        left.port_name = Some("/dev/ttyUSB0".to_string());
        let mut right = left.clone();
        right.port_name = Some("/dev/ttyUSB1".to_string());
        // Both have an empty host/user; comparing those would make every serial
        // bookmark a duplicate of every other.
        assert!(!same_endpoint(&left, &right));
        assert!(same_endpoint(&left, &left.clone()));

        let mut unnamed = left.clone();
        unnamed.port_name = None;
        assert!(!same_endpoint(&unnamed, &unnamed.clone()), "no device path, no match");
    }

    #[test]
    fn the_trust_gate_strips_what_runs_on_connect_unless_it_is_allowed() {
        let mut connection = ssh("web", "10.0.0.1", "ops");
        connection.password = Some("hunter2".to_string());
        connection.post_connect_commands = vec!["sudo -i".to_string()];
        connection.auto_login_rules = vec![SavedAutoLoginRule {
            expect: "assword:".to_string(),
            response: Some("hunter2".to_string()),
            case_sensitive: false,
            timeout_secs: default_expect_timeout(),
        }];
        connection.jump_hosts = vec![SavedJumpHost {
            id: "j1".to_string(),
            host: "bastion".to_string(),
            port: 22,
            user: "ops".to_string(),
            auth_type: "password".to_string(),
            password: Some("bastion-pw".to_string()),
            private_key: None,
            passphrase: None,
        }];

        let risks = ImportRisks::survey(std::slice::from_ref(&connection));
        assert_eq!(risks.post_connect_commands, 1);
        assert_eq!(risks.auto_login_responses, 1);
        assert_eq!(risks.jump_host_credentials, 1);
        assert_eq!(risks.passwords, 1);

        let mut stripped = connection.clone();
        apply_trust(&mut stripped, &ImportTrust::default());
        assert!(stripped.post_connect_commands.is_empty());
        assert_eq!(stripped.auto_login_rules[0].response, None);
        assert_eq!(stripped.auto_login_rules[0].expect, "assword:", "the topology stays");
        assert_eq!(stripped.password, None);
        assert_eq!(stripped.jump_hosts[0].password, None);
        assert_eq!(stripped.jump_hosts[0].host, "bastion", "the hop itself stays");

        let mut trusted = connection.clone();
        apply_trust(&mut trusted, &ImportTrust { allow_commands: true, allow_credentials: true });
        assert_eq!(trusted.post_connect_commands, vec!["sudo -i".to_string()]);
        assert_eq!(trusted.password.as_deref(), Some("hunter2"));

        // The two switches are independent: commands can be kept without
        // adopting somebody else's passwords.
        let mut half = connection.clone();
        apply_trust(&mut half, &ImportTrust { allow_commands: true, allow_credentials: false });
        assert_eq!(half.post_connect_commands, vec!["sudo -i".to_string()]);
        assert_eq!(half.password, None);
        assert_eq!(half.auto_login_rules[0].response.as_deref(), Some("hunter2"));
    }

    #[test]
    fn added_bookmarks_do_not_collide_by_name_inside_one_group() {
        let mut first = ssh("web", "10.0.0.1", "ops");
        first.group = Some("Prod".to_string());
        let mut second = ssh("web (2)", "10.0.0.2", "ops");
        second.group = Some("Prod".to_string());
        let existing = vec![first, second];

        assert_eq!(unique_name("web", Some("Prod"), &existing), "web (3)");
        // A different group is a different namespace.
        assert_eq!(unique_name("web", Some("Lab"), &existing), "web");
        assert_eq!(unique_name("db", Some("Prod"), &existing), "db");
    }

    #[test]
    fn foreign_files_are_rejected() {
        assert!(parse_auraterm_export("{\"format\":\"other\",\"version\":1,\"connections\":[]}", "").is_err());
        assert!(parse_auraterm_export("not json", "").is_err());
    }
}
