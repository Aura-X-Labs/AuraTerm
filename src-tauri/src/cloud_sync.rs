//! End-to-end encrypted cloud sync for bookmarks, settings and known-hosts.
//!
//! AuraTerm has no sync backend of its own. Instead, the user points the app at
//! a storage provider they already control — a **GitHub Gist**, a **Gitee Gist**,
//! a **WebDAV** server, or an **AuraXLab account** (the official self-hostable
//! companion server) — and AuraTerm uploads a single encrypted blob there.
//!
//! ## Zero-knowledge by construction
//!
//! Everything that leaves the device is encrypted *before* upload with a
//! user-chosen **sync passphrase** (see [`crate::encryption::encrypt_sync_blob`]),
//! which is independent of the account password / OAuth token used to reach the
//! provider. The provider only ever stores ciphertext, so neither GitHub/Gitee,
//! a WebDAV host, nor an AuraXLab server operator can read the synced data.
//!
//! ## What is synced
//!
//! - **Bookmarks** — saved connection metadata (`connections.json`). Always on.
//! - **Settings** — a curated, device-independent subset (theme, fonts, quick
//!   buttons, output rules…). Window bounds, workspace/pane layout, serial
//!   history and the master-password hash are deliberately excluded. Optional.
//! - **Known hosts** — trusted SSH host-key fingerprints. Optional.
//! - **Credentials** — passwords / private keys. Off by default; requires the
//!   master password to be unlocked, since they must be read in the clear before
//!   being re-encrypted into the sync blob.
//!
//! ## Conflict handling
//!
//! Bookmarks/credentials merge by id (the most recently uploaded copy wins on a
//! conflict); known-hosts union with **local entries winning** (sync must never
//! silently override a fingerprint trusted on this device). A `replace` pull is
//! offered for the "make this device authoritative" case. `cloud_sync_now`
//! performs a two-way sync (merge-pull, then push the merged result).

use crate::connections::{self, SavedConnection};
use crate::encryption::{self, CredentialStore, MasterPasswordState, StoredCredential};
use crate::settings;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use zeroize::Zeroizing;

/// File name used for the encrypted blob inside Gist / WebDAV providers.
const SYNC_FILE_NAME: &str = "auraterm-sync.enc";
/// Encrypted, device-local sync configuration (provider tokens live here).
const SYNC_CONFIG_FILE: &str = "sync_config.enc";
const BUNDLE_SCHEMA: u32 = 1;

/// Top-level settings keys that are safe and useful to sync across devices.
/// Everything not listed here (window bounds, workspace/pane state, serial
/// history, log paths, the master-password hash, …) stays device-local.
const SYNCED_SETTINGS_KEYS: &[&str] = &[
    "fontSize",
    "fontFamily",
    "scrollback",
    "logFileNameTemplate",
    "theme",
    "uiThemeMode",
    "rendererMode",
    "ctrlCCopy",
    "ctrlVPaste",
    "middleClickPaste",
    "showInputBar",
    "quickButtons",
    "outputRules",
    "autoOpenSftp",
    "zmodemDownloadPath",
    "restoreTabsOnStartup",
];

// ============================================================================
// Session passphrase state
// ============================================================================

/// Session cache for the sync passphrase. Held in memory only while the app is
/// running (never persisted), and wiped on lock — mirrors `MasterPasswordState`.
#[derive(Default)]
pub struct SyncState {
    inner: Mutex<Option<Zeroizing<String>>>,
}

impl SyncState {
    pub fn set(&self, passphrase: String) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Zeroizing::new(passphrase));
    }

    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }

    pub fn get(&self) -> Option<Zeroizing<String>> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_unlocked(&self) -> bool {
        self.inner
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }
}

/// Resolve the passphrase to use for this operation: an explicit one (which is
/// also cached for the session) or the session cache. Errors if neither exists.
fn resolve_passphrase(
    sync_state: &SyncState,
    explicit: Option<String>,
) -> Result<Zeroizing<String>, String> {
    if let Some(pass) = explicit {
        if pass.is_empty() {
            return Err("Sync passphrase must not be empty".to_string());
        }
        sync_state.set(pass.clone());
        return Ok(Zeroizing::new(pass));
    }
    sync_state
        .get()
        .ok_or_else(|| "Sync is locked — enter your sync passphrase first.".to_string())
}

// ============================================================================
// Persistent configuration (encrypted at rest with the device-local key)
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GistProvider {
    token: String,
    gist_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct WebdavProvider {
    url: String,
    username: String,
    password: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AuraxlabProvider {
    base_url: String,
    username: String,
    token: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SyncConfig {
    provider: String, // "" | "github" | "gitee" | "webdav" | "auraxlab"
    include_settings: bool,
    include_known_hosts: bool,
    include_credentials: bool,
    auto_sync: bool,
    device_id: String,
    device_label: String,
    last_sync_at: Option<u64>,
    last_remote_version: Option<String>,
    github: GistProvider,
    gitee: GistProvider,
    webdav: WebdavProvider,
    auraxlab: AuraxlabProvider,
}

fn sync_config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(SYNC_CONFIG_FILE))
}

fn load_config(app: &AppHandle) -> Result<SyncConfig, String> {
    let path = sync_config_path(app)?;
    if !path.exists() {
        return Ok(SyncConfig::default());
    }
    let encrypted = fs::read(&path).map_err(|e| format!("Failed to read sync config: {e}"))?;
    let key = encryption::load_or_create_local_key(app)?;
    let plaintext = Zeroizing::new(
        encryption::decrypt_data(&encrypted, &key)
            .map_err(|_| "Sync config is corrupt or was written on another device".to_string())?,
    );
    serde_json::from_slice(&plaintext).map_err(|e| format!("Failed to parse sync config: {e}"))
}

fn save_config(app: &AppHandle, config: &SyncConfig) -> Result<(), String> {
    let path = sync_config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let key = encryption::load_or_create_local_key(app)?;
    let plaintext = Zeroizing::new(
        serde_json::to_vec(config).map_err(|e| format!("Failed to serialize sync config: {e}"))?,
    );
    let encrypted = encryption::encrypt_data(&plaintext, &key)?;
    fs::write(&path, &encrypted).map_err(|e| format!("Failed to write sync config: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ---- frontend-facing (redacted) views & inputs ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GistView {
    token_set: bool,
    gist_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebdavView {
    url: String,
    username: String,
    password_set: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuraxlabView {
    base_url: String,
    username: String,
    token_set: bool,
}

/// Redacted configuration sent to the UI: secrets are reduced to boolean flags
/// so tokens/passwords never round-trip back through the frontend.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfigView {
    provider: String,
    include_settings: bool,
    include_known_hosts: bool,
    include_credentials: bool,
    auto_sync: bool,
    device_id: String,
    device_label: String,
    last_sync_at: Option<u64>,
    last_remote_version: Option<String>,
    passphrase_unlocked: bool,
    github: GistView,
    gitee: GistView,
    webdav: WebdavView,
    auraxlab: AuraxlabView,
}

impl SyncConfigView {
    fn from_config(config: &SyncConfig, passphrase_unlocked: bool) -> Self {
        Self {
            provider: config.provider.clone(),
            include_settings: config.include_settings,
            include_known_hosts: config.include_known_hosts,
            include_credentials: config.include_credentials,
            auto_sync: config.auto_sync,
            device_id: config.device_id.clone(),
            device_label: config.device_label.clone(),
            last_sync_at: config.last_sync_at,
            last_remote_version: config.last_remote_version.clone(),
            passphrase_unlocked,
            github: GistView {
                token_set: !config.github.token.is_empty(),
                gist_id: config.github.gist_id.clone(),
            },
            gitee: GistView {
                token_set: !config.gitee.token.is_empty(),
                gist_id: config.gitee.gist_id.clone(),
            },
            webdav: WebdavView {
                url: config.webdav.url.clone(),
                username: config.webdav.username.clone(),
                password_set: !config.webdav.password.is_empty(),
            },
            auraxlab: AuraxlabView {
                base_url: config.auraxlab.base_url.clone(),
                username: config.auraxlab.username.clone(),
                token_set: !config.auraxlab.token.is_empty(),
            },
        }
    }
}

/// Editable configuration patch from the UI. Secret fields are `Option`:
/// `None` keeps the stored value, `Some("")` clears it, `Some(x)` replaces it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettingsInput {
    provider: String,
    include_settings: bool,
    include_known_hosts: bool,
    include_credentials: bool,
    auto_sync: bool,
    device_label: String,
    github_token: Option<String>,
    github_gist_id: Option<String>,
    gitee_token: Option<String>,
    gitee_gist_id: Option<String>,
    webdav_url: Option<String>,
    webdav_username: Option<String>,
    webdav_password: Option<String>,
    auraxlab_base_url: Option<String>,
    auraxlab_token: Option<String>,
    auraxlab_username: Option<String>,
}

fn apply_input(config: &mut SyncConfig, input: SyncSettingsInput) {
    config.provider = input.provider;
    config.include_settings = input.include_settings;
    config.include_known_hosts = input.include_known_hosts;
    config.include_credentials = input.include_credentials;
    config.auto_sync = input.auto_sync;
    config.device_label = input.device_label;
    if let Some(v) = input.github_token {
        config.github.token = v;
    }
    if let Some(v) = input.github_gist_id {
        config.github.gist_id = v;
    }
    if let Some(v) = input.gitee_token {
        config.gitee.token = v;
    }
    if let Some(v) = input.gitee_gist_id {
        config.gitee.gist_id = v;
    }
    if let Some(v) = input.webdav_url {
        config.webdav.url = v;
    }
    if let Some(v) = input.webdav_username {
        config.webdav.username = v;
    }
    if let Some(v) = input.webdav_password {
        config.webdav.password = v;
    }
    if let Some(v) = input.auraxlab_base_url {
        config.auraxlab.base_url = v;
    }
    if let Some(v) = input.auraxlab_token {
        config.auraxlab.token = v;
    }
    if let Some(v) = input.auraxlab_username {
        config.auraxlab.username = v;
    }
    // Default the AuraXLab server to the official one when left blank.
    if config.auraxlab.base_url.trim().is_empty() {
        config.auraxlab.base_url = DEFAULT_AURAXLAB_BASE_URL.to_string();
    }
}

// ============================================================================
// The sync bundle (plaintext payload, encrypted before it ever leaves the host)
// ============================================================================

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncBundle {
    schema: u32,
    exported_at: u64,
    device_id: String,
    device_label: String,
    #[serde(default)]
    bookmarks: Vec<SavedConnection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings: Option<Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    known_hosts: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    credentials: Vec<StoredCredential>,
}

/// Outcome of a push / pull / two-way sync, surfaced to the UI.
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pushed: bool,
    pulled: bool,
    bookmarks_total: usize,
    bookmarks_added: usize,
    known_hosts_added: usize,
    credentials_synced: usize,
    settings_applied: bool,
    remote_version: Option<String>,
    message: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn extract_settings_subset(app: &AppHandle) -> Result<Value, String> {
    let current = settings::get_settings(app.clone())?;
    let value = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let mut out = serde_json::Map::new();
    if let Value::Object(map) = value {
        for key in SYNCED_SETTINGS_KEYS {
            if let Some(v) = map.get(*key) {
                out.insert((*key).to_string(), v.clone());
            }
        }
    }
    Ok(Value::Object(out))
}

fn apply_settings_subset(app: &AppHandle, subset: &Value) -> Result<(), String> {
    let Value::Object(incoming) = subset else {
        return Ok(());
    };
    let current = settings::get_settings(app.clone())?;
    let mut value = serde_json::to_value(current).map_err(|e| e.to_string())?;
    if let Value::Object(map) = &mut value {
        for key in SYNCED_SETTINGS_KEYS {
            if let Some(v) = incoming.get(*key) {
                map.insert((*key).to_string(), v.clone());
            }
        }
    }
    let merged: settings::Settings = serde_json::from_value(value).map_err(|e| e.to_string())?;
    settings::save_settings(app.clone(), merged)
}

/// Assemble the current device state into a bundle, honoring the include flags.
async fn build_bundle(
    app: &AppHandle,
    master_state: &MasterPasswordState,
    config: &SyncConfig,
) -> Result<SyncBundle, String> {
    let mut bundle = SyncBundle {
        schema: BUNDLE_SCHEMA,
        exported_at: now_ms(),
        device_id: config.device_id.clone(),
        device_label: config.device_label.clone(),
        bookmarks: connections::load_connections(app)?,
        ..Default::default()
    };

    if config.include_settings {
        bundle.settings = Some(extract_settings_subset(app)?);
    }

    if config.include_known_hosts {
        bundle.known_hosts = crate::ssh::export_known_hosts(app).await?;
    }

    if config.include_credentials {
        if !encryption::credentials_accessible(app, master_state) {
            return Err(
                "Unlock the master password before syncing saved credentials.".to_string(),
            );
        }
        let secret = encryption::resolve_secret(app, master_state)?;
        let store = encryption::load_encrypted_credentials(app, &secret)?;
        bundle.credentials = store.credentials.clone();
    }

    Ok(bundle)
}

/// Merge a downloaded bundle into local state. `replace` makes the bundle
/// authoritative for bookmarks (and credentials); otherwise entries are unioned.
async fn apply_bundle(
    app: &AppHandle,
    master_state: &MasterPasswordState,
    config: &SyncConfig,
    bundle: SyncBundle,
    replace: bool,
) -> Result<SyncResult, String> {
    let mut result = SyncResult::default();

    // -- bookmarks --
    let local = connections::load_connections(app)?;
    let merged = merge_bookmarks(local, bundle.bookmarks, replace);
    result.bookmarks_added = merged.added;
    result.bookmarks_total = merged.items.len();
    connections::write_connections(app, &merged.items)?;

    // -- credentials (only when readable; encrypted at rest under the local key/master pw) --
    if !bundle.credentials.is_empty() && encryption::credentials_accessible(app, master_state) {
        let secret = encryption::resolve_secret(app, master_state)?;
        let mut store = encryption::load_encrypted_credentials(app, &secret)
            .unwrap_or_else(|_| CredentialStore { credentials: Vec::new() });
        let mut synced = 0usize;
        for incoming in bundle.credentials {
            store
                .credentials
                .retain(|c| c.connection_id != incoming.connection_id);
            store.credentials.push(incoming);
            synced += 1;
        }
        encryption::save_encrypted_credentials(app, &store, &secret)?;
        result.credentials_synced = synced;
    }

    // -- settings subset --
    if let Some(subset) = &bundle.settings {
        if config.include_settings {
            apply_settings_subset(app, subset)?;
            result.settings_applied = true;
        }
    }

    // -- known hosts (union, local wins) --
    if !bundle.known_hosts.is_empty() && config.include_known_hosts {
        result.known_hosts_added = crate::ssh::import_known_hosts(app, bundle.known_hosts).await?;
    }

    result.pulled = true;
    Ok(result)
}

struct MergeOutcome {
    items: Vec<SavedConnection>,
    added: usize,
}

/// Union by id; the incoming (remote) copy wins on a conflict. With `replace`,
/// the remote set becomes authoritative wholesale.
fn merge_bookmarks(
    local: Vec<SavedConnection>,
    remote: Vec<SavedConnection>,
    replace: bool,
) -> MergeOutcome {
    if replace {
        let added = remote.len();
        return MergeOutcome {
            items: remote,
            added,
        };
    }
    let mut items = local;
    let mut added = 0usize;
    for incoming in remote {
        if let Some(pos) = items.iter().position(|c| c.id == incoming.id) {
            items[pos] = incoming;
        } else {
            items.push(incoming);
            added += 1;
        }
    }
    MergeOutcome { items, added }
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ============================================================================
// HTTP plumbing & providers
// ============================================================================

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("AuraTerm-Sync/1.0")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// A downloaded encrypted blob plus the provider's opaque version marker.
struct RemoteBlob {
    data: Vec<u8>,
    version: Option<String>,
}

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn json_message(body: &Value, status: StatusCode) -> String {
    body.get("message")
        .or_else(|| body.get("error"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("HTTP {}", status.as_u16()))
}

fn header_version(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get(reqwest::header::ETAG)
        .or_else(|| resp.headers().get(reqwest::header::LAST_MODIFIED))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn normalize_base_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

/// The official AuraXLab server. Used as the default so users never have to type
/// a URL; self-hosters can still point at their own server explicitly.
const DEFAULT_AURAXLAB_BASE_URL: &str = "https://auraxlab.com";

/// Normalize an AuraXLab server URL, falling back to the official server when
/// the caller leaves it blank.
fn resolve_auraxlab_base(raw: &str) -> String {
    let normalized = normalize_base_url(raw);
    if normalized.is_empty() {
        DEFAULT_AURAXLAB_BASE_URL.to_string()
    } else {
        normalized
    }
}

// ---- GitHub / Gitee Gist (shared shape) ----

async fn gist_push(
    api_base: &str,
    token: &str,
    gist_id: &str,
    content_b64: &str,
    bearer: bool,
) -> Result<(String, Option<String>), String> {
    let client = http_client()?;
    let files = json!({ SYNC_FILE_NAME: { "content": content_b64 } });
    let resp = if gist_id.is_empty() {
        let mut body = json!({
            "description": "AuraTerm encrypted sync vault",
            "public": false,
            "files": files,
        });
        if !bearer {
            body["access_token"] = json!(token);
        }
        let mut req = client.post(format!("{api_base}/gists"));
        if bearer {
            req = req.bearer_auth(token);
        }
        req.json(&body).send().await
    } else {
        let mut body = json!({ "files": files });
        if !bearer {
            body["access_token"] = json!(token);
        }
        let mut req = client.patch(format!("{api_base}/gists/{gist_id}"));
        if bearer {
            req = req.bearer_auth(token);
        }
        req.json(&body).send().await
    }
    .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    if !status.is_success() {
        return Err(format!("Gist upload failed: {}", json_message(&body, status)));
    }
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(gist_id)
        .to_string();
    let version = body
        .get("updated_at")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok((id, version))
}

async fn gist_pull(
    api_base: &str,
    token: &str,
    gist_id: &str,
    bearer: bool,
) -> Result<RemoteBlob, String> {
    if gist_id.is_empty() {
        return Err("No Gist has been created yet — push from one device first.".to_string());
    }
    let client = http_client()?;
    let url = if bearer {
        format!("{api_base}/gists/{gist_id}")
    } else {
        format!("{api_base}/gists/{gist_id}?access_token={token}")
    };
    let mut req = client.get(url);
    if bearer {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    if !status.is_success() {
        return Err(format!("Gist download failed: {}", json_message(&body, status)));
    }
    let content = body
        .get("files")
        .and_then(|f| f.get(SYNC_FILE_NAME))
        .and_then(|file| file.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| "The Gist does not contain AuraTerm sync data.".to_string())?;
    let data = STANDARD
        .decode(content.trim())
        .map_err(|e| format!("Synced data is not valid base64: {e}"))?;
    let version = body
        .get("updated_at")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(RemoteBlob { data, version })
}

// ---- WebDAV ----

async fn webdav_push(cfg: &WebdavProvider, blob: &[u8]) -> Result<Option<String>, String> {
    let client = http_client()?;
    let resp = client
        .put(cfg.url.trim())
        .basic_auth(&cfg.username, Some(cfg.password.clone()))
        .body(blob.to_vec())
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "WebDAV upload failed: HTTP {}",
            resp.status().as_u16()
        ));
    }
    Ok(header_version(&resp))
}

async fn webdav_pull(cfg: &WebdavProvider) -> Result<RemoteBlob, String> {
    let client = http_client()?;
    let resp = client
        .get(cfg.url.trim())
        .basic_auth(&cfg.username, Some(cfg.password.clone()))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Err("No sync data found on the WebDAV server yet.".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!(
            "WebDAV download failed: HTTP {}",
            resp.status().as_u16()
        ));
    }
    let version = header_version(&resp);
    let data = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok(RemoteBlob { data, version })
}

// ---- AuraXLab account (the official self-hostable server) ----

fn auraxlab_vault_url(base_url: &str) -> String {
    format!("{}/api/v1/auraterm/sync/vault", resolve_auraxlab_base(base_url))
}

async fn auraxlab_push(
    cfg: &AuraxlabProvider,
    blob: &[u8],
    base_version: Option<&str>,
    device_id: &str,
    device_label: &str,
) -> Result<Option<String>, String> {
    let client = http_client()?;
    let body = json!({
        "blob": STANDARD.encode(blob),
        "baseVersion": base_version.and_then(|v| v.parse::<i64>().ok()),
        "contentHash": sha256_hex(blob),
        "deviceId": device_id,
        "deviceLabel": device_label,
    });
    let resp = client
        .put(auraxlab_vault_url(&cfg.base_url))
        .basic_auth(&cfg.token, Some(""))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let payload = parse_json(&bytes);
    if status == StatusCode::CONFLICT {
        return Err(
            "The server has newer data than this device. Pull first, then push again.".to_string(),
        );
    }
    if !status.is_success() {
        return Err(format!(
            "AuraXLab sync failed: {}",
            json_message(&payload, status)
        ));
    }
    Ok(payload
        .get("version")
        .map(|v| v.to_string().trim_matches('"').to_string()))
}

async fn auraxlab_pull(cfg: &AuraxlabProvider) -> Result<RemoteBlob, String> {
    let client = http_client()?;
    let resp = client
        .get(auraxlab_vault_url(&cfg.base_url))
        .basic_auth(&cfg.token, Some(""))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let payload = parse_json(&bytes);
    if status == StatusCode::NOT_FOUND {
        return Err("Your AuraXLab account has no synced data yet.".to_string());
    }
    if !status.is_success() {
        return Err(format!(
            "AuraXLab download failed: {}",
            json_message(&payload, status)
        ));
    }
    let content = payload
        .get("blob")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Server response did not contain a sync blob.".to_string())?;
    let data = STANDARD
        .decode(content.trim())
        .map_err(|e| format!("Synced data is not valid base64: {e}"))?;
    let version = payload.get("version").map(|v| v.to_string().trim_matches('"').to_string());
    Ok(RemoteBlob { data, version })
}

// ---- provider dispatch ----

async fn provider_push(
    config: &mut SyncConfig,
    blob: &[u8],
) -> Result<Option<String>, String> {
    let content_b64 = STANDARD.encode(blob);
    match config.provider.as_str() {
        "github" => {
            let (id, version) = gist_push(
                "https://api.github.com",
                &config.github.token,
                &config.github.gist_id,
                &content_b64,
                true,
            )
            .await?;
            config.github.gist_id = id;
            Ok(version)
        }
        "gitee" => {
            let (id, version) = gist_push(
                "https://gitee.com/api/v5",
                &config.gitee.token,
                &config.gitee.gist_id,
                &content_b64,
                false,
            )
            .await?;
            config.gitee.gist_id = id;
            Ok(version)
        }
        "webdav" => webdav_push(&config.webdav, blob).await,
        "auraxlab" => {
            auraxlab_push(
                &config.auraxlab,
                blob,
                config.last_remote_version.as_deref(),
                &config.device_id,
                &config.device_label,
            )
            .await
        }
        other => Err(format!("Unknown sync provider: '{other}'")),
    }
}

async fn provider_pull(config: &SyncConfig) -> Result<RemoteBlob, String> {
    match config.provider.as_str() {
        "github" => {
            gist_pull(
                "https://api.github.com",
                &config.github.token,
                &config.github.gist_id,
                true,
            )
            .await
        }
        "gitee" => {
            gist_pull(
                "https://gitee.com/api/v5",
                &config.gitee.token,
                &config.gitee.gist_id,
                false,
            )
            .await
        }
        "webdav" => webdav_pull(&config.webdav).await,
        "auraxlab" => auraxlab_pull(&config.auraxlab).await,
        other => Err(format!("Unknown sync provider: '{other}'")),
    }
}

fn ensure_provider_ready(config: &SyncConfig) -> Result<(), String> {
    match config.provider.as_str() {
        "github" => {
            if config.github.token.is_empty() {
                return Err("Add a GitHub personal access token (gist scope) first.".to_string());
            }
        }
        "gitee" => {
            if config.gitee.token.is_empty() {
                return Err("Add a Gitee private token (gists scope) first.".to_string());
            }
        }
        "webdav" => {
            if config.webdav.url.is_empty() {
                return Err("Set the WebDAV file URL first.".to_string());
            }
        }
        "auraxlab" => {
            if config.auraxlab.base_url.is_empty() || config.auraxlab.token.is_empty() {
                return Err("Sign in to your AuraXLab account first.".to_string());
            }
        }
        "" => return Err("Choose a sync provider first.".to_string()),
        other => return Err(format!("Unknown sync provider: '{other}'")),
    }
    Ok(())
}

// ============================================================================
// Tauri commands
// ============================================================================

#[tauri::command]
pub fn get_sync_config(
    app: AppHandle,
    sync_state: State<'_, SyncState>,
) -> Result<SyncConfigView, String> {
    let config = load_config(&app)?;
    Ok(SyncConfigView::from_config(&config, sync_state.is_unlocked()))
}

#[tauri::command]
pub fn set_sync_config(
    app: AppHandle,
    input: SyncSettingsInput,
    sync_state: State<'_, SyncState>,
) -> Result<SyncConfigView, String> {
    let mut config = load_config(&app)?;
    apply_input(&mut config, input);
    if config.device_id.is_empty() {
        config.device_id = uuid::Uuid::new_v4().to_string();
    }
    if config.device_label.trim().is_empty() {
        config.device_label = format!("device-{}", &config.device_id[..8.min(config.device_id.len())]);
    }
    save_config(&app, &config)?;
    Ok(SyncConfigView::from_config(&config, sync_state.is_unlocked()))
}

#[tauri::command]
pub fn set_sync_passphrase(passphrase: String, sync_state: State<'_, SyncState>) -> Result<(), String> {
    if passphrase.is_empty() {
        return Err("Sync passphrase must not be empty".to_string());
    }
    sync_state.set(passphrase);
    Ok(())
}

#[tauri::command]
pub fn lock_sync_passphrase(sync_state: State<'_, SyncState>) -> Result<(), String> {
    sync_state.clear();
    Ok(())
}

#[tauri::command]
pub fn is_sync_unlocked(sync_state: State<'_, SyncState>) -> bool {
    sync_state.is_unlocked()
}

#[tauri::command]
pub async fn cloud_sync_push(
    app: AppHandle,
    passphrase: Option<String>,
    master_state: State<'_, MasterPasswordState>,
    sync_state: State<'_, SyncState>,
) -> Result<SyncResult, String> {
    let pass = resolve_passphrase(&sync_state, passphrase)?;
    let mut config = load_config(&app)?;
    ensure_provider_ready(&config)?;
    if config.device_id.is_empty() {
        config.device_id = uuid::Uuid::new_v4().to_string();
    }

    let bundle = build_bundle(&app, &master_state, &config).await?;
    let plaintext =
        Zeroizing::new(serde_json::to_vec(&bundle).map_err(|e| e.to_string())?);
    let blob = encryption::encrypt_sync_blob(&plaintext, &pass)?;

    let version = provider_push(&mut config, &blob).await?;
    config.last_sync_at = Some(now_ms());
    if version.is_some() {
        config.last_remote_version = version.clone();
    }
    save_config(&app, &config)?;

    Ok(SyncResult {
        pushed: true,
        bookmarks_total: bundle.bookmarks.len(),
        remote_version: version,
        message: "Uploaded encrypted data to the cloud.".to_string(),
        ..Default::default()
    })
}

#[tauri::command]
pub async fn cloud_sync_pull(
    app: AppHandle,
    passphrase: Option<String>,
    replace: bool,
    master_state: State<'_, MasterPasswordState>,
    sync_state: State<'_, SyncState>,
) -> Result<SyncResult, String> {
    let pass = resolve_passphrase(&sync_state, passphrase)?;
    let mut config = load_config(&app)?;
    ensure_provider_ready(&config)?;

    let remote = provider_pull(&config).await?;
    let plaintext = Zeroizing::new(encryption::decrypt_sync_blob(&remote.data, &pass)?);
    let bundle: SyncBundle =
        serde_json::from_slice(&plaintext).map_err(|e| format!("Corrupt sync bundle: {e}"))?;

    let mut result = apply_bundle(&app, &master_state, &config, bundle, replace).await?;
    config.last_sync_at = Some(now_ms());
    if remote.version.is_some() {
        config.last_remote_version = remote.version.clone();
    }
    save_config(&app, &config)?;

    result.remote_version = remote.version;
    result.message = if replace {
        "Replaced local data with the cloud copy.".to_string()
    } else {
        "Merged the cloud copy into local data.".to_string()
    };
    Ok(result)
}

#[tauri::command]
pub async fn cloud_sync_now(
    app: AppHandle,
    passphrase: Option<String>,
    master_state: State<'_, MasterPasswordState>,
    sync_state: State<'_, SyncState>,
) -> Result<SyncResult, String> {
    let pass = resolve_passphrase(&sync_state, passphrase)?;
    let mut config = load_config(&app)?;
    ensure_provider_ready(&config)?;
    if config.device_id.is_empty() {
        config.device_id = uuid::Uuid::new_v4().to_string();
    }

    // 1) Pull & merge (tolerate "nothing uploaded yet").
    let mut result = SyncResult::default();
    match provider_pull(&config).await {
        Ok(remote) => {
            let plaintext = Zeroizing::new(encryption::decrypt_sync_blob(&remote.data, &pass)?);
            let bundle: SyncBundle = serde_json::from_slice(&plaintext)
                .map_err(|e| format!("Corrupt sync bundle: {e}"))?;
            result = apply_bundle(&app, &master_state, &config, bundle, false).await?;
            if remote.version.is_some() {
                config.last_remote_version = remote.version;
            }
        }
        Err(e) => {
            // First-ever sync (or empty remote): proceed straight to push.
            result.message = format!("(nothing to merge: {e}) ");
        }
    }

    // 2) Push the merged result back.
    let bundle = build_bundle(&app, &master_state, &config).await?;
    let plaintext = Zeroizing::new(serde_json::to_vec(&bundle).map_err(|e| e.to_string())?);
    let blob = encryption::encrypt_sync_blob(&plaintext, &pass)?;
    let version = provider_push(&mut config, &blob).await?;

    config.last_sync_at = Some(now_ms());
    if version.is_some() {
        config.last_remote_version = version.clone();
    }
    save_config(&app, &config)?;

    result.pushed = true;
    result.bookmarks_total = bundle.bookmarks.len();
    result.remote_version = version;
    result.message.push_str("Two-way sync complete.");
    Ok(result)
}

#[tauri::command]
pub async fn cloud_sync_test_connection(app: AppHandle) -> Result<String, String> {
    let config = load_config(&app)?;
    ensure_provider_ready(&config)?;
    match provider_pull(&config).await {
        Ok(_) => Ok("Connected — found existing sync data.".to_string()),
        // A "no data yet" style error still proves the endpoint + auth work.
        Err(e)
            if e.contains("no synced data")
                || e.contains("No sync data")
                || e.contains("No Gist")
                || e.contains("has no synced") =>
        {
            Ok("Connected — no data uploaded yet.".to_string())
        }
        Err(e) => Err(e),
    }
}

/// Register a new AuraXLab account from the desktop app (web confirmation email
/// is still required before the account can sync).
#[tauri::command]
pub async fn auraxlab_register(
    base_url: String,
    email: String,
    username: String,
    password: String,
) -> Result<String, String> {
    let client = http_client()?;
    let url = format!(
        "{}/api/v1/auraterm/sync/register",
        resolve_auraxlab_base(&base_url)
    );
    let resp = client
        .post(url)
        .json(&json!({ "email": email, "username": username, "password": password }))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    if !status.is_success() {
        return Err(json_message(&body, status));
    }
    Ok(body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("Account created. Check your email to confirm it, then sign in.")
        .to_string())
}

/// Sign in to an AuraXLab account, store the returned API token, and select the
/// AuraXLab provider.
#[tauri::command]
pub async fn auraxlab_login(
    app: AppHandle,
    base_url: String,
    email: String,
    password: String,
    sync_state: State<'_, SyncState>,
) -> Result<SyncConfigView, String> {
    let client = http_client()?;
    let resolved_base = resolve_auraxlab_base(&base_url);
    let url = format!("{}/api/v1/tokens/", resolved_base);
    let resp = client
        .post(url)
        .basic_auth(&email, Some(password))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    if !status.is_success() {
        return Err(format!("Sign-in failed: {}", json_message(&body, status)));
    }
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Server did not return a token.".to_string())?;

    let mut config = load_config(&app)?;
    config.provider = "auraxlab".to_string();
    config.auraxlab.base_url = resolved_base;
    config.auraxlab.token = token.to_string();
    config.auraxlab.username = email.clone();
    if config.device_id.is_empty() {
        config.device_id = uuid::Uuid::new_v4().to_string();
    }
    save_config(&app, &config)?;
    Ok(SyncConfigView::from_config(&config, sync_state.is_unlocked()))
}

/// Sign out of the AuraXLab account (clears the stored token only).
#[tauri::command]
pub fn auraxlab_logout(app: AppHandle, sync_state: State<'_, SyncState>) -> Result<SyncConfigView, String> {
    let mut config = load_config(&app)?;
    config.auraxlab.token = String::new();
    config.last_remote_version = None;
    save_config(&app, &config)?;
    Ok(SyncConfigView::from_config(&config, sync_state.is_unlocked()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookmark(id: &str, name: &str) -> SavedConnection {
        SavedConnection {
            id: id.to_string(),
            name: name.to_string(),
            group: None,
            log_path: None,
            protocol: "ssh".to_string(),
            host: "example.com".to_string(),
            port: 22,
            user: "root".to_string(),
            auth_type: "password".to_string(),
            password: None,
            private_key: None,
            passphrase: None,
            agent_forwarding: false,
            jump_hosts: Vec::new(),
            auto_login_rules: Vec::new(),
            post_connect_commands: Vec::new(),
            port_name: None,
            baud_rate: None,
            data_bits: None,
            stop_bits: None,
            parity: None,
            flow_control: None,
            created_at: 0,
            last_used: None,
            auto_reconnect: false,
            reconnect_type: "manual".to_string(),
            tunnels: Vec::new(),
        }
    }

    #[test]
    fn merge_unions_new_and_overwrites_conflicts() {
        let local = vec![bookmark("a", "local-a"), bookmark("b", "local-b")];
        let remote = vec![bookmark("b", "remote-b"), bookmark("c", "remote-c")];
        let out = merge_bookmarks(local, remote, false);
        assert_eq!(out.items.len(), 3); // a, b, c
        assert_eq!(out.added, 1); // only c is new
        let b = out.items.iter().find(|c| c.id == "b").unwrap();
        assert_eq!(b.name, "remote-b", "remote copy wins on id conflict");
    }

    #[test]
    fn merge_replace_makes_remote_authoritative() {
        let local = vec![bookmark("a", "local-a")];
        let remote = vec![bookmark("c", "remote-c")];
        let out = merge_bookmarks(local, remote, true);
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].id, "c");
    }

    #[test]
    fn bundle_serde_roundtrip_omits_empty_optionals() {
        let bundle = SyncBundle {
            schema: BUNDLE_SCHEMA,
            exported_at: 123,
            device_id: "dev".to_string(),
            device_label: "label".to_string(),
            bookmarks: vec![bookmark("a", "a")],
            settings: None,
            known_hosts: HashMap::new(),
            credentials: Vec::new(),
        };
        let json = serde_json::to_value(&bundle).unwrap();
        assert!(json.get("settings").is_none(), "empty settings omitted");
        assert!(json.get("knownHosts").is_none(), "empty knownHosts omitted");
        assert!(json.get("credentials").is_none(), "empty credentials omitted");
        let back: SyncBundle = serde_json::from_value(json).unwrap();
        assert_eq!(back.bookmarks.len(), 1);
        assert_eq!(back.schema, BUNDLE_SCHEMA);
    }

    #[test]
    fn config_view_redacts_secrets() {
        let mut config = SyncConfig::default();
        config.provider = "github".to_string();
        config.github.token = "ghp_secret".to_string();
        config.github.gist_id = "abc123".to_string();
        let view = SyncConfigView::from_config(&config, false);
        assert!(view.github.token_set);
        assert_eq!(view.github.gist_id, "abc123");
        // Serialized view must never contain the raw token.
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("ghp_secret"));
    }

    #[test]
    fn input_patch_keeps_unset_secrets() {
        let mut config = SyncConfig::default();
        config.github.token = "keep-me".to_string();
        let input = SyncSettingsInput {
            provider: "github".to_string(),
            include_settings: true,
            include_known_hosts: false,
            include_credentials: false,
            auto_sync: false,
            device_label: "laptop".to_string(),
            github_token: None, // unset -> preserve
            github_gist_id: Some("g1".to_string()),
            gitee_token: None,
            gitee_gist_id: None,
            webdav_url: None,
            webdav_username: None,
            webdav_password: None,
            auraxlab_base_url: None,
            auraxlab_token: None,
            auraxlab_username: None,
        };
        apply_input(&mut config, input);
        assert_eq!(config.github.token, "keep-me");
        assert_eq!(config.github.gist_id, "g1");
        assert_eq!(config.device_label, "laptop");
    }

    #[test]
    fn sha256_hex_is_lowercase_64_chars() {
        let h = sha256_hex(b"hello");
        assert_eq!(h.len(), 64);
        assert_eq!(
            h,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn auraxlab_base_defaults_to_official_server() {
        assert_eq!(resolve_auraxlab_base(""), "https://auraxlab.com");
        assert_eq!(resolve_auraxlab_base("   "), "https://auraxlab.com");
        // a self-hosted URL is honored (and trailing slash trimmed)
        assert_eq!(resolve_auraxlab_base("https://my.example.com/"), "https://my.example.com");
    }

    #[test]
    fn apply_input_defaults_blank_auraxlab_base() {
        let mut config = SyncConfig::default();
        let input = SyncSettingsInput {
            provider: "auraxlab".to_string(),
            include_settings: true,
            include_known_hosts: true,
            include_credentials: false,
            auto_sync: false,
            device_label: "d".to_string(),
            github_token: None,
            github_gist_id: None,
            gitee_token: None,
            gitee_gist_id: None,
            webdav_url: None,
            webdav_username: None,
            webdav_password: None,
            auraxlab_base_url: Some(String::new()), // blank -> official default
            auraxlab_token: None,
            auraxlab_username: None,
        };
        apply_input(&mut config, input);
        assert_eq!(config.auraxlab.base_url, "https://auraxlab.com");
    }

    // ========================================================================
    // Provider integration tests
    //
    // These exercise the REAL provider HTTP client code (reqwest + JSON/base64
    // handling + response parsing) end-to-end (encrypt -> push -> pull ->
    // decrypt) against in-process mock servers that emulate each provider's API
    // contract. The opt-in `#[ignore]`d tests below hit real endpoints when the
    // matching env vars are set, for true integration against live services.
    // ========================================================================

    use std::sync::Arc;

    /// Spawn a tiny in-process HTTP server; the handler maps
    /// (method, url, body) -> (status, body, headers). Returns the base URL.
    /// The server thread runs until the test process exits.
    fn spawn_mock<F>(handler: F) -> String
    where
        F: Fn(&str, &str, &[u8]) -> (u16, Vec<u8>, Vec<(&'static str, String)>) + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind mock server");
        let addr = server.server_addr().to_ip().expect("mock server ip addr");
        let base = format!("http://{}", addr);
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let method = request.method().to_string();
                let url = request.url().to_string();
                let mut body = Vec::new();
                let _ = request.as_reader().read_to_end(&mut body);
                let (status, data, headers) = handler(&method, &url, &body);
                let mut response = tiny_http::Response::from_data(data).with_status_code(status);
                for (key, value) in headers {
                    if let Ok(header) = tiny_http::Header::from_bytes(key.as_bytes(), value.as_bytes()) {
                        response.add_header(header);
                    }
                }
                let _ = request.respond(response);
            }
        });
        base
    }

    fn skip_unless_env(var: &str) -> Option<String> {
        match std::env::var(var) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => {
                eprintln!("SKIP: set {var} to run this real-endpoint integration test");
                None
            }
        }
    }

    // ---- GitHub / Gitee Gist (mock) ----

    /// Shared Gist mock: emulates POST /gists (create), PATCH /gists/{id}
    /// (update) and GET /gists/{id} (read) for both GitHub (Bearer) and Gitee
    /// (access_token) flows. Stores the single sync file's content in `state`.
    fn spawn_gist_mock(state: Arc<Mutex<String>>) -> String {
        assert_eq!(SYNC_FILE_NAME, "auraterm-sync.enc");
        spawn_mock(move |method, url, body| {
            let mut content = state.lock().unwrap();
            let parse = || -> Value { serde_json::from_slice(body).unwrap_or(Value::Null) };
            if method == "POST" && url.starts_with("/gists") {
                let v = parse();
                *content = v["files"][SYNC_FILE_NAME]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let resp = json!({"id": "gist_test_1", "updated_at": "2026-01-01T00:00:00Z"});
                (201, serde_json::to_vec(&resp).unwrap(), vec![])
            } else if method == "PATCH" && url.starts_with("/gists/gist_test_1") {
                let v = parse();
                if let Some(c) = v["files"][SYNC_FILE_NAME]["content"].as_str() {
                    *content = c.to_string();
                }
                let resp = json!({"id": "gist_test_1", "updated_at": "2026-01-02T00:00:00Z"});
                (200, serde_json::to_vec(&resp).unwrap(), vec![])
            } else if method == "GET" && url.starts_with("/gists/gist_test_1") {
                let resp = json!({
                    "files": {"auraterm-sync.enc": {"content": *content}},
                    "updated_at": "2026-01-02T00:00:00Z",
                });
                (200, serde_json::to_vec(&resp).unwrap(), vec![])
            } else {
                (404, b"{}".to_vec(), vec![])
            }
        })
    }

    #[tokio::test]
    async fn integ_github_gist_roundtrip() {
        let base = spawn_gist_mock(Arc::new(Mutex::new(String::new())));
        let blob = encryption::encrypt_sync_blob(b"{\"bookmarks\":[1]}", "pw").unwrap();

        // create
        let (id, version) = gist_push(&base, "tok", "", &STANDARD.encode(&blob), true)
            .await
            .unwrap();
        assert_eq!(id, "gist_test_1");
        assert!(version.is_some());

        // read back + decrypt
        let remote = gist_pull(&base, "tok", &id, true).await.unwrap();
        assert_eq!(remote.data, blob);
        assert_eq!(
            encryption::decrypt_sync_blob(&remote.data, "pw").unwrap(),
            b"{\"bookmarks\":[1]}"
        );

        // update via PATCH (gist_id reused)
        let blob2 = encryption::encrypt_sync_blob(b"{\"bookmarks\":[2]}", "pw").unwrap();
        gist_push(&base, "tok", &id, &STANDARD.encode(&blob2), true)
            .await
            .unwrap();
        let remote2 = gist_pull(&base, "tok", &id, true).await.unwrap();
        assert_eq!(
            encryption::decrypt_sync_blob(&remote2.data, "pw").unwrap(),
            b"{\"bookmarks\":[2]}"
        );
    }

    #[tokio::test]
    async fn integ_gitee_gist_roundtrip() {
        // Gitee flow (bearer = false): access_token travels in the body / query.
        let base = spawn_gist_mock(Arc::new(Mutex::new(String::new())));
        let blob = encryption::encrypt_sync_blob(b"gitee-payload", "pw").unwrap();
        let (id, _v) = gist_push(&base, "gitee_tok", "", &STANDARD.encode(&blob), false)
            .await
            .unwrap();
        assert_eq!(id, "gist_test_1");
        let remote = gist_pull(&base, "gitee_tok", &id, false).await.unwrap();
        assert_eq!(
            encryption::decrypt_sync_blob(&remote.data, "pw").unwrap(),
            b"gitee-payload"
        );
    }

    // ---- WebDAV (mock) ----

    #[tokio::test]
    async fn integ_webdav_roundtrip() {
        let stored = Arc::new(Mutex::new(Option::<Vec<u8>>::None));
        let s = stored.clone();
        let base = spawn_mock(move |method, _url, body| match method {
            "PUT" => {
                *s.lock().unwrap() = Some(body.to_vec());
                (201, Vec::new(), vec![("ETag", "\"etag-1\"".to_string())])
            }
            "GET" => match &*s.lock().unwrap() {
                Some(data) => (200, data.clone(), vec![("ETag", "\"etag-1\"".to_string())]),
                None => (404, Vec::new(), vec![]),
            },
            _ => (405, Vec::new(), vec![]),
        });

        let cfg = WebdavProvider {
            url: format!("{}/auraterm-sync.enc", base),
            username: "u".into(),
            password: "p".into(),
        };
        let blob = encryption::encrypt_sync_blob(b"webdav-data", "pw").unwrap();
        let version = webdav_push(&cfg, &blob).await.unwrap();
        assert_eq!(version.as_deref(), Some("\"etag-1\""));
        let remote = webdav_pull(&cfg).await.unwrap();
        assert_eq!(remote.data, blob);
        assert_eq!(
            encryption::decrypt_sync_blob(&remote.data, "pw").unwrap(),
            b"webdav-data"
        );
    }

    // ---- AuraXLab vault (mock) incl. optimistic-concurrency 409 ----

    #[tokio::test]
    async fn integ_auraxlab_roundtrip_and_conflict() {
        // state = (stored blob, version)
        let store = Arc::new(Mutex::new((Option::<String>::None, 0i64)));
        let st = store.clone();
        let base = spawn_mock(move |method, url, body| {
            if !url.contains("/auraterm/sync/vault") {
                return (404, b"{}".to_vec(), vec![]);
            }
            let mut g = st.lock().unwrap();
            match method {
                "PUT" => {
                    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                    let base_version = v.get("baseVersion").and_then(|x| x.as_i64());
                    let blob = v.get("blob").and_then(|x| x.as_str()).unwrap_or_default().to_string();
                    if let Some(bv) = base_version {
                        if bv != g.1 {
                            let resp = json!({"error": "conflict", "version": g.1});
                            return (409, serde_json::to_vec(&resp).unwrap(), vec![]);
                        }
                    }
                    g.0 = Some(blob);
                    g.1 += 1;
                    let resp = json!({"version": g.1, "updated_at": "2026-01-01T00:00:00Z"});
                    (200, serde_json::to_vec(&resp).unwrap(), vec![])
                }
                "GET" => match &g.0 {
                    Some(b) => (
                        200,
                        serde_json::to_vec(&json!({"blob": b, "version": g.1})).unwrap(),
                        vec![],
                    ),
                    None => (404, b"{}".to_vec(), vec![]),
                },
                _ => (405, b"{}".to_vec(), vec![]),
            }
        });

        let cfg = AuraxlabProvider {
            base_url: base,
            username: "u".into(),
            token: "tok".into(),
        };

        // first push (no base version) -> v1
        let blob = encryption::encrypt_sync_blob(b"axlab-1", "pw").unwrap();
        let v1 = auraxlab_push(&cfg, &blob, None, "dev", "label").await.unwrap();
        assert_eq!(v1.as_deref(), Some("1"));

        let pulled = auraxlab_pull(&cfg).await.unwrap();
        assert_eq!(pulled.data, blob);
        assert_eq!(pulled.version.as_deref(), Some("1"));
        assert_eq!(encryption::decrypt_sync_blob(&pulled.data, "pw").unwrap(), b"axlab-1");

        // stale push (baseVersion 0, server is at 1) -> 409 conflict
        let blob2 = encryption::encrypt_sync_blob(b"axlab-2", "pw").unwrap();
        let err = auraxlab_push(&cfg, &blob2, Some("0"), "dev", "label")
            .await
            .unwrap_err();
        assert!(err.to_lowercase().contains("pull"), "got: {err}");

        // correct push (baseVersion 1) -> v2
        let v2 = auraxlab_push(&cfg, &blob2, Some("1"), "dev", "label").await.unwrap();
        assert_eq!(v2.as_deref(), Some("2"));
    }

    // ---- opt-in real-endpoint integration (run with `--ignored`) ----

    #[tokio::test]
    #[ignore = "needs AURATERM_IT_GH_TOKEN (a GitHub PAT with gist scope)"]
    async fn real_github_gist_roundtrip() {
        let Some(token) = skip_unless_env("AURATERM_IT_GH_TOKEN") else { return };
        let blob = encryption::encrypt_sync_blob(b"auraterm-real-github", "it-pass").unwrap();
        let (id, _v) = gist_push("https://api.github.com", &token, "", &STANDARD.encode(&blob), true)
            .await
            .expect("create gist");
        let remote = gist_pull("https://api.github.com", &token, &id, true)
            .await
            .expect("read gist");
        assert_eq!(
            encryption::decrypt_sync_blob(&remote.data, "it-pass").unwrap(),
            b"auraterm-real-github"
        );
        // cleanup
        let _ = http_client()
            .unwrap()
            .delete(format!("https://api.github.com/gists/{id}"))
            .bearer_auth(&token)
            .send()
            .await;
        eprintln!("OK real GitHub Gist round-trip (gist {id} deleted)");
    }

    #[tokio::test]
    #[ignore = "needs AURATERM_IT_GITEE_TOKEN (a Gitee private token with gists scope)"]
    async fn real_gitee_gist_roundtrip() {
        let Some(token) = skip_unless_env("AURATERM_IT_GITEE_TOKEN") else { return };
        let blob = encryption::encrypt_sync_blob(b"auraterm-real-gitee", "it-pass").unwrap();
        let (id, _v) = gist_push("https://gitee.com/api/v5", &token, "", &STANDARD.encode(&blob), false)
            .await
            .expect("create gitee gist");
        let remote = gist_pull("https://gitee.com/api/v5", &token, &id, false)
            .await
            .expect("read gitee gist");
        assert_eq!(
            encryption::decrypt_sync_blob(&remote.data, "it-pass").unwrap(),
            b"auraterm-real-gitee"
        );
        let _ = http_client()
            .unwrap()
            .delete(format!("https://gitee.com/api/v5/gists/{id}?access_token={token}"))
            .send()
            .await;
        eprintln!("OK real Gitee Gist round-trip (gist {id} deleted)");
    }

    #[tokio::test]
    #[ignore = "needs AURATERM_IT_WEBDAV_URL (+ optional _USER/_PASS)"]
    async fn real_webdav_roundtrip() {
        let Some(url) = skip_unless_env("AURATERM_IT_WEBDAV_URL") else { return };
        let cfg = WebdavProvider {
            url,
            username: std::env::var("AURATERM_IT_WEBDAV_USER").unwrap_or_default(),
            password: std::env::var("AURATERM_IT_WEBDAV_PASS").unwrap_or_default(),
        };
        let blob = encryption::encrypt_sync_blob(b"auraterm-real-webdav", "it-pass").unwrap();
        webdav_push(&cfg, &blob).await.expect("webdav put");
        let remote = webdav_pull(&cfg).await.expect("webdav get");
        assert_eq!(remote.data, blob);
        assert_eq!(
            encryption::decrypt_sync_blob(&remote.data, "it-pass").unwrap(),
            b"auraterm-real-webdav"
        );
        eprintln!("OK real WebDAV round-trip");
    }

    #[tokio::test]
    #[ignore = "needs AURATERM_IT_AURAXLAB_URL + AURATERM_IT_AURAXLAB_TOKEN (a live AuraXLab server)"]
    async fn real_auraxlab_roundtrip() {
        let Some(base_url) = skip_unless_env("AURATERM_IT_AURAXLAB_URL") else { return };
        let Some(token) = skip_unless_env("AURATERM_IT_AURAXLAB_TOKEN") else { return };
        let cfg = AuraxlabProvider { base_url, username: String::new(), token };

        let payload = br#"{"bookmarks":[{"id":"it-1","name":"it"}]}"#;
        let blob = encryption::encrypt_sync_blob(payload, "it-pass").unwrap();

        // base our write on the current server version (vault may not exist yet)
        let base_version = match auraxlab_pull(&cfg).await {
            Ok(remote) => remote.version,
            Err(_) => None,
        };
        let pushed = auraxlab_push(&cfg, &blob, base_version.as_deref(), "it-device", "ci")
            .await
            .expect("auraxlab push");
        assert!(pushed.is_some());

        let pulled = auraxlab_pull(&cfg).await.expect("auraxlab pull");
        assert_eq!(pulled.data, blob);
        assert_eq!(encryption::decrypt_sync_blob(&pulled.data, "it-pass").unwrap(), payload);

        // a stale write must conflict (409 -> "pull first")
        let stale = auraxlab_push(&cfg, &blob, Some("0"), "it-device", "ci").await;
        assert!(stale.is_err(), "stale push should 409");
        eprintln!("OK real AuraXLab round-trip + conflict against {}", cfg.base_url);
    }
}
