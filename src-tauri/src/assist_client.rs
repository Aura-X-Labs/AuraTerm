//! Remote Assist — guest side inside AuraTerm (design §10).
//!
//! An `assist` tab is a terminal whose bytes come from another AuraTerm's
//! session through the relay: join with the code's route segment, prove
//! the secret segment to the host with SPAKE2, then render the host's
//! snapshot/output and — only while the host has granted control — send
//! keystrokes as E2EE `INPUT` frames. No account is needed on this side;
//! nothing about the session is persisted.

use crate::account::auraxlab_origin;
use crate::assist::{self, PROTOCOL_VERSION};
use crate::e2ee::{PeerCipher, DIRECTION_GUEST, DIRECTION_HOST};
use crate::util::{self, Utf8StreamDecoder};
use crate::{PtyExitEvent, PtyOutputEvent};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio_tungstenite::tungstenite::Message;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_INPUT_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub struct AssistClientState {
    sessions: Arc<Mutex<HashMap<String, GuestSession>>>,
}

type Sessions = Arc<Mutex<HashMap<String, GuestSession>>>;

struct GuestSession {
    assist_id: String,
    connection_id: String,
    cipher: PeerCipher,
    role: String,
    fence: Option<u64>,
    input_seq: u64,
    outbound: tokio::sync::mpsc::Sender<Message>,
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

/// Mirror of the guest's view of the session, emitted as
/// `assist-client-state:<id>` whenever it changes.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestStateEvent {
    pub id: String,
    /// "handshake" | "pending_approval" | "active" | "denied" | "ended"
    pub state: String,
    /// "viewer" | "controller"
    pub role: String,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub host_label: Option<String>,
    pub fingerprint: Option<String>,
    pub control_policy: Option<String>,
    pub reason: Option<String>,
}

fn emit_state<R: tauri::Runtime>(app: &AppHandle<R>, event: GuestStateEvent) {
    let _ = app.emit(&util::session_event("assist-client-state", &event.id), event);
}

fn emit_output<R: tauri::Runtime>(app: &AppHandle<R>, id: &str, decoder: &mut Utf8StreamDecoder, bytes: &[u8]) {
    let text = decoder.push(bytes);
    if !text.is_empty() {
        let _ = app.emit(
            &util::session_event("pty-output", id),
            PtyOutputEvent {
                id: id.to_string(),
                data: text,
            },
        );
    }
}

fn decode_hex(value: &str) -> Vec<u8> {
    if value.len() % 2 != 0 {
        return Vec::new();
    }
    (0..value.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn recv_text(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    >,
) -> Result<serde_json::Value, String> {
    loop {
        let message = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.next())
            .await
            .map_err(|_| "timed out waiting for the host".to_string())?
            .ok_or_else(|| "relay closed the connection".to_string())?
            .map_err(|e| format!("relay error: {e}"))?;
        match message {
            Message::Text(text) => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    return Ok(value);
                }
            }
            Message::Close(_) => return Err("relay closed the connection".into()),
            _ => continue,
        }
    }
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
        return Err("This assist code is invalid, expired, already used, or the host is offline.".into());
    }
    let grant: JoinGrant = response.json().await.map_err(|e| e.to_string())?;
    if !(grant.relay_url.starts_with("ws://") || grant.relay_url.starts_with("wss://")) {
        return Err("The server has no WebSocket relay configured.".into());
    }

    // ── relay admission ─────────────────────────────────────────────────
    let (socket, _) = tokio_tungstenite::connect_async(&grant.relay_url)
        .await
        .map_err(|e| format!("relay connection failed: {e}"))?;
    let (mut sink, mut stream) = socket.split();
    sink.send(Message::Text(json!({"kind": "AUTH", "ticket": grant.ticket}).to_string().into()))
        .await
        .map_err(|e| format!("relay AUTH failed: {e}"))?;
    let auth = recv_text(&mut stream).await?;
    if auth.get("kind").and_then(|v| v.as_str()) != Some("AUTH_OK") {
        return Err("The relay refused the assist ticket.".into());
    }
    let connection_id = auth
        .get("connection_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "relay did not assign a connection id".to_string())?
        .to_string();

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
    sink.send(Message::Text(
        json!({"kind": "PAKE_A", "protocol_version": PROTOCOL_VERSION,
               "pa": URL_SAFE_NO_PAD.encode(share)})
        .to_string()
        .into(),
    ))
    .await
    .map_err(|e| format!("relay send failed: {e}"))?;
    let pake_b = loop {
        let frame = recv_text(&mut stream).await?;
        match frame.get("kind").and_then(|v| v.as_str()) {
            Some("PAKE_B") => break frame,
            Some("PAKE_FAILED") => return Err("The assist code was not accepted by the host.".into()),
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
        let _ = sink.close().await;
        return Err("Assist code mismatch — check the last 8 characters and try again.".into());
    }
    sink.send(Message::Text(
        json!({"kind": "PAKE_CONFIRM", "confirm_a": URL_SAFE_NO_PAD.encode(keys.own_confirmation())})
            .to_string()
            .into(),
    ))
    .await
    .map_err(|e| format!("relay send failed: {e}"))?;
    let key = assist::session_key(&keys, &assist_id, &connection_id);
    let fingerprint = assist::fingerprint(&keys);
    let cipher = PeerCipher::new(*key);

    // ── pumps ───────────────────────────────────────────────────────────
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel::<Message>(256);
    tauri::async_runtime::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    {
        let mut sessions = state.sessions.lock().map_err(|e| e.to_string())?;
        sessions.insert(
            id.clone(),
            GuestSession {
                assist_id: assist_id.clone(),
                connection_id: connection_id.clone(),
                cipher: cipher.clone(),
                role: "viewer".into(),
                fence: None,
                input_seq: 0,
                outbound: outbound_tx.clone(),
            },
        );
    }
    send_inner(
        state,
        &id,
        json!({"kind": "ASSIST_HELLO", "client": "auraterm",
               "display_name": display_name.unwrap_or_default().chars().filter(|c| !c.is_control()).take(32).collect::<String>(),
               "app_version": env!("CARGO_PKG_VERSION")}),
    )
    .await?;
    emit_state(
        app,
        GuestStateEvent {
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
    spawn_reader(app.clone(), Arc::clone(&state.sessions), id.clone(), assist_id.clone(), connection_id.clone(), cipher, fingerprint.clone(), stream);
    Ok(AssistJoinView {
        assist_id,
        connection_id,
        fingerprint,
    })
}

fn spawn_reader<R: tauri::Runtime>(
    app: AppHandle<R>,
    sessions: Sessions,
    id: String,
    assist_id: String,
    connection_id: String,
    cipher: PeerCipher,
    fingerprint: String,
    mut stream: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    >,
) {
    tauri::async_runtime::spawn(async move {
        let mut decoder = Utf8StreamDecoder::new();
        let mut host_label: Option<String> = None;
        let mut cols: Option<u16> = None;
        let mut rows: Option<u16> = None;
        let mut role = "viewer".to_string();
        let mut end_reason = "disconnected".to_string();
        while let Some(message) = stream.next().await {
            let Ok(message) = message else { break };
            let Message::Text(text) = message else {
                if matches!(message, Message::Close(_)) {
                    break;
                }
                continue;
            };
            let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            match frame.get("kind").and_then(|v| v.as_str()) {
                Some("SESSION_END") => {
                    end_reason = frame.get("reason").and_then(|v| v.as_str()).unwrap_or("host_ended").to_string();
                    break;
                }
                Some("E2EE_FRAME") => {}
                _ => continue,
            }
            let Ok(inner) = cipher.decrypt(&assist_id, &connection_id, DIRECTION_HOST, &frame) else {
                continue;
            };
            let kind = inner.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            match kind {
                "ASSIST_STATE" => {
                    if let Some(label) = inner.get("host_label").and_then(|v| v.as_str()) {
                        host_label = Some(label.to_string());
                    }
                    cols = inner.get("cols").and_then(|v| v.as_u64()).map(|v| v as u16).or(cols);
                    rows = inner.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16).or(rows);
                    role = inner.get("role").and_then(|v| v.as_str()).unwrap_or("viewer").to_string();
                    let fence = inner.get("fence").and_then(|v| v.as_u64());
                    if let Ok(mut sessions) = sessions.lock() {
                        if let Some(session) = sessions.get_mut(&id) {
                            session.role = role.clone();
                            session.fence = fence.or(session.fence);
                        }
                    }
                    let status = inner.get("state").and_then(|v| v.as_str()).unwrap_or("active").to_string();
                    if status == "denied" {
                        end_reason = inner.get("reason").and_then(|v| v.as_str()).unwrap_or("denied").to_string();
                        emit_state(
                            &app,
                            GuestStateEvent {
                                id: id.clone(),
                                state: "denied".into(),
                                role: role.clone(),
                                cols,
                                rows,
                                host_label: host_label.clone(),
                                fingerprint: Some(fingerprint.clone()),
                                control_policy: None,
                                reason: Some(end_reason.clone()),
                            },
                        );
                        break;
                    }
                    emit_state(
                        &app,
                        GuestStateEvent {
                            id: id.clone(),
                            state: status,
                            role: role.clone(),
                            cols,
                            rows,
                            host_label: host_label.clone(),
                            fingerprint: Some(fingerprint.clone()),
                            control_policy: inner.get("control_policy").and_then(|v| v.as_str()).map(str::to_string),
                            reason: None,
                        },
                    );
                }
                "TERMINAL_SNAPSHOT" | "OUTPUT" => {
                    if kind == "TERMINAL_SNAPSHOT" {
                        cols = inner.get("cols").and_then(|v| v.as_u64()).map(|v| v as u16).or(cols);
                        rows = inner.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16).or(rows);
                        // Clear + home so the snapshot replaces whatever was shown.
                        emit_output(&app, &id, &mut decoder, b"\x1b[2J\x1b[H");
                        emit_state(
                            &app,
                            GuestStateEvent {
                                id: id.clone(),
                                state: "active".into(),
                                role: role.clone(),
                                cols,
                                rows,
                                host_label: host_label.clone(),
                                fingerprint: Some(fingerprint.clone()),
                                control_policy: None,
                                reason: None,
                            },
                        );
                    }
                    let bytes = decode_hex(inner.get("data_hex").and_then(|v| v.as_str()).unwrap_or(""));
                    emit_output(&app, &id, &mut decoder, &bytes);
                }
                "RESIZE" | "ASSIST_SESSION_SWITCHED" => {
                    cols = inner.get("cols").and_then(|v| v.as_u64()).map(|v| v as u16).or(cols);
                    rows = inner.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16).or(rows);
                    emit_state(
                        &app,
                        GuestStateEvent {
                            id: id.clone(),
                            state: "active".into(),
                            role: role.clone(),
                            cols,
                            rows,
                            host_label: host_label.clone(),
                            fingerprint: Some(fingerprint.clone()),
                            control_policy: None,
                            reason: (kind == "ASSIST_SESSION_SWITCHED").then(|| "switched".to_string()),
                        },
                    );
                }
                "CONTROL_GRANT" | "CONTROL_REVOKE" => {
                    role = if kind == "CONTROL_GRANT" { "controller" } else { "viewer" }.to_string();
                    let fence = inner.get("fence").and_then(|v| v.as_u64());
                    if let Ok(mut sessions) = sessions.lock() {
                        if let Some(session) = sessions.get_mut(&id) {
                            session.role = role.clone();
                            if fence.is_some() {
                                session.fence = fence;
                            }
                        }
                    }
                    emit_state(
                        &app,
                        GuestStateEvent {
                            id: id.clone(),
                            state: "active".into(),
                            role: role.clone(),
                            cols,
                            rows,
                            host_label: host_label.clone(),
                            fingerprint: Some(fingerprint.clone()),
                            control_policy: None,
                            reason: inner.get("reason").and_then(|v| v.as_str()).map(str::to_string),
                        },
                    );
                }
                _ => {}
            }
        }
        // Connection over: drop the session, tell the tab.
        if let Ok(mut sessions) = sessions.lock() {
            sessions.remove(&id);
        }
        emit_state(
            &app,
            GuestStateEvent {
                id: id.clone(),
                state: "ended".into(),
                role,
                cols,
                rows,
                host_label,
                fingerprint: Some(fingerprint),
                control_policy: None,
                reason: Some(end_reason.clone()),
            },
        );
        let _ = app.emit(
            &util::session_event("pty-exit", &id),
            PtyExitEvent {
                id: id.clone(),
                message: format!("Remote assist ended ({end_reason})"),
            },
        );
    });
}

async fn send_inner(state: &AssistClientState, id: &str, frame: serde_json::Value) -> Result<(), String> {
    let (cipher, outbound, assist_id, connection_id) = {
        let sessions = state.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.get(id).ok_or_else(|| "assist session is not connected".to_string())?;
        (
            session.cipher.clone(),
            session.outbound.clone(),
            session.assist_id.clone(),
            session.connection_id.clone(),
        )
    };
    let _guard = cipher.send_lock.lock().await;
    let envelope = cipher.encrypt(&assist_id, &connection_id, DIRECTION_GUEST, &frame)?;
    outbound
        .send(Message::Text(envelope.to_string().into()))
        .await
        .map_err(|_| "relay connection is closed".to_string())
}

/// Keystrokes from the tab; dropped unless the host granted control.
#[tauri::command]
pub async fn write_assist_input(state: State<'_, AssistClientState>, id: String, data: String) -> Result<(), String> {
    write_input(state.inner(), &id, &data).await
}

pub(crate) async fn write_input(state: &AssistClientState, id: &str, data: &str) -> Result<(), String> {
    if data.is_empty() || data.len() > MAX_INPUT_BYTES {
        return Ok(());
    }
    let (fence, seq) = {
        let mut sessions = state.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions.get_mut(id).ok_or_else(|| "assist session is not connected".to_string())?;
        if session.role != "controller" {
            return Ok(());
        }
        let Some(fence) = session.fence else { return Ok(()) };
        session.input_seq += 1;
        (fence, session.input_seq)
    };
    send_inner(
        state,
        id,
        json!({"kind": "INPUT", "fence": fence, "input_seq": seq, "data_hex": encode_hex(data.as_bytes())}),
    )
    .await
}

#[tauri::command]
pub async fn assist_request_control(state: State<'_, AssistClientState>, id: String) -> Result<(), String> {
    send_inner(state.inner(), &id, json!({"kind": "CONTROL_REQUEST"})).await
}

#[tauri::command]
pub async fn assist_release_control(state: State<'_, AssistClientState>, id: String) -> Result<(), String> {
    if let Ok(mut sessions) = state.sessions.lock() {
        if let Some(session) = sessions.get_mut(&id) {
            session.role = "viewer".into();
        }
    }
    send_inner(state.inner(), &id, json!({"kind": "CONTROL_RELEASE"})).await
}

#[tauri::command]
pub fn close_assist_session(state: State<'_, AssistClientState>, id: String) -> Result<(), String> {
    // Dropping the outbound sender closes the writer, which closes the socket;
    // the reader then emits the final state/exit events.
    state.sessions.lock().map_err(|e| e.to_string())?.remove(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;


    // ── end-to-end guest handshake against an in-process fake server ───────

    use crate::e2ee::DIRECTION_HOST;
    use crate::pake::Spake2Keys;
    use std::sync::Arc;
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
            wait_for(|| state.sessions.lock().unwrap().get("tab-guest").map(|s| s.fence.is_some()).unwrap_or(false), "ASSIST_STATE fence").await;
            write_input(&state, "tab-guest", "nope").await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(observed.lock().unwrap().frames_from_guest.len(), 1);
            // Request control → grant → typing reaches the host with the new fence.
            send_inner(&state, "tab-guest", json!({"kind": "CONTROL_REQUEST"})).await.unwrap();
            wait_for(|| state.sessions.lock().unwrap().get("tab-guest").map(|s| s.role == "controller").unwrap_or(false), "CONTROL_GRANT").await;
            write_input(&state, "tab-guest", "ls\n").await.unwrap();
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
            assert!(state.sessions.lock().unwrap().is_empty());
            // The host never got a confirmation (and so never derived keys).
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(observed.lock().unwrap().keys.is_none());
            // Garbage and unknown codes fail before any network I/O.
            assert!(join_session(app.handle(), &state, &origin, "t".into(), "not-a-code".into(), None).await.is_err());
            let unknown = join_session(app.handle(), &state, &origin, "t".into(), "ZZZZ-GHJK-LMNP".into(), None).await.unwrap_err();
            assert!(unknown.contains("invalid"), "{unknown}");
        });
    }

    #[test]
    fn hex_helpers_round_trip() {
        assert_eq!(encode_hex(b"ls\n"), "6c730a");
        assert_eq!(decode_hex("6c730a"), b"ls\n");
        assert!(decode_hex("abc").is_empty());
    }
}
