//! Live Relay — provider side (design `docs/plans/live-sync-design.md` §5).
//!
//! Another AuraTerm on the *same account* attaches to a session this device
//! already shares to Live Console. On the wire the consumer is just another
//! E2EE browser-channel viewer; what this module adds is the local
//! authority the design demands: the policy gate, the per-peer knock, the
//! concurrency cap and kick. The server's stored policy copy only lets
//! grant requests fail fast — every admission decision is taken *here*.
//!
//! Identity metadata flow: the relay data plane forwards nothing but the
//! consumer's E2EE public key (E2EE_INIT), so the control plane pushes a
//! `RELAY_GRANT_ISSUED` frame at grant time binding that key's hash to the
//! consumer's device id and label. Phase 2 admits viewers only — the relay
//! itself refuses upstream frames from non-controller browsers, so a
//! relay_viewer cannot even send INPUT (phase 3 territory).

use crate::cloud_bridge::{
    decode_hex, derive_peer_cipher, e2ee_context, ed25519_sign_b64, encode_hex, send_e2ee_frame,
    send_frame, sha256_hex, BridgeInner, CloudBridgeState, RemoteTxEvent, MAX_TX_BYTES,
};
use crate::e2ee::{sas_fingerprint, PeerCipher, DIRECTION_BROWSER};
use crate::settings::LiveRelaySettings;
use crate::shared_session::SharedSessionPort;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};

/// How long a knock may sit unanswered before the peer is denied.
const APPROVAL_TIMEOUT_SECS: u64 = 60;
/// Grant metadata older than this is dropped (the ticket has long expired).
const GRANT_META_TTL_SECS: u64 = 600;
pub(crate) const SAS_LABEL: &str = "auraxlab-relay-sas|";

/// Who a grant was issued to, pushed by the control plane and bound to the
/// consumer's E2EE key hash. Metadata only — admission still requires the
/// matching key to arrive in E2EE_INIT.
#[derive(Clone)]
pub(crate) struct RelayGrantMeta {
    consumer_device_id: String,
    consumer_label: String,
    issued_at: SystemTime,
}

/// A consumer that finished the E2EE handshake and is waiting for the local
/// user's decision (or for the approval timeout).
pub(crate) struct PendingRelayPeer {
    cloud_session_id: String,
    cipher: PeerCipher,
    fingerprint: String,
    consumer_label: String,
    consumer_device_id: String,
    /// The relay admitted it as `relay_controller`, so it may ask for
    /// control. It still starts as a viewer.
    may_request_control: bool,
}

/// An admitted relay peer (also registered in the share's peer map, which
/// is what actually fans terminal output out to it).
pub(crate) struct RelayPeerMeta {
    cloud_session_id: String,
    fingerprint: String,
    consumer_label: String,
    #[allow(dead_code)]
    consumer_device_id: String,
    joined_at: SystemTime,
    /// True once the local user granted this peer control. Write authority
    /// is this flag plus a matching fence — never anything the server said.
    controller: bool,
    /// Set when the peer's relay role lets it send upstream at all; a
    /// `relay_viewer` is refused by the relay before we ever see a frame.
    may_request_control: bool,
    /// Highest `input_seq` applied, for de-duplication within one fence.
    last_input_seq: u64,
    /// When this peer last typed. Only meaningful while it holds control;
    /// the idle sweep uses it to take write access back from a controller
    /// who walked away.
    last_input_at: SystemTime,
    /// Set while the peer asked for control and the local user has not
    /// answered yet.
    control_requested: bool,
}

/// One session a peer may ask this device to open. The id is opaque to
/// everyone but this device: the server forwards it untouched, and the
/// frontend resolves it back to a shell / port / bookmark. A peer can only
/// ever name an id already on this list.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct RelayOpenTarget {
    /// "local_shell" | "serial" | "bookmark"
    pub kind: String,
    pub id: String,
    pub label: String,
}

#[derive(Default)]
pub(crate) struct RelayProviderState {
    /// Full policy mirrored from settings (`cloud_bridge_set_relay_policy`).
    /// Defaults to all-closed until the frontend pushes the stored value.
    pub(crate) policy: LiveRelaySettings,
    /// peer_key_hash -> who the control plane issued a grant to.
    grants: HashMap<String, RelayGrantMeta>,
    /// connection_id -> handshaken peer awaiting local approval.
    pending: HashMap<String, PendingRelayPeer>,
    /// connection_id -> admitted peer metadata.
    peers: HashMap<String, RelayPeerMeta>,
    /// What this device is willing to open for a peer, pushed by the
    /// frontend (which is what can enumerate ports and bookmarks).
    pub(crate) targets: Vec<RelayOpenTarget>,
    /// True while the idle sweep is running, so admissions do not stack up
    /// duplicate sweepers.
    housekeeping_running: bool,
    /// Write-authority fence, owned by this device (design §5.10). Every
    /// grant bumps it, so INPUT carrying an older fence is stale by
    /// construction — a revoked peer cannot replay queued keystrokes.
    fence: u64,
}

impl RelayProviderState {
    fn relay_peer_count(&self) -> usize {
        self.pending.len() + self.peers.len()
    }
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ── frontend views ──────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayPeerView {
    pub connection_id: String,
    pub label: String,
    pub fingerprint: String,
    pub share_label: String,
    /// "pending" | "viewer" | "controller"
    pub state: String,
    pub joined_at: Option<u64>,
    /// The peer attached with a controller ticket, so control can be given.
    pub can_control: bool,
    /// It asked for control and is waiting for an answer.
    pub control_requested: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayProviderStatus {
    pub enabled: bool,
    pub peers: Vec<RelayPeerView>,
}

/// Payload of the `relay-knock` event shown by the provider-side dialog.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayKnockEvent {
    pub connection_id: String,
    pub label: String,
    pub fingerprint: String,
    pub share_label: String,
    /// "join" (waiting for admission) | "control" (asks to type)
    pub kind: String,
}

// ── inbound frame hook (called from cloud_bridge::process_inbound_frame) ────

/// Returns true when the frame was consumed by the relay provider. Cleanup
/// frames (E2EE_CLOSE, SESSION_UNSHARE) update relay state but return
/// false so the generic handler keeps its own bookkeeping.
pub(crate) async fn handle_frame<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle<R>,
    frame: &serde_json::Value,
) -> bool {
    let kind = frame.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "E2EE_FRAME" => {
            // Claim the envelope only when it belongs to a relay peer;
            // Live Console viewers stay on their own path.
            let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()) else {
                return false;
            };
            let channel = inner.lock().ok().and_then(|guard| {
                guard
                    .relay
                    .peers
                    .contains_key(connection_id)
                    .then(|| peer_channel(&guard, connection_id))
                    .flatten()
            });
            let Some((cipher, cloud_id)) = channel else { return false };
            let Ok(decrypted) = cipher.decrypt(&cloud_id, connection_id, DIRECTION_BROWSER, frame) else {
                return true;
            };
            handle_peer_frame(client, inner, port, app, connection_id, &decrypted).await;
            true
        }
        "RELAY_GRANT_ISSUED" => {
            let (Some(key_hash), Some(consumer_id)) = (
                frame.get("peer_key_hash").and_then(|v| v.as_str()),
                frame.get("consumer_device_id").and_then(|v| v.as_str()),
            ) else {
                return true;
            };
            let label = frame.get("consumer_label").and_then(|v| v.as_str()).unwrap_or("");
            if let Ok(mut guard) = inner.lock() {
                let now = SystemTime::now();
                guard
                    .relay
                    .grants
                    .retain(|_, meta| now.duration_since(meta.issued_at).map(|age| age.as_secs() < GRANT_META_TTL_SECS).unwrap_or(false));
                guard.relay.grants.insert(
                    key_hash.to_string(),
                    RelayGrantMeta {
                        consumer_device_id: consumer_id.to_string(),
                        consumer_label: label.chars().filter(|c| !c.is_control()).take(64).collect(),
                        issued_at: now,
                    },
                );
            }
            true
        }
        "E2EE_INIT" => {
            let role = frame.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if !role.starts_with("relay_") {
                return false;
            }
            handle_relay_init(client, inner, app, frame).await;
            true
        }
        "RELAY_OPEN_REQUEST" => {
            handle_open_request(client, inner, app, frame).await;
            true
        }
        "E2EE_CLOSE" => {
            if let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()) {
                let changed = if let Ok(mut guard) = inner.lock() {
                    let had_control = guard
                        .relay
                        .peers
                        .get(connection_id)
                        .map(|peer| peer.controller)
                        .unwrap_or(false);
                    // Retire the fence with the controller that held it, so
                    // a reconnecting peer cannot resume its write authority.
                    if had_control {
                        guard.relay.fence += 1;
                    }
                    guard.relay.pending.remove(connection_id).is_some()
                        | guard.relay.peers.remove(connection_id).is_some()
                } else {
                    false
                };
                if changed {
                    let _ = app.emit("relay-peers-changed", ());
                }
            }
            false
        }
        "SESSION_UNSHARE" => {
            if let Some(cloud_id) = frame.get("session_id").and_then(|v| v.as_str()) {
                if let Ok(mut guard) = inner.lock() {
                    guard.relay.pending.retain(|_, peer| peer.cloud_session_id != cloud_id);
                    guard.relay.peers.retain(|_, peer| peer.cloud_session_id != cloud_id);
                }
                let _ = app.emit("relay-peers-changed", ());
            }
            false
        }
        _ => false,
    }
}

/// One consumer arrived on the browser channel with a relay role: complete
/// the E2EE handshake first (so every later word — including a denial — is
/// end-to-end encrypted), then let the local policy decide.
async fn handle_relay_init<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    app: &AppHandle<R>,
    frame: &serde_json::Value,
) {
    let role = frame.get("role").and_then(|v| v.as_str()).unwrap_or("");
    let (Some(cloud_id), Some(connection_id), Some(peer_public)) = (
        frame.get("session_id").and_then(|v| v.as_str()),
        frame.get("connection_id").and_then(|v| v.as_str()),
        frame.get("peer_public_key").and_then(|v| v.as_str()),
    ) else {
        return;
    };
    // Policy + share + identity metadata, all under one lock.
    let admission = {
        let Ok(guard) = inner.lock() else { return };
        let Some(device) = guard.device.clone() else { return };
        let share_label = guard
            .shares
            .values()
            .find(|share| share.cloud_session_id == cloud_id)
            .map(|share| share.label.clone());
        let policy = guard.relay.policy.clone();
        let denial = if share_label.is_none() {
            Some("unknown_session")
        } else if !policy.enabled || !policy.allow_attach {
            Some("relay_disabled")
        } else if guard.relay.relay_peer_count() >= policy.max_concurrent_peers.max(1) as usize {
            Some("busy")
        } else {
            None
        };
        let grant = guard.relay.grants.get(&sha256_hex(peer_public)).cloned();
        (device, share_label.unwrap_or_default(), policy, denial, grant)
    };
    let (device, share_label, policy, denial, grant) = admission;

    // Handshake: derive, prove our identity, answer with E2EE_READY.
    let Ok((cipher, agent_public)) = derive_peer_cipher(cloud_id, connection_id, peer_public) else {
        return;
    };
    let Ok(proof) = ed25519_sign_b64(
        &device.identity_key,
        &e2ee_context(&device.device_id, cloud_id, connection_id, peer_public, &agent_public),
    ) else {
        return;
    };
    if send_frame(
        client,
        inner,
        cloud_id,
        json!({"kind": "E2EE_READY", "connection_id": connection_id,
               "agent_public_key": agent_public, "proof": proof}),
    )
    .await
    .is_err()
    {
        return;
    }
    let fingerprint = sas_fingerprint(&cipher.key, SAS_LABEL);

    if let Some(reason) = denial {
        let state = json!({"kind": "RELAY_STATE", "state": "denied", "reason": reason});
        let _ = send_e2ee_frame(client, inner, cloud_id, connection_id, &cipher, &state).await;
        return;
    }
    let (consumer_label, consumer_device_id) = grant
        .map(|meta| (meta.consumer_label, meta.consumer_device_id))
        .unwrap_or_default();
    // Only a relay_controller ticket can carry frames upstream at all; a
    // relay_viewer is refused by the relay, so never offer it the button.
    let may_request_control = role == "relay_controller";

    if policy.require_approval {
        let state = json!({"kind": "RELAY_STATE", "state": "pending_approval",
            "role": "viewer",
            "control_policy": if may_request_control { "on_request" } else { "view_only" },
            "host_label": share_label});
        if send_e2ee_frame(client, inner, cloud_id, connection_id, &cipher, &state).await.is_err() {
            return;
        }
        if let Ok(mut guard) = inner.lock() {
            guard.relay.pending.insert(
                connection_id.to_string(),
                PendingRelayPeer {
                    cloud_session_id: cloud_id.to_string(),
                    cipher,
                    fingerprint: fingerprint.clone(),
                    consumer_label: consumer_label.clone(),
                    consumer_device_id,
                    may_request_control,
                },
            );
        }
        let _ = app.emit(
            "relay-knock",
            RelayKnockEvent {
                connection_id: connection_id.to_string(),
                label: consumer_label,
                fingerprint,
                share_label,
                kind: "join".into(),
            },
        );
        let _ = app.emit("relay-peers-changed", ());
        spawn_approval_timeout(client.clone(), Arc::clone(inner), app.clone(), connection_id.to_string());
    } else {
        admit(
            client,
            inner,
            app,
            connection_id,
            PendingRelayPeer {
                cloud_session_id: cloud_id.to_string(),
                cipher,
                fingerprint,
                consumer_label,
                consumer_device_id,
                may_request_control,
            },
        )
        .await;
    }
}

/// Deny a still-pending peer after the knock sat unanswered too long.
fn spawn_approval_timeout<R: tauri::Runtime>(client: reqwest::Client, inner: Arc<Mutex<BridgeInner>>, app: AppHandle<R>, connection_id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(APPROVAL_TIMEOUT_SECS)).await;
        let expired = inner
            .lock()
            .ok()
            .and_then(|mut guard| guard.relay.pending.remove(&connection_id));
        let Some(peer) = expired else { return };
        let state = json!({"kind": "RELAY_STATE", "state": "denied", "reason": "approval_timeout"});
        let _ = send_e2ee_frame(&client, &inner, &peer.cloud_session_id, &connection_id, &peer.cipher, &state).await;
        let _ = app.emit("relay-peers-changed", ());
    });
}

/// Admission proper: announce the active state, replay the snapshot, then
/// register the peer so the output pump fans live bytes out to it. The
/// registration comes last for the same reason as in the viewer path — a
/// registered peer immediately receives encrypted output, and the consumer
/// renders the snapshot first.
async fn admit<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    app: &AppHandle<R>,
    connection_id: &str,
    peer: PendingRelayPeer,
) {
    let cloud_id = peer.cloud_session_id.clone();
    let ring = {
        let Ok(guard) = inner.lock() else { return };
        guard.shares.iter().find_map(|(local, share)| {
            (share.cloud_session_id == cloud_id).then(|| {
                (
                    Arc::clone(&share.ring),
                    share.label.clone(),
                    guard.terminal_size(local),
                )
            })
        })
    };
    let Some((ring, share_label, (cols, rows))) = ring else {
        // The share vanished while the knock was open.
        let state = json!({"kind": "RELAY_STATE", "state": "denied", "reason": "unknown_session"});
        let _ = send_e2ee_frame(client, inner, &cloud_id, connection_id, &peer.cipher, &state).await;
        return;
    };
    // `control_policy` tells the consumer whether to offer "ask for
    // control"; the answer still comes from this device, frame by frame.
    let control_policy = if peer.may_request_control { "on_request" } else { "view_only" };
    let state = json!({"kind": "RELAY_STATE", "state": "active", "role": "viewer",
        "control_policy": control_policy, "host_label": share_label,
        "cols": cols, "rows": rows, "fence": 0});
    if send_e2ee_frame(client, inner, &cloud_id, connection_id, &peer.cipher, &state).await.is_err() {
        return;
    }
    let (seq, bytes) = ring.lock().map(|r| r.e2ee_snapshot()).unwrap_or_default();
    let snapshot = json!({"kind": "TERMINAL_SNAPSHOT", "snapshot_seq": seq,
        "cols": cols, "rows": rows, "data_hex": encode_hex(&bytes)});
    if send_e2ee_frame(client, inner, &cloud_id, connection_id, &peer.cipher, &snapshot).await.is_err() {
        return;
    }
    if let Ok(mut guard) = inner.lock() {
        for share in guard.shares.values_mut() {
            if share.cloud_session_id != cloud_id {
                continue;
            }
            share.peers.insert(connection_id.to_string(), peer.cipher.clone());
            share.peer_roles.insert(connection_id.to_string(), "relay_viewer".to_string());
        }
        guard.relay.peers.insert(
            connection_id.to_string(),
            RelayPeerMeta {
                cloud_session_id: cloud_id,
                fingerprint: peer.fingerprint,
                consumer_label: peer.consumer_label,
                consumer_device_id: peer.consumer_device_id,
                joined_at: SystemTime::now(),
                controller: false,
                may_request_control: peer.may_request_control,
                last_input_seq: 0,
                last_input_at: SystemTime::now(),
                control_requested: false,
            },
        );
    }
    let _ = app.emit("relay-peers-changed", ());
    let _ = app.emit("cloud-bridge-peers-changed", ());
    spawn_idle_sweep(client.clone(), Arc::clone(inner), app.clone());
}

/// How often the idle sweep looks; the timeout itself is in minutes, so a
/// coarse tick is plenty and costs nothing while nobody is attached.
const IDLE_SWEEP_SECS: u64 = 20;

/// Take write access back from a controller who stopped typing.
///
/// The setting is worded "no input", and only a controller can produce
/// input — so this deliberately does *not* disconnect silent viewers.
/// Watching a long build without touching the keyboard is the normal case
/// for a viewer; a live cursor sitting unattended on someone else's
/// terminal is the case actually worth ending.
fn spawn_idle_sweep<R: tauri::Runtime>(client: reqwest::Client, inner: Arc<Mutex<BridgeInner>>, app: AppHandle<R>) {
    {
        let Ok(mut guard) = inner.lock() else { return };
        if guard.relay.housekeeping_running {
            return;
        }
        guard.relay.housekeeping_running = true;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(IDLE_SWEEP_SECS)).await;
            let expired = {
                let Ok(mut guard) = inner.lock() else { break };
                if guard.relay.peers.is_empty() && guard.relay.pending.is_empty() {
                    // Nobody attached: stand down until the next admission.
                    guard.relay.housekeeping_running = false;
                    break;
                }
                let timeout = guard.relay.policy.idle_timeout_minutes;
                if timeout == 0 {
                    Vec::new()
                } else {
                    let cutoff = Duration::from_secs(u64::from(timeout) * 60);
                    let now = SystemTime::now();
                    guard
                        .relay
                        .peers
                        .iter()
                        .filter(|(_, peer)| {
                            peer.controller
                                && now
                                    .duration_since(peer.last_input_at)
                                    .map(|idle| idle >= cutoff)
                                    .unwrap_or(false)
                        })
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>()
                }
            };
            for connection_id in expired {
                let _ = set_relay_control(&client, &inner, &app, &connection_id, false).await;
            }
        }
    });
}

// ── control authority (design §5.10) ───────────────────────────────────────

/// Grant or revoke one peer's write authority. Granting bumps the fence and
/// demotes whoever held control, so exactly one peer can type at a time and
/// any INPUT still in flight under the old fence is stale on arrival.
///
/// Every caller is local: a menu action, a knock answer, or a policy change.
/// Nothing the server or the peer says reaches this function.
pub(crate) async fn set_relay_control<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    app: &AppHandle<R>,
    connection_id: &str,
    grant: bool,
) -> Result<(), String> {
    // (connection_id, cipher, cloud_id, frame) for each peer to notify.
    let mut notify: Vec<(String, PeerCipher, String, serde_json::Value)> = Vec::new();
    {
        let mut guard = inner.lock().map_err(|e| e.to_string())?;
        if grant {
            let policy = guard.relay.policy.clone();
            let allow_input = policy.allow_remote_input && guard.allow_remote_send;
            let may_ask = guard
                .relay
                .peers
                .get(connection_id)
                .map(|peer| peer.may_request_control)
                .unwrap_or(false);
            if !allow_input {
                return Err("Remote input is switched off on this device.".into());
            }
            if !may_ask {
                return Err("That peer attached read-only and cannot be given control.".into());
            }
            guard.relay.fence += 1;
            // One controller at a time.
            let others: Vec<String> = guard
                .relay
                .peers
                .iter()
                .filter(|(id, peer)| id.as_str() != connection_id && peer.controller)
                .map(|(id, _)| id.clone())
                .collect();
            for other in others {
                if let Some(peer) = guard.relay.peers.get_mut(&other) {
                    peer.controller = false;
                    peer.last_input_seq = 0;
                }
                if let Some((cipher, cloud_id)) = peer_channel(&guard, &other) {
                    notify.push((other, cipher, cloud_id, json!({"kind": "CONTROL_REVOKE", "reason": "replaced"})));
                }
            }
        }
        let fence = guard.relay.fence;
        let Some(peer) = guard.relay.peers.get_mut(connection_id) else {
            return Ok(());
        };
        peer.controller = grant;
        peer.control_requested = false;
        peer.last_input_seq = 0;
        peer.last_input_at = SystemTime::now();
        let frame = if grant {
            json!({"kind": "CONTROL_GRANT", "fence": fence})
        } else {
            json!({"kind": "CONTROL_REVOKE", "reason": "revoked"})
        };
        if let Some((cipher, cloud_id)) = peer_channel(&guard, connection_id) {
            notify.push((connection_id.to_string(), cipher, cloud_id, frame));
        }
    }
    for (peer_id, cipher, cloud_id, frame) in notify {
        let _ = send_e2ee_frame(client, inner, &cloud_id, &peer_id, &cipher, &frame).await;
    }
    let _ = app.emit("relay-peers-changed", ());
    Ok(())
}

/// The cipher + session a peer's frames travel on, taken from the share's
/// peer map (the same map the output pump fans bytes through).
fn peer_channel(guard: &BridgeInner, connection_id: &str) -> Option<(PeerCipher, String)> {
    let cloud_id = guard.relay.peers.get(connection_id)?.cloud_session_id.clone();
    let cipher = guard
        .shares
        .values()
        .find(|share| share.cloud_session_id == cloud_id)
        .and_then(|share| share.peers.get(connection_id).cloned())?;
    Some((cipher, cloud_id))
}

/// Decrypted traffic from an admitted relay peer. Returns true when the
/// frame was consumed here, so the Live Console handler never sees it —
/// relay write authority is a local fence, not a server-signed lease.
async fn handle_peer_frame<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    port: &Arc<dyn SharedSessionPort>,
    app: &AppHandle<R>,
    connection_id: &str,
    frame: &serde_json::Value,
) {
    match frame.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
        "CONTROL_REQUEST" => {
            let decision = {
                let Ok(mut guard) = inner.lock() else { return };
                let require_approval = guard.relay.policy.require_approval;
                let allow_input = guard.relay.policy.allow_remote_input && guard.allow_remote_send;
                let Some(peer) = guard.relay.peers.get_mut(connection_id) else { return };
                if !peer.may_request_control || !allow_input {
                    None
                } else if require_approval {
                    peer.control_requested = true;
                    Some((false, peer.consumer_label.clone(), peer.fingerprint.clone(), peer.cloud_session_id.clone()))
                } else {
                    Some((true, peer.consumer_label.clone(), peer.fingerprint.clone(), peer.cloud_session_id.clone()))
                }
            };
            let Some((auto, label, fingerprint, cloud_id)) = decision else { return };
            if auto {
                let _ = set_relay_control(client, inner, app, connection_id, true).await;
                return;
            }
            let share_label = inner
                .lock()
                .ok()
                .and_then(|guard| {
                    guard
                        .shares
                        .values()
                        .find(|share| share.cloud_session_id == cloud_id)
                        .map(|share| share.label.clone())
                })
                .unwrap_or_default();
            let _ = app.emit(
                "relay-knock",
                RelayKnockEvent {
                    connection_id: connection_id.to_string(),
                    label,
                    fingerprint,
                    share_label,
                    kind: "control".into(),
                },
            );
            let _ = app.emit("relay-peers-changed", ());
        }
        "CONTROL_RELEASE" => {
            let _ = set_relay_control(client, inner, app, connection_id, false).await;
        }
        "INPUT" => {
            let validated = {
                let Ok(mut guard) = inner.lock() else { return };
                // The global Live Console TX gate covers Live Relay too:
                // one switch blocks remote input on every path (§5.14).
                if !guard.allow_remote_send || !guard.relay.policy.allow_remote_input {
                    return;
                }
                let fence = guard.relay.fence;
                let Some(peer) = guard.relay.peers.get(connection_id) else { return };
                let cloud_id = peer.cloud_session_id.clone();
                let input_seq = frame.get("input_seq").and_then(|v| v.as_u64()).unwrap_or(0);
                if !peer.controller
                    || frame.get("fence").and_then(|v| v.as_u64()) != Some(fence)
                    || input_seq <= peer.last_input_seq
                {
                    return;
                }
                let bytes = frame
                    .get("data_hex")
                    .and_then(|v| v.as_str())
                    .and_then(|hex| decode_hex(hex).ok())
                    .filter(|bytes| !bytes.is_empty() && bytes.len() <= MAX_TX_BYTES);
                let Some(bytes) = bytes else { return };
                // The share must still permit TX; a re-share can have
                // downgraded it to read-only under a live controller.
                let target = guard.shares.iter().find_map(|(local, share)| {
                    (share.cloud_session_id == cloud_id
                        && share.policy.allows_tx(SystemTime::now()))
                    .then(|| (local.clone(), share.protocol))
                });
                let Some((local_id, protocol)) = target else { return };
                if let Some(peer) = guard.relay.peers.get_mut(connection_id) {
                    peer.last_input_seq = input_seq;
                    peer.last_input_at = SystemTime::now();
                }
                Some((local_id, protocol, bytes, fence, input_seq, cloud_id))
            };
            let Some((local_id, protocol, bytes, fence, input_seq, cloud_id)) = validated else {
                return;
            };
            let byte_count = bytes.len();
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
            let cipher = inner.lock().ok().and_then(|guard| peer_channel(&guard, connection_id).map(|(c, _)| c));
            if let Some(cipher) = cipher {
                let ack = json!({"kind": "INPUT_ACK", "input_seq": input_seq,
                                 "fence": fence, "byte_count": byte_count});
                let _ = send_e2ee_frame(client, inner, &cloud_id, connection_id, &cipher, &ack).await;
            }
        }
        _ => {}
    }
}

// ── open mode (design §5.9) ────────────────────────────────────────────────

/// Payload of `relay-open-request`: the frontend opens the named target as
/// a real, visible tab, shares it, and reports the session id back.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayOpenRequestEvent {
    pub request_id: String,
    pub kind: String,
    pub target_id: String,
    pub label: String,
    pub consumer_label: String,
    /// True when the local user still has to approve it.
    pub needs_approval: bool,
}

/// A peer asked this device to open a fresh session.
///
/// This is the one path where a remote party causes AuraTerm to start
/// something, and it stays fenced in by four separate conditions: Live
/// Relay is on, the *kind* is switched on, the id is already on this
/// device's own advertised list, and the concurrency cap has room. The
/// peer never names a program, a working directory or an environment —
/// only an opaque id this device minted itself, which the frontend
/// resolves back to its own shell, its own port, or its own bookmark.
/// A browser cannot reach this code at all: it has no device credential.
async fn handle_open_request<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    app: &AppHandle<R>,
    frame: &serde_json::Value,
) {
    let Some(request_id) = frame.get("request_id").and_then(|v| v.as_str()) else {
        return;
    };
    let kind = frame.get("open_kind").and_then(|v| v.as_str()).unwrap_or("");
    let target_id = frame.get("target_id").and_then(|v| v.as_str()).unwrap_or("");
    let consumer_label = frame
        .get("consumer_label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>();

    let decision = {
        let Ok(guard) = inner.lock() else { return };
        decide_open(&guard.relay, kind, target_id)
    };
    match decision {
        Err(reason) => {
            resolve_open(client, inner, request_id, None, Some(reason)).await;
        }
        Ok(target) => {
            let needs_approval = inner
                .lock()
                .map(|guard| guard.relay.policy.require_approval)
                .unwrap_or(true);
            let _ = app.emit(
                "relay-open-request",
                RelayOpenRequestEvent {
                    request_id: request_id.to_string(),
                    kind: target.kind,
                    target_id: target.id,
                    label: target.label,
                    consumer_label,
                    needs_approval,
                },
            );
        }
    }
}

/// The whole gate an open request must pass, kept pure so the policy can
/// be tested without any relay or HTTP plumbing. Order matters only for the
/// refusal reason the peer is shown; any single failure refuses.
pub(crate) fn decide_open(
    relay: &RelayProviderState,
    kind: &str,
    target_id: &str,
) -> Result<RelayOpenTarget, &'static str> {
    let policy = &relay.policy;
    if !policy.enabled {
        return Err("relay_disabled");
    }
    let kind_allowed = match kind {
        "local_shell" => policy.allow_open_shell,
        "serial" => policy.allow_open_serial,
        "bookmark" => policy.allow_open_bookmark,
        _ => false,
    };
    if !kind_allowed {
        return Err("open_kind_disabled");
    }
    if relay.relay_peer_count() >= policy.max_concurrent_peers.max(1) as usize {
        return Err("busy");
    }
    // An id this device never advertised is refused outright, so a stale or
    // guessed target can never start anything.
    relay
        .targets
        .iter()
        .find(|candidate| candidate.kind == kind && candidate.id == target_id)
        .cloned()
        .ok_or("unknown_target")
}

/// Tell the control plane what happened, so the waiting peer stops polling.
pub(crate) async fn resolve_open(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    request_id: &str,
    session_id: Option<&str>,
    reason: Option<&str>,
) {
    let device = inner.lock().ok().and_then(|guard| guard.device.clone());
    let Some(device) = device else { return };
    let _ = client
        .post(format!(
            "{}/api/v1/auraterm/console/relay/opens/{}/resolve",
            device.base_url, request_id
        ))
        .header("Authorization", format!("Bearer {}", device.credential))
        .json(&json!({"session_id": session_id, "reason": reason}))
        .send()
        .await;
}

/// The frontend calls this once it has opened (or refused to open) the tab.
#[tauri::command]
pub async fn relay_resolve_open(
    state: State<'_, CloudBridgeState>,
    request_id: String,
    session_id: Option<String>,
    reason: Option<String>,
) -> Result<(), String> {
    resolve_open(
        &state.client,
        &state.inner,
        &request_id,
        session_id.as_deref(),
        reason.as_deref(),
    )
    .await;
    Ok(())
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn relay_provider_status(state: State<'_, CloudBridgeState>) -> Result<RelayProviderStatus, String> {
    let guard = state.inner.lock().map_err(|e| e.to_string())?;
    let share_label = |cloud_id: &str| {
        guard
            .shares
            .values()
            .find(|share| share.cloud_session_id == cloud_id)
            .map(|share| share.label.clone())
            .unwrap_or_default()
    };
    let mut peers: Vec<RelayPeerView> = guard
        .relay
        .pending
        .iter()
        .map(|(connection_id, peer)| RelayPeerView {
            connection_id: connection_id.clone(),
            label: peer.consumer_label.clone(),
            fingerprint: peer.fingerprint.clone(),
            share_label: share_label(&peer.cloud_session_id),
            state: "pending".into(),
            joined_at: None,
            can_control: peer.may_request_control,
            control_requested: false,
        })
        .chain(guard.relay.peers.iter().map(|(connection_id, peer)| RelayPeerView {
            connection_id: connection_id.clone(),
            label: peer.consumer_label.clone(),
            fingerprint: peer.fingerprint.clone(),
            share_label: share_label(&peer.cloud_session_id),
            state: if peer.controller { "controller" } else { "viewer" }.into(),
            joined_at: Some(unix_seconds(peer.joined_at)),
            can_control: peer.may_request_control,
            control_requested: peer.control_requested,
        }))
        .collect();
    peers.sort_by(|a, b| a.connection_id.cmp(&b.connection_id));
    Ok(RelayProviderStatus {
        enabled: guard.relay.policy.enabled,
        peers,
    })
}

/// Local decision on a knock. Admits or denies the pending peer; a stale
/// connection id (peer already gone) is a silent no-op.
#[tauri::command]
pub async fn relay_respond_knock(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
    connection_id: String,
    allow: bool,
    with_control: Option<bool>,
) -> Result<(), String> {
    let pending = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .relay
        .pending
        .remove(&connection_id);
    let Some(peer) = pending else {
        // Not a join knock: an admitted peer asking for control.
        if state.inner.lock().map_err(|e| e.to_string())?.relay.peers.contains_key(&connection_id) {
            return set_relay_control(&state.client, &state.inner, &app, &connection_id, allow).await;
        }
        return Ok(());
    };
    if allow {
        admit(&state.client, &state.inner, &app, &connection_id, peer).await;
        if with_control == Some(true) {
            let _ = set_relay_control(&state.client, &state.inner, &app, &connection_id, true).await;
        }
    } else {
        let frame = json!({"kind": "RELAY_STATE", "state": "denied", "reason": "denied"});
        let _ = send_e2ee_frame(&state.client, &state.inner, &peer.cloud_session_id, &connection_id, &peer.cipher, &frame).await;
        let _ = app.emit("relay-peers-changed", ());
    }
    Ok(())
}

/// Give or take back one peer's write authority from the local UI.
#[tauri::command]
pub async fn relay_set_control(
    app: AppHandle,
    state: State<'_, CloudBridgeState>,
    connection_id: String,
    grant: bool,
) -> Result<(), String> {
    set_relay_control(&state.client, &state.inner, &app, &connection_id, grant).await
}

/// Take control back from every relay peer at once (the panic button).
#[tauri::command]
pub async fn relay_revoke_all_control(app: AppHandle, state: State<'_, CloudBridgeState>) -> Result<(), String> {
    let holders: Vec<String> = {
        let guard = state.inner.lock().map_err(|e| e.to_string())?;
        guard
            .relay
            .peers
            .iter()
            .filter(|(_, peer)| peer.controller)
            .map(|(id, _)| id.clone())
            .collect()
    };
    for connection_id in holders {
        let _ = set_relay_control(&state.client, &state.inner, &app, &connection_id, false).await;
    }
    Ok(())
}

/// Kick one admitted peer (or cancel a pending knock): tell it inside the
/// E2EE channel, then stop fanning output out to it.
#[tauri::command]
pub async fn relay_kick(app: AppHandle, state: State<'_, CloudBridgeState>, connection_id: String) -> Result<(), String> {
    let removed = {
        let mut guard = state.inner.lock().map_err(|e| e.to_string())?;
        let pending = guard.relay.pending.remove(&connection_id);
        let admitted = guard.relay.peers.remove(&connection_id);
        if admitted.as_ref().is_some_and(|peer| peer.controller) {
            guard.relay.fence += 1;
        }
        let cipher = guard.shares.values_mut().find_map(|share| {
            share.peer_roles.remove(&connection_id);
            share.peers.remove(&connection_id)
        });
        match (pending, admitted) {
            (Some(peer), _) => Some((peer.cloud_session_id, peer.cipher)),
            (None, Some(meta)) => cipher.map(|cipher| (meta.cloud_session_id, cipher)),
            (None, None) => None,
        }
    };
    let Some((cloud_id, cipher)) = removed else { return Ok(()) };
    let frame = json!({"kind": "RELAY_CLOSE", "reason": "kicked"});
    let _ = send_e2ee_frame(&state.client, &state.inner, &cloud_id, &connection_id, &cipher, &frame).await;
    let _ = app.emit("relay-peers-changed", ());
    let _ = app.emit("cloud-bridge-peers-changed", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_bridge::test_support::{install_fake_device, install_fake_share};
    use crate::e2ee::DIRECTION_AGENT;
    use crate::relay_client::derive_browser_cipher;
    use crate::shared_session::{SessionProtocol, SharedSessionPort, TxPolicy};
    use crate::terminal_event_hub::{SubscriptionToken, TerminalEvent, TerminalEventHub};
    use async_trait::async_trait;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use p256::{ecdh::EphemeralSecret, elliptic_curve::sec1::ToEncodedPoint};
    use std::time::Duration;

    struct NullPort {
        hub: Arc<TerminalEventHub>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    #[async_trait]
    impl SharedSessionPort for NullPort {
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

    const CLOUD_ID: &str = "cloud-session-1";
    const CONNECTION_ID: &str = "conn-relay-1";

    struct Harness {
        app: tauri::App<tauri::test::MockRuntime>,
        state: CloudBridgeState,
        port: Arc<dyn SharedSessionPort>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        outbound: tokio::sync::mpsc::Receiver<serde_json::Value>,
    }

    fn harness(policy: LiveRelaySettings) -> Harness {
        harness_with_share(policy, TxPolicy::ReadWrite)
    }

    fn harness_with_share(policy: LiveRelaySettings, tx: TxPolicy) -> Harness {
        let hub = Arc::new(TerminalEventHub::new());
        let written = Arc::new(Mutex::new(Vec::new()));
        let port: Arc<dyn SharedSessionPort> = Arc::new(NullPort {
            hub,
            written: Arc::clone(&written),
        });
        let state = CloudBridgeState::new(Arc::clone(&port));
        let outbound = install_fake_device(&state.inner, "http://127.0.0.1:9");
        {
            let mut guard = state.inner.lock().unwrap();
            // A real signing seed so E2EE_READY proofs verify.
            guard.device.as_mut().unwrap().identity_key = URL_SAFE_NO_PAD.encode([9_u8; 32]);
            guard.relay.policy = policy;
            // The global TX gate is fail-closed by default; Live Console's
            // frontend pushes the persisted value at startup.
            guard.allow_remote_send = true;
            guard.terminal_sizes.insert("tab-1".into(), (100, 30));
        }
        let ring = install_fake_share(&state, "tab-1", CLOUD_ID, "prod-web-01", tx);
        ring.lock().unwrap().push(b"host$ ".to_vec());
        Harness {
            app: tauri::test::mock_app(),
            state,
            port,
            written,
            outbound,
        }
    }

    fn enabled_policy(require_approval: bool) -> LiveRelaySettings {
        LiveRelaySettings {
            enabled: true,
            require_approval,
            ..LiveRelaySettings::default()
        }
    }

    /// Drive one consumer up to E2EE_READY; returns its cipher.
    async fn knock(harness: &mut Harness) -> crate::e2ee::PeerCipher {
        knock_as(harness, "relay_viewer", CONNECTION_ID).await
    }

    async fn knock_as(harness: &mut Harness, role: &str, connection_id: &str) -> crate::e2ee::PeerCipher {
        let own_secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
        let own_public = URL_SAFE_NO_PAD.encode(own_secret.public_key().to_encoded_point(false).as_bytes());
        let client = reqwest::Client::new();
        // Identity metadata arrives from the control plane first.
        assert!(
            handle_frame(
                &client,
                &harness.state.inner,
                &harness.port,
                harness.app.handle(),
                &json!({"kind": "RELAY_GRANT_ISSUED", "session_id": CLOUD_ID,
                        "peer_key_hash": sha256_hex(&own_public),
                        "consumer_device_id": "device-b",
                        "consumer_label": "Travel laptop", "role": role}),
            )
            .await
        );
        assert!(
            handle_frame(
                &client,
                &harness.state.inner,
                &harness.port,
                harness.app.handle(),
                &json!({"kind": "E2EE_INIT", "session_id": CLOUD_ID,
                        "connection_id": connection_id,
                        "peer_public_key": own_public, "role": role}),
            )
            .await
        );
        let ready = harness.outbound.recv().await.expect("E2EE_READY");
        assert_eq!(ready["frame"]["kind"], "E2EE_READY");
        derive_browser_cipher(
            CLOUD_ID,
            connection_id,
            own_secret,
            ready["frame"]["agent_public_key"].as_str().unwrap(),
        )
        .unwrap()
    }

    /// Encrypt a consumer frame and push it through the provider hook.
    async fn peer_says(harness: &Harness, connection_id: &str, cipher: &crate::e2ee::PeerCipher, frame: serde_json::Value) {
        let envelope = cipher
            .encrypt(CLOUD_ID, connection_id, DIRECTION_BROWSER, &frame)
            .unwrap();
        let mut outer = envelope;
        outer["connection_id"] = json!(connection_id);
        outer["session_id"] = json!(CLOUD_ID);
        assert!(
            handle_frame(
                &reqwest::Client::new(),
                &harness.state.inner,
                &harness.port,
                harness.app.handle(),
                &outer,
            )
            .await,
            "provider must claim its peer's envelope"
        );
    }

    async fn next_decrypted(harness: &mut Harness, cipher: &crate::e2ee::PeerCipher) -> serde_json::Value {
        next_decrypted_for(harness, CONNECTION_ID, cipher).await
    }

    async fn next_decrypted_for(
        harness: &mut Harness,
        connection_id: &str,
        cipher: &crate::e2ee::PeerCipher,
    ) -> serde_json::Value {
        // Bounded: a missing frame should fail the test, not hang it.
        let envelope = tokio::time::timeout(Duration::from_secs(5), harness.outbound.recv())
            .await
            .expect("timed out waiting for an outbound frame")
            .expect("outbound frame");
        cipher
            .decrypt(CLOUD_ID, connection_id, DIRECTION_AGENT, &envelope["frame"])
            .expect("decryptable envelope")
    }

    #[test]
    fn disabled_policy_denies_after_handshake() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(LiveRelaySettings::default()); // enabled=false
            let cipher = knock(&mut harness).await;
            let state = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(state["kind"], "RELAY_STATE");
            assert_eq!(state["state"], "denied");
            assert_eq!(state["reason"], "relay_disabled");
            assert!(harness.state.inner.lock().unwrap().relay.peers.is_empty());
        });
    }

    #[test]
    fn auto_admission_replays_snapshot_and_registers_peer() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(false));
            let cipher = knock(&mut harness).await;
            let state = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(state["state"], "active");
            assert_eq!(state["role"], "viewer");
            assert_eq!(state["host_label"], "prod-web-01");
            assert_eq!(state["cols"], 100);
            let snapshot = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(snapshot["kind"], "TERMINAL_SNAPSHOT");
            assert_eq!(snapshot["data_hex"], encode_hex(b"host$ "));
            let guard = harness.state.inner.lock().unwrap();
            assert!(guard.relay.peers.contains_key(CONNECTION_ID));
            let share = guard.shares.get("tab-1").unwrap();
            assert!(share.peers.contains_key(CONNECTION_ID));
            assert_eq!(share.peer_roles.get(CONNECTION_ID).map(String::as_str), Some("relay_viewer"));
            drop(guard);
            // Status view carries the control-plane label and the SAS.
            let status_peer = {
                let guard = harness.state.inner.lock().unwrap();
                guard.relay.peers.get(CONNECTION_ID).map(|p| (p.consumer_label.clone(), p.fingerprint.clone())).unwrap()
            };
            assert_eq!(status_peer.0, "Travel laptop");
            assert_eq!(status_peer.1, sas_fingerprint(&cipher.key, SAS_LABEL));
        });
    }

    #[test]
    fn knock_flow_waits_then_admits_and_kick_closes() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(true));
            let cipher = knock(&mut harness).await;
            let state = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(state["state"], "pending_approval");
            assert!(harness.state.inner.lock().unwrap().relay.pending.contains_key(CONNECTION_ID));

            // Approve: active + snapshot, peer registered.
            let pending = harness.state.inner.lock().unwrap().relay.pending.remove(CONNECTION_ID).unwrap();
            admit(&reqwest::Client::new(), &harness.state.inner, harness.app.handle(), CONNECTION_ID, pending).await;
            let state = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(state["state"], "active");
            let snapshot = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(snapshot["kind"], "TERMINAL_SNAPSHOT");

            // Kick: E2EE RELAY_CLOSE and full deregistration.
            let removed = {
                let mut guard = harness.state.inner.lock().unwrap();
                let meta = guard.relay.peers.remove(CONNECTION_ID).unwrap();
                let cipher = guard
                    .shares
                    .values_mut()
                    .find_map(|share| {
                        share.peer_roles.remove(CONNECTION_ID);
                        share.peers.remove(CONNECTION_ID)
                    })
                    .unwrap();
                (meta.cloud_session_id, cipher)
            };
            let frame = json!({"kind": "RELAY_CLOSE", "reason": "kicked"});
            send_e2ee_frame(&reqwest::Client::new(), &harness.state.inner, &removed.0, CONNECTION_ID, &removed.1, &frame)
                .await
                .unwrap();
            let close = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(close["kind"], "RELAY_CLOSE");
            assert_eq!(close["reason"], "kicked");
            let guard = harness.state.inner.lock().unwrap();
            assert!(guard.relay.peers.is_empty());
            assert!(guard.shares.get("tab-1").unwrap().peers.is_empty());
        });
    }

    // ── phase 3: control authority ─────────────────────────────────────────

    /// A read-only (relay_viewer) peer is never offered control, and asking
    /// anyway changes nothing.
    #[test]
    fn viewer_peer_cannot_obtain_control() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(false));
            let cipher = knock(&mut harness).await;
            let state = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(state["control_policy"], "view_only");
            let _snapshot = next_decrypted(&mut harness, &cipher).await;

            peer_says(&harness, CONNECTION_ID, &cipher, json!({"kind": "CONTROL_REQUEST"})).await;
            // Explicit grants are refused too: the ticket was read-only.
            let handle = harness.app.handle().clone();
            let refused = set_relay_control(&reqwest::Client::new(), &harness.state.inner, &handle, CONNECTION_ID, true).await;
            assert!(refused.is_err(), "a viewer ticket must not be grantable");
            assert!(!harness.state.inner.lock().unwrap().relay.peers[CONNECTION_ID].controller);
        });
    }

    /// Auto-grant path: request -> CONTROL_GRANT with a fence -> INPUT is
    /// written to the local session and acked.
    #[test]
    fn controller_peer_types_after_an_auto_grant() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(false));
            let cipher = knock_as(&mut harness, "relay_controller", CONNECTION_ID).await;
            let state = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(state["control_policy"], "on_request");
            let _snapshot = next_decrypted(&mut harness, &cipher).await;

            // Input before any grant is dropped on the floor.
            peer_says(&harness, CONNECTION_ID, &cipher,
                      json!({"kind": "INPUT", "fence": 1, "input_seq": 1, "data_hex": "6e6f"})).await;
            assert!(harness.written.lock().unwrap().is_empty());

            peer_says(&harness, CONNECTION_ID, &cipher, json!({"kind": "CONTROL_REQUEST"})).await;
            let grant = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(grant["kind"], "CONTROL_GRANT");
            let fence = grant["fence"].as_u64().unwrap();
            assert_eq!(fence, 1);

            peer_says(&harness, CONNECTION_ID, &cipher,
                      json!({"kind": "INPUT", "fence": fence, "input_seq": 1, "data_hex": "6c730a"})).await;
            let ack = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(ack["kind"], "INPUT_ACK");
            assert_eq!(ack["byte_count"], 3);
            assert_eq!(harness.written.lock().unwrap().as_slice(), &[b"ls\n".to_vec()]);

            // A replayed input_seq under the same fence is de-duplicated.
            peer_says(&harness, CONNECTION_ID, &cipher,
                      json!({"kind": "INPUT", "fence": fence, "input_seq": 1, "data_hex": "41"})).await;
            assert_eq!(harness.written.lock().unwrap().len(), 1);
        });
    }

    /// Revoking is immediate: the peer is told, and neither the old fence
    /// nor a guessed newer one gets another byte through.
    #[test]
    fn revoke_makes_the_peer_read_only_at_once() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(false));
            let cipher = knock_as(&mut harness, "relay_controller", CONNECTION_ID).await;
            let _state = next_decrypted(&mut harness, &cipher).await;
            let _snapshot = next_decrypted(&mut harness, &cipher).await;
            peer_says(&harness, CONNECTION_ID, &cipher, json!({"kind": "CONTROL_REQUEST"})).await;
            let grant = next_decrypted(&mut harness, &cipher).await;
            let fence = grant["fence"].as_u64().unwrap();
            peer_says(&harness, CONNECTION_ID, &cipher,
                      json!({"kind": "INPUT", "fence": fence, "input_seq": 1, "data_hex": "6c730a"})).await;
            let _ack = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(harness.written.lock().unwrap().len(), 1);

            let handle = harness.app.handle().clone();
            set_relay_control(&reqwest::Client::new(), &harness.state.inner, &handle, CONNECTION_ID, false)
                .await
                .unwrap();
            let revoke = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(revoke["kind"], "CONTROL_REVOKE");

            for (f, seq) in [(fence, 2_u64), (fence + 1, 3)] {
                peer_says(&harness, CONNECTION_ID, &cipher,
                          json!({"kind": "INPUT", "fence": f, "input_seq": seq, "data_hex": "41"})).await;
            }
            assert_eq!(harness.written.lock().unwrap().len(), 1, "no byte may land after a revoke");
        });
    }

    /// Granting control to a second peer demotes the first and bumps the
    /// fence, so the displaced controller's queued input is stale.
    #[test]
    fn granting_control_displaces_the_previous_controller() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(false));
            let first = knock_as(&mut harness, "relay_controller", CONNECTION_ID).await;
            let _s = next_decrypted(&mut harness, &first).await;
            let _snap = next_decrypted(&mut harness, &first).await;
            peer_says(&harness, CONNECTION_ID, &first, json!({"kind": "CONTROL_REQUEST"})).await;
            let grant = next_decrypted(&mut harness, &first).await;
            let old_fence = grant["fence"].as_u64().unwrap();

            let second = knock_as(&mut harness, "relay_controller", "conn-relay-2").await;
            let _s2 = next_decrypted_for(&mut harness, "conn-relay-2", &second).await;
            let _snap2 = next_decrypted_for(&mut harness, "conn-relay-2", &second).await;

            let handle = harness.app.handle().clone();
            set_relay_control(&reqwest::Client::new(), &harness.state.inner, &handle, "conn-relay-2", true)
                .await
                .unwrap();
            // The displaced peer hears about it; the new one gets the fence.
            let revoke = next_decrypted(&mut harness, &first).await;
            assert_eq!(revoke["kind"], "CONTROL_REVOKE");
            assert_eq!(revoke["reason"], "replaced");
            let grant2 = next_decrypted_for(&mut harness, "conn-relay-2", &second).await;
            assert_eq!(grant2["kind"], "CONTROL_GRANT");
            assert!(grant2["fence"].as_u64().unwrap() > old_fence);

            // Old controller typing on the old fence is ignored.
            peer_says(&harness, CONNECTION_ID, &first,
                      json!({"kind": "INPUT", "fence": old_fence, "input_seq": 9, "data_hex": "41"})).await;
            assert!(harness.written.lock().unwrap().is_empty());
        });
    }

    /// The global Live Console TX gate covers Live Relay: one switch off,
    /// no relay peer can type, whatever the relay policy says.
    #[test]
    fn global_remote_send_gate_blocks_relay_input() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(false));
            let cipher = knock_as(&mut harness, "relay_controller", CONNECTION_ID).await;
            let _s = next_decrypted(&mut harness, &cipher).await;
            let _snap = next_decrypted(&mut harness, &cipher).await;
            peer_says(&harness, CONNECTION_ID, &cipher, json!({"kind": "CONTROL_REQUEST"})).await;
            let grant = next_decrypted(&mut harness, &cipher).await;
            let fence = grant["fence"].as_u64().unwrap();

            harness.state.inner.lock().unwrap().allow_remote_send = false;
            peer_says(&harness, CONNECTION_ID, &cipher,
                      json!({"kind": "INPUT", "fence": fence, "input_seq": 1, "data_hex": "41"})).await;
            assert!(harness.written.lock().unwrap().is_empty());
        });
    }

    /// With require_approval the request raises a knock instead of granting.
    #[test]
    fn control_request_raises_a_knock_when_approval_is_required() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness(enabled_policy(true));
            let cipher = knock_as(&mut harness, "relay_controller", CONNECTION_ID).await;
            let _pending = next_decrypted(&mut harness, &cipher).await;
            let waiting = harness.state.inner.lock().unwrap().relay.pending.remove(CONNECTION_ID).unwrap();
            let handle = harness.app.handle().clone();
            admit(&reqwest::Client::new(), &harness.state.inner, &handle, CONNECTION_ID, waiting).await;
            let _active = next_decrypted(&mut harness, &cipher).await;
            let _snap = next_decrypted(&mut harness, &cipher).await;

            peer_says(&harness, CONNECTION_ID, &cipher, json!({"kind": "CONTROL_REQUEST"})).await;
            {
                let guard = harness.state.inner.lock().unwrap();
                let peer = &guard.relay.peers[CONNECTION_ID];
                assert!(peer.control_requested, "the request must be parked for the local user");
                assert!(!peer.controller, "approval required means no auto-grant");
            }
            // Answering yes grants it.
            set_relay_control(&reqwest::Client::new(), &harness.state.inner, &handle, CONNECTION_ID, true)
                .await
                .unwrap();
            let grant = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(grant["kind"], "CONTROL_GRANT");
        });
    }

    /// The share's own TX policy outranks a granted controller: a read-only
    /// share never takes a byte, however the control state looks.
    #[test]
    fn read_only_share_refuses_input_from_a_controller() {
        tauri::async_runtime::block_on(async {
            let mut harness = harness_with_share(enabled_policy(false), TxPolicy::ReadOnly);
            let cipher = knock_as(&mut harness, "relay_controller", CONNECTION_ID).await;
            let _state = next_decrypted(&mut harness, &cipher).await;
            let _snapshot = next_decrypted(&mut harness, &cipher).await;
            peer_says(&harness, CONNECTION_ID, &cipher, json!({"kind": "CONTROL_REQUEST"})).await;
            let grant = next_decrypted(&mut harness, &cipher).await;
            let fence = grant["fence"].as_u64().unwrap();

            peer_says(&harness, CONNECTION_ID, &cipher,
                      json!({"kind": "INPUT", "fence": fence, "input_seq": 1, "data_hex": "41"})).await;
            assert!(harness.written.lock().unwrap().is_empty(),
                    "a read-only share must refuse TX even under a live grant");
        });
    }

    // ── phase 4: the open gate ─────────────────────────────────────────────

    fn open_state(policy: LiveRelaySettings, targets: Vec<RelayOpenTarget>) -> RelayProviderState {
        RelayProviderState {
            policy,
            targets,
            ..RelayProviderState::default()
        }
    }

    fn shell_target() -> RelayOpenTarget {
        RelayOpenTarget {
            kind: "local_shell".into(),
            id: "shell".into(),
            label: "Local Shell".into(),
        }
    }

    /// Every gate defaults closed: a fresh install refuses every kind.
    #[test]
    fn open_is_refused_by_default() {
        let state = open_state(LiveRelaySettings::default(), vec![shell_target()]);
        assert_eq!(decide_open(&state, "local_shell", "shell"), Err("relay_disabled"));

        // Relay on, but the per-kind switches are still off.
        let mut policy = enabled_policy(false);
        policy.allow_open_shell = false;
        let state = open_state(policy, vec![shell_target()]);
        assert_eq!(decide_open(&state, "local_shell", "shell"), Err("open_kind_disabled"));
        assert_eq!(decide_open(&state, "serial", "bm:1"), Err("open_kind_disabled"));
        assert_eq!(decide_open(&state, "bookmark", "bm:1"), Err("open_kind_disabled"));
    }

    /// A kind switch only opens its own kind.
    #[test]
    fn open_switches_are_per_kind() {
        let mut policy = enabled_policy(false);
        policy.allow_open_shell = true;
        let targets = vec![
            shell_target(),
            RelayOpenTarget { kind: "serial".into(), id: "bm:s".into(), label: "COM3".into() },
            RelayOpenTarget { kind: "bookmark".into(), id: "bm:b".into(), label: "prod".into() },
        ];
        let state = open_state(policy, targets);
        assert_eq!(decide_open(&state, "local_shell", "shell").unwrap().id, "shell");
        assert_eq!(decide_open(&state, "serial", "bm:s"), Err("open_kind_disabled"));
        assert_eq!(decide_open(&state, "bookmark", "bm:b"), Err("open_kind_disabled"));
    }

    /// A peer may only name an id this device itself advertised — no
    /// guessing, no stale ids, and never a program or path of its own.
    #[test]
    fn open_only_accepts_an_advertised_target_id() {
        let mut policy = enabled_policy(false);
        policy.allow_open_bookmark = true;
        let state = open_state(policy, vec![RelayOpenTarget {
            kind: "bookmark".into(),
            id: "bm:known".into(),
            label: "prod-web".into(),
        }]);
        assert_eq!(decide_open(&state, "bookmark", "bm:known").unwrap().label, "prod-web");
        assert_eq!(decide_open(&state, "bookmark", "bm:guessed"), Err("unknown_target"));
        // A shell-shaped id under a kind it was not advertised for is not a
        // way in either.
        assert_eq!(decide_open(&state, "bookmark", "shell"), Err("unknown_target"));
        assert_eq!(decide_open(&state, "not_a_kind", "bm:known"), Err("open_kind_disabled"));
    }

    /// The concurrency cap covers opening, not just attaching.
    #[test]
    fn open_respects_the_concurrency_cap() {
        tauri::async_runtime::block_on(async {
            let mut policy = enabled_policy(false);
            policy.allow_open_shell = true;
            policy.max_concurrent_peers = 1;
            let mut harness = harness(policy);
            harness.state.inner.lock().unwrap().relay.targets = vec![shell_target()];
            // No peers yet: allowed.
            {
                let guard = harness.state.inner.lock().unwrap();
                assert!(decide_open(&guard.relay, "local_shell", "shell").is_ok());
            }
            // One peer attaches, filling the cap.
            let cipher = knock(&mut harness).await;
            let _state = next_decrypted(&mut harness, &cipher).await;
            let _snapshot = next_decrypted(&mut harness, &cipher).await;
            let guard = harness.state.inner.lock().unwrap();
            assert_eq!(decide_open(&guard.relay, "local_shell", "shell"), Err("busy"));
        });
    }

    #[test]
    fn concurrency_cap_denies_with_busy() {
        tauri::async_runtime::block_on(async {
            let mut policy = enabled_policy(false);
            policy.max_concurrent_peers = 1;
            let mut harness = harness(policy);
            let cipher = knock(&mut harness).await;
            let state = next_decrypted(&mut harness, &cipher).await;
            assert_eq!(state["state"], "active");
            let _snapshot = next_decrypted(&mut harness, &cipher).await;
            // Second consumer on a fresh connection id hits the cap.
            let own_secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
            let own_public = URL_SAFE_NO_PAD.encode(own_secret.public_key().to_encoded_point(false).as_bytes());
            let client = reqwest::Client::new();
            handle_frame(
                &client,
                &harness.state.inner,
                &harness.port,
                harness.app.handle(),
                &json!({"kind": "E2EE_INIT", "session_id": CLOUD_ID,
                        "connection_id": "conn-relay-2",
                        "peer_public_key": own_public, "role": "relay_viewer"}),
            )
            .await;
            let ready = harness.outbound.recv().await.unwrap();
            assert_eq!(ready["frame"]["kind"], "E2EE_READY");
            let second = derive_browser_cipher(CLOUD_ID, "conn-relay-2", own_secret, ready["frame"]["agent_public_key"].as_str().unwrap()).unwrap();
            let envelope = harness.outbound.recv().await.unwrap();
            let state = second.decrypt(CLOUD_ID, "conn-relay-2", DIRECTION_AGENT, &envelope["frame"]).unwrap();
            assert_eq!(state["state"], "denied");
            assert_eq!(state["reason"], "busy");
        });
    }
}
