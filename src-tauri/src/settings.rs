use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTheme {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection_background: String,
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
    pub bright_black: String,
    pub bright_red: String,
    pub bright_green: String,
    pub bright_yellow: String,
    pub bright_blue: String,
    pub bright_magenta: String,
    pub bright_cyan: String,
    pub bright_white: String,
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self {
            background: "#000000".to_string(),
            foreground: "#dcdcdc".to_string(),
            cursor: "#ffffff".to_string(),
            selection_background: "#264f78".to_string(),
            black: "#1f252d".to_string(),
            red: "#c35b65".to_string(),
            green: "#7fb069".to_string(),
            yellow: "#d0b26f".to_string(),
            blue: "#6ca0d8".to_string(),
            magenta: "#a889d8".to_string(),
            cyan: "#5fb3b3".to_string(),
            white: "#dcdcdc".to_string(),
            bright_black: "#5c6370".to_string(),
            bright_red: "#e06c75".to_string(),
            bright_green: "#98c379".to_string(),
            bright_yellow: "#e5c07b".to_string(),
            bright_blue: "#61afef".to_string(),
            bright_magenta: "#c678dd".to_string(),
            bright_cyan: "#56b6c2".to_string(),
            bright_white: "#ffffff".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickButton {
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub toolbar: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub session_groups: Vec<String>,
    #[serde(default)]
    pub send_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub pattern: String,
    #[serde(default = "default_true")]
    pub is_regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_global_scope")]
    pub scope: String,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub bell: bool,
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub auto_response: Option<String>,
    #[serde(default = "default_rule_cooldown")]
    pub cooldown_ms: u64,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UiThemeMode {
    #[default]
    FollowTerminal,
    Light,
    Dark,
}

/// Terminal renderer backend. Mirrors the frontend `RendererMode` type
/// (`"auto" | "webgl" | "dom"`). The frontend owns the actual rendering
/// behaviour; this exists so the persisted settings schema stays in sync.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RendererMode {
    #[default]
    Auto,
    Webgl,
    Dom,
}

/// Interface language. Mirrors the frontend `AppLanguage` type
/// (`"system" | "en" | "zh-CN"`). `System` follows the OS/browser language and
/// is resolved to a concrete locale on the frontend.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl Language {
    /// Best-effort concrete locale for native UI (e.g. the macOS menubar) built
    /// before the frontend resolves `System`. Defaults to English.
    pub fn to_locale(self) -> &'static str {
        match self {
            Language::ZhCn => "zh-CN",
            Language::System | Language::En => "en",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Interface language; `System` follows the OS language.
    #[serde(default)]
    pub language: Language,
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
    #[serde(default)]
    pub ui_theme_mode: UiThemeMode,
    /// Terminal renderer backend: "auto" | "webgl" | "dom".
    #[serde(default)]
    pub renderer_mode: RendererMode,
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
    /// Shared output highlight and trigger rules.
    #[serde(default)]
    pub output_rules: Vec<OutputRule>,
    /// Automatically open the SFTP browser for the active SSH session.
    #[serde(default)]
    pub auto_open_sftp: bool,
    /// Destination directory for files received through inline Zmodem.
    #[serde(default = "default_zmodem_download_path")]
    pub zmodem_download_path: String,
    /// Most recently used serial connection parameters
    #[serde(default)]
    pub last_serial_config: Option<SerialHistoryItem>,
    /// Recent serial parameter presets inferred from usage history
    #[serde(default)]
    pub recent_serial_configs: Vec<SerialHistoryItem>,
    /// Input history for the terminal input bar (most recent first)
    #[serde(default)]
    pub input_history: Vec<String>,
    /// Whether to restore the previous session tabs on startup
    #[serde(default)]
    pub restore_tabs_on_startup: bool,
    /// Persisted split-pane layout state from the frontend
    #[serde(default)]
    pub pane_layout: Option<serde_json::Value>,
    /// Persisted workspace snapshot including tabs and pane layout
    #[serde(default)]
    pub workspace_state: Option<serde_json::Value>,
    /// Master password hash (Argon2 format)
    #[serde(default)]
    pub master_password_hash: Option<String>,
    /// Master password salt (Base64 encoded)
    #[serde(default)]
    pub master_password_salt: Option<String>,
    /// Whether credentials have been initialized with encryption
    #[serde(default)]
    pub credentials_initialized: bool,
    /// When a master password is set, whether to cache it in the OS keychain
    /// for silent auto-unlock on the next launch (macOS / Windows only).
    #[serde(default)]
    pub remember_master_password: bool,
}

fn default_true() -> bool { true }
fn default_global_scope() -> String { "global".to_string() }
fn default_rule_cooldown() -> u64 { 1000 }
fn default_log_save_path() -> String { "~/AuraTerm/logs".to_string() }
fn default_log_file_name_template() -> String { "{session}_{timestamp}".to_string() }
fn default_zmodem_download_path() -> String { "~/AuraTerm/downloads".to_string() }


impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::System,
            font_size: 15,
            font_family: r#"Consolas, "Courier New", monospace"#.to_string(),
            scrollback: 10000,
            shell_path: None,
            window_bounds: None,
            log_save_path: default_log_save_path(),
            log_file_name_template: default_log_file_name_template(),
            theme: TerminalTheme::default(),
            ui_theme_mode: UiThemeMode::FollowTerminal,
            renderer_mode: RendererMode::Auto,
            ctrl_c_copy: true,
            ctrl_v_paste: true,
            middle_click_paste: true,
            show_input_bar: true,
            quick_buttons: vec![],
            output_rules: vec![],
            auto_open_sftp: false,
            zmodem_download_path: default_zmodem_download_path(),
            last_serial_config: None,
            recent_serial_configs: vec![],
            input_history: vec![],
            restore_tabs_on_startup: false,
            pane_layout: None,
            workspace_state: None,
            master_password_hash: None,
            master_password_salt: None,
            credentials_initialized: false,
            remember_master_password: false,
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
    crate::util::write_atomic(&path, &content)?;
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

#[cfg(test)]
mod tests {
    use super::Settings;
    use std::{collections::BTreeSet, fs, path::PathBuf};
    use serde_json::json;

    fn frontend_settings_ts_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("src")
            .join("settings.ts")
    }

    fn load_frontend_app_settings_keys() -> BTreeSet<String> {
        let content = fs::read_to_string(frontend_settings_ts_path())
            .expect("frontend settings.ts should be readable during schema tests");
        let marker = "export interface AppSettings {";
        let start = content
            .find(marker)
            .map(|idx| idx + marker.len())
            .expect("AppSettings interface should exist in frontend settings.ts");
        let rest = &content[start..];
        let end = rest
            .find("\n}")
            .expect("AppSettings interface should have a closing brace");
        let block = &rest[..end];

        block
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty()
                    || trimmed.starts_with("/**")
                    || trimmed.starts_with('*')
                    || trimmed.starts_with("*/")
                    || trimmed.starts_with("//")
                {
                    return None;
                }

                let (name, _ty) = trimmed.split_once(':')?;
                let normalized_name = name.trim().trim_end_matches('?');
                if normalized_name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                    Some(normalized_name.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    fn serialized_settings_keys(settings: &Settings) -> BTreeSet<String> {
        serde_json::to_value(settings)
            .expect("settings should serialize to JSON")
            .as_object()
            .expect("settings JSON should be an object")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn settings_deserialize_input_history_from_frontend_shape() {
        let mut payload = serde_json::to_value(Settings::default())
            .expect("default settings should serialize to JSON");
        payload["inputHistory"] = json!(["ls", "pwd"]);

        let settings: Settings = serde_json::from_value(payload)
        .expect("settings should deserialize from frontend camelCase shape");

        assert_eq!(settings.input_history, vec!["ls", "pwd"]);
    }

    #[test]
    fn settings_default_log_template_matches_frontend_default() {
        assert_eq!(Settings::default().log_file_name_template, "{session}_{timestamp}");
    }

    #[test]
    fn settings_schema_keys_match_frontend_app_settings_interface() {
        let frontend_keys = load_frontend_app_settings_keys();
        let backend_keys = serialized_settings_keys(&Settings::default());

        assert_eq!(backend_keys, frontend_keys);
    }

    #[test]
    fn settings_round_trip_preserves_frontend_schema_defaults() {
        let frontend_keys = load_frontend_app_settings_keys();
        let serialized = serde_json::to_value(Settings::default())
            .expect("default settings should serialize to JSON");
        let round_tripped: Settings = serde_json::from_value(serialized.clone())
            .expect("default settings JSON should deserialize back into Settings");
        let reserialized = serde_json::to_value(round_tripped)
            .expect("round-tripped settings should serialize to JSON");

        assert_eq!(serialized, reserialized);
        assert_eq!(reserialized.as_object().expect("round-trip JSON should be an object").keys().cloned().collect::<BTreeSet<_>>(), frontend_keys);
        assert_eq!(reserialized["inputHistory"], json!([]));
        assert_eq!(reserialized["logFileNameTemplate"], json!("{session}_{timestamp}"));
    }
}
