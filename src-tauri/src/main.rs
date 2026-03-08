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
mod serial;
mod settings;
mod ssh;
mod telnet;

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
}

#[derive(Clone)]
struct AppState {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    
}

#[derive(Clone, Serialize)]
pub(crate) struct PtyOutputEvent {
    id: String,
    data: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct PtyExitEvent {
    id: String,
    message: String,
}

/// Detect default shell on Windows: Git Bash -> PowerShell -> CMD
fn detect_default_shell_windows() -> String {
    // 1. Check Git Bash in common locations
    let git_bash_paths = vec![
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ];

    for path in &git_bash_paths {
        if std::path::Path::new(path).exists() {
            return path.to_string();
        }
    }

    // 2. Check via ProgramFiles environment variable
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        let git_bash = format!("{}\\Git\\bin\\bash.exe", program_files);
        if std::path::Path::new(&git_bash).exists() {
            return git_bash;
        }
    }

    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        let git_bash = format!("{}\\Git\\bin\\bash.exe", program_files_x86);
        if std::path::Path::new(&git_bash).exists() {
            return git_bash;
        }
    }

    // 3. Fallback to CMD
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
}

fn user_home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            let mut combined = std::path::PathBuf::from(drive);
            combined.push(path);
            Some(combined)
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn expand_tilde_path(path: &str) -> std::path::PathBuf {
    if path == "~" {
        return user_home_dir();
    }
    if let Some(stripped) = path.strip_prefix("~/") {
        return user_home_dir().join(stripped);
    }
    std::path::PathBuf::from(path)
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

    // Get shell path from settings or detect default
    let settings = settings::get_settings(app.clone()).unwrap_or_default();
    let shell_path = if let Some(custom_shell) = settings.shell_path {
        if !custom_shell.trim().is_empty() {
            custom_shell
        } else {
            // Empty string, use default
            if cfg!(target_os = "windows") {
                detect_default_shell_windows()
            } else {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            }
        }
    } else {
        // Not configured, use default
        if cfg!(target_os = "windows") {
            detect_default_shell_windows()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        }
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

#[command]
fn append_to_log(path: String, content: String) -> Result<(), String> {
    let expanded = expand_tilde_path(&path);

    if let Some(parent) = expanded.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&expanded)
        .map_err(|e| e.to_string())?;
    file.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
fn save_terminal_log(content: String, tab_name: String) -> Result<String, String> {
    let logs_dir = user_home_dir().join("AuraTerm").join("logs");
    std::fs::create_dir_all(&logs_dir).map_err(|e| e.to_string())?;

    // Sanitize the tab name so it is safe for use in a filename
    let safe_name: String = tab_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '@' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Human-readable timestamp (UTC seconds formatted as YYYYMMDD_HHMMSS)
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    // Rough Gregorian calendar calculation for YYYYMMDD
    let (year, month, day) = {
        let mut y = 1970u64;
        let mut d = days;
        loop {
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let days_in_year = if leap { 366 } else { 365 };
            if d < days_in_year {
                break;
            }
            d -= days_in_year;
            y += 1;
        }
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let month_days: [u64; 12] = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut mo = 1u64;
        let mut rem = d;
        for &mdays in &month_days {
            if rem < mdays {
                break;
            }
            rem -= mdays;
            mo += 1;
        }
        (y, mo, rem + 1)
    };
    let ts = format!("{:04}{:02}{:02}_{:02}{:02}{:02}", year, month, day, h, m, s);

    let filename = format!("{}_{}.log", ts, safe_name);
    let path = logs_dir.join(&filename);
    std::fs::write(&path, content.as_bytes()).map_err(|e| e.to_string())?;

    Ok(path.to_string_lossy().into_owned())
}

#[derive(Serialize)]
struct VersionInfo {
    version: &'static str,
    build_time: &'static str,
}

#[command]
fn get_version_info() -> VersionInfo {
    VersionInfo {
        version: env!("CARGO_PKG_VERSION"),
        build_time: env!("BUILD_TIME"),
    }
}

fn main() {
    let app_state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        
    };

    tauri::Builder::default()
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, MenuItem, SubmenuBuilder};

                let new_local_item = MenuItem::with_id(_app, "menu-new-local", "Local Shell", true, None::<&str>)?;
                let new_ssh_item = MenuItem::with_id(_app, "menu-new-ssh", "SSH", true, None::<&str>)?;
                let new_telnet_item = MenuItem::with_id(_app, "menu-new-telnet", "Telnet", true, None::<&str>)?;
                let new_serial_item = MenuItem::with_id(_app, "menu-new-serial", "Serial", true, None::<&str>)?;
                let close_tab_item = MenuItem::with_id(_app, "menu-close-tab", "Close Tab", true, None::<&str>)?;
                let settings_item = MenuItem::with_id(_app, "menu-open-settings", "Settings", true, None::<&str>)?;
                let toggle_bookmarks_item = MenuItem::with_id(_app, "menu-toggle-bookmarks", "Toggle Bookmarks", true, None::<&str>)?;
                let toggle_remote_files_item = MenuItem::with_id(_app, "menu-toggle-remote-files", "Toggle Remote Files", true, None::<&str>)?;
                let exit_item = MenuItem::with_id(_app, "exit", "Exit", true, None::<&str>)?;
                let about_item = MenuItem::with_id(_app, "about", "About AuraTerm", true, None::<&str>)?;

                let new_session_menu = SubmenuBuilder::new(_app, "New Session")
                    .item(&new_local_item)
                    .item(&new_ssh_item)
                    .item(&new_telnet_item)
                    .item(&new_serial_item)
                    .build()?;

                let preferences_menu = SubmenuBuilder::new(_app, "Preferences")
                    .item(&settings_item)
                    .build()?;

                let file_menu = SubmenuBuilder::new(_app, "File")
                    .item(&new_session_menu)
                    .item(&close_tab_item)
                    .separator()
                    .item(&preferences_menu)
                    .separator()
                    .item(&exit_item)
                    .build()?;
                let view_menu = SubmenuBuilder::new(_app, "View")
                    .item(&toggle_bookmarks_item)
                    .item(&toggle_remote_files_item)
                    .build()?;
                let help_menu = SubmenuBuilder::new(_app, "Help")
                    .item(&about_item)
                    .build()?;
                let menu = MenuBuilder::new(_app)
                    .item(&file_menu)
                    .item(&view_menu)
                    .item(&help_menu)
                    .build()?;

                _app.set_menu(menu)?;
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "about" => {
                    let _ = app.emit("show-about", ());
                }
                "menu-open-settings" => {
                    let _ = app.emit("menu-open-settings", ());
                }
                "menu-new-local" => {
                    let _ = app.emit("menu-new-local", ());
                }
                "menu-new-ssh" => {
                    let _ = app.emit("menu-new-ssh", ());
                }
                "menu-new-telnet" => {
                    let _ = app.emit("menu-new-telnet", ());
                }
                "menu-new-serial" => {
                    let _ = app.emit("menu-new-serial", ());
                }
                "menu-close-tab" => {
                    let _ = app.emit("menu-close-tab", ());
                }
                "menu-toggle-bookmarks" => {
                    let _ = app.emit("menu-toggle-bookmarks", ());
                }
                "menu-toggle-remote-files" => {
                    let _ = app.emit("menu-toggle-remote-files", ());
                }
                "exit" => {
                    app.exit(0);
                }
                _ => {}
            }
        })
        .manage(app_state)
        .manage(ssh::SshState::default())
        .manage(telnet::TelnetState::default())
        .manage(serial::SerialState::default())
        .invoke_handler(tauri::generate_handler![
            get_version_info,
            start_pty,
            write_pty_input,
            resize_pty,
            close_pty,
            ssh::start_ssh_pty,
            ssh::write_ssh_pty_input,
            ssh::resize_ssh_pty,
            ssh::close_ssh_pty,
            ssh::answer_ssh_mfa,
            ssh::ssh_list_remote_dir,
            ssh::ssh_create_remote_dir,
            ssh::ssh_remove_remote_entry,
            ssh::ssh_upload_file,
            ssh::ssh_download_file,
            telnet::start_telnet_session,
            telnet::write_telnet_input,
            telnet::close_telnet_session,
            serial::list_serial_ports,
            serial::start_serial_session,
            serial::write_serial_input,
            serial::close_serial_session,
            settings::get_settings,
            settings::save_settings,
            connections::get_connections,
            connections::save_connection,
            connections::delete_connection,
            connections::touch_connection,
            save_terminal_log,
            append_to_log,
        ])
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
