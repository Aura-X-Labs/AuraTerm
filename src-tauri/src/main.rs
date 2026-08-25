// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine as _;
#[cfg(unix)]
use std::ffi::CStr;

use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{
    command, AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, Position,
    Size, State, WindowEvent,
};

#[macro_use]
mod logging;
mod account;
mod ai;
mod cloud_sync;
mod cloud_bridge;
mod e2ee;
mod pake;
mod assist;
mod assist_host;
mod assist_client;
mod bookmark_share;
mod connections;
mod encryption;
mod keychain;
mod pty_broker;
mod rfc2217;
mod serial;
mod serial_link;
mod serial_params;
mod settings;
mod ssh;
mod telnet;
mod terminal_event_hub;
mod shared_session;
mod util;
mod zmodem;

use pty_broker::{
    CloseReason, OpenTerminalRequest, PortablePtyAdapter, PtyBroker, TerminalSize, LOCAL_OWNER,
};
use terminal_event_hub::{TerminalEvent, TerminalEventHub};

#[derive(Clone)]
struct AppState {
    /// Owns every local PTY session; shared seam for the Tauri UI today and
    /// the Cloud Console agent later.
    broker: Arc<PtyBroker>,
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
    // Track the last reported inner size so we don't spam logs when the OS emits
    // redundant Resized events with the same dimensions (common on macOS).
    let last_logged_size = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            if let Ok(size) = event_window.inner_size() {
                let packed = ((size.width as u64) << 32) | (size.height as u64);
                let prev = last_logged_size.swap(packed, std::sync::atomic::Ordering::Relaxed);
                if prev != packed {
                    crate::debug_log!("[Window] New inner size: {}x{}", size.width, size.height);
                }
            }
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

/// Local Tauri UI adapter: subscribes a session's raw output on the event hub
/// and keeps the historical behaviour — Zmodem routing, streaming UTF-8
/// decode, `pty-output:<id>` / `pty-exit:<id>` events — on the UI side of the
/// raw-byte seam. Zmodem protocol responses are written back through the
/// broker as trusted local input.
fn attach_tauri_ui_adapter(
    app: &AppHandle,
    broker: &Arc<PtyBroker>,
    zmodem: &zmodem::ZmodemState,
    id: &str,
) {
    let app_handle = app.clone();
    let pty_id = id.to_string();
    let zmodem_state = zmodem.clone();
    let broker_weak = Arc::downgrade(broker);
    let mut decoder = util::Utf8StreamDecoder::new();
    broker.hub().subscribe(id, move |event| match event {
        TerminalEvent::Output(bytes) => {
            let (_, response) = util::pump_stream_chunk(
                &app_handle,
                &pty_id,
                &mut decoder,
                bytes,
                &zmodem_state,
            );
            if !response.is_empty() {
                if let Some(broker) = broker_weak.upgrade() {
                    let _ = broker.input(&pty_id, LOCAL_OWNER, &response, LOCAL_OWNER);
                }
            }
        }
        TerminalEvent::Exit(message) => {
            util::emit_pty_exit(&app_handle, &pty_id, message.clone());
        }
        TerminalEvent::Disconnected(_) | TerminalEvent::Reconnected => {}
    });
}

#[command]
fn start_pty(
    app: AppHandle,
    state: State<'_, AppState>,
    zmodem: State<'_, zmodem::ZmodemState>,
    cols: u16,
    rows: u16,
    id: String,
    cwd: Option<String>,
) -> Result<String, String> {
    let shell_path = resolve_local_shell_path(&app);
    zmodem.reset_session(&id);
    // Subscribe before opening so the first prompt bytes cannot be missed.
    attach_tauri_ui_adapter(&app, &state.broker, zmodem.inner(), &id);

    let request = OpenTerminalRequest {
        session_id: id.clone(),
        size: TerminalSize { cols, rows },
        shell_path,
        cwd,
    };
    if let Err(error) = state.broker.open(request) {
        state.broker.hub().drop_session(&id);
        return Err(error);
    }

    Ok(id)
}

#[command]
fn write_pty_input(state: State<'_, AppState>, id: String, data: String) -> Result<(), String> {
    state
        .broker
        .input(&id, LOCAL_OWNER, data.as_bytes(), LOCAL_OWNER)
        .map(|_| ())
}

#[command]
fn write_pty_bytes(state: State<'_, AppState>, id: String, data: Vec<u8>) -> Result<(), String> {
    state
        .broker
        .input(&id, LOCAL_OWNER, &data, LOCAL_OWNER)
        .map(|_| ())
}

#[command]
fn resize_pty(state: State<'_, AppState>, id: String, cols: u16, rows: u16) -> Result<(), String> {
    state
        .broker
        .resize(&id, TerminalSize { cols, rows }, LOCAL_OWNER)
}

#[command]
fn close_pty(
    state: State<'_, AppState>,
    zmodem: State<'_, zmodem::ZmodemState>,
    id: String,
) -> Result<(), String> {
    state.broker.close(&id, CloseReason::LocalRequest)?;
    zmodem.reset_session(&id);
    Ok(())
}

/// Credential-protection state the frontend queries on startup to decide
/// whether to prompt for a master password.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialSecurityState {
    /// A master password is configured (vs. the device-local-key default).
    password_enabled: bool,
    /// Credentials are accessible right now without prompting.
    unlocked: bool,
    /// The master password is cached in the OS keychain for auto-unlock.
    remember_enabled: bool,
    /// This platform supports keychain storage (macOS / Windows).
    remember_available: bool,
}

#[command]
fn get_credential_security_state(
    app: AppHandle,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<CredentialSecurityState, String> {
    let settings = settings::get_settings(app)?;
    let password_enabled = settings.master_password_hash.is_some();
    Ok(CredentialSecurityState {
        password_enabled,
        unlocked: !password_enabled || master_state.is_unlocked(),
        remember_enabled: settings.remember_master_password,
        remember_available: keychain::is_available(),
    })
}

/// Try to unlock silently from the OS keychain (the "remember" feature).
/// Returns whether credentials are now accessible. Local-key mode is always
/// accessible, so it returns `true` with nothing to do.
#[command]
fn try_auto_unlock(
    app: AppHandle,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<bool, String> {
    if master_state.is_unlocked() {
        return Ok(true);
    }
    let settings = settings::get_settings(app)?;
    let Some(stored_hash) = settings.master_password_hash else {
        return Ok(true); // 无密码模式，无需解锁
    };
    if !settings.remember_master_password || !keychain::is_available() {
        return Ok(false);
    }
    let Some(password) = keychain::load_password()? else {
        return Ok(false);
    };
    if encryption::verify_master_password(&password, &stored_hash)? {
        master_state.set(password);
        Ok(true)
    } else {
        // 钥匙串里的密码已失效（例如主密码被改），清掉避免反复失败。
        let _ = keychain::clear();
        Ok(false)
    }
}

/// Enable a master password from the default no-password (local-key) mode.
/// Existing credentials are re-encrypted under the new password — never wiped.
#[command]
fn set_master_password(
    app: AppHandle,
    password: String,
    remember: bool,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<(), String> {
    use rand::Rng;

    let mut settings = settings::get_settings(app.clone())?;
    if settings.master_password_hash.is_some() {
        return Err("A master password is already set; use change_master_password.".to_string());
    }

    // 迁移而非清空：用当前的本地密钥解出现有凭据，再用新主密码重加密。
    let local_secret =
        encryption::CredentialSecret::LocalKey(encryption::load_or_create_local_key(&app)?);
    let store = encryption::load_encrypted_credentials(&app, &local_secret)
        .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() });

    let mut rng = rand::thread_rng();
    let salt: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    let password_hash = encryption::hash_master_password(&password, &salt)
        .map_err(|e| format!("Failed to hash password: {}", e))?;
    let pw_secret =
        encryption::CredentialSecret::Password(zeroize::Zeroizing::new(password.clone()));
    encryption::save_encrypted_credentials(&app, &store, &pw_secret)?;

    let remember = remember && keychain::is_available();
    settings.master_password_hash = Some(password_hash);
    settings.master_password_salt = Some(base64::engine::general_purpose::STANDARD.encode(&salt));
    settings.credentials_initialized = true;
    settings.remember_master_password = remember;
    settings::save_settings(app.clone(), settings)?;

    if remember {
        keychain::store_password(&password)?;
    }
    master_state.set(password);
    Ok(())
}

/// Change an existing master password (re-encrypts the credential store).
#[command]
fn change_master_password(
    app: AppHandle,
    current_password: String,
    new_password: String,
    remember: bool,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<(), String> {
    use rand::Rng;

    let mut settings = settings::get_settings(app.clone())?;
    let Some(stored_hash) = settings.master_password_hash.clone() else {
        return Err("No master password is set.".to_string());
    };
    if !encryption::verify_master_password(&current_password, &stored_hash)? {
        return Err("Incorrect master password".to_string());
    }

    let old_secret =
        encryption::CredentialSecret::Password(zeroize::Zeroizing::new(current_password));
    let store = encryption::load_encrypted_credentials(&app, &old_secret)
        .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() });

    let mut rng = rand::thread_rng();
    let salt: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    let new_hash = encryption::hash_master_password(&new_password, &salt)?;
    let new_secret =
        encryption::CredentialSecret::Password(zeroize::Zeroizing::new(new_password.clone()));
    encryption::save_encrypted_credentials(&app, &store, &new_secret)?;

    let remember = remember && keychain::is_available();
    settings.master_password_hash = Some(new_hash);
    settings.master_password_salt = Some(base64::engine::general_purpose::STANDARD.encode(&salt));
    settings.remember_master_password = remember;
    settings::save_settings(app.clone(), settings)?;

    if remember {
        keychain::store_password(&new_password)?;
    } else {
        let _ = keychain::clear();
    }
    // Drop the previous password's memoized derived keys.
    encryption::clear_kdf_cache();
    master_state.set(new_password);
    Ok(())
}

/// Remove the master password, switching back to device-local-key mode.
/// Existing credentials are re-encrypted under the local key — never wiped.
#[command]
fn disable_master_password(
    app: AppHandle,
    current_password: String,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(app.clone())?;
    let Some(stored_hash) = settings.master_password_hash.clone() else {
        return Ok(()); // 已是无密码模式
    };
    if !encryption::verify_master_password(&current_password, &stored_hash)? {
        return Err("Incorrect master password".to_string());
    }

    let pw_secret =
        encryption::CredentialSecret::Password(zeroize::Zeroizing::new(current_password));
    let store = encryption::load_encrypted_credentials(&app, &pw_secret)
        .unwrap_or_else(|_| encryption::CredentialStore { credentials: Vec::new() });
    let local_secret =
        encryption::CredentialSecret::LocalKey(encryption::load_or_create_local_key(&app)?);
    encryption::save_encrypted_credentials(&app, &store, &local_secret)?;

    settings.master_password_hash = None;
    settings.master_password_salt = None;
    settings.remember_master_password = false;
    settings::save_settings(app.clone(), settings)?;
    let _ = keychain::clear();
    master_state.clear();
    encryption::clear_kdf_cache();
    Ok(())
}

/// Toggle whether the master password is cached in the OS keychain.
#[command]
fn set_remember_master_password(
    app: AppHandle,
    enabled: bool,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(app.clone())?;
    if settings.master_password_hash.is_none() {
        return Err("No master password is set.".to_string());
    }
    if enabled {
        if !keychain::is_available() {
            return Err("Keychain is not available on this platform.".to_string());
        }
        let password = master_state.get()?;
        keychain::store_password(&password)?;
        settings.remember_master_password = true;
    } else {
        let _ = keychain::clear();
        settings.remember_master_password = false;
    }
    settings::save_settings(app.clone(), settings)?;
    Ok(())
}

#[command]
fn verify_master_password(
    app: AppHandle,
    password: String,
    remember: bool,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<bool, String> {
    let mut settings = settings::get_settings(app.clone())?;

    let Some(stored_hash) = settings.master_password_hash.clone() else {
        return Err("Master password not set".to_string());
    };

    let ok = encryption::verify_master_password(&password, &stored_hash)?;
    if ok {
        if remember && keychain::is_available() {
            let _ = keychain::store_password(&password);
            settings.remember_master_password = true;
            let _ = settings::save_settings(app.clone(), settings);
        }
        master_state.set(password);
    }
    Ok(ok)
}

/// 查询当前会话主密码是否已解锁。
/// 主要用于前端在路由切换或重新挂载时判断是否需要再次提示用户输入。
#[command]
fn is_master_password_unlocked(
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<bool, String> {
    Ok(master_state.is_unlocked())
}

/// 锁定主密码（清除会话缓存），下次访问凭据需要重新输入主密码。
#[command]
fn lock_master_password(
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<(), String> {
    master_state.clear();
    encryption::clear_kdf_cache();
    Ok(())
}

#[command]
fn export_connections(
    app: AppHandle,
    password: String,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<String, String> {
    // 备份用用户提供的备份密码加密；读取本地库用当前解锁方式（主密码或本地密钥）。
    let secret = encryption::resolve_secret(&app, &master_state)?;
    encryption::export_credentials_backup(&app, &password, &secret)
}

#[command]
fn import_connections(
    app: AppHandle,
    password: String,
    encrypted_data: String,
    master_state: State<'_, encryption::MasterPasswordState>,
) -> Result<(), String> {
    let secret = encryption::resolve_secret(&app, &master_state)?;
    encryption::import_credentials_backup(&app, &password, &encrypted_data, &secret)
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

/// Everything the native menubar renders beyond static labels: the UI locale
/// plus the Cloud menu's account / toggle state. The frontend keeps it fresh
/// via `set_menu_language` and `sync_cloud_menu_state`; each update rebuilds
/// the menu (macOS only), which is how checkmarks and the Sign In / My
/// Account swap stay canonical.
#[derive(Default)]
struct MenuModel {
    locale: String,
    cloud_signed_in: bool,
    cloud_console_on: bool,
    cloud_remote_send_on: bool,
}

struct MenuModelState(Mutex<MenuModel>);

/// Build the native macOS menubar from the menu model (`locale` is `"en"` or
/// `"zh-CN"`; anything else falls back to English). Keep the item IDs and
/// accelerators stable across languages — only visible text and check state
/// change.
#[cfg(target_os = "macos")]
fn build_app_menu(
    app: &AppHandle,
    model: &MenuModel,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder};

    let zh = model.locale == "zh-CN";
    // Pick the Chinese label when the locale is zh-CN, otherwise English.
    let l = |en: &'static str, cn: &'static str| -> &'static str { if zh { cn } else { en } };

    let new_window_item = MenuItem::with_id(app, "menu-new-window", l("New Window", "新建窗口"), true, Some("Cmd+N"))?;
    let close_window_item = MenuItem::with_id(app, "menu-close-window", l("Close Window", "关闭窗口"), true, Some("Cmd+W"))?;
    let minimize_item = MenuItem::with_id(app, "minimize", l("Minimize", "最小化"), true, Some("Cmd+M"))?;
    let zoom_item = MenuItem::with_id(app, "maximize", l("Zoom", "缩放"), true, None::<&str>)?;

    let new_local_item = MenuItem::with_id(app, "menu-new-local", l("Local Shell", "本地终端"), true, None::<&str>)?;
    let new_ssh_item = MenuItem::with_id(app, "menu-new-ssh", l("SSH", "SSH"), true, None::<&str>)?;
    let new_telnet_item = MenuItem::with_id(app, "menu-new-telnet", l("Telnet", "Telnet"), true, None::<&str>)?;
    let new_serial_item = MenuItem::with_id(app, "menu-new-serial", l("Serial", "串口"), true, None::<&str>)?;
    let new_rfc2217_item = MenuItem::with_id(app, "menu-new-rfc2217", l("Network Serial", "网络串口"), true, None::<&str>)?;
    let new_raw_tcp_item = MenuItem::with_id(app, "menu-new-raw-tcp", l("Raw TCP", "裸 TCP"), true, None::<&str>)?;
    let close_tab_item = MenuItem::with_id(app, "menu-close-tab", l("Close Tab", "关闭标签页"), true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "menu-open-settings", l("Settings", "设置"), true, None::<&str>)?;

    // Cloud menu: account → sync → monitoring. One account entry whose label
    // follows the sign-in state; the two toggles render as checkmarks.
    let account_item = MenuItem::with_id(
        app,
        "menu-open-account",
        if model.cloud_signed_in { l("My Account…", "我的账户…") } else { l("Sign In…", "登录…") },
        true,
        None::<&str>,
    )?;
    let sync_now_item = MenuItem::with_id(app, "menu-sync-now", l("Sync Now", "立即同步"), true, None::<&str>)?;
    let sync_settings_item = MenuItem::with_id(app, "menu-open-cloud-sync", l("Sync Settings…", "同步设置…"), true, None::<&str>)?;
    let cloud_console_item = CheckMenuItem::with_id(
        app,
        "menu-toggle-cloud-console",
        "Cloud Console",
        true,
        model.cloud_console_on,
        None::<&str>,
    )?;
    let remote_send_item = CheckMenuItem::with_id(
        app,
        "menu-toggle-remote-send",
        l("Allow Remote Send", "允许远程发送"),
        true,
        model.cloud_remote_send_on,
        None::<&str>,
    )?;
    let remote_assist_item = MenuItem::with_id(app, "menu-remote-assist", l("Live Share…", "Live Share…"), true, None::<&str>)?;
    let join_assist_item = MenuItem::with_id(app, "menu-join-assist", l("Join Live Share…", "加入 Live Share…"), true, None::<&str>)?;

    let toggle_bookmarks_item = MenuItem::with_id(app, "menu-toggle-bookmarks", l("Toggle Bookmarks", "切换书签栏"), true, None::<&str>)?;
    let bookmark_manager_item = MenuItem::with_id(app, "menu-bookmark-manager", l("Bookmark Manager…", "书签管理…"), true, None::<&str>)?;
    let command_palette_item = MenuItem::with_id(app, "menu-open-command-palette", l("Command Palette", "命令面板"), true, Some("Cmd+Shift+P"))?;
    let toggle_remote_files_item = MenuItem::with_id(app, "menu-toggle-remote-files", l("Toggle Remote Files", "切换远程文件"), true, None::<&str>)?;
    let toggle_tunnels_item = MenuItem::with_id(app, "menu-toggle-tunnels", l("Port Forwarding…", "端口转发…"), true, None::<&str>)?;
    let increase_font_size_item = MenuItem::with_id(app, "menu-increase-font-size", l("Increase Terminal Font Size", "增大终端字号"), true, Some("Cmd+="))?;
    let decrease_font_size_item = MenuItem::with_id(app, "menu-decrease-font-size", l("Decrease Terminal Font Size", "减小终端字号"), true, Some("Cmd+-"))?;
    let reset_font_size_item = MenuItem::with_id(app, "menu-reset-font-size", l("Reset Terminal Font Size", "重置终端字号"), true, Some("Cmd+0"))?;
    let exit_item = MenuItem::with_id(app, "exit", l("Exit", "退出"), true, None::<&str>)?;
    let about_item = MenuItem::with_id(app, "about", l("About AuraTerm", "关于 AuraTerm"), true, None::<&str>)?;
    let fullscreen_item = PredefinedMenuItem::fullscreen(app, None)?;

    let undo_item = PredefinedMenuItem::undo(app, None)?;
    let redo_item = PredefinedMenuItem::redo(app, None)?;
    let cut_item = PredefinedMenuItem::cut(app, None)?;
    let copy_item = PredefinedMenuItem::copy(app, None)?;
    let paste_item = PredefinedMenuItem::paste(app, None)?;
    let select_all_item = PredefinedMenuItem::select_all(app, None)?;

    let new_session_menu = SubmenuBuilder::new(app, l("New Session", "新建会话"))
        .item(&new_local_item)
        .item(&new_ssh_item)
        .item(&new_telnet_item)
        .item(&new_serial_item)
        .item(&new_rfc2217_item)
        .item(&new_raw_tcp_item)
        .build()?;

    let file_menu = SubmenuBuilder::new(app, l("File", "文件"))
        .item(&new_session_menu)
        .item(&close_tab_item)
        .separator()
        .item(&settings_item)
        .separator()
        .item(&exit_item)
        .build()?;
    let edit_menu = SubmenuBuilder::new(app, l("Edit", "编辑"))
        .item(&undo_item)
        .item(&redo_item)
        .separator()
        .item(&cut_item)
        .item(&copy_item)
        .item(&paste_item)
        .item(&select_all_item)
        .build()?;
    let view_menu = SubmenuBuilder::new(app, l("View", "视图"))
        .item(&toggle_bookmarks_item)
        .item(&bookmark_manager_item)
        .item(&command_palette_item)
        .separator()
        .item(&increase_font_size_item)
        .item(&decrease_font_size_item)
        .item(&reset_font_size_item)
        .separator()
        .item(&fullscreen_item)
        .build()?;
    let tools_menu = SubmenuBuilder::new(app, l("Tools", "工具"))
        .item(&toggle_remote_files_item)
        .item(&toggle_tunnels_item)
        .build()?;
    let cloud_menu = SubmenuBuilder::new(app, l("Cloud", "云服务"))
        .item(&account_item)
        .separator()
        .item(&sync_now_item)
        .item(&sync_settings_item)
        .separator()
        .item(&cloud_console_item)
        .item(&remote_send_item)
        .separator()
        .item(&remote_assist_item)
        .item(&join_assist_item)
        .build()?;
    let window_menu = SubmenuBuilder::new(app, l("Window", "窗口"))
        .item(&new_window_item)
        .item(&close_window_item)
        .separator()
        .item(&minimize_item)
        .item(&zoom_item)
        .build()?;
    let help_menu = SubmenuBuilder::new(app, l("Help", "帮助"))
        .item(&about_item)
        .build()?;
    MenuBuilder::new(app)
        .item(&file_menu)
        .item(&edit_menu)
        .item(&view_menu)
        .item(&tools_menu)
        .item(&cloud_menu)
        .item(&window_menu)
        .item(&help_menu)
        .build()
}

/// Rebuild the native menubar from the current menu model. No-op on
/// platforms without a native app menu.
fn rebuild_app_menu(app: &AppHandle, state: &MenuModelState) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let menu = {
            let model = state.0.lock().map_err(|e| e.to_string())?;
            build_app_menu(app, &model).map_err(|e| e.to_string())?
        };
        app.set_menu(menu).map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
    }
    Ok(())
}

/// Rebuild the native menubar in the given locale. Invoked by the frontend
/// whenever the UI language changes (and on startup to resolve `System`).
#[command]
fn set_menu_language(
    app: AppHandle,
    state: State<'_, MenuModelState>,
    locale: String,
) -> Result<(), String> {
    state.0.lock().map_err(|e| e.to_string())?.locale = locale;
    rebuild_app_menu(&app, &state)
}

/// Keep the native Cloud menu in sync with the frontend: sign-in state picks
/// the account item's label, the two booleans drive the checkmarks. Invoked
/// on startup and after every auth change or toggle.
#[command]
fn sync_cloud_menu_state(
    app: AppHandle,
    state: State<'_, MenuModelState>,
    signed_in: bool,
    console_on: bool,
    remote_send_on: bool,
) -> Result<(), String> {
    {
        let mut model = state.0.lock().map_err(|e| e.to_string())?;
        model.cloud_signed_in = signed_in;
        model.cloud_console_on = console_on;
        model.cloud_remote_send_on = remote_send_on;
    }
    rebuild_app_menu(&app, &state)
}

fn main() {
    // Parse command line arguments for startup directory
    let startup_dir = std::env::args()
        .nth(1)
        .filter(|arg| !arg.starts_with('-') && std::path::Path::new(arg).is_dir());

    let event_hub = Arc::new(TerminalEventHub::new());
    let serial_state = serial::SerialState::new(event_hub.clone());
    let ssh_state = ssh::SshState::new(event_hub.clone());
    let telnet_state = telnet::TelnetState::new(event_hub.clone());
    let app_state = AppState {
        broker: Arc::new(PtyBroker::new(Box::new(PortablePtyAdapter), event_hub.clone())),
        window_bounds_save_state: Arc::new(Mutex::new(WindowBoundsSaveState {
            last_saved: None,
            last_save_at: None,
        })),
        startup_dir: Arc::new(Mutex::new(startup_dir)),
    };
    let shared_port: Arc<dyn shared_session::SharedSessionPort> = Arc::new(
        shared_session::UnifiedSharedSessionPort::new(
            event_hub.clone(), serial_state.clone(), ssh_state.clone(),
            telnet_state.clone(), app_state.broker.clone(),
        ),
    );

    tauri::Builder::default()
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            {
                // Seed the menu model from persisted settings and build the
                // native menubar. `System` resolves to English here; the
                // frontend re-applies the actual locale via
                // `set_menu_language` and pushes the sign-in state via
                // `sync_cloud_menu_state` once it loads (see App.vue).
                let saved = settings::get_settings(_app.handle().clone()).ok();
                let state = _app.state::<MenuModelState>();
                if let Ok(mut model) = state.0.lock() {
                    model.locale = saved
                        .as_ref()
                        .map(|settings| settings.language.to_locale().to_string())
                        .unwrap_or_else(|| "en".to_string());
                    model.cloud_console_on = saved
                        .as_ref()
                        .is_some_and(|settings| settings.auto_share_to_cloud);
                    model.cloud_remote_send_on = saved
                        .as_ref()
                        .is_none_or(|settings| settings.allow_remote_send);
                }
                rebuild_app_menu(_app.handle(), &state)?;
            }

            if let Err(error) = apply_saved_window_bounds(_app.handle()) {
                crate::warn_log!("failed to restore saved window bounds: {error}");
            }
            if let Err(error) = setup_window_bounds_persistence(_app.handle()) {
                crate::warn_log!("failed to set up window bounds persistence: {error}");
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
                        format!("window-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())),
                        tauri::WebviewUrl::App("index.html?role=child".into())
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
                "exit" => {
                    app.exit(0);
                }
                // Every other `menu-*` id is forwarded verbatim as an app
                // event; `useAppEventListeners.ts` maps events to handlers.
                // (A per-id list here silently dropped newly added items.)
                id if id.starts_with("menu-") => {
                    let _ = app.emit(id, ());
                }
                _ => {}
            }
        })
        .manage(MenuModelState(Mutex::new(MenuModel::default())))
        .manage(app_state)
        .manage(ssh_state)
        .manage(ssh::ForwardingState::default())
        .manage(telnet_state)
        .manage(serial_state)
        .manage(cloud_bridge::CloudBridgeState::new(shared_port))
        .manage(assist_client::AssistClientState::default())
        .manage(zmodem::ZmodemState::default())
        .manage(encryption::MasterPasswordState::default())
        .manage(connections::ImportPlanState::default())
        .manage(cloud_sync::SyncState::default())
        .manage(ai::AiState::default())
        .invoke_handler(tauri::generate_handler![
            get_version_info,
            get_startup_dir,
            set_menu_language,
            sync_cloud_menu_state,
            start_pty,
            write_pty_input,
            write_pty_bytes,
            resize_pty,
            close_pty,
            ssh::start_ssh_pty,
            ssh::ssh_generate_key_pair,
            ssh::write_ssh_pty_input,
            ssh::write_ssh_pty_bytes,
            ssh::resize_ssh_pty,
            ssh::close_ssh_pty,
            ssh::answer_ssh_mfa,
            ssh::answer_ssh_host_key_mismatch,
            ssh::ssh_list_known_hosts,
            ssh::ssh_delete_known_host,
            ssh::ssh_reset_known_hosts,
            ssh::answer_ssh_reconnect_choice,
            ssh::rename_ssh_session,
            ssh::ssh_list_remote_dir,
            ssh::ssh_create_remote_dir,
            ssh::ssh_remove_remote_entry,
            ssh::ssh_read_remote_text_file,
            ssh::ssh_write_remote_text_file,
            ssh::ssh_upload_file,
            ssh::ssh_download_file,
            ssh::ssh_start_tunnel,
            ssh::ssh_stop_tunnel,
            ssh::ssh_list_tunnels,
            telnet::start_telnet_session,
            telnet::write_telnet_input,
            telnet::write_telnet_bytes,
            telnet::resize_telnet,
            telnet::close_telnet_session,
            serial::list_serial_ports,
            serial::start_serial_session,
            serial::write_serial_input,
            serial::write_serial_bytes,
            serial::close_serial_session,
            serial::get_serial_status,
            serial::set_serial_params,
            serial::send_serial_break,
            serial::set_serial_signals,
            serial::purge_serial_buffers,
            cloud_bridge::cloud_bridge_rotate_credential,
            cloud_bridge::cloud_bridge_share_session,
            cloud_bridge::cloud_bridge_stop_share,
            cloud_bridge::cloud_bridge_status,
            cloud_bridge::cloud_bridge_set_allow_remote_send,
            cloud_bridge::cloud_bridge_report_size,
            assist_host::assist_start,
            assist_host::assist_stop,
            assist_host::assist_status,
            assist_host::assist_respond_join,
            assist_host::assist_set_role,
            assist_host::assist_kick,
            assist_host::assist_revoke_all_control,
            assist_host::assist_switch_session,
            assist_host::assist_set_follow_active_tab,
            assist_host::assist_extend,
            assist_client::assist_join,
            assist_client::write_assist_input,
            assist_client::assist_request_control,
            assist_client::assist_release_control,
            assist_client::close_assist_session,
            zmodem::zmodem_start_send,
            zmodem::zmodem_cancel,
            settings::get_settings,
            settings::save_settings,
            get_credential_security_state,
            try_auto_unlock,
            set_master_password,
            change_master_password,
            disable_master_password,
            set_remember_master_password,
            verify_master_password,
            is_master_password_unlocked,
            lock_master_password,
            export_connections,
            import_connections,
            connections::get_connections,
            connections::save_connection,
            connections::delete_connection,
            connections::touch_connection,
            connections::import_bookmarks,
            connections::preview_bookmark_import,
            connections::retarget_bookmark_import,
            connections::discard_bookmark_import,
            connections::apply_bookmark_import,
            connections::export_bookmarks,
            connections::export_group_bookmarks,
            bookmark_share::create_bookmark_share,
            bookmark_share::redeem_bookmark_share,
            bookmark_share::list_bookmark_shares,
            bookmark_share::revoke_bookmark_share,
            connections::delete_connections,
            connections::move_connections,
            connections::rename_group,
            connections::duplicate_connection,
            cloud_sync::get_sync_config,
            cloud_sync::set_sync_config,
            cloud_sync::set_sync_passphrase,
            cloud_sync::lock_sync_passphrase,
            cloud_sync::is_sync_unlocked,
            cloud_sync::cloud_sync_push,
            cloud_sync::cloud_sync_pull,
            cloud_sync::cloud_sync_now,
            cloud_sync::cloud_sync_test_connection,
            cloud_sync::auraxlab_request_email_code,
            cloud_sync::auraxlab_verify_email_code,
            cloud_sync::auraxlab_register,
            account::auraxlab_account_state,
            account::auraxlab_account_refresh,
            account::auraxlab_account_restore,
            account::auraxlab_account_login,
            account::auraxlab_account_logout,
            account::auraxlab_account_enable_console,
            account::auraxlab_account_pause_console,
            ai::ai_chat_start,
            ai::ai_chat_cancel,
            ai::ai_complete,
            ai::ai_test_connection,
            ai::ai_set_api_key,
            ai::ai_clear_api_key,
            ai::ai_has_api_key,
            save_terminal_log,
            append_to_log,
        ])
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
