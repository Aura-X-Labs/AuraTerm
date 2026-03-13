// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
#[cfg(unix)]
use std::ffi::CStr;

use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{
    command, AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, Position,
    Size, State, WindowEvent,
};

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
    window_bounds_save_state: Arc<Mutex<WindowBoundsSaveState>>,
    startup_dir: Arc<Mutex<Option<String>>>,
}

struct WindowBoundsSaveState {
    last_saved: Option<settings::WindowBounds>,
    last_save_at: Option<Instant>,
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

#[cfg(unix)]
fn detect_login_shell_unix() -> String {
    unsafe {
        let passwd = libc::getpwuid(libc::getuid());
        if !passwd.is_null() {
            let shell = CStr::from_ptr((*passwd).pw_shell);
            if let Ok(shell) = shell.to_str() {
                if !shell.trim().is_empty() {
                    return shell.to_string();
                }
            }
        }
    }

    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

fn resolve_local_shell_path(app: &AppHandle) -> String {
    let settings = settings::get_settings(app.clone()).unwrap_or_default();

    if let Some(custom_shell) = settings.shell_path {
        if !custom_shell.trim().is_empty() {
            return custom_shell;
        }
    }

    if cfg!(target_os = "windows") {
        detect_default_shell_windows()
    } else {
        #[cfg(unix)]
        {
            detect_login_shell_unix()
        }

        #[cfg(not(unix))]
        {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        }
    }
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

fn is_window_bounds_visible(bounds: &settings::WindowBounds, work_area: &PhysicalRect<i32, u32>) -> bool {
    let left = i64::from(bounds.x);
    let top = i64::from(bounds.y);
    let right = left + i64::from(bounds.width);
    let bottom = top + i64::from(bounds.height);

    let work_left = i64::from(work_area.position.x);
    let work_top = i64::from(work_area.position.y);
    let work_right = work_left + i64::from(work_area.size.width);
    let work_bottom = work_top + i64::from(work_area.size.height);

    right > work_left && left < work_right && bottom > work_top && top < work_bottom
}

fn clamp_window_bounds_to_work_area(
    bounds: &settings::WindowBounds,
    work_area: &PhysicalRect<i32, u32>,
) -> settings::WindowBounds {
    let max_width = work_area.size.width.max(200);
    let max_height = work_area.size.height.max(200);

    let width = bounds.width.clamp(200, max_width);
    let height = bounds.height.clamp(200, max_height);

    let min_x = work_area.position.x;
    let min_y = work_area.position.y;
    let max_x = work_area
        .position
        .x
        .saturating_add((work_area.size.width.saturating_sub(width)) as i32);
    let max_y = work_area
        .position
        .y
        .saturating_add((work_area.size.height.saturating_sub(height)) as i32);

    settings::WindowBounds {
        x: bounds.x.clamp(min_x, max_x),
        y: bounds.y.clamp(min_y, max_y),
        width,
        height,
    }
}

fn resolve_window_bounds_for_restore(
    saved: &settings::WindowBounds,
    work_areas: &[PhysicalRect<i32, u32>],
) -> Option<settings::WindowBounds> {
    if work_areas.is_empty() {
        return None;
    }

    for work_area in work_areas {
        if is_window_bounds_visible(saved, work_area) {
            return Some(clamp_window_bounds_to_work_area(saved, work_area));
        }
    }

    Some(clamp_window_bounds_to_work_area(saved, &work_areas[0]))
}

fn current_window_bounds(window: &tauri::WebviewWindow) -> Result<settings::WindowBounds, String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.outer_size().map_err(|error| error.to_string())?;

    Ok(settings::WindowBounds {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

fn save_window_bounds_if_needed(app: &AppHandle, bounds: settings::WindowBounds) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let mut guard = state
            .window_bounds_save_state
            .lock()
            .map_err(|error| error.to_string())?;

        let recently_saved = guard
            .last_save_at
            .is_some_and(|instant| instant.elapsed() < Duration::from_millis(250));

        if guard.last_saved.as_ref() == Some(&bounds) || recently_saved {
            if guard.last_saved.as_ref() != Some(&bounds) {
                guard.last_saved = Some(bounds);
            }
            return Ok(());
        }

        guard.last_saved = Some(bounds.clone());
        guard.last_save_at = Some(Instant::now());
    }

    let mut next_settings = settings::get_settings(app.clone())?;
    next_settings.window_bounds = Some(bounds);
    settings::save_settings(app.clone(), next_settings)
}

fn apply_saved_window_bounds(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;

    let saved_bounds = settings::get_settings(app.clone())?.window_bounds;
    let Some(saved_bounds) = saved_bounds else {
        return Ok(());
    };

    let monitors = window.available_monitors().map_err(|error| error.to_string())?;
    let work_areas: Vec<PhysicalRect<i32, u32>> = monitors.into_iter().map(|monitor| monitor.work_area().clone()).collect();

    let Some(bounds) = resolve_window_bounds_for_restore(&saved_bounds, &work_areas) else {
        return Ok(());
    };

    window
        .set_size(Size::Physical(PhysicalSize::new(bounds.width, bounds.height)))
        .map_err(|error| error.to_string())?;
    window
        .set_position(Position::Physical(PhysicalPosition::new(bounds.x, bounds.y)))
        .map_err(|error| error.to_string())?;

    let state = app.state::<AppState>();
    let mut guard = state
        .window_bounds_save_state
        .lock()
        .map_err(|error| error.to_string())?;
    guard.last_saved = Some(bounds);
    guard.last_save_at = Some(Instant::now());

    Ok(())
}

fn setup_window_bounds_persistence(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;

    let app_handle = app.clone();
    let event_window = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            if let Ok(bounds) = current_window_bounds(&event_window) {
                let _ = save_window_bounds_if_needed(&app_handle, bounds);
            }
        }
        WindowEvent::CloseRequested { .. } => {
            if let Ok(bounds) = current_window_bounds(&event_window) {
                let _ = save_window_bounds_if_needed(&app_handle, bounds);
            }
        }
        _ => {}
    });

    Ok(())
}

#[command]
fn start_pty(app: AppHandle, state: State<'_, AppState>, cols: u16, rows: u16, id: String, cwd: Option<String>) -> Result<String, String> {

    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;

    let shell_path = resolve_local_shell_path(&app);

    #[cfg(unix)]
    let mut command = {
        let mut command = CommandBuilder::new_default_prog();
        command.env("SHELL", &shell_path);
        command.env("TERM", "xterm-256color");
        command
    };

    #[cfg(windows)]
    let mut command = {
        let mut command = CommandBuilder::new(shell_path);
        command.env("TERM", "xterm-256color");
        command
    };

    // Set working directory if specified
    if let Some(dir) = cwd {
        if std::path::Path::new(&dir).is_dir() {
            command.cwd(&dir);
        }
    }

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

#[command]
fn get_startup_dir(state: State<'_, AppState>) -> Option<String> {
    state.startup_dir.lock().ok().and_then(|guard| guard.clone())
}

fn main() {
    // Parse command line arguments for startup directory
    let startup_dir = std::env::args()
        .nth(1)
        .filter(|arg| !arg.starts_with('-') && std::path::Path::new(arg).is_dir());

    let app_state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        window_bounds_save_state: Arc::new(Mutex::new(WindowBoundsSaveState {
            last_saved: None,
            last_save_at: None,
        })),
        startup_dir: Arc::new(Mutex::new(startup_dir)),
    };

    tauri::Builder::default()
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder};

                let new_window_item = MenuItem::with_id(_app, "menu-new-window", "New Window", true, Some("Cmd+N"))?;
                let close_window_item = MenuItem::with_id(_app, "menu-close-window", "Close Window", true, Some("Cmd+W"))?;
                let minimize_item = MenuItem::with_id(_app, "minimize", "Minimize", true, Some("Cmd+M"))?;
                let zoom_item = MenuItem::with_id(_app, "maximize", "Zoom", true, None::<&str>)?;

                let new_local_item = MenuItem::with_id(_app, "menu-new-local", "Local Shell", true, None::<&str>)?;
                let new_ssh_item = MenuItem::with_id(_app, "menu-new-ssh", "SSH", true, None::<&str>)?;
                let new_telnet_item = MenuItem::with_id(_app, "menu-new-telnet", "Telnet", true, None::<&str>)?;
                let new_serial_item = MenuItem::with_id(_app, "menu-new-serial", "Serial", true, None::<&str>)?;
                let close_tab_item = MenuItem::with_id(_app, "menu-close-tab", "Close Tab", true, None::<&str>)?;
                let settings_item = MenuItem::with_id(_app, "menu-open-settings", "Settings", true, None::<&str>)?;
                let toggle_bookmarks_item = MenuItem::with_id(_app, "menu-toggle-bookmarks", "Toggle Bookmarks", true, None::<&str>)?;
                let toggle_remote_files_item = MenuItem::with_id(_app, "menu-toggle-remote-files", "Toggle Remote Files", true, None::<&str>)?;
                let increase_font_size_item = MenuItem::with_id(
                    _app,
                    "menu-increase-font-size",
                    "Increase Terminal Font Size",
                    true,
                    Some("Cmd+="),
                )?;
                let decrease_font_size_item = MenuItem::with_id(
                    _app,
                    "menu-decrease-font-size",
                    "Decrease Terminal Font Size",
                    true,
                    Some("Cmd+-"),
                )?;
                let reset_font_size_item = MenuItem::with_id(
                    _app,
                    "menu-reset-font-size",
                    "Reset Terminal Font Size",
                    true,
                    Some("Cmd+0"),
                )?;
                let exit_item = MenuItem::with_id(_app, "exit", "Exit", true, None::<&str>)?;
                let about_item = MenuItem::with_id(_app, "about", "About AuraTerm", true, None::<&str>)?;
                let fullscreen_item = PredefinedMenuItem::fullscreen(_app, None)?;

                let undo_item = PredefinedMenuItem::undo(_app, None)?;
                let redo_item = PredefinedMenuItem::redo(_app, None)?;
                let cut_item = PredefinedMenuItem::cut(_app, None)?;
                let copy_item = PredefinedMenuItem::copy(_app, None)?;
                let paste_item = PredefinedMenuItem::paste(_app, None)?;
                let select_all_item = PredefinedMenuItem::select_all(_app, None)?;

                let new_session_menu = SubmenuBuilder::new(_app, "New Session")
                    .item(&new_local_item)
                    .item(&new_ssh_item)
                    .item(&new_telnet_item)
                    .item(&new_serial_item)
                    .build()?;

                let file_menu = SubmenuBuilder::new(_app, "File")
                    .item(&new_session_menu)
                    .item(&close_tab_item)
                    .separator()
                    .item(&settings_item)
                    .separator()
                    .item(&exit_item)
                    .build()?;
                let edit_menu = SubmenuBuilder::new(_app, "Edit")
                    .item(&undo_item)
                    .item(&redo_item)
                    .separator()
                    .item(&cut_item)
                    .item(&copy_item)
                    .item(&paste_item)
                    .item(&select_all_item)
                    .build()?;
                let view_menu = SubmenuBuilder::new(_app, "View")
                    .item(&toggle_bookmarks_item)
                    .item(&toggle_remote_files_item)
                    .separator()
                    .item(&increase_font_size_item)
                    .item(&decrease_font_size_item)
                    .item(&reset_font_size_item)
                    .separator()
                    .item(&fullscreen_item)
                    .build()?;
                let window_menu = SubmenuBuilder::new(_app, "Window")
                    .item(&new_window_item)
                    .item(&close_window_item)
                    .separator()
                    .item(&minimize_item)
                    .item(&zoom_item)
                    .build()?;
                let help_menu = SubmenuBuilder::new(_app, "Help")
                    .item(&about_item)
                    .build()?;
                let menu = MenuBuilder::new(_app)
                    .item(&file_menu)
                    .item(&edit_menu)
                    .item(&view_menu)
                    .item(&window_menu)
                    .item(&help_menu)
                    .build()?;

                _app.set_menu(menu)?;
            }

            if let Err(error) = apply_saved_window_bounds(_app.handle()) {
                eprintln!("failed to restore saved window bounds: {error}");
            }
            if let Err(error) = setup_window_bounds_persistence(_app.handle()) {
                eprintln!("failed to set up window bounds persistence: {error}");
            }

            Ok(())
        })
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "minimize" => {
                    if let Some(window) = app.webview_windows().values().next() {
                        let _ = window.minimize();
                    }
                }
                "maximize" => {
                    if let Some(window) = app.webview_windows().values().next() {
                        if let Ok(is_maximized) = window.is_maximized() {
                            if is_maximized {
                                let _ = window.unmaximize();
                            } else {
                                let _ = window.maximize();
                            }
                        }
                    }
                }
                "about" => {
                    let _ = app.emit("show-about", ());
                }
                "menu-new-window" => {
                    let _ = tauri::WebviewWindowBuilder::new(
                        app,
                        format!("window-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()),
                        tauri::WebviewUrl::App("index.html".into())
                    )
                    .title("AuraTerm")
                    .inner_size(1000.0, 800.0)
                    .decorations(false)
                    .shadow(true)
                    .visible(true)
                    .build();
                }
                "menu-close-window" => {
                    // In Tauri 2.0, on_menu_event is on the AppHandle, but we want to close the active window.
                    // For now, we'll close all windows if we can't determine the active one,
                    // or just the main one if it's the only one.
                    // A better way would be to track the focused window.
                    for window in app.webview_windows().values() {
                        let _ = window.close();
                    }
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
                "menu-increase-font-size" => {
                    let _ = app.emit("menu-increase-font-size", ());
                }
                "menu-decrease-font-size" => {
                    let _ = app.emit("menu-decrease-font-size", ());
                }
                "menu-reset-font-size" => {
                    let _ = app.emit("menu-reset-font-size", ());
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
            get_startup_dir,
            start_pty,
            write_pty_input,
            resize_pty,
            close_pty,
            ssh::start_ssh_pty,
            ssh::write_ssh_pty_input,
            ssh::resize_ssh_pty,
            ssh::close_ssh_pty,
            ssh::answer_ssh_mfa,
            ssh::answer_ssh_reconnect_choice,
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
