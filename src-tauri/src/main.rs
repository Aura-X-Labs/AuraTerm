// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};
use tauri::{command, AppHandle, Emitter, State};

mod connections;
mod settings;
mod ssh;

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
}

#[derive(Clone)]
struct AppState {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    ssh_state: ssh::SshState,
}

#[derive(Clone, Serialize)]
struct PtyOutputEvent {
    id: String,
    data: String,
}

#[derive(Clone, Serialize)]
struct PtyExitEvent {
    id: String,
    message: String,
}

#[command]
fn start_pty(app: AppHandle, state: State<'_, AppState>, cols: u16, rows: u16, id: String) -> Result<String, String> {
    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;

    let shell_path = if cfg!(target_os = "windows") {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    };

    let command = CommandBuilder::new(shell_path);
    let child = pty_pair
        .slave
        .spawn_command(command)
        .map_err(|error| error.to_string())?;

    drop(pty_pair.slave);

    let writer = pty_pair
        .master
        .take_writer()
        .map_err(|error| error.to_string())?;

    let mut reader = pty_pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;

    {
        let mut guard = state.sessions.lock().map_err(|error| error.to_string())?;
        guard.insert(
            id.clone(),
            PtySession {
                master: pty_pair.master,
                writer,
                child,
            },
        );
    }

    let app_handle = app.clone();
    let pty_id = id.clone();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = app_handle.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: pty_id.clone(),
                            message: "PTY closed".to_string(),
                        },
                    );
                    break;
                }
                Ok(size) => {
                    let output = String::from_utf8_lossy(&buffer[..size]).to_string();
                    let _ = app_handle.emit(
                        "pty-output",
                        PtyOutputEvent {
                            id: pty_id.clone(),
                            data: output,
                        },
                    );
                }
                Err(_) => {
                    let _ = app_handle.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: pty_id.clone(),
                            message: "PTY read error".to_string(),
                        },
                    );
                    break;
                }
            }
        }
    });

    Ok(id)
}

#[command]
fn write_pty_input(state: State<'_, AppState>, id: String, data: String) -> Result<(), String> {
    let mut guard = state.sessions.lock().map_err(|error| error.to_string())?;
    let Some(session) = guard.get_mut(&id) else {
        return Err("PTY session not found".to_string());
    };

    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|error| error.to_string())?;
    session.writer.flush().map_err(|error| error.to_string())?;

    Ok(())
}

#[command]
fn resize_pty(state: State<'_, AppState>, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let mut guard = state.sessions.lock().map_err(|error| error.to_string())?;
    let Some(session) = guard.get_mut(&id) else {
        return Ok(());
    };

    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;

    Ok(())
}

#[command]
fn close_pty(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut guard = state.sessions.lock().map_err(|error| error.to_string())?;
    if let Some(mut session) = guard.remove(&id) {
        let _ = session.child.kill();
    }
    Ok(())
}

fn main() {
    let app_state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        ssh_state: ssh::SshState::default(),
    };

    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, MenuItem, SubmenuBuilder};
                let about_item = MenuItem::with_id(app, "about", "About AuraTerm", true, None::<&str>)?;
                let help_menu = SubmenuBuilder::new(app, "Help")
                    .item(&about_item)
                    .build()?;
                let menu = MenuBuilder::new(app)
                    .item(&help_menu)
                    .build()?;
                app.set_menu(menu)?;
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "about" {
                let _ = app.emit("show-about", ());
            }
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            start_pty,
            write_pty_input,
            resize_pty,
            close_pty,
            ssh::start_ssh_pty,
            ssh::write_ssh_pty_input,
            ssh::resize_ssh_pty,
            ssh::close_ssh_pty,
            settings::get_settings,
            settings::save_settings,
            connections::get_connections,
            connections::save_connection,
            connections::delete_connection,
            connections::touch_connection,
        ])
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
