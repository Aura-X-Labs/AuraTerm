//! SSH port-forwarding / tunnel manager.
//!
//! Implements the three classic OpenSSH forwarding modes on top of the existing
//! per-session russh [`client::Handle`](russh::client::Handle):
//!
//! - **Local (`-L`)** — bind a local TCP port; every accepted connection is
//!   spliced through a `direct-tcpip` channel to `dest_host:dest_port` reachable
//!   *from the SSH server*.
//! - **Dynamic (`-D`)** — bind a local TCP port that speaks **SOCKS5**; the
//!   destination is chosen per-connection by the SOCKS client and tunnelled the
//!   same way as a local forward.
//! - **Remote (`-R`)** — ask the server to listen on `bind_address:bind_port`
//!   (via [`tcpip_forward`](russh::client::Handle::tcpip_forward)); inbound
//!   connections arrive as `forwarded-tcpip` channels on the
//!   [`ClientHandler`](super::known_hosts::ClientHandler) and are spliced to a
//!   local `dest_host:dest_port`.
//!
//! All three reuse [`pump`] — `tokio::io::copy_bidirectional` over
//! [`Channel::into_stream`](russh::Channel::into_stream) — to move bytes.
//!
//! Tunnels are keyed by `"{session_id}\0{tunnel_id}"`; the live registry lives in
//! [`ForwardingState`]. Status transitions are surfaced to the frontend through
//! the global [`SSH_TUNNEL_STATUS_EVENT`] event.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, RwLock};

use super::SshState;

/// Global event carrying tunnel lifecycle transitions to the frontend.
pub(super) const SSH_TUNNEL_STATUS_EVENT: &str = "ssh-tunnel-status";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelType {
    /// `-L`: listen locally, forward to a destination reachable from the server.
    Local,
    /// `-R`: server listens, forwards back to a destination reachable from us.
    Remote,
    /// `-D`: listen locally as a SOCKS5 proxy.
    Dynamic,
}

/// Frontend-supplied tunnel definition (camelCase to match the TS payload).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub tunnel_type: TunnelType,
    #[serde(default)]
    pub bind_address: Option<String>,
    pub bind_port: u16,
    #[serde(default)]
    pub dest_host: Option<String>,
    #[serde(default)]
    pub dest_port: Option<u16>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TunnelStatusPayload {
    session_id: String,
    tunnel_id: String,
    tunnel_type: TunnelType,
    /// One of `starting` | `active` | `error` | `stopped`.
    status: String,
    message: Option<String>,
}

/// What a remote (`-R`) forward should connect to locally, plus the bind address
/// used to (re-)issue the server-side `tcpip-forward` request after a reconnect.
#[derive(Clone)]
pub(super) struct RemoteForwardTarget {
    pub bind_address: String,
    pub host: String,
    pub port: u16,
}

/// Shared, per-session map of `server_bind_port -> local target`, consulted by
/// the russh [`ClientHandler`](super::known_hosts::ClientHandler) when the
/// server opens a `forwarded-tcpip` channel. Held behind a std mutex because the
/// handler reads it from a sync context inside `check`/open callbacks.
pub(super) type RemoteForwardRegistry = Arc<StdMutex<HashMap<u32, RemoteForwardTarget>>>;

pub(super) fn new_remote_forward_registry() -> RemoteForwardRegistry {
    Arc::new(StdMutex::new(HashMap::new()))
}

/// Live state for a single running tunnel.
struct TunnelRuntime {
    tunnel_type: TunnelType,
    bind_address: String,
    bind_port: u16,
    /// Notified to tear down the listener and any in-flight pumps.
    cancel: Arc<Notify>,
    /// Latest status string, surfaced by [`ssh_list_tunnels`].
    status: Arc<StdMutex<String>>,
}

/// Registry of every running tunnel across all sessions.
#[derive(Clone, Default)]
pub struct ForwardingState {
    tunnels: Arc<RwLock<HashMap<String, TunnelRuntime>>>,
}

fn tunnel_key(session_id: &str, tunnel_id: &str) -> String {
    format!("{session_id}\0{tunnel_id}")
}

fn emit_status(
    app: &AppHandle,
    status_cell: &Arc<StdMutex<String>>,
    session_id: &str,
    tunnel_id: &str,
    tunnel_type: TunnelType,
    status: &str,
    message: Option<String>,
) {
    if let Ok(mut cell) = status_cell.lock() {
        *cell = status.to_string();
    }
    let _ = app.emit(
        SSH_TUNNEL_STATUS_EVENT,
        TunnelStatusPayload {
            session_id: session_id.to_string(),
            tunnel_id: tunnel_id.to_string(),
            tunnel_type,
            status: status.to_string(),
            message,
        },
    );
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTunnelInfo {
    tunnel_id: String,
    status: String,
}

/// Start a tunnel for `session_id`. For local/dynamic forwards the listener is
/// bound synchronously so that "address already in use" surfaces immediately to
/// the caller; the accept loop then runs in a background task.
#[tauri::command]
pub async fn ssh_start_tunnel(
    app: AppHandle,
    ssh_state: State<'_, SshState>,
    fwd_state: State<'_, ForwardingState>,
    session_id: String,
    spec: TunnelSpec,
) -> Result<(), String> {
    let key = tunnel_key(&session_id, &spec.id);
    if fwd_state.tunnels.read().await.contains_key(&key) {
        return Err("Tunnel is already running".to_string());
    }

    let bind_address = spec
        .bind_address
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let cancel = Arc::new(Notify::new());
    let status = Arc::new(StdMutex::new("starting".to_string()));

    let runtime = TunnelRuntime {
        tunnel_type: spec.tunnel_type,
        bind_address: bind_address.clone(),
        bind_port: spec.bind_port,
        cancel: cancel.clone(),
        status: status.clone(),
    };
    fwd_state.tunnels.write().await.insert(key.clone(), runtime);

    emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "starting", None);

    match spec.tunnel_type {
        TunnelType::Local | TunnelType::Dynamic => {
            if spec.tunnel_type == TunnelType::Local
                && (spec.dest_host.as_deref().unwrap_or("").trim().is_empty()
                    || spec.dest_port.is_none())
            {
                fwd_state.tunnels.write().await.remove(&key);
                let message = "Local forward requires a destination host and port".to_string();
                emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "error", Some(message.clone()));
                return Err(message);
            }

            let listener = match TcpListener::bind((bind_address.as_str(), spec.bind_port)).await {
                Ok(listener) => listener,
                Err(error) => {
                    fwd_state.tunnels.write().await.remove(&key);
                    let message = format!("Failed to bind {bind_address}:{}: {error}", spec.bind_port);
                    emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "error", Some(message.clone()));
                    return Err(message);
                }
            };

            emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "active", None);

            let app_task = app.clone();
            let ssh_task = ssh_state.inner().clone();
            let fwd_task = fwd_state.inner().clone();
            let status_task = status.clone();
            let dest_host = spec.dest_host.clone();
            let dest_port = spec.dest_port;
            let tunnel_id = spec.id.clone();
            let tunnel_type = spec.tunnel_type;
            tokio::spawn(async move {
                run_forward_listener(
                    app_task,
                    ssh_task,
                    fwd_task,
                    listener,
                    key,
                    session_id,
                    tunnel_id,
                    tunnel_type,
                    dest_host,
                    dest_port,
                    status_task,
                    cancel,
                )
                .await;
            });
            Ok(())
        }
        TunnelType::Remote => {
            let dest_host = spec
                .dest_host
                .clone()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let (dest_host, dest_port) = match (dest_host, spec.dest_port) {
                (Some(host), Some(port)) => (host, port),
                _ => {
                    fwd_state.tunnels.write().await.remove(&key);
                    let message = "Remote forward requires a destination host and port".to_string();
                    emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "error", Some(message.clone()));
                    return Err(message);
                }
            };

            let handle = match ssh_state.handle(&session_id).await {
                Some(handle) => handle,
                None => {
                    fwd_state.tunnels.write().await.remove(&key);
                    let message = "SSH session is not connected".to_string();
                    emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "error", Some(message.clone()));
                    return Err(message);
                }
            };

            let registry = ssh_state.ensure_remote_forwards(&session_id).await;
            if let Ok(mut map) = registry.lock() {
                map.insert(
                    spec.bind_port as u32,
                    RemoteForwardTarget {
                        bind_address: bind_address.clone(),
                        host: dest_host,
                        port: dest_port,
                    },
                );
            }

            let forward_result = {
                let guard = handle.lock().await;
                guard
                    .tcpip_forward(bind_address.clone(), spec.bind_port as u32)
                    .await
            };

            match forward_result {
                Ok(_) => {
                    emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "active", None);
                    Ok(())
                }
                Err(error) => {
                    if let Ok(mut map) = registry.lock() {
                        map.remove(&(spec.bind_port as u32));
                    }
                    fwd_state.tunnels.write().await.remove(&key);
                    let message = format!("Server refused remote forward: {error}");
                    emit_status(&app, &status, &session_id, &spec.id, spec.tunnel_type, "error", Some(message.clone()));
                    Err(message)
                }
            }
        }
    }
}

#[tauri::command]
pub async fn ssh_stop_tunnel(
    app: AppHandle,
    ssh_state: State<'_, SshState>,
    fwd_state: State<'_, ForwardingState>,
    session_id: String,
    tunnel_id: String,
) -> Result<(), String> {
    let key = tunnel_key(&session_id, &tunnel_id);
    let runtime = fwd_state.tunnels.write().await.remove(&key);
    let Some(runtime) = runtime else {
        return Ok(());
    };

    // Wake the listener and any in-flight pump tasks so they drop their sockets.
    runtime.cancel.notify_waiters();

    if runtime.tunnel_type == TunnelType::Remote {
        if let Some(handle) = ssh_state.handle(&session_id).await {
            let guard = handle.lock().await;
            let _ = guard
                .cancel_tcpip_forward(runtime.bind_address.clone(), runtime.bind_port as u32)
                .await;
        }
        if let Some(registry) = ssh_state.remote_forwards_opt(&session_id).await {
            if let Ok(mut map) = registry.lock() {
                map.remove(&(runtime.bind_port as u32));
            }
        }
    }

    emit_status(&app, &runtime.status, &session_id, &tunnel_id, runtime.tunnel_type, "stopped", None);
    Ok(())
}

#[tauri::command]
pub async fn ssh_list_tunnels(
    fwd_state: State<'_, ForwardingState>,
    session_id: String,
) -> Result<Vec<ActiveTunnelInfo>, String> {
    let prefix = format!("{session_id}\0");
    let map = fwd_state.tunnels.read().await;
    let mut out = Vec::new();
    for (key, runtime) in map.iter() {
        if let Some(tunnel_id) = key.strip_prefix(&prefix) {
            let status = runtime
                .status
                .lock()
                .map(|cell| cell.clone())
                .unwrap_or_else(|_| "active".to_string());
            out.push(ActiveTunnelInfo {
                tunnel_id: tunnel_id.to_string(),
                status,
            });
        }
    }
    Ok(out)
}

/// Stop and forget every tunnel belonging to `session_id`. Called when the SSH
/// session itself closes so listeners do not linger on dead handles.
pub(super) async fn stop_session_tunnels(fwd_state: &ForwardingState, session_id: &str) {
    let prefix = format!("{session_id}\0");
    let mut removed = Vec::new();
    {
        let mut map = fwd_state.tunnels.write().await;
        let keys: Vec<String> = map
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(runtime) = map.remove(&key) {
                removed.push(runtime);
            }
        }
    }
    for runtime in removed {
        runtime.cancel.notify_waiters();
    }
}

/// Accept loop for local/dynamic forwards. Each accepted socket is handled in
/// its own task so a slow SOCKS handshake never blocks new connections.
#[allow(clippy::too_many_arguments)]
async fn run_forward_listener(
    app: AppHandle,
    ssh_state: SshState,
    fwd_state: ForwardingState,
    listener: TcpListener,
    key: String,
    session_id: String,
    tunnel_id: String,
    tunnel_type: TunnelType,
    dest_host: Option<String>,
    dest_port: Option<u16>,
    status: Arc<StdMutex<String>>,
    cancel: Arc<Notify>,
) {
    loop {
        tokio::select! {
            _ = cancel.notified() => {
                // Stop initiated via ssh_stop_tunnel, which already removed the
                // map entry and emitted "stopped". Nothing more to do here.
                return;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let ssh_conn = ssh_state.clone();
                        let session_conn = session_id.clone();
                        let dest_host_conn = dest_host.clone();
                        let cancel_conn = cancel.clone();
                        tokio::spawn(async move {
                            handle_forward_connection(
                                ssh_conn,
                                session_conn,
                                tunnel_type,
                                stream,
                                peer,
                                dest_host_conn,
                                dest_port,
                                cancel_conn,
                            )
                            .await;
                        });
                    }
                    Err(error) => {
                        fwd_state.tunnels.write().await.remove(&key);
                        emit_status(
                            &app,
                            &status,
                            &session_id,
                            &tunnel_id,
                            tunnel_type,
                            "error",
                            Some(format!("Listener stopped: {error}")),
                        );
                        return;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_forward_connection(
    ssh_state: SshState,
    session_id: String,
    tunnel_type: TunnelType,
    mut stream: TcpStream,
    peer: SocketAddr,
    dest_host: Option<String>,
    dest_port: Option<u16>,
    cancel: Arc<Notify>,
) {
    let (target_host, target_port) = match tunnel_type {
        TunnelType::Dynamic => match socks5_negotiate(&mut stream).await {
            Ok(target) => target,
            Err(_) => return,
        },
        _ => match (dest_host, dest_port) {
            (Some(host), Some(port)) => (host, port),
            _ => return,
        },
    };

    let handle = match ssh_state.handle(&session_id).await {
        Some(handle) => handle,
        None => {
            if tunnel_type == TunnelType::Dynamic {
                let _ = socks5_reply(&mut stream, 0x03).await; // network unreachable
            }
            return;
        }
    };

    let channel = {
        let guard = handle.lock().await;
        guard
            .channel_open_direct_tcpip(
                target_host.clone(),
                target_port as u32,
                peer.ip().to_string(),
                peer.port() as u32,
            )
            .await
    };

    let channel = match channel {
        Ok(channel) => channel,
        Err(_) => {
            if tunnel_type == TunnelType::Dynamic {
                let _ = socks5_reply(&mut stream, 0x05).await; // connection refused
            }
            return;
        }
    };

    if tunnel_type == TunnelType::Dynamic && socks5_reply(&mut stream, 0x00).await.is_err() {
        return;
    }

    let mut channel_stream = channel.into_stream();
    tokio::select! {
        _ = cancel.notified() => {}
        _ = tokio::io::copy_bidirectional(&mut channel_stream, &mut stream) => {}
    }
}

/// Splice a server-initiated `forwarded-tcpip` channel (remote `-R` forward) to a
/// freshly opened local TCP connection.
///
/// The channel open is confirmed only once the local connection is up; a refused
/// target answers `SSH_OPEN_CONNECT_FAILED`, which is what OpenSSH does and what
/// lets the remote peer fail fast instead of writing into a channel we are about
/// to drop.
pub(super) async fn pump_channel_to_local(
    channel: russh::Channel<russh::client::Msg>,
    host: String,
    port: u16,
    reply: russh::client::ChannelOpenHandle,
) {
    let Ok(mut tcp) = TcpStream::connect((host.as_str(), port)).await else {
        reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
        return;
    };
    reply.accept().await;
    let mut channel_stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut channel_stream, &mut tcp).await;
}

// ---------------------------------------------------------------------------
// Minimal SOCKS5 (RFC 1928) server for dynamic (`-D`) forwarding.
// Only the no-auth method and the CONNECT command are supported, which is what
// browsers / `curl --socks5` use for an SSH dynamic proxy.
// ---------------------------------------------------------------------------

async fn socks5_negotiate<S>(stream: &mut S) -> std::io::Result<(String, u16)>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use std::io::{Error, ErrorKind};

    // Greeting: VER, NMETHODS, METHODS[NMETHODS].
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;
    if header[0] != 0x05 {
        return Err(Error::new(ErrorKind::InvalidData, "unsupported SOCKS version"));
    }
    let mut methods = vec![0u8; header[1] as usize];
    stream.read_exact(&mut methods).await?;
    // Select "no authentication required".
    stream.write_all(&[0x05, 0x00]).await?;

    // Request: VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT.
    let mut request = [0u8; 4];
    stream.read_exact(&mut request).await?;
    if request[0] != 0x05 {
        return Err(Error::new(ErrorKind::InvalidData, "unsupported SOCKS version"));
    }
    if request[1] != 0x01 {
        // Only CONNECT is supported.
        let _ = socks5_reply(stream, 0x07).await; // command not supported
        return Err(Error::new(ErrorKind::Unsupported, "only CONNECT is supported"));
    }

    let host = match request[3] {
        0x01 => {
            let mut addr = [0u8; 4];
            stream.read_exact(&mut addr).await?;
            std::net::Ipv4Addr::from(addr).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut domain = vec![0u8; len[0] as usize];
            stream.read_exact(&mut domain).await?;
            String::from_utf8(domain)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid domain name"))?
        }
        0x04 => {
            let mut addr = [0u8; 16];
            stream.read_exact(&mut addr).await?;
            std::net::Ipv6Addr::from(addr).to_string()
        }
        _ => {
            let _ = socks5_reply(stream, 0x08).await; // address type not supported
            return Err(Error::new(ErrorKind::InvalidData, "unsupported address type"));
        }
    };

    let mut port = [0u8; 2];
    stream.read_exact(&mut port).await?;
    Ok((host, u16::from_be_bytes(port)))
}

/// Write a SOCKS5 reply with `rep` and a dummy IPv4 `0.0.0.0:0` bound address.
async fn socks5_reply<S>(stream: &mut S, rep: u8) -> std::io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    stream
        .write_all(&[0x05, rep, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn tunnel_type_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&TunnelType::Local).unwrap(), "\"local\"");
        assert_eq!(serde_json::to_string(&TunnelType::Remote).unwrap(), "\"remote\"");
        assert_eq!(serde_json::to_string(&TunnelType::Dynamic).unwrap(), "\"dynamic\"");
    }

    #[test]
    fn tunnel_spec_accepts_frontend_payload() {
        let spec: TunnelSpec = serde_json::from_str(
            r#"{"id":"t1","type":"local","bindAddress":"127.0.0.1","bindPort":8080,"destHost":"db","destPort":5432,"name":"x","autoStart":true}"#,
        )
        .expect("spec should parse");
        assert_eq!(spec.id, "t1");
        assert!(matches!(spec.tunnel_type, TunnelType::Local));
        assert_eq!(spec.bind_port, 8080);
        assert_eq!(spec.dest_host.as_deref(), Some("db"));
        assert_eq!(spec.dest_port, Some(5432));
    }

    #[test]
    fn tunnel_key_is_session_scoped() {
        assert_eq!(tunnel_key("tab-0", "t1"), "tab-0\0t1");
        assert_ne!(tunnel_key("a", "t"), tunnel_key("b", "t"));
    }

    #[tokio::test]
    async fn socks5_parses_domain_connect_request() {
        let (mut client, mut server) = tokio::io::duplex(256);

        let client_task = tokio::spawn(async move {
            // Greeting: VER=5, NMETHODS=1, METHOD=no-auth.
            client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut selection = [0u8; 2];
            client.read_exact(&mut selection).await.unwrap();
            assert_eq!(selection, [0x05, 0x00]);

            // Request: CONNECT, domain "example.com", port 443.
            let host = b"example.com";
            let mut request = vec![0x05, 0x01, 0x00, 0x03, host.len() as u8];
            request.extend_from_slice(host);
            request.extend_from_slice(&443u16.to_be_bytes());
            client.write_all(&request).await.unwrap();
        });

        let (host, port) = socks5_negotiate(&mut server).await.expect("negotiate ok");
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        client_task.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_parses_ipv4_connect_request() {
        let (mut client, mut server) = tokio::io::duplex(256);

        let client_task = tokio::spawn(async move {
            client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut selection = [0u8; 2];
            client.read_exact(&mut selection).await.unwrap();
            // Request: CONNECT, IPv4 10.0.0.5, port 22.
            client
                .write_all(&[0x05, 0x01, 0x00, 0x01, 10, 0, 0, 5, 0x00, 0x16])
                .await
                .unwrap();
        });

        let (host, port) = socks5_negotiate(&mut server).await.expect("negotiate ok");
        assert_eq!(host, "10.0.0.5");
        assert_eq!(port, 22);
        client_task.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_rejects_unsupported_command() {
        let (mut client, mut server) = tokio::io::duplex(256);

        let client_task = tokio::spawn(async move {
            client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut selection = [0u8; 2];
            client.read_exact(&mut selection).await.unwrap();
            // BIND (0x02) is not supported.
            client
                .write_all(&[0x05, 0x02, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            // Server should send a failure reply.
            let mut reply = [0u8; 10];
            let _ = client.read_exact(&mut reply).await;
            reply[1]
        });

        let result = socks5_negotiate(&mut server).await;
        assert!(result.is_err());
        let reply_code = client_task.await.unwrap();
        assert_eq!(reply_code, 0x07); // command not supported
    }
}
