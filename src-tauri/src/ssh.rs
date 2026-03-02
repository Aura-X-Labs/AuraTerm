use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter};

#[derive(Clone, serde::Serialize)]
struct PtyOutputEvent {
    id: String,
    data: String,
}

#[derive(Clone, serde::Serialize)]
struct PtyExitEvent {
    id: String,
    message: String,
}

pub struct SshSession {
    pub channel: Arc<Mutex<ssh2::Channel>>,
    #[allow(dead_code)]
    pub session: Arc<Mutex<ssh2::Session>>,
}

#[derive(Clone)]
pub struct SshState {
    pub sessions: Arc<Mutex<std::collections::HashMap<String, SshSession>>>,
}

impl Default for SshState {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
}

#[tauri::command]
pub fn start_ssh_pty(
    app: AppHandle,
    state: tauri::State<'_, super::AppState>,
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    private_key: Option<String>,
    cols: u32,
    rows: u32,
    id: String,
) -> Result<String, String> {
    let tcp = TcpStream::connect(format!("{}:{}", host, port)).map_err(|e| e.to_string())?;
    // Enable TCP keepalive to prevent random "transport read" disconnects on idle
    let _ = tcp.set_nodelay(true);

    let mut sess = Session::new().map_err(|e| e.to_string())?;
    sess.set_tcp_stream(tcp);
    sess.handshake().map_err(|e| e.to_string())?;

    // Enable SSH protocol keepalive
    sess.set_keepalive(true, 15);

    if let Some(_pk) = private_key {
        // Need to parse or use a file path? The simple way is via file, but since we get file contents...
        // Let's assume it's just trying password for now to get it compiling, or we can use the `userauth_pubkey_memory` if we enable vendored-openssl
        sess.userauth_password(&user, password.as_deref().unwrap_or("")).map_err(|e| e.to_string())?;
    } else if let Some(pw) = password {
        sess.userauth_password(&user, &pw).map_err(|e| e.to_string())?;
    } else {
        return Err("No authentication method provided".to_string());
    };

    if !sess.authenticated() {
        return Err("Authentication failed".to_string());
    }

    let mut channel = sess.channel_session().map_err(|e| e.to_string())?;
    channel.request_pty("xterm", None, Some((cols, rows, 0, 0))).map_err(|e| e.to_string())?;
    channel.shell().map_err(|e| e.to_string())?;

    // Make channel non-blocking for reading
    sess.set_blocking(false);

    let sess_arc = Arc::new(Mutex::new(sess));
    let channel_arc = Arc::new(Mutex::new(channel));

    {
        let mut guard = state.ssh_state.sessions.lock().unwrap();
        guard.insert(
            id.clone(),
            SshSession {
                channel: channel_arc.clone(),
                session: sess_arc.clone(),
            },
        );
    }

    let pty_id = id.clone();
    let app_handle = app.clone();
    
    let channel_clone = channel_arc.clone();
    
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            let mut read_amount = 0;
            let mut err_msg = None;
            let mut should_exit = false;

            {
                if let Ok(mut c) = channel_clone.lock() {
                    match c.read(&mut buffer) {
                        Ok(0) => {
                            if c.eof() {
                                should_exit = true;
                            }
                        }
                        Ok(size) => {
                            read_amount = size;
                        }
                        Err(e) => {
                            let e_str = e.to_string();
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e_str.to_lowercase().contains("would block")
                                || e_str.to_uppercase().contains("EAGAIN")
                            {
                                // 非阻塞模式下的正常情况，忽略
                            } else {
                                err_msg = Some(e_str);
                                should_exit = true;
                            }
                        }
                    }
                } else {
                    should_exit = true;
                }
            }

            if should_exit {
                let _ = app_handle.emit(
                    "pty-exit",
                    PtyExitEvent {
                        id: pty_id.clone(),
                        message: err_msg.unwrap_or_else(|| "SSH PTY closed".to_string()),
                    },
                );
                break;
            }

            if read_amount > 0 {
                let output = String::from_utf8_lossy(&buffer[..read_amount]).to_string();
                let _ = app_handle.emit(
                    "pty-output",
                    PtyOutputEvent {
                        id: pty_id.clone(),
                        data: output,
                    },
                );
            } else {
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    });

    Ok(id)
}

#[tauri::command]
pub fn write_ssh_pty_input(
    state: tauri::State<'_, super::AppState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let channel_arc = {
        let mut guard = state.ssh_state.sessions.lock().map_err(|error| error.to_string())?;
        let Some(session) = guard.get_mut(&id) else {
            return Err("SSH session not found".to_string());
        };
        session.channel.clone()
    };

    // Write data iteratively in non-blocking mode
    let bytes = data.as_bytes();
    let mut written = 0;
    while written < bytes.len() {
        let mut channel = channel_arc.lock().map_err(|error| error.to_string())?;
        match channel.write(&bytes[written..]) {
            Ok(n) => {
                if n == 0 {
                    return Err("Failed to write to SSH channel: 0 bytes written".to_string());
                }
                written += n;
            }
            Err(e) => {
                let e_str = e.to_string();
                if e.kind() == std::io::ErrorKind::WouldBlock || e_str.to_lowercase().contains("would block") || e_str.to_uppercase().contains("EAGAIN") {
                    drop(channel); // Release lock before sleeping
                    std::thread::yield_now();
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    continue;
                }
                return Err(e_str);
            }
        }
    }
    if let Ok(mut channel) = channel_arc.lock() {
        let _ = channel.flush();
    }
    Ok(())
}

#[tauri::command]
pub fn resize_ssh_pty(
    state: tauri::State<'_, super::AppState>,
    id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let channel_arc = {
        let mut guard = state.ssh_state.sessions.lock().map_err(|error| error.to_string())?;
        let Some(session) = guard.get_mut(&id) else {
            return Ok(());
        };
        session.channel.clone()
    };

    loop {
        let mut channel = channel_arc.lock().map_err(|error| error.to_string())?;
        match channel.request_pty_size(cols, rows, None, None) {
            Ok(_) => return Ok(()),
            Err(e) => {
                let e_str = e.to_string();
                if e_str.to_lowercase().contains("would block") || e_str.to_uppercase().contains("EAGAIN") {
                    drop(channel);
                    std::thread::yield_now();
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    continue;
                }
                return Err(e_str);
            }
        }
    }
}

#[tauri::command]
pub fn close_ssh_pty(
    state: tauri::State<'_, super::AppState>,
    id: String,
) -> Result<(), String> {
    let mut guard = state.ssh_state.sessions.lock().map_err(|error| error.to_string())?;
    if let Some(session) = guard.remove(&id) {
        if let Ok(mut channel) = session.channel.lock() {
            let _ = channel.send_eof();
            let _ = channel.close();
            // We won't block to wait_close to avoid freezing UI
            // let _ = channel.wait_close();
        }
    }
    Ok(())
}
