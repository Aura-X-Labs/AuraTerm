use crate::{PtyExitEvent, PtyOutputEvent};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::task::AbortHandle;

const INTERACTIVE_WRITE_TIMEOUT_SECS: u64 = 10;

struct TelnetSession {
    writer: mpsc::UnboundedSender<String>,
    reader_abort: AbortHandle,
    writer_abort: AbortHandle,
}

#[derive(Clone, Default)]
pub struct TelnetState {
    sessions: Arc<Mutex<HashMap<String, TelnetSession>>>,
}

#[tauri::command]
pub async fn start_telnet_session(
    app: AppHandle,
    state: State<'_, TelnetState>,
    id: String,
    host: String,
    port: u16,
) -> Result<(), String> {
    let stream = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| e.to_string())?;
    let (mut reader, mut writer) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let read_app = app.clone();
    let read_id = id.clone();
    let reader_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 4096];
        let mut decoder = crate::util::Utf8StreamDecoder::new();
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    let _ = read_app.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: read_id.clone(),
                            message: "Telnet connection closed".to_string(),
                        },
                    );
                    break;
                }
                Ok(size) => {
                    let output = decoder.push(&buffer[..size]);
                    if output.is_empty() {
                        continue;
                    }
                    let _ = read_app.emit(
                        "pty-output",
                        PtyOutputEvent {
                            id: read_id.clone(),
                            data: output,
                        },
                    );
                }
                Err(error) => {
                    let _ = read_app.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: read_id.clone(),
                            message: format!("Telnet read error: {}", error),
                        },
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
                writer.write_all(data.as_bytes()),
            ).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let _ = write_app.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: write_id.clone(),
                            message: format!("Telnet write error: {}", error),
                        },
                    );
                    break;
                }
                Err(_) => {
                    let _ = write_app.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: write_id.clone(),
                            message: format!(
                                "Telnet write timed out after {} seconds",
                                INTERACTIVE_WRITE_TIMEOUT_SECS
                            ),
                        },
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
        },
    );

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
        .send(data)
        .map_err(|_| "Failed to send Telnet input".to_string())
}

#[tauri::command]
pub async fn close_telnet_session(
    state: State<'_, TelnetState>,
    id: String,
) -> Result<(), String> {
    let mut guard = state.sessions.lock().await;
    if let Some(session) = guard.remove(&id) {
        session.reader_abort.abort();
        session.writer_abort.abort();
    }
    Ok(())
}
