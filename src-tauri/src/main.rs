// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
};
use tauri::{command, AppHandle, Emitter, State};

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
}

#[derive(Clone)]
struct AppState {
    session: Arc<Mutex<Option<PtySession>>>,
}

#[command]
fn start_pty(app: AppHandle, state: State<'_, AppState>, cols: u16, rows: u16) -> Result<(), String> {
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
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
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
        let mut guard = state.session.lock().map_err(|error| error.to_string())?;
        if let Some(mut existing) = guard.take() {
            let _ = existing.child.kill();
        }

        *guard = Some(PtySession {
            master: pty_pair.master,
            writer,
            child,
        });
    }

    let app_handle = app.clone();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = app_handle.emit("pty-exit", "PTY closed");
                    break;
                }
                Ok(size) => {
                    let output = String::from_utf8_lossy(&buffer[..size]).to_string();
                    let _ = app_handle.emit("pty-output", output);
                }
                Err(_) => {
                    let _ = app_handle.emit("pty-exit", "PTY read error");
                    break;
                }
            }
        }
    });

    Ok(())
}

#[command]
fn write_pty_input(state: State<'_, AppState>, data: String) -> Result<(), String> {
    let mut guard = state.session.lock().map_err(|error| error.to_string())?;
    let Some(session) = guard.as_mut() else {
        return Err("PTY session not started".to_string());
    };

    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|error| error.to_string())?;
    session.writer.flush().map_err(|error| error.to_string())?;

    Ok(())
}

#[command]
fn resize_pty(state: State<'_, AppState>, cols: u16, rows: u16) -> Result<(), String> {
    let mut guard = state.session.lock().map_err(|error| error.to_string())?;
    let Some(session) = guard.as_mut() else {
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
fn close_pty(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.session.lock().map_err(|error| error.to_string())?;
    if let Some(mut session) = guard.take() {
        let _ = session.child.kill();
    }
    Ok(())
}

fn main() {
    let app_state = AppState {
        session: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            start_pty,
            write_pty_input,
            resize_pty,
            close_pty
        ])
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}