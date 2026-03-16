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
const AURATERM_RECONNECT_SESSION_PREFIX: &str = "at-";
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

enum InteractiveWriteOutcome {
    Sent,
    Dropped(String),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconnectSessionPromptPayload {
    pub id: String,
    pub tool: String,
    pub sessions: Vec<String>,
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
    reconnect_prompt_responses: Arc<Mutex<HashMap<String, mpsc::Sender<Option<String>>>>>,
    /// Tracks whether auto-reconnect is active for each session.
    /// When set to false, the reconnect loop will exit.
    reconnect_flags: Arc<Mutex<HashMap<String, Arc<tokio::sync::Notify>>>>,
    /// Stores reconnect config per session: (host, port, user, password, private_key, reconnect_type)
    reconnect_configs: Arc<Mutex<HashMap<String, ReconnectConfig>>>,
}

#[derive(Clone)]
struct ReconnectConfig {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    private_key: Option<String>,
    reconnect_type: ReconnectType,
    session_name: String,
    checked_existing_sessions: bool,
    cols: u32,
    rows: u32,
    last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReconnectType {
    /// Manual reconnect: after disconnect the user can trigger a new connection from the UI.
    Manual,
    /// Reconnect only: no server-side session manager. Running tasks will be lost on disconnect.
    Simple,
    Screen,
    Tmux,
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
            reconnect_prompt_responses: Arc::new(Mutex::new(HashMap::new())),
            reconnect_flags: Arc::new(Mutex::new(HashMap::new())),
            reconnect_configs: Arc::new(Mutex::new(HashMap::new())),
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

fn build_screen_attach_command(session_name: &str) -> String {
    let escaped_session_name = shell_escape(session_name);
    format!(
        concat!(
            "tmp_rc=$(mktemp /tmp/auraterm-screenrc.XXXXXX 2>/dev/null || mktemp -t auraterm-screenrc.XXXXXX) || exit 1; ",
            "printf '%s\\n%s\\n%s\\n' 'termcapinfo xterm* ti@:te@' 'defscrollback 10000' 'escape ^Bb' > \"$tmp_rc\"; ",
            "printf '\\033[32m[AuraTerm] Screen mode: wheel scroll and 10000 lines scrollback enabled. Escape key is Ctrl+B.\\033[0m\\n'; ",
            "screen -S {sess} -X eval 'termcapinfo xterm* ti@:te@' 'defscrollback 10000' 'escape ^Bb' >/dev/null 2>&1 || true; ",
            "screen -dr {sess} 2>/dev/null || screen -c \"$tmp_rc\" -S {sess}; ",
            "status=$?; rm -f \"$tmp_rc\"; exit $status"
        ),
        sess = escaped_session_name,
    )
}

fn auraterm_reconnect_session_name(id: &str) -> String {
    format!("{AURATERM_RECONNECT_SESSION_PREFIX}{id}")
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

async fn cleanup_ssh_runtime_state(state: &SshState, id: &str) {
    state.connections.lock().await.remove(id);
    state.resize_channels.lock().await.remove(id);
    state.auth_responses.lock().await.remove(id);
    state.handles.lock().await.remove(id);
    state.reconnect_prompt_responses.lock().await.remove(id);
}

async fn cleanup_ssh_session(state: &SshState, id: &str) {
    cleanup_ssh_runtime_state(state, id).await;
    state.reconnect_configs.lock().await.remove(id);
    // Do NOT remove reconnect_flags here — that's handled only in close_ssh_pty
    // to allow the reconnect loop to restart the connection.
}

/// Stop reconnect loop for a session and clean up all state.
async fn stop_and_cleanup_ssh_session(state: &SshState, id: &str) {
    // Signal the reconnect loop to stop.
    if let Some(notify) = state.reconnect_flags.lock().await.remove(id) {
        notify.notify_waiters();
    }
    cleanup_ssh_session(state, id).await;
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

async fn set_reconnect_session_metadata(
    state: &SshState,
    id: &str,
    session_name: Option<String>,
    checked_existing_sessions: Option<bool>,
) {
    if let Some(config) = state.reconnect_configs.lock().await.get_mut(id) {
        if let Some(name) = session_name {
            config.session_name = name;
        }
        if let Some(checked) = checked_existing_sessions {
            config.checked_existing_sessions = checked;
        }
    }
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
pub async fn answer_ssh_reconnect_choice(
    state: State<'_, SshState>,
    id: String,
    session_name: Option<String>,
) -> Result<(), String> {
    let tx = state
        .reconnect_prompt_responses
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| "Reconnect prompt channel not found".to_string())?;

    tx.send(session_name.filter(|value| !value.trim().is_empty()))
        .await
        .map_err(|_| "Failed to send reconnect choice".to_string())
}

#[tauri::command]
pub async fn rename_ssh_session(
    state: State<'_, SshState>,
    id: String,
    new_name: String,
) -> Result<(), String> {
    let new_name = new_name.trim().to_string();
    if new_name.is_empty() {
        return Err("New session name cannot be empty".to_string());
    }

    let (old_name, reconnect_type) = {
        let configs = state.reconnect_configs.lock().await;
        let config = configs
            .get(&id)
            .ok_or_else(|| "SSH session not found or not in reconnect mode".to_string())?;
        (config.session_name.clone(), config.reconnect_type)
    };

    if old_name == new_name {
        return Ok(());
    }

    let rename_cmd = match reconnect_type {
        ReconnectType::Tmux => format!(
            "tmux rename-session -t {} {} 2>/dev/null || true",
            shell_escape(&old_name),
            shell_escape(&new_name),
        ),
        ReconnectType::Screen => format!(
            "screen -S {} -X sessionname {} 2>/dev/null; true",
            shell_escape(&old_name),
            shell_escape(&new_name),
        ),
        _ => return Err("Session is not in screen/tmux mode".to_string()),
    };

    let handle = get_ssh_handle(&state, &id).await?;
    run_remote_command_capture(&handle, rename_cmd).await?;

    if let Some(config) = state.reconnect_configs.lock().await.get_mut(&id) {
        config.session_name = new_name;
    }

    Ok(())
}

#[tauri::command]
pub async fn write_ssh_pty_input(
    app: AppHandle,
    state: State<'_, SshState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let tx = {
        let connections = state.connections.lock().await;
        connections.get(&id).cloned()
    };

    if let Some(tx) = tx {
        if tx.send(data).await.is_err() {
            let message = "SSH connection lost; input was not sent".to_string();
            let _ = app.emit(
                "pty-output",
                TerminalDataPayload {
                    id,
                    data: format!("\r\n\x1b[31m[{message}]\x1b[0m\r\n"),
                },
            );
            Err(message)
        } else {
            Ok(())
        }
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
    // For session-persistence modes (tmux/screen), try to destroy the remote session first.
    let config = state.reconnect_configs.lock().await.get(&id).cloned();
    if let Some(cfg) = config {
        let kill_cmd = match cfg.reconnect_type {
            ReconnectType::Manual => None,
            ReconnectType::Simple => None,
            ReconnectType::Tmux => Some(format!(
                "tmux kill-session -t {} 2>/dev/null; exit 0",
                shell_escape(&cfg.session_name)
            )),
            ReconnectType::Screen => Some(format!(
                "screen -S {} -X quit 2>/dev/null; exit 0",
                shell_escape(&cfg.session_name)
            )),
        };
        if let Some(cmd) = kill_cmd {
            if let Ok(handle) = get_ssh_handle(state.inner(), &id).await {
                // Best-effort: open a channel, run the kill command, ignore errors.
                let _ = async {
                    let guard = handle.lock().await;
                    let channel = guard.channel_open_session().await?;
                    channel.exec(true, cmd).await?;
                    // Drain until EOF so the server has time to process the command.
                    let mut ch = channel;
                    loop {
                        match ch.wait().await {
                            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                    Ok::<_, russh::Error>(())
                }.await;
            }
        }
    }
    stop_and_cleanup_ssh_session(state.inner(), &id).await;
    Ok(())
}

/// Authenticate an SSH session and return a connected handle.
async fn authenticate_ssh(
    addr: &str,
    user: &str,
    password: Option<&str>,
    private_key: Option<&str>,
    app: &AppHandle,
    id: &str,
    auth_response_rx: &mut mpsc::Receiver<String>,
) -> Result<SharedSshHandle, String> {
    let handler = ClientHandler {};
    let config = Arc::new(client::Config {
        // Send a keepalive every 15 s; disconnect only after 8 consecutive
        // missing replies (= 120 s). This more frequent probing helps detect
        // TCP silent drops by NAT/Firewall faster while maintaining tolerance.
        keepalive_interval: Some(std::time::Duration::from_secs(15)),
        keepalive_max: 8,
        ..Default::default()
    });

    let mut session = client::connect(config, addr, handler)
        .await
        .map_err(|error| format!("Connection error: {error}"))?;

    println!("Authenticating as {}...", user);

    let password = password.filter(|v| !v.is_empty());
    let private_key = private_key.filter(|v| !v.trim().is_empty());

    let auth_res_start = if let Some(pk) = private_key {
        let decoded_key = decode_secret_key(pk, password)
            .map_err(|error| format!("Private key parse error: {error}"))?;
        let rsa_hash = session
            .best_supported_rsa_hash()
            .await
            .map_err(|error| format!("Failed to determine RSA hash algorithm: {error}"))?
            .flatten();

        match session
            .authenticate_publickey(user.to_string(), PrivateKeyWithHashAlg::new(Arc::new(decoded_key), rsa_hash))
            .await
        {
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
        match session.authenticate_password(user.to_string(), pwd.to_string()).await {
            Ok(russh::client::AuthResult::Success) => {
                println!("Password authentication successful!");
                None
            }
            Ok(russh::client::AuthResult::Failure { .. }) => {
                println!("Password auth failed, falling back to keyboard-interactive");
                Some(session.authenticate_keyboard_interactive_start(user.to_string(), None).await)
            }
            Err(error) => return Err(format!("Password auth error: {error}")),
        }
    } else {
        Some(session.authenticate_keyboard_interactive_start(user.to_string(), None).await)
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
                russh::client::KeyboardInteractiveAuthResponse::InfoRequest { name, instructions, prompts } => {
                    let payload = KeyboardInteractivePromptPayload {
                        id: id.to_string(),
                        name: name.clone(),
                        instruction: instructions.clone(),
                        prompts: prompts.iter().map(|p| SshMfaPrompt { text: p.prompt.clone(), echo: p.echo }).collect(),
                    };
                    let _ = app.emit("ssh-mfa-prompt", payload);

                    let mut answers = Vec::new();
                    for _ in &prompts {
                        match auth_response_rx.recv().await {
                            Some(response) => answers.push(response),
                            None => return Err("Failed to collect MFA response".to_string()),
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

    Ok(Arc::new(Mutex::new(session)))
}

/// Check whether a command exists on the remote host.
async fn run_remote_command_capture(handle: &SharedSshHandle, command: String) -> Result<String, String> {
    let Ok(channel) = ({
        let guard = handle.lock().await;
        guard.channel_open_session().await
    }) else {
        return Err("Failed to open remote command channel".to_string());
    };

    channel
        .exec(true, command)
        .await
        .map_err(|error| format!("Remote command failed: {error}"))?;

    let mut output = String::new();
    let mut ch = channel;
    loop {
        match ch.wait().await {
            Some(ChannelMsg::Data { ref data }) | Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                output.push_str(&String::from_utf8_lossy(data));
            }
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
            _ => {}
        }
    }

    Ok(output)
}

async fn remote_command_exists(handle: &SharedSshHandle, cmd: &str) -> bool {
    let check_cmd = format!("command -v {} >/dev/null 2>&1 && echo __EXISTS__", cmd);
    match run_remote_command_capture(handle, check_cmd).await {
        Ok(output) => output.contains("__EXISTS__"),
        Err(_) => false,
    }
}

async fn list_detached_auraterm_sessions(
    handle: &SharedSshHandle,
    reconnect_type: ReconnectType,
) -> Result<Vec<String>, String> {
    let output = match reconnect_type {
        ReconnectType::Manual => return Ok(vec![]),
        ReconnectType::Simple => return Ok(vec![]),
        ReconnectType::Tmux => run_remote_command_capture(
            handle,
            "tmux list-sessions -F '#{session_name} #{?session_attached,1,0}' 2>/dev/null || true".to_string(),
        )
        .await?,
        ReconnectType::Screen => run_remote_command_capture(
            handle,
            "screen -ls 2>/dev/null || true".to_string(),
        )
        .await?,
    };

    let mut sessions = match reconnect_type {
        ReconnectType::Manual => vec![],
        ReconnectType::Simple => vec![],
        ReconnectType::Tmux => output
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let name = parts.next()?;
                let attached = parts.next()?;
                (attached == "0" && name.starts_with(AURATERM_RECONNECT_SESSION_PREFIX))
                    .then(|| name.to_string())
            })
            .collect::<Vec<_>>(),
        ReconnectType::Screen => output
            .lines()
            .filter(|line| line.contains("Detached"))
            .filter_map(|line| {
                let token = line.split_whitespace().next()?;
                let (_, name) = token.split_once('.')?;
                name.starts_with(AURATERM_RECONNECT_SESSION_PREFIX)
                    .then(|| name.to_string())
            })
            .collect::<Vec<_>>(),
    };

    sessions.sort();
    sessions.dedup();
    Ok(sessions)
}

async fn prompt_for_existing_reconnect_session(
    app: &AppHandle,
    state: &SshState,
    id: &str,
    reconnect_type: ReconnectType,
    sessions: Vec<String>,
) -> Result<Option<String>, String> {
    if sessions.is_empty() {
        return Ok(None);
    }

    let (tx, mut rx) = mpsc::channel::<Option<String>>(1);
    state
        .reconnect_prompt_responses
        .lock()
        .await
        .insert(id.to_string(), tx);

    let tool = match reconnect_type {
        ReconnectType::Manual => "manual",
        ReconnectType::Simple => "simple",
        ReconnectType::Tmux => "tmux",
        ReconnectType::Screen => "screen",
    };

    if app
        .emit(
            "ssh-reconnect-session-prompt",
            ReconnectSessionPromptPayload {
                id: id.to_string(),
                tool: tool.to_string(),
                sessions,
            },
        )
        .is_err()
    {
        state.reconnect_prompt_responses.lock().await.remove(id);
        return Ok(None);
    }

    let response = tokio::time::timeout(tokio::time::Duration::from_secs(120), rx.recv())
        .await
        .ok()
        .flatten()
        .flatten();

    state.reconnect_prompt_responses.lock().await.remove(id);
    Ok(response)
}

/// Open a PTY channel and attach/create a screen or tmux session.
/// For Manual/Simple mode or when the multiplexer is unavailable, falls back to a plain shell.
/// Returns (channel, used_multiplexer: bool).
async fn open_pty_channel(
    handle: &SharedSshHandle,
    session_name: &str,
    cols: u32,
    rows: u32,
    reconnect_type: Option<ReconnectType>,
) -> Result<(russh::Channel<client::Msg>, bool), String> {
    let guard = handle.lock().await;
    let channel = guard
        .channel_open_session()
        .await
        .map_err(|error| format!("Channel error: {error}"))?;
    drop(guard);

    channel
        .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .map_err(|error| format!("PTY request failed: {error}"))?;

    if let Some(rt) = reconnect_type {
        match rt {
            // Manual/Simple modes skip the multiplexer and go straight to the shell.
            ReconnectType::Manual => {}
            ReconnectType::Simple => {}
            ReconnectType::Tmux | ReconnectType::Screen => {
                let tool_name = match rt {
                    ReconnectType::Tmux => "tmux",
                    ReconnectType::Screen => "screen",
                    ReconnectType::Manual => unreachable!(),
                    ReconnectType::Simple => unreachable!(),
                };
                let tool_available = remote_command_exists(handle, tool_name).await;

                if tool_available {
                    let attach_cmd = match rt {
                        ReconnectType::Tmux => format!(
                            // Attach or create; then immediately enable mouse scrolling.
                            "tmux new-session -A -s {sess} 2>/dev/null || tmux attach-session -t {sess} 2>/dev/null || tmux new-session -s {sess}; tmux set -g mouse on 2>/dev/null; true",
                            sess = shell_escape(session_name)
                        ),
                        ReconnectType::Screen => build_screen_attach_command(session_name),
                        ReconnectType::Manual => unreachable!(),
                        ReconnectType::Simple => unreachable!(),
                    };
                    channel
                        .exec(false, attach_cmd)
                        .await
                        .map_err(|error| format!("Failed to start {}: {error}", tool_name))?;
                    return Ok((channel, true));
                }
                // Tool not available — fall through to plain shell with a notice (written after returning).
            }
        }
    }

    // Plain shell fallback (Simple mode or multiplexer unavailable).
    channel
        .request_shell(true)
        .await
        .map_err(|error| format!("Shell request failed: {error}"))?;
    Ok((channel, false))
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
    auto_reconnect: Option<bool>,
    reconnect_type: Option<ReconnectType>,
) -> Result<(), String> {
    println!("Starting SSH connection to {}@{}:{}", user, host, port);

    // Stop any previous session for this id.
    stop_and_cleanup_ssh_session(state.inner(), &id).await;

    let rt = reconnect_type.unwrap_or_else(|| {
        if auto_reconnect.unwrap_or(false) {
            ReconnectType::Simple
        } else {
            ReconnectType::Manual
        }
    });
    let use_reconnect = !matches!(rt, ReconnectType::Manual);

    // Always create a fresh auth-response channel registered under this id.
    let (auth_response_tx, auth_response_rx) = mpsc::channel::<String>(4);
    state.auth_responses.lock().await.insert(id.clone(), auth_response_tx);

    if use_reconnect {
        // Store reconnect configuration.
        state.reconnect_configs.lock().await.insert(id.clone(), ReconnectConfig {
            host: host.clone(),
            port,
            user: user.clone(),
            password: password.clone(),
            private_key: private_key.clone(),
            reconnect_type: rt,
            session_name: auraterm_reconnect_session_name(&id),
            checked_existing_sessions: false,
            cols,
            rows,
            last_error: None,
        });

        // Create a cancellation notifier and register it.
        let cancel_notify = Arc::new(tokio::sync::Notify::new());
        state.reconnect_flags.lock().await.insert(id.clone(), cancel_notify.clone());

        let state_clone = state.inner().clone();
        let app_clone = app.clone();
        let id_clone = id.clone();

        drop(auth_response_rx); // will be recreated fresh each reconnect attempt
        tokio::spawn(async move {
            run_reconnect_loop(
                app_clone,
                state_clone,
                id_clone,
                cancel_notify,
            ).await;
        });

        Ok(())
    } else {
        // Single-shot connection (no reconnect).
        let addr = format!("{}:{}", host, port);
        // For Manual/Simple mode, no multiplexer is needed even in single-shot mode.
        let mux_type = match rt {
            ReconnectType::Manual => None,
            ReconnectType::Simple => None,
            other => Some(other),
        };
        let result = do_single_ssh_connect(
            &app,
            state.inner(),
            &id,
            &addr,
            &user,
            password.as_deref(),
            private_key.as_deref(),
            cols,
            rows,
            mux_type,
            None,
            false,
            auth_response_rx,
        ).await;

        match result {
            Ok(_rx) => Ok(()),
            Err(err) => {
                cleanup_ssh_session(state.inner(), &id).await;
                Err(err)
            }
        }
    }
}

/// The reconnect loop: keeps trying to connect every 5 seconds until cancelled.
async fn run_reconnect_loop(
    app: AppHandle,
    state: SshState,
    id: String,
    cancel_notify: Arc<tokio::sync::Notify>,
) {
    let mut first_attempt = true;

    loop {
        // Check for cancellation before each attempt.
        if is_cancelled(&cancel_notify) {
            break;
        }

        if !first_attempt {
            // Get the last error if available
            let last_error = {
                let mut configs = state.reconnect_configs.lock().await;
                if let Some(cfg) = configs.get_mut(&id) {
                    cfg.last_error.take()
                } else {
                    None
                }
            };

            let notice = if let Some(err) = last_error {
                format!("\r\n\x1b[31m[Disconnected: {err}]\x1b[0m\r\n\x1b[33m[Reconnecting in 5 s...]\x1b[0m\r\n")
            } else {
                "\r\n\x1b[33m[Disconnected; reconnecting in 5 s...]\x1b[0m\r\n".to_string()
            };

            // Emit a notice to the terminal.
            let _ = app.emit(
                "pty-output",
                TerminalDataPayload {
                    id: id.clone(),
                    data: notice,
                },
            );

            // Wait 5 seconds, but abort early if cancelled.
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
                _ = cancel_notify.notified() => { break; }
            }

            if is_cancelled(&cancel_notify) {
                break;
            }
        }
        first_attempt = false;

        // Retrieve the latest reconnect config (cols/rows may have been updated by resize).
        let cfg = {
            let guard = state.reconnect_configs.lock().await;
            match guard.get(&id).cloned() {
                Some(c) => c,
                None => break,
            }
        };

        let addr = format!("{}:{}", cfg.host, cfg.port);

        // Ensure a fresh auth-response channel is available.
        let (auth_tx, rx) = mpsc::channel::<String>(4);
        state.auth_responses.lock().await.insert(id.clone(), auth_tx);

        // For Manual/Simple mode, pass None as reconnect_type so no multiplexer is used.
        let mux_type = match cfg.reconnect_type {
            ReconnectType::Manual => None,
            ReconnectType::Simple => None,
            other => Some(other),
        };
        let result = do_single_ssh_connect(
            &app,
            &state,
            &id,
            &addr,
            &cfg.user,
            cfg.password.as_deref(),
            cfg.private_key.as_deref(),
            cfg.cols,
            cfg.rows,
            mux_type,
            Some(cfg.session_name.clone()),
            !cfg.checked_existing_sessions,
            rx,
        ).await;

        match result {
            Ok(_new_rx) => {
                // Connection is running; wait until it drops (cleanup_ssh_session removes handles).
                // Poll until the handle is gone or we're cancelled.
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(500)) => {
                            let gone = state.handles.lock().await.get(&id).is_none();
                            if gone { break; }
                        }
                        _ = cancel_notify.notified() => { return; }
                    }
                }
            }
            Err(err) => {
                cleanup_ssh_runtime_state(&state, &id).await;
                let _ = app.emit(
                    "pty-output",
                    TerminalDataPayload {
                        id: id.clone(),
                        data: format!("\r\n\x1b[31m[Reconnect failed: {}]\x1b[0m\r\n", err),
                    },
                );
            }
        }
    }

    // Loop exited — do final cleanup.
    cleanup_ssh_session(&state, &id).await;
    let _ = app.emit(
        "pty-exit",
        PtyExitPayload {
            id: id.clone(),
            message: "SSH session ended".to_string(),
        },
    );
}

fn is_cancelled(notify: &Arc<tokio::sync::Notify>) -> bool {
    // Peek whether any waiters have been notified already by trying a non-blocking check.
    // Tokio Notify does not expose a "is_notified" API, so we use a workaround:
    // register a waker, immediately poll it.
    use std::future::Future;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    static VTABLE: RawWakerVTable = RawWakerVTable::new(
        |p| RawWaker::new(p, &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    let raw = RawWaker::new(std::ptr::null(), &VTABLE);
    let waker = unsafe { Waker::from_raw(raw) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(notify.notified());
    matches!(fut.as_mut().poll(&mut cx), Poll::Ready(()))
}

/// Connect once to SSH, open a PTY, and spawn the IO loop.
/// Returns Ok(auth_response_rx) on success (ownership moved into caller for reuse),
/// or Err on failure.
async fn do_single_ssh_connect(
    app: &AppHandle,
    state: &SshState,
    id: &str,
    addr: &str,
    user: &str,
    password: Option<&str>,
    private_key: Option<&str>,
    cols: u32,
    rows: u32,
    reconnect_type: Option<ReconnectType>,
    reconnect_session_name: Option<String>,
    should_prompt_existing_sessions: bool,
    mut auth_response_rx: mpsc::Receiver<String>,
) -> Result<mpsc::Receiver<String>, String> {
    println!("Connecting to {}...", addr);

    let session_handle = authenticate_ssh(addr, user, password, private_key, app, id, &mut auth_response_rx).await?;

    println!("Requesting PTY and shell...");
    let mut selected_session_name = reconnect_session_name.unwrap_or_else(|| auraterm_reconnect_session_name(id));

    if let Some(rt) = reconnect_type {
        let tool_name = match rt {
            ReconnectType::Manual => "",
            ReconnectType::Simple => "",
            ReconnectType::Tmux => "tmux",
            ReconnectType::Screen => "screen",
        };

        if should_prompt_existing_sessions {
            if remote_command_exists(&session_handle, tool_name).await {
                let sessions = list_detached_auraterm_sessions(&session_handle, rt).await?;
                if let Some(existing_session) =
                    prompt_for_existing_reconnect_session(app, state, id, rt, sessions).await?
                {
                    selected_session_name = existing_session;
                }
            }

            set_reconnect_session_metadata(
                state,
                id,
                Some(selected_session_name.clone()),
                Some(true),
            )
            .await;
        }
    }

    let (channel, used_multiplexer) = open_pty_channel(
        &session_handle,
        &selected_session_name,
        cols,
        rows,
        reconnect_type,
    )
    .await?;

    if reconnect_type.is_some() {
        set_reconnect_session_metadata(state, id, Some(selected_session_name.clone()), None).await;
    }

    let (input_tx, input_rx) = mpsc::channel::<String>(512);
    let (resize_tx, resize_rx) = mpsc::channel::<(u32, u32)>(32);

    state.connections.lock().await.insert(id.to_string(), input_tx);
    state.resize_channels.lock().await.insert(id.to_string(), resize_tx);
    state
        .handles
        .lock()
        .await
        .insert(id.to_string(), session_handle.clone());

    let _ = app.emit(
        "ssh-connected",
        TerminalDataPayload { id: id.to_string(), data: String::new() },
    );

    if !used_multiplexer {
        if let Some(rt) = reconnect_type {
            let tool = match rt {
                ReconnectType::Tmux => Some("tmux"),
                ReconnectType::Screen => Some("screen"),
                ReconnectType::Manual => None,
                ReconnectType::Simple => None,
            };
            if let Some(tool_name) = tool {
                let _ = app.emit(
                    "pty-output",
                    TerminalDataPayload {
                        id: id.to_string(),
                        data: format!(
                            "\r\n\x1b[33m[Reconnect mode: {} not found on remote host, using plain shell]\x1b[0m\r\n",
                            tool_name
                        ),
                    },
                );
            }
        }
    }

    let connection_id = id.to_string();
    let app_handle = app.clone();
    let state_clone = state.clone();
    let session_handle_for_io = session_handle.clone();

    tokio::spawn(async move {
        run_channel_io_loop(
            app_handle,
            state_clone,
            connection_id,
            session_handle_for_io,
            channel,
            input_rx,
            resize_rx,
        )
        .await;
    });

    Ok(auth_response_rx)
}

/// Drive the SSH channel: forward data to the frontend, forward input/resize to the channel.
async fn run_channel_io_loop(
    app: AppHandle,
    state: SshState,
    id: String,
    handle: SharedSshHandle,
    mut channel: russh::Channel<client::Msg>,
    mut input_rx: mpsc::Receiver<String>,
    mut resize_rx: mpsc::Receiver<(u32, u32)>,
) {
    let mut last_write_error_notice_at: Option<std::time::Instant> = None;
    let final_exit_message;

    loop {
        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { ref data }) | Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                        let _ = app.emit(
                            "pty-output",
                            TerminalDataPayload {
                                id: id.clone(),
                                data: String::from_utf8_lossy(data).to_string(),
                            },
                        );
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => {
                        final_exit_message = Some("SSH connection closed by remote".to_string());
                        break;
                    }
                    None => {
                        final_exit_message = Some("SSH channel closed (None)".to_string());
                        break;
                    }
                    _ => {}
                }
            }
            Some(input) = input_rx.recv() => {
                let input_bytes = input.into_bytes();
                match write_interactive_input(&mut channel, &handle, input_bytes.as_slice()).await {
                    InteractiveWriteOutcome::Sent => {}
                    InteractiveWriteOutcome::Dropped(message) => {
                        let should_emit = last_write_error_notice_at
                            .map(|t| t.elapsed() >= std::time::Duration::from_secs(2))
                            .unwrap_or(true);
                        if should_emit {
                            last_write_error_notice_at = Some(std::time::Instant::now());
                            let _ = app.emit(
                                "pty-output",
                                TerminalDataPayload {
                                    id: id.clone(),
                                    data: format!("\r\n\x1b[33m[{message}]\x1b[0m\r\n"),
                                },
                            );
                        }
                    }
                }
            }
            Some((c, r)) = resize_rx.recv() => {
                let _ = channel.window_change(c, r, 0, 0).await;
                // Keep cols/rows up-to-date for reconnect.
                if let Some(cfg) = state.reconnect_configs.lock().await.get_mut(&id) {
                    cfg.cols = c;
                    cfg.rows = r;
                }
            }
            else => {
                final_exit_message = Some("Interactive IO loop ended unexpectedly".to_string());
                break;
            }
        }
    }

    // Connection dropped — remove handles so the reconnect loop detects it.
    state.connections.lock().await.remove(&id);
    state.resize_channels.lock().await.remove(&id);
    state.handles.lock().await.remove(&id);

    // If auto-reconnect is enabled, store the exit message so it can be shown.
    if let Some(ref msg) = final_exit_message {
        if let Some(cfg) = state.reconnect_configs.lock().await.get_mut(&id) {
             cfg.last_error = Some(msg.clone());
        }
    }

    // Do NOT emit pty-exit here when auto-reconnect is enabled; the reconnect loop handles messaging.
    let has_reconnect = state.reconnect_flags.lock().await.contains_key(&id);
    if !has_reconnect {
        let _ = app.emit(
            "pty-exit",
            PtyExitPayload {
                id: id.clone(),
                message: final_exit_message.unwrap_or_else(|| "SSH connection closed".to_string()),
            },
        );
    }
}

/// Try to write interactive input to the SSH channel.
///
/// **Important**: We intentionally do NOT cancel / abort `channel.data()` via a
/// short timeout.  In russh the channel internally manages an SSH window; when
/// the window is full (fast typing / slow consumer) the write future blocks
/// until the server sends a window-adjust.  Cancelling the future mid-flight
/// corrupts the channel and produces "channel closed" errors on subsequent
/// writes — which is exactly the bug the user reported.
///
/// Any write error here is treated as non-fatal for session lifetime.
/// We keep the read loop alive and wait for explicit `Eof`/`Close` from the
/// remote side before exiting, which avoids false session exits.
async fn write_interactive_input(
    channel: &mut russh::Channel<client::Msg>,
    _handle: &SharedSshHandle,
    input_bytes: &[u8],
) -> InteractiveWriteOutcome {
    match channel.data(input_bytes).await {
        // Write succeeded — everything is fine.
        Ok(()) => InteractiveWriteOutcome::Sent,

        // Do not terminate the whole session on a single write failure.
        // If the session is really gone, channel.wait() will soon emit Close/Eof.
        Err(error) => InteractiveWriteOutcome::Dropped(format!(
            "SSH write failed: {error}; input dropped"
        )),
    }
}
