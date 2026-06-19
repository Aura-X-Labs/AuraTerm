use serde::Serialize;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use zmodem2::{Action, Event, FileInfo, Position, Receiver, Sender};

const RECEIVE_SIGNATURE: &[u8] = b"**\x18B00";
const SEND_SIGNATURE: &[u8] = b"**\x18B01";
const PROGRESS_STEP: u64 = 64 * 1024;
const MAX_BROWSER_UPLOAD: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Upload,
    Download,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZmodemEvent {
    pub id: String,
    direction: Direction,
    status: String,
    file_name: Option<String>,
    local_path: Option<String>,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
    message: Option<String>,
}

pub struct ProcessedChunk {
    pub terminal: Vec<u8>,
    pub response: Vec<u8>,
}

#[derive(Default)]
struct Detector {
    pending: Vec<u8>,
}

impl Detector {
    fn push(&mut self, bytes: &[u8]) -> (Vec<u8>, Option<(Direction, Vec<u8>)>) {
        self.pending.extend_from_slice(bytes);
        if let Some((index, direction)) = find_signature(&self.pending) {
            let terminal = self.pending[..index].to_vec();
            let protocol = self.pending[index..].to_vec();
            self.pending.clear();
            return (terminal, Some((direction, protocol)));
        }

        let keep = longest_signature_prefix_suffix(&self.pending);
        let emit_len = self.pending.len().saturating_sub(keep);
        let terminal = self.pending.drain(..emit_len).collect();
        (terminal, None)
    }

    fn reset(&mut self) {
        self.pending.clear();
    }
}

fn find_signature(bytes: &[u8]) -> Option<(usize, Direction)> {
    for index in 0..bytes.len() {
        if bytes[index..].starts_with(RECEIVE_SIGNATURE) {
            return Some((index, Direction::Download));
        }
        if bytes[index..].starts_with(SEND_SIGNATURE) {
            return Some((index, Direction::Upload));
        }
    }
    None
}

fn longest_signature_prefix_suffix(bytes: &[u8]) -> usize {
    let max = bytes.len().min(RECEIVE_SIGNATURE.len().saturating_sub(1));
    (1..=max)
        .rev()
        .find(|length| {
            let suffix = &bytes[bytes.len() - length..];
            RECEIVE_SIGNATURE.starts_with(suffix) || SEND_SIGNATURE.starts_with(suffix)
        })
        .unwrap_or(0)
}

enum Transfer {
    AwaitingUpload { wire: Vec<u8> },
    Sending(SendTransfer),
    Receiving(ReceiveTransfer),
}

#[derive(Default)]
struct StreamSession {
    detector: Detector,
    transfer: Option<Transfer>,
}

struct SendTransfer {
    machine: Sender,
    file_name: String,
    data: Vec<u8>,
    wire: Vec<u8>,
    transferred: u64,
    last_reported: u64,
}

struct ReceiveTransfer {
    machine: Receiver,
    directory: PathBuf,
    file: Option<File>,
    file_name: Option<String>,
    local_path: Option<PathBuf>,
    wire: Vec<u8>,
    transferred: u64,
    total: Option<u64>,
    last_reported: u64,
}

#[derive(Clone, Default)]
pub struct ZmodemState {
    sessions: Arc<Mutex<HashMap<String, StreamSession>>>,
}

impl ZmodemState {
    pub fn reset_session(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(id);
        }
    }

    pub fn process_incoming(&self, app: &AppHandle, id: &str, bytes: &[u8]) -> ProcessedChunk {
        let Ok(mut sessions) = self.sessions.lock() else {
            return ProcessedChunk {
                terminal: bytes.to_vec(),
                response: Vec::new(),
            };
        };
        let session = sessions.entry(id.to_string()).or_default();

        if session.transfer.is_none() {
            let (terminal, detection) = session.detector.push(bytes);
            let Some((direction, protocol)) = detection else {
                return ProcessedChunk {
                    terminal,
                    response: Vec::new(),
                };
            };
            emit_event(app, id, direction, "detected", None, None, 0, None, None);
            match direction {
                Direction::Upload => {
                    session.transfer = Some(Transfer::AwaitingUpload { wire: protocol });
                    return ProcessedChunk {
                        terminal,
                        response: Vec::new(),
                    };
                }
                Direction::Download => {
                    let directory = download_directory(app);
                    match Receiver::new() {
                        Ok(machine) => {
                            session.transfer = Some(Transfer::Receiving(ReceiveTransfer {
                                machine,
                                directory,
                                file: None,
                                file_name: None,
                                local_path: None,
                                wire: protocol,
                                transferred: 0,
                                total: None,
                                last_reported: 0,
                            }));
                        }
                        Err(error) => {
                            emit_event(
                                app,
                                id,
                                direction,
                                "failed",
                                None,
                                None,
                                0,
                                None,
                                Some(error.to_string()),
                            );
                            session.detector.reset();
                            return ProcessedChunk {
                                terminal,
                                response: Vec::new(),
                            };
                        }
                    }
                }
            }

            let response = drive_transfer(app, id, session);
            return ProcessedChunk { terminal, response };
        }

        match session.transfer.as_mut() {
            Some(Transfer::AwaitingUpload { wire }) => wire.extend_from_slice(bytes),
            Some(Transfer::Sending(transfer)) => transfer.wire.extend_from_slice(bytes),
            Some(Transfer::Receiving(transfer)) => transfer.wire.extend_from_slice(bytes),
            None => {}
        }
        let response = drive_transfer(app, id, session);
        ProcessedChunk {
            terminal: Vec::new(),
            response,
        }
    }

    pub fn start_send(
        &self,
        app: &AppHandle,
        id: &str,
        file_name: String,
        data: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        if data.is_empty() {
            return Err("Cannot send an empty file with Zmodem".to_string());
        }
        if data.len() > MAX_BROWSER_UPLOAD {
            return Err("Zmodem browser uploads are limited to 256 MiB".to_string());
        }
        let size = u32::try_from(data.len())
            .map_err(|_| "Zmodem file exceeds 4 GiB protocol limit".to_string())?;
        let safe_name = safe_file_name(&file_name);
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Zmodem state is unavailable".to_string())?;
        let session = sessions.entry(id.to_string()).or_default();
        let buffered = match session.transfer.take() {
            Some(Transfer::AwaitingUpload { wire }) => wire,
            other => {
                session.transfer = other;
                return Err("The remote session is not waiting for a Zmodem upload".to_string());
            }
        };
        let mut machine = Sender::new().map_err(|error| error.to_string())?;
        machine
            .start_file(FileInfo::new(
                safe_name.as_bytes(),
                Some(Position::new(size)),
            ))
            .map_err(|error| error.to_string())?;
        machine.finish().map_err(|error| error.to_string())?;
        session.transfer = Some(Transfer::Sending(SendTransfer {
            machine,
            file_name: safe_name.clone(),
            data,
            wire: buffered,
            transferred: 0,
            last_reported: 0,
        }));
        emit_event(
            app,
            id,
            Direction::Upload,
            "started",
            Some(safe_name),
            None,
            0,
            Some(size as u64),
            None,
        );
        Ok(drive_transfer(app, id, session))
    }

    pub fn cancel(&self, app: &AppHandle, id: &str) -> Vec<u8> {
        let mut direction = Direction::Download;
        if let Ok(mut sessions) = self.sessions.lock() {
            if let Some(session) = sessions.get_mut(id) {
                if matches!(
                    session.transfer,
                    Some(Transfer::Sending(_)) | Some(Transfer::AwaitingUpload { .. })
                ) {
                    direction = Direction::Upload;
                }
                session.transfer = None;
                session.detector.reset();
            }
        }
        emit_event(app, id, direction, "cancelled", None, None, 0, None, None);
        let mut cancel = vec![0x18; 8];
        cancel.extend_from_slice(&[0x08; 8]);
        cancel
    }
}

fn drive_transfer(app: &AppHandle, id: &str, session: &mut StreamSession) -> Vec<u8> {
    let mut response = Vec::new();
    let mut finished = false;
    let result = match session.transfer.as_mut() {
        Some(Transfer::Sending(transfer)) => {
            drive_sender(app, id, transfer, &mut response, &mut finished)
        }
        Some(Transfer::Receiving(transfer)) => {
            drive_receiver(app, id, transfer, &mut response, &mut finished)
        }
        _ => Ok(()),
    };
    if let Err(error) = result {
        let direction = match session.transfer {
            Some(Transfer::Sending(_)) | Some(Transfer::AwaitingUpload { .. }) => Direction::Upload,
            _ => Direction::Download,
        };
        emit_event(
            app,
            id,
            direction,
            "failed",
            None,
            None,
            0,
            None,
            Some(error),
        );
        finished = true;
    }
    if finished {
        session.transfer = None;
        session.detector.reset();
    }
    response
}

enum OwnedAction {
    WriteWire(Vec<u8>),
    ReadFile { offset: usize, max_len: usize },
    WriteFile(Vec<u8>),
    Event(OwnedEvent),
    Idle,
}

enum OwnedEvent {
    FileStarted { name: String, size: Option<u64> },
    FileCompleted,
    SessionCompleted,
    Aborted,
}

fn own_action(action: Action<'_>) -> OwnedAction {
    match action {
        Action::WriteWire(bytes) => OwnedAction::WriteWire(bytes.to_vec()),
        Action::ReadFile { offset, max_len } => OwnedAction::ReadFile {
            offset: offset.get() as usize,
            max_len,
        },
        Action::WriteFile(bytes) => OwnedAction::WriteFile(bytes.to_vec()),
        Action::Event(event) => OwnedAction::Event(match event {
            Event::FileStarted(info) => OwnedEvent::FileStarted {
                name: String::from_utf8_lossy(info.name).to_string(),
                size: info.size.map(|value| value.get() as u64),
            },
            Event::FileCompleted => OwnedEvent::FileCompleted,
            Event::SessionCompleted => OwnedEvent::SessionCompleted,
            Event::Aborted => OwnedEvent::Aborted,
            _ => OwnedEvent::Aborted,
        }),
        Action::Idle => OwnedAction::Idle,
        _ => OwnedAction::Idle,
    }
}

fn drive_sender(
    app: &AppHandle,
    id: &str,
    transfer: &mut SendTransfer,
    response: &mut Vec<u8>,
    finished: &mut bool,
) -> Result<(), String> {
    for _ in 0..4096 {
        match own_action(transfer.machine.poll()) {
            OwnedAction::WriteWire(bytes) => {
                response.extend_from_slice(&bytes);
                transfer.machine.wire_written(bytes.len());
            }
            OwnedAction::ReadFile { offset, max_len } => {
                let end = transfer.data.len().min(offset.saturating_add(max_len));
                if offset >= end {
                    return Err("Zmodem sender requested data beyond the file".to_string());
                }
                transfer
                    .machine
                    .submit_file(&transfer.data[offset..end])
                    .map_err(|error| error.to_string())?;
                transfer.transferred = end as u64;
                maybe_emit_progress(
                    app,
                    id,
                    Direction::Upload,
                    &transfer.file_name,
                    None,
                    transfer.transferred,
                    Some(transfer.data.len() as u64),
                    &mut transfer.last_reported,
                );
            }
            OwnedAction::Event(OwnedEvent::FileCompleted) => {}
            OwnedAction::Event(OwnedEvent::SessionCompleted) => {
                emit_event(
                    app,
                    id,
                    Direction::Upload,
                    "completed",
                    Some(transfer.file_name.clone()),
                    None,
                    transfer.data.len() as u64,
                    Some(transfer.data.len() as u64),
                    None,
                );
                *finished = true;
                break;
            }
            OwnedAction::Event(OwnedEvent::Aborted) => {
                return Err("Remote aborted the Zmodem upload".to_string())
            }
            OwnedAction::Idle => {
                if transfer.wire.is_empty() {
                    break;
                }
                let consumed = transfer
                    .machine
                    .submit_wire(&transfer.wire)
                    .map_err(|error| error.to_string())?;
                if consumed == 0 {
                    break;
                }
                transfer.wire.drain(..consumed);
            }
            _ => {}
        }
    }
    Ok(())
}

fn drive_receiver(
    app: &AppHandle,
    id: &str,
    transfer: &mut ReceiveTransfer,
    response: &mut Vec<u8>,
    finished: &mut bool,
) -> Result<(), String> {
    for _ in 0..4096 {
        match own_action(transfer.machine.poll()) {
            OwnedAction::WriteWire(bytes) => {
                response.extend_from_slice(&bytes);
                transfer.machine.wire_written(bytes.len());
            }
            OwnedAction::WriteFile(bytes) => {
                let file = transfer
                    .file
                    .as_mut()
                    .ok_or_else(|| "Zmodem data arrived before file metadata".to_string())?;
                file.write_all(&bytes)
                    .map_err(|error| format!("Failed to save Zmodem download: {error}"))?;
                transfer
                    .machine
                    .file_written(bytes.len())
                    .map_err(|error| error.to_string())?;
                transfer.transferred = transfer.transferred.saturating_add(bytes.len() as u64);
                maybe_emit_progress(
                    app,
                    id,
                    Direction::Download,
                    transfer.file_name.as_deref().unwrap_or("download"),
                    transfer.local_path.as_ref(),
                    transfer.transferred,
                    transfer.total,
                    &mut transfer.last_reported,
                );
            }
            OwnedAction::Event(OwnedEvent::FileStarted { name, size }) => {
                std::fs::create_dir_all(&transfer.directory).map_err(|error| {
                    format!("Failed to create Zmodem download directory: {error}")
                })?;
                let file_name = safe_file_name(&name);
                let path = unique_path(&transfer.directory, &file_name);
                let file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)
                    .map_err(|error| format!("Failed to create Zmodem download: {error}"))?;
                transfer.file = Some(file);
                transfer.file_name = Some(file_name.clone());
                transfer.local_path = Some(path.clone());
                transfer.transferred = 0;
                transfer.total = size;
                emit_event(
                    app,
                    id,
                    Direction::Download,
                    "started",
                    Some(file_name),
                    Some(path),
                    0,
                    size,
                    None,
                );
            }
            OwnedAction::Event(OwnedEvent::FileCompleted) => {
                if let Some(file) = transfer.file.as_mut() {
                    file.flush()
                        .map_err(|error| format!("Failed to finalize Zmodem download: {error}"))?;
                }
                emit_event(
                    app,
                    id,
                    Direction::Download,
                    "completed",
                    transfer.file_name.clone(),
                    transfer.local_path.clone(),
                    transfer.transferred,
                    transfer.total,
                    None,
                );
                transfer.file = None;
            }
            OwnedAction::Event(OwnedEvent::SessionCompleted) => {
                *finished = true;
                break;
            }
            OwnedAction::Event(OwnedEvent::Aborted) => {
                return Err("Remote aborted the Zmodem download".to_string())
            }
            OwnedAction::Idle => {
                if transfer.wire.is_empty() {
                    break;
                }
                let consumed = transfer
                    .machine
                    .submit_wire(&transfer.wire)
                    .map_err(|error| error.to_string())?;
                if consumed == 0 {
                    break;
                }
                transfer.wire.drain(..consumed);
            }
            _ => {}
        }
    }
    Ok(())
}

fn maybe_emit_progress(
    app: &AppHandle,
    id: &str,
    direction: Direction,
    file_name: &str,
    path: Option<&PathBuf>,
    transferred: u64,
    total: Option<u64>,
    last_reported: &mut u64,
) {
    if transferred.saturating_sub(*last_reported) < PROGRESS_STEP && total != Some(transferred) {
        return;
    }
    *last_reported = transferred;
    emit_event(
        app,
        id,
        direction,
        "progress",
        Some(file_name.to_string()),
        path.cloned(),
        transferred,
        total,
        None,
    );
}

fn emit_event(
    app: &AppHandle,
    id: &str,
    direction: Direction,
    status: &str,
    file_name: Option<String>,
    local_path: Option<PathBuf>,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
    message: Option<String>,
) {
    let _ = app.emit(
        &crate::util::session_event("zmodem", id),
        ZmodemEvent {
            id: id.to_string(),
            direction,
            status: status.to_string(),
            file_name,
            local_path: local_path.map(|value| value.to_string_lossy().to_string()),
            transferred_bytes,
            total_bytes,
            message,
        },
    );
}

fn safe_file_name(value: &str) -> String {
    Path::new(value.trim())
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .unwrap_or("zmodem-download.bin")
        .to_string()
}

fn unique_path(directory: &Path, file_name: &str) -> PathBuf {
    let initial = directory.join(file_name);
    if !initial.exists() {
        return initial;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let candidate = match extension {
            Some(extension) => directory.join(format!("{stem} ({index}).{extension}")),
            None => directory.join(format!("{stem} ({index})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{stem}-{}", uuid::Uuid::new_v4()))
}

fn download_directory(app: &AppHandle) -> PathBuf {
    crate::settings::get_settings(app.clone())
        .ok()
        .map(|settings| expand_tilde(&settings.zmodem_download_path))
        .unwrap_or_else(|| expand_tilde("~/AuraTerm/downloads"))
}

fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" || value.starts_with("~/") {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(home) = home {
            return if value == "~" {
                PathBuf::from(home)
            } else {
                PathBuf::from(home).join(&value[2..])
            };
        }
    }
    PathBuf::from(value)
}

#[tauri::command]
pub fn zmodem_start_send(
    app: AppHandle,
    state: tauri::State<'_, ZmodemState>,
    id: String,
    file_name: String,
    data: Vec<u8>,
) -> Result<Vec<u8>, String> {
    state.start_send(&app, &id, file_name, data)
}

#[tauri::command]
pub fn zmodem_cancel(app: AppHandle, state: tauri::State<'_, ZmodemState>, id: String) -> Vec<u8> {
    state.cancel(&app, &id)
}

#[cfg(test)]
mod tests {
    use super::{
        find_signature, longest_signature_prefix_suffix, safe_file_name, Detector, Direction,
    };

    #[test]
    fn detector_preserves_normal_output_and_split_signatures() {
        let mut detector = Detector::default();
        let (first, found) = detector.push(b"hello **\x18");
        assert_eq!(first, b"hello ");
        assert!(found.is_none());
        let (second, found) = detector.push(b"B00000000000000");
        assert!(second.is_empty());
        assert!(matches!(found, Some((Direction::Download, _))));
    }

    #[test]
    fn detector_identifies_upload_handshake() {
        assert!(matches!(
            find_signature(b"rz\r**\x18B0100"),
            Some((3, Direction::Upload))
        ));
        assert_eq!(longest_signature_prefix_suffix(b"text*"), 1);
    }

    #[test]
    fn incoming_names_cannot_escape_download_directory() {
        assert_eq!(safe_file_name("../../secret.txt"), "secret.txt");
        assert_eq!(safe_file_name(""), "zmodem-download.bin");
    }
}
