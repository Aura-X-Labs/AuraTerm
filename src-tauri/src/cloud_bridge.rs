//! Phase-1 Cloud Console bridge for existing Serial sessions.
//!
//! The agent owns device enrollment, relay admission and the explicit
//! local-session -> shared-session mapping. RX frames are sent directly from
//! a bounded process-local channel to the relay HTTP adapter. No terminal
//! bytes are written to disk, application settings, or logs.

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
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

#[derive(Clone)]
struct DeviceConfig {
    base_url: String,
    identity_key: String,
    credential: String,
    device_id: String,
    key_version: u32,
    boot_id: String,
    relay_connection: String,
    authority_keys: HashMap<String, Vec<u8>>,
}

#[derive(Clone)]
struct PendingEnrollment {
    base_url: String,
    identity_key: String,
    pkce_verifier: String,
    device_code: String,
}

const DEFAULT_RX_RING_BYTES: usize = 256 * 1024;
const MAX_RX_RING_BYTES: usize = 4 * 1024 * 1024;
const MAX_E2EE_SNAPSHOT_BYTES: usize = 20 * 1024;
const MAX_TX_BYTES: usize = 16 * 1024;

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
    e2ee_enabled: bool,
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

#[derive(Default)]
struct BridgeInner {
    device: Option<DeviceConfig>,
    pending: Option<PendingEnrollment>,
    shares: HashMap<String, SharedSession>,
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
pub struct BridgeStatus {
    enrolled: bool,
    connected: bool,
    pending_user_code: Option<String>,
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
struct GrantResponse {
    relay_grant: String,
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

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK];
    let mut outer_pad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK];
    let mut outer_pad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    outer.finalize().to_vec()
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
    if hmac_sha256(key, signed.as_bytes()) != signature {
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
    let identity_key = random_secret(48);
    let pkce_verifier = random_secret(48);
    let response = state
        .client
        .post(format!("{base_url}/api/v1/auraterm/console/enrollments"))
        .json(&json!({
            "device_public_key": identity_key,
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
        pkce_verifier,
        device_code: result.device_code,
    });
    Ok(EnrollmentView {
        user_code: result.user_code,
        fingerprint: result.fingerprint,
        expires_in: result.expires_in,
    })
}

#[tauri::command]
pub async fn cloud_bridge_redeem_enrollment(
    state: State<'_, CloudBridgeState>,
) -> Result<(), String> {
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
        credential: result.credential,
        device_id: result.device_id,
        key_version: result.key_version,
        boot_id: Uuid::new_v4().to_string(),
        relay_connection: String::new(),
        authority_keys,
    };
    let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
    inner.device = Some(device);
    inner.pending = None;
    Ok(())
}

#[tauri::command]
pub async fn cloud_bridge_connect(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
) -> Result<(), String> {
    let mut device = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .device
        .clone()
        .ok_or_else(|| "Enroll this AuraTerm device first.".to_string())?;
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
    let context = format!(
        "auraxlab-console|connect-grant|{}|{}|{}|{}|relay:inmemory",
        device.device_id, device.boot_id, device.key_version, challenge.nonce
    );
    let proof = hmac_sha256_hex(device.identity_key.as_bytes(), context.as_bytes());
    let grant: GrantResponse = json_response(
        state
            .client
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
    let relay: RelayConnectResponse = json_response(
        state
            .client
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
    state.inner.lock().map_err(|e| e.to_string())?.device = Some(device.clone());
    spawn_control_pump(
        state.client.clone(),
        Arc::clone(&state.inner),
        Arc::clone(&state.port),
        app,
        device.clone(),
    );
    recover_shares(
        state.client.clone(),
        Arc::clone(&state.inner),
        device.clone(),
    )
    .await;
    Ok(())
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
            for mut frame in frames.frames {
                let kind = frame.get("kind").and_then(|value| value.as_str());
                if kind == Some("E2EE_INIT") {
                    let Some(cloud_id) = frame.get("session_id").and_then(|v| v.as_str()) else { continue; };
                    let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()) else { continue; };
                    let Some(peer_public) = frame.get("peer_public_key").and_then(|v| v.as_str()) else { continue; };
                    let Ok((peer, agent_public)) = derive_peer_cipher(
                        cloud_id, connection_id, peer_public) else { continue; };
                    let ring = if let Ok(mut guard) = inner.lock() {
                        guard.shares.values_mut().find_map(|share| {
                            if share.cloud_session_id != cloud_id { return None; }
                            share.peers.insert(connection_id.to_string(), peer.clone());
                            Some(Arc::clone(&share.ring))
                        })
                    } else { None };
                    let Some(ring) = ring else { continue; };
                    let proof = hmac_sha256_hex(device.identity_key.as_bytes(),
                        e2ee_context(&device.device_id, cloud_id, connection_id,
                            peer_public, &agent_public).as_bytes());
                    if send_frame(&client, &device, cloud_id, json!({
                        "kind": "E2EE_READY", "connection_id": connection_id,
                        "agent_public_key": agent_public, "proof": proof,
                    })).await.is_err() { continue; }
                    let (seq, bytes) = ring.lock().map(|r| r.e2ee_snapshot()).unwrap_or_default();
                    let snapshot = json!({"kind": "TERMINAL_SNAPSHOT",
                        "snapshot_seq": seq, "cols": 80, "rows": 24,
                        "data_hex": encode_hex(&bytes)});
                    let _ = send_e2ee_frame(&client, &device, cloud_id,
                        connection_id, &peer, &snapshot).await;
                    continue;
                }
                if kind == Some("E2EE_CLOSE") {
                    if let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()) {
                        if let Ok(mut guard) = inner.lock() {
                            for share in guard.shares.values_mut() { share.peers.remove(connection_id); }
                        }
                    }
                    continue;
                }
                let mut e2ee_reply = None;
                if kind == Some("E2EE_FRAME") {
                    let Some(cloud_id) = frame.get("session_id").and_then(|v| v.as_str()).map(str::to_string) else { continue; };
                    let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()).map(str::to_string) else { continue; };
                    let peer = inner.lock().ok().and_then(|guard| guard.shares.values()
                        .find(|share| share.cloud_session_id == cloud_id)
                        .and_then(|share| share.peers.get(&connection_id).cloned()));
                    let Some(peer) = peer else { continue; };
                    let Ok(decrypted) = decrypt_e2ee_frame(
                        &cloud_id, &connection_id, &peer, &frame) else { continue; };
                    frame = decrypted;
                    e2ee_reply = Some((connection_id, peer));
                }
                let kind = frame.get("kind").and_then(|value| value.as_str());
                if kind == Some("INPUT") {
                    let cloud_id = match frame.get("session_id").and_then(|value| value.as_str()) {
                        Some(value) => value.to_string(),
                        None => continue,
                    };
                    let validated = if let Ok(mut guard) = inner.lock() {
                        guard.shares.iter_mut().find_map(|(local_id, share)| {
                            if share.cloud_session_id != cloud_id {
                                return None;
                            }
                            Some(verify_input_frame(share, &device, &frame).map(
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
                                continue;
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
                            if let Some((connection_id, peer)) = &e2ee_reply {
                                let _ = send_e2ee_frame(&client, &device,
                                    &cloud_id, connection_id, peer, &ack).await;
                            } else {
                                let _ = send_frame(&client, &device,
                                    &cloud_id, ack).await;
                            }
                        }
                    }
                    continue;
                }
                if kind != Some("SESSION_UNSHARE") {
                    continue;
                }
                let Some(cloud_id) = frame.get("session_id").and_then(|value| value.as_str())
                else {
                    continue;
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
        if let Ok(mut guard) = inner.lock() {
            if guard
                .device
                .as_ref()
                .is_some_and(|configured| configured.relay_connection == device.relay_connection)
            {
                if let Some(configured) = guard.device.as_mut() {
                    configured.relay_connection.clear();
                }
            }
        }
    });
}

#[tauri::command]
pub async fn cloud_bridge_share_session(
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
    let device = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .device
        .clone()
        .ok_or_else(|| "Enroll this AuraTerm device first.".to_string())?;
    if device.relay_connection.is_empty() {
        return Err("Connect Cloud Console before sharing.".into());
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
                e2ee_enabled: true,
                peers: HashMap::new(),
                last_fence: 0,
                last_input_seq: 0,
            },
        );
    spawn_output_pump(
        state.client.clone(),
        Arc::clone(&state.inner),
        local_session_id,
        device,
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

async fn send_e2ee_frame(client: &reqwest::Client, device: &DeviceConfig,
                         session_id: &str, connection_id: &str,
                         peer: &PeerCipher, frame: &serde_json::Value
                         ) -> Result<(), String> {
    let _guard = peer.send_lock.lock().await;
    let envelope = encrypt_e2ee_frame(
        session_id, connection_id, "agent", peer, frame)?;
    send_frame(client, device, session_id, envelope).await
}

fn spawn_output_pump(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    local_session_id: String,
    device: DeviceConfig,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            let (cloud_id, ring, notify, e2ee, peers) = match inner.lock().ok().and_then(|guard| {
                guard.shares.get(&local_session_id).map(|share| {
                    (
                        share.cloud_session_id.clone(),
                        Arc::clone(&share.ring),
                        Arc::clone(&share.output_notify),
                        share.e2ee_enabled,
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
                if e2ee {
                    let (snapshot_seq, snapshot_bytes) = ring.lock()
                        .map(|r| r.e2ee_snapshot()).unwrap_or_default();
                    let snapshot = json!({"kind": "TERMINAL_SNAPSHOT",
                        "snapshot_seq": snapshot_seq, "cols": 80, "rows": 24,
                        "data_hex": encode_hex(&snapshot_bytes)});
                    for (connection_id, peer) in &peers {
                        let _ = send_e2ee_frame(&client, &device, &cloud_id,
                            connection_id, peer, &snapshot).await;
                    }
                    if let Ok(mut ring) = ring.lock() { ring.delivered_seq = snapshot_seq; };
                    continue;
                }
                if send_ring_snapshot(&client, &device, &cloud_id, &ring)
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
            let output = json!({"kind": "OUTPUT", "output_seq": seq,
                "data_hex": encode_hex(&bytes)});
            if e2ee {
                for (connection_id, peer) in &peers {
                    let _ = send_e2ee_frame(&client, &device, &cloud_id,
                        connection_id, peer, &output).await;
                }
            } else if send_frame(&client, &device, &cloud_id, output)
                .await.is_err() {
                    break;
            }
            if let Ok(mut ring) = ring.lock() {
                ring.delivered_seq = ring.delivered_seq.max(seq);
            };
        }
    });
}

async fn send_ring_snapshot(
    client: &reqwest::Client,
    device: &DeviceConfig,
    cloud_id: &str,
    ring: &Arc<Mutex<RxRing>>,
) -> Result<(), String> {
    let (seq, bytes) = ring.lock().map_err(|e| e.to_string())?.snapshot();
    send_frame(
        client,
        device,
        cloud_id,
        json!({
            "kind": "TERMINAL_SNAPSHOT", "snapshot_seq": seq,
            "cols": 80, "rows": 24, "data_hex": encode_hex(&bytes),
        }),
    )
    .await?;
    if let Ok(mut ring) = ring.lock() {
        ring.delivered_seq = seq;
    }
    Ok(())
}

async fn recover_shares(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    device: DeviceConfig,
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
                        share.e2ee_enabled,
                        share.peers.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (local, cloud, ring, e2ee, peers) in shares {
        let recovered = if e2ee {
            let (seq, bytes) = ring.lock().map(|r| r.e2ee_snapshot()).unwrap_or_default();
            let snapshot = json!({"kind": "TERMINAL_SNAPSHOT",
                "snapshot_seq": seq, "cols": 80, "rows": 24,
                "data_hex": encode_hex(&bytes)});
            let mut ok = true;
            for (connection_id, peer) in &peers {
                ok &= send_e2ee_frame(&client, &device, &cloud,
                    connection_id, peer, &snapshot).await.is_ok();
            }
            ok
        } else {
            send_ring_snapshot(&client, &device, &cloud, &ring).await.is_ok()
        };
        if recovered {
            spawn_output_pump(client.clone(), Arc::clone(&inner), local, device.clone());
        }
    }
}

async fn send_frame(
    client: &reqwest::Client,
    device: &DeviceConfig,
    session_id: &str,
    frame: serde_json::Value,
) -> Result<(), String> {
    let response = client
        .post(format!(
            "{}/console-relay/v1/agent/{}/frames",
            device.base_url, device.relay_connection
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
    Ok(BridgeStatus {
        enrolled: inner.device.is_some(),
        connected: inner
            .device
            .as_ref()
            .is_some_and(|d| !d.relay_connection.is_empty()),
        pending_user_code: None,
        shares,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DeviceConfig, SharedSession) {
        let key = b"phase-2-test-authority-key".to_vec();
        let device = DeviceConfig {
            base_url: String::new(),
            identity_key: String::new(),
            credential: String::new(),
            device_id: "device-1".into(),
            key_version: 1,
            boot_id: String::new(),
            relay_connection: String::new(),
            authority_keys: HashMap::from([("k1".into(), key)]),
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
            e2ee_enabled: true,
            peers: HashMap::new(),
            last_fence: 0,
            last_input_seq: 0,
        };
        (device, share)
    }

    fn lease(device: &DeviceConfig, fence: u64, exp: i64) -> String {
        let claims = json!({
            "session_id": "session-1", "device_id": "device-1",
            "lease_id": format!("lease-{fence}"), "fence": fence,
            "holder_key_hash": "holder", "permissions": ["input"],
            "exp": exp
        });
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let mac = hmac_sha256(
            &device.authority_keys["k1"],
            format!("lease-grant.{payload}").as_bytes(),
        );
        format!("cav1.k1.{payload}.{}", URL_SAFE_NO_PAD.encode(mac))
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
