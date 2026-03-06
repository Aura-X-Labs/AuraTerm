use russh::{client, ChannelMsg};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{mpsc, Mutex};

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
        }
    }
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
    let mut connections = state.connections.lock().await;
    let mut resize_channels = state.resize_channels.lock().await;
    let mut auth_responses = state.auth_responses.lock().await;
    
    connections.remove(&id);
    resize_channels.remove(&id);
    auth_responses.remove(&id);
    
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
    _private_key: Option<String>,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    println!(
        "Starting SSH connection to {}@{}:{} with JumpServer bypass for MFA",
        user, host, port
    );

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

    let session_res = client::connect(config, addr, handler).await;
    if let Err(e) = session_res {
         return Err(format!("Connection error: {}", e));
    }
    let mut session = session_res.unwrap();
    

    println!("Authenticating as {}...", user);

    let auth_res_start = if let Some(pwd) = password {
        // Try password first if provided
        let res = session.authenticate_password(user.clone(), pwd).await;
        match res {
            Ok(russh::client::AuthResult::Success) => {
                println!("Password authentication successful!");
                // we should continue
                None
            }
            Ok(russh::client::AuthResult::Failure { .. }) => {
                println!("Password auth failed, falling back to keyboard-interactive");
                Some(session.authenticate_keyboard_interactive_start(user.clone(), None).await)
            }
            Err(e) => return Err(format!("Password auth error: {}", e)),
        }
    } else {
        Some(session.authenticate_keyboard_interactive_start(user.clone(), None).await)
    };

    if let Some(auth_res_future) = auth_res_start {
        let mut auth_res = auth_res_future.map_err(|e| format!("Keyboard interactive init failed: {}", e))?;

        loop {
            match auth_res {
                russh::client::KeyboardInteractiveAuthResponse::Success => {
                    println!("Authentication successful!");
                    break;
                }
                russh::client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                    return Err("Authentication failed".into());
                }
                russh::client::KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => {
                    let prompt_payloads: Vec<SshMfaPrompt> = prompts
                        .iter()
                        .map(|p| SshMfaPrompt {
                            text: p.prompt.clone(),
                            echo: p.echo,
                        })
                        .collect();

                    let payload = KeyboardInteractivePromptPayload {
                        id: id.clone(),
                        name: name.clone(),
                        instruction: instructions.clone(),
                        prompts: prompt_payloads,
                    };

                    let _ = app.emit("ssh-mfa-prompt", payload);

                    let mut answers = Vec::new();
                    for _ in prompts.iter() {
                        if let Some(resp) = auth_response_rx.recv().await {
                            answers.push(resp);
                        } else {
                            return Err("Failed to get response".into());
                        }
                    }

                    auth_res = session
                        .authenticate_keyboard_interactive_respond(answers)
                        .await
                        .map_err(|e| format!("Failed auth step: {}", e))?;
                }
            }
        }
    }

    println!("Requesting PTY and shell...");
    let channel_res = session.channel_open_session().await;
    if channel_res.is_err() {
        return Err(format!("Channel error: {}", channel_res.unwrap_err()));
    }
    let mut channel = channel_res.unwrap();

    channel.request_pty(false, "xterm-256color", cols, rows, 0, 0, &[]).await
        .map_err(|e| format!("PTY request failed: {}", e))?;

    channel.request_shell(true).await
        .map_err(|e| format!("Shell request failed: {}", e))?;

    let (input_tx, mut input_rx) = mpsc::channel::<String>(32);
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(32);
    
    state.connections.lock().await.insert(id.clone(), input_tx);
    state.resize_channels.lock().await.insert(id.clone(), resize_tx);

    let connection_id_clone = id.clone();
    let app_handle = app.clone();

    let _ = app.emit("ssh-connected", TerminalDataPayload {
        id: id.clone(),
        data: String::new(),
    });

    tokio::spawn(async move {
        let mut _session_handle = session;
        loop {
            tokio::select! {
                Some(msg) = channel.wait() => {
                    match msg {
                        ChannelMsg::Data { ref data } => {
                            let payload = TerminalDataPayload {
                                id: connection_id_clone.clone(),
                                data: String::from_utf8_lossy(data).to_string(),
                            };
                            let _ = app_handle.emit("pty-output", payload);
                        }
                        ChannelMsg::ExtendedData { ref data, .. } => {
                            let payload = TerminalDataPayload {
                                id: connection_id_clone.clone(),
                                data: String::from_utf8_lossy(data).to_string(),
                            };
                            let _ = app_handle.emit("pty-output", payload);
                        }
                        ChannelMsg::Eof => {
                            let _ = app_handle.emit(
                                "pty-exit",
                                PtyExitPayload {
                                    id: connection_id_clone.clone(),
                                    message: "SSH channel closed".to_string(),
                                },
                            );
                            break;
                        }
                        ChannelMsg::Close => {
                            let _ = app_handle.emit(
                                "pty-exit",
                                PtyExitPayload {
                                    id: connection_id_clone.clone(),
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
        
    });

    Ok(())
}
