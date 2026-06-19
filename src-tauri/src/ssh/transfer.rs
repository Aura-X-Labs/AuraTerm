//! SFTP / SCP file transfer and remote file-manager commands.
//!
//! This module contains:
//! - Remote and local path helpers (`remote_parent_path`, `join_remote_path`,
//!   `expand_local_path`, `home_dir`, `default_download_dir`,
//!   `unique_local_path`, `file_name_from_remote_path`).
//! - Transfer-progress event emission helpers (`emit_transfer_progress`,
//!   `maybe_emit_transfer_progress`).
//! - SFTP session bootstrap (`open_sftp_session`).
//! - Low-level SCP primitives (`read_scp_line`, `read_scp_ack`, `scp_upload`,
//!   `scp_download_to_path`).
//! - Remote browsing, editing, and transfer Tauri commands exposed to the frontend:
//!   - `ssh_list_remote_dir`
//!   - `ssh_create_remote_dir`
//!   - `ssh_remove_remote_entry`
//!   - `ssh_read_remote_text_file`
//!   - `ssh_write_remote_text_file`
//!   - `ssh_upload_file`
//!   - `ssh_download_file`

use russh::client;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, SeekFrom};

use super::types::{
    RemoteDirectoryListing, RemoteFileEntry, SshTransferDirection, SshTransferMode,
    SshTransferProgressPayload, SshTransferStatus,
};
use super::{
    open_ssh_channel, shell_escape, SshState, SSH_TRANSFER_PROGRESS_EVENT,
    TRANSFER_CHUNK_SIZE, TRANSFER_PROGRESS_EMIT_STEP,
};

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
    home_dir().join("AuraTerm").join("downloads")
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

pub(super) fn file_name_from_remote_path(path: &str) -> Result<String, String> {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .ok_or_else(|| format!("Invalid remote path: {path}"))
}

pub(super) fn emit_transfer_progress(
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

pub(super) fn maybe_emit_transfer_progress(
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
    let mut remote_file = sftp
        .create(path)
        .await
        .map_err(|error| format!("Failed to open remote file for writing: {error}"))?;
    remote_file
        .write_all(content.as_bytes())
        .await
        .map_err(|error| format!("Failed to write remote file: {error}"))?;
    remote_file
        .shutdown()
        .await
        .map_err(|error| format!("Failed to finalize remote file: {error}"))?;
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
    resume: bool,
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
            let resume_offset = if resume {
                sftp.metadata(remote_path.clone()).await.map(|metadata| metadata.len()).unwrap_or(0)
            } else {
                0
            };
            if resume_offset > total_bytes {
                return Err("Remote file is larger than the local upload; disable resume to overwrite it".to_string());
            }
            let mut remote_file = if resume_offset > 0 {
                let mut file = sftp
                    .open_with_flags(remote_path.clone(), OpenFlags::CREATE | OpenFlags::WRITE)
                    .await
                    .map_err(|error| format!("Failed to resume remote file: {error}"))?;
                file.seek(SeekFrom::Start(resume_offset))
                    .await
                    .map_err(|error| format!("Failed to seek remote file: {error}"))?;
                file
            } else {
                sftp.create(remote_path.clone())
                    .await
                    .map_err(|error| format!("Failed to create remote file: {error}"))?
            };
            let mut transferred_bytes = resume_offset;
            let mut last_reported_bytes = 0_u64;

            for chunk in data[resume_offset as usize..].chunks(TRANSFER_CHUNK_SIZE) {
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
            let resume_offset = if resume {
                fs::metadata(&destination_path).await.map(|metadata| metadata.len()).unwrap_or(0)
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
                .open(&destination_path)
                .await
                .map_err(|error| format!("Failed to create local file: {error}"))?;
            let mut buffer = vec![0_u8; TRANSFER_CHUNK_SIZE];
            let mut transferred_bytes = resume_offset;
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
            if resume {
                return Err("Resume is available for SFTP transfers only".to_string());
            }
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
