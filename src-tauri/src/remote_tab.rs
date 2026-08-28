//! Remote tab plumbing — the protocol-independent half of "a terminal tab
//! whose bytes come from another machine through the relay".
//!
//! Extracted from the Live Share guest (`assist_client.rs`) so the Live
//! Relay consumer (design `docs/plans/live-sync-design.md` §5.11) can reuse
//! it. What lives here is everything *after* admission and key agreement:
//!
//! * the relay WebSocket admission (`AUTH` with a one-time ticket) and the
//!   outbound pump,
//! * the per-tab E2EE session registry ([`RemoteTabs`]) with role/fence
//!   gated input,
//! * the reader loop that turns decrypted peer frames into `pty-output` /
//!   `pty-exit` events and per-tab state events.
//!
//! What deliberately stays outside: how the ticket is obtained and how the
//! E2EE key is agreed. Live Share proves a shared code with SPAKE2; Live
//! Relay will do ECDH plus a locally verified device-identity signature.
//! Both end with a [`PeerCipher`] and hand it to [`RemoteTabs::insert`].
//!
//! The peer-frame vocabulary differs per protocol only in frame *names*
//! (`ASSIST_STATE` vs. a future relay state frame); the field contract is
//! shared. A protocol supplies a [`TabProtocol`] whose `classify` maps its
//! frame kinds onto [`FrameClass`]; all handling logic lives here. The
//! peer-state contract: `state`, `role`, `fence`, `cols`, `rows`,
//! `host_label`, `control_policy`, `reason` — all optional except `state`.

use crate::e2ee::PeerCipher;
use crate::util::{self, Utf8StreamDecoder};
use crate::{PtyExitEvent, PtyOutputEvent};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio_tungstenite::tungstenite::Message;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on one INPUT frame's payload; larger writes are dropped.
pub const MAX_INPUT_BYTES: usize = 16 * 1024;

pub type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// How the shared reader loop should treat one decrypted peer frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameClass {
    /// Peer's authoritative view of the tab (role, dims, label, fence…).
    PeerState,
    /// Full-screen replacement: clear the tab, then render the payload.
    Snapshot,
    /// Incremental terminal output.
    Output,
    /// The remote terminal was resized.
    Resize,
    /// The peer switched which of its sessions backs this tab.
    SessionSwitched,
    /// We were granted control (payload carries the new fence).
    ControlGrant,
    /// Our control was revoked.
    ControlRevoke,
    /// Not part of the shared vocabulary; ignore.
    Ignore,
}

/// The protocol-specific constants a feature plugs into the shared plumbing.
pub struct TabProtocol {
    /// Event name prefix for per-tab state events, e.g. "assist-client-state".
    pub state_event: &'static str,
    /// AAD direction label on frames we send (e.g. `DIRECTION_GUEST`).
    pub own_direction: &'static str,
    /// AAD direction label on the peer's frames (e.g. `DIRECTION_HOST`).
    pub peer_direction: &'static str,
    /// Human prefix for the final `pty-exit` message, e.g. "Remote assist ended".
    pub ended_message: &'static str,
    /// Reason reported when the transport ends without naming one.
    pub default_end_reason: &'static str,
    /// Maps the protocol's decrypted frame kinds onto [`FrameClass`].
    pub classify: fn(&str) -> FrameClass,
}

/// Mirror of the local view of one remote tab, emitted as
/// `<state_event>:<id>` whenever it changes. Field names are part of the
/// frontend contract — `TerminalComponent.vue` listens for these.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabStateEvent {
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

struct RemoteTab {
    /// AAD session scope (Live Share: the assist id; Live Relay: the cloud
    /// session id).
    scope_id: String,
    connection_id: String,
    cipher: PeerCipher,
    /// "viewer" | "controller"
    role: String,
    /// Latest write-authority fence from the peer; input is dropped until set.
    fence: Option<u64>,
    input_seq: u64,
    outbound: tokio::sync::mpsc::Sender<Message>,
}

type Tabs = Arc<Mutex<HashMap<String, RemoteTab>>>;

/// Registry of live remote tabs for one feature (one per Tauri state).
pub struct RemoteTabs {
    protocol: &'static TabProtocol,
    tabs: Tabs,
}

/// A relay connection admitted with a one-time ticket. The outbound pump is
/// already running: sends go through `outbound`, and dropping every clone of
/// it closes the socket.
pub struct RelayAdmission {
    pub connection_id: String,
    pub outbound: tokio::sync::mpsc::Sender<Message>,
    pub stream: WsStream,
}

/// Connect to `relay_url` and authenticate with `ticket` (first frame must
/// be AUTH; see `relay_server.py`). Spawns the outbound writer task.
pub async fn connect_relay(relay_url: &str, ticket: &str) -> Result<RelayAdmission, String> {
    if !(relay_url.starts_with("ws://") || relay_url.starts_with("wss://")) {
        return Err("The server has no WebSocket relay configured.".into());
    }
    let (socket, _) = tokio_tungstenite::connect_async(relay_url)
        .await
        .map_err(|e| format!("relay connection failed: {e}"))?;
    let (mut sink, mut stream) = socket.split();
    sink.send(Message::Text(json!({"kind": "AUTH", "ticket": ticket}).to_string().into()))
        .await
        .map_err(|e| format!("relay AUTH failed: {e}"))?;
    let auth = recv_text(&mut stream).await?;
    if auth.get("kind").and_then(|v| v.as_str()) != Some("AUTH_OK") {
        return Err("The relay refused the ticket.".into());
    }
    let connection_id = auth
        .get("connection_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "relay did not assign a connection id".to_string())?
        .to_string();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel::<Message>(256);
    tauri::async_runtime::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    Ok(RelayAdmission { connection_id, outbound: outbound_tx, stream })
}

/// Wait for the next JSON text frame, bounded by [`HANDSHAKE_TIMEOUT`].
pub async fn recv_text(stream: &mut WsStream) -> Result<serde_json::Value, String> {
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

pub fn decode_hex(value: &str) -> Vec<u8> {
    if value.len() % 2 != 0 {
        return Vec::new();
    }
    (0..value.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

impl RemoteTabs {
    pub fn new(protocol: &'static TabProtocol) -> Self {
        Self {
            protocol,
            tabs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a freshly keyed connection as tab `id` (role starts as
    /// "viewer"; input stays dropped until the peer announces a fence).
    pub fn insert(
        &self,
        id: &str,
        scope_id: &str,
        connection_id: &str,
        cipher: PeerCipher,
        outbound: tokio::sync::mpsc::Sender<Message>,
    ) {
        if let Ok(mut tabs) = self.tabs.lock() {
            tabs.insert(
                id.to_string(),
                RemoteTab {
                    scope_id: scope_id.to_string(),
                    connection_id: connection_id.to_string(),
                    cipher,
                    role: "viewer".into(),
                    fence: None,
                    input_seq: 0,
                    outbound,
                },
            );
        }
    }

    /// Drop the tab. Dropping the outbound sender closes the writer, which
    /// closes the socket; the reader then emits the final state/exit events.
    pub fn close(&self, id: &str) -> Result<(), String> {
        self.tabs.lock().map_err(|e| e.to_string())?.remove(id);
        Ok(())
    }

    /// Locally downgrade the tab's role (e.g. releasing control before the
    /// peer confirms), so input stops being sent immediately.
    pub fn set_role(&self, id: &str, role: &str) {
        if let Ok(mut tabs) = self.tabs.lock() {
            if let Some(tab) = tabs.get_mut(id) {
                tab.role = role.to_string();
            }
        }
    }

    /// Introspection used by tests today and by Live Relay's status
    /// surface next (design §5.12); not wired to a command yet.
    #[allow(dead_code)]
    pub fn contains(&self, id: &str) -> bool {
        self.tabs.lock().map(|tabs| tabs.contains_key(id)).unwrap_or(false)
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tabs.lock().map(|tabs| tabs.is_empty()).unwrap_or(true)
    }

    #[allow(dead_code)]
    pub fn role(&self, id: &str) -> Option<String> {
        self.tabs.lock().ok()?.get(id).map(|tab| tab.role.clone())
    }

    #[allow(dead_code)]
    pub fn fence(&self, id: &str) -> Option<u64> {
        self.tabs.lock().ok()?.get(id).and_then(|tab| tab.fence)
    }

    /// Encrypt `frame` for the tab's peer and queue it on the outbound pump.
    pub async fn send(&self, id: &str, frame: serde_json::Value) -> Result<(), String> {
        let (cipher, outbound, scope_id, connection_id) = {
            let tabs = self.tabs.lock().map_err(|e| e.to_string())?;
            let tab = tabs.get(id).ok_or_else(|| "remote session is not connected".to_string())?;
            (
                tab.cipher.clone(),
                tab.outbound.clone(),
                tab.scope_id.clone(),
                tab.connection_id.clone(),
            )
        };
        let _guard = cipher.send_lock.lock().await;
        let envelope = cipher.encrypt(&scope_id, &connection_id, self.protocol.own_direction, &frame)?;
        outbound
            .send(Message::Text(envelope.to_string().into()))
            .await
            .map_err(|_| "relay connection is closed".to_string())
    }

    /// Keystrokes from the tab; silently dropped unless the peer granted
    /// control and announced a fence. Every accepted write carries a
    /// strictly increasing `input_seq` so the device side can de-duplicate.
    pub async fn write_input(&self, id: &str, data: &str) -> Result<(), String> {
        if data.is_empty() || data.len() > MAX_INPUT_BYTES {
            return Ok(());
        }
        let (fence, seq) = {
            let mut tabs = self.tabs.lock().map_err(|e| e.to_string())?;
            let tab = tabs.get_mut(id).ok_or_else(|| "remote session is not connected".to_string())?;
            if tab.role != "controller" {
                return Ok(());
            }
            let Some(fence) = tab.fence else { return Ok(()) };
            tab.input_seq += 1;
            (fence, tab.input_seq)
        };
        self.send(
            id,
            json!({"kind": "INPUT", "fence": fence, "input_seq": seq, "data_hex": encode_hex(data.as_bytes())}),
        )
        .await
    }

    pub fn emit_state<R: tauri::Runtime>(&self, app: &AppHandle<R>, event: TabStateEvent) {
        let _ = app.emit(&util::session_event(self.protocol.state_event, &event.id), event);
    }

    /// Pump the tab's inbound stream until it ends: decrypt peer frames,
    /// render terminal bytes, mirror role/fence changes into the registry,
    /// and finish with an "ended" state event plus `pty-exit`.
    pub fn spawn_reader<R: tauri::Runtime>(&self, app: AppHandle<R>, id: String, fingerprint: String, stream: WsStream) {
        let (scope_id, connection_id, cipher) = {
            let Ok(tabs) = self.tabs.lock() else { return };
            let Some(tab) = tabs.get(&id) else { return };
            (tab.scope_id.clone(), tab.connection_id.clone(), tab.cipher.clone())
        };
        spawn_reader_task(
            app,
            self.protocol,
            Arc::clone(&self.tabs),
            id,
            scope_id,
            connection_id,
            cipher,
            fingerprint,
            stream,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_reader_task<R: tauri::Runtime>(
    app: AppHandle<R>,
    protocol: &'static TabProtocol,
    tabs: Tabs,
    id: String,
    scope_id: String,
    connection_id: String,
    cipher: PeerCipher,
    fingerprint: String,
    mut stream: WsStream,
) {
    let state_event = protocol.state_event;
    tauri::async_runtime::spawn(async move {
        let mut decoder = Utf8StreamDecoder::new();
        let mut host_label: Option<String> = None;
        let mut cols: Option<u16> = None;
        let mut rows: Option<u16> = None;
        let mut role = "viewer".to_string();
        let mut end_reason = "disconnected".to_string();
        let emit_state = |event: TabStateEvent| {
            let _ = app.emit(&util::session_event(state_event, &event.id), event);
        };
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
                    end_reason = frame
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or(protocol.default_end_reason)
                        .to_string();
                    break;
                }
                Some("E2EE_FRAME") => {}
                _ => continue,
            }
            let Ok(inner) = cipher.decrypt(&scope_id, &connection_id, protocol.peer_direction, &frame) else {
                continue;
            };
            let kind = inner.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let class = (protocol.classify)(kind);
            match class {
                FrameClass::PeerState => {
                    if let Some(label) = inner.get("host_label").and_then(|v| v.as_str()) {
                        host_label = Some(label.to_string());
                    }
                    cols = inner.get("cols").and_then(|v| v.as_u64()).map(|v| v as u16).or(cols);
                    rows = inner.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16).or(rows);
                    role = inner.get("role").and_then(|v| v.as_str()).unwrap_or("viewer").to_string();
                    let fence = inner.get("fence").and_then(|v| v.as_u64());
                    if let Ok(mut tabs) = tabs.lock() {
                        if let Some(tab) = tabs.get_mut(&id) {
                            tab.role = role.clone();
                            tab.fence = fence.or(tab.fence);
                        }
                    }
                    let status = inner.get("state").and_then(|v| v.as_str()).unwrap_or("active").to_string();
                    if status == "denied" {
                        end_reason = inner.get("reason").and_then(|v| v.as_str()).unwrap_or("denied").to_string();
                        emit_state(TabStateEvent {
                            id: id.clone(),
                            state: "denied".into(),
                            role: role.clone(),
                            cols,
                            rows,
                            host_label: host_label.clone(),
                            fingerprint: Some(fingerprint.clone()),
                            control_policy: None,
                            reason: Some(end_reason.clone()),
                        });
                        break;
                    }
                    emit_state(TabStateEvent {
                        id: id.clone(),
                        state: status,
                        role: role.clone(),
                        cols,
                        rows,
                        host_label: host_label.clone(),
                        fingerprint: Some(fingerprint.clone()),
                        control_policy: inner.get("control_policy").and_then(|v| v.as_str()).map(str::to_string),
                        reason: None,
                    });
                }
                FrameClass::Snapshot | FrameClass::Output => {
                    if class == FrameClass::Snapshot {
                        cols = inner.get("cols").and_then(|v| v.as_u64()).map(|v| v as u16).or(cols);
                        rows = inner.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16).or(rows);
                        // Clear + home so the snapshot replaces whatever was shown.
                        emit_output(&app, &id, &mut decoder, b"\x1b[2J\x1b[H");
                        emit_state(TabStateEvent {
                            id: id.clone(),
                            state: "active".into(),
                            role: role.clone(),
                            cols,
                            rows,
                            host_label: host_label.clone(),
                            fingerprint: Some(fingerprint.clone()),
                            control_policy: None,
                            reason: None,
                        });
                    }
                    let bytes = decode_hex(inner.get("data_hex").and_then(|v| v.as_str()).unwrap_or(""));
                    emit_output(&app, &id, &mut decoder, &bytes);
                }
                FrameClass::Resize | FrameClass::SessionSwitched => {
                    cols = inner.get("cols").and_then(|v| v.as_u64()).map(|v| v as u16).or(cols);
                    rows = inner.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16).or(rows);
                    emit_state(TabStateEvent {
                        id: id.clone(),
                        state: "active".into(),
                        role: role.clone(),
                        cols,
                        rows,
                        host_label: host_label.clone(),
                        fingerprint: Some(fingerprint.clone()),
                        control_policy: None,
                        reason: (class == FrameClass::SessionSwitched).then(|| "switched".to_string()),
                    });
                }
                FrameClass::ControlGrant | FrameClass::ControlRevoke => {
                    role = if class == FrameClass::ControlGrant { "controller" } else { "viewer" }.to_string();
                    let fence = inner.get("fence").and_then(|v| v.as_u64());
                    if let Ok(mut tabs) = tabs.lock() {
                        if let Some(tab) = tabs.get_mut(&id) {
                            tab.role = role.clone();
                            if fence.is_some() {
                                tab.fence = fence;
                            }
                        }
                    }
                    emit_state(TabStateEvent {
                        id: id.clone(),
                        state: "active".into(),
                        role: role.clone(),
                        cols,
                        rows,
                        host_label: host_label.clone(),
                        fingerprint: Some(fingerprint.clone()),
                        control_policy: None,
                        reason: inner.get("reason").and_then(|v| v.as_str()).map(str::to_string),
                    });
                }
                FrameClass::Ignore => {}
            }
        }
        // Connection over: drop the tab, tell the frontend.
        if let Ok(mut tabs) = tabs.lock() {
            tabs.remove(&id);
        }
        emit_state(TabStateEvent {
            id: id.clone(),
            state: "ended".into(),
            role,
            cols,
            rows,
            host_label,
            fingerprint: Some(fingerprint),
            control_policy: None,
            reason: Some(end_reason.clone()),
        });
        let _ = app.emit(
            &util::session_event("pty-exit", &id),
            PtyExitEvent {
                id: id.clone(),
                message: format!("{} ({end_reason})", protocol.ended_message),
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_helpers_round_trip() {
        assert_eq!(encode_hex(b"ls\n"), "6c730a");
        assert_eq!(decode_hex("6c730a"), b"ls\n");
        assert!(decode_hex("abc").is_empty());
    }
}
