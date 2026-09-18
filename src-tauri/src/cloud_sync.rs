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
//! ## Merge rules (design `docs/plans/sync-merge-rules-design.md`)
//!
//! The device keeps a **base** ([`SyncBase`], `sync_base.enc`): what it and the
//! cloud copy agreed on at the end of its last sync. A two-way sync compares
//! the local state and the cloud copy against it ([`crate::sync_merge`]), so an
//! edit or delete made here is not mistaken for stale data and rolled back,
//! deletes travel in both directions, and only a different change to the same
//! entry on both sides is a conflict — the cloud copy wins it and the result
//! names it.
//!
//! - **Lineage.** "Missing from the cloud copy" only means "deleted there" if
//!   that copy evolved from the base. Every upload carries the `lineage` id of
//!   the copy it was merged from; a first upload, a legacy migration and a
//!   push-only that did not start from the current cloud copy begin a new one,
//!   and builds older than this field drop it. A cloud copy of another lineage
//!   is merged the old way: union, cloud copy wins by id, nothing is deleted.
//! - **Mass-delete guard.** A sync that would carry [`MASS_DELETE_THRESHOLD`]
//!   or more deletes in one direction holds them back until a manual run
//!   confirms them; a local bookmark list that is suddenly empty is restored
//!   from the cloud copy instead.
//! - **Nothing is dropped, nothing is re-sent.** The vault is replaced
//!   wholesale on a push, so sections this device does not upload — settings
//!   or known-hosts switched off, the credentials envelope while the master
//!   password is locked — are carried over from the cloud copy; and when the
//!   upload would not change the cloud copy at all, it is not sent.
//!
//! Vaults uploaded by older builds (`e2e-v1`, encrypted under the removed sync
//! passphrase) are detected on pull and handed to the one-time migration in
//! `cloud_sync_legacy.rs`; they are never overwritten silently.

use crate::account::auraxlab_origin;
use crate::connections::{self, SavedConnection};
use crate::encryption::{self, CredentialStore, MasterPasswordState, StoredCredential};
use crate::settings;
use crate::sync_merge::{merge_settings, three_way, Held};

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
/// Encrypted, device-local [`SyncBase`].
const SYNC_BASE_FILE: &str = "sync_base.enc";
/// A sync that would carry this many deletes in one direction holds them back
/// until a manual run confirms them.
pub(crate) const MASS_DELETE_THRESHOLD: usize = 9;
/// `rest-v2` payload schema (server validates `schema == 2`).
pub(crate) const PAYLOAD_SCHEMA: u32 = 2;

/// Stable error texts the frontend classifies (`classifySyncError` in
/// `src/cloudSync.ts`). Keep the wording in sync with that regex table.
pub(crate) const ERR_SIGN_IN: &str = "Sign in to your AuraXLab account again — the saved credential is no longer valid.";
pub(crate) const ERR_LEGACY_VAULT: &str = "The cloud copy still uses the old sync passphrase format; migrate it once from Sync settings.";
pub(crate) const ERR_NOT_SIGNED_IN: &str = "Sign in to your AuraXLab account first.";
/// Another device uploaded since the copy this push was based on (HTTP 409).
pub(crate) const ERR_CONFLICT: &str = "The server has newer data than this device. Pull first, then push again.";

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
    let plaintext = Zeroizing::new(serde_json::to_vec(config).map_err(|e| format!("Failed to serialize sync config: {e}"))?);
    write_device_file(app, &sync_config_path(app)?, &plaintext).map_err(|e| format!("Failed to write sync config: {e}"))
}

/// Encrypt `plaintext` under the device-local key and write it, owner-only.
fn write_device_file(app: &AppHandle, path: &std::path::Path, plaintext: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let key = encryption::load_or_create_local_key(app)?;
    let encrypted = encryption::encrypt_data(plaintext, &key)?;
    fs::write(path, &encrypted).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn sync_base_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(SYNC_BASE_FILE))
}

/// A base that is missing or unreadable is simply no base: the next sync
/// merges the old way and writes a fresh one.
fn load_base(app: &AppHandle) -> Option<SyncBase> {
    let encrypted = fs::read(sync_base_path(app).ok()?).ok()?;
    let key = encryption::load_or_create_local_key(app).ok()?;
    let plaintext = Zeroizing::new(encryption::decrypt_data(&encrypted, &key).ok()?);
    serde_json::from_slice(&plaintext).ok()
}

fn save_base(app: &AppHandle, base: &SyncBase) -> Result<(), String> {
    let plaintext = Zeroizing::new(serde_json::to_vec(base).map_err(|e| format!("Failed to serialize sync base: {e}"))?);
    write_device_file(app, &sync_base_path(app)?, &plaintext).map_err(|e| format!("Failed to write sync base: {e}"))
}

/// The base describes one account's cloud copy; it goes when the sign-in does.
fn forget_base(app: &AppHandle) {
    if let Ok(path) = sync_base_path(app) {
        let _ = fs::remove_file(path);
    }
}

fn is_signed_in(config: &SyncConfig) -> bool {
    config.auraxlab.token.starts_with("axsync_")
}

/// Forget which cloud version this device last saw (sign-out, provider change).
fn forget_remote(config: &mut SyncConfig) {
    config.last_remote_version = None;
}

/// GitHub Gist, Gitee Gist and WebDAV were removed (design §1.3). Their
/// fields are simply no longer part of `SyncConfig`, so the next save drops the
/// stored tokens; here the selection is cleared and the one-time notice raised.
/// A config that also holds an AuraXLab sign-in keeps syncing through it.
fn migrate_removed_providers(config: &mut SyncConfig) -> bool {
    match config.provider.as_str() {
        "github" | "gitee" | "webdav" => {
            config.provider = if is_signed_in(config) { "auraxlab".to_string() } else { String::new() };
            forget_remote(config);
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncPayload {
    pub(crate) schema: u32,
    pub(crate) exported_at: u64,
    pub(crate) device_id: String,
    pub(crate) device_label: String,
    /// Identifies the chain of uploads this copy belongs to: every upload
    /// keeps the lineage of the copy it was merged from. Builds that predate
    /// the field drop it, which marks their upload as unrelated — see the
    /// module docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lineage: Option<String>,
    #[serde(default)]
    pub(crate) bookmarks: Vec<SavedConnection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) settings: Option<Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) known_hosts: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credentials: Option<CredentialsEnvelope>,
}

/// What this device and the cloud copy agreed on at the end of its last sync —
/// the common ancestor the next three-way merge compares both sides with.
/// A section that took no part keeps its older snapshot: any common ancestor
/// is a valid base, only a less precise one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SyncBase {
    /// Lineage of the cloud copy this snapshot belongs to. A cloud copy of
    /// another lineage did not evolve from it, so nothing may be inferred.
    pub(crate) lineage: Option<String>,
    /// The cloud version the snapshot mirrors.
    pub(crate) version: Option<String>,
    pub(crate) bookmarks: Vec<SavedConnection>,
    pub(crate) settings: Option<Value>,
    /// The envelope as last merged. It stays sealed under the master password,
    /// so the snapshot never holds credentials the device-local key can open.
    pub(crate) credentials: Option<CredentialsEnvelope>,
}

/// What a finished run amounted to. The UI words its result from this and the
/// counts; `SyncResult::message` says the same in English, for logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncOutcome {
    #[default]
    Synced,
    FirstSync,
    UpToDate,
    Uploaded,
    Merged,
    Replaced,
    Migrated,
    MigrationOverwrote,
    AlreadyMigrated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    Bookmark,
    Credentials,
    Setting,
}

/// One thing both sides changed differently. The cloud copy won it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub(crate) kind: ConflictKind,
    /// The bookmark's name, or the settings key.
    pub(crate) name: String,
    /// The element, for a list-valued setting merged item by item.
    pub(crate) item: Option<String>,
}

impl SyncConflict {
    fn new(kind: ConflictKind, name: impl Into<String>) -> Self {
        Self { kind, name: name.into(), item: None }
    }

    fn label(&self) -> String {
        match (self.kind, &self.item) {
            (ConflictKind::Bookmark, _) => self.name.clone(),
            (ConflictKind::Credentials, _) => format!("{} (credentials)", self.name),
            (ConflictKind::Setting, None) => format!("setting {}", self.name),
            (ConflictKind::Setting, Some(item)) => format!("setting {}: {item}", self.name),
        }
    }
}

/// Deletes a sync held back for confirmation (the mass-delete guard).
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldDeletes {
    /// Bookmarks deleted here, not yet dropped from the cloud copy.
    pub(crate) local: usize,
    /// Bookmarks gone from the cloud copy, not yet removed here.
    pub(crate) remote: usize,
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
    /// Existing bookmarks whose content the merge actually changed.
    pub(crate) bookmarks_updated: usize,
    /// Bookmarks removed here because the cloud copy dropped them.
    pub(crate) bookmarks_removed: usize,
    pub(crate) outcome: SyncOutcome,
    /// What both sides changed differently; the cloud copy won each of them.
    pub(crate) conflicts: Vec<SyncConflict>,
    /// Set when deletes were held back; a run with `confirm_deletes` applies them.
    pub(crate) deletes_held: Option<HeldDeletes>,
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

/// Returns whether any synced key actually changed; identical settings are
/// not rewritten.
fn apply_settings_subset(app: &AppHandle, subset: &Value) -> Result<bool, String> {
    let Value::Object(incoming) = subset else {
        return Ok(false);
    };
    let current = settings::get_settings(app.clone())?;
    let mut value = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let mut changed = false;
    if let Value::Object(map) = &mut value {
        for key in SYNCED_SETTINGS_KEYS {
            if let Some(v) = incoming.get(*key) {
                if map.get(*key) != Some(v) {
                    map.insert((*key).to_string(), v.clone());
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return Ok(false);
    }
    let merged: settings::Settings = serde_json::from_value(value).map_err(|e| e.to_string())?;
    settings::save_settings(app.clone(), merged)?;
    Ok(true)
}

// ============================================================================
// Device-local state
// ============================================================================

/// The device-local state a sync reads and writes. The app backs it with its
/// config directory ([`AppStore`]); tests drive the same flow with an
/// in-memory device, so two "devices" can sync through one mock vault.
pub(crate) trait SyncStore {
    fn bookmarks(&self) -> Result<Vec<SavedConnection>, String>;
    fn write_bookmarks(&self, items: &[SavedConnection]) -> Result<(), String>;
    fn settings_subset(&self) -> Result<Value, String>;
    /// Returns whether any synced key actually changed.
    fn apply_settings_subset(&self, subset: &Value) -> Result<bool, String>;
    async fn known_hosts(&self) -> Result<HashMap<String, String>, String>;
    /// Union, local wins. Returns how many entries were new.
    async fn import_known_hosts(&self, hosts: HashMap<String, String>) -> Result<usize, String>;
    /// The master password when the credential store can take part in the
    /// tier-2 envelope, otherwise the [`skip`] code saying why not.
    fn envelope_key(&self) -> Result<Result<Zeroizing<String>, &'static str>, String>;
    fn credentials(&self) -> Result<Vec<StoredCredential>, String>;
    fn write_credentials(&self, items: Vec<StoredCredential>) -> Result<(), String>;
    fn save_config(&self, config: &SyncConfig) -> Result<(), String>;
    /// `None` when there is no usable base; never an error.
    fn base(&self) -> Option<SyncBase>;
    fn save_base(&self, base: &SyncBase) -> Result<(), String>;
}

/// [`SyncStore`] over the app's config directory.
pub(crate) struct AppStore<'a> {
    app: &'a AppHandle,
    master_state: &'a MasterPasswordState,
}

impl<'a> AppStore<'a> {
    pub(crate) fn new(app: &'a AppHandle, master_state: &'a MasterPasswordState) -> Self {
        Self { app, master_state }
    }
}

impl SyncStore for AppStore<'_> {
    fn bookmarks(&self) -> Result<Vec<SavedConnection>, String> {
        connections::load_connections(self.app)
    }

    fn write_bookmarks(&self, items: &[SavedConnection]) -> Result<(), String> {
        connections::write_connections(self.app, items)
    }

    fn settings_subset(&self) -> Result<Value, String> {
        extract_settings_subset(self.app)
    }

    fn apply_settings_subset(&self, subset: &Value) -> Result<bool, String> {
        apply_settings_subset(self.app, subset)
    }

    async fn known_hosts(&self) -> Result<HashMap<String, String>, String> {
        crate::ssh::export_known_hosts(self.app).await
    }

    async fn import_known_hosts(&self, hosts: HashMap<String, String>) -> Result<usize, String> {
        crate::ssh::import_known_hosts(self.app, hosts).await
    }

    fn envelope_key(&self) -> Result<Result<Zeroizing<String>, &'static str>, String> {
        if !master_password_mode(self.app) {
            return Ok(Err(skip::LOCAL_KEY_MODE));
        }
        if !self.master_state.is_unlocked() {
            return Ok(Err(skip::MASTER_LOCKED));
        }
        Ok(Ok(self.master_state.get()?))
    }

    fn credentials(&self) -> Result<Vec<StoredCredential>, String> {
        let secret = encryption::resolve_secret(self.app, self.master_state)?;
        let mut store = encryption::load_encrypted_credentials(self.app, &secret)?;
        // `CredentialStore` is ZeroizeOnDrop, so the list cannot be moved out.
        Ok(std::mem::take(&mut store.credentials))
    }

    fn write_credentials(&self, items: Vec<StoredCredential>) -> Result<(), String> {
        let secret = encryption::resolve_secret(self.app, self.master_state)?;
        encryption::save_encrypted_credentials(self.app, &CredentialStore { credentials: items }, &secret)
    }

    fn save_config(&self, config: &SyncConfig) -> Result<(), String> {
        save_config(self.app, config)
    }

    fn base(&self) -> Option<SyncBase> {
        load_base(self.app)
    }

    fn save_base(&self, base: &SyncBase) -> Result<(), String> {
        save_base(self.app, base)
    }
}

/// Seal `credentials` into a tier-2 envelope under the master password.
fn seal_envelope(credentials: &[StoredCredential], password: &str) -> Result<CredentialsEnvelope, String> {
    let plaintext = Zeroizing::new(serde_json::to_vec(credentials).map_err(|e| e.to_string())?);
    let blob = encryption::encrypt_credentials_envelope(&plaintext, password)?;
    Ok(CredentialsEnvelope {
        format: encryption::CRED_ENVELOPE_FORMAT.to_string(),
        blob: STANDARD.encode(blob),
    })
}

/// Seal the local credential store into a tier-2 envelope, or say why not.
fn seal_credentials(store: &impl SyncStore) -> Result<Result<CredentialsEnvelope, &'static str>, String> {
    let password = match store.envelope_key()? {
        Ok(password) => password,
        Err(reason) => return Ok(Err(reason)),
    };
    Ok(Ok(seal_envelope(&store.credentials()?, &password)?))
}

/// Assemble this device's tier-1 state into a payload, honoring the include
/// flags. The credentials envelope is the caller's to add.
async fn build_tier1_payload(store: &impl SyncStore, config: &SyncConfig) -> Result<SyncPayload, String> {
    let mut payload = SyncPayload {
        schema: PAYLOAD_SCHEMA,
        exported_at: now_ms(),
        device_id: config.device_id.clone(),
        device_label: config.device_label.clone(),
        bookmarks: store.bookmarks()?,
        ..Default::default()
    };

    if config.include_settings {
        payload.settings = Some(store.settings_subset()?);
    }

    if config.include_known_hosts {
        payload.known_hosts = store.known_hosts().await?;
    }
    Ok(payload)
}

fn new_lineage() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Fill in what this upload would otherwise drop from the cloud copy: whole
/// sections this device does not upload, and settings keys it does not know
/// (a newer build syncs more of them). The vault is replaced wholesale on
/// every push, so anything left out is gone for the other devices.
fn carry_over(outgoing: &mut SyncPayload, remote: &SyncPayload, config: &SyncConfig) {
    match (&mut outgoing.settings, &remote.settings) {
        (None, Some(theirs)) => outgoing.settings = Some(theirs.clone()),
        (Some(Value::Object(ours)), Some(Value::Object(theirs))) => {
            for (key, value) in theirs {
                if !ours.contains_key(key) {
                    ours.insert(key.clone(), value.clone());
                }
            }
        }
        _ => {}
    }
    if !config.include_known_hosts {
        outgoing.known_hosts = remote.known_hosts.clone();
    }
    if outgoing.credentials.is_none() {
        outgoing.credentials = remote.credentials.clone();
    }
}

/// Whether uploading `outgoing` would change the tier-1 content of the cloud
/// copy. Bookmark order is not synced, and the export stamp and device fields
/// differ on every upload by design, so none of those count. The lineage does:
/// a cloud copy without one (an older build uploaded it) has to be re-stamped
/// before anything can be inferred from it.
fn tier1_differs(outgoing: &SyncPayload, remote: &SyncPayload) -> bool {
    fn by_id(items: &[SavedConnection]) -> Option<HashMap<&str, Value>> {
        let map: HashMap<&str, Value> = items.iter().map(|c| Some((c.id.as_str(), serde_json::to_value(c).ok()?))).collect::<Option<_>>()?;
        (map.len() == items.len()).then_some(map)
    }
    match (by_id(&outgoing.bookmarks), by_id(&remote.bookmarks)) {
        (Some(ours), Some(theirs)) if ours == theirs => {}
        _ => return true,
    }
    outgoing.lineage != remote.lineage || outgoing.settings != remote.settings || outgoing.known_hosts != remote.known_hosts
}

/// Whether two credential lists hold different entries, in any order.
fn credentials_differ(ours: &[StoredCredential], theirs: &[StoredCredential]) -> bool {
    ours.len() != theirs.len() || ours.iter().any(|c| !theirs.contains(c))
}

/// Merge tier-1 data (bookmarks, settings, known-hosts) into local state.
/// `replace` makes the remote bookmarks authoritative; otherwise entries union.
pub(crate) async fn apply_tier1(
    store: &impl SyncStore,
    config: &SyncConfig,
    bookmarks: Vec<SavedConnection>,
    settings_subset: Option<&Value>,
    known_hosts: HashMap<String, String>,
    replace: bool,
    result: &mut SyncResult,
) -> Result<(), String> {
    let local = store.bookmarks()?;
    let merged = merge_bookmarks(local, bookmarks, replace);
    result.bookmarks_added = merged.added;
    result.bookmarks_updated = merged.updated;
    result.bookmarks_total = merged.items.len();
    if replace || merged.added > 0 || merged.updated > 0 {
        store.write_bookmarks(&merged.items)?;
    }

    if let Some(subset) = settings_subset {
        if config.include_settings {
            result.settings_applied = store.apply_settings_subset(subset)?;
        }
    }

    // Union, local wins: sync never overrides a fingerprint trusted here.
    if !known_hosts.is_empty() && config.include_known_hosts {
        result.known_hosts_added = store.import_known_hosts(known_hosts).await?;
    }
    result.pulled = true;
    Ok(())
}

/// Merge decrypted credentials into the local store by connection id.
pub(crate) fn merge_plain_credentials(store: &impl SyncStore, incoming: Vec<StoredCredential>) -> Result<usize, String> {
    if incoming.is_empty() {
        return Ok(0);
    }
    let mut local = store.credentials().unwrap_or_default();
    let changed = merge_credentials(&mut local, incoming);
    if changed > 0 {
        store.write_credentials(local)?;
    }
    Ok(changed)
}

/// Incoming wins by connection id. Returns how many entries were new or
/// actually different, so a repeat sync of identical data reports zero.
fn merge_credentials(local: &mut Vec<StoredCredential>, incoming: Vec<StoredCredential>) -> usize {
    let mut changed = 0usize;
    for credential in incoming {
        match local.iter().position(|c| c.connection_id == credential.connection_id) {
            Some(pos) if local[pos] == credential => {}
            Some(pos) => {
                local[pos] = credential;
                changed += 1;
            }
            None => {
                local.push(credential);
                changed += 1;
            }
        }
    }
    changed
}

/// Decrypt a tier-2 envelope, or say with a [`skip`] code why it cannot be read.
fn open_envelope(envelope: &CredentialsEnvelope, password: &str) -> Result<Vec<StoredCredential>, &'static str> {
    if envelope.format != encryption::CRED_ENVELOPE_FORMAT {
        return Err(skip::CORRUPT);
    }
    let blob = STANDARD.decode(envelope.blob.trim()).map_err(|_| skip::CORRUPT)?;
    let plaintext = match encryption::decrypt_credentials_envelope(&blob, password) {
        Ok(plaintext) => Zeroizing::new(plaintext),
        Err(error) if error.contains("different master password") => return Err(skip::MISMATCH),
        Err(_) => return Err(skip::CORRUPT),
    };
    serde_json::from_slice(&plaintext).map_err(|_| skip::CORRUPT)
}

/// Open the tier-2 envelope and merge it, or record why it was skipped. A
/// skipped or unreadable envelope never fails the tier-1 merge (design §9).
fn apply_credentials_envelope(store: &impl SyncStore, envelope: &CredentialsEnvelope, result: &mut SyncResult) -> Result<(), String> {
    if envelope.format != encryption::CRED_ENVELOPE_FORMAT {
        result.credentials_skipped = Some(skip::CORRUPT.to_string());
        return Ok(());
    }
    let password = match store.envelope_key()? {
        Ok(password) => password,
        Err(reason) => {
            result.credentials_skipped = Some(reason.to_string());
            return Ok(());
        }
    };
    match open_envelope(envelope, &password) {
        Ok(incoming) => result.credentials_synced = merge_plain_credentials(store, incoming)?,
        Err(reason) => result.credentials_skipped = Some(reason.to_string()),
    }
    Ok(())
}

/// What the credentials half of a sync puts into the upload.
struct CredentialsPlan {
    /// Freshly sealed, or the cloud envelope carried over untouched.
    envelope: Option<CredentialsEnvelope>,
    /// Whether that differs from what the cloud copy holds.
    changed: bool,
    /// Whether the envelope took part, so it can serve as the next base.
    merged: bool,
}

/// Three-way merge the cloud envelope with the local store and decide what the
/// upload carries. Whenever this device cannot seal — credential sync off,
/// device-local key, master password locked — the cloud envelope is carried
/// over as is, so the push does not wipe it for the other devices.
fn reconcile_credentials(
    store: &impl SyncStore,
    config: &SyncConfig,
    remote: Option<&CredentialsEnvelope>,
    base: Option<&CredentialsEnvelope>,
    bookmarks: &BookmarkChanges,
    result: &mut SyncResult,
) -> Result<CredentialsPlan, String> {
    let carried = CredentialsPlan { envelope: remote.cloned(), changed: false, merged: false };
    if !config.include_credentials {
        return Ok(carried);
    }
    let password = match store.envelope_key()? {
        Ok(password) => password,
        Err(reason) => {
            result.credentials_skipped = Some(reason.to_string());
            return Ok(carried);
        }
    };
    let ours = store.credentials()?;
    let theirs = match remote.map(|envelope| open_envelope(envelope, &password)) {
        None => Vec::new(),
        Some(Ok(list)) => list,
        Some(Err(reason)) => {
            // An envelope this device cannot read is replaced by its own, as
            // before: a changed master password must be able to move on.
            result.credentials_skipped = Some(reason.to_string());
            return Ok(CredentialsPlan { envelope: Some(seal_envelope(&ours, &password)?), changed: true, merged: true });
        }
    };
    // The base is usually the very envelope the cloud copy still holds.
    let before = match (base, remote) {
        (Some(base), Some(remote)) if base.blob == remote.blob => Some(theirs.clone()),
        (Some(base), _) => open_envelope(base, &password).ok(),
        (None, _) => None,
    };

    let mut merged = three_way(before.as_deref(), ours.clone(), theirs.clone(), &bookmarks.held);
    // A bookmark this sync deleted takes its credential along, on both sides.
    merged.local.retain(|c| !bookmarks.removed.contains(&c.connection_id));
    merged.upload.retain(|c| !bookmarks.removed.contains(&c.connection_id) && !bookmarks.dropped.contains(&c.connection_id));
    for id in &merged.conflicts {
        result.conflicts.push(SyncConflict::new(ConflictKind::Credentials, bookmarks.names.get(id).unwrap_or(id)));
    }

    let changed_here = merged.local.iter().filter(|c| !ours.contains(c)).count() + ours.iter().filter(|c| !merged.local.iter().any(|m| m.connection_id == c.connection_id)).count();
    let changed = credentials_differ(&merged.upload, &theirs);
    let envelope = if changed { Some(seal_envelope(&merged.upload, &password)?) } else { remote.cloned() };
    if changed_here > 0 {
        result.credentials_synced += changed_here;
        store.write_credentials(merged.local)?;
    }
    Ok(CredentialsPlan { envelope, changed, merged: true })
}

/// What the bookmark merge decided, for the sections that follow it.
struct BookmarkChanges {
    /// The new cloud state (differs from the local one only by held deletes).
    upload: Vec<SavedConnection>,
    /// `upload`, plus the base copies of held remote deletes — so the next
    /// sync sees them as "gone from the cloud copy" again, not as new here.
    base: Vec<SavedConnection>,
    held: Held,
    /// Ids removed here / dropped from the upload by this merge.
    removed: Vec<String>,
    dropped: Vec<String>,
    names: HashMap<String, String>,
}

/// Three-way merge the cloud copy's bookmarks into the local list.
fn reconcile_bookmarks(
    store: &impl SyncStore,
    theirs: &[SavedConnection],
    base: Option<&[SavedConnection]>,
    confirm_deletes: bool,
    result: &mut SyncResult,
) -> Result<BookmarkChanges, String> {
    let local = store.bookmarks()?;
    // An empty list where the base had entries is a lost `connections.json`,
    // not a wish to delete everything: merge without a base, which restores.
    let lost = |before: &[SavedConnection]| local.is_empty() && !before.is_empty();
    let base = base.filter(|before| !lost(before));

    let mut held = Held::default();
    let mut merged = three_way(base, local.clone(), theirs.to_vec(), &held);
    result.deletes_held = None; // a retried round decides again
    if !confirm_deletes {
        if merged.removed.len() >= MASS_DELETE_THRESHOLD {
            held.remote_deletes = merged.removed.iter().cloned().collect();
        }
        if merged.dropped.len() >= MASS_DELETE_THRESHOLD {
            held.local_deletes = merged.dropped.iter().cloned().collect();
        }
        if !held.is_empty() {
            result.deletes_held = Some(HeldDeletes { local: held.local_deletes.len(), remote: held.remote_deletes.len() });
            merged = three_way(base, local.clone(), theirs.to_vec(), &held);
        }
    }

    if serde_json::to_value(&local).ok() != serde_json::to_value(&merged.local).ok() {
        store.write_bookmarks(&merged.local)?;
    }
    let names: HashMap<String, String> = merged.local.iter().map(|c| (c.id.clone(), c.name.clone())).collect();
    result.bookmarks_added += merged.added;
    result.bookmarks_updated += merged.updated;
    result.bookmarks_removed += merged.removed.len();
    result.conflicts.extend(merged.conflicts.iter().map(|id| SyncConflict::new(ConflictKind::Bookmark, names.get(id).unwrap_or(id))));

    let mut next_base = merged.upload.clone();
    next_base.extend(base.unwrap_or_default().iter().filter(|c| held.remote_deletes.contains(&c.id)).cloned());
    Ok(BookmarkChanges { upload: merged.upload, base: next_base, held, removed: merged.removed, dropped: merged.dropped, names })
}

/// What merging the cloud copy decided for the upload and the next base.
struct Reconciled {
    bookmarks: BookmarkChanges,
    credentials: CredentialsPlan,
}

/// Merge the cloud copy into local state, section by section. `base` is the
/// snapshot to compare with, or `None` when the cloud copy is not known to
/// have evolved from it (then: union, cloud copy wins, no deletes).
async fn reconcile(
    store: &impl SyncStore,
    config: &SyncConfig,
    theirs: &SyncPayload,
    base: Option<&SyncBase>,
    confirm_deletes: bool,
    result: &mut SyncResult,
) -> Result<Reconciled, String> {
    let bookmarks = reconcile_bookmarks(store, &theirs.bookmarks, base.map(|b| b.bookmarks.as_slice()), confirm_deletes, result)?;

    if let (true, Some(remote)) = (config.include_settings, &theirs.settings) {
        let merged = merge_settings(base.and_then(|b| b.settings.as_ref()), &store.settings_subset()?, remote);
        result.conflicts.extend(merged.conflicts.into_iter().map(|c| SyncConflict { kind: ConflictKind::Setting, name: c.key, item: c.item }));
        if !merged.apply.is_empty() {
            result.settings_applied |= store.apply_settings_subset(&Value::Object(merged.apply))?;
        }
    }

    // Union, local wins: sync never overrides a fingerprint trusted here.
    if !theirs.known_hosts.is_empty() && config.include_known_hosts {
        result.known_hosts_added += store.import_known_hosts(theirs.known_hosts.clone()).await?;
    }

    let credentials = reconcile_credentials(store, config, theirs.credentials.as_ref(), base.and_then(|b| b.credentials.as_ref()), &bookmarks, result)?;
    result.pulled = true;
    Ok(Reconciled { bookmarks, credentials })
}

/// Merge a downloaded payload into local state.
async fn apply_payload(store: &impl SyncStore, config: &SyncConfig, payload: SyncPayload, replace: bool) -> Result<SyncResult, String> {
    let mut result = SyncResult::default();
    apply_tier1(store, config, payload.bookmarks, payload.settings.as_ref(), payload.known_hosts, replace, &mut result).await?;
    if config.include_credentials {
        if let Some(envelope) = &payload.credentials {
            apply_credentials_envelope(store, envelope, &mut result)?;
        }
    }
    Ok(result)
}

struct MergeOutcome {
    items: Vec<SavedConnection>,
    added: usize,
    /// Existing entries the incoming copy actually changed.
    updated: usize,
}

/// Union by id; the incoming (remote) copy wins on a conflict. With `replace`,
/// the remote set becomes authoritative wholesale.
fn merge_bookmarks(local: Vec<SavedConnection>, remote: Vec<SavedConnection>, replace: bool) -> MergeOutcome {
    if replace {
        let added = remote.len();
        return MergeOutcome { items: remote, added, updated: 0 };
    }
    let mut items = local;
    let mut added = 0usize;
    let mut updated = 0usize;
    for incoming in remote {
        if let Some(pos) = items.iter().position(|c| c.id == incoming.id) {
            if serde_json::to_value(&items[pos]).ok() != serde_json::to_value(&incoming).ok() {
                updated += 1;
            }
            items[pos] = incoming;
        } else {
            items.push(incoming);
            added += 1;
        }
    }
    MergeOutcome { items, added, updated }
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
        forget_remote(config);
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
        StatusCode::CONFLICT => Err(ERR_CONFLICT.to_string()),
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

/// A parsed `rest-v2` cloud copy and the version it was served at.
pub(crate) struct RemoteCopy {
    payload: SyncPayload,
    version: Option<String>,
}

impl RemoteCopy {
    /// The base, if this copy is known to have evolved from it.
    fn related<'a>(&self, base: Option<&'a SyncBase>) -> Option<&'a SyncBase> {
        base.filter(|base| base.lineage.is_some() && base.lineage == self.payload.lineage)
    }
}

/// `PUT` the payload on top of `base_version` (or the last known version) and
/// note the new cloud version. The caller saves the config.
async fn upload(config: &mut SyncConfig, payload: &SyncPayload, base_version: Option<String>, result: &mut SyncResult) -> Result<(), String> {
    let text = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    let base = base_version.or_else(|| config.last_remote_version.clone());
    let version = auraxlab_push(&config.auraxlab, &text, base.as_deref(), &config.device_id, &config.device_label).await?;
    config.last_sync_at = Some(now_ms());
    if version.is_some() {
        config.last_remote_version = version.clone();
    }
    result.pushed = true;
    result.bookmarks_total = payload.bookmarks.len();
    result.remote_version = version;
    Ok(())
}

/// Upload this device's state as it is, without merging: the cloud copy
/// becomes a mirror of it. `carry` is the cloud copy being replaced, if one
/// was read, so the sections this device does not upload survive.
///
/// The upload continues the cloud copy's lineage only when it starts from
/// exactly that copy (same lineage, same version as the base) — then the local
/// state is that copy plus the edits made here. Anything else (first upload,
/// legacy migration, a copy other devices have moved on) overwrites entries
/// this device never saw, so it begins a new lineage and the other devices
/// merge it as a union instead of reading the overwrite as deletes.
pub(crate) async fn push_current_state(
    store: &impl SyncStore,
    config: &mut SyncConfig,
    base_version: Option<String>,
    carry: Option<&RemoteCopy>,
    result: &mut SyncResult,
) -> Result<(), String> {
    ensure_device_id(config);
    let base = store.base();
    let continued = carry.and_then(|remote| remote.related(base.as_ref()).filter(|base| base.version.is_some() && base.version == remote.version));

    let mut payload = build_tier1_payload(store, config).await?;
    payload.lineage = Some(continued.and_then(|base| base.lineage.clone()).unwrap_or_else(new_lineage));
    if config.include_credentials {
        match seal_credentials(store)? {
            Ok(envelope) => payload.credentials = Some(envelope),
            Err(reason) if result.credentials_skipped.is_none() => result.credentials_skipped = Some(reason.to_string()),
            Err(_) => {}
        }
    }
    let sealed = payload.credentials.clone();
    if let Some(remote) = carry {
        carry_over(&mut payload, &remote.payload, config);
    }
    upload(config, &payload, base_version, result).await?;

    store.save_base(&SyncBase {
        lineage: payload.lineage,
        version: result.remote_version.clone(),
        bookmarks: payload.bookmarks,
        settings: if config.include_settings { payload.settings } else { continued.and_then(|base| base.settings.clone()) },
        credentials: sealed.or_else(|| continued.and_then(|base| base.credentials.clone())),
    })?;
    store.save_config(config)
}

/// `GET` the vault as a `rest-v2` copy. `Ok(None)` means nothing has been
/// uploaded yet; a legacy vault is an error so it is never merged or replaced
/// by accident.
async fn pull_remote_copy(config: &SyncConfig) -> Result<Option<RemoteCopy>, String> {
    let Some(remote) = auraxlab_pull(&config.auraxlab).await? else {
        return Ok(None);
    };
    match remote.content {
        RemoteContent::Payload(text) => Ok(Some(RemoteCopy { payload: parse_payload(&text)?, version: remote.version })),
        RemoteContent::LegacyBlob(_) => Err(ERR_LEGACY_VAULT.to_string()),
    }
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
    push_only(&AppStore::new(&app, &master_state), &mut config).await
}

#[tauri::command]
pub async fn cloud_sync_pull(app: AppHandle, replace: bool, master_state: State<'_, MasterPasswordState>) -> Result<SyncResult, String> {
    let mut config = load_config(&app)?;
    ensure_signed_in(&config)?;
    pull_only(&AppStore::new(&app, &master_state), &mut config, replace).await
}

#[tauri::command]
pub async fn cloud_sync_now(app: AppHandle, confirm_deletes: Option<bool>, master_state: State<'_, MasterPasswordState>) -> Result<SyncResult, String> {
    let mut config = load_config(&app)?;
    ensure_signed_in(&config)?;
    sync_now(&AppStore::new(&app, &master_state), &mut config, confirm_deletes.unwrap_or(false)).await
}

/// Upload this device's state without merging the cloud copy first.
async fn push_only(store: &impl SyncStore, config: &mut SyncConfig) -> Result<SyncResult, String> {
    // Read the copy being replaced only to carry over what this device does
    // not upload. A copy that cannot be read (legacy, corrupt) is replaced
    // outright, as before — this is the way out of a broken vault.
    let carry = pull_remote_copy(config).await.ok().flatten();
    let mut result = SyncResult::default();
    push_current_state(store, config, None, carry.as_ref(), &mut result).await?;
    result.outcome = SyncOutcome::Uploaded;
    result.message = "Uploaded to your AuraXLab account.".to_string();
    Ok(result)
}

/// Download the cloud copy and merge it (or, with `replace`, adopt it).
async fn pull_only(store: &impl SyncStore, config: &mut SyncConfig, replace: bool) -> Result<SyncResult, String> {
    let Some(remote) = pull_remote_copy(config).await? else {
        return Err("Your AuraXLab account has no synced data yet.".to_string());
    };
    let base = store.base();
    let related = remote.related(base.as_ref());
    let kept_settings = related.and_then(|base| base.settings.clone());
    let kept_credentials = related.and_then(|base| base.credentials.clone());
    let version = remote.version.clone();
    let theirs = remote.payload;

    let mut result;
    let (base_bookmarks, envelope_merged) = if replace {
        result = apply_payload(store, config, theirs.clone(), true).await?;
        let envelope_merged = config.include_credentials && (theirs.credentials.is_none() || result.credentials_skipped.is_none());
        (theirs.bookmarks.clone(), envelope_merged)
    } else {
        result = SyncResult::default();
        let merged = reconcile(store, config, &theirs, related, false, &mut result).await?;
        result.bookmarks_total = store.bookmarks()?.len();
        // Nothing was uploaded, so the base is the cloud copy, not the merge
        // result — plus the held remote deletes, which must stay "gone from
        // the cloud copy" rather than turn into "new here".
        let mut cloud = theirs.bookmarks.clone();
        let before = related.map(|base| base.bookmarks.as_slice()).unwrap_or_default();
        cloud.extend(before.iter().filter(|c| merged.bookmarks.held.remote_deletes.contains(&c.id)).cloned());
        (cloud, merged.credentials.merged)
    };

    // The cloud copy itself is the common ancestor now: whatever the local
    // state has beyond it is a local change the next two-way sync uploads.
    store.save_base(&SyncBase {
        lineage: theirs.lineage.clone(),
        version: version.clone(),
        bookmarks: base_bookmarks,
        settings: if config.include_settings && theirs.settings.is_some() { theirs.settings.clone() } else { kept_settings },
        credentials: if envelope_merged { theirs.credentials.clone() } else { kept_credentials },
    })?;
    config.last_sync_at = Some(now_ms());
    if version.is_some() {
        config.last_remote_version = version.clone();
    }
    store.save_config(config)?;

    result.remote_version = version;
    (result.outcome, result.message) = if replace {
        (SyncOutcome::Replaced, "Replaced local data with the cloud copy.".to_string())
    } else {
        (SyncOutcome::Merged, "Merged the cloud copy into local data.".to_string())
    };
    describe_outcome(&mut result);
    Ok(result)
}

/// Two-way sync: three-way merge the cloud copy into local state, then upload
/// the result if it says anything the cloud copy does not. `confirm_deletes`
/// lets a manual run carry out deletes the mass-delete guard held back.
async fn sync_now(store: &impl SyncStore, config: &mut SyncConfig, confirm_deletes: bool) -> Result<SyncResult, String> {
    let mut result = SyncResult::default();
    // Another device can upload between the pull and the push (409). By then
    // the local merge is written and the base is not, so the round can simply
    // run again: what was merged already compares equal the second time.
    match sync_round(store, config, confirm_deletes, &mut result).await {
        Err(error) if error == ERR_CONFLICT => sync_round(store, config, confirm_deletes, &mut result).await?,
        other => other?,
    }

    let merged_anything = result.settings_applied || result.bookmarks_added + result.bookmarks_updated + result.bookmarks_removed + result.known_hosts_added + result.credentials_synced > 0;
    if result.pushed || merged_anything {
        result.message.push_str("Two-way sync complete.");
    } else {
        result.pulled = false;
        result.outcome = SyncOutcome::UpToDate;
        result.message = "Already up to date.".to_string();
    }
    describe_outcome(&mut result);
    Ok(result)
}

async fn sync_round(store: &impl SyncStore, config: &mut SyncConfig, confirm_deletes: bool, result: &mut SyncResult) -> Result<(), String> {
    // Only "nothing uploaded yet" proceeds straight to the push; every other
    // failure — including a legacy vault — stops here so the cloud copy is
    // never overwritten by mistake.
    let Some(remote) = pull_remote_copy(config).await? else {
        result.outcome = SyncOutcome::FirstSync;
        result.message = "(first sync) ".to_string();
        return push_current_state(store, config, None, None, result).await;
    };
    let base = store.base();
    let related = remote.related(base.as_ref());
    let theirs = &remote.payload;

    // 1) Merge the cloud copy into local state.
    let merged = reconcile(store, config, theirs, related, confirm_deletes, result).await?;

    // 2) Upload the result — unless the cloud copy already says the same.
    ensure_device_id(config);
    let mut outgoing = build_tier1_payload(store, config).await?;
    outgoing.bookmarks = merged.bookmarks.upload;
    outgoing.lineage = Some(theirs.lineage.clone().unwrap_or_else(new_lineage));
    outgoing.credentials = merged.credentials.envelope;
    carry_over(&mut outgoing, theirs, config);
    if merged.credentials.changed || tier1_differs(&outgoing, theirs) {
        upload(config, &outgoing, remote.version.clone(), result).await?;
    } else {
        config.last_sync_at = Some(now_ms());
        if remote.version.is_some() {
            config.last_remote_version = remote.version.clone();
        }
        result.remote_version = remote.version.clone();
    }
    result.bookmarks_total = outgoing.bookmarks.len();

    // 3) What both sides now agree on is the base of the next merge.
    store.save_base(&SyncBase {
        lineage: outgoing.lineage,
        version: result.remote_version.clone(),
        bookmarks: merged.bookmarks.base,
        settings: if config.include_settings { outgoing.settings } else { related.and_then(|base| base.settings.clone()) },
        credentials: if merged.credentials.merged { outgoing.credentials } else { related.and_then(|base| base.credentials.clone()) },
    })?;
    store.save_config(config)
}

/// Append what the user has to know beyond the counts: conflicts the cloud
/// copy won, and deletes waiting for a confirmed manual run.
fn describe_outcome(result: &mut SyncResult) {
    result.conflicts.sort();
    result.conflicts.dedup();
    if !result.conflicts.is_empty() {
        let n = result.conflicts.len();
        let labels: Vec<String> = result.conflicts.iter().map(SyncConflict::label).collect();
        result.message.push_str(&format!(" {n} conflict{} — cloud copy kept: {}.", if n == 1 { "" } else { "s" }, labels.join(", ")));
    }
    if let Some(held) = &result.deletes_held {
        let n = held.local + held.remote;
        result.message.push_str(&format!(" {n} bookmark deletions held back — run Sync now from Sync settings to confirm them."));
    }
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
    if config.auraxlab.account_subject != subject {
        // Another account's vault: nothing known about the old one applies.
        forget_remote(&mut config);
        forget_base(app);
    }
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
    forget_remote(&mut config);
    forget_base(app);
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
    fn credential_merge_counts_only_new_or_changed_entries() {
        let cred = |id: &str, password: &str| {
            let mut credential = StoredCredential::default();
            credential.connection_id = id.to_string();
            credential.password = Some(password.to_string());
            credential
        };
        let mut local = vec![cred("a", "one"), cred("b", "two")];

        // Re-syncing identical data is a no-op.
        assert_eq!(merge_credentials(&mut local, vec![cred("a", "one"), cred("b", "two")]), 0);

        // One changed, one new; the untouched entry is not counted.
        let changed = merge_credentials(&mut local, vec![cred("a", "one"), cred("b", "TWO"), cred("c", "three")]);
        assert_eq!(changed, 2);
        assert_eq!(local.len(), 3);
        assert_eq!(local[1].password.as_deref(), Some("TWO"));
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
        spawn_vault_mock_with(store, |_| {})
    }

    /// `before_put` runs on the stored vault before each upload is checked —
    /// the place to stage another device's upload landing first.
    fn spawn_vault_mock_with(store: MockVault, before_put: impl Fn(&mut (Option<(String, String)>, i64)) + Send + 'static) -> String {
        spawn_mock(move |method, url, body| {
            if !url.contains("/auraterm/sync/vault") {
                return (404, b"{}".to_vec(), vec![]);
            }
            let mut g = store.lock().unwrap();
            match method {
                "PUT" => {
                    before_put(&mut g);
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

    // ========================================================================
    // Two-way sync flow: in-memory devices sharing one mock vault
    // ========================================================================

    const MASTER: &str = "master-pw";

    /// An in-memory device: what [`AppStore`] keeps in the config directory.
    pub(crate) struct FakeDevice {
        bookmarks: Mutex<Vec<SavedConnection>>,
        settings: Mutex<serde_json::Map<String, Value>>,
        known_hosts: Mutex<HashMap<String, String>>,
        credentials: Mutex<Vec<StoredCredential>>,
        /// The master password when unlocked, otherwise the skip code.
        envelope_key: Mutex<Result<String, &'static str>>,
        base: Mutex<Option<SyncBase>>,
        pub(crate) config: SyncConfig,
    }

    impl FakeDevice {
        /// Signed in to the mock vault, syncing every section, unlocked.
        pub(crate) fn new(base: &str, label: &str) -> Self {
            Self {
                bookmarks: Mutex::new(Vec::new()),
                settings: Mutex::new(serde_json::Map::new()),
                known_hosts: Mutex::new(HashMap::new()),
                credentials: Mutex::new(Vec::new()),
                envelope_key: Mutex::new(Ok(MASTER.to_string())),
                base: Mutex::new(None),
                config: SyncConfig {
                    provider: "auraxlab".into(),
                    include_settings: true,
                    include_known_hosts: true,
                    include_credentials: true,
                    device_id: format!("id-{label}"),
                    device_label: label.to_string(),
                    auraxlab: mock_provider(base.to_string()),
                    ..Default::default()
                },
            }
        }

        fn lock(&self, reason: &'static str) {
            *self.envelope_key.lock().unwrap() = Err(reason);
        }

        fn unlock(&self) {
            *self.envelope_key.lock().unwrap() = Ok(MASTER.to_string());
        }

        fn save_bookmark(&self, item: SavedConnection) {
            let mut items = self.bookmarks.lock().unwrap();
            match items.iter().position(|c| c.id == item.id) {
                Some(pos) => items[pos] = item,
                None => items.push(item),
            }
        }

        fn delete_bookmark(&self, id: &str) {
            self.bookmarks.lock().unwrap().retain(|c| c.id != id);
        }

        fn bookmark_names(&self) -> Vec<String> {
            let mut names: Vec<String> = self.bookmarks.lock().unwrap().iter().map(|c| c.name.clone()).collect();
            names.sort();
            names
        }

        fn set_setting(&self, key: &str, value: Value) {
            self.settings.lock().unwrap().insert(key.to_string(), value);
        }

        fn setting(&self, key: &str) -> Option<Value> {
            self.settings.lock().unwrap().get(key).cloned()
        }

        fn save_password(&self, id: &str, password: &str) {
            let mut items = self.credentials.lock().unwrap();
            items.retain(|c| c.connection_id != id);
            items.push(credential(id, password));
        }

        fn password(&self, id: &str) -> Option<String> {
            self.credentials.lock().unwrap().iter().find(|c| c.connection_id == id).and_then(|c| c.password.clone())
        }

        async fn sync(&mut self) -> SyncResult {
            self.sync_with(false).await
        }

        /// A manual run that confirms held-back deletes.
        async fn sync_confirmed(&mut self) -> SyncResult {
            self.sync_with(true).await
        }

        async fn sync_with(&mut self, confirm_deletes: bool) -> SyncResult {
            let mut config = self.config.clone();
            let result = sync_now(&*self, &mut config, confirm_deletes).await.expect("sync succeeds");
            self.config = config;
            result
        }
    }

    impl SyncStore for FakeDevice {
        fn bookmarks(&self) -> Result<Vec<SavedConnection>, String> {
            Ok(self.bookmarks.lock().unwrap().clone())
        }

        fn write_bookmarks(&self, items: &[SavedConnection]) -> Result<(), String> {
            *self.bookmarks.lock().unwrap() = items.to_vec();
            Ok(())
        }

        fn settings_subset(&self) -> Result<Value, String> {
            Ok(Value::Object(self.settings.lock().unwrap().clone()))
        }

        fn apply_settings_subset(&self, subset: &Value) -> Result<bool, String> {
            let Value::Object(incoming) = subset else {
                return Ok(false);
            };
            let mut settings = self.settings.lock().unwrap();
            let mut changed = false;
            for (key, value) in incoming {
                if settings.get(key) != Some(value) {
                    settings.insert(key.clone(), value.clone());
                    changed = true;
                }
            }
            Ok(changed)
        }

        async fn known_hosts(&self) -> Result<HashMap<String, String>, String> {
            Ok(self.known_hosts.lock().unwrap().clone())
        }

        async fn import_known_hosts(&self, hosts: HashMap<String, String>) -> Result<usize, String> {
            let mut local = self.known_hosts.lock().unwrap();
            let before = local.len();
            for (host, fingerprint) in hosts {
                local.entry(host).or_insert(fingerprint);
            }
            Ok(local.len() - before)
        }

        fn envelope_key(&self) -> Result<Result<Zeroizing<String>, &'static str>, String> {
            Ok(self.envelope_key.lock().unwrap().clone().map(Zeroizing::new))
        }

        fn credentials(&self) -> Result<Vec<StoredCredential>, String> {
            Ok(self.credentials.lock().unwrap().clone())
        }

        fn write_credentials(&self, items: Vec<StoredCredential>) -> Result<(), String> {
            *self.credentials.lock().unwrap() = items;
            Ok(())
        }

        fn save_config(&self, _config: &SyncConfig) -> Result<(), String> {
            Ok(())
        }

        fn base(&self) -> Option<SyncBase> {
            self.base.lock().unwrap().clone()
        }

        fn save_base(&self, base: &SyncBase) -> Result<(), String> {
            *self.base.lock().unwrap() = Some(base.clone());
            Ok(())
        }
    }

    fn credential(id: &str, password: &str) -> StoredCredential {
        let mut credential = StoredCredential::default();
        credential.connection_id = id.to_string();
        credential.password = Some(password.to_string());
        credential
    }

    fn new_vault() -> (MockVault, String) {
        let vault: MockVault = Arc::new(Mutex::new((None, 0)));
        let base = spawn_vault_mock(Arc::clone(&vault));
        (vault, base)
    }

    fn vault_version(vault: &MockVault) -> i64 {
        vault.lock().unwrap().1
    }

    fn vault_payload(vault: &MockVault) -> SyncPayload {
        let text = vault.lock().unwrap().0.clone().expect("vault holds data").1;
        parse_payload(&text).expect("vault holds a rest-v2 payload")
    }

    fn vault_bookmark_names(vault: &MockVault) -> Vec<String> {
        let mut names: Vec<String> = vault_payload(vault).bookmarks.iter().map(|c| c.name.clone()).collect();
        names.sort();
        names
    }

    fn vault_passwords(vault: &MockVault) -> HashMap<String, String> {
        let envelope = vault_payload(vault).credentials.expect("vault holds a credentials envelope");
        let blob = STANDARD.decode(envelope.blob).unwrap();
        let plaintext = encryption::decrypt_credentials_envelope(&blob, MASTER).unwrap();
        let list: Vec<StoredCredential> = serde_json::from_slice(&plaintext).unwrap();
        list.iter().map(|c| (c.connection_id.clone(), c.password.clone().unwrap_or_default())).collect()
    }

    #[tokio::test]
    async fn sync_keeps_a_local_bookmark_edit() {
        let (vault, base) = new_vault();
        let mut device = FakeDevice::new(&base, "laptop");
        device.save_bookmark(bookmark("a", "before"));
        device.sync().await;

        device.save_bookmark(bookmark("a", "after"));
        device.sync().await;

        assert_eq!(device.bookmark_names(), ["after"], "the pull must not roll the edit back");
        assert_eq!(vault_bookmark_names(&vault), ["after"]);
    }

    #[tokio::test]
    async fn sync_keeps_a_local_bookmark_delete() {
        let (vault, base) = new_vault();
        let mut device = FakeDevice::new(&base, "laptop");
        device.save_bookmark(bookmark("a", "keep"));
        device.save_bookmark(bookmark("b", "drop"));
        device.sync().await;

        device.delete_bookmark("b");
        device.sync().await;

        assert_eq!(device.bookmark_names(), ["keep"], "the pull must not bring the bookmark back");
        assert_eq!(vault_bookmark_names(&vault), ["keep"]);
    }

    #[tokio::test]
    async fn sync_keeps_a_local_setting_and_password_edit() {
        let (vault, base) = new_vault();
        let mut device = FakeDevice::new(&base, "laptop");
        device.save_bookmark(bookmark("a", "host"));
        device.set_setting("fontSize", json!(13));
        device.save_password("a", "old");
        device.sync().await;

        device.set_setting("fontSize", json!(16));
        device.save_password("a", "new");
        device.sync().await;

        assert_eq!(device.setting("fontSize"), Some(json!(16)));
        assert_eq!(device.password("a").as_deref(), Some("new"));
        assert_eq!(vault_payload(&vault).settings.unwrap()["fontSize"], json!(16));
        assert_eq!(vault_passwords(&vault)["a"], "new");
    }

    #[tokio::test]
    async fn sync_with_nothing_to_send_does_not_upload() {
        let (vault, base) = new_vault();
        let mut device = FakeDevice::new(&base, "laptop");
        device.save_bookmark(bookmark("a", "host"));
        device.save_password("a", "pw");
        device.sync().await;
        assert_eq!(vault_version(&vault), 1);

        let result = device.sync().await;

        assert_eq!(vault_version(&vault), 1, "identical data must not be uploaded again");
        assert!(!result.pushed);
        assert!(!result.pulled);
        assert_eq!(result.outcome, SyncOutcome::UpToDate);
        assert_eq!(result.message, "Already up to date.");
    }

    #[tokio::test]
    async fn sync_from_a_locked_device_keeps_the_cloud_credentials() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "host"));
        desktop.save_password("a", "pw");
        desktop.sync().await;
        let sealed = vault_payload(&vault).credentials.expect("desktop uploaded an envelope").blob;

        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.lock(skip::MASTER_LOCKED);
        laptop.save_bookmark(bookmark("b", "other"));
        let result = laptop.sync().await;

        assert_eq!(result.credentials_skipped.as_deref(), Some(skip::MASTER_LOCKED));
        assert_eq!(vault_bookmark_names(&vault), ["host", "other"]);
        let kept = vault_payload(&vault).credentials.expect("the upload must carry the envelope over");
        assert_eq!(kept.blob, sealed, "carried over verbatim, not re-sealed");
    }

    #[tokio::test]
    async fn sync_merges_another_devices_upload_without_echoing_it_back() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "from-desktop"));
        desktop.sync().await;

        // The laptop lists its own bookmark first, so the two devices end up
        // with the same set in a different order.
        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.save_bookmark(bookmark("b", "from-laptop"));
        laptop.sync().await;
        assert_eq!(vault_version(&vault), 2);

        let result = desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["from-desktop", "from-laptop"]);
        assert!(result.pulled);
        assert_eq!(result.bookmarks_added, 1);
        assert!(!result.pushed, "order alone is not a difference worth uploading");
        assert_eq!(vault_version(&vault), 2);

        // And the desktop keeps fast-forwarding from the laptop's upload.
        desktop.save_bookmark(bookmark("a", "renamed"));
        desktop.sync().await;
        assert_eq!(vault_bookmark_names(&vault), ["from-laptop", "renamed"]);
    }

    #[tokio::test]
    async fn sync_merges_the_cloud_credentials_once_the_device_unlocks() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "host"));
        desktop.save_password("a", "desktop-pw");
        desktop.sync().await;

        // Locked: tier 1 merges, the envelope does not.
        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.lock(skip::MASTER_LOCKED);
        laptop.sync().await;
        assert_eq!(laptop.password("a"), None);

        // Unlocked with the cloud copy unchanged: tier 1 fast-forwards, but
        // the envelope was never merged and must not be overwritten.
        laptop.unlock();
        laptop.save_bookmark(bookmark("b", "other"));
        laptop.save_password("b", "laptop-pw");
        let result = laptop.sync().await;

        assert_eq!(result.credentials_synced, 1);
        assert_eq!(laptop.password("a").as_deref(), Some("desktop-pw"));
        let cloud = vault_passwords(&vault);
        assert_eq!(cloud.get("a").map(String::as_str), Some("desktop-pw"));
        assert_eq!(cloud.get("b").map(String::as_str), Some("laptop-pw"));
    }

    #[tokio::test]
    async fn sync_carries_over_sections_this_device_does_not_upload() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "host"));
        desktop.set_setting("theme", json!("dark"));
        desktop.known_hosts.lock().unwrap().insert("example.com:22".into(), "SHA256:abc".into());
        desktop.save_password("a", "pw");
        desktop.sync().await;

        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.config.include_settings = false;
        laptop.config.include_known_hosts = false;
        laptop.config.include_credentials = false;
        laptop.save_bookmark(bookmark("b", "other"));
        laptop.sync().await;

        let cloud = vault_payload(&vault);
        assert_eq!(cloud.bookmarks.len(), 2);
        assert_eq!(cloud.settings.unwrap()["theme"], json!("dark"));
        assert_eq!(cloud.known_hosts.get("example.com:22").map(String::as_str), Some("SHA256:abc"));
        assert_eq!(vault_passwords(&vault)["a"], "pw");
        assert_eq!(laptop.setting("theme"), None, "a section that is off is not applied here either");
    }

    #[tokio::test]
    async fn push_only_from_a_locked_device_keeps_the_cloud_credentials() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "host"));
        desktop.save_password("a", "pw");
        desktop.sync().await;

        desktop.lock(skip::MASTER_LOCKED);
        desktop.save_bookmark(bookmark("a", "renamed"));
        let mut config = desktop.config.clone();
        let result = push_only(&desktop, &mut config).await.unwrap();

        assert_eq!(result.credentials_skipped.as_deref(), Some(skip::MASTER_LOCKED));
        assert_eq!(vault_bookmark_names(&vault), ["renamed"]);
        assert_eq!(vault_passwords(&vault)["a"], "pw");
    }

    #[tokio::test]
    async fn sync_does_not_fast_forward_onto_a_recreated_vault() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "from-desktop"));
        desktop.sync().await;

        // The vault is deleted and another device starts it over: version 1
        // again, but not the upload the desktop merged.
        *vault.lock().unwrap() = (None, 0);
        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.save_bookmark(bookmark("b", "from-laptop"));
        laptop.sync().await;
        assert_eq!(vault_version(&vault), 1);

        desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["from-desktop", "from-laptop"]);
        assert_eq!(vault_bookmark_names(&vault), ["from-desktop", "from-laptop"]);
    }

    #[tokio::test]
    async fn sync_replaces_an_envelope_sealed_under_another_master_password() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "host"));
        desktop.save_password("a", "desktop-pw");
        desktop.sync().await;

        let mut laptop = FakeDevice::new(&base, "laptop");
        *laptop.envelope_key.lock().unwrap() = Ok("another-master".to_string());
        laptop.save_password("a", "laptop-pw");
        let result = laptop.sync().await;

        assert_eq!(result.credentials_skipped.as_deref(), Some(skip::MISMATCH));
        assert_eq!(laptop.password("a").as_deref(), Some("laptop-pw"));
        let envelope = vault_payload(&vault).credentials.unwrap();
        assert!(open_envelope(&envelope, "another-master").is_ok(), "unchanged behavior: the unreadable envelope is replaced");
    }

    #[test]
    fn carry_over_keeps_settings_keys_this_build_does_not_sync() {
        let config = SyncConfig { include_settings: true, include_known_hosts: true, ..Default::default() };
        let remote = SyncPayload { settings: Some(json!({"theme": "dark", "fromANewerBuild": 1})), ..Default::default() };
        let mut outgoing = SyncPayload { settings: Some(json!({"theme": "light"})), ..Default::default() };

        carry_over(&mut outgoing, &remote, &config);

        assert_eq!(outgoing.settings, Some(json!({"theme": "light", "fromANewerBuild": 1})));
    }

    #[test]
    fn tier1_difference_ignores_order_and_export_metadata() {
        let ours = SyncPayload { exported_at: 1, device_id: "x".into(), bookmarks: vec![bookmark("a", "A"), bookmark("b", "B")], ..Default::default() };
        let theirs = SyncPayload { exported_at: 2, device_id: "y".into(), bookmarks: vec![bookmark("b", "B"), bookmark("a", "A")], ..Default::default() };
        assert!(!tier1_differs(&ours, &theirs));

        let renamed = SyncPayload { bookmarks: vec![bookmark("a", "A"), bookmark("b", "renamed")], ..Default::default() };
        assert!(tier1_differs(&renamed, &theirs));
        let fewer = SyncPayload { bookmarks: vec![bookmark("a", "A")], ..Default::default() };
        assert!(tier1_differs(&fewer, &theirs));
    }

    // ---- three-way merge across devices (phase 1) ----

    /// Two devices that have synced the same bookmarks.
    async fn two_synced_devices(base: &str, bookmarks: &[(&str, &str)]) -> (FakeDevice, FakeDevice) {
        let mut desktop = FakeDevice::new(base, "desktop");
        for (id, name) in bookmarks {
            desktop.save_bookmark(bookmark(id, name));
        }
        desktop.sync().await;
        let mut laptop = FakeDevice::new(base, "laptop");
        laptop.sync().await;
        (desktop, laptop)
    }

    fn many(prefix: &str, count: usize) -> Vec<(String, String)> {
        (0..count).map(|i| (format!("{prefix}{i}"), format!("{prefix}-{i}"))).collect()
    }

    #[tokio::test]
    async fn sync_keeps_a_local_edit_made_while_another_device_uploaded() {
        let (vault, base) = new_vault();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &[("a", "host-a"), ("b", "host-b")]).await;

        desktop.save_bookmark(bookmark("a", "renamed-on-desktop"));
        laptop.save_bookmark(bookmark("b", "renamed-on-laptop"));
        laptop.sync().await;
        let result = desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["renamed-on-desktop", "renamed-on-laptop"]);
        assert_eq!(vault_bookmark_names(&vault), ["renamed-on-desktop", "renamed-on-laptop"]);
        assert_eq!(result.bookmarks_updated, 1);
        assert!(result.conflicts.is_empty());
        laptop.sync().await;
        assert_eq!(laptop.bookmark_names(), ["renamed-on-desktop", "renamed-on-laptop"]);
    }

    #[tokio::test]
    async fn sync_reports_a_conflict_and_keeps_the_cloud_copy() {
        let (vault, base) = new_vault();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &[("a", "host")]).await;

        desktop.save_bookmark(bookmark("a", "desktop-name"));
        laptop.save_bookmark(bookmark("a", "laptop-name"));
        laptop.sync().await;
        let result = desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["laptop-name"], "the first upload set the cloud copy");
        assert_eq!(vault_bookmark_names(&vault), ["laptop-name"]);
        assert_eq!(result.conflicts, [SyncConflict::new(ConflictKind::Bookmark, "laptop-name")]);
        assert!(result.message.contains("1 conflict — cloud copy kept: laptop-name."), "got: {}", result.message);
    }

    #[tokio::test]
    async fn sync_carries_a_delete_to_the_other_device() {
        let (vault, base) = new_vault();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &[("a", "keep"), ("b", "drop")]).await;
        laptop.save_password("b", "pw");

        desktop.delete_bookmark("b");
        desktop.sync().await;
        let result = laptop.sync().await;

        assert_eq!(laptop.bookmark_names(), ["keep"]);
        assert_eq!(result.bookmarks_removed, 1);
        assert_eq!(laptop.password("b"), None, "the deleted bookmark takes its credential along");
        assert_eq!(vault_bookmark_names(&vault), ["keep"]);
    }

    #[tokio::test]
    async fn sync_lets_an_edit_beat_a_delete() {
        let (vault, base) = new_vault();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &[("a", "host")]).await;

        desktop.delete_bookmark("a");
        laptop.save_bookmark(bookmark("a", "edited"));
        laptop.sync().await;
        desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["edited"]);
        assert_eq!(vault_bookmark_names(&vault), ["edited"]);
    }

    #[tokio::test]
    async fn sync_merges_settings_and_passwords_changed_on_different_devices() {
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "host-a"));
        desktop.save_bookmark(bookmark("b", "host-b"));
        desktop.set_setting("theme", json!("dark"));
        desktop.set_setting("fontSize", json!(13));
        desktop.save_password("a", "pw-a");
        desktop.save_password("b", "pw-b");
        desktop.sync().await;
        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.sync().await;

        desktop.set_setting("theme", json!("light"));
        desktop.save_password("a", "pw-a-desktop");
        laptop.set_setting("fontSize", json!(16));
        laptop.save_password("b", "pw-b-laptop");
        laptop.sync().await;
        let result = desktop.sync().await;

        assert!(result.conflicts.is_empty(), "got: {:?}", result.conflicts);
        assert_eq!(desktop.setting("theme"), Some(json!("light")));
        assert_eq!(desktop.setting("fontSize"), Some(json!(16)));
        assert_eq!(desktop.password("a").as_deref(), Some("pw-a-desktop"));
        assert_eq!(desktop.password("b").as_deref(), Some("pw-b-laptop"));
        let cloud = vault_passwords(&vault);
        assert_eq!((cloud["a"].as_str(), cloud["b"].as_str()), ("pw-a-desktop", "pw-b-laptop"));
        let settings = vault_payload(&vault).settings.unwrap();
        assert_eq!((&settings["theme"], &settings["fontSize"]), (&json!("light"), &json!(16)));
    }

    #[tokio::test]
    async fn sync_names_setting_and_credential_conflicts() {
        let (_vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.save_bookmark(bookmark("a", "host"));
        desktop.set_setting("theme", json!("dark"));
        desktop.save_password("a", "pw");
        desktop.sync().await;
        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.sync().await;

        desktop.set_setting("theme", json!("light"));
        desktop.save_password("a", "desktop-pw");
        laptop.set_setting("theme", json!("solarized"));
        laptop.save_password("a", "laptop-pw");
        laptop.sync().await;
        let result = desktop.sync().await;

        assert_eq!(result.conflicts, [SyncConflict::new(ConflictKind::Credentials, "host"), SyncConflict::new(ConflictKind::Setting, "theme")]);
        assert!(result.message.contains("2 conflicts — cloud copy kept: host (credentials), setting theme."), "got: {}", result.message);
        assert_eq!(desktop.setting("theme"), Some(json!("solarized")));
        assert_eq!(desktop.password("a").as_deref(), Some("laptop-pw"));
    }

    #[tokio::test]
    async fn sync_applies_the_cloud_settings_when_the_section_is_switched_on_later() {
        let (_vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.set_setting("theme", json!("dark"));
        desktop.sync().await;

        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.config.include_settings = false;
        laptop.set_setting("theme", json!("light"));
        laptop.sync().await;
        assert_eq!(laptop.setting("theme"), Some(json!("light")));

        // No settings base yet, so switching the section on is a first merge:
        // the cloud copy wins, as it does on a new device.
        laptop.config.include_settings = true;
        laptop.sync().await;
        assert_eq!(laptop.setting("theme"), Some(json!("dark")));
    }

    #[tokio::test]
    async fn sync_holds_back_a_mass_delete_from_the_cloud_copy_until_confirmed() {
        let (vault, base) = new_vault();
        let items = many("h", MASS_DELETE_THRESHOLD);
        let refs: Vec<(&str, &str)> = items.iter().map(|(id, name)| (id.as_str(), name.as_str())).chain([("keep", "keep")]).collect();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &refs).await;
        laptop.save_password("h0", "pw");

        for (id, _) in &items {
            desktop.delete_bookmark(id);
        }
        desktop.sync_confirmed().await;
        assert_eq!(vault_bookmark_names(&vault), ["keep"]);

        // The laptop's automatic run keeps everything and leaves the cloud copy alone.
        let result = laptop.sync().await;
        assert_eq!(result.deletes_held, Some(HeldDeletes { local: 0, remote: MASS_DELETE_THRESHOLD }));
        assert_eq!(result.bookmarks_removed, 0);
        assert_eq!(laptop.bookmark_names().len(), MASS_DELETE_THRESHOLD + 1);
        assert_eq!(laptop.password("h0").as_deref(), Some("pw"), "a held bookmark keeps its credential");
        assert_eq!(vault_bookmark_names(&vault), ["keep"], "holding back is not re-adding");
        assert!(result.message.contains("9 bookmark deletions held back"), "got: {}", result.message);

        // Still held on the next automatic run; a confirmed manual run applies them.
        assert!(laptop.sync().await.deletes_held.is_some());
        let result = laptop.sync_confirmed().await;
        assert_eq!(result.deletes_held, None);
        assert_eq!(result.bookmarks_removed, MASS_DELETE_THRESHOLD);
        assert_eq!(laptop.bookmark_names(), ["keep"]);
        assert_eq!(laptop.password("h0"), None);
    }

    #[tokio::test]
    async fn sync_holds_back_a_local_mass_delete_until_confirmed() {
        let (vault, base) = new_vault();
        let items = many("h", MASS_DELETE_THRESHOLD);
        let refs: Vec<(&str, &str)> = items.iter().map(|(id, name)| (id.as_str(), name.as_str())).chain([("keep", "keep")]).collect();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &refs).await;

        for (id, _) in &items {
            desktop.delete_bookmark(id);
        }
        let result = desktop.sync().await;

        assert_eq!(result.deletes_held, Some(HeldDeletes { local: MASS_DELETE_THRESHOLD, remote: 0 }));
        assert_eq!(desktop.bookmark_names(), ["keep"], "they stay deleted here");
        assert_eq!(vault_bookmark_names(&vault).len(), MASS_DELETE_THRESHOLD + 1, "but the cloud copy keeps them");
        laptop.sync().await;
        assert_eq!(laptop.bookmark_names().len(), MASS_DELETE_THRESHOLD + 1);

        desktop.sync_confirmed().await;
        assert_eq!(vault_bookmark_names(&vault), ["keep"]);
    }

    #[tokio::test]
    async fn sync_lets_a_few_deletes_through_unconfirmed() {
        let (vault, base) = new_vault();
        let items = many("h", MASS_DELETE_THRESHOLD - 1);
        let refs: Vec<(&str, &str)> = items.iter().map(|(id, name)| (id.as_str(), name.as_str())).chain([("keep", "keep")]).collect();
        let (mut desktop, _laptop) = two_synced_devices(&base, &refs).await;

        for (id, _) in &items {
            desktop.delete_bookmark(id);
        }
        let result = desktop.sync().await;

        assert_eq!(result.deletes_held, None);
        assert_eq!(vault_bookmark_names(&vault), ["keep"]);
    }

    #[tokio::test]
    async fn sync_restores_a_lost_bookmark_file_instead_of_emptying_the_cloud_copy() {
        let (vault, base) = new_vault();
        let (mut desktop, _laptop) = two_synced_devices(&base, &[("a", "one"), ("b", "two")]).await;

        desktop.write_bookmarks(&[]).unwrap();
        let result = desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["one", "two"]);
        assert_eq!(result.bookmarks_added, 2);
        assert_eq!(vault_bookmark_names(&vault), ["one", "two"]);
    }

    #[tokio::test]
    async fn sync_never_reads_deletes_into_an_upload_from_an_older_build() {
        let (vault, base) = new_vault();
        let (mut desktop, _laptop) = two_synced_devices(&base, &[("a", "one"), ("b", "two")]).await;

        // An older build round-trips the payload through a struct without
        // `lineage`, and here it uploads fewer bookmarks than the base has.
        let mut old = vault_payload(&vault);
        old.lineage = None;
        old.bookmarks.retain(|c| c.id == "a");
        {
            let mut g = vault.lock().unwrap();
            g.0 = Some(("rest-v2".to_string(), serde_json::to_string(&old).unwrap()));
            g.1 += 1;
        }

        let result = desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["one", "two"], "unrelated copy: union, nothing deleted");
        assert_eq!(result.bookmarks_removed, 0);
        assert_eq!(vault_bookmark_names(&vault), ["one", "two"]);
        assert!(vault_payload(&vault).lineage.is_some(), "and the cloud copy is stamped again");
    }

    #[tokio::test]
    async fn push_only_from_a_new_device_is_not_read_as_deletes() {
        let (vault, base) = new_vault();
        let (mut desktop, _laptop) = two_synced_devices(&base, &[("a", "one"), ("b", "two")]).await;

        let tablet = FakeDevice::new(&base, "tablet");
        tablet.save_bookmark(bookmark("t", "tablet-only"));
        let mut config = tablet.config.clone();
        push_only(&tablet, &mut config).await.unwrap();
        assert_eq!(vault_bookmark_names(&vault), ["tablet-only"]);

        desktop.sync().await;

        assert_eq!(desktop.bookmark_names(), ["one", "tablet-only", "two"]);
        assert_eq!(vault_bookmark_names(&vault), ["one", "tablet-only", "two"]);
    }

    #[tokio::test]
    async fn push_only_on_top_of_the_synced_copy_carries_deletes() {
        let (_vault, base) = new_vault();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &[("a", "keep"), ("b", "drop")]).await;
        desktop.sync().await; // picks up the version the laptop's first sync may have left

        desktop.delete_bookmark("b");
        let mut config = desktop.config.clone();
        push_only(&desktop, &mut config).await.unwrap();
        desktop.config = config;

        laptop.sync().await;
        assert_eq!(laptop.bookmark_names(), ["keep"]);
    }

    #[tokio::test]
    async fn pull_merge_leaves_local_changes_for_the_next_sync() {
        let (vault, base) = new_vault();
        let (mut desktop, mut laptop) = two_synced_devices(&base, &[("a", "host-a"), ("b", "host-b")]).await;

        laptop.save_bookmark(bookmark("b", "renamed-on-laptop"));
        laptop.sync().await;
        desktop.save_bookmark(bookmark("a", "renamed-on-desktop"));
        let mut config = desktop.config.clone();
        let result = pull_only(&desktop, &mut config, false).await.unwrap();
        desktop.config = config;

        assert_eq!(desktop.bookmark_names(), ["renamed-on-desktop", "renamed-on-laptop"]);
        assert_eq!(result.bookmarks_updated, 1);
        assert_eq!(vault_bookmark_names(&vault), ["host-a", "renamed-on-laptop"], "a pull uploads nothing");

        desktop.sync().await;
        assert_eq!(vault_bookmark_names(&vault), ["renamed-on-desktop", "renamed-on-laptop"]);
    }

    #[tokio::test]
    async fn sync_retries_once_when_another_device_uploads_in_between() {
        let vault: MockVault = Arc::new(Mutex::new((None, 0)));
        let staged: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let interloper = Arc::clone(&staged);
        let base = spawn_vault_mock_with(Arc::clone(&vault), move |g| {
            if let Some(payload) = interloper.lock().unwrap().take() {
                g.0 = Some(("rest-v2".to_string(), payload));
                g.1 += 1;
            }
        });
        let (mut desktop, _laptop) = two_synced_devices(&base, &[("a", "host-a"), ("b", "host-b")]).await;

        // The laptop's upload lands after the desktop's pull and before its push.
        let mut theirs = vault_payload(&vault);
        theirs.bookmarks.iter_mut().filter(|c| c.id == "b").for_each(|c| c.name = "renamed-on-laptop".into());
        *staged.lock().unwrap() = Some(serde_json::to_string(&theirs).unwrap());
        desktop.save_bookmark(bookmark("a", "renamed-on-desktop"));
        let result = desktop.sync().await;

        assert!(result.pushed);
        assert_eq!(desktop.bookmark_names(), ["renamed-on-desktop", "renamed-on-laptop"]);
        assert_eq!(vault_bookmark_names(&vault), ["renamed-on-desktop", "renamed-on-laptop"]);
    }

    #[tokio::test]
    async fn sync_merges_quick_buttons_added_on_two_devices() {
        let button = |id: &str| json!({"id": id, "label": id, "command": "ls"});
        let (vault, base) = new_vault();
        let mut desktop = FakeDevice::new(&base, "desktop");
        desktop.set_setting("quickButtons", json!([button("shared")]));
        desktop.sync().await;
        let mut laptop = FakeDevice::new(&base, "laptop");
        laptop.sync().await;

        desktop.set_setting("quickButtons", json!([button("shared"), button("from-desktop")]));
        laptop.set_setting("quickButtons", json!([button("shared"), button("from-laptop")]));
        laptop.sync().await;
        let result = desktop.sync().await;

        let merged = json!([button("shared"), button("from-desktop"), button("from-laptop")]);
        assert!(result.conflicts.is_empty(), "got: {:?}", result.conflicts);
        assert_eq!(desktop.setting("quickButtons"), Some(merged.clone()));
        assert_eq!(vault_payload(&vault).settings.unwrap()["quickButtons"], merged);

        // The laptop takes the desktop's list as it is, and that is the end of it.
        let version = vault_version(&vault);
        laptop.sync().await;
        assert_eq!(laptop.setting("quickButtons"), Some(merged));
        assert_eq!(vault_version(&vault), version, "no echo upload");
    }
}
