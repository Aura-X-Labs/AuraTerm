//! Live Relay — consumer side (design `docs/plans/live-sync-design.md` §5).
//!
//! A `relay` tab is a terminal mirroring a session that another AuraTerm on
//! the *same account* shares: list the account's devices with this device's
//! credential, ask AuraXLab for a one-time relay grant (signed with our
//! identity key over a fresh challenge nonce), join the relay's browser
//! channel, run the ECDH handshake — and verify the provider's identity
//! signature *locally* with the key the grant response carried, so the
//! handshake never trusts the server a second time. Everything after key
//! agreement is the shared `remote_tab` plumbing.
//!
//! Phase 2 is attach/view-only: the relay itself refuses upstream frames
//! from non-controller browsers, so this side sends nothing after AUTH.

use crate::cloud_bridge::{
    e2ee_context, ed25519_sign_b64, ed25519_verify, sha256_hex, CloudBridgeState, DeviceConfig,
};
use crate::e2ee::{sas_fingerprint, PeerCipher, DIRECTION_AGENT, DIRECTION_BROWSER};
use crate::relay_provider::SAS_LABEL;
use crate::remote_tab::{self, FrameClass, RemoteTabs, TabProtocol, TabStateEvent};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hkdf::Hkdf;
use p256::{ecdh::EphemeralSecret, elliptic_curve::sec1::ToEncodedPoint, PublicKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use tauri::{AppHandle, State};

fn classify_relay_frame(kind: &str) -> FrameClass {
    match kind {
        "RELAY_STATE" => FrameClass::PeerState,
        "TERMINAL_SNAPSHOT" => FrameClass::Snapshot,
        "OUTPUT" => FrameClass::Output,
        "RESIZE" => FrameClass::Resize,
        // Phase 3 vocabulary, already understood by the shared loop.
        "CONTROL_GRANT" => FrameClass::ControlGrant,
        "CONTROL_REVOKE" => FrameClass::ControlRevoke,
        "RELAY_CLOSE" => FrameClass::SessionEnd,
        _ => FrameClass::Ignore,
    }
}

static RELAY_TAB_PROTOCOL: TabProtocol = TabProtocol {
    state_event: "relay-client-state",
    own_direction: DIRECTION_BROWSER,
    peer_direction: DIRECTION_AGENT,
    ended_message: "Live Relay ended",
    default_end_reason: "disconnected",
    classify: classify_relay_frame,
};

pub struct RelayClientState {
    pub(crate) tabs: RemoteTabs,
}

impl Default for RelayClientState {
    fn default() -> Self {
        Self {
            tabs: RemoteTabs::new(&RELAY_TAB_PROTOCOL),
        }
    }
}

// ── server JSON (snake_case passthrough, camelCase toward the frontend) ─────

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RelayAttachTarget {
    pub session_id: String,
    #[serde(default)]
    pub share_label: Option<String>,
    #[serde(default)]
    pub source_protocol: Option<String>,
    #[serde(default)]
    pub tx_policy: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct RelayDeviceEntry {
    pub device_id: String,
    pub label: String,
    #[serde(default)]
    pub platform: Option<String>,
    pub presence: String,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub relay_policy: Option<serde_json::Value>,
    #[serde(default)]
    pub attach_targets: Vec<RelayAttachTarget>,
}

#[derive(Deserialize)]
struct RelayDeviceListing {
    devices: Vec<RelayDeviceEntry>,
}

#[derive(Deserialize)]
struct ChallengeResponse {
    nonce: String,
}

#[derive(Deserialize)]
struct RelayGrantResponse {
    session_id: String,
    ticket: String,
    relay_url: String,
    provider_device_id: String,
    provider_label: String,
    provider_identity_public_key: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayJoinView {
    pub session_id: String,
    pub connection_id: String,
    pub fingerprint: String,
    pub provider_label: String,
}

fn enrolled_device(state: &CloudBridgeState) -> Result<DeviceConfig, String> {
    state
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .device
        .clone()
        .ok_or_else(|| "This device is not bound to an AuraXLab account.".to_string())
}

async fn api_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let code = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
    match code {
        "FORBIDDEN" => "Live Relay is not enabled on the target device.".into(),
        "DEVICE_OFFLINE" => "The target device is not connected right now.".into(),
        "SESSION_ENDED" | "SESSION_NOT_FOUND" => "That shared session has ended.".into(),
        "QUOTA_EXCEEDED" => "Too many peers are attached to that session.".into(),
        _ => format!(
            "AuraXLab refused the request ({}): {}",
            status,
            body.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error")
        ),
    }
}

/// List the account's other devices (label, presence, policy, attachable
/// shared sessions), authenticated with this device's credential.
#[tauri::command]
pub async fn relay_list_devices(state: State<'_, CloudBridgeState>) -> Result<Vec<RelayDeviceEntry>, String> {
    let device = enrolled_device(state.inner())?;
    let response = state
        .client
        .get(format!("{}/api/v1/auraterm/console/relay/devices", device.base_url))
        .header("Authorization", format!("Bearer {}", device.credential))
        .send()
        .await
        .map_err(|e| format!("could not reach AuraXLab: {e}"))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let listing: RelayDeviceListing = response.json().await.map_err(|e| e.to_string())?;
    Ok(listing.devices)
}

/// Attach to `session_id` on `device_id` as a new terminal tab `id`.
#[tauri::command]
pub async fn relay_connect(
    app: AppHandle,
    bridge: State<'_, CloudBridgeState>,
    state: State<'_, RelayClientState>,
    id: String,
    device_id: String,
    session_id: String,
    want_control: Option<bool>,
) -> Result<RelayJoinView, String> {
    let device = enrolled_device(bridge.inner())?;
    connect_session(
        &app, &bridge.client, state.inner(), &device, id, device_id, session_id,
        want_control.unwrap_or(false),
    )
    .await
}

pub(crate) async fn connect_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    http: &reqwest::Client,
    state: &RelayClientState,
    device: &DeviceConfig,
    id: String,
    target_device_id: String,
    session_id: String,
    want_control: bool,
) -> Result<RelayJoinView, String> {
    // "controller" only buys the right to *ask*: the relay then forwards
    // our frames, and the provider decides whether they become keystrokes.
    // On a read-only share we stay a viewer so the relay drops our frames
    // outright rather than the provider having to.
    let role = if want_control { "controller" } else { "viewer" };
    let bearer = format!("Bearer {}", device.credential);

    // ── our half of the E2EE handshake, minted before the grant so the
    // ticket binds to exactly this key ────────────────────────────────────
    let own_secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
    let own_public = URL_SAFE_NO_PAD.encode(own_secret.public_key().to_encoded_point(false).as_bytes());

    // ── challenge + identity-signed grant request ─────────────────────────
    let response = http
        .post(format!("{}/api/v1/auraterm/console/connect-challenge", device.base_url))
        .header("Authorization", &bearer)
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| format!("could not reach AuraXLab: {e}"))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let challenge: ChallengeResponse = response.json().await.map_err(|e| e.to_string())?;
    let context = format!(
        "auraxlab-console|relay-grant|{}|{}|attach|{}|{}|{}",
        device.device_id,
        target_device_id,
        session_id,
        sha256_hex(&own_public),
        challenge.nonce
    );
    // The role is deliberately outside the signed context: it is authorised
    // by the share's own tx_policy server-side, and re-checked on the
    // device. Binding it here would buy nothing and break old proofs.
    let proof = ed25519_sign_b64(&device.identity_key, &context)?;
    let response = http
        .post(format!("{}/api/v1/auraterm/console/relay/grants", device.base_url))
        .header("Authorization", &bearer)
        .json(&json!({
            "target_device_id": target_device_id, "mode": "attach",
            "session_id": session_id, "role": role,
            "e2ee_public_key": own_public,
            "nonce": challenge.nonce, "proof": proof,
        }))
        .send()
        .await
        .map_err(|e| format!("could not reach AuraXLab: {e}"))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let grant: RelayGrantResponse = response.json().await.map_err(|e| e.to_string())?;

    // ── relay admission on the browser channel ────────────────────────────
    let admission = remote_tab::connect_relay(&grant.relay_url, &grant.ticket).await?;
    let connection_id = admission.connection_id;
    let outbound = admission.outbound;
    let mut stream = admission.stream;

    // ── E2EE_READY: verify the provider's identity locally ────────────────
    let ready = loop {
        let frame = remote_tab::recv_text(&mut stream).await?;
        match frame.get("kind").and_then(|v| v.as_str()) {
            Some("E2EE_READY") => break frame,
            Some("SESSION_END") => return Err("The shared session ended before the handshake.".into()),
            _ => continue,
        }
    };
    let agent_public = ready
        .get("agent_public_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "malformed E2EE_READY".to_string())?
        .to_string();
    let proof = ready.get("proof").and_then(|v| v.as_str()).unwrap_or("");
    let context = e2ee_context(
        &grant.provider_device_id,
        &grant.session_id,
        &connection_id,
        &own_public,
        &agent_public,
    );
    let identity = URL_SAFE_NO_PAD
        .decode(&grant.provider_identity_public_key)
        .map_err(|_| "malformed provider identity key".to_string())?;
    let signature = URL_SAFE_NO_PAD
        .decode(proof)
        .map_err(|_| "malformed provider proof".to_string())?;
    if !ed25519_verify(&identity, context.as_bytes(), &signature) {
        // Wrong or forged provider identity: never render a byte from it.
        drop(outbound);
        return Err("The other device failed its identity check — connection refused.".into());
    }

    // ── shared-secret derivation, mirroring the provider exactly ──────────
    let cipher = derive_browser_cipher(&grant.session_id, &connection_id, own_secret, &agent_public)?;
    let fingerprint = sas_fingerprint(&cipher.key, SAS_LABEL);

    // ── hand the keyed connection to the shared remote-tab plumbing ───────
    state.tabs.insert(&id, &grant.session_id, &connection_id, cipher, outbound);
    state.tabs.emit_state(
        app,
        TabStateEvent {
            id: id.clone(),
            state: "handshake".into(),
            role: "viewer".into(),
            cols: None,
            rows: None,
            host_label: Some(grant.provider_label.clone()),
            fingerprint: Some(fingerprint.clone()),
            // The provider's first RELAY_STATE replaces this with the real
            // policy; until then assume the conservative answer.
            control_policy: Some("view_only".into()),
            reason: None,
        },
    );
    state.tabs.spawn_reader(app.clone(), id, fingerprint.clone(), stream);
    Ok(RelayJoinView {
        session_id: grant.session_id,
        connection_id,
        fingerprint,
        provider_label: grant.provider_label,
    })
}

/// The browser half of the provider's `derive_peer_cipher`: same HKDF info,
/// same zero salt, our ephemeral secret against the agent's public key.
pub(crate) fn derive_browser_cipher(
    session_id: &str,
    connection_id: &str,
    own_secret: EphemeralSecret,
    agent_public_b64: &str,
) -> Result<PeerCipher, String> {
    let agent_bytes = URL_SAFE_NO_PAD
        .decode(agent_public_b64)
        .map_err(|_| "invalid agent E2EE public key".to_string())?;
    let agent = PublicKey::from_sec1_bytes(&agent_bytes).map_err(|_| "invalid agent E2EE public key".to_string())?;
    let shared = own_secret.diffie_hellman(&agent);
    let mut key = [0_u8; 32];
    let info = format!("auraxlab-console|e2ee-v1|{session_id}|{connection_id}");
    Hkdf::<Sha256>::new(Some(&[0_u8; 32]), shared.raw_secret_bytes())
        .expand(info.as_bytes(), &mut key)
        .map_err(|_| "could not derive E2EE key".to_string())?;
    Ok(PeerCipher::new(key))
}

/// Keystrokes from a relay tab; dropped locally unless the provider granted
/// control and announced a fence (`RemoteTabs::write_input`).
#[tauri::command]
pub async fn write_relay_input(state: State<'_, RelayClientState>, id: String, data: String) -> Result<(), String> {
    state.inner().tabs.write_input(&id, &data).await
}

#[tauri::command]
pub async fn relay_request_control(state: State<'_, RelayClientState>, id: String) -> Result<(), String> {
    state.inner().tabs.send(&id, json!({"kind": "CONTROL_REQUEST"})).await
}

#[tauri::command]
pub async fn relay_release_control(state: State<'_, RelayClientState>, id: String) -> Result<(), String> {
    // Stop sending immediately; the provider confirms with CONTROL_REVOKE.
    state.inner().tabs.set_role(&id, "viewer");
    state.inner().tabs.send(&id, json!({"kind": "CONTROL_RELEASE"})).await
}

#[tauri::command]
pub fn close_relay_session(state: State<'_, RelayClientState>, id: String) -> Result<(), String> {
    state.inner().tabs.close(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_bridge::derive_peer_cipher;
    use crate::cloud_bridge::test_support::install_fake_device;
    use crate::shared_session::{SessionProtocol, SharedSessionPort};
    use crate::terminal_event_hub::{SubscriptionToken, TerminalEvent, TerminalEventHub};
    use async_trait::async_trait;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

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

    const SESSION_ID: &str = "cloud-session-e2e";
    const CONNECTION_ID: &str = "conn-e2e-1";
    const PROVIDER_DEVICE: &str = "device-provider";
    const PROVIDER_SEED: [u8; 32] = [4_u8; 32];

    fn provider_identity_public() -> String {
        use ed25519_dalek::SigningKey;
        URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(&PROVIDER_SEED).verifying_key().to_bytes())
    }

    /// Minimal AuraXLab: answers connect-challenge and relay/grants,
    /// capturing the consumer's E2EE public key for the fake provider.
    fn fake_auraxlab(relay_url: String, identity_public: String, captured_key: Arc<Mutex<Option<String>>>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(&mut request.as_reader(), &mut body);
                let payload: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let (status, response) = match request.url() {
                    "/api/v1/auraterm/console/connect-challenge" => {
                        (200, json!({"nonce": "nonce-1", "expires_in": 60}))
                    }
                    "/api/v1/auraterm/console/relay/grants" => {
                        assert_eq!(payload["mode"], "attach");
                        assert_eq!(payload["role"], "viewer");
                        assert_eq!(payload["nonce"], "nonce-1");
                        assert!(payload["proof"].as_str().is_some_and(|p| !p.is_empty()));
                        *captured_key.lock().unwrap() =
                            payload["e2ee_public_key"].as_str().map(str::to_string);
                        (201, json!({
                            "session_id": SESSION_ID, "ticket": "tick-relay",
                            "relay_url": relay_url, "role": "relay_viewer",
                            "expires_at": "2026-01-01T00:00:00Z",
                            "provider_device_id": PROVIDER_DEVICE,
                            "provider_label": "Office desktop",
                            "provider_identity_public_key": identity_public,
                        }))
                    }
                    other => panic!("unexpected request: {other}"),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(response.to_string())
                        .with_status_code(status)
                        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()),
                );
            }
        });
        origin
    }

    /// Fake relay + provider in one WebSocket server: AUTH_OK, then the
    /// provider side of the handshake (signed E2EE_READY), an active
    /// RELAY_STATE, a snapshot, one OUTPUT chunk and a kick.
    async fn fake_relay_provider(captured_key: Arc<Mutex<Option<String>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tauri::async_runtime::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else { return };
            let (mut sink, mut stream) = ws.split();
            // AUTH
            let Some(Ok(Message::Text(text))) = stream.next().await else { return };
            let auth: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(auth["kind"], "AUTH");
            assert_eq!(auth["ticket"], "tick-relay");
            let _ = sink
                .send(Message::Text(json!({"kind": "AUTH_OK", "connection_id": CONNECTION_ID, "session_id": SESSION_ID, "role": "relay_viewer"}).to_string().into()))
                .await;
            // Provider handshake: the consumer key arrived via the grant.
            let peer_public = loop {
                if let Some(key) = captured_key.lock().unwrap().clone() {
                    break key;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            let (cipher, agent_public) = derive_peer_cipher(SESSION_ID, CONNECTION_ID, &peer_public).unwrap();
            let context = e2ee_context(PROVIDER_DEVICE, SESSION_ID, CONNECTION_ID, &peer_public, &agent_public);
            let proof = ed25519_sign_b64(&URL_SAFE_NO_PAD.encode(PROVIDER_SEED), &context).unwrap();
            let _ = sink
                .send(Message::Text(json!({"kind": "E2EE_READY", "connection_id": CONNECTION_ID, "agent_public_key": agent_public, "proof": proof}).to_string().into()))
                .await;
            for frame in [
                json!({"kind": "RELAY_STATE", "state": "active", "role": "viewer",
                       "control_policy": "view_only", "host_label": "prod-web-01",
                       "cols": 100, "rows": 30}),
                json!({"kind": "TERMINAL_SNAPSHOT", "snapshot_seq": 1, "cols": 100,
                       "rows": 30, "data_hex": "686f73742420"}),
                json!({"kind": "OUTPUT", "output_seq": 2, "data_hex": "6c73"}),
                json!({"kind": "RELAY_CLOSE", "reason": "kicked"}),
            ] {
                let envelope = cipher.encrypt(SESSION_ID, CONNECTION_ID, DIRECTION_AGENT, &frame).unwrap();
                let _ = sink.send(Message::Text(envelope.to_string().into())).await;
            }
        });
        url
    }

    fn test_device(origin: &str) -> DeviceConfig {
        // Reuse the fake enrollment (private fields), then point it at the
        // fake server with a real consumer signing seed.
        let hub = Arc::new(TerminalEventHub::new());
        let bridge = CloudBridgeState::new(Arc::new(NullPort { hub }));
        install_fake_device(&bridge.inner, origin);
        let mut device = bridge.inner.lock().unwrap().device.clone().unwrap();
        device.identity_key = URL_SAFE_NO_PAD.encode([6_u8; 32]);
        device
    }

    async fn wait_for<F: Fn() -> bool>(predicate: F, what: &str) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn consumer_attaches_verifies_identity_and_ends_on_kick() {
        tauri::async_runtime::block_on(async {
            let captured = Arc::new(Mutex::new(None));
            let relay_url = fake_relay_provider(Arc::clone(&captured)).await;
            let origin = fake_auraxlab(relay_url, provider_identity_public(), Arc::clone(&captured));
            let app = tauri::test::mock_app();
            let state = RelayClientState::default();
            let device = test_device(&origin);
            let joined = connect_session(
                app.handle(), &reqwest::Client::new(), &state, &device,
                "tab-relay".into(), PROVIDER_DEVICE.into(), SESSION_ID.into(), false,
            )
            .await
            .expect("attach succeeds");
            assert_eq!(joined.session_id, SESSION_ID);
            assert_eq!(joined.connection_id, CONNECTION_ID);
            assert_eq!(joined.provider_label, "Office desktop");
            assert_eq!(joined.fingerprint.len(), 9); // XXXX-XXXX
            // The kick ends the tab; the registry empties itself.
            wait_for(|| state.tabs.is_empty(), "RELAY_CLOSE teardown").await;
        });
    }

    #[test]
    fn forged_provider_identity_is_refused() {
        tauri::async_runtime::block_on(async {
            let captured = Arc::new(Mutex::new(None));
            let relay_url = fake_relay_provider(Arc::clone(&captured)).await;
            // The grant response names a different identity key than the
            // one that signs E2EE_READY: the consumer must walk away.
            let wrong_identity = URL_SAFE_NO_PAD.encode(
                ed25519_dalek::SigningKey::from_bytes(&[8_u8; 32]).verifying_key().to_bytes(),
            );
            let origin = fake_auraxlab(relay_url, wrong_identity, Arc::clone(&captured));
            let app = tauri::test::mock_app();
            let state = RelayClientState::default();
            let device = test_device(&origin);
            let error = connect_session(
                app.handle(), &reqwest::Client::new(), &state, &device,
                "tab-bad".into(), PROVIDER_DEVICE.into(), SESSION_ID.into(), false,
            )
            .await
            .expect_err("identity mismatch must refuse the connection");
            assert!(error.contains("identity"), "{error}");
            assert!(state.tabs.is_empty());
        });
    }

    /// The consumer's ECDH+HKDF must land on the provider's exact key, and
    /// both SAS fingerprints must agree.
    #[test]
    fn browser_and_agent_derivations_agree() {
        let own_secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
        let own_public = URL_SAFE_NO_PAD.encode(own_secret.public_key().to_encoded_point(false).as_bytes());
        let (agent_cipher, agent_public) = derive_peer_cipher("session-1", "connection-1", &own_public).unwrap();
        let browser_cipher = derive_browser_cipher("session-1", "connection-1", own_secret, &agent_public).unwrap();
        assert_eq!(agent_cipher.key, browser_cipher.key);
        assert_eq!(
            sas_fingerprint(&agent_cipher.key, SAS_LABEL),
            sas_fingerprint(&browser_cipher.key, SAS_LABEL)
        );
        // A frame the provider encrypts decrypts on the consumer side.
        let frame = json!({"kind": "RELAY_STATE", "state": "active"});
        let envelope = agent_cipher.encrypt("session-1", "connection-1", DIRECTION_AGENT, &frame).unwrap();
        let inner = browser_cipher.decrypt("session-1", "connection-1", DIRECTION_AGENT, &envelope).unwrap();
        assert_eq!(inner, frame);
    }

    /// The identity proof binds device, session, connection and both public
    /// keys; any mismatch must fail verification.
    #[test]
    fn provider_identity_proof_round_trip() {
        use ed25519_dalek::SigningKey;
        let seed = [7_u8; 32];
        let signing = SigningKey::from_bytes(&seed);
        let seed_b64 = URL_SAFE_NO_PAD.encode(seed);
        let public = signing.verifying_key().to_bytes();
        let context = e2ee_context("device-1", "session-1", "connection-1", "peer-pub", "agent-pub");
        let proof = ed25519_sign_b64(&seed_b64, &context).unwrap();
        let signature = URL_SAFE_NO_PAD.decode(&proof).unwrap();
        assert!(ed25519_verify(&public, context.as_bytes(), &signature));
        let tampered = e2ee_context("device-2", "session-1", "connection-1", "peer-pub", "agent-pub");
        assert!(!ed25519_verify(&public, tampered.as_bytes(), &signature));
    }
}
