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
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio_tungstenite::tungstenite::Message;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on one INPUT frame's payload (the hosts' `MAX_TX_BYTES`);
/// larger writes are split across several frames.
pub const MAX_INPUT_BYTES: usize = 16 * 1024;
/// Peer frames in a row that may fail to decrypt before the tab gives up
/// with a visible error. A lone bad frame (a relay hiccup, a stale replay)
/// is skipped; a run of them means the channel is unusable, and a tab that
/// silently stops updating is worse than one that says so.
pub const MAX_CONSECUTIVE_DECRYPT_FAILURES: u32 = 8;
/// End reason reported when [`MAX_CONSECUTIVE_DECRYPT_FAILURES`] is hit.
pub const DECRYPT_FAILED_REASON: &str = "decryption_failed";
/// End reason reported when the user closed the tab locally.
pub const CLOSED_REASON: &str = "closed";
/// How many closed-tab marks to remember. Marks are normally dropped when
/// the ending is finished; the cap only bounds tabs closed after their
/// connection was already gone, which nothing will try to revive.
const MAX_CLOSED_MARKS: usize = 256;

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
    /// The peer ended this tab from inside the E2EE channel (kick, share
    /// stopped): finish the loop with the frame's `reason`.
    SessionEnd,
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
type Closed = Arc<Mutex<VecDeque<String>>>;

/// Registry of live remote tabs for one feature (one per Tauri state).
pub struct RemoteTabs {
    protocol: &'static TabProtocol,
    tabs: Tabs,
    /// Tabs the user closed. Kept until the ending is finished so that
    /// neither a reader that is still unwinding nor a reconnect that is
    /// still in flight can mistake the close for a transport drop and
    /// bring the tab back as an invisible peer.
    closed: Closed,
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
    sink.send(Message::Text(json!({"kind": "AUTH", "ticket": ticket}).to_string()))
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

/// Decode a peer-supplied hex string. Works on raw bytes, so any non-hex
/// input — including multi-byte UTF-8 — is an error rather than a panic
/// from slicing inside a character.
pub fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    fn nibble(byte: u8) -> Result<u8, String> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err("invalid hex digit".into()),
        }
    }
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[high, low]| Ok((nibble(*high)? << 4) | nibble(*low)?))
        .collect()
}

/// Split `data` into pieces of at most `max` bytes without cutting a UTF-8
/// character in half (`max` must be at least 4).
fn utf8_chunks(data: &str, max: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let mut end = rest.len().min(max);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (head, tail) = rest.split_at(end);
        chunks.push(head);
        rest = tail;
    }
    chunks
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
            closed: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Register a freshly keyed connection as tab `id` (role starts as
    /// "viewer"; input stays dropped until the peer announces a fence).
    /// Refused when the user already closed that tab; `outbound` is then
    /// dropped, which closes the new socket.
    pub fn insert(
        &self,
        id: &str,
        scope_id: &str,
        connection_id: &str,
        cipher: PeerCipher,
        outbound: tokio::sync::mpsc::Sender<Message>,
    ) -> Result<(), String> {
        let mut tabs = self.tabs.lock().map_err(|e| e.to_string())?;
        // Checked while holding the registry lock: `close` marks the tab
        // before it takes this lock, so a close can never slip in between.
        if self.is_closed(id) {
            return Err("The remote tab was closed.".into());
        }
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
        Ok(())
    }

    /// The user closed the tab: mark it closed for good, then drop it.
    /// Dropping the outbound sender closes the writer, which closes the
    /// socket; the reader then reports a deliberate ending.
    pub fn close(&self, id: &str) -> Result<(), String> {
        {
            let mut closed = self.closed.lock().map_err(|e| e.to_string())?;
            if !closed.iter().any(|closed_id| closed_id == id) {
                if closed.len() >= MAX_CLOSED_MARKS {
                    closed.pop_front();
                }
                closed.push_back(id.to_string());
            }
        }
        self.tabs.lock().map_err(|e| e.to_string())?.remove(id);
        Ok(())
    }

    /// True once the user closed tab `id`; a reconnect must not revive it.
    pub fn is_closed(&self, id: &str) -> bool {
        is_marked_closed(&self.closed, id)
    }

    /// Drop the closed mark once nothing can try to revive the tab any more
    /// (an attach that failed before a reader or reconnect loop existed).
    pub fn forget_closed(&self, id: &str) {
        unmark_closed(&self.closed, id);
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
            .send(Message::Text(envelope.to_string()))
            .await
            .map_err(|_| "relay connection is closed".to_string())
    }

    /// Keystrokes from the tab; silently dropped unless the peer granted
    /// control and announced a fence. Every accepted write carries a
    /// strictly increasing `input_seq` so the device side can de-duplicate.
    /// A write larger than [`MAX_INPUT_BYTES`] (a big paste) goes out as
    /// several INPUT frames, each split on a character boundary.
    pub async fn write_input(&self, id: &str, data: &str) -> Result<(), String> {
        if data.is_empty() {
            return Ok(());
        }
        let chunks = utf8_chunks(data, MAX_INPUT_BYTES);
        // Reserve the whole run of sequence numbers at once so a concurrent
        // write cannot interleave its numbers with this paste's.
        let (fence, first_seq) = {
            let mut tabs = self.tabs.lock().map_err(|e| e.to_string())?;
            let tab = tabs.get_mut(id).ok_or_else(|| "remote session is not connected".to_string())?;
            if tab.role != "controller" {
                return Ok(());
            }
            let Some(fence) = tab.fence else { return Ok(()) };
            let first = tab.input_seq + 1;
            tab.input_seq += chunks.len() as u64;
            (fence, first)
        };
        for (seq, chunk) in (first_seq..).zip(chunks) {
            self.send(
                id,
                json!({"kind": "INPUT", "fence": fence, "input_seq": seq, "data_hex": encode_hex(chunk.as_bytes())}),
            )
            .await?;
        }
        Ok(())
    }

    pub fn emit_state<R: tauri::Runtime>(&self, app: &AppHandle<R>, event: TabStateEvent) {
        let _ = app.emit(&util::session_event(self.protocol.state_event, &event.id), event);
    }

    /// Pump the tab's inbound stream until it ends: decrypt peer frames,
    /// render terminal bytes, mirror role/fence changes into the registry,
    /// and finish with an "ended" state event plus `pty-exit`.
    pub fn spawn_reader<R: tauri::Runtime>(&self, app: AppHandle<R>, id: String, fingerprint: String, stream: WsStream) {
        self.spawn_reader_with_end(app, id, fingerprint, stream, None);
    }

    /// Pump the tab's stream, handing the outcome to `on_end` instead of
    /// closing the tab. A caller that supplies the channel owns the ending:
    /// it may reconnect quietly, or call [`RemoteTabs::finish`] to close
    /// the tab for real. Without it the reader closes the tab itself.
    pub fn spawn_reader_with_end<R: tauri::Runtime>(
        &self,
        app: AppHandle<R>,
        id: String,
        fingerprint: String,
        stream: WsStream,
        on_end: Option<tokio::sync::oneshot::Sender<TabEnd>>,
    ) {
        let (scope_id, connection_id, cipher) = {
            let Ok(tabs) = self.tabs.lock() else { return };
            let Some(tab) = tabs.get(&id) else { return };
            (tab.scope_id.clone(), tab.connection_id.clone(), tab.cipher.clone())
        };
        spawn_reader_task(
            app,
            self.protocol,
            Arc::clone(&self.tabs),
            Arc::clone(&self.closed),
            id,
            scope_id,
            connection_id,
            cipher,
            fingerprint,
            stream,
            on_end,
        );
    }

    /// Close the tab for good: drop it from the registry and emit the
    /// final state + `pty-exit`. Only needed by callers that took the
    /// ending into their own hands via [`RemoteTabs::spawn_reader_with_end`].
    pub fn finish<R: tauri::Runtime>(&self, app: &AppHandle<R>, id: &str, end: &TabEnd) {
        if let Ok(mut tabs) = self.tabs.lock() {
            tabs.remove(id);
        }
        self.forget_closed(id);
        emit_tab_end(app, self.protocol, id, end);
    }
}

fn is_marked_closed(closed: &Closed, id: &str) -> bool {
    closed.lock().map(|closed| closed.iter().any(|closed_id| closed_id == id)).unwrap_or(false)
}

fn unmark_closed(closed: &Closed, id: &str) {
    if let Ok(mut closed) = closed.lock() {
        closed.retain(|closed_id| closed_id != id);
    }
}

/// How a reader loop finished.
#[derive(Clone, Debug)]
pub struct TabEnd {
    pub reason: String,
    /// True when the peer ended the tab on purpose (kicked, denied, share
    /// stopped) rather than the transport dropping under it. Only the
    /// latter is worth reconnecting after.
    pub deliberate: bool,
    /// Last known view, so a caller that gives up can still emit a
    /// faithful final state instead of a blank one.
    pub role: String,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub host_label: Option<String>,
    pub fingerprint: String,
}

fn emit_tab_end<R: tauri::Runtime>(app: &AppHandle<R>, protocol: &'static TabProtocol, id: &str, end: &TabEnd) {
    let _ = app.emit(
        &util::session_event(protocol.state_event, id),
        TabStateEvent {
            id: id.to_string(),
            state: "ended".into(),
            role: end.role.clone(),
            cols: end.cols,
            rows: end.rows,
            host_label: end.host_label.clone(),
            fingerprint: Some(end.fingerprint.clone()),
            control_policy: None,
            reason: Some(end.reason.clone()),
        },
    );
    let _ = app.emit(
        &util::session_event("pty-exit", id),
        PtyExitEvent {
            id: id.to_string(),
            message: format!("{} ({})", protocol.ended_message, end.reason),
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_reader_task<R: tauri::Runtime>(
    app: AppHandle<R>,
    protocol: &'static TabProtocol,
    tabs: Tabs,
    closed: Closed,
    id: String,
    scope_id: String,
    connection_id: String,
    cipher: PeerCipher,
    fingerprint: String,
    mut stream: WsStream,
    on_end: Option<tokio::sync::oneshot::Sender<TabEnd>>,
) {
    let state_event = protocol.state_event;
    tauri::async_runtime::spawn(async move {
        let mut decoder = Utf8StreamDecoder::new();
        let mut host_label: Option<String> = None;
        let mut cols: Option<u16> = None;
        let mut rows: Option<u16> = None;
        let mut role = "viewer".to_string();
        let mut end_reason = "disconnected".to_string();
        // Set when the peer ends the tab on purpose; a transport drop
        // leaves it false, which is the only case worth reconnecting after.
        let mut deliberate = false;
        let mut decrypt_failures = 0_u32;
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
                    deliberate = true;
                    break;
                }
                Some("E2EE_FRAME") => {}
                _ => continue,
            }
            let inner = match cipher.decrypt(&scope_id, &connection_id, protocol.peer_direction, &frame) {
                Ok(inner) => {
                    decrypt_failures = 0;
                    inner
                }
                Err(error) => {
                    decrypt_failures += 1;
                    crate::warn_log!("[remote-tab] {id}: dropped undecryptable peer frame ({decrypt_failures}): {error}");
                    if decrypt_failures >= MAX_CONSECUTIVE_DECRYPT_FAILURES {
                        // Not deliberate: a fresh admission brings a fresh
                        // key, so a caller that reconnects may recover.
                        end_reason = DECRYPT_FAILED_REASON.to_string();
                        break;
                    }
                    continue;
                }
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
                        deliberate = true;
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
                    // A malformed payload is dropped, as before, but no
                    // longer able to take the reader down with a panic.
                    let bytes = decode_hex(inner.get("data_hex").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or_default();
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
                FrameClass::SessionEnd => {
                    end_reason = inner
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or(protocol.default_end_reason)
                        .to_string();
                    deliberate = true;
                    break;
                }
                FrameClass::Ignore => {}
            }
        }
        // Connection over. The tab always leaves the registry — its
        // connection is gone either way — but who announces the ending
        // depends on whether a caller claimed it.
        if let Ok(mut tabs) = tabs.lock() {
            tabs.remove(&id);
        }
        // A local close drops the socket under us and looks exactly like a
        // transport failure from here; the closed mark says otherwise, so
        // a reconnecting caller does not bring the tab back.
        if is_marked_closed(&closed, &id) {
            end_reason = CLOSED_REASON.to_string();
            deliberate = true;
        }
        let end = TabEnd {
            reason: end_reason,
            deliberate,
            role,
            cols,
            rows,
            host_label,
            fingerprint,
        };
        match on_end {
            Some(sender) => {
                // The caller decides: reconnect quietly, or call `finish`.
                let _ = sender.send(end);
            }
            None => {
                // No one else can revive this tab: the mark has done its job.
                unmark_closed(&closed, &id);
                emit_state(TabStateEvent {
                    id: id.clone(),
                    state: "ended".into(),
                    role: end.role.clone(),
                    cols: end.cols,
                    rows: end.rows,
                    host_label: end.host_label.clone(),
                    fingerprint: Some(end.fingerprint.clone()),
                    control_policy: None,
                    reason: Some(end.reason.clone()),
                });
                let _ = app.emit(
                    &util::session_event("pty-exit", &id),
                    PtyExitEvent {
                        id: id.clone(),
                        message: format!("{} ({})", protocol.ended_message, end.reason),
                    },
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_helpers_round_trip() {
        assert_eq!(encode_hex(b"ls\n"), "6c730a");
        assert_eq!(decode_hex("6c730a").unwrap(), b"ls\n");
        assert_eq!(decode_hex("6C730A").unwrap(), b"ls\n");
        assert!(decode_hex("").unwrap().is_empty());
        assert!(decode_hex("abc").is_err());
        assert!(decode_hex("zz").is_err());
    }

    #[test]
    fn hex_decoding_rejects_multibyte_input_without_panicking() {
        // Even byte length, but the slice boundary falls inside "é".
        assert!(decode_hex("aé0").is_err());
        assert!(decode_hex("é").is_err());
        assert!(decode_hex("41日本").is_err());
        assert!(decode_hex("+1").is_err());
    }

    // ── reader loop against an in-process fake relay ──────────────────────

    use crate::e2ee::{DIRECTION_GUEST, DIRECTION_HOST};
    use tokio::net::TcpListener;

    static TEST_PROTOCOL: TabProtocol = TabProtocol {
        state_event: "test-remote-state",
        own_direction: DIRECTION_GUEST,
        peer_direction: DIRECTION_HOST,
        ended_message: "Test ended",
        default_end_reason: "host_ended",
        classify: |kind| match kind {
            "STATE" => FrameClass::PeerState,
            "OUTPUT" => FrameClass::Output,
            "END" => FrameClass::SessionEnd,
            _ => FrameClass::Ignore,
        },
    };

    const SCOPE: &str = "scope-1";
    const CONNECTION: &str = "conn-1";

    /// Relay that admits one connection, sends `frames`, then forwards
    /// whatever the tab sends to `received` until the tab hangs up.
    async fn fake_relay(
        frames: Vec<serde_json::Value>,
        received: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tauri::async_runtime::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else { return };
            let (mut sink, mut stream) = ws.split();
            let Some(Ok(Message::Text(_auth))) = stream.next().await else { return };
            let _ = sink
                .send(Message::Text(json!({"kind": "AUTH_OK", "connection_id": CONNECTION}).to_string()))
                .await;
            for frame in frames {
                let _ = sink.send(Message::Text(frame.to_string())).await;
            }
            while let Some(Ok(message)) = stream.next().await {
                if let Message::Text(text) = message {
                    if let Ok(value) = serde_json::from_str(&text) {
                        let _ = received.send(value);
                    }
                }
            }
        });
        url
    }

    /// Admit tab `id` through `url` and return how its reader ended.
    async fn attach(
        tabs: &RemoteTabs,
        app: &tauri::App<tauri::test::MockRuntime>,
        url: &str,
        key: [u8; 32],
    ) -> tokio::sync::oneshot::Receiver<TabEnd> {
        let admission = connect_relay(url, "ticket").await.unwrap();
        tabs.insert("tab", SCOPE, &admission.connection_id, PeerCipher::new(key), admission.outbound).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tabs.spawn_reader_with_end(app.handle().clone(), "tab".into(), "FP".into(), admission.stream, Some(tx));
        rx
    }

    async fn ended(rx: tokio::sync::oneshot::Receiver<TabEnd>) -> TabEnd {
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("reader ends within 5s")
            .expect("reader reports its ending")
    }

    #[test]
    fn reader_survives_a_lost_frame() {
        tauri::async_runtime::block_on(async {
            let key = [1_u8; 32];
            let host = PeerCipher::new(key);
            let mut frames = Vec::new();
            frames.push(host.encrypt(SCOPE, CONNECTION, DIRECTION_HOST, &json!({"kind": "OUTPUT", "data_hex": "41"})).unwrap());
            // The host's send of this frame failed after its counter moved.
            let _lost = host.encrypt(SCOPE, CONNECTION, DIRECTION_HOST, &json!({"kind": "OUTPUT", "data_hex": "42"})).unwrap();
            frames.push(host.encrypt(SCOPE, CONNECTION, DIRECTION_HOST, &json!({"kind": "END", "reason": "after_gap"})).unwrap());
            let (received, _rx) = tokio::sync::mpsc::unbounded_channel();
            let url = fake_relay(frames, received).await;
            let app = tauri::test::mock_app();
            let tabs = RemoteTabs::new(&TEST_PROTOCOL);
            let end = ended(attach(&tabs, &app, &url, key).await).await;
            assert_eq!(end.reason, "after_gap");
            assert!(end.deliberate);
        });
    }

    #[test]
    fn large_paste_is_split_into_bounded_input_frames() {
        tauri::async_runtime::block_on(async {
            let key = [5_u8; 32];
            let host = PeerCipher::new(key);
            let grant = host
                .encrypt(SCOPE, CONNECTION, DIRECTION_HOST, &json!({"kind": "STATE", "state": "active", "role": "controller", "fence": 7}))
                .unwrap();
            let (received, mut inbox) = tokio::sync::mpsc::unbounded_channel();
            let url = fake_relay(vec![grant], received).await;
            let app = tauri::test::mock_app();
            let tabs = RemoteTabs::new(&TEST_PROTOCOL);
            let _end = attach(&tabs, &app, &url, key).await;
            for _ in 0..100 {
                if tabs.fence("tab").is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(tabs.fence("tab"), Some(7));

            // ~40 KiB with multi-byte characters straddling every 16 KiB
            // boundary candidate.
            let paste: String = "echo 日本語 ✓ 🚀\n".repeat(1500);
            assert!(paste.len() > 2 * MAX_INPUT_BYTES);
            tabs.write_input("tab", &paste).await.unwrap();

            let mut rebuilt = Vec::new();
            let mut last_seq = 0;
            while rebuilt.len() < paste.len() {
                let envelope = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
                    .await
                    .expect("INPUT frame within 5s")
                    .expect("relay open");
                let frame = host.decrypt(SCOPE, CONNECTION, DIRECTION_GUEST, &envelope).unwrap();
                assert_eq!(frame["kind"], "INPUT");
                assert_eq!(frame["fence"], 7);
                let seq = frame["input_seq"].as_u64().unwrap();
                assert!(seq > last_seq);
                last_seq = seq;
                let bytes = decode_hex(frame["data_hex"].as_str().unwrap()).unwrap();
                assert!(!bytes.is_empty() && bytes.len() <= MAX_INPUT_BYTES, "chunk of {} bytes", bytes.len());
                // Every chunk is valid UTF-8 on its own: no split characters.
                assert!(std::str::from_utf8(&bytes).is_ok());
                rebuilt.extend_from_slice(&bytes);
            }
            assert_eq!(rebuilt, paste.as_bytes());
            assert!(last_seq >= 3);
        });
    }

    #[test]
    fn closing_a_tab_is_a_deliberate_ending_and_blocks_revival() {
        tauri::async_runtime::block_on(async {
            let (received, _rx) = tokio::sync::mpsc::unbounded_channel();
            // The relay keeps the connection open: only our close ends it.
            let url = fake_relay(Vec::new(), received).await;
            let app = tauri::test::mock_app();
            let tabs = RemoteTabs::new(&TEST_PROTOCOL);
            let end = attach(&tabs, &app, &url, [1_u8; 32]).await;
            tabs.close("tab").unwrap();
            let end = ended(end).await;
            assert!(end.deliberate, "a local close must not look like a transport drop");
            assert_eq!(end.reason, CLOSED_REASON);
            // A reconnect that lands after the close is refused...
            let (outbound, _keep) = tokio::sync::mpsc::channel(1);
            assert!(tabs.insert("tab", SCOPE, CONNECTION, PeerCipher::new([1_u8; 32]), outbound).is_err());
            assert!(!tabs.contains("tab"));
            // ...until the ending is finished, which forgets the mark.
            tabs.finish(app.handle(), "tab", &end);
            assert!(!tabs.is_closed("tab"));
        });
    }

    #[test]
    fn closed_marks_are_bounded() {
        let tabs = RemoteTabs::new(&TEST_PROTOCOL);
        for index in 0..(MAX_CLOSED_MARKS + 10) {
            tabs.close(&format!("tab-{index}")).unwrap();
        }
        assert!(!tabs.is_closed("tab-0"));
        assert!(tabs.is_closed(&format!("tab-{}", MAX_CLOSED_MARKS + 9)));
        assert_eq!(tabs.closed.lock().unwrap().len(), MAX_CLOSED_MARKS);
    }

    #[test]
    fn utf8_chunks_respect_char_boundaries() {
        assert!(utf8_chunks("", 4).is_empty());
        assert_eq!(utf8_chunks("abcdef", 4), vec!["abcd", "ef"]);
        // "é" is two bytes; a 3-byte limit must not cut it in half.
        assert_eq!(utf8_chunks("aéé", 3), vec!["aé", "é"]);
        assert_eq!(utf8_chunks("🚀🚀", 5), vec!["🚀", "🚀"]);
    }

    #[test]
    fn persistent_decryption_failure_ends_the_tab_visibly() {
        tauri::async_runtime::block_on(async {
            // Every frame is sealed with a key the tab does not hold.
            let wrong = PeerCipher::new([2_u8; 32]);
            let frames = (0..MAX_CONSECUTIVE_DECRYPT_FAILURES)
                .map(|_| wrong.encrypt(SCOPE, CONNECTION, DIRECTION_HOST, &json!({"kind": "OUTPUT", "data_hex": "41"})).unwrap())
                .collect();
            let (received, _rx) = tokio::sync::mpsc::unbounded_channel();
            let url = fake_relay(frames, received).await;
            let app = tauri::test::mock_app();
            let tabs = RemoteTabs::new(&TEST_PROTOCOL);
            let end = ended(attach(&tabs, &app, &url, [1_u8; 32]).await).await;
            assert_eq!(end.reason, DECRYPT_FAILED_REASON);
            // A transport-level failure: a reconnecting caller may retry.
            assert!(!end.deliberate);
            assert!(!tabs.contains("tab"));
        });
    }
}
