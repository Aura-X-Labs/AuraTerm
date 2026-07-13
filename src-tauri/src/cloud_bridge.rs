//! Cloud Console bridge: device identity, relay admission and the explicit
//! local-session -> shared-session mapping.
//!
//! RX frames are sent directly from a bounded process-local channel to the
//! relay HTTP adapter. No terminal bytes are written to disk, application
//! settings, or logs. The device credential itself *is* persisted — encrypted
//! under the device-local key like `sync_config.enc` — so a restart does not
//! force re-enrollment.

use crate::encryption;
use crate::shared_session::{SessionPolicy, SessionProtocol, SharedSessionPort, TxPolicy};
use crate::terminal_event_hub::{SubscriptionToken, TerminalEvent};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use aes_gcm::{aead::{Aead, KeyInit, Payload}, Aes256Gcm, Nonce};
use hkdf::Hkdf;
use p256::{ecdh::EphemeralSecret, elliptic_curve::sec1::ToEncodedPoint, PublicKey};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use zeroize::Zeroizing;

const DEVICE_CONFIG_FILE: &str = "console_device.enc";

#[derive(Clone)]
struct DeviceConfig {
    base_url: String,
    /// URL-safe base64 of the Ed25519 private seed (32 bytes). Signs every
    /// proof-of-possession; never leaves the device after enrollment.
    identity_key: String,
    /// URL-safe base64 of the Ed25519 public key uploaded at enrollment.
    identity_public: String,
    credential: String,
    device_id: String,
    key_version: u32,
    label: String,
    boot_id: String,
    relay_connection: String,
    /// kid -> Ed25519 public key bytes (authority verification material).
    authority_keys: HashMap<String, Vec<u8>>,
}

/// On-disk form of the device identity (encrypted with the device-local
/// key). `boot_id` and `relay_connection` are runtime-only and never stored.
#[derive(Serialize, Deserialize)]
struct PersistedDevice {
    base_url: String,
    identity_key: String,
    #[serde(default)]
    identity_public: String,
    credential: String,
    device_id: String,
    key_version: u32,
    #[serde(default)]
    label: String,
    /// kid -> URL-safe base64 verification key, as served at enrollment.
    authority_keys: HashMap<String, String>,
}

#[derive(Clone)]
struct PendingEnrollment {
    base_url: String,
    identity_key: String,
    identity_public: String,
    pkce_verifier: String,
    device_code: String,
    user_code: String,
    label: String,
}

const DEFAULT_RX_RING_BYTES: usize = 256 * 1024;
const MAX_RX_RING_BYTES: usize = 4 * 1024 * 1024;
const MAX_E2EE_SNAPSHOT_BYTES: usize = 20 * 1024;
const MAX_TX_BYTES: usize = 16 * 1024;
/// Idle mode (no shares): presence ping cadence and how long a share-less
/// relay connection is kept before downgrading, so a quick unshare/re-share
/// does not flap the connection.
const IDLE_PING_INTERVAL_SECS: u64 = 30;
const IDLE_DOWNGRADE_GRACE_SECS: u64 = 60;

struct RxRing {
    chunks: VecDeque<(u64, Vec<u8>)>,
    bytes: usize,
    capacity: usize,
    next_seq: u64,
    delivered_seq: u64,
}

impl RxRing {
    fn new(capacity: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            capacity,
            next_seq: 1,
            delivered_seq: 0,
        }
    }

    fn push(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        for chunk in bytes.chunks(MAX_TX_BYTES) {
            let seq = self.next_seq;
            self.next_seq += 1;
            self.bytes += chunk.len();
            self.chunks.push_back((seq, chunk.to_vec()));
            while self.bytes > self.capacity && self.chunks.len() > 1 {
                if let Some((_, removed)) = self.chunks.pop_front() {
                    self.bytes -= removed.len();
                }
            }
        }
    }

    fn snapshot(&self) -> (u64, Vec<u8>) {
        let mut bytes = Vec::with_capacity(self.bytes);
        for (_, chunk) in &self.chunks {
            bytes.extend_from_slice(chunk);
        }
        (self.next_seq.saturating_sub(1), bytes)
    }

    fn e2ee_snapshot(&self) -> (u64, Vec<u8>) {
        let (seq, bytes) = self.snapshot();
        let start = bytes.len().saturating_sub(MAX_E2EE_SNAPSHOT_BYTES);
        (seq, bytes[start..].to_vec())
    }
}

struct SharedSession {
    cloud_session_id: String,
    label: String,
    protocol: SessionProtocol,
    subscription: SubscriptionToken,
    policy: SessionPolicy,
    ring: Arc<Mutex<RxRing>>,
    output_notify: Arc<tokio::sync::Notify>,
    peers: HashMap<String, PeerCipher>,
    last_fence: u64,
    last_input_seq: u64,
}

#[derive(Clone)]
struct PeerCipher {
    key: [u8; 32],
    counters: Arc<Mutex<PeerCounters>>,
    send_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct PeerCounters {
    sent: u64,
    received: u64,
}

/// How agent frames reach the relay right now. Hot-swapped on reconnect so
/// long-lived pumps always send through the *current* connection.
#[derive(Clone)]
enum AgentTransport {
    /// Development HTTP polling adapter (`/console-relay/v1`).
    Http { base_url: String, connection: String },
    /// Standalone wss relay: frames go through the writer task's channel.
    Ws { outbound: tokio::sync::mpsc::Sender<serde_json::Value> },
}

#[derive(Default)]
struct BridgeInner {
    device: Option<DeviceConfig>,
    pending: Option<PendingEnrollment>,
    shares: HashMap<String, SharedSession>,
    transport: Option<AgentTransport>,
    /// The supervisor keeps the agent online while this is true; unbind and
    /// explicit disconnect clear it.
    want_online: bool,
    supervisor_running: bool,
    connecting: bool,
}

pub struct CloudBridgeState {
    port: Arc<dyn SharedSessionPort>,
    inner: Arc<Mutex<BridgeInner>>,
    client: reqwest::Client,
}

impl CloudBridgeState {
    pub fn new(port: Arc<dyn SharedSessionPort>) -> Self {
        Self {
            port,
            inner: Arc::new(Mutex::new(BridgeInner::default())),
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollmentView {
    user_code: String,
    fingerprint: String,
    expires_in: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemOutcome {
    /// "ok" | "pending" | "denied" | "expired"
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    enrolled: bool,
    connected: bool,
    reconnecting: bool,
    /// Enrolled and online-by-ping, holding no relay connection (no shares).
    standby: bool,
    pending_user_code: Option<String>,
    device_id: Option<String>,
    device_label: Option<String>,
    base_url: Option<String>,
    fingerprint: Option<String>,
    shares: Vec<ShareView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    local_session_id: String,
    cloud_session_id: String,
    label: String,
    protocol: SessionProtocol,
    tx_policy: TxPolicy,
    tx_expires_at: Option<u64>,
    tx_allowed: bool,
}

#[derive(Deserialize)]
struct EnrollmentResponse {
    user_code: String,
    device_code: String,
    fingerprint: String,
    expires_in: u64,
}
#[derive(Deserialize)]
struct RedeemResponse {
    device_id: String,
    credential: String,
    key_version: u32,
    authority_keys: HashMap<String, String>,
}
#[derive(Deserialize)]
struct ChallengeResponse {
    nonce: String,
}
#[derive(Deserialize)]
struct RotateResponse {
    credential: String,
    key_version: u32,
}
#[derive(Deserialize)]
struct GrantResponse {
    relay_grant: String,
    #[serde(default)]
    relay_url: Option<String>,
}
#[derive(Deserialize)]
struct RelayConnectResponse {
    connection_id: String,
}
#[derive(Deserialize)]
struct ShareResponse {
    session_id: String,
}
#[derive(Deserialize)]
struct RelayFramesResponse {
    frames: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct LeaseClaims {
    session_id: String,
    device_id: String,
    lease_id: String,
    fence: u64,
    holder_key_hash: String,
    permissions: Vec<String>,
    exp: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteTxEvent {
    local_session_id: String,
    byte_count: usize,
    fence: u64,
}

fn device_config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    Ok(dir.join(DEVICE_CONFIG_FILE))
}

fn save_device_config(app: &AppHandle, device: &DeviceConfig) -> Result<(), String> {
    let persisted = PersistedDevice {
        base_url: device.base_url.clone(),
        identity_key: device.identity_key.clone(),
        identity_public: device.identity_public.clone(),
        credential: device.credential.clone(),
        device_id: device.device_id.clone(),
        key_version: device.key_version,
        label: device.label.clone(),
        authority_keys: device
            .authority_keys
            .iter()
            .map(|(kid, key)| (kid.clone(), URL_SAFE_NO_PAD.encode(key)))
            .collect(),
    };
    let path = device_config_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let key = encryption::load_or_create_local_key(app)?;
    let plaintext = Zeroizing::new(
        serde_json::to_vec(&persisted).map_err(|e| format!("serialize device config: {e}"))?,
    );
    let encrypted = encryption::encrypt_data(&plaintext, &key)?;
    std::fs::write(&path, &encrypted).map_err(|e| format!("write device config: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn load_device_config(app: &AppHandle) -> Result<Option<DeviceConfig>, String> {
    let path = device_config_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let encrypted = std::fs::read(&path).map_err(|e| format!("read device config: {e}"))?;
    let key = encryption::load_or_create_local_key(app)?;
    let plaintext = Zeroizing::new(
        encryption::decrypt_data(&encrypted, &key)
            .map_err(|_| "device config is corrupt or from another machine".to_string())?,
    );
    let persisted: PersistedDevice = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("parse device config: {e}"))?;
    let authority_keys = persisted
        .authority_keys
        .into_iter()
        .map(|(kid, encoded)| {
            URL_SAFE_NO_PAD
                .decode(encoded)
                .map(|key| (kid, key))
                .map_err(|e| format!("invalid stored authority key: {e}"))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(Some(DeviceConfig {
        base_url: persisted.base_url,
        identity_key: persisted.identity_key,
        identity_public: persisted.identity_public,
        credential: persisted.credential,
        device_id: persisted.device_id,
        key_version: persisted.key_version,
        label: persisted.label,
        boot_id: Uuid::new_v4().to_string(),
        relay_connection: String::new(),
        authority_keys,
    }))
}

fn delete_device_config(app: &AppHandle) -> Result<(), String> {
    let path = device_config_path(app)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("remove device config: {e}"))?;
    }
    Ok(())
}

fn normalize_base_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if !(value.starts_with("https://")
        || value.starts_with("http://localhost")
        || value.starts_with("http://127.0.0.1"))
    {
        return Err(
            "Cloud Console URL must use HTTPS (localhost HTTP is allowed for development).".into(),
        );
    }
    Ok(value.to_string())
}

fn random_secret(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

fn sha256_hex(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// Generate a fresh Ed25519 device identity: (private seed b64, public b64).
fn ed25519_keypair() -> (String, String) {
    let signing = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    (
        URL_SAFE_NO_PAD.encode(signing.to_bytes()),
        URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes()),
    )
}

/// Sign a domain-separated context with the device's Ed25519 private seed;
/// returns a URL-safe base64 signature matching the server's `_unb64` proof
/// format.
fn ed25519_sign_b64(seed_b64: &str, message: &str) -> Result<String, String> {
    use ed25519_dalek::Signer;
    let seed: [u8; 32] = URL_SAFE_NO_PAD
        .decode(seed_b64)
        .map_err(|_| "invalid identity key".to_string())?
        .try_into()
        .map_err(|_| "invalid identity key length".to_string())?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    Ok(URL_SAFE_NO_PAD.encode(signing.sign(message.as_bytes()).to_bytes()))
}

/// Verify an Ed25519 signature (e.g. a `cav1` lease grant) with a 32-byte
/// public key. Any malformed input fails closed.
fn ed25519_verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    use ed25519_dalek::Verifier;
    let Ok(public_bytes) = <[u8; 32]>::try_from(public_key) else {
        return false;
    };
    let Ok(verifying) = ed25519_dalek::VerifyingKey::from_bytes(&public_bytes) else {
        return false;
    };
    let Ok(sig_bytes) = <[u8; 64]>::try_from(signature) else {
        return false;
    };
    verifying
        .verify(message, &ed25519_dalek::Signature::from_bytes(&sig_bytes))
        .is_ok()
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.is_empty() || value.len() % 2 != 0 || value.len() > MAX_TX_BYTES * 2 {
        return Err("invalid TX byte length".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "invalid TX hex bytes".to_string())
        })
        .collect()
}

fn verify_input_frame(
    share: &mut SharedSession,
    device: &DeviceConfig,
    frame: &serde_json::Value,
) -> Result<(Vec<u8>, u64, u64), String> {
    if !share.policy.allows_tx(std::time::SystemTime::now()) {
        return Err("remote TX is disabled by local policy".into());
    }
    let token = frame
        .get("lease_grant")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing TX lease".to_string())?;
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != "cav1" {
        return Err("malformed TX lease".into());
    }
    let key = device
        .authority_keys
        .get(parts[1])
        .ok_or_else(|| "unknown TX lease signer".to_string())?;
    let signature = URL_SAFE_NO_PAD
        .decode(parts[3])
        .map_err(|_| "malformed TX lease signature".to_string())?;
    let signed = format!("lease-grant.{}", parts[2]);
    if !ed25519_verify(key, signed.as_bytes(), &signature) {
        return Err("invalid TX lease signature".into());
    }
    let claims: LeaseClaims = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| "malformed TX lease claims".to_string())?,
    )
    .map_err(|_| "invalid TX lease claims".to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs() as i64;
    let frame_fence = frame
        .get("fence")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing TX fence".to_string())?;
    let input_seq = frame
        .get("input_seq")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing TX input sequence".to_string())?;
    if claims.exp < now
        || claims.session_id != share.cloud_session_id
        || claims.device_id != device.device_id
        || frame.get("lease_id").and_then(|v| v.as_str()) != Some(&claims.lease_id)
        || frame_fence != claims.fence
        || !claims.permissions.iter().any(|p| p == "input")
        || claims.holder_key_hash.is_empty()
    {
        return Err("TX lease claims do not match this session".into());
    }
    if frame_fence < share.last_fence {
        return Err("stale TX fence".into());
    }
    if frame_fence > share.last_fence {
        share.last_fence = frame_fence;
        share.last_input_seq = 0;
    }
    if input_seq <= share.last_input_seq {
        return Err("duplicate or stale TX input sequence".into());
    }
    let bytes = decode_hex(
        frame
            .get("data_hex")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing TX bytes".to_string())?,
    )?;
    share.last_input_seq = input_seq;
    Ok((bytes, frame_fence, input_seq))
}

async fn json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, String> {
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Cloud Console returned {}: {}", status, body));
    }
    serde_json::from_str(&body).map_err(|e| format!("Invalid Cloud Console response: {e}"))
}

#[tauri::command]
pub async fn cloud_bridge_begin_enrollment(
    state: State<'_, CloudBridgeState>,
    base_url: String,
    label: String,
    platform: String,
) -> Result<EnrollmentView, String> {
    let base_url = normalize_base_url(&base_url)?;
    let (identity_key, identity_public) = ed25519_keypair();
    let pkce_verifier = random_secret(48);
    let response = state
        .client
        .post(format!("{base_url}/api/v1/auraterm/console/enrollments"))
        .json(&json!({
            "device_public_key": identity_public,
            "pkce_challenge": sha256_hex(&pkce_verifier),
            "label": label,
            "platform": platform,
            "app_version": env!("CARGO_PKG_VERSION"),
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let result: EnrollmentResponse = json_response(response).await?;
    state.inner.lock().map_err(|e| e.to_string())?.pending = Some(PendingEnrollment {
        base_url,
        identity_key,
        identity_public,
        pkce_verifier,
        device_code: result.device_code,
        user_code: result.user_code.clone(),
        label,
    });
    Ok(EnrollmentView {
        user_code: result.user_code,
        fingerprint: result.fingerprint,
        expires_in: result.expires_in,
    })
}

/// Authorize the pending enrollment with the account password (the
/// login-and-bind fast path). The password is used for this one request and
/// never stored; on success the enrollment is approved server-side and
/// `cloud_bridge_redeem_enrollment` completes without the browser step.
#[tauri::command]
pub async fn cloud_bridge_authorize_enrollment(
    state: State<'_, CloudBridgeState>,
    email: String,
    password: String,
) -> Result<(), String> {
    let pending = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .pending
        .clone()
        .ok_or_else(|| "No Cloud Console enrollment is pending.".to_string())?;
    let fingerprint = sha256_hex(&pending.identity_public);
    let response = state
        .client
        .post(format!(
            "{}/api/v1/auraterm/console/enrollments/authorize",
            pending.base_url
        ))
        .basic_auth(&email, Some(&password))
        .json(&json!({"user_code": pending.user_code, "fingerprint": fingerprint}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let _: serde_json::Value = json_response(response).await?;
    Ok(())
}

#[tauri::command]
pub async fn cloud_bridge_redeem_enrollment(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
) -> Result<RedeemOutcome, String> {
    let pending = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .pending
        .clone()
        .ok_or_else(|| "No Cloud Console enrollment is pending.".to_string())?;
    let response = state
        .client
        .post(format!(
            "{}/api/v1/auraterm/console/enrollments/token",
            pending.base_url
        ))
        .json(&json!({"device_code": pending.device_code, "pkce_verifier": pending.pkce_verifier}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match response.status().as_u16() {
        428 => return Ok(RedeemOutcome { status: "pending".into() }),
        403 => {
            state.inner.lock().map_err(|e| e.to_string())?.pending = None;
            return Ok(RedeemOutcome { status: "denied".into() });
        }
        410 => {
            state.inner.lock().map_err(|e| e.to_string())?.pending = None;
            return Ok(RedeemOutcome { status: "expired".into() });
        }
        _ => {}
    }
    let result: RedeemResponse = json_response(response).await?;
    let authority_keys = result
        .authority_keys
        .into_iter()
        .map(|(kid, encoded)| {
            URL_SAFE_NO_PAD
                .decode(encoded)
                .map(|key| (kid, key))
                .map_err(|e| format!("Invalid authority verification key: {e}"))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let device = DeviceConfig {
        base_url: pending.base_url,
        identity_key: pending.identity_key,
        identity_public: pending.identity_public,
        credential: result.credential,
        device_id: result.device_id,
        key_version: result.key_version,
        label: pending.label,
        boot_id: Uuid::new_v4().to_string(),
        relay_connection: String::new(),
        authority_keys,
    };
    save_device_config(&app, &device)?;
    let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
    inner.device = Some(device);
    inner.pending = None;
    Ok(RedeemOutcome { status: "ok".into() })
}

#[tauri::command]
pub async fn cloud_bridge_connect(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
) -> Result<(), String> {
    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        if inner.device.is_none() {
            return Err("Enroll this AuraTerm device first.".to_string());
        }
        inner.want_online = true;
    }
    connect_inner(
        state.client.clone(),
        Arc::clone(&state.inner),
        Arc::clone(&state.port),
        app.clone(),
    )
    .await?;
    ensure_supervisor(
        state.client.clone(),
        Arc::clone(&state.inner),
        Arc::clone(&state.port),
        app,
    );
    Ok(())
}

/// Restore the persisted device identity at startup and bring the bridge
/// online in the background. Missing/undecryptable config is not an error:
/// the bridge simply stays unenrolled.
#[tauri::command]
pub async fn cloud_bridge_restore(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
) -> Result<bool, String> {
    let Some(device) = load_device_config(&app).unwrap_or(None) else {
        return Ok(false);
    };
    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        if inner.device.is_some() {
            return Ok(true); // already restored
        }
        inner.device = Some(device);
        inner.want_online = true;
    }
    ensure_supervisor(
        state.client.clone(),
        Arc::clone(&state.inner),
        Arc::clone(&state.port),
        app,
    );
    Ok(true)
}

/// Unbind this device: best-effort server-side self-revocation, then drop
/// all shares, forget the credential and delete the encrypted config file.
#[tauri::command]
pub async fn cloud_bridge_unbind(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
) -> Result<(), String> {
    let device = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.want_online = false;
        inner.pending = None;
        inner.device.clone()
    };
    if let Some(device) = &device {
        let _ = state
            .client
            .delete(format!(
                "{}/api/v1/auraterm/console/device",
                device.base_url
            ))
            .header("Authorization", format!("Bearer {}", device.credential))
            .send()
            .await;
    }
    let removed: Vec<SharedSession> = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.device = None;
        inner.transport = None;
        inner.shares.drain().map(|(_, share)| share).collect()
    };
    for share in removed {
        state.port.unsubscribe_rx(&share.subscription);
    }
    delete_device_config(&app)?;
    Ok(())
}

/// Rotate the device credential and identity key. The *old* Ed25519 key
/// signs the rotation, so a leaked credential alone cannot rotate; the server
/// advances key_version and auth_epoch atomically, invalidating the old
/// material. The new encrypted config is written before returning.
#[tauri::command]
pub async fn cloud_bridge_rotate_credential(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
) -> Result<(), String> {
    let device = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .device
        .clone()
        .ok_or_else(|| "This AuraTerm device is not bound.".to_string())?;
    let bearer = format!("Bearer {}", device.credential);
    let challenge: ChallengeResponse = json_response(
        state
            .client
            .post(format!(
                "{}/api/v1/auraterm/console/connect-challenge",
                device.base_url
            ))
            .header("Authorization", &bearer)
            .json(&json!({}))
            .send()
            .await
            .map_err(|e| e.to_string())?,
    )
    .await?;
    let (new_seed, new_public) = ed25519_keypair();
    let context = format!(
        "auraxlab-console|rotate|{}|{}|{}",
        device.device_id,
        sha256_hex(&new_public),
        challenge.nonce
    );
    let proof = ed25519_sign_b64(&device.identity_key, &context)?;
    let result: RotateResponse = json_response(
        state
            .client
            .post(format!("{}/api/v1/auraterm/console/rotate", device.base_url))
            .header("Authorization", &bearer)
            .json(&json!({
                "nonce": challenge.nonce,
                "new_identity_public_key": new_public,
                "proof": proof,
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?,
    )
    .await?;
    let mut rotated = device.clone();
    rotated.identity_key = new_seed;
    rotated.identity_public = new_public;
    rotated.credential = result.credential;
    rotated.key_version = result.key_version;
    save_device_config(&app, &rotated)?;
    state.inner.lock().map_err(|e| e.to_string())?.device = Some(rotated);
    Ok(())
}

async fn connect_inner(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    port: Arc<dyn SharedSessionPort>,
    app: AppHandle,
) -> Result<(), String> {
    // Single-flight: a second caller while a connect is in progress is a
    // no-op instead of racing for the relay connection.
    {
        let mut guard = inner.lock().map_err(|e| e.to_string())?;
        if guard.connecting {
            return Ok(());
        }
        if guard
            .device
            .as_ref()
            .is_some_and(|device| !device.relay_connection.is_empty())
        {
            return Ok(());
        }
        guard.connecting = true;
    }
    let result = connect_attempt(&client, &inner, &port, &app).await;
    if let Ok(mut guard) = inner.lock() {
        guard.connecting = false;
    }
    result
}

async fn connect_attempt(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle,
) -> Result<(), String> {
    let mut device = inner
        .lock()
        .map_err(|e| e.to_string())?
        .device
        .clone()
        .ok_or_else(|| "Enroll this AuraTerm device first.".to_string())?;
    let bearer = format!("Bearer {}", device.credential);
    let challenge: ChallengeResponse = json_response(
        client
            .post(format!(
                "{}/api/v1/auraterm/console/connect-challenge",
                device.base_url
            ))
            .header("Authorization", &bearer)
            .json(&json!({}))
            .send()
            .await
            .map_err(|e| e.to_string())?,
    )
    .await?;
    let context = format!(
        "auraxlab-console|connect-grant|{}|{}|{}|{}|relay:inmemory",
        device.device_id, device.boot_id, device.key_version, challenge.nonce
    );
    let proof = ed25519_sign_b64(&device.identity_key, &context)?;
    let grant: GrantResponse = json_response(
        client
            .post(format!(
                "{}/api/v1/auraterm/console/connect-grant",
                device.base_url
            ))
            .header("Authorization", &bearer)
            .json(&json!({
                "boot_id": device.boot_id, "key_version": device.key_version,
                "nonce": challenge.nonce, "proof": proof,
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?,
    )
    .await?;
    let relay_url = grant.relay_url.clone().unwrap_or_default();
    if relay_url.starts_with("ws://") || relay_url.starts_with("wss://") {
        connect_websocket(client, inner, port, app, device, &grant, &relay_url).await?;
    } else {
        let relay: RelayConnectResponse = json_response(
            client
                .post(format!(
                    "{}/console-relay/v1/agent/connect",
                    device.base_url
                ))
                .json(&json!({"relay_grant": grant.relay_grant, "boot_id": device.boot_id}))
                .send()
                .await
                .map_err(|e| e.to_string())?,
        )
        .await?;
        device.relay_connection = relay.connection_id;
        {
            let mut guard = inner.lock().map_err(|e| e.to_string())?;
            guard.device = Some(device.clone());
            guard.transport = Some(AgentTransport::Http {
                base_url: device.base_url.clone(),
                connection: device.relay_connection.clone(),
            });
        }
        spawn_control_pump(
            client.clone(),
            Arc::clone(inner),
            Arc::clone(port),
            app.clone(),
            device.clone(),
        );
    }
    recover_shares(client.clone(), Arc::clone(inner)).await;
    let _ = app.emit("cloud-bridge-connected", ());
    Ok(())
}

async fn connect_websocket(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle,
    mut device: DeviceConfig,
    grant: &GrantResponse,
    relay_url: &str,
) -> Result<(), String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut socket, _) = tokio_tungstenite::connect_async(relay_url)
        .await
        .map_err(|e| format!("relay websocket connect failed: {e}"))?;
    socket
        .send(Message::Text(
            json!({"kind": "AUTH", "relay_grant": grant.relay_grant,
                   "boot_id": device.boot_id})
            .to_string()
            .into(),
        ))
        .await
        .map_err(|e| format!("relay AUTH send failed: {e}"))?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
        .await
        .map_err(|_| "relay AUTH timed out".to_string())?
        .ok_or_else(|| "relay closed during AUTH".to_string())?
        .map_err(|e| format!("relay AUTH failed: {e}"))?;
    let Message::Text(text) = reply else {
        return Err("unexpected relay AUTH reply".into());
    };
    let reply: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if reply.get("kind").and_then(|v| v.as_str()) != Some("AUTH_OK") {
        return Err(format!(
            "relay refused the agent: {}",
            reply.get("error").and_then(|v| v.as_str()).unwrap_or("AUTH_FAILED")
        ));
    }
    device.relay_connection = reply
        .get("connection_id")
        .and_then(|v| v.as_str())
        .unwrap_or("ws")
        .to_string();
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(64);
    {
        let mut guard = inner.lock().map_err(|e| e.to_string())?;
        guard.device = Some(device.clone());
        guard.transport = Some(AgentTransport::Ws { outbound: outbound_tx });
    }
    spawn_ws_pumps(
        client.clone(),
        Arc::clone(inner),
        Arc::clone(port),
        app.clone(),
        device,
        socket,
        outbound_rx,
    );
    Ok(())
}

/// Deliberately drop the relay connection for idle mode. Clearing the
/// transport closes the websocket writer channel (the writer then closes
/// the socket) and makes the HTTP pump exit at its next tick; an HTTP relay
/// connection is also deleted server-side so presence drops immediately.
async fn disconnect_transport(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
) {
    let transport = {
        let Ok(mut guard) = inner.lock() else { return };
        if let Some(device) = guard.device.as_mut() {
            device.relay_connection.clear();
        }
        guard.transport.take()
    };
    if let Some(AgentTransport::Http { base_url, connection }) = transport {
        let _ = client
            .delete(format!("{base_url}/console-relay/v1/agent/{connection}"))
            .send()
            .await;
    }
}

#[derive(Deserialize)]
struct PresencePingResponse {
    #[serde(default)]
    ping_interval: Option<u64>,
}

/// Idle-mode liveness: one authenticated POST carrying no payload. The
/// server stages it in Redis with a short TTL; no relay connection exists.
async fn idle_presence_ping(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
) -> Result<Option<u64>, String> {
    let device = inner
        .lock()
        .map_err(|e| e.to_string())?
        .device
        .clone()
        .ok_or_else(|| "device is not enrolled".to_string())?;
    let response = client
        .post(format!(
            "{}/api/v1/auraterm/console/presence",
            device.base_url
        ))
        .header("Authorization", format!("Bearer {}", device.credential))
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("presence ping rejected: {}", response.status()));
    }
    let body: PresencePingResponse = response
        .json()
        .await
        .unwrap_or(PresencePingResponse { ping_interval: None });
    Ok(body.ping_interval)
}

/// Keep the agent reachable. With at least one share the relay connection
/// is held up (exponential backoff on failures); with none it is dropped
/// after a grace period and the agent downgrades to lightweight presence
/// pings — no data plane, just liveness. One supervisor per app.
fn ensure_supervisor(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    port: Arc<dyn SharedSessionPort>,
    app: AppHandle,
) {
    {
        let Ok(mut guard) = inner.lock() else { return };
        if guard.supervisor_running {
            return;
        }
        guard.supervisor_running = true;
    }
    tauri::async_runtime::spawn(async move {
        let mut failures: u32 = 0;
        let mut ping_interval = IDLE_PING_INTERVAL_SECS;
        let mut next_ping = std::time::Instant::now();
        let mut idle_connected_since: Option<std::time::Instant> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let Some((want_online, has_shares, connected)) =
                inner.lock().ok().map(|guard| {
                    (
                        guard.want_online && guard.device.is_some(),
                        !guard.shares.is_empty(),
                        guard.device.as_ref().is_some_and(
                            |device| !device.relay_connection.is_empty()),
                    )
                })
            else {
                continue;
            };
            if !want_online {
                failures = 0;
                idle_connected_since = None;
                continue;
            }
            if has_shares {
                idle_connected_since = None;
                if connected {
                    failures = 0;
                    continue;
                }
                match connect_inner(
                    client.clone(),
                    Arc::clone(&inner),
                    Arc::clone(&port),
                    app.clone(),
                )
                .await
                {
                    Ok(()) => failures = 0,
                    Err(_) => {
                        failures = failures.saturating_add(1);
                        let backoff = 2_u64.saturating_pow(failures.min(5)).min(60);
                        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    }
                }
                continue;
            }
            failures = 0;
            if connected {
                let since = *idle_connected_since
                    .get_or_insert_with(std::time::Instant::now);
                if since.elapsed().as_secs() >= IDLE_DOWNGRADE_GRACE_SECS {
                    disconnect_transport(&client, &inner).await;
                    idle_connected_since = None;
                    next_ping = std::time::Instant::now();
                }
                continue;
            }
            idle_connected_since = None;
            if std::time::Instant::now() >= next_ping {
                next_ping = std::time::Instant::now()
                    + std::time::Duration::from_secs(ping_interval);
                if let Ok(Some(interval)) =
                    idle_presence_ping(&client, &inner).await
                {
                    ping_interval = interval.clamp(10, 300);
                }
            }
        }
    });
}

/// Handle one relay->agent frame. Shared by the HTTP polling pump and the
/// websocket reader so both transports keep identical semantics.
async fn process_inbound_frame(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle,
    device: &DeviceConfig,
    mut frame: serde_json::Value,
) {
    let kind = frame.get("kind").and_then(|value| value.as_str());
    if kind == Some("E2EE_INIT") {
        let Some(cloud_id) = frame.get("session_id").and_then(|v| v.as_str()) else { return; };
        let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()) else { return; };
        let Some(peer_public) = frame.get("peer_public_key").and_then(|v| v.as_str()) else { return; };
        let Ok((peer, agent_public)) = derive_peer_cipher(
            cloud_id, connection_id, peer_public) else { return; };
        let ring = if let Ok(mut guard) = inner.lock() {
            guard.shares.values_mut().find_map(|share| {
                if share.cloud_session_id != cloud_id { return None; }
                share.peers.insert(connection_id.to_string(), peer.clone());
                Some(Arc::clone(&share.ring))
            })
        } else { None };
        let Some(ring) = ring else { return; };
        let Ok(proof) = ed25519_sign_b64(&device.identity_key,
            &e2ee_context(&device.device_id, cloud_id, connection_id,
                peer_public, &agent_public)) else { return; };
        if send_frame(client, inner, cloud_id, json!({
            "kind": "E2EE_READY", "connection_id": connection_id,
            "agent_public_key": agent_public, "proof": proof,
        })).await.is_err() { return; }
        let (seq, bytes) = ring.lock().map(|r| r.e2ee_snapshot()).unwrap_or_default();
        let snapshot = json!({"kind": "TERMINAL_SNAPSHOT",
            "snapshot_seq": seq, "cols": 80, "rows": 24,
            "data_hex": encode_hex(&bytes)});
        let _ = send_e2ee_frame(client, inner, cloud_id,
            connection_id, &peer, &snapshot).await;
        return;
    }
    if kind == Some("E2EE_CLOSE") {
        if let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()) {
            if let Ok(mut guard) = inner.lock() {
                for share in guard.shares.values_mut() { share.peers.remove(connection_id); }
            }
        }
        return;
    }
    let mut e2ee_reply = None;
    if kind == Some("E2EE_FRAME") {
        let Some(cloud_id) = frame.get("session_id").and_then(|v| v.as_str()).map(str::to_string) else { return; };
        let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()).map(str::to_string) else { return; };
        let peer = inner.lock().ok().and_then(|guard| guard.shares.values()
            .find(|share| share.cloud_session_id == cloud_id)
            .and_then(|share| share.peers.get(&connection_id).cloned()));
        let Some(peer) = peer else { return; };
        let Ok(decrypted) = decrypt_e2ee_frame(
            &cloud_id, &connection_id, &peer, &frame) else { return; };
        frame = decrypted;
        e2ee_reply = Some((connection_id, peer));
    }
    let kind = frame.get("kind").and_then(|value| value.as_str());
    if kind == Some("INPUT") {
        // TX is accepted only from inside a viewer's E2EE envelope; a
        // plaintext INPUT would expose keystrokes to the relay.
        let Some((reply_connection, reply_peer)) = e2ee_reply.as_ref() else {
            return;
        };
        let cloud_id = match frame.get("session_id").and_then(|value| value.as_str()) {
            Some(value) => value.to_string(),
            None => return,
        };
        let validated = if let Ok(mut guard) = inner.lock() {
            guard.shares.iter_mut().find_map(|(local_id, share)| {
                if share.cloud_session_id != cloud_id {
                    return None;
                }
                Some(verify_input_frame(share, device, &frame).map(
                    |(bytes, fence, input_seq)| {
                        (local_id.clone(), bytes, fence, input_seq)
                    },
                ))
            })
        } else {
            None
        };
        if let Some(Ok((local_id, bytes, fence, input_seq))) = validated {
            let byte_count = bytes.len();
            let protocol = inner
                .lock()
                .ok()
                .and_then(|guard| guard.shares.get(&local_id).map(|s| s.protocol));
            if let Some(protocol) = protocol {
                if port.write_tx(protocol, &local_id, &bytes).await.is_err() {
                    return;
                }
                let _ = app.emit(
                    "cloud-bridge-remote-tx",
                    RemoteTxEvent {
                        local_session_id: local_id,
                        byte_count,
                        fence,
                    },
                );
                let ack = json!({
                        "kind": "INPUT_ACK", "input_seq": input_seq,
                        "fence": fence, "byte_count": byte_count
                    });
                let _ = send_e2ee_frame(client, inner, &cloud_id,
                    reply_connection, reply_peer, &ack).await;
            }
        }
        return;
    }
    if kind != Some("SESSION_UNSHARE") {
        return;
    }
    let Some(cloud_id) = frame.get("session_id").and_then(|value| value.as_str())
    else {
        return;
    };
    let removed = if let Ok(mut guard) = inner.lock() {
        let local_id = guard.shares.iter().find_map(|(local_id, share)| {
            (share.cloud_session_id == cloud_id).then(|| local_id.clone())
        });
        local_id.and_then(|local_id| guard.shares.remove(&local_id))
    } else {
        None
    };
    if let Some(share) = removed {
        port.unsubscribe_rx(&share.subscription);
    }
}

/// Clear the connection-level state when a transport ends, so the status
/// pill flips to reconnecting and the supervisor dials again.
fn mark_disconnected(inner: &Arc<Mutex<BridgeInner>>, relay_connection: &str) {
    if let Ok(mut guard) = inner.lock() {
        if guard
            .device
            .as_ref()
            .is_some_and(|configured| configured.relay_connection == relay_connection)
        {
            if let Some(configured) = guard.device.as_mut() {
                configured.relay_connection.clear();
            }
            guard.transport = None;
        }
    }
}

fn spawn_control_pump(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    port: Arc<dyn SharedSessionPort>,
    app: AppHandle,
    device: DeviceConfig,
) {
    tauri::async_runtime::spawn(async move {
        let mut ticks = 0_u8;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let current = inner.lock().ok().and_then(|guard| {
                guard.device.as_ref().map(|d| d.relay_connection.clone())
            });
            if current.as_deref() != Some(device.relay_connection.as_str()) {
                break; // deliberately downgraded to idle, or replaced
            }
            let response = match client
                .get(format!(
                    "{}/console-relay/v1/agent/{}/frames",
                    device.base_url, device.relay_connection
                ))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response,
                _ => break,
            };
            let frames: RelayFramesResponse = match response.json().await {
                Ok(frames) => frames,
                Err(_) => break,
            };
            for frame in frames.frames {
                process_inbound_frame(&client, &inner, &port, &app, &device, frame).await;
            }
            ticks = ticks.wrapping_add(1);
            if ticks % 30 == 0 {
                let heartbeat = client
                    .post(format!(
                        "{}/console-relay/v1/agent/{}/heartbeat",
                        device.base_url, device.relay_connection
                    ))
                    .json(&json!({}))
                    .send()
                    .await;
                if !heartbeat.is_ok_and(|response| response.status().is_success()) {
                    break;
                }
            }
        }
        mark_disconnected(&inner, &device.relay_connection);
    });
}

/// Websocket transport: one writer task (channel -> sink, plus heartbeats)
/// and one reader task feeding `process_inbound_frame`.
fn spawn_ws_pumps(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    port: Arc<dyn SharedSessionPort>,
    app: AppHandle,
    device: DeviceConfig,
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    mut outbound_rx: tokio::sync::mpsc::Receiver<serde_json::Value>,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut sink, mut stream) = socket.split();
    let writer_connection = device.relay_connection.clone();
    let writer_inner = Arc::clone(&inner);
    tauri::async_runtime::spawn(async move {
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            tokio::select! {
                envelope = outbound_rx.recv() => {
                    let Some(envelope) = envelope else { break };
                    let Ok(text) = serde_json::to_string(&envelope) else { continue };
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                _ = heartbeat.tick() => {
                    let ping = serde_json::json!({"kind": "HEARTBEAT"}).to_string();
                    if sink.send(Message::Text(ping.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
        mark_disconnected(&writer_inner, &writer_connection);
    });

    tauri::async_runtime::spawn(async move {
        while let Some(message) = stream.next().await {
            let Ok(message) = message else { break };
            let Message::Text(text) = message else { continue };
            let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
            if frame.get("kind").and_then(|v| v.as_str()) == Some("HEARTBEAT_ACK") {
                continue;
            }
            process_inbound_frame(&client, &inner, &port, &app, &device, frame).await;
        }
        mark_disconnected(&inner, &device.relay_connection);
    });
}

#[tauri::command]
pub async fn cloud_bridge_share_session(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
    local_session_id: String,
    protocol: SessionProtocol,
    label: String,
    tx_policy: TxPolicy,
    tx_expires_in_seconds: Option<u64>,
    rx_ring_bytes: Option<usize>,
) -> Result<ShareView, String> {
    if !state.port.contains(protocol, &local_session_id).await {
        return Err(format!("{:?} session is not connected.", protocol));
    }
    let tx_expires_at = match tx_policy {
        TxPolicy::Temporary => {
            let seconds = tx_expires_in_seconds.unwrap_or(900).clamp(1, 86_400);
            Some(std::time::SystemTime::now() + std::time::Duration::from_secs(seconds))
        }
        _ => None,
    };
    let policy = SessionPolicy {
        tx: tx_policy.clone(),
        tx_expires_at,
        rx_ring_bytes: rx_ring_bytes
            .unwrap_or(DEFAULT_RX_RING_BYTES)
            .clamp(4096, MAX_RX_RING_BYTES),
    };
    let allow_tx = policy.allows_tx(std::time::SystemTime::now());
    let device = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        let device = inner
            .device
            .clone()
            .ok_or_else(|| "Enroll this AuraTerm device first.".to_string())?;
        inner.want_online = true;
        device
    };
    // Idle mode holds no relay connection; sharing dials on demand.
    if device.relay_connection.is_empty() {
        connect_inner(
            state.client.clone(),
            Arc::clone(&state.inner),
            Arc::clone(&state.port),
            app.clone(),
        )
        .await?;
        ensure_supervisor(
            state.client.clone(),
            Arc::clone(&state.inner),
            Arc::clone(&state.port),
            app,
        );
        let connected = state
            .inner
            .lock()
            .map_err(|e| e.to_string())?
            .device
            .as_ref()
            .is_some_and(|d| !d.relay_connection.is_empty());
        if !connected {
            return Err("Cloud Console connection failed.".into());
        }
    }
    if state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .shares
        .contains_key(&local_session_id)
    {
        return Err("This session is already shared.".into());
    }
    let response = state
        .client
        .post(format!(
            "{}/api/v1/auraterm/console/shared-sessions",
            device.base_url
        ))
        .header("Authorization", format!("Bearer {}", device.credential))
        .json(&json!({
            "protocol_version": 1,
            "capabilities": if allow_tx {
                vec!["snapshot_v1", "tx_v1", "e2ee_v1", "multi_viewer_v1"]
            } else {
                vec!["snapshot_v1", "e2ee_v1", "multi_viewer_v1"]
            },
            "tx_allowed": allow_tx,
            "source_protocol": protocol,
            "share_label": label.clone(),
            "tx_policy": tx_policy.clone(),
            "tx_expires_at": system_time_seconds(policy.tx_expires_at),
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let shared: ShareResponse = json_response(response).await?;
    let ring = Arc::new(Mutex::new(RxRing::new(policy.rx_ring_bytes)));
    let output_notify = Arc::new(tokio::sync::Notify::new());
    let callback_ring = Arc::clone(&ring);
    let callback_notify = Arc::clone(&output_notify);
    let subscription = state.port.subscribe_rx(
        &local_session_id,
        Box::new(move |event| {
            if let TerminalEvent::Output(bytes) = event {
                if let Ok(mut ring) = callback_ring.lock() {
                    ring.push(bytes.clone());
                }
                callback_notify.notify_one();
            }
        }),
    );
    let view = ShareView {
        local_session_id: local_session_id.clone(),
        cloud_session_id: shared.session_id.clone(),
        label: label.clone(),
        protocol,
        tx_policy: policy.tx.clone(),
        tx_expires_at: system_time_seconds(policy.tx_expires_at),
        tx_allowed: allow_tx,
    };
    state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .shares
        .insert(
            local_session_id.clone(),
            SharedSession {
                cloud_session_id: shared.session_id,
                label,
                protocol,
                subscription,
                policy,
                ring,
                output_notify,
                peers: HashMap::new(),
                last_fence: 0,
                last_input_seq: 0,
            },
        );
    spawn_output_pump(
        state.client.clone(),
        Arc::clone(&state.inner),
        local_session_id,
    );
    Ok(view)
}

fn system_time_seconds(value: Option<std::time::SystemTime>) -> Option<u64> {
    value.and_then(|time| {
        time.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    })
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn e2ee_context(device_id: &str, session_id: &str, connection_id: &str,
                peer_public_key: &str, agent_public_key: &str) -> String {
    format!("auraxlab-console|e2ee-v1|{device_id}|{session_id}|{connection_id}|{}|{}",
        sha256_hex(peer_public_key), sha256_hex(agent_public_key))
}

fn derive_peer_cipher(session_id: &str, connection_id: &str,
                      peer_public_key: &str) -> Result<(PeerCipher, String), String> {
    let peer_bytes = URL_SAFE_NO_PAD.decode(peer_public_key)
        .map_err(|_| "invalid browser E2EE public key".to_string())?;
    let peer = PublicKey::from_sec1_bytes(&peer_bytes)
        .map_err(|_| "invalid browser E2EE public key".to_string())?;
    let secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
    let agent_public = secret.public_key();
    let shared = secret.diffie_hellman(&peer);
    let mut key = [0_u8; 32];
    let info = format!("auraxlab-console|e2ee-v1|{session_id}|{connection_id}");
    Hkdf::<Sha256>::new(Some(&[0_u8; 32]), shared.raw_secret_bytes().as_slice())
        .expand(info.as_bytes(), &mut key)
        .map_err(|_| "could not derive E2EE key".to_string())?;
    Ok((PeerCipher { key,
        counters: Arc::new(Mutex::new(PeerCounters::default())),
        send_lock: Arc::new(tokio::sync::Mutex::new(())) }, URL_SAFE_NO_PAD.encode(
        agent_public.to_encoded_point(false).as_bytes())))
}

fn encrypt_e2ee_frame(session_id: &str, connection_id: &str,
                      direction: &str, peer: &PeerCipher,
                      frame: &serde_json::Value) -> Result<serde_json::Value, String> {
    let cipher = Aes256Gcm::new_from_slice(&peer.key)
        .map_err(|_| "invalid E2EE key".to_string())?;
    let mut nonce = [0_u8; 12];
    rand::thread_rng().fill(&mut nonce);
    let counter = {
        let mut counters = peer.counters.lock().map_err(|e| e.to_string())?;
        counters.sent += 1;
        counters.sent
    };
    let aad = format!("{session_id}|{connection_id}|{direction}|{counter}");
    let plaintext = serde_json::to_vec(frame).map_err(|e| e.to_string())?;
    let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), Payload {
        msg: &plaintext, aad: aad.as_bytes(),
    }).map_err(|_| "E2EE encryption failed".to_string())?;
    Ok(json!({
        "kind": "E2EE_FRAME", "connection_id": connection_id,
        "counter": counter,
        "nonce": URL_SAFE_NO_PAD.encode(nonce),
        "ciphertext": URL_SAFE_NO_PAD.encode(ciphertext),
    }))
}

fn decrypt_e2ee_frame(session_id: &str, connection_id: &str,
                      peer: &PeerCipher, envelope: &serde_json::Value
                      ) -> Result<serde_json::Value, String> {
    let nonce = URL_SAFE_NO_PAD.decode(envelope.get("nonce")
        .and_then(|v| v.as_str()).ok_or_else(|| "missing E2EE nonce".to_string())?)
        .map_err(|_| "invalid E2EE nonce".to_string())?;
    if nonce.len() != 12 { return Err("invalid E2EE nonce length".into()); }
    let ciphertext = URL_SAFE_NO_PAD.decode(envelope.get("ciphertext")
        .and_then(|v| v.as_str()).ok_or_else(|| "missing E2EE ciphertext".to_string())?)
        .map_err(|_| "invalid E2EE ciphertext".to_string())?;
    let cipher = Aes256Gcm::new_from_slice(&peer.key)
        .map_err(|_| "invalid E2EE key".to_string())?;
    let counter = envelope.get("counter").and_then(|v| v.as_u64())
        .ok_or_else(|| "missing E2EE counter".to_string())?;
    {
        let counters = peer.counters.lock().map_err(|e| e.to_string())?;
        if counter != counters.received + 1 {
            return Err("replayed or out-of-order E2EE frame".into());
        }
    }
    let aad = format!("{session_id}|{connection_id}|browser|{counter}");
    let plaintext = cipher.decrypt(Nonce::from_slice(&nonce), Payload {
        msg: &ciphertext, aad: aad.as_bytes(),
    }).map_err(|_| "E2EE authentication failed".to_string())?;
    let decoded = serde_json::from_slice(&plaintext)
        .map_err(|_| "invalid E2EE payload".to_string())?;
    peer.counters.lock().map_err(|e| e.to_string())?.received = counter;
    Ok(decoded)
}

async fn send_e2ee_frame(client: &reqwest::Client,
                         inner: &Arc<Mutex<BridgeInner>>,
                         session_id: &str, connection_id: &str,
                         peer: &PeerCipher, frame: &serde_json::Value
                         ) -> Result<(), String> {
    let _guard = peer.send_lock.lock().await;
    let envelope = encrypt_e2ee_frame(
        session_id, connection_id, "agent", peer, frame)?;
    send_frame(client, inner, session_id, envelope).await
}

fn spawn_output_pump(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    local_session_id: String,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            let (cloud_id, ring, notify, peers) = match inner.lock().ok().and_then(|guard| {
                guard.shares.get(&local_session_id).map(|share| {
                    (
                        share.cloud_session_id.clone(),
                        Arc::clone(&share.ring),
                        Arc::clone(&share.output_notify),
                        share.peers.clone(),
                    )
                })
            }) {
                Some(values) => values,
                None => break,
            };
            let next = ring.lock().ok().and_then(|ring| {
                ring.chunks
                    .iter()
                    .find(|(seq, _)| *seq > ring.delivered_seq)
                    .cloned()
            });
            let Some((seq, bytes)) = next else {
                notify.notified().await;
                continue;
            };
            let expected = ring
                .lock()
                .map(|ring| ring.delivered_seq + 1)
                .unwrap_or(seq);
            if seq != expected {
                let (snapshot_seq, snapshot_bytes) = ring.lock()
                    .map(|r| r.e2ee_snapshot()).unwrap_or_default();
                let snapshot = json!({"kind": "TERMINAL_SNAPSHOT",
                    "snapshot_seq": snapshot_seq, "cols": 80, "rows": 24,
                    "data_hex": encode_hex(&snapshot_bytes)});
                for (connection_id, peer) in &peers {
                    let _ = send_e2ee_frame(&client, &inner, &cloud_id,
                        connection_id, peer, &snapshot).await;
                }
                if let Ok(mut ring) = ring.lock() { ring.delivered_seq = snapshot_seq; };
                continue;
            }
            // Terminal bytes leave the device only inside per-viewer E2EE
            // envelopes: with no attached peer this loop sends nothing.
            let output = json!({"kind": "OUTPUT", "output_seq": seq,
                "data_hex": encode_hex(&bytes)});
            for (connection_id, peer) in &peers {
                let _ = send_e2ee_frame(&client, &inner, &cloud_id,
                    connection_id, peer, &output).await;
            }
            if let Ok(mut ring) = ring.lock() {
                ring.delivered_seq = ring.delivered_seq.max(seq);
            };
        }
    });
}

async fn recover_shares(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
) {
    let shares = inner
        .lock()
        .map(|guard| {
            guard
                .shares
                .iter()
                .map(|(local, share)| {
                    (
                        local.clone(),
                        share.cloud_session_id.clone(),
                        Arc::clone(&share.ring),
                        share.peers.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (local, cloud, ring, peers) in shares {
        let (seq, bytes) = ring.lock().map(|r| r.e2ee_snapshot()).unwrap_or_default();
        let snapshot = json!({"kind": "TERMINAL_SNAPSHOT",
            "snapshot_seq": seq, "cols": 80, "rows": 24,
            "data_hex": encode_hex(&bytes)});
        let mut recovered = true;
        for (connection_id, peer) in &peers {
            recovered &= send_e2ee_frame(&client, &inner, &cloud,
                connection_id, peer, &snapshot).await.is_ok();
        }
        if recovered {
            spawn_output_pump(client.clone(), Arc::clone(&inner), local);
        }
    }
}

async fn send_frame(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    session_id: &str,
    frame: serde_json::Value,
) -> Result<(), String> {
    let transport = inner
        .lock()
        .map_err(|e| e.to_string())?
        .transport
        .clone()
        .ok_or_else(|| "relay is not connected".to_string())?;
    match transport {
        AgentTransport::Http { base_url, connection } => {
            let response = client
                .post(format!(
                    "{base_url}/console-relay/v1/agent/{connection}/frames"
                ))
                .json(&json!({"session_id": session_id, "frame": frame}))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(format!("Relay rejected RX frame: {}", response.status()))
            }
        }
        AgentTransport::Ws { outbound } => outbound
            .send(json!({"session_id": session_id, "frame": frame}))
            .await
            .map_err(|_| "relay websocket writer is closed".to_string()),
    }
}

#[tauri::command]
pub async fn cloud_bridge_stop_share(
    state: State<'_, CloudBridgeState>,
    local_session_id: String,
) -> Result<(), String> {
    let (device, share) = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        let device = inner
            .device
            .clone()
            .ok_or_else(|| "Device is not enrolled.".to_string())?;
        let share = inner
            .shares
            .remove(&local_session_id)
            .ok_or_else(|| "Serial session is not shared.".to_string())?;
        (device, share)
    };
    state.port.unsubscribe_rx(&share.subscription);
    let response = state
        .client
        .delete(format!(
            "{}/api/v1/auraterm/console/shared-sessions/{}",
            device.base_url, share.cloud_session_id
        ))
        .header("Authorization", format!("Bearer {}", device.credential))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Could not stop cloud share: {}", response.status()))
    }
}

#[tauri::command]
pub fn cloud_bridge_status(state: State<'_, CloudBridgeState>) -> Result<BridgeStatus, String> {
    let inner = state.inner.lock().map_err(|e| e.to_string())?;
    let mut shares = inner
        .shares
        .iter()
        .map(|(local, share)| ShareView {
            local_session_id: local.clone(),
            cloud_session_id: share.cloud_session_id.clone(),
            label: share.label.clone(),
            protocol: share.protocol,
            tx_policy: share.policy.tx.clone(),
            tx_expires_at: system_time_seconds(share.policy.tx_expires_at),
            tx_allowed: share.policy.allows_tx(std::time::SystemTime::now()),
        })
        .collect::<Vec<_>>();
    shares.sort_by(|a, b| a.local_session_id.cmp(&b.local_session_id));
    let connected = inner
        .device
        .as_ref()
        .is_some_and(|d| !d.relay_connection.is_empty());
    let has_shares = !inner.shares.is_empty();
    Ok(BridgeStatus {
        enrolled: inner.device.is_some(),
        connected,
        reconnecting: inner.want_online && !connected && inner.device.is_some()
            && has_shares,
        standby: inner.want_online && !connected && inner.device.is_some()
            && !has_shares,
        pending_user_code: inner
            .pending
            .as_ref()
            .map(|pending| pending.user_code.clone()),
        device_id: inner.device.as_ref().map(|d| d.device_id.clone()),
        device_label: inner.device.as_ref().map(|d| d.label.clone()),
        base_url: inner.device.as_ref().map(|d| d.base_url.clone()),
        fingerprint: inner
            .device
            .as_ref()
            .map(|d| sha256_hex(&d.identity_public)),
        shares,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority_signing_key() -> ed25519_dalek::SigningKey {
        // Deterministic test authority key (fixed seed).
        ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32])
    }

    fn fixture() -> (DeviceConfig, SharedSession) {
        let public = authority_signing_key().verifying_key().to_bytes().to_vec();
        let device = DeviceConfig {
            base_url: String::new(),
            identity_key: String::new(),
            identity_public: String::new(),
            credential: String::new(),
            device_id: "device-1".into(),
            key_version: 1,
            label: String::new(),
            boot_id: String::new(),
            relay_connection: String::new(),
            authority_keys: HashMap::from([("k1".into(), public)]),
        };
        let hub = crate::terminal_event_hub::TerminalEventHub::new();
        let share = SharedSession {
            cloud_session_id: "session-1".into(),
            label: "COM1".into(),
            protocol: SessionProtocol::Serial,
            subscription: hub.subscribe("local-1", |_| {}),
            policy: SessionPolicy {
                tx: TxPolicy::ReadWrite,
                tx_expires_at: None,
                rx_ring_bytes: DEFAULT_RX_RING_BYTES,
            },
            ring: Arc::new(Mutex::new(RxRing::new(DEFAULT_RX_RING_BYTES))),
            output_notify: Arc::new(tokio::sync::Notify::new()),
            peers: HashMap::new(),
            last_fence: 0,
            last_input_seq: 0,
        };
        (device, share)
    }

    fn lease(_device: &DeviceConfig, fence: u64, exp: i64) -> String {
        use ed25519_dalek::Signer;
        let claims = json!({
            "session_id": "session-1", "device_id": "device-1",
            "lease_id": format!("lease-{fence}"), "fence": fence,
            "holder_key_hash": "holder", "permissions": ["input"],
            "exp": exp
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let signature = authority_signing_key()
            .sign(format!("lease-grant.{payload}").as_bytes());
        format!("cav1.k1.{payload}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    fn input(device: &DeviceConfig, fence: u64, seq: u64, exp: i64) -> serde_json::Value {
        json!({"kind": "INPUT", "lease_id": format!("lease-{fence}"),
               "fence": fence, "input_seq": seq, "data_hex": "410d0a",
               "lease_grant": lease(device, fence, exp)})
    }

    #[test]
    fn tx_lease_accepts_bytes_and_rejects_duplicates_and_stale_fences() {
        let (device, mut share) = fixture();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 60;
        let (bytes, fence, seq) =
            verify_input_frame(&mut share, &device, &input(&device, 2, 1, exp)).unwrap();
        assert_eq!(bytes, b"A\r\n");
        assert_eq!((fence, seq), (2, 1));
        assert!(verify_input_frame(&mut share, &device, &input(&device, 2, 1, exp)).is_err());
        assert!(verify_input_frame(&mut share, &device, &input(&device, 1, 2, exp)).is_err());
    }

    #[test]
    fn tx_policy_signature_and_expiry_fail_closed() {
        let (device, mut share) = fixture();
        let expired = input(&device, 1, 1, 0);
        assert!(verify_input_frame(&mut share, &device, &expired).is_err());
        share.policy.tx = TxPolicy::ReadOnly;
        let future = i64::MAX;
        assert!(verify_input_frame(&mut share, &device, &input(&device, 1, 1, future)).is_err());
    }

    #[test]
    fn rx_ring_is_bounded_and_keeps_monotonic_sequences() {
        let mut ring = RxRing::new(4);
        ring.push(b"abc".to_vec());
        ring.push(b"def".to_vec());
        assert_eq!(ring.chunks.front().unwrap().0, 2);
        assert_eq!(ring.snapshot(), (2, b"def".to_vec()));
    }

    #[test]
    fn e2ee_envelope_round_trips_and_authenticates_direction() {
        let browser_secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
        let browser_public = URL_SAFE_NO_PAD.encode(
            browser_secret.public_key().to_encoded_point(false).as_bytes());
        let (agent_peer, agent_public) = derive_peer_cipher(
            "session-1", "connection-1", &browser_public).unwrap();
        let agent_public = PublicKey::from_sec1_bytes(
            &URL_SAFE_NO_PAD.decode(agent_public).unwrap()).unwrap();
        let shared = browser_secret.diffie_hellman(&agent_public);
        let mut browser_key = [0_u8; 32];
        Hkdf::<Sha256>::new(Some(&[0_u8; 32]),
            shared.raw_secret_bytes().as_slice())
            .expand(b"auraxlab-console|e2ee-v1|session-1|connection-1",
                &mut browser_key).unwrap();
        assert_eq!(agent_peer.key, browser_key);

        let input = json!({"kind": "INPUT", "data_hex": "410d0a"});
        let envelope = encrypt_e2ee_frame(
            "session-1", "connection-1", "browser",
            &PeerCipher { key: browser_key,
                counters: Arc::new(Mutex::new(PeerCounters::default())),
                send_lock: Arc::new(tokio::sync::Mutex::new(())) },
            &input).unwrap();
        assert_eq!(decrypt_e2ee_frame(
            "session-1", "connection-1", &agent_peer, &envelope).unwrap(),
            input);
        assert!(decrypt_e2ee_frame(
            "session-1", "connection-1", &agent_peer, &envelope).is_err());
        assert!(decrypt_e2ee_frame(
            "session-1", "other-connection", &agent_peer, &envelope).is_err());
    }
}
