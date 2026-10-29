//! SFTP / SCP file transfer and remote file-manager commands.
//!
//! This module contains:
//! - Remote and local path helpers (`remote_parent_path`, `join_remote_path`,
//!   `expand_local_path`, `home_dir`, `default_download_dir`,
//!   `unique_local_path`, `file_name_from_remote_path`).
//! - Transfer-progress reporting and cancellation (`TransferReporter`,
//!   `ssh_cancel_transfer`).
//! - SFTP session bootstrap (`open_sftp_session`).
//! - Low-level SCP primitives (`read_scp_line`, `read_scp_ack`, `scp_upload`,
//!   `scp_download_to_path`).
//! - Remote browsing, editing, and transfer Tauri commands exposed to the frontend:
//!   - `ssh_list_remote_dir`
//!   - `ssh_create_remote_dir`
//!   - `ssh_remove_remote_entry`
//!   - `ssh_read_remote_text_file`
//!   - `ssh_write_remote_text_file`
//!   - `ssh_upload_file` (streams from a local path)
//!   - `ssh_download_file`
//!   - `ssh_cancel_transfer`

use russh::client;
use russh_sftp::client::{RawSftpSession, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, SeekFrom};

use super::types::{
    RemoteDirectoryListing, RemoteFileEntry, SshTransferDirection, SshTransferMode,
    SshTransferProgressPayload, SshTransferStatus,
};
use super::{
    open_ssh_channel, shell_escape, ReconnectCancellation, SshState, SSH_TRANSFER_PROGRESS_EVENT,
    TRANSFER_CHUNK_SIZE, TRANSFER_PROGRESS_EMIT_STEP,
};
use std::sync::Arc;

const REMOTE_TEXT_FILE_MAX_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn remote_parent_path(path: &str) -> Option<String> {
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

pub(super) fn join_remote_path(base: &str, name: &str) -> String {
    let clean_name = name.trim_matches('/');
    if base == "/" {
        format!("/{clean_name}")
    } else {
        format!("{}/{}", base.trim_end_matches('/'), clean_name)
    }
}

pub(super) fn expand_local_path(path: Option<&str>) -> PathBuf {
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

pub(super) fn home_dir() -> PathBuf {
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

pub(super) fn default_download_dir() -> PathBuf {
    crate::distribution::files_dir(&home_dir()).join("downloads")
}

pub(super) fn unique_local_path(dir: &Path, file_name: &str) -> PathBuf {
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

/// The last component of a remote (POSIX) path, made safe to create inside
/// the local download directory.
///
/// The remote path is split on `/` only — never parsed as a local path, where
/// on Windows `C:x` is a drive-relative path that `PathBuf::join` would let
/// escape the download directory.
pub(super) fn file_name_from_remote_path(path: &str) -> Result<String, String> {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    if name.is_empty() || name == "." || name == ".." {
        return Err(format!("Invalid remote path: {path}"));
    }
    Ok(sanitize_local_file_name(name, cfg!(windows)))
}

/// Device names Windows reserves in every directory, with or without an
/// extension (`CON`, `con.txt`, `COM1.log`, ...).
const WINDOWS_RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turn a name chosen by the remote side into one this platform stores as a
/// plain file in the target directory.
///
/// Everywhere: control characters and path separators become `_`. On
/// Windows also: `<>:"\|?*` become `_` (a `:` would otherwise write an NTFS
/// alternate data stream), trailing dots and spaces are dropped (Win32 strips
/// them, merging names), and reserved device names get a `_` prefix.
pub(super) fn sanitize_local_file_name(name: &str, windows: bool) -> String {
    let mut safe: String = name
        .chars()
        .map(|c| match c {
            '/' => '_',
            '\\' if windows => '_',
            c if c.is_control() => '_',
            '<' | '>' | ':' | '"' | '|' | '?' | '*' if windows => '_',
            c => c,
        })
        .collect();

    if windows {
        safe.truncate(safe.trim_end_matches(['.', ' ']).len());
        let stem = safe.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
        if WINDOWS_RESERVED_NAMES.contains(&stem.as_str()) {
            safe.insert(0, '_');
        }
    }
    if safe.is_empty() || safe == "." || safe == ".." {
        safe = "_".repeat(safe.len().max(1));
    }
    safe
}

/// Emits one transfer's lifecycle on [`SSH_TRANSFER_PROGRESS_EVENT`]: started,
/// throttled progress, and exactly one of completed / failed / cancelled.
pub(super) struct TransferReporter {
    /// `None` in tests, which drive transfers without a Tauri app.
    app: Option<AppHandle>,
    id: String,
    transfer_id: Option<String>,
    direction: SshTransferDirection,
    mode: SshTransferMode,
    file_name: String,
    remote_path: String,
    local_path: Option<PathBuf>,
    total_bytes: Option<u64>,
    last_reported_bytes: u64,
}

impl TransferReporter {
    #[allow(clippy::too_many_arguments)]
    fn new(
        app: &AppHandle,
        id: &str,
        transfer_id: Option<&str>,
        direction: SshTransferDirection,
        mode: SshTransferMode,
        file_name: &str,
        remote_path: &str,
        local_path: Option<&Path>,
        total_bytes: Option<u64>,
    ) -> Self {
        Self {
            app: Some(app.clone()),
            id: id.to_string(),
            transfer_id: transfer_id.map(str::to_string),
            direction,
            mode,
            file_name: file_name.to_string(),
            remote_path: remote_path.to_string(),
            local_path: local_path.map(Path::to_path_buf),
            total_bytes,
            last_reported_bytes: 0,
        }
    }

    #[cfg(test)]
    fn silent(direction: SshTransferDirection, mode: SshTransferMode) -> Self {
        Self {
            app: None,
            id: String::new(),
            transfer_id: None,
            direction,
            mode,
            file_name: String::new(),
            remote_path: String::new(),
            local_path: None,
            total_bytes: None,
            last_reported_bytes: 0,
        }
    }

    fn emit(&self, status: SshTransferStatus, transferred_bytes: u64, total_bytes: Option<u64>, message: Option<String>) {
        let Some(app) = &self.app else { return };
        let _ = app.emit(
            SSH_TRANSFER_PROGRESS_EVENT,
            SshTransferProgressPayload {
                id: self.id.clone(),
                transfer_id: self.transfer_id.clone(),
                direction: self.direction,
                status,
                mode: self.mode,
                file_name: self.file_name.clone(),
                remote_path: self.remote_path.clone(),
                local_path: self.local_path.as_ref().map(|value| value.to_string_lossy().into_owned()),
                transferred_bytes,
                total_bytes,
                message,
            },
        );
    }

    fn started(&self) {
        self.emit(SshTransferStatus::Started, 0, self.total_bytes, None);
    }

    /// Report progress at most every [`TRANSFER_PROGRESS_EMIT_STEP`] bytes.
    fn progress(&mut self, transferred_bytes: u64) {
        if transferred_bytes == 0
            || self.total_bytes.is_some_and(|total| transferred_bytes >= total)
            || transferred_bytes.saturating_sub(self.last_reported_bytes) < TRANSFER_PROGRESS_EMIT_STEP
        {
            return;
        }
        self.last_reported_bytes = transferred_bytes;
        self.emit(SshTransferStatus::Progress, transferred_bytes, self.total_bytes, None);
    }

    fn completed(&self, transferred_bytes: u64) {
        let total = Some(self.total_bytes.unwrap_or(transferred_bytes));
        self.emit(SshTransferStatus::Completed, transferred_bytes, total, None);
    }

    fn ended(&self, error: &str, cancelled: bool) {
        let status = if cancelled { SshTransferStatus::Cancelled } else { SshTransferStatus::Failed };
        self.emit(status, self.last_reported_bytes, self.total_bytes, Some(error.to_string()));
    }
}

/// Error text for a transfer stopped by [`ssh_cancel_transfer`].
pub(super) const TRANSFER_CANCELLED: &str = "Transfer cancelled";

/// A running transfer's entry in the cancellation registry, removed on drop.
struct TransferRegistration {
    state: SshState,
    transfer_id: Option<String>,
    token: Arc<ReconnectCancellation>,
}

impl TransferRegistration {
    fn new(state: &SshState, transfer_id: Option<&str>) -> Self {
        let token = Arc::new(ReconnectCancellation::default());
        if let Some(transfer_id) = transfer_id {
            if let Ok(mut transfers) = state.transfers.lock() {
                transfers.insert(transfer_id.to_string(), token.clone());
            }
        }
        Self { state: state.clone(), transfer_id: transfer_id.map(str::to_string), token }
    }

    /// Run `transfer` until it finishes or the transfer is cancelled; a
    /// cancel drops it mid-flight, closing its SFTP handle or SCP channel.
    async fn run<T>(&self, transfer: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
        tokio::select! {
            biased;
            _ = self.token.cancelled() => Err(TRANSFER_CANCELLED.to_string()),
            result = transfer => result,
        }
    }

    fn cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

impl Drop for TransferRegistration {
    fn drop(&mut self) {
        if let Some(transfer_id) = &self.transfer_id {
            if let Ok(mut transfers) = self.state.transfers.lock() {
                transfers.remove(transfer_id);
            }
        }
    }
}

/// Stop a running upload or download started with this `transfer_id`. The
/// transfer's own command then fails with [`TRANSFER_CANCELLED`] and emits a
/// `cancelled` status. Unknown ids (already finished) are not an error.
#[tauri::command]
pub async fn ssh_cancel_transfer(state: State<'_, SshState>, transfer_id: String) -> Result<(), String> {
    let token = state
        .transfers
        .lock()
        .map_err(|_| "Transfer registry unavailable".to_string())?
        .get(&transfer_id)
        .cloned();
    if let Some(token) = token {
        token.cancel();
    }
    Ok(())
}

pub(super) async fn open_sftp_session(state: &SshState, id: &str) -> Result<SftpSession, String> {
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

/// Send `total_bytes` from `local_file` with `scp -t`.
async fn scp_upload(
    state: &SshState,
    id: &str,
    remote_dir: &str,
    file_name: &str,
    local_file: &mut fs::File,
    total_bytes: u64,
    reporter: &mut TransferReporter,
) -> Result<(), String> {
    let channel = open_ssh_channel(state, id).await?;
    channel
        .exec(true, format!("scp -t {}", shell_escape(remote_dir)))
        .await
        .map_err(|error| format!("Failed to start SCP upload: {error}"))?;

    let mut stream = BufReader::new(channel.into_stream());
    read_scp_ack(&mut stream).await?;

    let header = format!("C0644 {} {}\n", total_bytes, file_name);
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

    // SCP announced the size up front, so send exactly that many bytes.
    let mut transferred_bytes = 0_u64;
    let mut buffer = vec![0_u8; TRANSFER_CHUNK_SIZE];
    while transferred_bytes < total_bytes {
        let want = (total_bytes - transferred_bytes).min(buffer.len() as u64) as usize;
        let read = local_file
            .read(&mut buffer[..want])
            .await
            .map_err(|error| format!("Failed to read local file: {error}"))?;
        if read == 0 {
            return Err("The local file got shorter while it was being uploaded".to_string());
        }
        stream
            .get_mut()
            .write_all(&buffer[..read])
            .await
            .map_err(|error| format!("Failed to write SCP data: {error}"))?;
        transferred_bytes += read as u64;
        reporter.progress(transferred_bytes);
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
    state: &SshState,
    id: &str,
    remote_path: &str,
    destination: &Path,
    total_bytes: Option<u64>,
    reporter: &mut TransferReporter,
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
    reporter.total_bytes = total_bytes.or(Some(size));

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
        reporter.progress(transferred_bytes);
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
pub async fn ssh_read_remote_text_file(
    state: State<'_, SshState>,
    id: String,
    path: String,
) -> Result<String, String> {
    let sftp = open_sftp_session(state.inner(), &id).await?;
    let metadata = sftp
        .metadata(path.clone())
        .await
        .map_err(|error| format!("Failed to inspect remote file: {error}"))?;
    if metadata.len() > REMOTE_TEXT_FILE_MAX_BYTES as u64 {
        return Err(format!(
            "Remote quick edit supports files up to {} MiB",
            REMOTE_TEXT_FILE_MAX_BYTES / 1024 / 1024
        ));
    }

    let remote_file = sftp
        .open(path)
        .await
        .map_err(|error| format!("Failed to open remote file: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    remote_file
        .take((REMOTE_TEXT_FILE_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("Failed to read remote file: {error}"))?;
    let _ = sftp.close().await;

    if bytes.len() > REMOTE_TEXT_FILE_MAX_BYTES {
        return Err("Remote file grew beyond the quick-edit size limit while reading".to_string());
    }
    if bytes.contains(&0) {
        return Err("Binary files cannot be opened in remote quick edit".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "Remote quick edit only supports UTF-8 text".to_string())
}

#[tauri::command]
pub async fn ssh_write_remote_text_file(
    state: State<'_, SshState>,
    id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    if content.len() > REMOTE_TEXT_FILE_MAX_BYTES {
        return Err(format!(
            "Remote quick edit supports files up to {} MiB",
            REMOTE_TEXT_FILE_MAX_BYTES / 1024 / 1024
        ));
    }

    let sftp = open_sftp_session(state.inner(), &id).await?;
    let state = state.inner().clone();
    let result = write_remote_file_atomically(
        &sftp,
        || async move { open_raw_sftp_session(&state, &id).await },
        &path,
        content.as_bytes(),
    )
    .await;
    let _ = sftp.close().await;
    result
}

/// SFTP extension that renames over an existing file atomically (POSIX
/// `rename(2)`); plain SFTPv3 RENAME refuses to replace an existing file.
const POSIX_RENAME: &str = "posix-rename@openssh.com";

/// A second, low-level SFTP session for extension requests the high-level
/// client does not expose.
async fn open_raw_sftp_session(state: &SshState, id: &str) -> Result<RawSftpSession, String> {
    let channel = open_ssh_channel(state, id).await?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| format!("Failed to start SFTP subsystem: {error}"))?;
    Ok(RawSftpSession::new(channel.into_stream()))
}

/// Replace `path` with `content` so that the file is never seen truncated or
/// half-written: write a temp file beside it, give it the original's mode
/// (and owner, where allowed), then rename it over the original.
///
/// Symlinks are followed first, so the link stays a link. Where the atomic
/// route would change the file — the directory is not writable, the owner
/// cannot be kept, or the server cannot rename over a file — it falls back
/// to writing in place, as before.
async fn write_remote_file_atomically<F, Fut>(
    sftp: &SftpSession,
    open_raw: F,
    path: &str,
    content: &[u8],
) -> Result<(), String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<RawSftpSession, String>>,
{
    let target = sftp.canonicalize(path.to_string()).await.unwrap_or_else(|_| path.to_string());
    let existing = sftp.metadata(target.clone()).await.ok();
    let directory = remote_parent_path(&target).unwrap_or_else(|| ".".to_string());
    let name = target.rsplit('/').next().unwrap_or("file");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let temp = join_remote_path(&directory, &format!(".{name}.auraterm-{}.tmp", &suffix[..12]));

    match write_temp_copy(sftp, &temp, existing.as_ref(), content).await {
        Ok(true) => {}
        // Could not create a faithful copy beside the file: keep the old,
        // non-atomic behaviour rather than failing the save.
        Ok(false) | Err(_) => {
            let _ = sftp.remove_file(temp.clone()).await;
            return write_remote_file_in_place(sftp, &target, content).await;
        }
    }

    let renamed = match posix_rename(open_raw().await, &temp, &target).await {
        Ok(true) => Ok(()),
        // No atomic replace on this server.
        Ok(false) => {
            let _ = sftp.remove_file(temp.clone()).await;
            return write_remote_file_in_place(sftp, &target, content).await;
        }
        Err(error) => Err(error),
    };
    if renamed.is_err() {
        let _ = sftp.remove_file(temp).await;
    }
    renamed
}

/// Create `temp` exclusively and fill it. `Ok(false)` when it cannot carry
/// the original's owner, so replacing the original would hand it to us.
async fn write_temp_copy(
    sftp: &SftpSession,
    temp: &str,
    existing: Option<&russh_sftp::client::fs::Metadata>,
    content: &[u8],
) -> Result<bool, String> {
    let mut attributes = FileAttributes::empty();
    // Start private; the original's mode is applied once the content is in.
    attributes.permissions = Some(0o600);
    let mut file = sftp
        .open_with_flags_and_attributes(
            temp.to_string(),
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            attributes,
        )
        .await
        .map_err(|error| format!("Failed to create temporary file: {error}"))?;
    file.write_all(content)
        .await
        .map_err(|error| format!("Failed to write remote file: {error}"))?;
    // Best effort: only servers with fsync@openssh.com support it.
    let _ = file.sync_all().await;
    file.shutdown()
        .await
        .map_err(|error| format!("Failed to finalize remote file: {error}"))?;

    let Some(existing) = existing else { return Ok(true) };
    if let Some(mode) = existing.permissions {
        let mut attributes = FileAttributes::empty();
        attributes.permissions = Some(mode & 0o7777);
        sftp.set_metadata(temp.to_string(), attributes)
            .await
            .map_err(|error| format!("Failed to copy file permissions: {error}"))?;
    }
    if let (Some(uid), Some(gid)) = (existing.uid, existing.gid) {
        let written = sftp
            .metadata(temp.to_string())
            .await
            .map_err(|error| format!("Failed to inspect temporary file: {error}"))?;
        if written.uid != Some(uid) || written.gid != Some(gid) {
            let mut attributes = FileAttributes::empty();
            attributes.uid = Some(uid);
            attributes.gid = Some(gid);
            if sftp.set_metadata(temp.to_string(), attributes).await.is_err() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// `Ok(false)` when the server does not offer the extension.
async fn posix_rename(
    raw: Result<RawSftpSession, String>,
    from: &str,
    to: &str,
) -> Result<bool, String> {
    let raw = raw?;
    let version = raw
        .init()
        .await
        .map_err(|error| format!("Failed to initialize SFTP session: {error}"))?;
    if version.extensions.get(POSIX_RENAME).map(String::as_str) != Some("1") {
        return Ok(false);
    }
    let mut data = Vec::with_capacity(8 + from.len() + to.len());
    for value in [from, to] {
        data.extend_from_slice(&(value.len() as u32).to_be_bytes());
        data.extend_from_slice(value.as_bytes());
    }
    // Dropping `raw` afterwards ends its session and channel.
    match raw.extended(POSIX_RENAME, data).await {
        Ok(Packet::Status(status)) if status.status_code == StatusCode::Ok => Ok(true),
        Ok(Packet::Status(status)) => Err(format!("Failed to replace remote file: {}", status.error_message)),
        Ok(_) => Err("Failed to replace remote file: unexpected SFTP reply".to_string()),
        Err(error) => Err(format!("Failed to replace remote file: {error}")),
    }
}

async fn write_remote_file_in_place(sftp: &SftpSession, path: &str, content: &[u8]) -> Result<(), String> {
    let mut remote_file = sftp
        .create(path.to_string())
        .await
        .map_err(|error| format!("Failed to open remote file for writing: {error}"))?;
    remote_file
        .write_all(content)
        .await
        .map_err(|error| format!("Failed to write remote file: {error}"))?;
    remote_file
        .shutdown()
        .await
        .map_err(|error| format!("Failed to finalize remote file: {error}"))?;
    Ok(())
}

/// Upload one local file into `remote_dir`, streaming it from disk.
///
/// The webview passes the path the user picked (file dialog or drop) rather
/// than the file's bytes: a JSON number array of a large file exhausted the
/// WebView's memory, and could not be cancelled. Pass a `transfer_id` to make
/// the upload cancellable with [`ssh_cancel_transfer`].
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ssh_upload_file(
    app: AppHandle,
    state: State<'_, SshState>,
    id: String,
    remote_dir: String,
    local_path: String,
    transfer_id: Option<String>,
    mode: SshTransferMode,
    resume: bool,
) -> Result<(), String> {
    let local_path = PathBuf::from(local_path);
    let file_name = local_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .ok_or_else(|| "Invalid file name".to_string())?;
    let remote_path = join_remote_path(&remote_dir, &file_name);

    let registration = TransferRegistration::new(state.inner(), transfer_id.as_deref());
    let mut reporter = TransferReporter::new(
        &app,
        &id,
        transfer_id.as_deref(),
        SshTransferDirection::Upload,
        mode,
        &file_name,
        &remote_path,
        Some(&local_path),
        None,
    );

    let result = registration
        .run(upload_local_file(
            state.inner(),
            &id,
            &remote_dir,
            &remote_path,
            &file_name,
            &local_path,
            mode,
            resume,
            &mut reporter,
        ))
        .await;
    match result {
        Ok(total_bytes) => {
            reporter.completed(total_bytes);
            Ok(())
        }
        Err(error) => {
            reporter.ended(&error, registration.cancelled());
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn upload_local_file(
    state: &SshState,
    id: &str,
    remote_dir: &str,
    remote_path: &str,
    file_name: &str,
    local_path: &Path,
    mode: SshTransferMode,
    resume: bool,
    reporter: &mut TransferReporter,
) -> Result<u64, String> {
    let mut local_file = fs::File::open(local_path)
        .await
        .map_err(|error| format!("Failed to open {}: {error}", local_path.display()))?;
    let metadata = local_file
        .metadata()
        .await
        .map_err(|error| format!("Failed to inspect {}: {error}", local_path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", local_path.display()));
    }
    let total_bytes = metadata.len();
    reporter.total_bytes = Some(total_bytes);
    reporter.started();

    match mode {
        SshTransferMode::Sftp => {
            let sftp = open_sftp_session(state, id).await?;
            let resume_offset = if resume {
                sftp.metadata(remote_path.to_string()).await.map(|metadata| metadata.len()).unwrap_or(0)
            } else {
                0
            };
            if resume_offset > total_bytes {
                return Err("Remote file is larger than the local upload; disable resume to overwrite it".to_string());
            }
            let mut remote_file = if resume_offset > 0 {
                let mut file = sftp
                    .open_with_flags(remote_path.to_string(), OpenFlags::CREATE | OpenFlags::WRITE)
                    .await
                    .map_err(|error| format!("Failed to resume remote file: {error}"))?;
                file.seek(SeekFrom::Start(resume_offset))
                    .await
                    .map_err(|error| format!("Failed to seek remote file: {error}"))?;
                local_file
                    .seek(SeekFrom::Start(resume_offset))
                    .await
                    .map_err(|error| format!("Failed to seek local file: {error}"))?;
                file
            } else {
                sftp.create(remote_path.to_string())
                    .await
                    .map_err(|error| format!("Failed to create remote file: {error}"))?
            };

            let mut transferred_bytes = resume_offset;
            let mut buffer = vec![0_u8; TRANSFER_CHUNK_SIZE];
            loop {
                let read = local_file
                    .read(&mut buffer)
                    .await
                    .map_err(|error| format!("Failed to read local file: {error}"))?;
                if read == 0 {
                    break;
                }
                remote_file
                    .write_all(&buffer[..read])
                    .await
                    .map_err(|error| format!("Failed to upload file over SFTP: {error}"))?;
                transferred_bytes += read as u64;
                reporter.progress(transferred_bytes);
            }
            remote_file
                .shutdown()
                .await
                .map_err(|error| format!("Failed to finalize SFTP upload: {error}"))?;
            let _ = sftp.close().await;
            Ok(transferred_bytes)
        }
        SshTransferMode::Scp => {
            scp_upload(state, id, remote_dir, file_name, &mut local_file, total_bytes, reporter).await?;
            Ok(total_bytes)
        }
    }
}

/// Download one remote file into `local_dir` (the default download folder when
/// empty). Pass a `transfer_id` to make it cancellable with
/// [`ssh_cancel_transfer`].
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ssh_download_file(
    app: AppHandle,
    state: State<'_, SshState>,
    id: String,
    remote_path: String,
    local_dir: Option<String>,
    expected_size: Option<u64>,
    transfer_id: Option<String>,
    mode: SshTransferMode,
    resume: bool,
) -> Result<String, String> {
    let file_name = file_name_from_remote_path(&remote_path)?;
    let destination_dir = expand_local_path(local_dir.as_deref());
    fs::create_dir_all(&destination_dir)
        .await
        .map_err(|error| format!("Failed to create download directory: {error}"))?;

    let destination_path = if resume {
        destination_dir.join(&file_name)
    } else {
        unique_local_path(&destination_dir, &file_name)
    };
    let total_bytes = expected_size.filter(|value| *value > 0);

    let registration = TransferRegistration::new(state.inner(), transfer_id.as_deref());
    let mut reporter = TransferReporter::new(
        &app,
        &id,
        transfer_id.as_deref(),
        SshTransferDirection::Download,
        mode,
        &file_name,
        &remote_path,
        Some(&destination_path),
        total_bytes,
    );
    reporter.started();

    let result = registration
        .run(download_remote_file(
            state.inner(),
            &id,
            &remote_path,
            &destination_path,
            total_bytes,
            mode,
            resume,
            &mut reporter,
        ))
        .await;
    match result {
        Ok(transferred_bytes) => {
            reporter.completed(transferred_bytes);
            Ok(destination_path.to_string_lossy().into_owned())
        }
        Err(error) => {
            reporter.ended(&error, registration.cancelled());
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_remote_file(
    state: &SshState,
    id: &str,
    remote_path: &str,
    destination_path: &Path,
    total_bytes: Option<u64>,
    mode: SshTransferMode,
    resume: bool,
    reporter: &mut TransferReporter,
) -> Result<u64, String> {
    match mode {
        SshTransferMode::Sftp => {
            let sftp = open_sftp_session(state, id).await?;
            let mut remote_file = sftp
                .open(remote_path.to_string())
                .await
                .map_err(|error| format!("Failed to open remote file: {error}"))?;
            let resume_offset = if resume {
                fs::metadata(destination_path).await.map(|metadata| metadata.len()).unwrap_or(0)
            } else {
                0
            };
            if total_bytes.is_some_and(|total| resume_offset > total) {
                return Err("Local file is larger than the remote download; disable resume to create a new copy".to_string());
            }
            if resume_offset > 0 {
                remote_file.seek(SeekFrom::Start(resume_offset))
                    .await
                    .map_err(|error| format!("Failed to seek remote file: {error}"))?;
            }
            let mut local_file = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(resume_offset > 0)
                .truncate(resume_offset == 0)
                .open(destination_path)
                .await
                .map_err(|error| format!("Failed to create local file: {error}"))?;
            let mut buffer = vec![0_u8; TRANSFER_CHUNK_SIZE];
            let mut transferred_bytes = resume_offset;

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
                reporter.progress(transferred_bytes);
            }
            remote_file
                .shutdown()
                .await
                .map_err(|error| format!("Failed to finalize SFTP download: {error}"))?;
            let _ = sftp.close().await;
            Ok(transferred_bytes)
        }
        SshTransferMode::Scp => {
            if resume {
                return Err("Resume is available for SFTP transfers only".to_string());
            }
            scp_download_to_path(state, id, remote_path, destination_path, total_bytes, reporter).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An SFTP stream served by the local OpenSSH `sftp-server`, if installed.
    #[cfg(unix)]
    fn local_sftp_stream() -> Option<tokio::net::UnixStream> {
        use std::os::fd::OwnedFd;
        use std::process::{Command, Stdio};

        let server = [
            "/usr/lib/openssh/sftp-server",
            "/usr/libexec/sftp-server",
            "/usr/libexec/openssh/sftp-server",
            "/usr/lib/ssh/sftp-server",
        ]
        .into_iter()
        .find(|path| Path::new(path).exists())?;
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().ok()?;
        let theirs_out = theirs.try_clone().ok()?;
        // It exits on EOF once our end is dropped.
        Command::new(server)
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::from(OwnedFd::from(theirs_out)))
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        ours.set_nonblocking(true).ok()?;
        tokio::net::UnixStream::from_std(ours).ok()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quick_edit_save_replaces_the_file_atomically() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let Some(stream) = local_sftp_stream() else {
            eprintln!("skipped: no local sftp-server");
            return;
        };
        let sftp = SftpSession::new(stream).await.unwrap();
        let dir = std::env::temp_dir().join(format!("auraterm-quick-edit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("app.conf");
        let link = dir.join("current.conf");
        std::fs::write(&file, "a much longer original body that must not survive\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let original_inode = std::fs::metadata(&file).unwrap().ino();

        write_remote_file_atomically(
            &sftp,
            || async { local_sftp_stream().map(RawSftpSession::new).ok_or_else(|| "no sftp-server".to_string()) },
            link.to_str().unwrap(),
            b"short\n",
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "short\n");
        let metadata = std::fs::metadata(&file).unwrap();
        assert_ne!(metadata.ino(), original_inode, "replaced by rename, not rewritten in place");
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o640, "mode kept");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".auraterm-"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_cancelled_transfer_stops_and_leaves_the_registry() {
        let state = SshState::default();
        let registration = TransferRegistration::new(&state, Some("t1"));
        let token = state.transfers.lock().unwrap().get("t1").cloned().expect("registered");

        let canceller = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            token.cancel();
        });
        // A transfer that would otherwise never finish (a stalled server).
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            registration.run(std::future::pending::<Result<(), String>>()),
        )
        .await
        .expect("cancel must stop the transfer");
        assert_eq!(result, Err(TRANSFER_CANCELLED.to_string()));
        assert!(registration.cancelled());
        canceller.await.unwrap();

        drop(registration);
        assert!(state.transfers.lock().unwrap().is_empty());
    }

    /// A minimal `scp -t` sink: acks the start, the header and the data, and
    /// keeps what it received.
    #[derive(Clone, Default)]
    struct ScpSink {
        received: Arc<std::sync::Mutex<Vec<u8>>>,
        header: Arc<std::sync::Mutex<Option<String>>>,
    }

    impl russh::server::Handler for ScpSink {
        type Error = russh::Error;

        async fn auth_none(&mut self, _: &str) -> Result<russh::server::Auth, Self::Error> {
            Ok(russh::server::Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            _: russh::Channel<russh::server::Msg>,
            reply: russh::server::ChannelOpenHandle,
            _: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            _: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            session.data(channel, vec![0u8])?;
            Ok(())
        }

        async fn data(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let mut header = self.header.lock().unwrap();
            let mut received = self.received.lock().unwrap();
            received.extend_from_slice(data);
            if header.is_none() {
                if let Some(end) = received.iter().position(|byte| *byte == b'\n') {
                    *header = Some(String::from_utf8_lossy(&received[..end]).into_owned());
                    received.drain(..=end);
                    session.data(channel, vec![0u8])?;
                }
            }
            if let Some(line) = header.as_ref() {
                let size: usize = line.split(' ').nth(1).unwrap().parse().unwrap();
                if received.len() == size + 1 {
                    session.data(channel, vec![0u8])?;
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn scp_upload_streams_the_file_from_disk() {
        let sink = ScpSink::default();
        let (address, server) = super::super::test_support::spawn_server(sink.clone()).await;
        let mut client = super::super::test_support::connect(address).await;
        assert!(client.authenticate_none("test").await.unwrap().success());
        let state = SshState::default();
        state.upsert_session("tab", |s| s.handle = Some(Arc::new(client))).await;

        let dir = std::env::temp_dir().join(format!("auraterm-upload-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.bin");
        // Several chunks plus a remainder.
        let content: Vec<u8> = (0..(TRANSFER_CHUNK_SIZE * 3 + 123)).map(|index| (index % 251) as u8).collect();
        std::fs::write(&path, &content).unwrap();

        let mut reporter = TransferReporter::silent(SshTransferDirection::Upload, SshTransferMode::Scp);
        let sent = upload_local_file(
            &state,
            "tab",
            "/tmp",
            "/tmp/payload.bin",
            "payload.bin",
            &path,
            SshTransferMode::Scp,
            false,
            &mut reporter,
        )
        .await
        .unwrap();

        assert_eq!(sent, content.len() as u64);
        assert_eq!(sink.header.lock().unwrap().as_deref(), Some(format!("C0644 {} payload.bin", content.len()).as_str()));
        let received = sink.received.lock().unwrap().clone();
        assert_eq!(&received[..content.len()], &content[..]);
        assert_eq!(received[content.len()..], [0u8], "terminated with the SCP end marker");
        assert_eq!(reporter.total_bytes, Some(content.len() as u64));

        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_names_are_split_on_slashes_only() {
        assert_eq!(file_name_from_remote_path("/var/log/syslog").unwrap(), "syslog");
        assert_eq!(file_name_from_remote_path("/var/log/").unwrap(), "log");
        assert_eq!(file_name_from_remote_path("relative").unwrap(), "relative");
        for bad in ["", "/", "/tmp/..", "/tmp/.", "//"] {
            assert!(file_name_from_remote_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn windows_names_cannot_escape_or_hit_special_files() {
        let windows = |name: &str| sanitize_local_file_name(name, true);
        // `:` would write an NTFS alternate data stream; `C:x` is drive-relative.
        assert_eq!(windows("notes.txt:secret"), "notes.txt_secret");
        assert_eq!(windows("C:x"), "C_x");
        assert_eq!(windows("a\\..\\b"), "a_.._b");
        assert_eq!(windows("what?<>|*\"q\""), "what______q_");
        // Reserved device names, with or without an extension, any case.
        assert_eq!(windows("CON"), "_CON");
        assert_eq!(windows("con.txt"), "_con.txt");
        assert_eq!(windows("Com1.log"), "_Com1.log");
        assert_eq!(windows("lpt9"), "_lpt9");
        assert_eq!(windows("console"), "console");
        assert_eq!(windows("COM10"), "COM10");
        // Win32 drops trailing dots and spaces.
        assert_eq!(windows("report. . "), "report");
        assert_eq!(windows("..."), "_");
        assert_eq!(windows("tab\there"), "tab_here");

        let joined = std::path::Path::new("downloads").join(windows("C:x"));
        assert_eq!(joined, std::path::Path::new("downloads").join("C_x"));
    }

    #[test]
    fn other_platforms_keep_legal_characters() {
        let unix = |name: &str| sanitize_local_file_name(name, false);
        assert_eq!(unix("a:b?*"), "a:b?*");
        assert_eq!(unix("CON"), "CON");
        assert_eq!(unix("trailing. "), "trailing. ");
        assert_eq!(unix("bell\u{7}"), "bell_");
    }
}
