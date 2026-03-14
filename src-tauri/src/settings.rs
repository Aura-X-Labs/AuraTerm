use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalTheme {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self {
            background: "#000000".to_string(),
            foreground: "#dcdcdc".to_string(),
            cursor: "#ffffff".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickButton {
    pub id: String,
    pub label: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialHistoryItem {
    pub id: String,
    pub name: String,
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: String,
    pub flow_control: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub font_size: u8,
    pub font_family: String,
    pub scrollback: u32,
    pub shell_path: Option<String>,
    #[serde(default)]
    pub window_bounds: Option<WindowBounds>,
    /// Default directory for session logs
    #[serde(default = "default_log_save_path")]
    pub log_save_path: String,
    /// Default filename template for session logs
    #[serde(default = "default_log_file_name_template")]
    pub log_file_name_template: String,
    pub theme: TerminalTheme,
    /// Ctrl+C copies selection to clipboard when text is selected
    #[serde(default = "default_true")]
    pub ctrl_c_copy: bool,
    /// Ctrl+V pastes clipboard content into the terminal
    #[serde(default = "default_true")]
    pub ctrl_v_paste: bool,
    /// Middle mouse button pastes clipboard content into the terminal
    #[serde(default = "default_true")]
    pub middle_click_paste: bool,
    /// Whether to show the input bar below the terminal
    #[serde(default = "default_true")]
    pub show_input_bar: bool,
    /// Quick-action buttons shown below the terminal
    #[serde(default)]
    pub quick_buttons: Vec<QuickButton>,
    /// Most recently used serial connection parameters
    #[serde(default)]
    pub last_serial_config: Option<SerialHistoryItem>,
    /// Recent serial parameter presets inferred from usage history
    #[serde(default)]
    pub recent_serial_configs: Vec<SerialHistoryItem>,
    /// Whether to restore the previous session tabs on startup
    #[serde(default)]
    pub restore_tabs_on_startup: bool,
    /// Persisted split-pane layout state from the frontend
    #[serde(default)]
    pub pane_layout: Option<serde_json::Value>,
    /// Persisted workspace snapshot including tabs and pane layout
    #[serde(default)]
    pub workspace_state: Option<serde_json::Value>,
}

fn default_true() -> bool { true }
fn default_log_save_path() -> String { "~/AuraTerm/logs".to_string() }
fn default_log_file_name_template() -> String { "{timestamp}_{session}".to_string() }


impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 15,
            font_family: r#"Consolas, "Courier New", monospace"#.to_string(),
            scrollback: 10000,
            shell_path: None,
            window_bounds: None,
            log_save_path: default_log_save_path(),
            log_file_name_template: default_log_file_name_template(),
            theme: TerminalTheme::default(),
            ctrl_c_copy: true,
            ctrl_v_paste: true,
            middle_click_paste: true,
            show_input_bar: true,
            quick_buttons: vec![],
            last_serial_config: None,
            recent_serial_configs: vec![],
            restore_tabs_on_startup: false,
            pane_layout: None,
            workspace_state: None,
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    Ok(config_dir.join("settings.json"))
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    let path = settings_path(&app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    // Merge loaded settings with defaults to tolerate missing fields
    let loaded: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let default_value = serde_json::to_value(Settings::default()).map_err(|e| e.to_string())?;
    let merged = merge_json(default_value, loaded);
    serde_json::from_value(merged).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let path = settings_path(&app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}

/// Recursively merge `patch` into `base`, returning the merged value.
fn merge_json(base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    match (base, patch) {
        (serde_json::Value::Object(mut base_map), serde_json::Value::Object(patch_map)) => {
            for (key, patch_val) in patch_map {
                let base_val = base_map.remove(&key).unwrap_or(serde_json::Value::Null);
                base_map.insert(key, merge_json(base_val, patch_val));
            }
            serde_json::Value::Object(base_map)
        }
        (_base, patch) => patch,
    }
}
