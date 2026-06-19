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

#[derive(Clone, Default)]
pub struct TelnetState {
    sessions: Arc<Mutex<HashMap<String, TelnetSession>>>,
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
    let stream = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| e.to_string())?;
    let (mut reader, mut writer) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let iac = Arc::new(Mutex::new(crate::util::TelnetIacFilter::with_window_size(cols, rows)));

    let read_app = app.clone();
    let read_id = id.clone();
    let response_tx = tx.clone();
    let reader_iac = iac.clone();
    let zmodem_state = zmodem.inner().clone();
    zmodem_state.reset_session(&id);
    let reader_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 4096];
        let mut decoder = crate::util::Utf8StreamDecoder::new();
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    crate::util::emit_pty_exit(&read_app, &read_id, "Telnet connection closed");
                    break;
                }
                Ok(size) => {
                    // Strip IAC command sequences before decoding so option bytes
                    // never corrupt the terminal; reply to negotiations via the
                    // writer channel.
                    let filtered = reader_iac.lock().await.push(&buffer[..size]);
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
                        let _ = response_tx.send(response);
                    }
                }
                Err(error) => {
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

    let mut guard = state.sessions.lock().await;
    guard.insert(
        id,
        TelnetSession {
            writer: tx,
            reader_abort: reader_task.abort_handle(),
            writer_abort: writer_task.abort_handle(),
            iac,
        },
    );

    Ok(())
}

#[tauri::command]
pub async fn write_telnet_bytes(
    state: State<'_, TelnetState>,
    id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let guard = state.sessions.lock().await;
    let session = guard.get(&id).ok_or_else(|| "Telnet session not found".to_string())?;
    session.writer.send(data).map_err(|_| "Failed to send Telnet bytes".to_string())
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
    let guard = state.sessions.lock().await;
    let Some(session) = guard.get(&id) else {
        return Err("Telnet session not found".to_string());
    };

    session
        .writer
        .send(data.into_bytes())
        .map_err(|_| "Failed to send Telnet input".to_string())
}

#[tauri::command]
pub async fn close_telnet_session(
    state: State<'_, TelnetState>,
    zmodem: State<'_, crate::zmodem::ZmodemState>,
    id: String,
) -> Result<(), String> {
    let mut guard = state.sessions.lock().await;
    if let Some(session) = guard.remove(&id) {
        session.reader_abort.abort();
        session.writer_abort.abort();
    }
    zmodem.reset_session(&id);
    Ok(())
}
