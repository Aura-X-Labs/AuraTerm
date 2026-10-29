use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::task::AbortHandle;

const INTERACTIVE_WRITE_TIMEOUT_SECS: u64 = 10;

struct TelnetSession {
    /// Raw bytes to write to the socket: user keystrokes plus IAC negotiation
    /// responses produced by the reader. Carries arbitrary bytes (not just
    /// UTF-8) because negotiation replies contain `0xFF` option bytes.
    writer: mpsc::UnboundedSender<Vec<u8>>,
    reader_abort: AbortHandle,
    writer_abort: AbortHandle,
    iac: Arc<Mutex<crate::util::TelnetIacFilter>>,
}

impl TelnetSession {
    fn stop(&self) {
        self.reader_abort.abort();
        self.writer_abort.abort();
    }
}

#[derive(Clone)]
pub struct TelnetState {
    sessions: Arc<Mutex<HashMap<String, TelnetSession>>>,
    hub: Arc<crate::terminal_event_hub::TerminalEventHub>,
    starts: crate::util::PendingStarts,
}

impl TelnetState {
    pub fn new(hub: Arc<crate::terminal_event_hub::TerminalEventHub>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            hub,
            starts: crate::util::PendingStarts::default(),
        }
    }

    pub async fn contains(&self, id: &str) -> bool {
        self.sessions.lock().await.contains_key(id)
    }

    /// Write user data (keystrokes, pastes, ZMODEM payload).
    ///
    /// Everything leaving the socket that is not itself a command has to have
    /// its `0xFF` bytes doubled, or the server reads them as `IAC` and starts
    /// interpreting the payload as negotiation. Negotiation replies produced by
    /// the reader go straight to `writer` instead, already framed.
    pub async fn write_bytes(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let (writer, iac) = {
            let guard = self.sessions.lock().await;
            let session = guard.get(id).ok_or_else(|| "Telnet session not found".to_string())?;
            (session.writer.clone(), session.iac.clone())
        };
        let binary = iac.lock().await.binary_out();
        writer
            .send(crate::util::telnet_escape_outbound(data, binary))
            .map_err(|_| "Failed to send Telnet bytes".to_string())
    }
}

#[tauri::command]
pub async fn start_telnet_session(
    app: AppHandle,
    state: State<'_, TelnetState>,
    zmodem: State<'_, crate::zmodem::ZmodemState>,
    id: String,
    host: String,
    port: u16,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let endpoint = format!("{host}:{port}");
    let connect = async move {
        // A server that never answers the SYN would otherwise leave the tab
        // on "Connecting..." for the OS timeout (minutes).
        crate::connection_test::tcp_connect(
            (host.as_str(), port),
            &endpoint,
            crate::connection_test::TCP_CONNECT_TIMEOUT,
        )
        .await
        .map_err(|failure| failure.detail)
    };
    let hub = state.hub.clone();
    let zmodem = zmodem.inner().clone();
    let session_id = id.clone();
    open_session(&state, id, connect, move |stream| {
        spawn_session(app, hub, zmodem, session_id, cols, rows, stream)
    })
    .await
}

/// Connect and register a Telnet session.
///
/// `connect` yields the socket and `spawn` starts the session's IO on it.
async fn open_session(
    state: &TelnetState,
    id: String,
    connect: impl std::future::Future<Output = Result<TcpStream, String>>,
    spawn: impl FnOnce(TcpStream) -> TelnetSession,
) -> Result<(), String> {
    let ticket = state.starts.begin(&id);
    let stream = connect.await?;
    // Register under the map lock, after checking whether the tab was closed
    // while connecting: a close that ran before the insert found nothing to
    // close and cancelled the ticket instead. Dropping `stream` hangs up.
    let mut sessions = state.sessions.lock().await;
    if ticket.is_cancelled() {
        return Err(crate::util::CLOSED_WHILE_CONNECTING.to_string());
    }
    // Restarting a tab reuses its id; the old connection must not live on.
    if let Some(old) = sessions.insert(id, spawn(stream)) {
        old.stop();
    }
    Ok(())
}

fn spawn_session(
    app: AppHandle,
    event_hub: Arc<crate::terminal_event_hub::TerminalEventHub>,
    zmodem_state: crate::zmodem::ZmodemState,
    id: String,
    cols: u16,
    rows: u16,
    stream: TcpStream,
) -> TelnetSession {
    let (mut reader, mut writer) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let iac = Arc::new(Mutex::new(crate::util::TelnetIacFilter::with_window_size(cols, rows)));

    let read_app = app.clone();
    let read_id = id.clone();
    let response_tx = tx.clone();
    let reader_iac = iac.clone();
    zmodem_state.reset_session(&id);
    let reader_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 4096];
        let mut decoder = crate::util::Utf8StreamDecoder::new();
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    event_hub.publish(&read_id, &crate::terminal_event_hub::TerminalEvent::Exit(
                        "Telnet connection closed".into()));
                    crate::util::emit_pty_exit(&read_app, &read_id, "Telnet connection closed");
                    break;
                }
                Ok(size) => {
                    // Strip IAC command sequences before decoding so option bytes
                    // never corrupt the terminal; reply to negotiations via the
                    // writer channel.
                    let filtered = reader_iac.lock().await.push(&buffer[..size]);
                    if !filtered.data.is_empty() {
                        event_hub.publish(&read_id, &crate::terminal_event_hub::TerminalEvent::Output(
                            filtered.data.clone()));
                    }
                    if !filtered.response.is_empty() {
                        let _ = response_tx.send(filtered.response);
                    }
                    let (_, response) = crate::util::pump_stream_chunk(
                        &read_app,
                        &read_id,
                        &mut decoder,
                        &filtered.data,
                        &zmodem_state,
                    );
                    if !response.is_empty() {
                        // ZMODEM replies are payload, not commands, so they need
                        // the same escaping as anything the user types.
                        let binary = reader_iac.lock().await.binary_out();
                        let _ = response_tx
                            .send(crate::util::telnet_escape_outbound(&response, binary));
                    }
                }
                Err(error) => {
                    event_hub.publish(&read_id, &crate::terminal_event_hub::TerminalEvent::Exit(
                        error.to_string()));
                    crate::util::emit_pty_exit(
                        &read_app,
                        &read_id,
                        format!("Telnet read error: {}", error),
                    );
                    break;
                }
            }
        }
    });

    let write_app = app.clone();
    let write_id = id.clone();
    let writer_task = tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(INTERACTIVE_WRITE_TIMEOUT_SECS),
                writer.write_all(&data),
            ).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    crate::util::emit_pty_exit(
                        &write_app,
                        &write_id,
                        format!("Telnet write error: {}", error),
                    );
                    break;
                }
                Err(_) => {
                    crate::util::emit_pty_exit(
                        &write_app,
                        &write_id,
                        format!(
                            "Telnet write timed out after {} seconds",
                            INTERACTIVE_WRITE_TIMEOUT_SECS
                        ),
                    );
                    break;
                }
            }
        }
        let _ = writer.shutdown().await;
    });

    TelnetSession {
        writer: tx,
        reader_abort: reader_task.abort_handle(),
        writer_abort: writer_task.abort_handle(),
        iac,
    }
}

#[tauri::command]
pub async fn write_telnet_bytes(
    state: State<'_, TelnetState>,
    id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    state.write_bytes(&id, &data).await
}

#[tauri::command]
pub async fn resize_telnet(
    state: State<'_, TelnetState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let (writer, iac) = {
        let guard = state.sessions.lock().await;
        let session = guard.get(&id).ok_or_else(|| "Telnet session not found".to_string())?;
        (session.writer.clone(), session.iac.clone())
    };
    let response = iac.lock().await.update_window_size(cols, rows);
    if !response.is_empty() {
        writer.send(response).map_err(|_| "Failed to send Telnet window size".to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn write_telnet_input(
    state: State<'_, TelnetState>,
    id: String,
    data: String,
) -> Result<(), String> {
    state.write_bytes(&id, data.as_bytes()).await
}

/// Check that `host:port` accepts a TCP connection, for the New Session
/// dialog's "Test" button.
///
/// Telnet has no login step of its own to test, so reachability is all there
/// is. The socket is shut down straight away without writing a byte: no IAC
/// negotiation, no banner read, nothing registered in `TelnetState`.
#[tauri::command]
pub async fn telnet_test_connection(
    host: String,
    port: u16,
) -> Result<crate::connection_test::ConnectionTestReport, String> {
    use crate::connection_test::{tcp_connect, ConnectionTestReport, TestCode, TCP_CONNECT_TIMEOUT};

    let started = std::time::Instant::now();
    let endpoint = format!("{host}:{port}");
    // Same address form as `start_telnet_session`.
    Ok(match tcp_connect((host.as_str(), port), &endpoint, TCP_CONNECT_TIMEOUT).await {
        Ok(mut stream) => {
            let _ = stream.shutdown().await;
            ConnectionTestReport::new(TestCode::Ok, started)
        }
        Err(failure) => ConnectionTestReport::from_failure(failure, started),
    })
}

#[tauri::command]
pub async fn close_telnet_session(
    state: State<'_, TelnetState>,
    zmodem: State<'_, crate::zmodem::ZmodemState>,
    id: String,
) -> Result<(), String> {
    close_session(&state, zmodem.inner(), &id).await;
    Ok(())
}

async fn close_session(state: &TelnetState, zmodem: &crate::zmodem::ZmodemState, id: &str) {
    // Still connecting: nothing is registered yet, so stop the start instead.
    state.starts.cancel(id);
    let session = state.sessions.lock().await.remove(id);
    if let Some(session) = session {
        session.stop();
    }
    zmodem.reset_session(id);
}

#[cfg(test)]
mod tests {
    use super::{close_session, open_session, telnet_test_connection, TelnetSession, TelnetState};
    use std::sync::Arc;
    use crate::connection_test::{TestCode, TestStatus};
    use std::time::{Duration, Instant};
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn a_listening_port_passes_without_a_single_byte_sent() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut received = Vec::new();
            // Ends at the client's FIN; bounded in case it never comes.
            let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut received)).await;
            (read.is_ok(), received)
        });

        let report = telnet_test_connection("127.0.0.1".to_string(), port)
            .await
            .expect("an expected outcome is never Err");
        assert_eq!(report.code, TestCode::Ok);
        assert_eq!(report.status, TestStatus::Success);
        assert!(report.detail.is_none());

        let (closed, received) = server.await.expect("server task");
        assert!(closed, "the test must hang up, not leave the socket open");
        // No IAC negotiation, no keystrokes: nothing reaches the device.
        assert!(received.is_empty(), "client sent {received:?}");
    }

    #[tokio::test]
    async fn a_closed_port_is_reported_as_refused() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let started = Instant::now();
        let report = telnet_test_connection("127.0.0.1".to_string(), port)
            .await
            .expect("an expected outcome is never Err");
        assert_eq!(report.code, TestCode::Refused);
        assert_eq!(report.status, TestStatus::Failure);
        assert!(report.detail.as_deref().unwrap_or("").contains(&format!("127.0.0.1:{port}")));
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn an_unknown_host_is_a_resolve_failure_not_an_error() {
        let report = telnet_test_connection("auraterm-test.invalid".to_string(), 23)
            .await
            .expect("an expected outcome is never Err");
        assert_eq!(report.code, TestCode::ResolveFailed);
    }

    fn telnet_state() -> TelnetState {
        TelnetState::new(Arc::new(crate::terminal_event_hub::TerminalEventHub::new()))
    }

    /// Stand-in for the real IO tasks: one task that owns the socket, so
    /// aborting the session is what hangs it up.
    fn hold_socket(stream: tokio::net::TcpStream) -> TelnetSession {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let _stream = stream;
            std::future::pending::<()>().await;
        });
        TelnetSession {
            writer: tx,
            reader_abort: task.abort_handle(),
            writer_abort: task.abort_handle(),
            iac: Arc::new(tokio::sync::Mutex::new(crate::util::TelnetIacFilter::default())),
        }
    }

    async fn expect_hang_up(stream: &mut tokio::net::TcpStream) {
        let mut byte = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte)).await;
        assert!(matches!(read, Ok(Ok(0))), "connection left open: {read:?}");
    }

    #[tokio::test]
    async fn a_tab_closed_while_connecting_leaves_no_session_behind() {
        let state = telnet_state();
        let zmodem = crate::zmodem::ZmodemState::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let release = Arc::new(tokio::sync::Notify::new());

        // The connect completes only after the tab has been closed.
        let gate = release.clone();
        let start = {
            let state = state.clone();
            tokio::spawn(async move {
                let connect = async move {
                    gate.notified().await;
                    tokio::net::TcpStream::connect(("127.0.0.1", port))
                        .await
                        .map_err(|e| e.to_string())
                };
                open_session(&state, "tab-1".into(), connect, hold_socket).await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        close_session(&state, &zmodem, "tab-1").await;
        release.notify_one();

        let result = start.await.expect("start task");
        assert_eq!(result, Err(crate::util::CLOSED_WHILE_CONNECTING.to_string()));
        assert!(!state.contains("tab-1").await);
        let (mut stream, _) = listener.accept().await.expect("accept");
        expect_hang_up(&mut stream).await;
    }

    #[tokio::test]
    async fn restarting_a_tab_hangs_up_the_connection_it_replaces() {
        let state = telnet_state();
        let zmodem = crate::zmodem::ZmodemState::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let connect = || async move {
            tokio::net::TcpStream::connect(("127.0.0.1", port)).await.map_err(|e| e.to_string())
        };

        open_session(&state, "tab-1".into(), connect(), hold_socket).await.expect("first start");
        let (mut first, _) = listener.accept().await.expect("accept first");
        open_session(&state, "tab-1".into(), connect(), hold_socket).await.expect("second start");
        expect_hang_up(&mut first).await;

        assert!(state.contains("tab-1").await);
        let (mut second, _) = listener.accept().await.expect("accept second");
        close_session(&state, &zmodem, "tab-1").await;
        expect_hang_up(&mut second).await;
    }
}
