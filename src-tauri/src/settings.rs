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
            foreground: "#ffffff".to_string(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub font_size: u8,
    pub font_family: String,
    pub scrollback: u32,
    pub shell_path: Option<String>,
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
    /// Quick-action buttons shown below the terminal
    #[serde(default)]
    pub quick_buttons: Vec<QuickButton>,
    /// Most recently used serial connection parameters
    #[serde(default)]
    pub last_serial_config: Option<SerialHistoryItem>,
    /// Recent serial parameter presets inferred from usage history
    #[serde(default)]
    pub recent_serial_configs: Vec<SerialHistoryItem>,
}

fn default_true() -> bool { true }

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: 14,
            font_family: r#"Consolas, "Courier New", monospace"#.to_string(),
            scrollback: 1000,
            shell_path: None,
            theme: TerminalTheme::default(),
            ctrl_c_copy: true,
            ctrl_v_paste: true,
            middle_click_paste: true,
            quick_buttons: vec![],
            last_serial_config: None,
            recent_serial_configs: vec![],
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
