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
    derive_peer_cipher, e2ee_context, ed25519_sign_b64, encode_hex, send_e2ee_frame, send_frame,
    sha256_hex, BridgeInner, CloudBridgeState,
};
use crate::e2ee::{sas_fingerprint, PeerCipher};
use crate::settings::LiveRelaySettings;
use serde::Serialize;
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
}

/// An admitted relay peer (also registered in the share's peer map, which
/// is what actually fans terminal output out to it).
pub(crate) struct RelayPeerMeta {
    cloud_session_id: String,
    fingerprint: String,
    consumer_label: String,
    /// Kept for phase 3's per-device audit trail and revocation UX.
    #[allow(dead_code)]
    consumer_device_id: String,
    joined_at: SystemTime,
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
    /// "pending" | "viewer"
    pub state: String,
    pub joined_at: Option<u64>,
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
}

// ── inbound frame hook (called from cloud_bridge::process_inbound_frame) ────

/// Returns true when the frame was consumed by the relay provider. Cleanup
/// frames (E2EE_CLOSE, SESSION_UNSHARE) update relay state but return
/// false so the generic handler keeps its own bookkeeping.
pub(crate) async fn handle_frame<R: tauri::Runtime>(
    client: &reqwest::Client,
    inner: &Arc<Mutex<BridgeInner>>,
    app: &AppHandle<R>,
    frame: &serde_json::Value,
) -> bool {
    let kind = frame.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
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
        "E2EE_CLOSE" => {
            if let Some(connection_id) = frame.get("connection_id").and_then(|v| v.as_str()) {
                let changed = if let Ok(mut guard) = inner.lock() {
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

    if policy.require_approval {
        let state = json!({"kind": "RELAY_STATE", "state": "pending_approval",
            "role": "viewer", "control_policy": "view_only",
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
    let state = json!({"kind": "RELAY_STATE", "state": "active", "role": "viewer",
        "control_policy": "view_only", "host_label": share_label,
        "cols": cols, "rows": rows});
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
            },
        );
    }
    let _ = app.emit("relay-peers-changed", ());
    let _ = app.emit("cloud-bridge-peers-changed", ());
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
        })
        .chain(guard.relay.peers.iter().map(|(connection_id, peer)| RelayPeerView {
            connection_id: connection_id.clone(),
            label: peer.consumer_label.clone(),
            fingerprint: peer.fingerprint.clone(),
            share_label: share_label(&peer.cloud_session_id),
            state: "viewer".into(),
            joined_at: Some(unix_seconds(peer.joined_at)),
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
) -> Result<(), String> {
    let pending = state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .relay
        .pending
        .remove(&connection_id);
    let Some(peer) = pending else { return Ok(()) };
    if allow {
        admit(&state.client, &state.inner, &app, &connection_id, peer).await;
    } else {
        let frame = json!({"kind": "RELAY_STATE", "state": "denied", "reason": "denied"});
        let _ = send_e2ee_frame(&state.client, &state.inner, &peer.cloud_session_id, &connection_id, &peer.cipher, &frame).await;
        let _ = app.emit("relay-peers-changed", ());
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
    use crate::shared_session::{SessionProtocol, SharedSessionPort};
    use crate::terminal_event_hub::{SubscriptionToken, TerminalEvent, TerminalEventHub};
    use async_trait::async_trait;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use p256::{ecdh::EphemeralSecret, elliptic_curve::sec1::ToEncodedPoint};

    struct NullPort {
        hub: Arc<TerminalEventHub>,
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
        async fn write_tx(&self, _protocol: SessionProtocol, _session_id: &str, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }
    }

    const CLOUD_ID: &str = "cloud-session-1";
    const CONNECTION_ID: &str = "conn-relay-1";

    struct Harness {
        app: tauri::App<tauri::test::MockRuntime>,
        state: CloudBridgeState,
        outbound: tokio::sync::mpsc::Receiver<serde_json::Value>,
    }

    fn harness(policy: LiveRelaySettings) -> Harness {
        let hub = Arc::new(TerminalEventHub::new());
        let port: Arc<dyn SharedSessionPort> = Arc::new(NullPort { hub });
        let state = CloudBridgeState::new(port);
        let outbound = install_fake_device(&state.inner, "http://127.0.0.1:9");
        {
            let mut guard = state.inner.lock().unwrap();
            // A real signing seed so E2EE_READY proofs verify.
            guard.device.as_mut().unwrap().identity_key = URL_SAFE_NO_PAD.encode([9_u8; 32]);
            guard.relay.policy = policy;
            guard.terminal_sizes.insert("tab-1".into(), (100, 30));
        }
        let ring = install_fake_share(&state, "tab-1", CLOUD_ID, "prod-web-01");
        ring.lock().unwrap().push(b"host$ ".to_vec());
        Harness {
            app: tauri::test::mock_app(),
            state,
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
        let own_secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
        let own_public = URL_SAFE_NO_PAD.encode(own_secret.public_key().to_encoded_point(false).as_bytes());
        let client = reqwest::Client::new();
        // Identity metadata arrives from the control plane first.
        assert!(
            handle_frame(
                &client,
                &harness.state.inner,
                harness.app.handle(),
                &json!({"kind": "RELAY_GRANT_ISSUED", "session_id": CLOUD_ID,
                        "peer_key_hash": sha256_hex(&own_public),
                        "consumer_device_id": "device-b",
                        "consumer_label": "Travel laptop", "role": "relay_viewer"}),
            )
            .await
        );
        assert!(
            handle_frame(
                &client,
                &harness.state.inner,
                harness.app.handle(),
                &json!({"kind": "E2EE_INIT", "session_id": CLOUD_ID,
                        "connection_id": CONNECTION_ID,
                        "peer_public_key": own_public, "role": "relay_viewer"}),
            )
            .await
        );
        let ready = harness.outbound.recv().await.expect("E2EE_READY");
        assert_eq!(ready["frame"]["kind"], "E2EE_READY");
        derive_browser_cipher(
            CLOUD_ID,
            CONNECTION_ID,
            own_secret,
            ready["frame"]["agent_public_key"].as_str().unwrap(),
        )
        .unwrap()
    }

    async fn next_decrypted(harness: &mut Harness, cipher: &crate::e2ee::PeerCipher) -> serde_json::Value {
        let envelope = harness.outbound.recv().await.expect("outbound frame");
        cipher
            .decrypt(CLOUD_ID, CONNECTION_ID, DIRECTION_AGENT, &envelope["frame"])
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
