//! Remote Assist — host side (design `docs/plans/remote-assist-design.md` §9).
//!
//! The host mints a code whose secret segment lives only in this process,
//! registers metadata with AuraXLab, and then authenticates every guest
//! with SPAKE2 through the relay. After the handshake the guest is a peer
//! of the existing E2EE envelope (`e2ee::PeerCipher`, direction host/guest)
//! and the shared session's RX ring is fanned out to it exactly like a
//! Cloud Console viewer. Every authorization decision — admit, deny, grant
//! control, revoke, kick — is taken here, never by the server.

use crate::assist::{self, PROTOCOL_VERSION};
use crate::cloud_bridge::{
    connect_inner, encode_hex, ensure_supervisor, json_response, send_frame, system_time_seconds, BridgeInner,
    CloudBridgeState, DeviceConfig, RemoteTxEvent, RxRing, DEFAULT_RX_RING_BYTES, MAX_TX_BYTES,
};
use crate::e2ee::{PeerCipher, DIRECTION_GUEST, DIRECTION_HOST};
use crate::pake::Spake2Keys;
use crate::shared_session::{SessionProtocol, SharedSessionPort};
use crate::terminal_event_hub::{SubscriptionToken, TerminalEvent};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager, State};
use zeroize::Zeroizing;

const APPROVAL_TIMEOUT_SECS: u64 = 60;
const DEFAULT_CONTROL_SECS: u64 = 30 * 60;
const MAX_FAILED_ATTEMPTS: u32 = 3;
const HOUSEKEEPING_SECS: u64 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlPolicy {
    ViewOnly,
    OnRequest,
    AutoGrant,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistPolicy {
    pub control: ControlPolicy,
    pub approval_required: bool,
    pub single_use: bool,
    pub max_guests: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GuestState {
    PendingApproval,
    Viewer,
    Controller { expires_at: Option<SystemTime> },
}

pub(crate) struct Guest {
    cipher: PeerCipher,
    state: GuestState,
    client: String,
    display_name: String,
    fingerprint: String,
    joined_at: SystemTime,
    control_requested: bool,
    hello_seen: bool,
}

impl Guest {
    fn is_active(&self) -> bool {
        !matches!(self.state, GuestState::PendingApproval)
    }

    fn role_name(&self) -> &'static str {
        match self.state {
            GuestState::PendingApproval => "pending",
            GuestState::Viewer => "viewer",
            GuestState::Controller { .. } => "controller",
        }
    }
}

pub(crate) struct AssistHost {
    pub(crate) assist_id: String,
    route_code: String,
    secret: Zeroizing<String>,
    w: Zeroizing<[u8; 32]>,
    pub(crate) local_session_id: String,
    protocol: SessionProtocol,
    label: String,
    subscription: SubscriptionToken,
    ring: Arc<Mutex<RxRing>>,
    output_notify: Arc<tokio::sync::Notify>,
    policy: AssistPolicy,
    follow_active_tab: bool,
    guests: HashMap<String, Guest>,
    pending_pake: HashMap<String, Spake2Keys>,
    failed_attempts: u32,
    fence: u64,
    join_expires_at: SystemTime,
    join_url_base: String,
    created_at: SystemTime,
    locked: bool,
}

impl AssistHost {
    /// Ciphers of guests that finished admission (viewers + controllers).
    pub(crate) fn active_ciphers(&self) -> Vec<(String, PeerCipher)> {
        self.guests
            .iter()
            .filter(|(_, guest)| guest.is_active())
            .map(|(id, guest)| (id.clone(), guest.cipher.clone()))
            .collect()
    }

    pub(crate) fn status_view(&self) -> AssistStatusView {
        let now = SystemTime::now();
        let mut guests: Vec<GuestView> = self
            .guests
            .iter()
            .map(|(connection_id, guest)| GuestView {
                connection_id: connection_id.clone(),
                role: guest.role_name().to_string(),
                client: guest.client.clone(),
                display_name: guest.display_name.clone(),
                fingerprint: guest.fingerprint.clone(),
                joined_at: system_time_seconds(Some(guest.joined_at)),
                control_expires_at: match guest.state {
                    GuestState::Controller { expires_at } => system_time_seconds(expires_at),
                    _ => None,
                },
                control_requested: guest.control_requested,
            })
            .collect();
        guests.sort_by(|a, b| a.joined_at.cmp(&b.joined_at));
        AssistStatusView {
            assist_id: self.assist_id.clone(),
            code: assist::format_code(&self.route_code, &self.secret),
            link: format!("{}#{}", self.join_url_base, assist::format_code(&self.route_code, &self.secret)),
            local_session_id: self.local_session_id.clone(),
            protocol: self.protocol,
            label: self.label.clone(),
            policy: self.policy.clone(),
            follow_active_tab: self.follow_active_tab,
            join_expires_at: system_time_seconds(Some(self.join_expires_at)).unwrap_or(0),
            join_open: now < self.join_expires_at && (!self.policy.single_use || self.guests.is_empty()),
            created_at: system_time_seconds(Some(self.created_at)).unwrap_or(0),
            failed_attempts: self.failed_attempts,
            fence: self.fence,
            locked: self.locked,
            guests,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestView {
    pub connection_id: String,
    /// "pending" | "viewer" | "controller"
    pub role: String,
    pub client: String,
    pub display_name: String,
    pub fingerprint: String,
    pub joined_at: Option<u64>,
    pub control_expires_at: Option<u64>,
    pub control_requested: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistStatusView {
    pub assist_id: String,
    pub code: String,
    pub link: String,
    pub local_session_id: String,
    pub protocol: SessionProtocol,
    pub label: String,
    pub policy: AssistPolicy,
    pub follow_active_tab: bool,
    pub join_expires_at: u64,
    pub join_open: bool,
    pub created_at: u64,
    pub failed_attempts: u32,
    pub fence: u64,
    pub locked: bool,
    pub guests: Vec<GuestView>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct KnockEvent {
    connection_id: String,
    display_name: String,
    client: String,
    fingerprint: String,
    /// "join" | "control"
    kind: String,
}

#[derive(Deserialize)]
struct CreateResponse {
    assist_id: String,
    route_code: String,
    join_expires_at: Option<String>,
    #[serde(default)]
    join_url_base: Option<String>,
}

fn now_secs() -> u64 {
    system_time_seconds(Some(SystemTime::now())).unwrap_or(0)
}

fn device_of(inner: &Arc<Mutex<BridgeInner>>) -> Result<DeviceConfig, String> {
    inner
        .lock()
        .map_err(|e| e.to_string())?
        .device
        .clone()
        .ok_or_else(|| "Sign in and bind this device first.".to_string())
}

async fn device_post(client: &reqwest::Client, device: &DeviceConfig, path: &str, body: serde_json::Value) -> Result<reqwest::Response, String> {
    client
        .post(format!("{}/api/v1/auraterm/console{path}", device.base_url))
        .header("Authorization", format!("Bearer {}", device.credential))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())
}

/// Fire-and-forget metadata event toward the control plane. Never carries
/// terminal payload, PAKE values or the code.
fn report_event(client: reqwest::Client, inner: Arc<Mutex<BridgeInner>>, assist_id: String, kind: &'static str, connection_id: String, extra: serde_json::Value) {
    tauri::async_runtime::spawn(async move {
        let Ok(device) = device_of(&inner) else { return };
        let mut payload = json!({"connection_id": connection_id});
        if let (Some(target), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let _ = device_post(
            &client,
            &device,
            &format!("/assist/sessions/{assist_id}/events"),
            json!({"kind": kind, "payload": payload}),
        )
        .await;
    });
}

/// Encrypt `frame` for one guest and hand it to the relay.
pub(crate) async fn send_to_guest(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    assist_id: &str,
    connection_id: &str,
    cipher: &PeerCipher,
    frame: &serde_json::Value,
) -> Result<(), String> {
    let _guard = cipher.send_lock.lock().await;
    let envelope = cipher.encrypt(assist_id, connection_id, DIRECTION_HOST, frame)?;
    send_frame(client, inner, assist_id, envelope).await
}

fn state_frame(assist: &AssistHost, guest: &Guest, size: (u16, u16)) -> serde_json::Value {
    let (state, role) = match guest.state {
        GuestState::PendingApproval => ("pending_approval", "viewer"),
        GuestState::Viewer => ("active", "viewer"),
        GuestState::Controller { .. } => ("active", "controller"),
    };
    json!({
        "kind": "ASSIST_STATE", "state": state, "role": role,
        "cols": size.0, "rows": size.1,
        "host_label": assist.label, "fingerprint": guest.fingerprint,
        "control_policy": assist.policy.control, "fence": assist.fence,
        "control_expires_at": match guest.state {
            GuestState::Controller { expires_at } => system_time_seconds(expires_at),
            _ => None,
        },
    })
}

fn snapshot_frame(ring: &Arc<Mutex<RxRing>>, size: (u16, u16)) -> serde_json::Value {
    let (seq, bytes) = ring.lock().map(|r| r.e2ee_snapshot()).unwrap_or_default();
    json!({"kind": "TERMINAL_SNAPSHOT", "snapshot_seq": seq,
        "cols": size.0, "rows": size.1, "data_hex": encode_hex(&bytes)})
}

fn subscribe_ring(port: &Arc<dyn SharedSessionPort>, local_session_id: &str, notify: &Arc<tokio::sync::Notify>) -> (SubscriptionToken, Arc<Mutex<RxRing>>) {
    let ring = Arc::new(Mutex::new(RxRing::new(DEFAULT_RX_RING_BYTES)));
    let callback_ring = Arc::clone(&ring);
    let callback_notify = Arc::clone(notify);
    let subscription = port.subscribe_rx(
        local_session_id,
        Box::new(move |event| {
            if let TerminalEvent::Output(bytes) = event {
                if let Ok(mut ring) = callback_ring.lock() {
                    ring.push(bytes.clone());
                }
                callback_notify.notify_one();
            }
        }),
    );
    (subscription, ring)
}

// ── commands ────────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistStartView {
    pub assist_id: String,
    pub code: String,
    pub link: String,
    pub join_expires_at: u64,
}

#[tauri::command]
pub async fn assist_start(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
    local_session_id: String,
    protocol: SessionProtocol,
    label: String,
    control_policy: ControlPolicy,
    approval_required: Option<bool>,
    single_use: Option<bool>,
    max_guests: Option<usize>,
    join_ttl_seconds: Option<u64>,
    follow_active_tab: Option<bool>,
) -> Result<AssistStartView, String> {
    if !state.port.contains(protocol, &local_session_id).await {
        return Err(format!("{:?} session is not connected.", protocol));
    }
    if state.inner.lock().map_err(|e| e.to_string())?.assist.is_some() {
        return Err("A remote assist session is already running; stop it first.".into());
    }
    let device = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        let device = inner.device.clone().ok_or_else(|| "Sign in and bind this device first.".to_string())?;
        inner.want_online = true;
        device
    };
    if device.relay_connection.is_empty() {
        connect_inner(state.client.clone(), Arc::clone(&state.inner), Arc::clone(&state.port), app.clone()).await?;
        ensure_supervisor(state.client.clone(), Arc::clone(&state.inner), Arc::clone(&state.port), app.clone());
        let connected = state
            .inner
            .lock()
            .map_err(|e| e.to_string())?
            .device
            .as_ref()
            .is_some_and(|d| !d.relay_connection.is_empty());
        if !connected {
            return Err("Could not reach the AuraXLab relay.".into());
        }
    } else {
        ensure_supervisor(state.client.clone(), Arc::clone(&state.inner), Arc::clone(&state.port), app.clone());
    }
    let policy = AssistPolicy {
        control: control_policy,
        approval_required: approval_required.unwrap_or(false),
        single_use: single_use.unwrap_or(true),
        max_guests: max_guests.unwrap_or(1).clamp(1, 3),
    };
    let join_ttl = join_ttl_seconds.unwrap_or(600).clamp(60, 3600);
    let label = label.chars().filter(|c| !c.is_control()).take(96).collect::<String>();
    let response = device_post(
        &state.client,
        &device,
        "/assist/sessions",
        json!({
            "protocol_version": PROTOCOL_VERSION,
            "source_protocol": protocol,
            "share_label": label,
            "control_policy": control_policy,
            "approval_required": policy.approval_required,
            "single_use": policy.single_use,
            "max_guests": policy.max_guests,
            "join_ttl": join_ttl,
        }),
    )
    .await?;
    let created: CreateResponse = json_response(response).await?;
    let secret = assist::generate_secret();
    let w = assist::derive_w(&secret, &created.assist_id);
    let join_expires_at = created
        .join_expires_at
        .as_deref()
        .and_then(parse_iso_seconds)
        .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap_or_else(|| SystemTime::now() + Duration::from_secs(join_ttl));
    let output_notify = Arc::new(tokio::sync::Notify::new());
    let (subscription, ring) = subscribe_ring(&state.port, &local_session_id, &output_notify);
    let host = AssistHost {
        assist_id: created.assist_id.clone(),
        route_code: created.route_code.clone(),
        secret,
        w,
        local_session_id: local_session_id.clone(),
        protocol,
        label,
        subscription,
        ring,
        output_notify,
        policy,
        follow_active_tab: follow_active_tab.unwrap_or(false),
        guests: HashMap::new(),
        pending_pake: HashMap::new(),
        failed_attempts: 0,
        fence: 0,
        join_expires_at,
        join_url_base: created.join_url_base.unwrap_or_else(|| "https://auraxlab.com/assist".to_string()),
        created_at: SystemTime::now(),
        locked: false,
    };
    let view = AssistStartView {
        assist_id: host.assist_id.clone(),
        code: assist::format_code(&host.route_code, &host.secret),
        link: format!("{}#{}", host.join_url_base, assist::format_code(&host.route_code, &host.secret)),
        join_expires_at: system_time_seconds(Some(join_expires_at)).unwrap_or(0),
    };
    state.inner.lock().map_err(|e| e.to_string())?.assist = Some(host);
    spawn_output_pump(state.client.clone(), Arc::clone(&state.inner), created.assist_id.clone());
    spawn_housekeeping(state.client.clone(), Arc::clone(&state.inner), app.clone(), created.assist_id);
    let _ = app.emit("assist-changed", ());
    Ok(view)
}

/// "2026-08-23T12:00:00Z" → epoch seconds (good enough for a deadline).
fn parse_iso_seconds(value: &str) -> Option<u64> {
    let value = value.trim_end_matches('Z');
    let (date, time) = value.split_once('T')?;
    let mut date_parts = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (date_parts.next()??, date_parts.next()??, date_parts.next()??);
    let mut time_parts = time.split(':').map(|p| p.split('.').next().and_then(|q| q.parse::<i64>().ok()));
    let (hour, minute, second) = (time_parts.next()??, time_parts.next()??, time_parts.next()??);
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(secs).ok()
}

#[tauri::command]
pub async fn assist_stop(app: AppHandle, state: State<'_, CloudBridgeState>, reason: Option<String>) -> Result<(), String> {
    stop_assist(&state.client, &state.inner, &state.port, &app, reason.unwrap_or_else(|| "host_ended".into()), true).await
}

async fn stop_assist<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle<R>,
    reason: String,
    tell_server: bool,
) -> Result<(), String> {
    let (device, assist) = {
        let mut guard = inner.lock().map_err(|e| e.to_string())?;
        let device = guard.device.clone();
        let assist = guard.assist.take().ok_or_else(|| "No remote assist session is running.".to_string())?;
        (device, assist)
    };
    port.unsubscribe_rx(&assist.subscription);
    assist.output_notify.notify_one();
    let _ = app.emit("assist-ended", json!({"reason": reason}));
    let _ = app.emit("assist-changed", ());
    if tell_server {
        if let Some(device) = device {
            let response = client
                .delete(format!("{}/api/v1/auraterm/console/assist/sessions/{}", device.base_url, assist.assist_id))
                .header("Authorization", format!("Bearer {}", device.credential))
                .json(&json!({"reason": reason}))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !response.status().is_success() && response.status().as_u16() != 404 {
                return Err(format!("Could not end remote assist: {}", response.status()));
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn assist_status(state: State<'_, CloudBridgeState>) -> Result<Option<AssistStatusView>, String> {
    Ok(state.inner.lock().map_err(|e| e.to_string())?.assist.as_ref().map(|a| a.status_view()))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JoinDecision {
    AllowView,
    AllowControl,
    Deny,
}

#[tauri::command]
pub async fn assist_respond_join(app: AppHandle, state: State<'_, CloudBridgeState>, connection_id: String, decision: JoinDecision) -> Result<(), String> {
    match decision {
        JoinDecision::Deny => deny_guest(&state.client, &state.inner, &app, &connection_id, "denied").await,
        JoinDecision::AllowView => set_role(&state.client, &state.inner, &app, &connection_id, GuestState::Viewer, true).await,
        JoinDecision::AllowControl => {
            let expires = Some(SystemTime::now() + Duration::from_secs(DEFAULT_CONTROL_SECS));
            set_role(&state.client, &state.inner, &app, &connection_id, GuestState::Controller { expires_at: expires }, true).await
        }
    }
}

#[tauri::command]
pub async fn assist_set_role(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
    connection_id: String,
    role: String,
    duration_seconds: Option<u64>,
) -> Result<(), String> {
    let target = match role.as_str() {
        "viewer" => GuestState::Viewer,
        "controller" => GuestState::Controller {
            expires_at: duration_seconds.map(|s| SystemTime::now() + Duration::from_secs(s.clamp(60, 8 * 3600))).or(Some(SystemTime::now() + Duration::from_secs(DEFAULT_CONTROL_SECS))),
        },
        _ => return Err("role must be viewer or controller".into()),
    };
    set_role(&state.client, &state.inner, &app, &connection_id, target, false).await
}

#[tauri::command]
pub async fn assist_kick(app: AppHandle, state: State<'_, CloudBridgeState>, connection_id: String) -> Result<(), String> {
    deny_guest(&state.client, &state.inner, &app, &connection_id, "kicked").await
}

/// Panic button: every controller drops to viewer and the fence advances so
/// in-flight INPUT frames are discarded.
#[tauri::command]
pub async fn assist_revoke_all_control(app: AppHandle, state: State<'_, CloudBridgeState>) -> Result<(), String> {
    let controllers: Vec<String> = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .assist
        .as_ref()
        .map(|a| {
            a.guests
                .iter()
                .filter(|(_, g)| matches!(g.state, GuestState::Controller { .. }))
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    for connection_id in controllers {
        set_role(&state.client, &state.inner, &app, &connection_id, GuestState::Viewer, false).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn assist_switch_session(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
    local_session_id: String,
    protocol: SessionProtocol,
    label: String,
) -> Result<(), String> {
    if !state.port.contains(protocol, &local_session_id).await {
        return Err(format!("{:?} session is not connected.", protocol));
    }
    let (assist_id, old_subscription, targets, size, ring) = {
        let mut guard = state.inner.lock().map_err(|e| e.to_string())?;
        let size = guard.terminal_size(&local_session_id);
        let assist = guard.assist.as_mut().ok_or_else(|| "No remote assist session is running.".to_string())?;
        if assist.local_session_id == local_session_id {
            return Ok(());
        }
        let (subscription, ring) = subscribe_ring(&state.port, &local_session_id, &assist.output_notify);
        let old = std::mem::replace(&mut assist.subscription, subscription);
        assist.ring = Arc::clone(&ring);
        assist.local_session_id = local_session_id;
        assist.protocol = protocol;
        assist.label = label.chars().filter(|c| !c.is_control()).take(96).collect();
        (assist.assist_id.clone(), old, assist.active_ciphers(), size, ring)
    };
    state.port.unsubscribe_rx(&old_subscription);
    let switched = json!({"kind": "ASSIST_SESSION_SWITCHED", "cols": size.0, "rows": size.1});
    let snapshot = snapshot_frame(&ring, size);
    for (connection_id, cipher) in &targets {
        let _ = send_to_guest(&state.client, &state.inner, &assist_id, connection_id, cipher, &switched).await;
        let _ = send_to_guest(&state.client, &state.inner, &assist_id, connection_id, cipher, &snapshot).await;
    }
    if let Ok(mut ring) = ring.lock() {
        ring.delivered_seq = ring.chunks.back().map(|(seq, _)| *seq).unwrap_or(0);
    }
    let _ = app.emit("assist-changed", ());
    Ok(())
}

#[tauri::command]
pub fn assist_set_follow_active_tab(state: State<'_, CloudBridgeState>, follow: bool) -> Result<(), String> {
    if let Some(assist) = state.inner.lock().map_err(|e| e.to_string())?.assist.as_mut() {
        assist.follow_active_tab = follow;
    }
    Ok(())
}

// ── guest lifecycle helpers ─────────────────────────────────────────────────

async fn set_role<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    app: &AppHandle<R>,
    connection_id: &str,
    target: GuestState,
    from_pending: bool,
) -> Result<(), String> {
    let (assist_id, cipher, frames, event, size, ring) = {
        let mut guard = inner.lock().map_err(|e| e.to_string())?;
        let size = guard
            .assist
            .as_ref()
            .map(|a| guard.terminal_size(&a.local_session_id))
            .unwrap_or((80, 24));
        let assist = guard.assist.as_mut().ok_or_else(|| "No remote assist session is running.".to_string())?;
        let was_pending = assist
            .guests
            .get(connection_id)
            .map(|g| matches!(g.state, GuestState::PendingApproval))
            .unwrap_or(false);
        if from_pending && !was_pending {
            return Err("Guest is no longer waiting for approval.".into());
        }
        let mut frames = Vec::new();
        let mut event = None;
        let granting = matches!(target, GuestState::Controller { .. });
        if granting {
            if assist.policy.control == ControlPolicy::ViewOnly {
                return Err("This assist session is view-only.".into());
            }
            // One controller at a time: demote the current one first.
            let current: Vec<String> = assist
                .guests
                .iter()
                .filter(|(id, g)| id.as_str() != connection_id && matches!(g.state, GuestState::Controller { .. }))
                .map(|(id, _)| id.clone())
                .collect();
            for other in current {
                if let Some(guest) = assist.guests.get_mut(&other) {
                    guest.state = GuestState::Viewer;
                    frames.push((other.clone(), guest.cipher.clone(), json!({"kind": "CONTROL_REVOKE", "reason": "replaced"})));
                }
            }
            assist.fence += 1;
        }
        let fence = assist.fence;
        let (had_control, cipher) = {
            let guest = assist.guests.get_mut(connection_id).ok_or_else(|| "Unknown guest.".to_string())?;
            let had_control = matches!(guest.state, GuestState::Controller { .. });
            guest.state = target.clone();
            guest.control_requested = false;
            (had_control, guest.cipher.clone())
        };
        match &target {
            GuestState::Controller { expires_at } => {
                frames.push((
                    connection_id.to_string(),
                    cipher.clone(),
                    json!({"kind": "CONTROL_GRANT", "fence": fence, "expires_at": system_time_seconds(*expires_at)}),
                ));
                event = Some("control_granted");
            }
            GuestState::Viewer if had_control => {
                assist.fence += 1;
                frames.push((connection_id.to_string(), cipher.clone(), json!({"kind": "CONTROL_REVOKE", "reason": "revoked"})));
                event = Some("control_revoked");
            }
            _ => {}
        }
        let guest = assist.guests.get(connection_id).ok_or_else(|| "Unknown guest.".to_string())?;
        let state_frame = state_frame(assist, guest, size);
        (assist.assist_id.clone(), cipher, (frames, state_frame), event, size, Arc::clone(&assist.ring))
    };
    let (extra_frames, state_frame) = frames;
    for (id, peer_cipher, frame) in &extra_frames {
        let _ = send_to_guest(client, inner, &assist_id, id, peer_cipher, frame).await;
    }
    let _ = send_to_guest(client, inner, &assist_id, connection_id, &cipher, &state_frame).await;
    if from_pending {
        let snapshot = snapshot_frame(&ring, size);
        let _ = send_to_guest(client, inner, &assist_id, connection_id, &cipher, &snapshot).await;
    }
    if let Some(kind) = event {
        report_event(client.clone(), Arc::clone(inner), assist_id, kind, connection_id.to_string(), json!({}));
    }
    let _ = app.emit("assist-changed", ());
    Ok(())
}

async fn deny_guest<R: tauri::Runtime>(client: &reqwest::Client, inner: &Arc<Mutex<BridgeInner>>, app: &AppHandle<R>, connection_id: &str, reason: &str) -> Result<(), String> {
    let (assist_id, removed) = {
        let mut guard = inner.lock().map_err(|e| e.to_string())?;
        let assist = guard.assist.as_mut().ok_or_else(|| "No remote assist session is running.".to_string())?;
        let removed = assist.guests.remove(connection_id);
        assist.pending_pake.remove(connection_id);
        if removed.as_ref().is_some_and(|g| matches!(g.state, GuestState::Controller { .. })) {
            assist.fence += 1;
        }
        (assist.assist_id.clone(), removed)
    };
    if let Some(guest) = removed {
        let frame = json!({"kind": "ASSIST_STATE", "state": "denied", "role": "viewer", "reason": reason});
        let _ = send_to_guest(client, inner, &assist_id, connection_id, &guest.cipher, &frame).await;
        let kind = if guest.is_active() { "guest_kicked" } else { "guest_denied" };
        // The server closes the relay connection for us.
        report_event(client.clone(), Arc::clone(inner), assist_id, kind, connection_id.to_string(), json!({}));
    }
    let _ = app.emit("assist-changed", ());
    Ok(())
}

// ── inbound frames ──────────────────────────────────────────────────────────

fn decode_b64(value: Option<&serde_json::Value>) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(value?.as_str()?).ok()
}

/// Returns true when the frame belonged to the assist session (handled or
/// deliberately dropped), false to let the Cloud Console path see it.
pub(crate) async fn handle_frame<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle<R>,
    frame: &serde_json::Value,
) -> bool {
    let kind = frame.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let session_id = frame.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    let assist_id = match inner.lock().ok().and_then(|g| g.assist.as_ref().map(|a| a.assist_id.clone())) {
        Some(id) => id,
        None => return matches!(kind, "ASSIST_INIT" | "PAKE_A" | "PAKE_CONFIRM"),
    };
    if matches!(kind, "ASSIST_INIT" | "PAKE_A" | "PAKE_CONFIRM") {
        if session_id != assist_id {
            return true; // stale frame for a session we no longer host
        }
    } else if session_id != assist_id {
        return false;
    }
    let connection_id = frame.get("connection_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match kind {
        "ASSIST_INIT" => {
            // A guest was admitted by the relay; nothing to do until PAKE_A.
            let version = frame.get("protocol_version").and_then(|v| v.as_u64()).unwrap_or(0);
            if version != u64::from(PROTOCOL_VERSION) {
                let _ = send_frame(client, inner, &assist_id, json!({"kind": "PAKE_FAILED", "connection_id": connection_id})).await;
            }
            true
        }
        "PAKE_A" => {
            handle_pake_a(client, inner, app, &assist_id, &connection_id, frame).await;
            true
        }
        "PAKE_CONFIRM" => {
            handle_pake_confirm(client, inner, app, &assist_id, &connection_id, frame).await;
            true
        }
        "E2EE_FRAME" => {
            handle_guest_envelope(client, inner, port, app, &assist_id, &connection_id, frame).await;
            true
        }
        "E2EE_CLOSE" => {
            handle_guest_close(client, inner, app, &assist_id, &connection_id).await;
            true
        }
        "SESSION_UNSHARE" | "SESSION_END" => {
            let reason = frame.get("reason").and_then(|v| v.as_str()).unwrap_or("server_ended").to_string();
            let _ = stop_assist(client, inner, port, app, reason, false).await;
            true
        }
        _ => true,
    }
}

async fn handle_pake_a<R: tauri::Runtime>(client: &reqwest::Client, inner: &Arc<Mutex<BridgeInner>>, app: &AppHandle<R>, assist_id: &str, connection_id: &str, frame: &serde_json::Value) {
    let failed = json!({"kind": "PAKE_FAILED", "connection_id": connection_id});
    let version = frame.get("protocol_version").and_then(|v| v.as_u64()).unwrap_or(0);
    let pa = decode_b64(frame.get("pa"));
    let outcome = {
        let Ok(mut guard) = inner.lock() else { return };
        let Some(assist) = guard.assist.as_mut() else { return };
        if assist.locked || version != u64::from(PROTOCOL_VERSION) || !(now_secs() < system_time_seconds(Some(assist.join_expires_at)).unwrap_or(0) || !assist.guests.is_empty()) {
            None
        } else if assist.policy.single_use && !assist.guests.is_empty() {
            None
        } else if assist.guests.len() >= assist.policy.max_guests {
            None
        } else {
            let host = assist::host_pake(&assist.w, assist_id, connection_id).ok();
            match (host, pa) {
                (Some(host), Some(pa)) => {
                    let share = *host.share();
                    match host.finish(&pa) {
                        Ok(keys) => {
                            let confirm = URL_SAFE_NO_PAD.encode(keys.own_confirmation());
                            assist.pending_pake.insert(connection_id.to_string(), keys);
                            Some(json!({"kind": "PAKE_B", "connection_id": connection_id,
                                "pb": URL_SAFE_NO_PAD.encode(share), "confirm_b": confirm}))
                        }
                        Err(_) => {
                            assist.failed_attempts += 1;
                            None
                        }
                    }
                }
                _ => {
                    assist.failed_attempts += 1;
                    None
                }
            }
        }
    };
    match outcome {
        Some(pake_b) => {
            let _ = send_frame(client, inner, assist_id, pake_b).await;
        }
        None => {
            let _ = send_frame(client, inner, assist_id, failed).await;
            report_event(client.clone(), Arc::clone(inner), assist_id.to_string(), "pake_failed", connection_id.to_string(), json!({}));
            check_lock(client, inner, app, assist_id).await;
        }
    }
}

async fn handle_pake_confirm<R: tauri::Runtime>(client: &reqwest::Client, inner: &Arc<Mutex<BridgeInner>>, app: &AppHandle<R>, assist_id: &str, connection_id: &str, frame: &serde_json::Value) {
    let confirm = decode_b64(frame.get("confirm_a"));
    let admitted = {
        let Ok(mut guard) = inner.lock() else { return };
        let size = guard
            .assist
            .as_ref()
            .map(|a| guard.terminal_size(&a.local_session_id))
            .unwrap_or((80, 24));
        let Some(assist) = guard.assist.as_mut() else { return };
        let Some(keys) = assist.pending_pake.remove(connection_id) else { return };
        let ok = confirm.as_deref().is_some_and(|c| keys.verify_peer_confirmation(c));
        if !ok {
            assist.failed_attempts += 1;
            None
        } else {
            let key = assist::session_key(&keys, assist_id, connection_id);
            let fingerprint = assist::fingerprint(&keys);
            let state = if assist.policy.approval_required {
                GuestState::PendingApproval
            } else if assist.policy.control == ControlPolicy::AutoGrant
                && !assist.guests.values().any(|g| matches!(g.state, GuestState::Controller { .. }))
            {
                assist.fence += 1;
                GuestState::Controller {
                    expires_at: Some(SystemTime::now() + Duration::from_secs(DEFAULT_CONTROL_SECS)),
                }
            } else {
                GuestState::Viewer
            };
            let guest = Guest {
                cipher: PeerCipher::new(*key),
                state,
                client: "unknown".into(),
                display_name: String::new(),
                fingerprint,
                joined_at: SystemTime::now(),
                control_requested: false,
                hello_seen: false,
            };
            let state_frame = state_frame(assist, &guest, size);
            let snapshot = guest.is_active().then(|| snapshot_frame(&assist.ring, size));
            let cipher = guest.cipher.clone();
            assist.guests.insert(connection_id.to_string(), guest);
            Some((cipher, state_frame, snapshot))
        }
    };
    match admitted {
        Some((cipher, state_frame, snapshot)) => {
            let _ = send_to_guest(client, inner, assist_id, connection_id, &cipher, &state_frame).await;
            if let Some(snapshot) = snapshot {
                let _ = send_to_guest(client, inner, assist_id, connection_id, &cipher, &snapshot).await;
            }
            report_event(client.clone(), Arc::clone(inner), assist_id.to_string(), "guest_admitted", connection_id.to_string(), json!({"client": "unknown"}));
            let _ = app.emit("assist-changed", ());
        }
        None => {
            let _ = send_frame(client, inner, assist_id, json!({"kind": "PAKE_FAILED", "connection_id": connection_id})).await;
            report_event(client.clone(), Arc::clone(inner), assist_id.to_string(), "pake_failed", connection_id.to_string(), json!({}));
            check_lock(client, inner, app, assist_id).await;
        }
    }
}

/// Three failed guesses end the session locally and tell the server (which
/// independently locks after the same count).
async fn check_lock<R: tauri::Runtime>(client: &reqwest::Client, inner: &Arc<Mutex<BridgeInner>>, app: &AppHandle<R>, assist_id: &str) {
    let lock = inner
        .lock()
        .ok()
        .and_then(|mut g| g.assist.as_mut().map(|a| {
            if a.failed_attempts >= MAX_FAILED_ATTEMPTS && !a.locked {
                a.locked = true;
                true
            } else {
                false
            }
        }))
        .unwrap_or(false);
    if lock {
        let _ = app.emit("assist-locked", ());
        report_event(client.clone(), Arc::clone(inner), assist_id.to_string(), "locked", String::new(), json!({}));
        // Leave the state in place briefly so the UI can show "locked";
        // housekeeping tears it down.
    }
}

async fn handle_guest_envelope<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle<R>,
    assist_id: &str,
    connection_id: &str,
    frame: &serde_json::Value,
) {
    let cipher = inner
        .lock()
        .ok()
        .and_then(|g| g.assist.as_ref().and_then(|a| a.guests.get(connection_id).map(|guest| guest.cipher.clone())));
    let Some(cipher) = cipher else { return };
    let Ok(inner_frame) = cipher.decrypt(assist_id, connection_id, DIRECTION_GUEST, frame) else {
        return;
    };
    match inner_frame.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
        "ASSIST_HELLO" => {
            let knock = {
                let Ok(mut guard) = inner.lock() else { return };
                let Some(assist) = guard.assist.as_mut() else { return };
                let Some(guest) = assist.guests.get_mut(connection_id) else { return };
                guest.client = inner_frame
                    .get("client")
                    .and_then(|v| v.as_str())
                    .filter(|c| matches!(*c, "web" | "auraterm"))
                    .unwrap_or("unknown")
                    .to_string();
                guest.display_name = inner_frame
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(32)
                    .collect();
                guest.hello_seen = true;
                matches!(guest.state, GuestState::PendingApproval).then(|| KnockEvent {
                    connection_id: connection_id.to_string(),
                    display_name: guest.display_name.clone(),
                    client: guest.client.clone(),
                    fingerprint: guest.fingerprint.clone(),
                    kind: "join".into(),
                })
            };
            if let Some(knock) = knock {
                let _ = app.emit("assist-guest-knock", knock);
            }
            let _ = app.emit("assist-changed", ());
        }
        "CONTROL_REQUEST" => {
            let action = {
                let Ok(mut guard) = inner.lock() else { return };
                let Some(assist) = guard.assist.as_mut() else { return };
                let policy = assist.policy.control;
                let Some(guest) = assist.guests.get_mut(connection_id) else { return };
                if !guest.is_active() || policy == ControlPolicy::ViewOnly {
                    None
                } else if policy == ControlPolicy::AutoGrant {
                    Some(true)
                } else {
                    guest.control_requested = true;
                    Some(false)
                }
                .map(|auto| {
                    (
                        auto,
                        KnockEvent {
                            connection_id: connection_id.to_string(),
                            display_name: guest.display_name.clone(),
                            client: guest.client.clone(),
                            fingerprint: guest.fingerprint.clone(),
                            kind: "control".into(),
                        },
                    )
                })
            };
            match action {
                Some((true, _)) => {
                    let expires = Some(SystemTime::now() + Duration::from_secs(DEFAULT_CONTROL_SECS));
                    let _ = set_role(client, inner, app, connection_id, GuestState::Controller { expires_at: expires }, false).await;
                }
                Some((false, knock)) => {
                    let _ = app.emit("assist-guest-knock", knock);
                    let _ = app.emit("assist-changed", ());
                }
                None => {
                    let frame = json!({"kind": "CONTROL_REVOKE", "reason": "view_only"});
                    let _ = send_to_guest(client, inner, assist_id, connection_id, &cipher, &frame).await;
                }
            }
        }
        "CONTROL_RELEASE" => {
            let _ = set_role(client, inner, app, connection_id, GuestState::Viewer, false).await;
        }
        "INPUT" => {
            let validated = {
                let Ok(mut guard) = inner.lock() else { return };
                let Some(assist) = guard.assist.as_mut() else { return };
                let fence = assist.fence;
                let local = assist.local_session_id.clone();
                let protocol = assist.protocol;
                let Some(guest) = assist.guests.get_mut(connection_id) else { return };
                let live = match guest.state {
                    GuestState::Controller { expires_at } => expires_at.map_or(true, |t| t > SystemTime::now()),
                    _ => false,
                };
                if !live {
                    None
                } else if inner_frame.get("fence").and_then(|v| v.as_u64()) != Some(fence) {
                    None
                } else {
                    let data = inner_frame
                        .get("data_hex")
                        .and_then(|v| v.as_str())
                        .and_then(|hex| decode_hex(hex).ok())
                        .filter(|bytes| !bytes.is_empty() && bytes.len() <= MAX_TX_BYTES);
                    data.map(|bytes| (local, protocol, bytes, fence, inner_frame.get("input_seq").and_then(|v| v.as_u64()).unwrap_or(0)))
                }
            };
            let Some((local, protocol, bytes, fence, input_seq)) = validated else {
                return;
            };
            let byte_count = bytes.len();
            if port.write_tx(protocol, &local, &bytes).await.is_err() {
                return;
            }
            let _ = app.emit(
                "cloud-bridge-remote-tx",
                RemoteTxEvent {
                    local_session_id: local,
                    byte_count,
                    fence,
                },
            );
            let ack = json!({"kind": "INPUT_ACK", "input_seq": input_seq, "fence": fence, "byte_count": byte_count});
            let _ = send_to_guest(client, inner, assist_id, connection_id, &cipher, &ack).await;
        }
        _ => {}
    }
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

async fn handle_guest_close<R: tauri::Runtime>(client: &reqwest::Client, inner: &Arc<Mutex<BridgeInner>>, app: &AppHandle<R>, assist_id: &str, connection_id: &str) {
    let (removed, abandoned) = {
        let Ok(mut guard) = inner.lock() else { return };
        let Some(assist) = guard.assist.as_mut() else { return };
        // A guest that got PAKE_B and vanished without confirming almost
        // certainly saw its own confirmation check fail (wrong code) — with
        // the host sending cB first, that is the only way a guessing guest
        // ever ends, so it must count as a failed attempt.
        let abandoned = assist.pending_pake.remove(connection_id).is_some();
        if abandoned {
            assist.failed_attempts += 1;
        }
        let removed = assist.guests.remove(connection_id);
        if removed.as_ref().is_some_and(|g| matches!(g.state, GuestState::Controller { .. })) {
            assist.fence += 1;
        }
        (removed.is_some(), abandoned)
    };
    if abandoned {
        report_event(client.clone(), Arc::clone(inner), assist_id.to_string(), "pake_failed", connection_id.to_string(), json!({}));
        check_lock(client, inner, app, assist_id).await;
    }
    if removed {
        report_event(client.clone(), Arc::clone(inner), assist_id.to_string(), "guest_left", connection_id.to_string(), json!({}));
        let _ = app.emit("assist-changed", ());
    }
}

// ── background tasks ────────────────────────────────────────────────────────

/// Fan the RX ring out to every admitted guest, like the Cloud Console
/// output pump but keyed on the assist session (which may switch tabs).
fn spawn_output_pump(client: reqwest::Client, inner: Arc<Mutex<BridgeInner>>, assist_id: String) {
    tauri::async_runtime::spawn(async move {
        loop {
            let Some((ring, notify, targets, size)) = inner.lock().ok().and_then(|guard| {
                guard.assist.as_ref().filter(|a| a.assist_id == assist_id).map(|a| {
                    (
                        Arc::clone(&a.ring),
                        Arc::clone(&a.output_notify),
                        a.active_ciphers(),
                        guard.terminal_size(&a.local_session_id),
                    )
                })
            }) else {
                break;
            };
            let next = ring
                .lock()
                .ok()
                .and_then(|ring| ring.chunks.iter().find(|(seq, _)| *seq > ring.delivered_seq).cloned());
            let Some((seq, bytes)) = next else {
                notify.notified().await;
                continue;
            };
            let expected = ring.lock().map(|ring| ring.delivered_seq + 1).unwrap_or(seq);
            if seq != expected {
                let snapshot = snapshot_frame(&ring, size);
                let snapshot_seq = snapshot.get("snapshot_seq").and_then(|v| v.as_u64()).unwrap_or(seq);
                for (connection_id, cipher) in &targets {
                    let _ = send_to_guest(&client, &inner, &assist_id, connection_id, cipher, &snapshot).await;
                }
                if let Ok(mut ring) = ring.lock() {
                    ring.delivered_seq = snapshot_seq;
                };
                continue;
            }
            let output = json!({"kind": "OUTPUT", "output_seq": seq, "data_hex": encode_hex(&bytes)});
            for (connection_id, cipher) in &targets {
                let _ = send_to_guest(&client, &inner, &assist_id, connection_id, cipher, &output).await;
            }
            if let Ok(mut ring) = ring.lock() {
                ring.delivered_seq = ring.delivered_seq.max(seq);
            };
        }
    });
}

/// Expire pending approvals and control grants; tear down a locked session.
fn spawn_housekeeping<R: tauri::Runtime>(client: reqwest::Client, inner: Arc<Mutex<BridgeInner>>, app: AppHandle<R>, assist_id: String) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(HOUSEKEEPING_SECS)).await;
            let now = SystemTime::now();
            let (alive, locked, expired_pending, expired_control) = {
                let Ok(guard) = inner.lock() else { break };
                let Some(assist) = guard.assist.as_ref().filter(|a| a.assist_id == assist_id) else {
                    break;
                };
                let expired_pending: Vec<String> = assist
                    .guests
                    .iter()
                    .filter(|(_, g)| matches!(g.state, GuestState::PendingApproval) && g.joined_at + Duration::from_secs(APPROVAL_TIMEOUT_SECS) < now)
                    .map(|(id, _)| id.clone())
                    .collect();
                let expired_control: Vec<String> = assist
                    .guests
                    .iter()
                    .filter(|(_, g)| matches!(g.state, GuestState::Controller { expires_at: Some(t) } if t < now))
                    .map(|(id, _)| id.clone())
                    .collect();
                (true, assist.locked, expired_pending, expired_control)
            };
            if !alive {
                break;
            }
            if locked {
                let port = app.state::<CloudBridgeState>().port.clone();
                let _ = stop_assist(&client, &inner, &port, &app, "locked".into(), true).await;
                break;
            }
            for connection_id in expired_pending {
                let _ = deny_guest(&client, &inner, &app, &connection_id, "approval_timeout").await;
            }
            for connection_id in expired_control {
                let _ = set_role(&client, &inner, &app, &connection_id, GuestState::Viewer, false).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_deadline_parses_to_epoch_seconds() {
        assert_eq!(parse_iso_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_seconds("2026-08-23T12:00:00Z"), Some(1_787_486_400));
        assert_eq!(parse_iso_seconds("2026-08-23T12:00:00.123456Z"), Some(1_787_486_400));
        assert_eq!(parse_iso_seconds("garbage"), None);
    }


    // ── end-to-end host handshake with a simulated guest ───────────────────

    use crate::cloud_bridge::test_support::install_fake_device;
    use crate::terminal_event_hub::TerminalEventHub;
    use async_trait::async_trait;

    struct FakePort {
        hub: Arc<TerminalEventHub>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    #[async_trait]
    impl SharedSessionPort for FakePort {
        async fn contains(&self, _protocol: SessionProtocol, _session_id: &str) -> bool {
            true
        }
        fn subscribe_rx(&self, session_id: &str, mut sink: Box<dyn FnMut(&TerminalEvent) + Send>) -> SubscriptionToken {
            self.hub.subscribe(session_id, move |event| sink(event))
        }
        fn unsubscribe_rx(&self, subscription: &SubscriptionToken) {
            self.hub.unsubscribe(subscription);
        }
        async fn write_tx(&self, _protocol: SessionProtocol, _session_id: &str, bytes: &[u8]) -> Result<(), String> {
            self.written.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }
    }

    struct Harness {
        app: tauri::App<tauri::test::MockRuntime>,
        state: CloudBridgeState,
        port: Arc<dyn SharedSessionPort>,
        hub: Arc<TerminalEventHub>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        outbound: tokio::sync::mpsc::Receiver<serde_json::Value>,
        secret: String,
    }

    const ASSIST_ID: &str = "assist-test-1";
    const LOCAL_ID: &str = "tab-1";

    fn harness(policy: ControlPolicy, approval_required: bool) -> Harness {
        let hub = Arc::new(TerminalEventHub::new());
        let written = Arc::new(Mutex::new(Vec::new()));
        let port: Arc<dyn SharedSessionPort> = Arc::new(FakePort {
            hub: Arc::clone(&hub),
            written: Arc::clone(&written),
        });
        let state = CloudBridgeState::new(Arc::clone(&port));
        // Unroutable base URL: event reports fail fast and silently.
        let outbound = install_fake_device(&state.inner, "http://127.0.0.1:9");
        let secret = assist::generate_secret();
        let w = assist::derive_w(&secret, ASSIST_ID);
        let output_notify = Arc::new(tokio::sync::Notify::new());
        let (subscription, ring) = subscribe_ring(&port, LOCAL_ID, &output_notify);
        state.inner.lock().unwrap().terminal_sizes.insert(LOCAL_ID.into(), (120, 40));
        state.inner.lock().unwrap().assist = Some(AssistHost {
            assist_id: ASSIST_ID.into(),
            route_code: "BCDF".into(),
            secret: secret.clone(),
            w,
            local_session_id: LOCAL_ID.into(),
            protocol: SessionProtocol::Local,
            label: "build box".into(),
            subscription,
            ring,
            output_notify,
            policy: AssistPolicy {
                control: policy,
                approval_required,
                single_use: false,
                max_guests: 3,
            },
            follow_active_tab: false,
            guests: HashMap::new(),
            pending_pake: HashMap::new(),
            failed_attempts: 0,
            fence: 0,
            join_expires_at: SystemTime::now() + Duration::from_secs(600),
            join_url_base: "https://auraxlab.com/assist".into(),
            created_at: SystemTime::now(),
            locked: false,
        });
        Harness {
            app: tauri::test::mock_app(),
            state,
            port,
            hub,
            written,
            outbound,
            secret: secret.to_string(),
        }
    }

    impl Harness {
        async fn feed(&self, frame: serde_json::Value) {
            let handle = self.app.handle().clone();
            handle_frame(&self.state.client, &self.state.inner, &self.port, &handle, &frame).await;
        }

        async fn next_frame(&mut self) -> serde_json::Value {
            tokio::time::timeout(Duration::from_secs(5), self.outbound.recv())
                .await
                .expect("host frame within 5s")
                .expect("transport open")
        }

        fn b64(bytes: &[u8]) -> String {
            URL_SAFE_NO_PAD.encode(bytes)
        }

        /// Run the guest side of SPAKE2 for `connection_id` with `secret`;
        /// returns the guest cipher when the host accepted it.
        async fn join(&mut self, connection_id: &str, secret: &str) -> Option<PeerCipher> {
            self.feed(json!({"kind": "ASSIST_INIT", "session_id": ASSIST_ID,
                "connection_id": connection_id, "protocol_version": 1})).await;
            let w = assist::derive_w(secret, ASSIST_ID);
            let guest = assist::guest_pake(&w, ASSIST_ID, connection_id).unwrap();
            let share = *guest.share();
            self.feed(json!({"kind": "PAKE_A", "session_id": ASSIST_ID,
                "connection_id": connection_id, "protocol_version": 1,
                "pa": Self::b64(&share)})).await;
            let reply = self.next_frame().await;
            let frame = &reply["frame"];
            if frame["kind"] == "PAKE_FAILED" {
                return None;
            }
            assert_eq!(frame["kind"], "PAKE_B");
            assert_eq!(frame["connection_id"], connection_id);
            let pb = URL_SAFE_NO_PAD.decode(frame["pb"].as_str().unwrap()).unwrap();
            let keys = guest.finish(&pb).unwrap();
            let confirm_b = URL_SAFE_NO_PAD.decode(frame["confirm_b"].as_str().unwrap()).unwrap();
            if !keys.verify_peer_confirmation(&confirm_b) {
                return None;
            }
            self.feed(json!({"kind": "PAKE_CONFIRM", "session_id": ASSIST_ID,
                "connection_id": connection_id,
                "confirm_a": Self::b64(keys.own_confirmation())})).await;
            let key = assist::session_key(&keys, ASSIST_ID, connection_id);
            Some(PeerCipher::new(*key))
        }

        async fn next_plain(&mut self, connection_id: &str, cipher: &PeerCipher) -> serde_json::Value {
            let envelope = self.next_frame().await;
            let frame = &envelope["frame"];
            assert_eq!(frame["kind"], "E2EE_FRAME", "got {frame}");
            assert_eq!(frame["connection_id"], connection_id);
            cipher.decrypt(ASSIST_ID, connection_id, DIRECTION_HOST, frame).unwrap()
        }

        async fn send_plain(&self, connection_id: &str, cipher: &PeerCipher, inner: serde_json::Value) {
            let mut envelope = cipher.encrypt(ASSIST_ID, connection_id, DIRECTION_GUEST, &inner).unwrap();
            envelope["session_id"] = json!(ASSIST_ID);
            self.feed(envelope).await;
        }
    }

    #[test]
    fn guest_handshake_state_snapshot_input_and_resize() {
        tauri::async_runtime::block_on(async {
            let mut h = harness(ControlPolicy::AutoGrant, false);
            let secret = h.secret.clone();
            let cipher = h.join("guest-1", &secret).await.expect("correct code is admitted");

            // ASSIST_STATE (auto-grant → controller) then the snapshot with the real grid.
            let state = h.next_plain("guest-1", &cipher).await;
            assert_eq!(state["kind"], "ASSIST_STATE");
            assert_eq!(state["state"], "active");
            assert_eq!(state["role"], "controller");
            assert_eq!(state["cols"], 120);
            assert_eq!(state["rows"], 40);
            assert_eq!(state["fence"], 1);
            let fingerprint = state["fingerprint"].as_str().unwrap().to_string();
            assert_eq!(fingerprint.len(), 9);
            let snapshot = h.next_plain("guest-1", &cipher).await;
            assert_eq!(snapshot["kind"], "TERMINAL_SNAPSHOT");
            assert_eq!(snapshot["cols"], 120);

            // HELLO updates the guest label; the status view reflects it.
            h.send_plain("guest-1", &cipher, json!({"kind": "ASSIST_HELLO", "client": "web", "display_name": "Ada"})).await;
            let view = h.state.inner.lock().unwrap().assist.as_ref().unwrap().status_view();
            assert_eq!(view.guests.len(), 1);
            assert_eq!(view.guests[0].display_name, "Ada");
            assert_eq!(view.guests[0].client, "web");
            assert_eq!(view.guests[0].role, "controller");
            assert_eq!(view.guests[0].fingerprint, fingerprint);

            // Live output reaches the guest through the pump.
            spawn_output_pump(h.state.client.clone(), Arc::clone(&h.state.inner), ASSIST_ID.into());
            h.hub.publish(LOCAL_ID, &TerminalEvent::Output(b"hello guest".to_vec()));
            let output = h.next_plain("guest-1", &cipher).await;
            assert_eq!(output["kind"], "OUTPUT");
            assert_eq!(output["data_hex"], encode_hex(b"hello guest"));

            // INPUT with the current fence is written to the local session and acked.
            h.send_plain("guest-1", &cipher, json!({"kind": "INPUT", "fence": 1, "input_seq": 1, "data_hex": "6c730a"})).await;
            let ack = h.next_plain("guest-1", &cipher).await;
            assert_eq!(ack["kind"], "INPUT_ACK");
            assert_eq!(ack["byte_count"], 3);
            assert_eq!(h.written.lock().unwrap().as_slice(), &[b"ls\n".to_vec()]);

            // Stale fence / revoked control: dropped silently.
            let handle = h.app.handle().clone();
            set_role(&h.state.client, &h.state.inner, &handle, "guest-1", GuestState::Viewer, false).await.unwrap();
            let revoke = h.next_plain("guest-1", &cipher).await;
            assert_eq!(revoke["kind"], "CONTROL_REVOKE");
            let state = h.next_plain("guest-1", &cipher).await;
            assert_eq!(state["role"], "viewer");
            h.send_plain("guest-1", &cipher, json!({"kind": "INPUT", "fence": 1, "input_seq": 2, "data_hex": "41"})).await;
            h.send_plain("guest-1", &cipher, json!({"kind": "INPUT", "fence": 2, "input_seq": 3, "data_hex": "41"})).await;
            assert_eq!(h.written.lock().unwrap().len(), 1);

            // A viewer asking for control under auto-grant gets it back.
            h.send_plain("guest-1", &cipher, json!({"kind": "CONTROL_REQUEST"})).await;
            let grant = h.next_plain("guest-1", &cipher).await;
            assert_eq!(grant["kind"], "CONTROL_GRANT");
            let fence = grant["fence"].as_u64().unwrap();
            assert!(fence >= 3);

            // Guest leaving advances the fence and clears it from the view.
            h.feed(json!({"kind": "E2EE_CLOSE", "session_id": ASSIST_ID, "connection_id": "guest-1"})).await;
            let guard = h.state.inner.lock().unwrap();
            let assist = guard.assist.as_ref().unwrap();
            assert!(assist.guests.is_empty());
            assert!(assist.fence > fence);
        });
    }

    #[test]
    fn wrong_code_fails_and_three_failures_lock_the_session() {
        tauri::async_runtime::block_on(async {
            let mut h = harness(ControlPolicy::OnRequest, false);
            let secret = h.secret.clone();
            let wrong = if secret.starts_with('B') { "CCCCCCCC" } else { "BBBBBBBB" };
            for attempt in 1..=3 {
                // A wrong guess fails the guest's check of cB first (the host
                // sends its confirmation with PAKE_B); the guest then just
                // drops the connection. The host must still count that as a
                // failed attempt when the relay reports the close.
                let cid = format!("bad-{attempt}");
                assert!(h.join(&cid, wrong).await.is_none());
                h.feed(json!({"kind": "E2EE_CLOSE", "session_id": ASSIST_ID, "connection_id": cid})).await;
            }
            {
                let guard = h.state.inner.lock().unwrap();
                let assist = guard.assist.as_ref().unwrap();
                assert_eq!(assist.failed_attempts, 3);
                assert!(assist.locked);
                assert!(assist.guests.is_empty());
            }
            // Locked: even the right code is refused at PAKE_A now, and a
            // confirmation for a connection that never got PAKE_B is ignored.
            assert!(h.join("late", &secret).await.is_none());
            h.feed(json!({"kind": "PAKE_CONFIRM", "session_id": ASSIST_ID, "connection_id": "late",
                "confirm_a": Harness::b64(&[0_u8; 32])})).await;
            assert!(tokio::time::timeout(Duration::from_millis(200), h.outbound.recv()).await.is_err());
            assert_eq!(h.state.inner.lock().unwrap().assist.as_ref().unwrap().failed_attempts, 3);
        });
    }

    #[test]
    fn bogus_confirmation_is_refused_and_counted() {
        tauri::async_runtime::block_on(async {
            let mut h = harness(ControlPolicy::OnRequest, false);
            let secret = h.secret.clone();
            let w = assist::derive_w(&secret, ASSIST_ID);
            let guest = assist::guest_pake(&w, ASSIST_ID, "bogus").unwrap();
            h.feed(json!({"kind": "ASSIST_INIT", "session_id": ASSIST_ID, "connection_id": "bogus", "protocol_version": 1})).await;
            h.feed(json!({"kind": "PAKE_A", "session_id": ASSIST_ID, "connection_id": "bogus", "protocol_version": 1,
                "pa": Harness::b64(guest.share())})).await;
            assert_eq!(h.next_frame().await["frame"]["kind"], "PAKE_B");
            h.feed(json!({"kind": "PAKE_CONFIRM", "session_id": ASSIST_ID, "connection_id": "bogus",
                "confirm_a": Harness::b64(&[0_u8; 32])})).await;
            let failed = h.next_frame().await;
            assert_eq!(failed["frame"]["kind"], "PAKE_FAILED");
            assert_eq!(failed["frame"]["connection_id"], "bogus");
            let guard = h.state.inner.lock().unwrap();
            let assist = guard.assist.as_ref().unwrap();
            assert_eq!(assist.failed_attempts, 1);
            assert!(!assist.locked);
            assert!(assist.guests.is_empty());
            assert!(assist.pending_pake.is_empty());
        });
    }

    #[test]
    fn approval_required_holds_the_guest_until_the_host_decides() {
        tauri::async_runtime::block_on(async {
            let mut h = harness(ControlPolicy::OnRequest, true);
            let secret = h.secret.clone();
            let cipher = h.join("guest-2", &secret).await.unwrap();
            let state = h.next_plain("guest-2", &cipher).await;
            assert_eq!(state["state"], "pending_approval");
            // No snapshot and no output before approval.
            spawn_output_pump(h.state.client.clone(), Arc::clone(&h.state.inner), ASSIST_ID.into());
            h.hub.publish(LOCAL_ID, &TerminalEvent::Output(b"secret output".to_vec()));
            assert!(tokio::time::timeout(Duration::from_millis(300), h.outbound.recv()).await.is_err());

            let handle = h.app.handle().clone();
            set_role(&h.state.client, &h.state.inner, &handle, "guest-2", GuestState::Viewer, true).await.unwrap();
            let state = h.next_plain("guest-2", &cipher).await;
            assert_eq!((state["state"].as_str(), state["role"].as_str()), (Some("active"), Some("viewer")));
            let snapshot = h.next_plain("guest-2", &cipher).await;
            assert_eq!(snapshot["kind"], "TERMINAL_SNAPSHOT");
            assert_eq!(snapshot["data_hex"], encode_hex(b"secret output"));

            // Denying a pending guest is only valid while pending.
            assert!(set_role(&h.state.client, &h.state.inner, &handle, "guest-2", GuestState::Viewer, true).await.is_err());
            deny_guest(&h.state.client, &h.state.inner, &handle, "guest-2", "kicked").await.unwrap();
            let denied = h.next_plain("guest-2", &cipher).await;
            assert_eq!(denied["state"], "denied");
            assert!(h.state.inner.lock().unwrap().assist.as_ref().unwrap().guests.is_empty());
        });
    }

    #[test]
    fn hex_decoding_rejects_odd_and_non_hex() {
        assert_eq!(decode_hex("410d0a").unwrap(), b"A\r\n");
        assert!(decode_hex("41f").is_err());
        assert!(decode_hex("zz").is_err());
    }
}
