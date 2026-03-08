use russh::{
    client,
    keys::{decode_secret_key, PrivateKeyWithHashAlg},
    ChannelMsg,
};
use russh_sftp::client::SftpSession;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};

type SharedSshHandle = Arc<Mutex<client::Handle<ClientHandler>>>;
const SSH_TRANSFER_PROGRESS_EVENT: &str = "ssh-transfer-progress";
const TRANSFER_CHUNK_SIZE: usize = 64 * 1024;
const TRANSFER_PROGRESS_EMIT_STEP: u64 = 256 * 1024;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshTransferMode {
    Sftp,
    Scp,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshTransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshTransferStatus {
    Started,
    Progress,
    Completed,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTransferProgressPayload {
    pub id: String,
    pub direction: SshTransferDirection,
    pub status: SshTransferStatus,
    pub mode: SshTransferMode,
    pub file_name: String,
    pub remote_path: String,
    pub local_path: Option<String>,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: Option<u64>,
    pub permissions: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirectoryListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<RemoteFileEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SshMfaPrompt {
    pub text: String,
    pub echo: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeyboardInteractivePromptPayload {
    pub id: String,
    pub name: String,
    pub instruction: String,
    pub prompts: Vec<SshMfaPrompt>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TerminalDataPayload {
    pub id: String,
    pub data: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PtyExitPayload {
    pub id: String,
    pub message: String,
}

struct ClientHandler {}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    fn check_server_key(self: &mut Self, _server_public_key: &russh::keys::PublicKey) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        async { Ok(true) }
    }
}

#[derive(Clone)]
pub struct SshState {
    pub connections: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    pub auth_responses: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    pub resize_channels: Arc<Mutex<HashMap<String, mpsc::Sender<(u32, u32)>>>>,
    handles: Arc<Mutex<HashMap<String, SharedSshHandle>>>,
}

impl Default for SshState {
    fn default() -> Self {
        Self::new()
    }
}

impl SshState {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            auth_responses: Arc::new(Mutex::new(HashMap::new())),
            resize_channels: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

fn remote_parent_path(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }

    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) => Some("/".to_string()),
        Some((parent, _)) if !parent.is_empty() => Some(parent.to_string()),
        _ => None,
    }
}

fn join_remote_path(base: &str, name: &str) -> String {
    let clean_name = name.trim_matches('/');
    if base == "/" {
        format!("/{clean_name}")
    } else {
        format!("{}/{}", base.trim_end_matches('/'), clean_name)
    }
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn expand_local_path(path: Option<&str>) -> PathBuf {
    let fallback = default_download_dir();
    let Some(value) = path else {
        return fallback;
    };

    let trimmed = value.trim();
    if trimmed.is_empty() {
        return fallback;
    }

    if trimmed == "~" {
        return home_dir();
    }

    if let Some(stripped) = trimmed.strip_prefix("~/") {
        return home_dir().join(stripped);
    }

    PathBuf::from(trimmed)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            let mut combined = PathBuf::from(drive);
            combined.push(path);
            Some(combined)
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_download_dir() -> PathBuf {
    home_dir().join("AuraTerm").join("downloads")
}

fn unique_local_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }

    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();

    for index in 1.. {
        let candidate = dir.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    dir.join(file_name)
}

fn file_name_from_remote_path(path: &str) -> Result<String, String> {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .ok_or_else(|| format!("Invalid remote path: {path}"))
}

fn emit_transfer_progress(
    app: &AppHandle,
    id: &str,
    direction: SshTransferDirection,
    status: SshTransferStatus,
    mode: SshTransferMode,
    file_name: &str,
    remote_path: &str,
    local_path: Option<&Path>,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
) {
    let _ = app.emit(
        SSH_TRANSFER_PROGRESS_EVENT,
        SshTransferProgressPayload {
            id: id.to_string(),
            direction,
            status,
            mode,
            file_name: file_name.to_string(),
            remote_path: remote_path.to_string(),
            local_path: local_path.map(|value| value.to_string_lossy().into_owned()),
            transferred_bytes,
            total_bytes,
        },
    );
}

fn maybe_emit_transfer_progress(
    app: &AppHandle,
    id: &str,
    direction: SshTransferDirection,
    mode: SshTransferMode,
    file_name: &str,
    remote_path: &str,
    local_path: Option<&Path>,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
    last_reported_bytes: &mut u64,
) {
    if transferred_bytes == 0 {
        return;
    }

    if total_bytes.is_some_and(|total| transferred_bytes >= total) {
        return;
    }

    if transferred_bytes.saturating_sub(*last_reported_bytes) < TRANSFER_PROGRESS_EMIT_STEP {
        return;
    }

    *last_reported_bytes = transferred_bytes;
    emit_transfer_progress(
        app,
        id,
        direction,
        SshTransferStatus::Progress,
        mode,
        file_name,
        remote_path,
        local_path,
        transferred_bytes,
        total_bytes,
    );
}

async fn cleanup_ssh_session(state: &SshState, id: &str) {
    state.connections.lock().await.remove(id);
    state.resize_channels.lock().await.remove(id);
    state.auth_responses.lock().await.remove(id);
    state.handles.lock().await.remove(id);
}

async fn get_ssh_handle(state: &SshState, id: &str) -> Result<SharedSshHandle, String> {
    state
        .handles
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| "SSH session is not ready yet".to_string())
}

async fn open_ssh_channel(
    state: &SshState,
    id: &str,
) -> Result<russh::Channel<client::Msg>, String> {
    let handle = get_ssh_handle(state, id).await?;
    let guard = handle.lock().await;
    guard
        .channel_open_session()
        .await
        .map_err(|error| format!("Failed to open SSH channel: {error}"))
}

async fn open_sftp_session(state: &SshState, id: &str) -> Result<SftpSession, String> {
    let channel = open_ssh_channel(state, id).await?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| format!("Failed to start SFTP subsystem: {error}"))?;

    SftpSession::new(channel.into_stream())
        .await
        .map_err(|error| format!("Failed to initialize SFTP session: {error}"))
}

async fn read_scp_line(
    stream: &mut BufReader<russh::ChannelStream<client::Msg>>,
) -> Result<String, String> {
    let mut buffer = Vec::new();
    let read = stream
        .read_until(b'\n', &mut buffer)
        .await
        .map_err(|error| format!("SCP read failed: {error}"))?;
    if read == 0 {
        return Err("SCP connection closed unexpectedly".to_string());
    }
    if matches!(buffer.last(), Some(b'\n')) {
        buffer.pop();
    }
    Ok(String::from_utf8_lossy(&buffer).to_string())
}

async fn read_scp_ack(
    stream: &mut BufReader<russh::ChannelStream<client::Msg>>,
) -> Result<(), String> {
    let mut code = [0_u8; 1];
    stream
        .read_exact(&mut code)
        .await
        .map_err(|error| format!("SCP acknowledgement failed: {error}"))?;

    match code[0] {
        0 => Ok(()),
        1 | 2 => {
            let message = read_scp_line(stream).await?;
            Err(format!("SCP error: {message}"))
        }
        value => Err(format!("Unexpected SCP response byte: {value}")),
    }
}

async fn scp_upload(
    app: &AppHandle,
    state: &SshState,
    id: &str,
    remote_dir: &str,
    file_name: &str,
    data: &[u8],
) -> Result<(), String> {
    let remote_path = join_remote_path(remote_dir, file_name);
    let channel = open_ssh_channel(state, id).await?;
    channel
        .exec(true, format!("scp -t {}", shell_escape(remote_dir)))
        .await
        .map_err(|error| format!("Failed to start SCP upload: {error}"))?;

    let mut stream = BufReader::new(channel.into_stream());
    read_scp_ack(&mut stream).await?;

    let header = format!("C0644 {} {}\n", data.len(), file_name);
    stream
        .get_mut()
        .write_all(header.as_bytes())
        .await
        .map_err(|error| format!("Failed to write SCP header: {error}"))?;
    stream
        .get_mut()
        .flush()
        .await
        .map_err(|error| format!("Failed to flush SCP header: {error}"))?;
    read_scp_ack(&mut stream).await?;

    let total_bytes = data.len() as u64;
    let mut transferred_bytes = 0_u64;
    let mut last_reported_bytes = 0_u64;

    for chunk in data.chunks(TRANSFER_CHUNK_SIZE) {
        stream
            .get_mut()
            .write_all(chunk)
            .await
            .map_err(|error| format!("Failed to write SCP data: {error}"))?;
        transferred_bytes += chunk.len() as u64;
        maybe_emit_transfer_progress(
            app,
            id,
            SshTransferDirection::Upload,
            SshTransferMode::Scp,
            file_name,
            &remote_path,
            None,
            transferred_bytes,
            Some(total_bytes),
            &mut last_reported_bytes,
        );
    }
    stream
        .get_mut()
        .write_all(&[0])
        .await
        .map_err(|error| format!("Failed to finalize SCP upload: {error}"))?;
    stream
        .get_mut()
        .flush()
        .await
        .map_err(|error| format!("Failed to flush SCP data: {error}"))?;
    read_scp_ack(&mut stream).await?;
    stream
        .get_mut()
        .shutdown()
        .await
        .map_err(|error| format!("Failed to close SCP upload stream: {error}"))?;

    Ok(())
}

async fn scp_download_to_path(
    app: &AppHandle,
    state: &SshState,
    id: &str,
    remote_path: &str,
    destination: &Path,
    file_name: &str,
    total_bytes: Option<u64>,
) -> Result<u64, String> {
    let channel = open_ssh_channel(state, id).await?;
    channel
        .exec(true, format!("scp -f {}", shell_escape(remote_path)))
        .await
        .map_err(|error| format!("Failed to start SCP download: {error}"))?;

    let mut stream = BufReader::new(channel.into_stream());
    stream
        .get_mut()
        .write_all(&[0])
        .await
        .map_err(|error| format!("Failed to request SCP download: {error}"))?;
    stream
        .get_mut()
        .flush()
        .await
        .map_err(|error| format!("Failed to flush SCP request: {error}"))?;

    let header = loop {
        let line = read_scp_line(&mut stream).await?;
        if let Some(rest) = line.strip_prefix('T') {
            let _ = rest;
            stream
                .get_mut()
                .write_all(&[0])
                .await
                .map_err(|error| format!("Failed to acknowledge SCP time header: {error}"))?;
            stream
                .get_mut()
                .flush()
                .await
                .map_err(|error| format!("Failed to flush SCP time acknowledgement: {error}"))?;
            continue;
        }
        break line;
    };

    if header.starts_with('D') {
        return Err("SCP directory download is not supported in the current UI".to_string());
    }

    let mut parts = header.splitn(3, ' ');
    let mode = parts.next().unwrap_or_default();
    let size = parts
        .next()
        .ok_or_else(|| format!("Invalid SCP header: {header}"))?
        .parse::<u64>()
        .map_err(|error| format!("Invalid SCP file size: {error}"))?;
    let _name = parts
        .next()
        .ok_or_else(|| format!("Invalid SCP header: {header}"))?;

    if !mode.starts_with('C') {
        return Err(format!("Unsupported SCP header: {header}"));
    }

    stream
        .get_mut()
        .write_all(&[0])
        .await
        .map_err(|error| format!("Failed to acknowledge SCP file header: {error}"))?;
    stream
        .get_mut()
        .flush()
        .await
        .map_err(|error| format!("Failed to flush SCP file acknowledgement: {error}"))?;

    let mut local_file = fs::File::create(destination)
        .await
        .map_err(|error| format!("Failed to create local file: {error}"))?;
    let mut remaining = size;
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_SIZE];
    let mut transferred_bytes = 0_u64;
    let mut last_reported_bytes = 0_u64;
    let expected_total = total_bytes.or(Some(size));

    while remaining > 0 {
        let next = remaining.min(buffer.len() as u64) as usize;
        stream
            .read_exact(&mut buffer[..next])
            .await
            .map_err(|error| format!("Failed to read SCP data: {error}"))?;
        local_file
            .write_all(&buffer[..next])
            .await
            .map_err(|error| format!("Failed to write local file: {error}"))?;
        remaining -= next as u64;
        transferred_bytes += next as u64;
        maybe_emit_transfer_progress(
            app,
            id,
            SshTransferDirection::Download,
            SshTransferMode::Scp,
            file_name,
            remote_path,
            Some(destination),
            transferred_bytes,
            expected_total,
            &mut last_reported_bytes,
        );
    }

    let mut trailer = [0_u8; 1];
    stream
        .read_exact(&mut trailer)
        .await
        .map_err(|error| format!("Failed to finalize SCP download: {error}"))?;
    if trailer[0] != 0 {
        return Err(format!("Unexpected SCP trailer byte: {}", trailer[0]));
    }

    stream
        .get_mut()
        .write_all(&[0])
        .await
        .map_err(|error| format!("Failed to acknowledge downloaded file: {error}"))?;
    stream
        .get_mut()
        .flush()
        .await
        .map_err(|error| format!("Failed to flush download acknowledgement: {error}"))?;
    stream
        .get_mut()
        .shutdown()
        .await
        .map_err(|error| format!("Failed to close SCP download stream: {error}"))?;

    Ok(transferred_bytes)
}

#[tauri::command]
pub async fn ssh_list_remote_dir(
    state: State<'_, SshState>,
    id: String,
    path: Option<String>,
) -> Result<RemoteDirectoryListing, String> {
    let sftp = open_sftp_session(state.inner(), &id).await?;
    let requested_path = path.unwrap_or_else(|| ".".to_string());
    let canonical_path = sftp
        .canonicalize(requested_path.clone())
        .await
        .unwrap_or(requested_path);

    let mut entries = Vec::new();
    for entry in sftp
        .read_dir(canonical_path.clone())
        .await
        .map_err(|error| format!("Failed to read remote directory: {error}"))?
    {
        let metadata = entry.metadata();
        let modified_at = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());
        let kind = if metadata.is_dir() {
            "directory"
        } else if metadata.is_symlink() {
            "symlink"
        } else if metadata.is_regular() {
            "file"
        } else {
            "other"
        };
        let name = entry.file_name();
        entries.push(RemoteFileEntry {
            path: join_remote_path(&canonical_path, &name),
            name,
            kind: kind.to_string(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified_at,
            permissions: metadata.permissions().to_string(),
        });
    }

    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    let _ = sftp.close().await;

    Ok(RemoteDirectoryListing {
        parent: remote_parent_path(&canonical_path),
        path: canonical_path,
        entries,
    })
}

#[tauri::command]
pub async fn ssh_create_remote_dir(
    state: State<'_, SshState>,
    id: String,
    parent_path: String,
    name: String,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Folder name cannot be empty".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Folder name cannot contain path separators".to_string());
    }

    let remote_path = join_remote_path(&parent_path, name);
    let sftp = open_sftp_session(state.inner(), &id).await?;
    sftp.create_dir(remote_path.clone())
        .await
        .map_err(|error| format!("Failed to create remote directory: {error}"))?;
    let _ = sftp.close().await;
    Ok(remote_path)
}

#[tauri::command]
pub async fn ssh_remove_remote_entry(
    state: State<'_, SshState>,
    id: String,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let sftp = open_sftp_session(state.inner(), &id).await?;
    if is_dir {
        sftp.remove_dir(path)
            .await
            .map_err(|error| format!("Failed to remove remote directory: {error}"))?;
    } else {
        sftp.remove_file(path)
            .await
            .map_err(|error| format!("Failed to remove remote file: {error}"))?;
    }
    let _ = sftp.close().await;
    Ok(())
}

#[tauri::command]
pub async fn ssh_upload_file(
    app: AppHandle,
    state: State<'_, SshState>,
    id: String,
    remote_dir: String,
    file_name: String,
    data: Vec<u8>,
    mode: SshTransferMode,
) -> Result<(), String> {
    let file_name = Path::new(&file_name)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .ok_or_else(|| "Invalid file name".to_string())?;
    let remote_path = join_remote_path(&remote_dir, &file_name);
    let total_bytes = data.len() as u64;

    emit_transfer_progress(
        &app,
        &id,
        SshTransferDirection::Upload,
        SshTransferStatus::Started,
        mode,
        &file_name,
        &remote_path,
        None,
        0,
        Some(total_bytes),
    );

    match mode {
        SshTransferMode::Sftp => {
            let sftp = open_sftp_session(state.inner(), &id).await?;
            let mut remote_file = sftp
                .create(remote_path.clone())
                .await
                .map_err(|error| format!("Failed to create remote file: {error}"))?;
            let mut transferred_bytes = 0_u64;
            let mut last_reported_bytes = 0_u64;

            for chunk in data.chunks(TRANSFER_CHUNK_SIZE) {
                remote_file
                    .write_all(chunk)
                    .await
                    .map_err(|error| format!("Failed to upload file over SFTP: {error}"))?;
                transferred_bytes += chunk.len() as u64;
                maybe_emit_transfer_progress(
                    &app,
                    &id,
                    SshTransferDirection::Upload,
                    mode,
                    &file_name,
                    &remote_path,
                    None,
                    transferred_bytes,
                    Some(total_bytes),
                    &mut last_reported_bytes,
                );
            }
            remote_file
                .shutdown()
                .await
                .map_err(|error| format!("Failed to finalize SFTP upload: {error}"))?;
            let _ = sftp.close().await;
            emit_transfer_progress(
                &app,
                &id,
                SshTransferDirection::Upload,
                SshTransferStatus::Completed,
                mode,
                &file_name,
                &remote_path,
                None,
                total_bytes,
                Some(total_bytes),
            );
            Ok(())
        }
        SshTransferMode::Scp => {
            scp_upload(&app, state.inner(), &id, &remote_dir, &file_name, &data).await?;
            emit_transfer_progress(
                &app,
                &id,
                SshTransferDirection::Upload,
                SshTransferStatus::Completed,
                mode,
                &file_name,
                &remote_path,
                None,
                total_bytes,
                Some(total_bytes),
            );
            Ok(())
        }
    }
}

#[tauri::command]
pub async fn ssh_download_file(
    app: AppHandle,
    state: State<'_, SshState>,
    id: String,
    remote_path: String,
    local_dir: Option<String>,
    expected_size: Option<u64>,
    mode: SshTransferMode,
) -> Result<String, String> {
    let file_name = file_name_from_remote_path(&remote_path)?;
    let destination_dir = expand_local_path(local_dir.as_deref());
    fs::create_dir_all(&destination_dir)
        .await
        .map_err(|error| format!("Failed to create download directory: {error}"))?;

    let destination_path = unique_local_path(&destination_dir, &file_name);
    let total_bytes = expected_size.filter(|value| *value > 0);

    emit_transfer_progress(
        &app,
        &id,
        SshTransferDirection::Download,
        SshTransferStatus::Started,
        mode,
        &file_name,
        &remote_path,
        Some(&destination_path),
        0,
        total_bytes,
    );

    let transferred_bytes = match mode {
        SshTransferMode::Sftp => {
            let sftp = open_sftp_session(state.inner(), &id).await?;
            let mut remote_file = sftp
                .open(remote_path.clone())
                .await
                .map_err(|error| format!("Failed to open remote file: {error}"))?;
            let mut local_file = fs::File::create(&destination_path)
                .await
                .map_err(|error| format!("Failed to create local file: {error}"))?;
            let mut buffer = vec![0_u8; TRANSFER_CHUNK_SIZE];
            let mut transferred_bytes = 0_u64;
            let mut last_reported_bytes = 0_u64;

            loop {
                let read = remote_file
                    .read(&mut buffer)
                    .await
                    .map_err(|error| format!("Failed to download file over SFTP: {error}"))?;
                if read == 0 {
                    break;
                }
                local_file
                    .write_all(&buffer[..read])
                    .await
                    .map_err(|error| format!("Failed to write local file: {error}"))?;
                transferred_bytes += read as u64;
                maybe_emit_transfer_progress(
                    &app,
                    &id,
                    SshTransferDirection::Download,
                    mode,
                    &file_name,
                    &remote_path,
                    Some(&destination_path),
                    transferred_bytes,
                    total_bytes,
                    &mut last_reported_bytes,
                );
            }
            remote_file
                .shutdown()
                .await
                .map_err(|error| format!("Failed to finalize SFTP download: {error}"))?;
            let _ = sftp.close().await;
            transferred_bytes
        }
        SshTransferMode::Scp => {
            scp_download_to_path(
                &app,
                state.inner(),
                &id,
                &remote_path,
                &destination_path,
                &file_name,
                total_bytes,
            )
            .await?
        }
    };

    emit_transfer_progress(
        &app,
        &id,
        SshTransferDirection::Download,
        SshTransferStatus::Completed,
        mode,
        &file_name,
        &remote_path,
        Some(&destination_path),
        transferred_bytes,
        Some(total_bytes.unwrap_or(transferred_bytes)),
    );

    Ok(destination_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn answer_ssh_mfa(
    state: State<'_, SshState>,
    id: String,
    responses: Vec<String>,
) -> Result<(), String> {
    let mut auth_responses = state.auth_responses.lock().await;
    if let Some(tx) = auth_responses.get_mut(&id) {
        for response in responses {
            tx.send(response)
                .await
                .map_err(|_| "Failed to send MFA response".to_string())?;
        }
        Ok(())
    } else {
        Err("Auth response channel not found".to_string())
    }
}

#[tauri::command]
pub async fn write_ssh_pty_input(
    state: State<'_, SshState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let mut connections = state.connections.lock().await;
    if let Some(tx) = connections.get_mut(&id) {
        let _ = tx.send(data).await;
        Ok(())
    } else {
        Err("Connection not found".to_string())
    }
}

#[tauri::command]
pub async fn resize_ssh_pty(
    state: State<'_, SshState>,
    id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let mut resize_channels = state.resize_channels.lock().await;
    if let Some(tx) = resize_channels.get_mut(&id) {
        let _ = tx.send((cols, rows)).await;
        Ok(())
    } else {
        Err("Connection not found".to_string())
    }
}

#[tauri::command]
pub async fn close_ssh_pty(
    state: State<'_, SshState>,
    id: String,
) -> Result<(), String> {
    cleanup_ssh_session(state.inner(), &id).await;
    Ok(())
}

#[tauri::command]
pub async fn start_ssh_pty(
    app: AppHandle,
    state: State<'_, SshState>,
    id: String,
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    private_key: Option<String>,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    println!(
        "Starting SSH connection to {}@{}:{} with JumpServer bypass for MFA",
        user, host, port
    );

    cleanup_ssh_session(state.inner(), &id).await;

    let (auth_response_tx, mut auth_response_rx) = mpsc::channel(1);

    state
        .auth_responses
        .lock()
        .await
        .insert(id.clone(), auth_response_tx);

    let handler = ClientHandler {};

    let config = std::sync::Arc::new(client::Config {
        ..Default::default()
    });

    let addr = format!("{}:{}", host, port);
    println!("Connecting to {}...", addr);
    let result: Result<(), String> = async {
        let mut session = client::connect(config, addr, handler)
            .await
            .map_err(|error| format!("Connection error: {error}"))?;

        println!("Authenticating as {}...", user);

        let password = password.filter(|value| !value.is_empty());
        let private_key = private_key.filter(|value| !value.trim().is_empty());

        let auth_res_start = if let Some(private_key) = private_key {
            let decoded_key = decode_secret_key(&private_key, password.as_deref())
                .map_err(|error| format!("Private key parse error: {error}"))?;
            let rsa_hash = session
                .best_supported_rsa_hash()
                .await
                .map_err(|error| format!("Failed to determine RSA hash algorithm: {error}"))?
                .flatten();

            let result = session
                .authenticate_publickey(
                    user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(decoded_key), rsa_hash),
                )
                .await;

            match result {
                Ok(russh::client::AuthResult::Success) => {
                    println!("Public key authentication successful!");
                    None
                }
                Ok(russh::client::AuthResult::Failure { .. }) => {
                    return Err("Private key authentication failed".to_string());
                }
                Err(error) => return Err(format!("Private key auth error: {error}")),
            }
        } else if let Some(pwd) = password {
            let result = session.authenticate_password(user.clone(), pwd).await;
            match result {
                Ok(russh::client::AuthResult::Success) => {
                    println!("Password authentication successful!");
                    None
                }
                Ok(russh::client::AuthResult::Failure { .. }) => {
                    println!("Password auth failed, falling back to keyboard-interactive");
                    Some(session.authenticate_keyboard_interactive_start(user.clone(), None).await)
                }
                Err(error) => return Err(format!("Password auth error: {error}")),
            }
        } else {
            Some(session.authenticate_keyboard_interactive_start(user.clone(), None).await)
        };

        if let Some(auth_res_future) = auth_res_start {
            let mut auth_res = auth_res_future
                .map_err(|error| format!("Keyboard interactive init failed: {error}"))?;

            loop {
                match auth_res {
                    russh::client::KeyboardInteractiveAuthResponse::Success => {
                        println!("Authentication successful!");
                        break;
                    }
                    russh::client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                        return Err("Authentication failed".to_string());
                    }
                    russh::client::KeyboardInteractiveAuthResponse::InfoRequest {
                        name,
                        instructions,
                        prompts,
                    } => {
                        let payload = KeyboardInteractivePromptPayload {
                            id: id.clone(),
                            name: name.clone(),
                            instruction: instructions.clone(),
                            prompts: prompts
                                .iter()
                                .map(|prompt| SshMfaPrompt {
                                    text: prompt.prompt.clone(),
                                    echo: prompt.echo,
                                })
                                .collect(),
                        };

                        let _ = app.emit("ssh-mfa-prompt", payload);

                        let mut answers = Vec::new();
                        for _ in prompts.iter() {
                            if let Some(response) = auth_response_rx.recv().await {
                                answers.push(response);
                            } else {
                                return Err("Failed to collect MFA response".to_string());
                            }
                        }

                        auth_res = session
                            .authenticate_keyboard_interactive_respond(answers)
                            .await
                            .map_err(|error| format!("Failed auth step: {error}"))?;
                    }
                }
            }
        }

        let session_handle: SharedSshHandle = Arc::new(Mutex::new(session));

        println!("Requesting PTY and shell...");
        let mut channel = {
            let handle = session_handle.lock().await;
            handle
                .channel_open_session()
                .await
                .map_err(|error| format!("Channel error: {error}"))?
        };

        channel
            .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .map_err(|error| format!("PTY request failed: {error}"))?;

        channel
            .request_shell(true)
            .await
            .map_err(|error| format!("Shell request failed: {error}"))?;

        let (input_tx, mut input_rx) = mpsc::channel::<String>(32);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(32);

        state.connections.lock().await.insert(id.clone(), input_tx);
        state.resize_channels.lock().await.insert(id.clone(), resize_tx);
        state.handles.lock().await.insert(id.clone(), session_handle);

        let connection_id = id.clone();
        let app_handle = app.clone();
        let state_clone = state.inner().clone();

        let _ = app.emit(
            "ssh-connected",
            TerminalDataPayload {
                id: id.clone(),
                data: String::new(),
            },
        );

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(msg) = channel.wait() => {
                        match msg {
                            ChannelMsg::Data { ref data } | ChannelMsg::ExtendedData { ref data, .. } => {
                                let _ = app_handle.emit(
                                    "pty-output",
                                    TerminalDataPayload {
                                        id: connection_id.clone(),
                                        data: String::from_utf8_lossy(data).to_string(),
                                    },
                                );
                            }
                            ChannelMsg::Eof => {
                                let _ = app_handle.emit(
                                    "pty-exit",
                                    PtyExitPayload {
                                        id: connection_id.clone(),
                                        message: "SSH channel closed".to_string(),
                                    },
                                );
                                break;
                            }
                            ChannelMsg::Close => {
                                let _ = app_handle.emit(
                                    "pty-exit",
                                    PtyExitPayload {
                                        id: connection_id.clone(),
                                        message: "SSH connection closed".to_string(),
                                    },
                                );
                                break;
                            }
                            _ => {}
                        }
                    }
                    Some(input) = input_rx.recv() => {
                        let _ = channel.data(input.into_bytes().as_slice()).await;
                    }
                    Some((c, r)) = resize_rx.recv() => {
                        let _ = channel.window_change(c, r, 0, 0).await;
                    }
                    else => break,
                }
            }

            cleanup_ssh_session(&state_clone, &connection_id).await;
        });

        Ok(())
    }
    .await;

    if result.is_err() {
        cleanup_ssh_session(state.inner(), &id).await;
    }

    result
}
