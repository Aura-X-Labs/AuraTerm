//! SSH `known_hosts` store, host-key fingerprint tracking, and the
//! `russh::client::Handler` implementation that records / validates fingerprints.
//!
//! Extracted from the original monolithic `ssh.rs` to keep fingerprint / trust
//! logic self-contained. This module owns:
//!
//! - [`KnownHostsStore`] on-disk JSON structure
//! - [`ClientHandler`] russh handler used during the TCP/SSH handshake
//! - fingerprint scope helpers ([`known_host_scope`], [`parse_known_host_scope`])
//! - audit log for host-key overrides
//! - Tauri commands that manage the trusted hosts list from the frontend
//! - [`prompt_for_host_key_override`] which asks the user whether to trust a
//!   newly-observed fingerprint when a mismatch is detected.

use russh::{client, keys::HashAlg};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use super::types::{SshHostKeyMismatchPromptPayload, TrustedSshHostKeyEntry};
use super::{SshState, SECURITY_AUDIT_LOG_FILE, SSH_KNOWN_HOSTS_FILE};

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct KnownHostsStore {
    #[serde(default)]
    pub(super) hosts: HashMap<String, String>,
}

/// russh handler used during the initial SSH handshake.
///
/// Records the server fingerprint into `observed_fingerprint` and, when an
/// `expected_fingerprint` is supplied, only accepts the server key if it
/// matches bit-for-bit. When `expected_fingerprint` is `None`, the handshake
/// is accepted unconditionally — the caller is responsible for comparing
/// the observed fingerprint against the trusted store afterwards.
pub(super) struct ClientHandler {
    expected_fingerprint: Option<String>,
    observed_fingerprint: Arc<std::sync::Mutex<Option<String>>>,
    /// Shared per-session map consulted when the server opens a `forwarded-tcpip`
    /// channel for a remote (`-R`) port forward. Empty for connections without
    /// any active remote forward.
    remote_forwards: super::forwarding::RemoteForwardRegistry,
    agent_forwarding: bool,
}

impl ClientHandler {
    pub(super) fn new(
        expected_fingerprint: Option<String>,
        observed_fingerprint: Arc<std::sync::Mutex<Option<String>>>,
        remote_forwards: super::forwarding::RemoteForwardRegistry,
        agent_forwarding: bool,
    ) -> Self {
        Self {
            expected_fingerprint,
            observed_fingerprint,
            remote_forwards,
            agent_forwarding,
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    fn check_server_key(
        self: &mut Self,
        server_public_key: &russh::keys::PublicKey,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        let observed = self.observed_fingerprint.clone();
        let expected = self.expected_fingerprint.clone();
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();

        async move {
            if let Ok(mut guard) = observed.lock() {
                *guard = Some(fingerprint.clone());
            }

            if let Some(expected_fingerprint) = expected {
                return Ok(expected_fingerprint == fingerprint);
            }

            Ok(true)
        }
    }

    /// Inbound connection for a remote (`-R`) forward: look up the local target
    /// by the server-side bind port and splice the channel to a fresh local
    /// TCP connection. Unknown ports are dropped (channel closed).
    fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut russh::client::Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let _ = connected_address;
        let target = self
            .remote_forwards
            .lock()
            .ok()
            .and_then(|map| map.get(&connected_port).cloned());

        async move {
            if let Some(target) = target {
                tokio::spawn(super::forwarding::pump_channel_to_local(
                    channel,
                    target.host,
                    target.port,
                ));
            }
            Ok(())
        }
    }

    fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _session: &mut russh::client::Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let enabled = self.agent_forwarding;
        async move {
            if enabled {
                tokio::spawn(async move {
                    if let Ok(mut agent) = super::connect_local_agent_stream().await {
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut agent).await;
                    }
                });
            }
            Ok(())
        }
    }
}

pub(super) fn known_host_scope(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

pub(super) fn parse_known_host_scope(scope: &str) -> Option<(String, u16)> {
    let (host, port_text) = scope.rsplit_once(':')?;
    let port = port_text.parse::<u16>().ok()?;
    Some((host.to_string(), port))
}

pub(super) fn summarize_fingerprint(fingerprint: &str) -> String {
    const PREFIX_LEN: usize = 16;
    const SUFFIX_LEN: usize = 12;

    if fingerprint.len() <= PREFIX_LEN + SUFFIX_LEN + 3 {
        return fingerprint.to_string();
    }

    format!(
        "{}...{}",
        &fingerprint[..PREFIX_LEN],
        &fingerprint[fingerprint.len() - SUFFIX_LEN..]
    )
}

pub(super) async fn append_host_key_override_audit_log(
    app: &AppHandle,
    host: &str,
    port: u16,
    old_fingerprint: &str,
    new_fingerprint: &str,
) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve app config directory: {error}"))?;
    fs::create_dir_all(&config_dir)
        .await
        .map_err(|error| format!("Failed to create config directory: {error}"))?;

    let path = config_dir.join(SECURITY_AUDIT_LOG_FILE);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let timestamp_secs = now.as_secs();
    let timestamp_millis = now.subsec_millis();

    let line = format!(
        "ts_unix={timestamp_secs}.{timestamp_millis:03} event=ssh_host_key_override host={host} port={port} old_summary=\"{}\" new_summary=\"{}\" old=\"{}\" new=\"{}\"\n",
        summarize_fingerprint(old_fingerprint),
        summarize_fingerprint(new_fingerprint),
        old_fingerprint,
        new_fingerprint,
    );

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| format!("Failed to open security audit log: {error}"))?;
    file.write_all(line.as_bytes())
        .await
        .map_err(|error| format!("Failed to append security audit log: {error}"))?;
    file.flush()
        .await
        .map_err(|error| format!("Failed to flush security audit log: {error}"))?;

    Ok(())
}

pub(super) fn known_hosts_path(app: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve app config directory: {error}"))?;
    Ok(config_dir.join(SSH_KNOWN_HOSTS_FILE))
}

pub(super) async fn load_known_hosts(app: &AppHandle) -> Result<KnownHostsStore, String> {
    let path = known_hosts_path(app)?;
    if !path.exists() {
        return Ok(KnownHostsStore::default());
    }

    let content = fs::read_to_string(&path)
        .await
        .map_err(|error| format!("Failed to read SSH known hosts: {error}"))?;

    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse SSH known hosts: {error}"))
}

pub(super) async fn save_known_hosts(
    app: &AppHandle,
    known_hosts: &KnownHostsStore,
) -> Result<(), String> {
    let path = known_hosts_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Failed to create config directory: {error}"))?;
    }

    let content = serde_json::to_string_pretty(known_hosts)
        .map_err(|error| format!("Failed to serialize SSH known hosts: {error}"))?;

    fs::write(&path, content)
        .await
        .map_err(|error| format!("Failed to write SSH known hosts: {error}"))
}

#[tauri::command]
pub async fn ssh_list_known_hosts(app: AppHandle) -> Result<Vec<TrustedSshHostKeyEntry>, String> {
    let known_hosts = load_known_hosts(&app).await?;
    let mut entries = Vec::new();

    for (scope, fingerprint) in known_hosts.hosts {
        if let Some((host, port)) = parse_known_host_scope(&scope) {
            entries.push(TrustedSshHostKeyEntry {
                host,
                port,
                fingerprint_summary: summarize_fingerprint(&fingerprint),
                fingerprint,
            });
        }
    }

    entries.sort_by(|left, right| {
        left.host
            .cmp(&right.host)
            .then_with(|| left.port.cmp(&right.port))
    });

    Ok(entries)
}

#[tauri::command]
pub async fn ssh_delete_known_host(app: AppHandle, host: String, port: u16) -> Result<(), String> {
    let mut known_hosts = load_known_hosts(&app).await?;
    known_hosts
        .hosts
        .remove(&known_host_scope(host.trim(), port));
    save_known_hosts(&app, &known_hosts).await
}

#[tauri::command]
pub async fn ssh_reset_known_hosts(app: AppHandle) -> Result<(), String> {
    let mut known_hosts = load_known_hosts(&app).await?;
    known_hosts.hosts.clear();
    save_known_hosts(&app, &known_hosts).await
}

pub(super) fn observed_fingerprint_value(
    observed: &Arc<std::sync::Mutex<Option<String>>>,
) -> Option<String> {
    observed.lock().ok().and_then(|guard| guard.clone())
}

/// Perform the TCP + SSH-handshake step against `host:port`, pinning the
/// server-key fingerprint to `expected_fingerprint` when supplied.
///
/// On error the observed fingerprint (if any) is returned alongside the error
/// string so that the caller can emit a host-key-mismatch prompt.
pub(super) async fn connect_with_expected_fingerprint(
    host: String,
    port: u16,
    expected_fingerprint: Option<String>,
    remote_forwards: super::forwarding::RemoteForwardRegistry,
    agent_forwarding: bool,
) -> Result<(client::Handle<ClientHandler>, Option<String>), (String, Option<String>)> {
    let observed_fingerprint = Arc::new(std::sync::Mutex::new(None));
    let handler = ClientHandler::new(
        expected_fingerprint,
        observed_fingerprint.clone(),
        remote_forwards,
        agent_forwarding,
    );
    let config = Arc::new(client::Config {
        // Send a keepalive every 15 s; disconnect only after 8 consecutive
        // missing replies (= 120 s). This more frequent probing helps detect
        // TCP silent drops by NAT/Firewall faster while maintaining tolerance.
        keepalive_interval: Some(std::time::Duration::from_secs(15)),
        keepalive_max: 8,
        ..Default::default()
    });

    let addr = format!("{host}:{port}");
    match client::connect(config, addr, handler).await {
        Ok(session) => Ok((session, observed_fingerprint_value(&observed_fingerprint))),
        Err(error) => Err((
            format!("Connection error: {error}"),
            observed_fingerprint_value(&observed_fingerprint),
        )),
    }
}

pub(super) async fn connect_stream_with_expected_fingerprint<R>(
    stream: R,
    expected_fingerprint: Option<String>,
    remote_forwards: super::forwarding::RemoteForwardRegistry,
    agent_forwarding: bool,
) -> Result<(client::Handle<ClientHandler>, Option<String>), (String, Option<String>)>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let observed_fingerprint = Arc::new(std::sync::Mutex::new(None));
    let handler = ClientHandler::new(
        expected_fingerprint,
        observed_fingerprint.clone(),
        remote_forwards,
        agent_forwarding,
    );
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(15)),
        keepalive_max: 8,
        ..Default::default()
    });

    match client::connect_stream(config, stream, handler).await {
        Ok(session) => Ok((session, observed_fingerprint_value(&observed_fingerprint))),
        Err(error) => Err((
            format!("Connection error: {error}"),
            observed_fingerprint_value(&observed_fingerprint),
        )),
    }
}

pub(super) async fn prompt_for_host_key_override(
    app: &AppHandle,
    state: SshState,
    id: &str,
    host: &str,
    port: u16,
    expected_fingerprint: &str,
    observed_fingerprint: &str,
) -> Result<bool, String> {
    let (tx, mut rx) = mpsc::channel::<bool>(1);
    state
        .upsert_session(id, |s| s.host_key_prompt_tx = Some(tx))
        .await;

    let emitted = app.emit(
        &crate::util::session_event("ssh-host-key-mismatch-prompt", id),
        SshHostKeyMismatchPromptPayload {
            id: id.to_string(),
            host: host.to_string(),
            port,
            expected_fingerprint: expected_fingerprint.to_string(),
            observed_fingerprint: observed_fingerprint.to_string(),
        },
    );

    if emitted.is_err() {
        state
            .with_session_mut(id, |s| s.host_key_prompt_tx = None)
            .await;
        return Err("Failed to emit SSH host key mismatch prompt".to_string());
    }

    let decision = tokio::time::timeout(tokio::time::Duration::from_secs(180), rx.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or(false);

    state
        .with_session_mut(id, |s| s.host_key_prompt_tx = None)
        .await;
    Ok(decision)
}
