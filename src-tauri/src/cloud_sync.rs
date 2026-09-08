//! Configuration sync with the AuraXLab account (design
//! `docs/plans/sync-passphrase-removal-design.md`).
//!
//! Signing in to AuraXLab is all it takes: the scoped sync credential the
//! account center stores (`axsync_…`, in the device-encrypted `sync_config.enc`)
//! authenticates every request, and there is no second secret to type. Data is
//! split into two tiers by sensitivity:
//!
//! - **Tier 1 — bookmarks, a curated settings subset, SSH known-hosts.** Sent
//!   as plaintext JSON over TLS (`rest-v2` payload); the server encrypts it at
//!   rest under its own key and can read it. A password reset therefore leaves
//!   the vault usable.
//! - **Tier 2 — saved credentials (passwords / private keys).** Encrypted here,
//!   under a key derived from the *master password*
//!   ([`crate::encryption::encrypt_credentials_envelope`]), before they join the
//!   payload as an opaque `AURACRED` field. The server never opens it. Every
//!   device taking part in credential sync must use the same master password,
//!   and losing it loses the synced credentials — exactly like the local
//!   `credentials.enc`.
//!
//! ## What is synced
//!
//! - **Bookmarks** — saved connection metadata (`connections.json`). Always on.
//! - **Settings** — a device-independent subset (theme, fonts, quick buttons,
//!   output rules…). Window bounds, workspace/pane layout, serial history and
//!   the master-password hash are deliberately excluded. Optional.
//! - **Known hosts** — trusted SSH host-key fingerprints. Optional.
//! - **Credentials** — see tier 2. Off by default; needs master-password mode
//!   *and* the master password unlocked, otherwise that part is skipped and the
//!   result says why — the rest of the sync still runs.
//!
//! ## Conflict handling
//!
//! Bookmarks/credentials merge by id (the most recently uploaded copy wins on a
//! conflict); known-hosts union with **local entries winning** (sync must never
//! silently override a fingerprint trusted on this device). A `replace` pull is
//! offered for the "make this device authoritative" case. `cloud_sync_now`
//! performs a two-way sync (merge-pull, then push the merged result) with the
//! server's optimistic-concurrency version.
//!
//! Vaults uploaded by older builds (`e2e-v1`, encrypted under the removed sync
//! passphrase) are detected on pull and handed to the one-time migration in
//! `cloud_sync_legacy.rs`; they are never overwritten silently.

use crate::account::auraxlab_origin;
use crate::connections::{self, SavedConnection};
use crate::encryption::{self, CredentialStore, MasterPasswordState, StoredCredential};
use crate::settings;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use zeroize::Zeroizing;

/// Encrypted, device-local sync configuration (the account credential lives here).
const SYNC_CONFIG_FILE: &str = "sync_config.enc";
/// `rest-v2` payload schema (server validates `schema == 2`).
pub(crate) const PAYLOAD_SCHEMA: u32 = 2;

/// Stable error texts the frontend classifies (`classifySyncError` in
/// `src/cloudSync.ts`). Keep the wording in sync with that regex table.
pub(crate) const ERR_SIGN_IN: &str = "Sign in to your AuraXLab account again — the saved credential is no longer valid.";
pub(crate) const ERR_LEGACY_VAULT: &str = "The cloud copy still uses the old sync passphrase format; migrate it once from Sync settings.";
pub(crate) const ERR_NOT_SIGNED_IN: &str = "Sign in to your AuraXLab account first.";

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
    // Explicitly created bookmark folders travel with the bookmarks themselves.
    "bookmarkGroups",
    "autoOpenSftp",
    "zmodemDownloadPath",
    "restoreTabsOnStartup",
    // AI assistant config only — the API key lives outside settings.json
    // (encrypted under the device-local key) and never syncs.
    "aiConfig",
];

// ============================================================================
// Persistent configuration (encrypted at rest with the device-local key)
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AuraxlabProvider {
    /// Read-only compatibility field for one migration release. It is never
    /// serialized again and is never used as a network destination.
    #[serde(rename = "baseUrl", skip_serializing)]
    pub(crate) legacy_base_url: String,
    /// Test-only endpoint seam; persisted configs always deserialize to None.
    #[serde(skip)]
    pub(crate) endpoint_override: Option<String>,
    pub(crate) account_subject: String,
    pub(crate) email: String,
    pub(crate) username: String,
    pub(crate) token: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SyncConfig {
    /// "" | "auraxlab". Older configs may still say "github" / "gitee" /
    /// "webdav"; `load_config` clears those and raises the one-time notice.
    pub(crate) provider: String,
    pub(crate) include_settings: bool,
    pub(crate) include_known_hosts: bool,
    pub(crate) include_credentials: bool,
    pub(crate) auto_sync: bool,
    pub(crate) device_id: String,
    pub(crate) device_label: String,
    pub(crate) last_sync_at: Option<u64>,
    pub(crate) last_remote_version: Option<String>,
    /// Set once when a removed provider (Gist / WebDAV) was found in the stored
    /// config; the UI shows a notice until the user acknowledges it.
    pub(crate) legacy_provider_notice: bool,
    pub(crate) auraxlab: AuraxlabProvider,
}

fn sync_config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(SYNC_CONFIG_FILE))
}

pub(crate) fn load_config(app: &AppHandle) -> Result<SyncConfig, String> {
    let path = sync_config_path(app)?;
    if !path.exists() {
        return Ok(SyncConfig::default());
    }
    let encrypted = fs::read(&path).map_err(|e| format!("Failed to read sync config: {e}"))?;
    let key = encryption::load_or_create_local_key(app)?;
    let plaintext =
        Zeroizing::new(encryption::decrypt_data(&encrypted, &key).map_err(|_| "Sync config is corrupt or was written on another device".to_string())?);
    let mut config: SyncConfig = serde_json::from_slice(&plaintext).map_err(|e| format!("Failed to parse sync config: {e}"))?;
    let mut changed = migrate_legacy_auraxlab_config(&mut config);
    changed |= migrate_removed_providers(&mut config);
    if changed {
        save_config(app, &config)?;
    }
    Ok(config)
}

pub(crate) fn save_config(app: &AppHandle, config: &SyncConfig) -> Result<(), String> {
    let path = sync_config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let key = encryption::load_or_create_local_key(app)?;
    let plaintext = Zeroizing::new(serde_json::to_vec(config).map_err(|e| format!("Failed to serialize sync config: {e}"))?);
    let encrypted = encryption::encrypt_data(&plaintext, &key)?;
    fs::write(&path, &encrypted).map_err(|e| format!("Failed to write sync config: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn is_signed_in(config: &SyncConfig) -> bool {
    config.auraxlab.token.starts_with("axsync_")
}

/// GitHub Gist, Gitee Gist and WebDAV were removed (design §1.3). Their
/// fields are simply no longer part of `SyncConfig`, so the next save drops the
/// stored tokens; here the selection is cleared and the one-time notice raised.
/// A config that also holds an AuraXLab sign-in keeps syncing through it.
fn migrate_removed_providers(config: &mut SyncConfig) -> bool {
    match config.provider.as_str() {
        "github" | "gitee" | "webdav" => {
            config.provider = if is_signed_in(config) { "auraxlab".to_string() } else { String::new() };
            config.last_remote_version = None;
            config.legacy_provider_notice = true;
            true
        }
        _ => false,
    }
}

// ---- frontend-facing (redacted) views & inputs ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuraxlabView {
    username: String,
    email: String,
    token_set: bool,
}

/// Redacted configuration sent to the UI: the account credential is reduced
/// to a boolean so it never round-trips back through the frontend.
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
    /// "masterPassword" | "localKey" — credential sync needs the former.
    credentials_mode: &'static str,
    master_unlocked: bool,
    legacy_provider_notice: bool,
    auraxlab: AuraxlabView,
}

impl SyncConfigView {
    fn from_config(config: &SyncConfig, credentials_mode: &'static str, master_unlocked: bool) -> Self {
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
            credentials_mode,
            master_unlocked,
            legacy_provider_notice: config.legacy_provider_notice,
            auraxlab: AuraxlabView {
                username: config.auraxlab.username.clone(),
                email: config.auraxlab.email.clone(),
                token_set: is_signed_in(config),
            },
        }
    }
}

/// Editable configuration patch from the UI. The provider is no longer a
/// choice (signing in *is* the configuration) and there are no secrets left
/// to patch.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettingsInput {
    include_settings: bool,
    include_known_hosts: bool,
    include_credentials: bool,
    auto_sync: bool,
    device_label: String,
}

fn apply_input(config: &mut SyncConfig, input: SyncSettingsInput) {
    config.include_settings = input.include_settings;
    config.include_known_hosts = input.include_known_hosts;
    config.include_credentials = input.include_credentials;
    config.auto_sync = input.auto_sync;
    config.device_label = input.device_label;
}

/// Whether this device protects `credentials.enc` with a master password
/// (the only mode that can take part in credential sync).
fn master_password_mode(app: &AppHandle) -> bool {
    settings::get_settings(app.clone())
        .map(|s| s.master_password_hash.is_some())
        .unwrap_or(false)
}

fn credentials_mode_label(app: &AppHandle) -> &'static str {
    if master_password_mode(app) {
        "masterPassword"
    } else {
        "localKey"
    }
}

// ============================================================================
// The rest-v2 payload
// ============================================================================

/// Tier-2 field inside the payload: base64 of an `AURACRED` envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CredentialsEnvelope {
    pub(crate) format: String,
    pub(crate) blob: String,
}

/// What travels to AuraXLab as the `payload` text. Tier-1 fields are plain;
/// `credentials` is the opaque tier-2 envelope. Absent `credentials` means
/// "this upload carries none" — never "delete them".
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncPayload {
    pub(crate) schema: u32,
    pub(crate) exported_at: u64,
    pub(crate) device_id: String,
    pub(crate) device_label: String,
    #[serde(default)]
    pub(crate) bookmarks: Vec<SavedConnection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) settings: Option<Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) known_hosts: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credentials: Option<CredentialsEnvelope>,
}

/// Why the credentials part of a sync did not happen. Stable codes for the UI.
pub(crate) mod skip {
    /// Device uses the device-local key, not a master password.
    pub const LOCAL_KEY_MODE: &str = "localKeyMode";
    /// Master-password mode, but locked right now.
    pub const MASTER_LOCKED: &str = "masterLocked";
    /// Envelope was sealed under a different master password.
    pub const MISMATCH: &str = "mismatch";
    /// Envelope format unknown or data corrupt.
    pub const CORRUPT: &str = "corrupt";
}

/// Outcome of a push / pull / two-way sync, surfaced to the UI.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub(crate) pushed: bool,
    pub(crate) pulled: bool,
    pub(crate) bookmarks_total: usize,
    pub(crate) bookmarks_added: usize,
    pub(crate) known_hosts_added: usize,
    pub(crate) credentials_synced: usize,
    /// One of the [`skip`] codes when credential sync was requested but did
    /// not run; `None` when it ran or was not requested.
    pub(crate) credentials_skipped: Option<String>,
    pub(crate) settings_applied: bool,
    pub(crate) remote_version: Option<String>,
    pub(crate) message: String,
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

/// Seal the local credential store into a tier-2 envelope, or say why not.
fn seal_credentials(app: &AppHandle, master_state: &MasterPasswordState) -> Result<Result<CredentialsEnvelope, &'static str>, String> {
    if !master_password_mode(app) {
        return Ok(Err(skip::LOCAL_KEY_MODE));
    }
    if !master_state.is_unlocked() {
        return Ok(Err(skip::MASTER_LOCKED));
    }
    let password = master_state.get()?;
    let secret = encryption::resolve_secret(app, master_state)?;
    let store = encryption::load_encrypted_credentials(app, &secret)?;
    let plaintext = Zeroizing::new(serde_json::to_vec(&store.credentials).map_err(|e| e.to_string())?);
    let blob = encryption::encrypt_credentials_envelope(&plaintext, &password)?;
    Ok(Ok(CredentialsEnvelope {
        format: encryption::CRED_ENVELOPE_FORMAT.to_string(),
        blob: STANDARD.encode(blob),
    }))
}

/// Assemble the current device state into a payload, honoring the include
/// flags. The second value is the credentials skip reason, if any.
async fn build_payload(app: &AppHandle, master_state: &MasterPasswordState, config: &SyncConfig) -> Result<(SyncPayload, Option<String>), String> {
    let mut payload = SyncPayload {
        schema: PAYLOAD_SCHEMA,
        exported_at: now_ms(),
        device_id: config.device_id.clone(),
        device_label: config.device_label.clone(),
        bookmarks: connections::load_connections(app)?,
        ..Default::default()
    };

    if config.include_settings {
        payload.settings = Some(extract_settings_subset(app)?);
    }

    if config.include_known_hosts {
        payload.known_hosts = crate::ssh::export_known_hosts(app).await?;
    }

    let mut skipped = None;
    if config.include_credentials {
        match seal_credentials(app, master_state)? {
            Ok(envelope) => payload.credentials = Some(envelope),
            Err(reason) => skipped = Some(reason.to_string()),
        }
    }

    Ok((payload, skipped))
}

/// Merge tier-1 data (bookmarks, settings, known-hosts) into local state.
/// `replace` makes the remote bookmarks authoritative; otherwise entries union.
pub(crate) async fn apply_tier1(
    app: &AppHandle,
    config: &SyncConfig,
    bookmarks: Vec<SavedConnection>,
    settings_subset: Option<&Value>,
    known_hosts: HashMap<String, String>,
    replace: bool,
    result: &mut SyncResult,
) -> Result<(), String> {
    let local = connections::load_connections(app)?;
    let merged = merge_bookmarks(local, bookmarks, replace);
    result.bookmarks_added = merged.added;
    result.bookmarks_total = merged.items.len();
    connections::write_connections(app, &merged.items)?;

    if let Some(subset) = settings_subset {
        if config.include_settings {
            apply_settings_subset(app, subset)?;
            result.settings_applied = true;
        }
    }

    // Union, local wins: sync never overrides a fingerprint trusted here.
    if !known_hosts.is_empty() && config.include_known_hosts {
        result.known_hosts_added = crate::ssh::import_known_hosts(app, known_hosts).await?;
    }
    result.pulled = true;
    Ok(())
}

/// Merge decrypted credentials into the local store by connection id.
pub(crate) fn merge_plain_credentials(app: &AppHandle, master_state: &MasterPasswordState, incoming: Vec<StoredCredential>) -> Result<usize, String> {
    if incoming.is_empty() {
        return Ok(0);
    }
    let secret = encryption::resolve_secret(app, master_state)?;
    let mut store = encryption::load_encrypted_credentials(app, &secret).unwrap_or_else(|_| CredentialStore { credentials: Vec::new() });
    let mut synced = 0usize;
    for credential in incoming {
        store.credentials.retain(|c| c.connection_id != credential.connection_id);
        store.credentials.push(credential);
        synced += 1;
    }
    encryption::save_encrypted_credentials(app, &store, &secret)?;
    Ok(synced)
}

/// Open the tier-2 envelope and merge it, or record why it was skipped. A
/// skipped or unreadable envelope never fails the tier-1 merge (design §9).
fn apply_credentials_envelope(app: &AppHandle, master_state: &MasterPasswordState, envelope: &CredentialsEnvelope, result: &mut SyncResult) -> Result<(), String> {
    if envelope.format != encryption::CRED_ENVELOPE_FORMAT {
        result.credentials_skipped = Some(skip::CORRUPT.to_string());
        return Ok(());
    }
    if !master_password_mode(app) {
        result.credentials_skipped = Some(skip::LOCAL_KEY_MODE.to_string());
        return Ok(());
    }
    if !master_state.is_unlocked() {
        result.credentials_skipped = Some(skip::MASTER_LOCKED.to_string());
        return Ok(());
    }
    let Ok(blob) = STANDARD.decode(envelope.blob.trim()) else {
        result.credentials_skipped = Some(skip::CORRUPT.to_string());
        return Ok(());
    };
    let password = master_state.get()?;
    let plaintext = match encryption::decrypt_credentials_envelope(&blob, &password) {
        Ok(plaintext) => Zeroizing::new(plaintext),
        Err(error) => {
            result.credentials_skipped = Some(if error.contains("different master password") { skip::MISMATCH } else { skip::CORRUPT }.to_string());
            return Ok(());
        }
    };
    let incoming: Vec<StoredCredential> = match serde_json::from_slice(&plaintext) {
        Ok(list) => list,
        Err(_) => {
            result.credentials_skipped = Some(skip::CORRUPT.to_string());
            return Ok(());
        }
    };
    result.credentials_synced = merge_plain_credentials(app, master_state, incoming)?;
    Ok(())
}

/// Merge a downloaded payload into local state.
async fn apply_payload(
    app: &AppHandle,
    master_state: &MasterPasswordState,
    config: &SyncConfig,
    payload: SyncPayload,
    replace: bool,
) -> Result<SyncResult, String> {
    let mut result = SyncResult::default();
    apply_tier1(app, config, payload.bookmarks, payload.settings.as_ref(), payload.known_hosts, replace, &mut result).await?;
    if config.include_credentials {
        if let Some(envelope) = &payload.credentials {
            apply_credentials_envelope(app, master_state, envelope, &mut result)?;
        }
    }
    Ok(result)
}

struct MergeOutcome {
    items: Vec<SavedConnection>,
    added: usize,
}

/// Union by id; the incoming (remote) copy wins on a conflict. With `replace`,
/// the remote set becomes authoritative wholesale.
fn merge_bookmarks(local: Vec<SavedConnection>, remote: Vec<SavedConnection>, replace: bool) -> MergeOutcome {
    if replace {
        let added = remote.len();
        return MergeOutcome { items: remote, added };
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

fn parse_payload(text: &str) -> Result<SyncPayload, String> {
    let payload: SyncPayload = serde_json::from_str(text).map_err(|e| format!("Corrupt sync payload: {e}"))?;
    if payload.schema != PAYLOAD_SCHEMA {
        return Err(format!("Unsupported sync payload schema {} (upgrade AuraTerm)", payload.schema));
    }
    Ok(payload)
}

// ============================================================================
// HTTP plumbing & the AuraXLab vault API
// ============================================================================

pub(crate) fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("AuraTerm-Sync/2.0")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// What the server holds for this account.
#[derive(Debug)]
pub(crate) enum RemoteContent {
    /// `rest-v2`: the plaintext JSON payload.
    Payload(String),
    /// `e2e-v1`: ciphertext under the old sync passphrase (migration only).
    LegacyBlob(Vec<u8>),
}

#[derive(Debug)]
pub(crate) struct RemoteVault {
    pub(crate) content: RemoteContent,
    pub(crate) version: Option<String>,
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

fn version_string(body: &Value) -> Option<String> {
    body.get("version").map(|v| v.to_string().trim_matches('"').to_string())
}

fn normalize_base_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn is_official_auraxlab_url(raw: &str) -> bool {
    let normalized = normalize_base_url(raw);
    normalized.is_empty() || normalized == "https://auraxlab.com"
}

/// One-release migration for encrypted configs written before the official
/// AuraXLab origin became fixed. Third-party hosts and generic legacy tokens
/// are quarantined locally and are never contacted.
fn migrate_legacy_auraxlab_config(config: &mut SyncConfig) -> bool {
    let legacy = config.auraxlab.legacy_base_url.clone();
    let mut changed = !legacy.is_empty();
    if !is_official_auraxlab_url(&legacy) || (!config.auraxlab.token.is_empty() && !config.auraxlab.token.starts_with("axsync_")) {
        config.auraxlab.token.clear();
        config.auraxlab.account_subject.clear();
        config.auraxlab.email.clear();
        config.auraxlab.username.clear();
        config.last_remote_version = None;
        changed = true;
    } else if config.auraxlab.email.is_empty() && config.auraxlab.username.contains('@') {
        // The old username cache actually held the login email.
        config.auraxlab.email = config.auraxlab.username.clone();
        changed = true;
    }
    config.auraxlab.legacy_base_url.clear();
    changed
}

impl AuraxlabProvider {
    fn endpoint(&self) -> String {
        #[cfg(test)]
        if let Some(endpoint) = &self.endpoint_override {
            return normalize_base_url(endpoint);
        }
        auraxlab_origin()
    }
}

fn auraxlab_vault_url(cfg: &AuraxlabProvider) -> String {
    format!("{}/api/v1/auraterm/sync/vault", cfg.endpoint())
}

/// `PUT` a `rest-v2` payload. Returns the new server version.
pub(crate) async fn auraxlab_push(cfg: &AuraxlabProvider, payload_text: &str, base_version: Option<&str>, device_id: &str, device_label: &str) -> Result<Option<String>, String> {
    let client = http_client()?;
    let body = json!({
        "format": "v2",
        "payload": payload_text,
        "baseVersion": base_version.and_then(|v| v.parse::<i64>().ok()),
        "deviceId": device_id,
        "deviceLabel": device_label,
    });
    let resp = client
        .put(auraxlab_vault_url(cfg))
        .basic_auth(&cfg.token, Some(""))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    match status {
        StatusCode::UNAUTHORIZED => Err(ERR_SIGN_IN.to_string()),
        StatusCode::CONFLICT => Err("The server has newer data than this device. Pull first, then push again.".to_string()),
        s if !s.is_success() => Err(format!("AuraXLab sync failed: {}", json_message(&body, s))),
        _ => Ok(version_string(&body)),
    }
}

/// `GET` the vault. `Ok(None)` means the account has no synced data yet.
pub(crate) async fn auraxlab_pull(cfg: &AuraxlabProvider) -> Result<Option<RemoteVault>, String> {
    let client = http_client()?;
    let resp = client
        .get(auraxlab_vault_url(cfg))
        .basic_auth(&cfg.token, Some(""))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    match status {
        StatusCode::NOT_FOUND => return Ok(None),
        StatusCode::UNAUTHORIZED => return Err(ERR_SIGN_IN.to_string()),
        StatusCode::SERVICE_UNAVAILABLE => {
            return Err("The server cannot read the synced data right now (at-rest key problem); contact the server administrator.".to_string())
        }
        s if !s.is_success() => return Err(format!("AuraXLab download failed: {}", json_message(&body, s))),
        _ => {}
    }
    let version = version_string(&body);
    if let Some(payload) = body.get("payload").and_then(|v| v.as_str()) {
        return Ok(Some(RemoteVault { content: RemoteContent::Payload(payload.to_string()), version }));
    }
    let content = body
        .get("blob")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Server response did not contain synced data.".to_string())?;
    let data = STANDARD.decode(content.trim()).map_err(|e| format!("Synced data is not valid base64: {e}"))?;
    if !encryption::is_legacy_sync_blob(&data) {
        return Err("Server response did not contain AuraTerm sync data.".to_string());
    }
    Ok(Some(RemoteVault { content: RemoteContent::LegacyBlob(data), version }))
}

pub(crate) fn ensure_signed_in(config: &SyncConfig) -> Result<(), String> {
    if !is_signed_in(config) {
        return Err(ERR_NOT_SIGNED_IN.to_string());
    }
    Ok(())
}

fn ensure_device_id(config: &mut SyncConfig) {
    if config.device_id.is_empty() {
        config.device_id = uuid::Uuid::new_v4().to_string();
    }
}

/// Build the current device state and push it as `rest-v2`, based on
/// `base_version` (or the config's last known version). Updates the config.
pub(crate) async fn push_current_state(
    app: &AppHandle,
    master_state: &MasterPasswordState,
    config: &mut SyncConfig,
    base_version: Option<String>,
    result: &mut SyncResult,
) -> Result<(), String> {
    ensure_device_id(config);
    let (payload, skipped) = build_payload(app, master_state, config).await?;
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let base = base_version.or_else(|| config.last_remote_version.clone());
    let version = auraxlab_push(&config.auraxlab, &text, base.as_deref(), &config.device_id, &config.device_label).await?;
    config.last_sync_at = Some(now_ms());
    if version.is_some() {
        config.last_remote_version = version.clone();
    }
    save_config(app, config)?;
    result.pushed = true;
    result.bookmarks_total = payload.bookmarks.len();
    result.remote_version = version;
    if result.credentials_skipped.is_none() {
        result.credentials_skipped = skipped;
    }
    Ok(())
}

// ============================================================================
// Tauri commands
// ============================================================================

#[tauri::command]
pub fn get_sync_config(app: AppHandle, master_state: State<'_, MasterPasswordState>) -> Result<SyncConfigView, String> {
    let config = load_config(&app)?;
    Ok(SyncConfigView::from_config(&config, credentials_mode_label(&app), master_state.is_unlocked()))
}

#[tauri::command]
pub fn set_sync_config(app: AppHandle, input: SyncSettingsInput, master_state: State<'_, MasterPasswordState>) -> Result<SyncConfigView, String> {
    let mut config = load_config(&app)?;
    apply_input(&mut config, input);
    ensure_device_id(&mut config);
    if config.device_label.trim().is_empty() {
        config.device_label = format!("device-{}", &config.device_id[..8.min(config.device_id.len())]);
    }
    save_config(&app, &config)?;
    Ok(SyncConfigView::from_config(&config, credentials_mode_label(&app), master_state.is_unlocked()))
}

/// The user has read the "Gist / WebDAV sync was removed" notice.
#[tauri::command]
pub fn acknowledge_legacy_provider_notice(app: AppHandle) -> Result<(), String> {
    let mut config = load_config(&app)?;
    if config.legacy_provider_notice {
        config.legacy_provider_notice = false;
        save_config(&app, &config)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn cloud_sync_push(app: AppHandle, master_state: State<'_, MasterPasswordState>) -> Result<SyncResult, String> {
    let mut config = load_config(&app)?;
    ensure_signed_in(&config)?;
    let mut result = SyncResult::default();
    push_current_state(&app, &master_state, &mut config, None, &mut result).await?;
    result.message = "Uploaded to your AuraXLab account.".to_string();
    Ok(result)
}

#[tauri::command]
pub async fn cloud_sync_pull(app: AppHandle, replace: bool, master_state: State<'_, MasterPasswordState>) -> Result<SyncResult, String> {
    let mut config = load_config(&app)?;
    ensure_signed_in(&config)?;

    let Some(remote) = auraxlab_pull(&config.auraxlab).await? else {
        return Err("Your AuraXLab account has no synced data yet.".to_string());
    };
    let payload = match remote.content {
        RemoteContent::Payload(text) => parse_payload(&text)?,
        RemoteContent::LegacyBlob(_) => return Err(ERR_LEGACY_VAULT.to_string()),
    };

    let mut result = apply_payload(&app, &master_state, &config, payload, replace).await?;
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
pub async fn cloud_sync_now(app: AppHandle, master_state: State<'_, MasterPasswordState>) -> Result<SyncResult, String> {
    let mut config = load_config(&app)?;
    ensure_signed_in(&config)?;

    // 1) Pull & merge. Only "nothing uploaded yet" proceeds straight to the
    //    push; every other failure — including a legacy vault — stops here so
    //    the cloud copy is never overwritten by mistake.
    let mut result = SyncResult::default();
    let mut base_version = None;
    match auraxlab_pull(&config.auraxlab).await? {
        Some(remote) => {
            let payload = match remote.content {
                RemoteContent::Payload(text) => parse_payload(&text)?,
                RemoteContent::LegacyBlob(_) => return Err(ERR_LEGACY_VAULT.to_string()),
            };
            result = apply_payload(&app, &master_state, &config, payload, false).await?;
            base_version = remote.version;
        }
        None => result.message = "(first sync) ".to_string(),
    }

    // 2) Push the merged result back.
    push_current_state(&app, &master_state, &mut config, base_version, &mut result).await?;
    result.message.push_str("Two-way sync complete.");
    Ok(result)
}

#[tauri::command]
pub async fn cloud_sync_test_connection(app: AppHandle) -> Result<String, String> {
    let config = load_config(&app)?;
    ensure_signed_in(&config)?;
    match auraxlab_pull(&config.auraxlab).await? {
        Some(RemoteVault { content: RemoteContent::Payload(_), .. }) => Ok("Connected — found existing sync data.".to_string()),
        Some(RemoteVault { content: RemoteContent::LegacyBlob(_), .. }) => Ok("Connected — the cloud copy needs a one-time migration.".to_string()),
        None => Ok("Connected — no data uploaded yet.".to_string()),
    }
}

/// Step 1 of sign-up: ask the server to email a verification code to `email`.
#[tauri::command]
pub async fn auraxlab_request_email_code(email: String) -> Result<String, String> {
    auraxlab_request_email_code_at(&auraxlab_origin(), email).await
}

async fn auraxlab_request_email_code_at(base_url: &str, email: String) -> Result<String, String> {
    let client = http_client()?;
    let url = format!("{}/api/v1/auraterm/sync/email/request-code", base_url);
    let resp = client
        .post(url)
        .json(&json!({ "email": email }))
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
        .unwrap_or("We emailed you a verification code.")
        .to_string())
}

/// Step 2 of sign-up: verify the emailed code for `email`.
#[tauri::command]
pub async fn auraxlab_verify_email_code(email: String, code: String) -> Result<String, String> {
    auraxlab_verify_email_code_at(&auraxlab_origin(), email, code).await
}

async fn auraxlab_verify_email_code_at(base_url: &str, email: String, code: String) -> Result<String, String> {
    let client = http_client()?;
    let url = format!("{}/api/v1/auraterm/sync/email/verify-code", base_url);
    let resp = client
        .post(url)
        .json(&json!({ "email": email, "code": code }))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    if !status.is_success() {
        return Err(json_message(&body, status));
    }
    Ok(body.get("message").and_then(|v| v.as_str()).unwrap_or("Email verified.").to_string())
}

/// Step 3 of sign-up: create the account. The email must already be verified
/// (steps 1-2); the server creates an already-confirmed account that can sign
/// in immediately.
#[tauri::command]
pub async fn auraxlab_register(email: String, username: String, password: String) -> Result<String, String> {
    let client = http_client()?;
    let url = format!("{}/api/v1/auraterm/sync/register", auraxlab_origin());
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

/// Decrypted backend-only view of the scoped account credential.
#[derive(Clone)]
pub(crate) struct LocalSyncAccount {
    pub(crate) subject: String,
    pub(crate) email: String,
    pub(crate) username: String,
    pub(crate) token: String,
}

pub(crate) fn local_sync_account(app: &AppHandle) -> Result<Option<LocalSyncAccount>, String> {
    let config = load_config(app)?;
    if !is_signed_in(&config) {
        return Ok(None);
    }
    Ok(Some(LocalSyncAccount {
        subject: config.auraxlab.account_subject,
        email: config.auraxlab.email,
        username: config.auraxlab.username,
        token: config.auraxlab.token,
    }))
}

pub(crate) fn store_account_login(app: &AppHandle, subject: &str, email: &str, username: &str, token: &str, device_label: &str) -> Result<(), String> {
    if subject.trim().is_empty() || !token.starts_with("axsync_") {
        return Err("AuraXLab returned an invalid account credential.".to_string());
    }
    let mut config = load_config(app)?;
    config.provider = "auraxlab".to_string();
    config.auraxlab.account_subject = subject.to_string();
    config.auraxlab.email = email.to_string();
    config.auraxlab.username = username.to_string();
    config.auraxlab.token = token.to_string();
    config.device_label = device_label.to_string();
    ensure_device_id(&mut config);
    save_config(app, &config)
}

pub(crate) fn clear_account_login(app: &AppHandle) -> Result<(), String> {
    let mut config = load_config(app)?;
    config.provider.clear();
    config.auraxlab.account_subject.clear();
    config.auraxlab.email.clear();
    config.auraxlab.username.clear();
    config.auraxlab.token.clear();
    config.last_remote_version = None;
    save_config(app, &config)
}

pub(crate) async fn revoke_sync_credential(token: &str) {
    if !token.starts_with("axsync_") {
        return;
    }
    if let Ok(client) = http_client() {
        let _ = client
            .delete(format!("{}/api/v1/auraterm/sync/credentials/current", auraxlab_origin()))
            .basic_auth(token, Some(""))
            .send()
            .await;
    }
}

/// Cloud Console traffic totals for the signed-in account, as reported by
/// `GET /api/v1/auraterm/account` (`traffic` object; absent on older servers).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTraffic {
    pub(crate) bytes_up: u64,
    pub(crate) bytes_down: u64,
    pub(crate) bytes_total: u64,
    pub(crate) sessions: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOverview {
    pub(crate) subject: String,
    pub(crate) username: String,
    pub(crate) email: String,
    pub(crate) confirmed: bool,
    pub(crate) traffic: Option<AccountTraffic>,
}

/// Fetch the signed-in account's profile and Cloud Console traffic totals
/// for the "My Account" view. Requires an AuraXLab sign-in; the stored
/// credential never leaves the backend.
pub(crate) async fn fetch_account_overview(app: &AppHandle) -> Result<AccountOverview, String> {
    let config = load_config(app)?;
    if config.auraxlab.token.is_empty() {
        return Err(ERR_NOT_SIGNED_IN.to_string());
    }
    let credential = config.auraxlab.token.clone();
    let client = http_client()?;
    let url = format!("{}/api/v1/auraterm/account", auraxlab_origin());
    let resp = client
        .get(url)
        .basic_auth(&credential, Some(""))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body = parse_json(&bytes);
    if !status.is_success() {
        return Err(json_message(&body, status));
    }
    let traffic = body.get("traffic").filter(|v| v.is_object()).map(|v| {
        let count = |key: &str| v.get(key).and_then(|n| n.as_u64()).unwrap_or(0);
        AccountTraffic {
            bytes_up: count("bytes_up"),
            bytes_down: count("bytes_down"),
            bytes_total: count("bytes_total"),
            sessions: count("sessions"),
        }
    });
    let overview = AccountOverview {
        subject: body
            .get("subject")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "AuraXLab did not return an account subject.".to_string())?
            .to_string(),
        username: body.get("username").and_then(|v| v.as_str()).unwrap_or(&config.auraxlab.username).to_string(),
        email: body.get("email").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        confirmed: body.get("confirmed").and_then(|v| v.as_bool()).unwrap_or(false),
        traffic,
    };
    if config.auraxlab.account_subject != overview.subject || config.auraxlab.email != overview.email || config.auraxlab.username != overview.username {
        // The request may finish after the user signs out or switches
        // accounts. Never write the stale snapshot back over newer state.
        let mut current = load_config(app)?;
        if current.auraxlab.token != credential {
            return Err("AuraXLab account changed while refreshing.".to_string());
        }
        current.auraxlab.account_subject = overview.subject.clone();
        current.auraxlab.email = overview.email.clone();
        current.auraxlab.username = overview.username.clone();
        save_config(app, &current)?;
    }
    Ok(overview)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    pub(crate) fn bookmark(id: &str, name: &str) -> SavedConnection {
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
            adopt_server_params: None,
            serial_auto_reconnect: None,
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
            origin: None,
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
    fn payload_serde_roundtrip_omits_empty_optionals() {
        let payload = SyncPayload {
            schema: PAYLOAD_SCHEMA,
            exported_at: 123,
            device_id: "dev".to_string(),
            device_label: "label".to_string(),
            bookmarks: vec![bookmark("a", "a")],
            ..Default::default()
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["schema"], 2);
        assert!(json.get("settings").is_none(), "empty settings omitted");
        assert!(json.get("knownHosts").is_none(), "empty knownHosts omitted");
        assert!(json.get("credentials").is_none(), "absent credentials omitted, never null");
        let back = parse_payload(&json.to_string()).unwrap();
        assert_eq!(back.bookmarks.len(), 1);
        assert!(back.credentials.is_none());
    }

    #[test]
    fn payload_rejects_other_schemas() {
        let err = parse_payload(r#"{"schema":1,"exportedAt":0,"deviceId":"d","deviceLabel":"l"}"#).unwrap_err();
        assert!(err.contains("schema 1"), "got: {err}");
        assert!(parse_payload("{nope").is_err());
    }

    #[test]
    fn payload_carries_the_credentials_envelope_opaquely() {
        let mut payload = SyncPayload { schema: PAYLOAD_SCHEMA, ..Default::default() };
        payload.credentials = Some(CredentialsEnvelope { format: encryption::CRED_ENVELOPE_FORMAT.into(), blob: "QUJD".into() });
        let text = serde_json::to_string(&payload).unwrap();
        assert!(text.contains(r#""credentials":{"format":"AURACRED/1","blob":"QUJD"}"#), "got: {text}");
        let back = parse_payload(&text).unwrap();
        assert_eq!(back.credentials.unwrap().blob, "QUJD");
    }

    #[test]
    fn config_view_redacts_the_account_credential() {
        let mut config = SyncConfig::default();
        config.provider = "auraxlab".to_string();
        config.auraxlab.token = "axsync_secret".to_string();
        config.auraxlab.username = "alice".to_string();
        let view = SyncConfigView::from_config(&config, "masterPassword", true);
        assert!(view.auraxlab.token_set);
        assert_eq!(view.credentials_mode, "masterPassword");
        assert!(view.master_unlocked);
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("axsync_secret"));
        assert!(json.contains(r#""credentialsMode":"masterPassword""#));
        assert!(json.contains(r#""legacyProviderNotice":false"#));
    }

    #[test]
    fn input_patch_touches_only_the_editable_fields() {
        let mut config = SyncConfig::default();
        config.provider = "auraxlab".into();
        config.auraxlab.token = "axsync_keep".into();
        config.device_id = "dev-1".into();
        apply_input(
            &mut config,
            SyncSettingsInput {
                include_settings: true,
                include_known_hosts: false,
                include_credentials: true,
                auto_sync: true,
                device_label: "laptop".to_string(),
            },
        );
        assert_eq!(config.auraxlab.token, "axsync_keep");
        assert_eq!(config.provider, "auraxlab");
        assert_eq!(config.device_id, "dev-1");
        assert!(config.include_credentials && config.auto_sync && !config.include_known_hosts);
        assert_eq!(config.device_label, "laptop");
    }

    #[test]
    fn legacy_auraxlab_config_quarantines_nonofficial_and_generic_tokens() {
        let mut config = SyncConfig::default();
        config.auraxlab.legacy_base_url = "https://other.example".into();
        config.auraxlab.token = "axsync_secret".into();
        assert!(migrate_legacy_auraxlab_config(&mut config));
        assert!(config.auraxlab.token.is_empty());

        config.auraxlab.legacy_base_url = "https://auraxlab.com".into();
        config.auraxlab.token = "legacy-token".into();
        assert!(migrate_legacy_auraxlab_config(&mut config));
        assert!(config.auraxlab.token.is_empty());

        config.auraxlab.legacy_base_url = "https://auraxlab.com/".into();
        config.auraxlab.token = "axsync_keep".into();
        assert!(migrate_legacy_auraxlab_config(&mut config));
        assert_eq!(config.auraxlab.token, "axsync_keep");
        assert!(config.auraxlab.legacy_base_url.is_empty());
    }

    #[test]
    fn removed_providers_are_cleared_with_a_one_time_notice() {
        // A config written by an older build still carries Gist fields; they
        // are ignored on read and dropped on the next save.
        let stored = json!({
            "provider": "github",
            "github": {"token": "ghp_secret", "gistId": "abc"},
            "lastRemoteVersion": "2026-01-01T00:00:00Z",
            "auraxlab": {"token": "", "username": ""}
        });
        let mut config: SyncConfig = serde_json::from_value(stored).unwrap();
        assert!(migrate_removed_providers(&mut config));
        assert_eq!(config.provider, "");
        assert!(config.legacy_provider_notice);
        assert!(config.last_remote_version.is_none());
        assert!(!serde_json::to_string(&config).unwrap().contains("ghp_secret"));

        // Signed in to AuraXLab as well: keep syncing through the account.
        let mut config = SyncConfig::default();
        config.provider = "webdav".into();
        config.auraxlab.token = "axsync_x".into();
        assert!(migrate_removed_providers(&mut config));
        assert_eq!(config.provider, "auraxlab");

        let mut config = SyncConfig::default();
        config.provider = "auraxlab".into();
        assert!(!migrate_removed_providers(&mut config));
        assert!(!config.legacy_provider_notice);
    }

    #[test]
    fn signed_in_requires_a_scoped_credential() {
        let mut config = SyncConfig::default();
        assert_eq!(ensure_signed_in(&config).unwrap_err(), ERR_NOT_SIGNED_IN);
        config.auraxlab.token = "legacy".into();
        assert!(ensure_signed_in(&config).is_err());
        config.auraxlab.token = "axsync_ok".into();
        assert!(ensure_signed_in(&config).is_ok());
    }

    // ========================================================================
    // AuraXLab vault API integration tests
    //
    // These exercise the REAL HTTP client code (reqwest + JSON handling +
    // response parsing) against an in-process mock that emulates the server
    // contract from AuraXLab `app/api/sync.py` (Phase 0). The opt-in
    // `#[ignore]`d test at the end hits a live server when env vars are set.
    // ========================================================================

    /// Spawn a tiny in-process HTTP server; the handler maps
    /// (method, url, body) -> (status, body, headers). Returns the base URL.
    /// The server thread runs until the test process exits.
    pub(crate) fn spawn_mock<F>(handler: F) -> String
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
        })
        ;
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

    /// Stored vault: (format, content, version). Emulates the Phase 0 server:
    /// `PUT {format: "v2", payload}` stores rest-v2 and hands the payload back
    /// on GET; a legacy `PUT {blob}` stores e2e-v1 and hands the blob back.
    pub(crate) type MockVault = Arc<Mutex<(Option<(String, String)>, i64)>>;

    pub(crate) fn spawn_vault_mock(store: MockVault) -> String {
        spawn_mock(move |method, url, body| {
            if !url.contains("/auraterm/sync/vault") {
                return (404, b"{}".to_vec(), vec![]);
            }
            let mut g = store.lock().unwrap();
            match method {
                "PUT" => {
                    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                    let base_version = v.get("baseVersion").and_then(|x| x.as_i64());
                    if let Some(bv) = base_version {
                        if bv != g.1 {
                            let resp = json!({"error": "conflict", "version": g.1});
                            return (409, serde_json::to_vec(&resp).unwrap(), vec![]);
                        }
                    }
                    let stored = if v.get("format").and_then(|f| f.as_str()) == Some("v2") {
                        let payload = v.get("payload").and_then(|x| x.as_str()).unwrap_or_default();
                        if serde_json::from_str::<Value>(payload).ok().and_then(|p| p.get("schema").and_then(|s| s.as_u64())) != Some(2) {
                            return (400, br#"{"message":"payload.schema must be 2"}"#.to_vec(), vec![]);
                        }
                        ("rest-v2".to_string(), payload.to_string())
                    } else {
                        ("e2e-v1".to_string(), v.get("blob").and_then(|x| x.as_str()).unwrap_or_default().to_string())
                    };
                    g.0 = Some(stored);
                    g.1 += 1;
                    let resp = json!({"version": g.1, "format": g.0.as_ref().unwrap().0});
                    (200, serde_json::to_vec(&resp).unwrap(), vec![])
                }
                "GET" => match &g.0 {
                    Some((format, content)) if format == "rest-v2" => {
                        (200, serde_json::to_vec(&json!({"format": format, "payload": content, "version": g.1})).unwrap(), vec![])
                    }
                    Some((format, content)) => (200, serde_json::to_vec(&json!({"format": format, "blob": content, "version": g.1})).unwrap(), vec![]),
                    None => (404, b"{}".to_vec(), vec![]),
                },
                _ => (405, b"{}".to_vec(), vec![]),
            }
        })
    }

    pub(crate) fn mock_provider(base: String) -> AuraxlabProvider {
        AuraxlabProvider {
            endpoint_override: Some(base),
            username: "u".into(),
            token: "axsync_tok".into(),
            ..Default::default()
        }
    }

    fn payload_text(marker: &str) -> String {
        json!({"schema": 2, "exportedAt": 1, "deviceId": "dev", "deviceLabel": "l", "bookmarks": [{"id": marker}]}).to_string()
    }

    #[tokio::test]
    async fn integ_auraxlab_v2_roundtrip_and_conflict() {
        let store: MockVault = Arc::new(Mutex::new((None, 0)));
        let cfg = mock_provider(spawn_vault_mock(store));

        assert!(auraxlab_pull(&cfg).await.unwrap().is_none(), "empty account pulls as None");

        let v1 = auraxlab_push(&cfg, &payload_text("one"), None, "dev", "label").await.unwrap();
        assert_eq!(v1.as_deref(), Some("1"));

        let pulled = auraxlab_pull(&cfg).await.unwrap().expect("vault exists");
        assert_eq!(pulled.version.as_deref(), Some("1"));
        match pulled.content {
            RemoteContent::Payload(text) => assert!(text.contains("\"one\""), "got: {text}"),
            RemoteContent::LegacyBlob(_) => panic!("rest-v2 must come back as a payload"),
        }

        // stale push (baseVersion 0, server is at 1) -> 409 conflict
        let err = auraxlab_push(&cfg, &payload_text("two"), Some("0"), "dev", "label").await.unwrap_err();
        assert!(err.to_lowercase().contains("pull"), "got: {err}");

        // correct push (baseVersion 1) -> v2
        let v2 = auraxlab_push(&cfg, &payload_text("two"), Some("1"), "dev", "label").await.unwrap();
        assert_eq!(v2.as_deref(), Some("2"));
    }

    #[tokio::test]
    async fn integ_auraxlab_legacy_vault_is_detected_not_parsed() {
        let blob = encryption::encrypt_sync_blob(b"{\"bookmarks\":[]}", "old-pass").unwrap();
        let store: MockVault = Arc::new(Mutex::new((Some(("e2e-v1".into(), STANDARD.encode(&blob))), 3)));
        let cfg = mock_provider(spawn_vault_mock(store));
        let pulled = auraxlab_pull(&cfg).await.unwrap().expect("vault exists");
        assert_eq!(pulled.version.as_deref(), Some("3"));
        match pulled.content {
            RemoteContent::LegacyBlob(data) => assert_eq!(data, blob),
            RemoteContent::Payload(_) => panic!("legacy blob must be flagged"),
        }
    }

    #[tokio::test]
    async fn integ_auraxlab_401_asks_to_sign_in_again() {
        let base = spawn_mock(|_, _, _| (401, br#"{"error":"unauthorized","message":"please sign in again"}"#.to_vec(), vec![]));
        let cfg = mock_provider(base);
        assert_eq!(auraxlab_pull(&cfg).await.unwrap_err(), ERR_SIGN_IN);
        assert_eq!(auraxlab_push(&cfg, &payload_text("x"), None, "d", "l").await.unwrap_err(), ERR_SIGN_IN);
    }

    #[tokio::test]
    async fn integ_auraxlab_503_names_the_server_side_cause() {
        let base = spawn_mock(|_, _, _| (503, br#"{"error":"vault unreadable"}"#.to_vec(), vec![]));
        let err = auraxlab_pull(&mock_provider(base)).await.unwrap_err();
        assert!(err.contains("server administrator"), "got: {err}");
    }

    #[tokio::test]
    async fn integ_auraxlab_email_verification_flow() {
        let base = spawn_mock(|method, url, body| {
            if method == "POST" && url.ends_with("/email/request-code") {
                let resp = json!({"status": "sent", "message": "We emailed you a verification code."});
                (200, serde_json::to_vec(&resp).unwrap(), vec![])
            } else if method == "POST" && url.ends_with("/email/verify-code") {
                let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                if v.get("code").and_then(|c| c.as_str()) == Some("123456") {
                    let resp = json!({"status": "verified", "message": "Email verified."});
                    (200, serde_json::to_vec(&resp).unwrap(), vec![])
                } else {
                    let resp = json!({"error": "bad request", "message": "Incorrect verification code"});
                    (400, serde_json::to_vec(&resp).unwrap(), vec![])
                }
            } else {
                (404, b"{}".to_vec(), vec![])
            }
        });

        let sent = auraxlab_request_email_code_at(&base, "a@b.com".into()).await.unwrap();
        assert!(sent.to_lowercase().contains("emailed"), "got: {sent}");

        let verified = auraxlab_verify_email_code_at(&base, "a@b.com".into(), "123456".into()).await.unwrap();
        assert!(verified.to_lowercase().contains("verified"), "got: {verified}");

        let err = auraxlab_verify_email_code_at(&base, "a@b.com".into(), "000000".into()).await.unwrap_err();
        assert!(err.to_lowercase().contains("incorrect"), "got: {err}");
    }

    // ---- opt-in real-endpoint integration (run with `--ignored`) ----

    #[tokio::test]
    #[ignore = "needs AURATERM_IT_AURAXLAB_URL + AURATERM_IT_AURAXLAB_TOKEN (a live AuraXLab server with Phase 0)"]
    async fn real_auraxlab_v2_roundtrip() {
        let Some(base_url) = skip_unless_env("AURATERM_IT_AURAXLAB_URL") else {
            return;
        };
        let Some(token) = skip_unless_env("AURATERM_IT_AURAXLAB_TOKEN") else {
            return;
        };
        let cfg = AuraxlabProvider {
            endpoint_override: Some(base_url.clone()),
            token,
            ..Default::default()
        };

        // base our write on the current server version (vault may not exist yet)
        let base_version = auraxlab_pull(&cfg).await.expect("auraxlab pull").and_then(|remote| remote.version);
        let pushed = auraxlab_push(&cfg, &payload_text("it-1"), base_version.as_deref(), "it-device", "ci")
            .await
            .expect("auraxlab push");
        assert!(pushed.is_some());

        let pulled = auraxlab_pull(&cfg).await.expect("auraxlab pull").expect("vault exists");
        match pulled.content {
            RemoteContent::Payload(text) => assert!(text.contains("it-1")),
            RemoteContent::LegacyBlob(_) => panic!("server should hand back the v2 payload"),
        }

        // a stale write must conflict (409 -> "pull first")
        let stale = auraxlab_push(&cfg, &payload_text("it-2"), Some("0"), "it-device", "ci").await;
        assert!(stale.is_err(), "stale push should 409");
        eprintln!("OK real AuraXLab v2 round-trip + conflict against {base_url}");
    }
}
