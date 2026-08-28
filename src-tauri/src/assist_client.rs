//! Remote Assist — guest side inside AuraTerm (design §10).
//!
//! An `assist` tab is a terminal whose bytes come from another AuraTerm's
//! session through the relay: join with the code's route segment, prove
//! the secret segment to the host with SPAKE2, then render the host's
//! snapshot/output and — only while the host has granted control — send
//! keystrokes as E2EE `INPUT` frames. No account is needed on this side;
//! nothing about the session is persisted.
//!
//! Only the assist-specific parts live here: the `/assist/v1/join` call,
//! the SPAKE2 key agreement, and the Tauri command surface. Everything
//! after key agreement — relay admission, the E2EE tab registry, the
//! reader loop rendering into the tab — is the shared `remote_tab` module.

use crate::account::auraxlab_origin;
use crate::assist::{self, PROTOCOL_VERSION};
use crate::e2ee::{PeerCipher, DIRECTION_GUEST, DIRECTION_HOST};
use crate::remote_tab::{self, FrameClass, RemoteTabs, TabProtocol, TabStateEvent};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, State};
use tokio_tungstenite::tungstenite::Message;

/// Assist frame names on the shared remote-tab vocabulary.
fn classify_assist_frame(kind: &str) -> FrameClass {
    match kind {
        "ASSIST_STATE" => FrameClass::PeerState,
        "TERMINAL_SNAPSHOT" => FrameClass::Snapshot,
        "OUTPUT" => FrameClass::Output,
        "RESIZE" => FrameClass::Resize,
        "ASSIST_SESSION_SWITCHED" => FrameClass::SessionSwitched,
        "CONTROL_GRANT" => FrameClass::ControlGrant,
        "CONTROL_REVOKE" => FrameClass::ControlRevoke,
        _ => FrameClass::Ignore,
    }
}

static ASSIST_TAB_PROTOCOL: TabProtocol = TabProtocol {
    state_event: "assist-client-state",
    own_direction: DIRECTION_GUEST,
    peer_direction: DIRECTION_HOST,
    ended_message: "Remote assist ended",
    default_end_reason: "host_ended",
    classify: classify_assist_frame,
};

pub struct AssistClientState {
    pub(crate) tabs: RemoteTabs,
}

impl Default for AssistClientState {
    fn default() -> Self {
        Self {
            tabs: RemoteTabs::new(&ASSIST_TAB_PROTOCOL),
        }
    }
}

#[derive(Deserialize)]
struct JoinGrant {
    assist_id: String,
    ticket: String,
    relay_url: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistJoinView {
    pub assist_id: String,
    pub connection_id: String,
    pub fingerprint: String,
}

/// Join a host's assist session as a new terminal tab `id`.
#[tauri::command]
pub async fn assist_join(
    app: AppHandle,
    state: State<'_, AssistClientState>,
    id: String,
    code: String,
    display_name: Option<String>,
) -> Result<AssistJoinView, String> {
    join_session(&app, state.inner(), &auraxlab_origin(), id, code, display_name).await
}

/// The join flow with an explicit AuraXLab origin (tests point it at an
/// in-process fake server; the command always uses the fixed origin).
pub(crate) async fn join_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AssistClientState,
    origin: &str,
    id: String,
    code: String,
    display_name: Option<String>,
) -> Result<AssistJoinView, String> {
    let parsed = assist::parse_code(&code)?;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{origin}/assist/v1/join"))
        .header("User-Agent", format!("AuraTerm/{}", env!("CARGO_PKG_VERSION")))
        .json(&json!({"route_code": parsed.route, "client": "auraterm",
                      "protocol_version": PROTOCOL_VERSION}))
        .send()
        .await
        .map_err(|e| format!("could not reach AuraXLab: {e}"))?;
    let status = response.status();
    if status.as_u16() == 429 {
        return Err("Too many attempts; please wait a minute and retry.".into());
    }
    if !status.is_success() {
        return Err("This share code is invalid, expired, already used, or the host is offline.".into());
    }
    let grant: JoinGrant = response.json().await.map_err(|e| e.to_string())?;

    // ── relay admission ─────────────────────────────────────────────────
    let admission = remote_tab::connect_relay(&grant.relay_url, &grant.ticket).await?;
    let connection_id = admission.connection_id;
    let outbound = admission.outbound;
    let mut stream = admission.stream;

    // ── SPAKE2 with the host ────────────────────────────────────────────
    let secret = parsed.secret.clone();
    let assist_id = grant.assist_id.clone();
    let w = tokio::task::spawn_blocking({
        let assist_id = assist_id.clone();
        move || assist::derive_w(&secret, &assist_id)
    })
    .await
    .map_err(|e| e.to_string())?;
    let guest = assist::guest_pake(&w, &assist_id, &connection_id)?;
    let share = *guest.share();
    outbound
        .send(Message::Text(
            json!({"kind": "PAKE_A", "protocol_version": PROTOCOL_VERSION,
                   "pa": URL_SAFE_NO_PAD.encode(share)})
            .to_string()
            .into(),
        ))
        .await
        .map_err(|_| "relay send failed".to_string())?;
    let pake_b = loop {
        let frame = remote_tab::recv_text(&mut stream).await?;
        match frame.get("kind").and_then(|v| v.as_str()) {
            Some("PAKE_B") => break frame,
            Some("PAKE_FAILED") => return Err("The share code was not accepted by the host.".into()),
            Some("SESSION_END") => return Err("The host ended the assist session.".into()),
            _ => continue,
        }
    };
    let pb = URL_SAFE_NO_PAD
        .decode(pake_b.get("pb").and_then(|v| v.as_str()).unwrap_or(""))
        .map_err(|_| "malformed PAKE_B".to_string())?;
    let keys = guest.finish(&pb).map_err(|e| e.to_string())?;
    let confirm_b = URL_SAFE_NO_PAD
        .decode(pake_b.get("confirm_b").and_then(|v| v.as_str()).unwrap_or(""))
        .map_err(|_| "malformed PAKE_B".to_string())?;
    if !keys.verify_peer_confirmation(&confirm_b) {
        // Our own check failed: either a typo in the code or a host that
        // does not know it. Drop the connection; the host counts it.
        drop(outbound);
        return Err("Share code mismatch — check the last 8 characters and try again.".into());
    }
    outbound
        .send(Message::Text(
            json!({"kind": "PAKE_CONFIRM", "confirm_a": URL_SAFE_NO_PAD.encode(keys.own_confirmation())})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| "relay send failed".to_string())?;
    let key = assist::session_key(&keys, &assist_id, &connection_id);
    let fingerprint = assist::fingerprint(&keys);
    let cipher = PeerCipher::new(*key);

    // ── hand the keyed connection to the shared remote-tab plumbing ────
    state.tabs.insert(&id, &assist_id, &connection_id, cipher, outbound);
    state
        .tabs
        .send(
            &id,
            json!({"kind": "ASSIST_HELLO", "client": "auraterm",
                   "display_name": display_name.unwrap_or_default().chars().filter(|c| !c.is_control()).take(32).collect::<String>(),
                   "app_version": env!("CARGO_PKG_VERSION")}),
        )
        .await?;
    state.tabs.emit_state(
        app,
        TabStateEvent {
            id: id.clone(),
            state: "handshake".into(),
            role: "viewer".into(),
            cols: None,
            rows: None,
            host_label: None,
            fingerprint: Some(fingerprint.clone()),
            control_policy: None,
            reason: None,
        },
    );
    state.tabs.spawn_reader(app.clone(), id, fingerprint.clone(), stream);
    Ok(AssistJoinView {
        assist_id,
        connection_id,
        fingerprint,
    })
}

/// Keystrokes from the tab; dropped unless the host granted control.
#[tauri::command]
pub async fn write_assist_input(state: State<'_, AssistClientState>, id: String, data: String) -> Result<(), String> {
    state.inner().tabs.write_input(&id, &data).await
}

#[tauri::command]
pub async fn assist_request_control(state: State<'_, AssistClientState>, id: String) -> Result<(), String> {
    state.inner().tabs.send(&id, json!({"kind": "CONTROL_REQUEST"})).await
}

#[tauri::command]
pub async fn assist_release_control(state: State<'_, AssistClientState>, id: String) -> Result<(), String> {
    state.inner().tabs.set_role(&id, "viewer");
    state.inner().tabs.send(&id, json!({"kind": "CONTROL_RELEASE"})).await
}

#[tauri::command]
pub fn close_assist_session(state: State<'_, AssistClientState>, id: String) -> Result<(), String> {
    state.inner().tabs.close(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── end-to-end guest handshake against an in-process fake server ───────

    use crate::e2ee::DIRECTION_HOST;
    use crate::pake::Spake2Keys;
    use crate::remote_tab::encode_hex;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// Minimal AuraXLab: answers /assist/v1/join with a ticket pointing at
    /// the fake relay. Runs on a plain thread (tiny_http is blocking).
    fn fake_auraxlab(relay_url: String, assist_id: String) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(&mut request.as_reader(), &mut body);
                let payload: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let ok = request.url() == "/assist/v1/join"
                    && payload["route_code"] == "BCDF"
                    && payload["client"] == "auraterm"
                    && payload["protocol_version"] == 1;
                let response = if ok {
                    tiny_http::Response::from_string(
                        json!({"assist_id": assist_id, "ticket": "tick-1", "relay_url": relay_url}).to_string(),
                    )
                    .with_status_code(200)
                } else {
                    tiny_http::Response::from_string(json!({"error": "ASSIST_CODE_INVALID"}).to_string()).with_status_code(404)
                };
                let _ = request.respond(response.with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
                ));
            }
        });
        origin
    }

    /// What the fake relay+host observed / produced, for assertions.
    struct HostSide {
        keys: Option<Spake2Keys>,
        frames_from_guest: Vec<serde_json::Value>,
    }

    /// Fake relay + host in one WebSocket server: admits with AUTH_OK, runs
    /// the host side of SPAKE2 with `secret`, then replies to the guest's
    /// E2EE frames like the real host would (ASSIST_STATE, snapshot,
    /// CONTROL_GRANT on request, INPUT_ACK + echo on input).
    async fn fake_relay_host(secret: String, assist_id: String, observed: Arc<Mutex<HostSide>>) -> String {
        use futures_util::{SinkExt, StreamExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tauri::async_runtime::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else { return };
            let (mut sink, mut stream) = ws.split();
            let connection_id = "conn-guest-1".to_string();
            let mut host_keys: Option<Spake2Keys> = None;
            let mut cipher: Option<PeerCipher> = None;
            let w = assist::derive_w(&secret, &assist_id);
            let mut fence = 0_u64;
            while let Some(Ok(message)) = stream.next().await {
                let Message::Text(text) = message else { continue };
                let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
                match frame["kind"].as_str().unwrap_or("") {
                    "AUTH" => {
                        assert_eq!(frame["ticket"], "tick-1");
                        let _ = sink
                            .send(Message::Text(json!({"kind": "AUTH_OK", "connection_id": connection_id, "role": "assist_guest"}).to_string().into()))
                            .await;
                    }
                    "PAKE_A" => {
                        let host = assist::host_pake(&w, &assist_id, &connection_id).unwrap();
                        let pa = URL_SAFE_NO_PAD.decode(frame["pa"].as_str().unwrap()).unwrap();
                        let share = *host.share();
                        let keys = host.finish(&pa).unwrap();
                        let confirm = URL_SAFE_NO_PAD.encode(keys.own_confirmation());
                        let _ = sink
                            .send(Message::Text(
                                json!({"kind": "PAKE_B", "connection_id": connection_id, "pb": URL_SAFE_NO_PAD.encode(share), "confirm_b": confirm}).to_string().into(),
                            ))
                            .await;
                        host_keys = Some(keys);
                    }
                    "PAKE_CONFIRM" => {
                        let keys = host_keys.take().unwrap();
                        let confirm_a = URL_SAFE_NO_PAD.decode(frame["confirm_a"].as_str().unwrap()).unwrap();
                        assert!(keys.verify_peer_confirmation(&confirm_a), "guest confirmation must verify");
                        let key = assist::session_key(&keys, &assist_id, &connection_id);
                        let peer = PeerCipher::new(*key);
                        let state = json!({"kind": "ASSIST_STATE", "state": "active", "role": "viewer",
                            "cols": 100, "rows": 30, "host_label": "Fake Host", "fingerprint": assist::fingerprint(&keys),
                            "control_policy": "on_request", "fence": fence});
                        let envelope = peer.encrypt(&assist_id, &connection_id, DIRECTION_HOST, &state).unwrap();
                        let _ = sink.send(Message::Text(envelope.to_string().into())).await;
                        let snapshot = json!({"kind": "TERMINAL_SNAPSHOT", "snapshot_seq": 1, "cols": 100, "rows": 30,
                            "data_hex": encode_hex(b"host$ ")});
                        let envelope = peer.encrypt(&assist_id, &connection_id, DIRECTION_HOST, &snapshot).unwrap();
                        let _ = sink.send(Message::Text(envelope.to_string().into())).await;
                        observed.lock().unwrap().keys = Some(keys);
                        cipher = Some(peer);
                    }
                    "E2EE_FRAME" => {
                        let Some(peer) = cipher.as_ref() else { continue };
                        let inner = peer.decrypt(&assist_id, &connection_id, DIRECTION_GUEST, &frame).unwrap();
                        observed.lock().unwrap().frames_from_guest.push(inner.clone());
                        match inner["kind"].as_str().unwrap_or("") {
                            "CONTROL_REQUEST" => {
                                fence += 1;
                                let grant = json!({"kind": "CONTROL_GRANT", "fence": fence, "expires_at": 0});
                                let envelope = peer.encrypt(&assist_id, &connection_id, DIRECTION_HOST, &grant).unwrap();
                                let _ = sink.send(Message::Text(envelope.to_string().into())).await;
                            }
                            "INPUT" => {
                                assert_eq!(inner["fence"].as_u64(), Some(fence));
                                let ack = json!({"kind": "INPUT_ACK", "input_seq": inner["input_seq"], "fence": fence});
                                let envelope = peer.encrypt(&assist_id, &connection_id, DIRECTION_HOST, &ack).unwrap();
                                let _ = sink.send(Message::Text(envelope.to_string().into())).await;
                                let echo = json!({"kind": "OUTPUT", "output_seq": 2, "data_hex": inner["data_hex"]});
                                let envelope = peer.encrypt(&assist_id, &connection_id, DIRECTION_HOST, &echo).unwrap();
                                let _ = sink.send(Message::Text(envelope.to_string().into())).await;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        });
        url
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
    fn guest_joins_handshakes_requests_control_and_types() {
        tauri::async_runtime::block_on(async {
            let assist_id = "assist-client-test".to_string();
            let secret = "GHJKLMNP".to_string();
            let observed = Arc::new(Mutex::new(HostSide { keys: None, frames_from_guest: Vec::new() }));
            let relay_url = fake_relay_host(secret.clone(), assist_id.clone(), Arc::clone(&observed)).await;
            let origin = fake_auraxlab(relay_url, assist_id.clone());
            let app = tauri::test::mock_app();
            let handle = app.handle().clone();
            let state = AssistClientState::default();

            let joined = join_session(&handle, &state, &origin, "tab-guest".into(), "bcdf-ghjk-lmnp".into(), Some("Ada".into()))
                .await
                .expect("join succeeds with the right code");
            assert_eq!(joined.assist_id, assist_id);
            assert_eq!(joined.connection_id, "conn-guest-1");
            // Both ends derived the same fingerprint.
            wait_for(|| observed.lock().unwrap().keys.is_some(), "host keys").await;
            assert_eq!(assist::fingerprint(observed.lock().unwrap().keys.as_ref().unwrap()), joined.fingerprint);
            // HELLO arrived encrypted with the guest's name.
            wait_for(|| !observed.lock().unwrap().frames_from_guest.is_empty(), "ASSIST_HELLO").await;
            {
                let host = observed.lock().unwrap();
                assert_eq!(host.frames_from_guest[0]["kind"], "ASSIST_HELLO");
                assert_eq!(host.frames_from_guest[0]["display_name"], "Ada");
                assert_eq!(host.frames_from_guest[0]["client"], "auraterm");
            }
            // Viewer: input is dropped locally, never sent.
            wait_for(|| state.tabs.fence("tab-guest").is_some(), "ASSIST_STATE fence").await;
            state.tabs.write_input("tab-guest", "nope").await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(observed.lock().unwrap().frames_from_guest.len(), 1);
            // Request control → grant → typing reaches the host with the new fence.
            state.tabs.send("tab-guest", json!({"kind": "CONTROL_REQUEST"})).await.unwrap();
            wait_for(|| state.tabs.role("tab-guest").as_deref() == Some("controller"), "CONTROL_GRANT").await;
            state.tabs.write_input("tab-guest", "ls\n").await.unwrap();
            wait_for(|| observed.lock().unwrap().frames_from_guest.iter().any(|f| f["kind"] == "INPUT"), "INPUT").await;
            let host = observed.lock().unwrap();
            let input = host.frames_from_guest.iter().find(|f| f["kind"] == "INPUT").unwrap();
            assert_eq!(input["data_hex"], encode_hex(b"ls\n"));
            assert_eq!(input["fence"], 1);
            assert_eq!(input["input_seq"], 1);
        });
    }

    #[test]
    fn wrong_code_is_detected_before_confirming() {
        tauri::async_runtime::block_on(async {
            let assist_id = "assist-client-bad".to_string();
            let observed = Arc::new(Mutex::new(HostSide { keys: None, frames_from_guest: Vec::new() }));
            let relay_url = fake_relay_host("GHJKLMNP".into(), assist_id.clone(), Arc::clone(&observed)).await;
            let origin = fake_auraxlab(relay_url, assist_id);
            let app = tauri::test::mock_app();
            let state = AssistClientState::default();
            let error = join_session(app.handle(), &state, &origin, "tab-bad".into(), "BCDF-GHJK-LMNQ".into(), None)
                .await
                .expect_err("a wrong secret must not join");
            assert!(error.contains("mismatch"), "{error}");
            assert!(state.tabs.is_empty());
            // The host never got a confirmation (and so never derived keys).
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(observed.lock().unwrap().keys.is_none());
            // Garbage and unknown codes fail before any network I/O.
            assert!(join_session(app.handle(), &state, &origin, "t".into(), "not-a-code".into(), None).await.is_err());
            let unknown = join_session(app.handle(), &state, &origin, "t".into(), "ZZZZ-GHJK-LMNP".into(), None).await.unwrap_err();
            assert!(unknown.contains("invalid"), "{unknown}");
        });
    }
}
