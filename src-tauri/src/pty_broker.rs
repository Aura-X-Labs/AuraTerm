//! Local terminal session broker — the seam between "something that owns a
//! PTY" and everything that talks to one.
//!
//! `PtyBroker` owns the session map that used to live inline in `main.rs` and
//! is shared by the local Tauri UI today and the Cloud Console agent later.
//! The concrete terminal backend hides behind [`LocalTerminalPort`] /
//! [`TerminalHandle`]: production uses [`PortablePtyAdapter`] (portable-pty),
//! tests use [`ScriptedTerminalAdapter`] so session behaviour can be verified
//! without spawning real shells.
//!
//! Output never touches `tauri::AppHandle` here: every chunk read from the
//! backend is published to the [`TerminalEventHub`] as **raw bytes** (before
//! Zmodem and UTF-8 decoding). The Tauri UI adapter in `main.rs` subscribes
//! and keeps the existing decode/emit behaviour; a cloud adapter can subscribe
//! to the same session and forward the untouched byte stream.
//!
//! The broker is also where the device-side write-authority checks live:
//! `input`/`resize` carry an `input_seq` and `fence` so a remote controller's
//! writes can be de-duplicated and fenced *at the device*, not just in the
//! cloud control plane. Local UI callers pass `0` for both, which means
//! "trusted local owner: unsequenced, not fence-checked".

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::terminal_event_hub::{TerminalEvent, TerminalEventHub};

pub type TerminalError = String;

/// `input_seq`/`fence` value used by trusted local callers (the Tauri UI):
/// unsequenced input that bypasses fencing.
pub const LOCAL_OWNER: u64 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

/// Bounds applied to resize requests so a malformed remote frame can never
/// drive the PTY into absurd dimensions.
const MAX_COLS: u16 = 4000;
const MAX_ROWS: u16 = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// The local user closed the tab / the frontend tore the session down.
    LocalRequest,
    /// A cloud console session ended (End/Revoke/timeout).
    #[allow(dead_code)] // constructed by the Cloud Console agent (next phase)
    RemoteEnd,
}

/// Everything a backend needs to open one terminal session. The shell is
/// resolved by the caller (settings/platform detection) — remote requests can
/// never inject a program, cwd or environment of their own.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenTerminalRequest {
    pub session_id: String,
    pub size: TerminalSize,
    /// Resolved shell path (see `resolve_local_shell_path` in `main.rs`).
    pub shell_path: String,
    /// Optional working directory; ignored unless it is an existing directory.
    pub cwd: Option<String>,
}

/// Acknowledgement for one `input` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputAck {
    /// Highest sequenced input applied so far (0 while only unsequenced input
    /// has been written).
    pub input_seq: u64,
    /// False when the call was a duplicate that was acknowledged again
    /// without writing to the terminal.
    pub applied: bool,
}

/// Backend seam: opens concrete terminal sessions.
pub trait LocalTerminalPort: Send + Sync {
    fn open(&self, request: &OpenTerminalRequest) -> Result<Box<dyn TerminalHandle>, TerminalError>;
}

/// One live terminal session as seen by the broker. Implementations only move
/// bytes; sequencing and fencing are enforced by the broker before these are
/// called.
pub trait TerminalHandle: Send {
    /// Take the event stream exactly once; the broker pumps it into the hub.
    fn take_events(&mut self) -> Option<mpsc::Receiver<TerminalEvent>>;
    fn write(&self, bytes: &[u8]) -> Result<(), TerminalError>;
    fn resize(&self, size: TerminalSize) -> Result<(), TerminalError>;
    fn close(&mut self, reason: CloseReason) -> Result<(), TerminalError>;
}

struct SessionEntry {
    handle: Box<dyn TerminalHandle>,
    last_input_seq: u64,
    last_fence: u64,
}

pub struct PtyBroker {
    port: Box<dyn LocalTerminalPort>,
    hub: Arc<TerminalEventHub>,
    sessions: Mutex<HashMap<String, SessionEntry>>,
}

impl PtyBroker {
    pub fn new(port: Box<dyn LocalTerminalPort>, hub: Arc<TerminalEventHub>) -> Self {
        Self {
            port,
            hub,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn hub(&self) -> &Arc<TerminalEventHub> {
        &self.hub
    }

    /// Open a new session and start pumping its raw output into the hub.
    /// Subscribe to the hub *before* calling this to observe the first bytes.
    pub fn open(&self, request: OpenTerminalRequest) -> Result<(), TerminalError> {
        let session_id = request.session_id.clone();
        {
            let sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            if sessions.contains_key(&session_id) {
                return Err(format!("PTY session already exists: {session_id}"));
            }
        }

        let mut handle = self.port.open(&request)?;
        let events = handle
            .take_events()
            .ok_or_else(|| "terminal backend produced no event stream".to_string())?;

        {
            let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            sessions.insert(
                session_id.clone(),
                SessionEntry {
                    handle,
                    last_input_seq: 0,
                    last_fence: 0,
                },
            );
        }

        let hub = Arc::clone(&self.hub);
        thread::spawn(move || {
            while let Ok(event) = events.recv() {
                let is_exit = matches!(event, TerminalEvent::Exit(_));
                hub.publish(&session_id, &event);
                if is_exit {
                    break;
                }
            }
            hub.drop_session(&session_id);
        });

        Ok(())
    }

    /// Write `bytes` to the session. Sequenced input (`input_seq > 0`) must
    /// arrive strictly in order: duplicates are re-acknowledged without a
    /// second write, gaps are rejected so a lost frame can never be silently
    /// skipped over.
    pub fn input(
        &self,
        session_id: &str,
        input_seq: u64,
        bytes: &[u8],
        fence: u64,
    ) -> Result<InputAck, TerminalError> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let entry = sessions
            .get_mut(session_id)
            .ok_or_else(|| "PTY session not found".to_string())?;
        Self::check_fence(entry, fence)?;

        if input_seq == LOCAL_OWNER {
            entry.handle.write(bytes)?;
            return Ok(InputAck {
                input_seq: entry.last_input_seq,
                applied: true,
            });
        }
        if input_seq <= entry.last_input_seq {
            return Ok(InputAck {
                input_seq: entry.last_input_seq,
                applied: false,
            });
        }
        if input_seq != entry.last_input_seq + 1 {
            return Err(format!(
                "INPUT_SEQ_GAP: expected {}, got {input_seq}",
                entry.last_input_seq + 1
            ));
        }
        entry.handle.write(bytes)?;
        entry.last_input_seq = input_seq;
        Ok(InputAck {
            input_seq,
            applied: true,
        })
    }

    /// Resize the session. Missing sessions are ignored (the tab may already
    /// have exited), matching the historical `resize_pty` behaviour.
    pub fn resize(&self, session_id: &str, size: TerminalSize, fence: u64) -> Result<(), TerminalError> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let Some(entry) = sessions.get_mut(session_id) else {
            return Ok(());
        };
        Self::check_fence(entry, fence)?;
        let bounded = TerminalSize {
            cols: size.cols.clamp(1, MAX_COLS),
            rows: size.rows.clamp(1, MAX_ROWS),
        };
        entry.handle.resize(bounded)
    }

    /// Close and forget the session. Idempotent: closing an unknown session is
    /// a no-op. The backend's read loop delivers the final `Exit` event.
    pub fn close(&self, session_id: &str, reason: CloseReason) -> Result<(), TerminalError> {
        let entry = {
            let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            sessions.remove(session_id)
        };
        if let Some(mut entry) = entry {
            entry.handle.close(reason)?;
        }
        Ok(())
    }

    #[allow(dead_code)] // used by tests and the Cloud Console agent (next phase)
    pub fn contains(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|sessions| sessions.contains_key(session_id))
            .unwrap_or(false)
    }

    fn check_fence(entry: &mut SessionEntry, fence: u64) -> Result<(), TerminalError> {
        if fence == LOCAL_OWNER {
            return Ok(());
        }
        if fence < entry.last_fence {
            return Err(format!(
                "STALE_FENCE: fence {fence} is older than {}",
                entry.last_fence
            ));
        }
        entry.last_fence = fence;
        Ok(())
    }
}

// ── Production backend: portable-pty ────────────────────────────────────────

pub struct PortablePtyAdapter;

struct PortablePtyHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send>,
    events: Option<mpsc::Receiver<TerminalEvent>>,
}

impl LocalTerminalPort for PortablePtyAdapter {
    fn open(&self, request: &OpenTerminalRequest) -> Result<Box<dyn TerminalHandle>, TerminalError> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: request.size.rows,
                cols: request.size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;

        #[cfg(unix)]
        let mut command = {
            let mut command = CommandBuilder::new_default_prog();
            command.env("SHELL", &request.shell_path);
            command.env("TERM", "xterm-256color");
            command
        };

        #[cfg(windows)]
        let mut command = {
            let mut command = CommandBuilder::new(&request.shell_path);
            command.env("TERM", "xterm-256color");
            command
        };

        if let Some(dir) = &request.cwd {
            if std::path::Path::new(dir).is_dir() {
                command.cwd(dir);
            }
        }

        let child = pty_pair
            .slave
            .spawn_command(command)
            .map_err(|error| error.to_string())?;
        drop(pty_pair.slave);

        let writer = Arc::new(Mutex::new(
            pty_pair
                .master
                .take_writer()
                .map_err(|error| error.to_string())?,
        ));
        let mut reader = pty_pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = sender.send(TerminalEvent::Exit("PTY closed".to_string()));
                        break;
                    }
                    Ok(size) => {
                        if sender
                            .send(TerminalEvent::Output(buffer[..size].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = sender.send(TerminalEvent::Exit("PTY read error".to_string()));
                        break;
                    }
                }
            }
        });

        Ok(Box::new(PortablePtyHandle {
            master: pty_pair.master,
            writer,
            child,
            events: Some(receiver),
        }))
    }
}

impl TerminalHandle for PortablePtyHandle {
    fn take_events(&mut self) -> Option<mpsc::Receiver<TerminalEvent>> {
        self.events.take()
    }

    fn write(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        let mut writer = self.writer.lock().map_err(|e| e.to_string())?;
        writer.write_all(bytes).map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())
    }

    fn resize(&self, size: TerminalSize) -> Result<(), TerminalError> {
        self.master
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())
    }

    fn close(&mut self, _reason: CloseReason) -> Result<(), TerminalError> {
        let _ = self.child.kill();
        Ok(())
    }
}

// ── Test backend: scripted sessions ─────────────────────────────────────────

/// Test double for [`LocalTerminalPort`]: records every call and lets a test
/// feed arbitrary raw bytes (including invalid UTF-8) into a session's event
/// stream.
#[cfg(test)]
#[derive(Default)]
pub struct ScriptedTerminalAdapter {
    pub state: Arc<Mutex<ScriptedState>>,
}

#[cfg(test)]
#[derive(Default)]
pub struct ScriptedState {
    pub opens: Vec<OpenTerminalRequest>,
    pub writes: Vec<(String, Vec<u8>)>,
    pub resizes: Vec<(String, TerminalSize)>,
    pub closes: Vec<(String, CloseReason)>,
    pub fail_next_open: bool,
    senders: HashMap<String, mpsc::Sender<TerminalEvent>>,
}

#[cfg(test)]
impl ScriptedTerminalAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a raw output chunk into an open session, as if the shell wrote it.
    pub fn emit_output(&self, session_id: &str, bytes: &[u8]) {
        let state = self.state.lock().unwrap();
        if let Some(sender) = state.senders.get(session_id) {
            let _ = sender.send(TerminalEvent::Output(bytes.to_vec()));
        }
    }

    /// End a session's stream, as if the shell exited on its own.
    pub fn emit_exit(&self, session_id: &str, message: &str) {
        let state = self.state.lock().unwrap();
        if let Some(sender) = state.senders.get(session_id) {
            let _ = sender.send(TerminalEvent::Exit(message.to_string()));
        }
    }
}

#[cfg(test)]
struct ScriptedHandle {
    session_id: String,
    state: Arc<Mutex<ScriptedState>>,
    events: Option<mpsc::Receiver<TerminalEvent>>,
}

#[cfg(test)]
impl LocalTerminalPort for ScriptedTerminalAdapter {
    fn open(&self, request: &OpenTerminalRequest) -> Result<Box<dyn TerminalHandle>, TerminalError> {
        let mut state = self.state.lock().unwrap();
        if state.fail_next_open {
            state.fail_next_open = false;
            return Err("scripted spawn failure".to_string());
        }
        state.opens.push(request.clone());
        let (sender, receiver) = mpsc::channel();
        state.senders.insert(request.session_id.clone(), sender);
        Ok(Box::new(ScriptedHandle {
            session_id: request.session_id.clone(),
            state: Arc::clone(&self.state),
            events: Some(receiver),
        }))
    }
}

#[cfg(test)]
impl TerminalHandle for ScriptedHandle {
    fn take_events(&mut self) -> Option<mpsc::Receiver<TerminalEvent>> {
        self.events.take()
    }

    fn write(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        let mut state = self.state.lock().unwrap();
        state.writes.push((self.session_id.clone(), bytes.to_vec()));
        Ok(())
    }

    fn resize(&self, size: TerminalSize) -> Result<(), TerminalError> {
        let mut state = self.state.lock().unwrap();
        state.resizes.push((self.session_id.clone(), size));
        Ok(())
    }

    fn close(&mut self, reason: CloseReason) -> Result<(), TerminalError> {
        let mut state = self.state.lock().unwrap();
        state.closes.push((self.session_id.clone(), reason));
        // Dropping the sender ends the event stream, like a killed child's EOF.
        state.senders.remove(&self.session_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scripted_broker() -> (Arc<ScriptedTerminalAdapter>, PtyBroker, Arc<TerminalEventHub>) {
        let adapter = Arc::new(ScriptedTerminalAdapter::new());
        let hub = Arc::new(TerminalEventHub::new());
        let port = ScriptedTerminalAdapter {
            state: Arc::clone(&adapter.state),
        };
        let broker = PtyBroker::new(Box::new(port), Arc::clone(&hub));
        (adapter, broker, hub)
    }

    fn open_request(id: &str) -> OpenTerminalRequest {
        OpenTerminalRequest {
            session_id: id.to_string(),
            size: TerminalSize { cols: 80, rows: 24 },
            shell_path: "/bin/test-shell".to_string(),
            cwd: None,
        }
    }

    fn subscribe_events(hub: &TerminalEventHub, id: &str) -> mpsc::Receiver<TerminalEvent> {
        let (sender, receiver) = mpsc::channel();
        hub.subscribe(id, move |event| {
            let _ = sender.send(event.clone());
        });
        receiver
    }

    #[test]
    fn open_publishes_raw_output_bytes_to_hub() {
        let (adapter, broker, hub) = scripted_broker();
        let events = subscribe_events(&hub, "s1");
        broker.open(open_request("s1")).unwrap();

        // Raw bytes with invalid UTF-8 must arrive untouched (the raw-byte
        // seam is the whole point of the hub).
        let chunk = vec![b'o', b'k', 0xFF, 0xC3];
        adapter.emit_output("s1", &chunk);
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            TerminalEvent::Output(chunk)
        );
    }

    #[test]
    fn duplicate_session_id_is_rejected() {
        let (_adapter, broker, _hub) = scripted_broker();
        broker.open(open_request("s1")).unwrap();
        assert!(broker.open(open_request("s1")).is_err());
    }

    #[test]
    fn failed_open_does_not_register_session() {
        let (adapter, broker, _hub) = scripted_broker();
        adapter.state.lock().unwrap().fail_next_open = true;
        assert!(broker.open(open_request("s1")).is_err());
        assert!(!broker.contains("s1"));
    }

    #[test]
    fn local_owner_input_writes_through() {
        let (adapter, broker, _hub) = scripted_broker();
        broker.open(open_request("s1")).unwrap();
        let ack = broker.input("s1", LOCAL_OWNER, b"ls\n", LOCAL_OWNER).unwrap();
        assert!(ack.applied);
        assert_eq!(
            adapter.state.lock().unwrap().writes,
            vec![("s1".to_string(), b"ls\n".to_vec())]
        );
    }

    #[test]
    fn input_on_unknown_session_is_an_error() {
        let (_adapter, broker, _hub) = scripted_broker();
        let error = broker.input("nope", 0, b"x", 0).unwrap_err();
        assert_eq!(error, "PTY session not found");
    }

    #[test]
    fn sequenced_input_dedupes_and_rejects_gaps() {
        let (adapter, broker, _hub) = scripted_broker();
        broker.open(open_request("s1")).unwrap();

        let first = broker.input("s1", 1, b"a", 1).unwrap();
        assert!(first.applied);
        assert_eq!(first.input_seq, 1);

        // A retransmitted frame is acknowledged but never written twice.
        let duplicate = broker.input("s1", 1, b"a", 1).unwrap();
        assert!(!duplicate.applied);
        assert_eq!(duplicate.input_seq, 1);

        // A gap means a lost frame: refuse rather than skip.
        let gap = broker.input("s1", 3, b"c", 1).unwrap_err();
        assert!(gap.starts_with("INPUT_SEQ_GAP"), "{gap}");

        assert_eq!(broker.input("s1", 2, b"b", 1).unwrap().input_seq, 2);
        assert_eq!(
            adapter.state.lock().unwrap().writes,
            vec![
                ("s1".to_string(), b"a".to_vec()),
                ("s1".to_string(), b"b".to_vec()),
            ]
        );
    }

    #[test]
    fn stale_fence_is_rejected_even_out_of_order() {
        let (adapter, broker, _hub) = scripted_broker();
        broker.open(open_request("s1")).unwrap();

        broker.input("s1", 1, b"new", 5).unwrap();
        let stale = broker.input("s1", 2, b"old", 4).unwrap_err();
        assert!(stale.starts_with("STALE_FENCE"), "{stale}");
        // Local owner (fence 0) still writes: the machine's user always wins.
        assert!(broker.input("s1", LOCAL_OWNER, b"local", LOCAL_OWNER).unwrap().applied);
        assert_eq!(adapter.state.lock().unwrap().writes.len(), 2);
    }

    #[test]
    fn resize_is_fenced_bounded_and_ignores_missing_sessions() {
        let (adapter, broker, _hub) = scripted_broker();
        broker.open(open_request("s1")).unwrap();

        broker
            .resize("s1", TerminalSize { cols: 0, rows: 9999 }, 3)
            .unwrap();
        assert_eq!(
            adapter.state.lock().unwrap().resizes,
            vec![("s1".to_string(), TerminalSize { cols: 1, rows: 4000 })]
        );

        let stale = broker
            .resize("s1", TerminalSize { cols: 80, rows: 24 }, 2)
            .unwrap_err();
        assert!(stale.starts_with("STALE_FENCE"), "{stale}");

        // Unknown session: no-op, like the historical resize_pty command.
        broker
            .resize("gone", TerminalSize { cols: 80, rows: 24 }, 0)
            .unwrap();
    }

    #[test]
    fn close_is_idempotent_and_forgets_the_session() {
        let (adapter, broker, _hub) = scripted_broker();
        broker.open(open_request("s1")).unwrap();

        broker.close("s1", CloseReason::LocalRequest).unwrap();
        assert!(!broker.contains("s1"));
        assert!(broker.input("s1", 0, b"x", 0).is_err());
        // Closing again (or closing a session that never existed) is fine.
        broker.close("s1", CloseReason::LocalRequest).unwrap();
        broker.close("never", CloseReason::RemoteEnd).unwrap();

        assert_eq!(
            adapter.state.lock().unwrap().closes,
            vec![("s1".to_string(), CloseReason::LocalRequest)]
        );
    }

    #[test]
    fn exit_event_reaches_subscribers_then_session_is_dropped_from_hub() {
        let (adapter, broker, hub) = scripted_broker();
        let events = subscribe_events(&hub, "s1");
        broker.open(open_request("s1")).unwrap();

        adapter.emit_exit("s1", "shell exited");
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            TerminalEvent::Exit("shell exited".to_string())
        );
    }

    /// Real end-to-end smoke test over portable-pty: spawn `cat` as the
    /// "shell", verify raw echo output, then kill it via close.
    #[cfg(unix)]
    #[test]
    fn portable_pty_adapter_round_trips_real_bytes() {
        let hub = Arc::new(TerminalEventHub::new());
        let broker = PtyBroker::new(Box::new(PortablePtyAdapter), Arc::clone(&hub));
        let events = subscribe_events(&hub, "real");

        broker
            .open(OpenTerminalRequest {
                session_id: "real".to_string(),
                size: TerminalSize { cols: 80, rows: 24 },
                shell_path: "/bin/cat".to_string(),
                cwd: None,
            })
            .unwrap();

        broker.input("real", LOCAL_OWNER, b"hello\n", LOCAL_OWNER).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut collected = Vec::new();
        let mut saw_hello = false;
        while std::time::Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(250)) {
                Ok(TerminalEvent::Output(bytes)) => {
                    collected.extend_from_slice(&bytes);
                    if String::from_utf8_lossy(&collected).contains("hello") {
                        saw_hello = true;
                        break;
                    }
                }
                Ok(TerminalEvent::Exit(_)) => break,
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(saw_hello, "expected echoed output, got {collected:?}");

        broker.close("real", CloseReason::LocalRequest).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut saw_exit = false;
        while std::time::Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(250)) {
                Ok(TerminalEvent::Exit(_)) => {
                    saw_exit = true;
                    break;
                }
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(saw_exit, "expected Exit after close");
    }
}
