//! Phase-1 Cloud Console bridge for existing Serial sessions.
//!
//! The agent owns device enrollment, relay admission and the explicit
//! local-session -> shared-session mapping. RX frames are sent directly from
//! a bounded process-local channel to the relay HTTP adapter. No terminal
//! bytes are written to disk, application settings, or logs.

use crate::terminal_event_hub::{SubscriptionToken, TerminalEvent, TerminalEventHub};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::State;
use tokio::sync::mpsc;
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
}

#[derive(Clone)]
struct PendingEnrollment {
    base_url: String,
    identity_key: String,
    pkce_verifier: String,
    device_code: String,
}

struct SharedSerial {
    cloud_session_id: String,
    label: String,
    subscription: SubscriptionToken,
}

#[derive(Default)]
struct BridgeInner {
    device: Option<DeviceConfig>,
    pending: Option<PendingEnrollment>,
    shares: HashMap<String, SharedSerial>,
}

pub struct CloudBridgeState {
    hub: Arc<TerminalEventHub>,
    inner: Arc<Mutex<BridgeInner>>,
    client: reqwest::Client,
}

impl CloudBridgeState {
    pub fn new(hub: Arc<TerminalEventHub>) -> Self {
        Self {
            hub,
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
    let device = DeviceConfig {
        base_url: pending.base_url,
        identity_key: pending.identity_key,
        credential: result.credential,
        device_id: result.device_id,
        key_version: result.key_version,
        boot_id: Uuid::new_v4().to_string(),
        relay_connection: String::new(),
    };
    let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
    inner.device = Some(device);
    inner.pending = None;
    Ok(())
}

#[tauri::command]
pub async fn cloud_bridge_connect(state: State<'_, CloudBridgeState>) -> Result<(), String> {
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
        Arc::clone(&state.hub),
        device,
    );
    Ok(())
}

fn spawn_control_pump(
    client: reqwest::Client,
    inner: Arc<Mutex<BridgeInner>>,
    hub: Arc<TerminalEventHub>,
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
            for frame in frames.frames {
                if frame.get("kind").and_then(|value| value.as_str()) != Some("SESSION_UNSHARE") {
                    continue;
                }
                let Some(cloud_id) = frame.get("session_id").and_then(|value| value.as_str()) else {
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
                    hub.unsubscribe(&share.subscription);
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
            if guard.device.as_ref().is_some_and(|configured| {
                configured.relay_connection == device.relay_connection
            }) {
                if let Some(configured) = guard.device.as_mut() {
                    configured.relay_connection.clear();
                }
            }
        }
    });
}

#[tauri::command]
pub async fn cloud_bridge_share_serial(
    state: State<'_, CloudBridgeState>,
    serial: State<'_, crate::serial::SerialState>,
    local_session_id: String,
    label: String,
) -> Result<ShareView, String> {
    if !serial.contains(&local_session_id).await {
        return Err("Serial session is not connected.".into());
    }
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
        return Err("This Serial session is already shared.".into());
    }
    let response = state
        .client
        .post(format!(
            "{}/api/v1/auraterm/console/shared-sessions",
            device.base_url
        ))
        .header("Authorization", format!("Bearer {}", device.credential))
        .json(&json!({"protocol_version": 1, "capabilities": ["snapshot_v1"]}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let shared: ShareResponse = json_response(response).await?;
    send_frame(&state.client, &device, &shared.session_id,
        json!({"kind": "TERMINAL_SNAPSHOT", "snapshot_seq": 0, "cols": 80, "rows": 24, "data_hex": ""})).await?;

    let (tx, mut rx) = mpsc::channel::<TerminalEvent>(256);
    let subscription = state.hub.subscribe(&local_session_id, move |event| {
        let _ = tx.try_send(event.clone());
    });
    let client = state.client.clone();
    let pump_device = device.clone();
    let cloud_id = shared.session_id.clone();
    tauri::async_runtime::spawn(async move {
        let mut output_seq = 1_u64;
        while let Some(event) = rx.recv().await {
            match event {
                TerminalEvent::Output(bytes) => {
                    let data_hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    if send_frame(
                        &client,
                        &pump_device,
                        &cloud_id,
                        json!({"kind": "OUTPUT", "output_seq": output_seq, "data_hex": data_hex}),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                    output_seq += 1;
                }
                TerminalEvent::Exit(_) => break,
            }
        }
    });
    let view = ShareView {
        local_session_id: local_session_id.clone(),
        cloud_session_id: shared.session_id.clone(),
        label: label.clone(),
    };
    state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .shares
        .insert(
            local_session_id,
            SharedSerial {
                cloud_session_id: shared.session_id,
                label,
                subscription,
            },
        );
    Ok(view)
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
    state.hub.unsubscribe(&share.subscription);
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
