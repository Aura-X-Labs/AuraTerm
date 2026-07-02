//! AI assistant backend.
//!
//! Streams chat completions from either the Anthropic Messages API or any
//! OpenAI-compatible Chat Completions endpoint (DeepSeek, Kimi, Ollama, …).
//! Responses are forwarded to the frontend as incremental `ai-stream` events
//! keyed by request id, mirroring the established `pty-output` event flow.
//!
//! The API key is stored in its own file, AES-256-GCM encrypted under the
//! device-local key (`credentials.key`). It deliberately does *not* live in
//! the credential store: that store rides along with encrypted cloud sync and
//! backup export, and the AI key must never leave this machine.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use zeroize::Zeroizing;

use crate::encryption;
use crate::settings::{self, AiConfig};

const API_KEY_FILE: &str = "ai_api_key.enc";
const DEFAULT_ANTHROPIC_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;
/// Cap a provider error body so a misconfigured endpoint returning an HTML
/// page does not flood the UI.
const MAX_ERROR_BODY: usize = 600;

#[derive(Default)]
pub struct AiState {
    active: Arc<Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AiStreamEvent {
    request_id: String,
    /// "delta" | "done" | "error" | "cancelled"
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
}

fn emit_stream_event(app: &AppHandle, event: AiStreamEvent) {
    let _ = app.emit("ai-stream", event);
}

fn emit_delta(app: &AppHandle, request_id: &str, text: String) {
    emit_stream_event(app, AiStreamEvent {
        request_id: request_id.to_string(),
        kind: "delta",
        text: Some(text),
        message: None,
        input_tokens: None,
        output_tokens: None,
    });
}

fn emit_done(app: &AppHandle, request_id: &str, usage: &StreamUsage) {
    emit_stream_event(app, AiStreamEvent {
        request_id: request_id.to_string(),
        kind: "done",
        text: None,
        message: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
    });
}

fn emit_error(app: &AppHandle, request_id: &str, message: String) {
    emit_stream_event(app, AiStreamEvent {
        request_id: request_id.to_string(),
        kind: "error",
        text: None,
        message: Some(message),
        input_tokens: None,
        output_tokens: None,
    });
}

// ---------------------------------------------------------------------------
// API key storage
// ---------------------------------------------------------------------------

fn api_key_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(config_dir.join(API_KEY_FILE))
}

fn load_api_key(app: &AppHandle) -> Result<Zeroizing<String>, String> {
    let path = api_key_path(app)?;
    if !path.exists() {
        return Err("AI API key is not configured".to_string());
    }
    let encrypted = fs::read(&path).map_err(|e| format!("Failed to read AI API key: {}", e))?;
    let key = encryption::load_or_create_local_key(app)?;
    let plaintext = Zeroizing::new(encryption::decrypt_data(&encrypted, &key)?);
    String::from_utf8(plaintext.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| "Stored AI API key is corrupt".to_string())
}

#[tauri::command]
pub fn ai_set_api_key(app: AppHandle, api_key: String) -> Result<(), String> {
    let api_key = Zeroizing::new(api_key);
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("API key must not be empty".to_string());
    }
    let path = api_key_path(&app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let key = encryption::load_or_create_local_key(&app)?;
    let encrypted = encryption::encrypt_data(trimmed.as_bytes(), &key)?;
    fs::write(&path, &encrypted).map_err(|e| format!("Failed to write AI API key: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[tauri::command]
pub fn ai_clear_api_key(app: AppHandle) -> Result<(), String> {
    let path = api_key_path(&app)?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("Failed to remove AI API key: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub fn ai_has_api_key(app: AppHandle) -> bool {
    api_key_path(&app).map(|p| p.exists()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Request plumbing
// ---------------------------------------------------------------------------

fn http_client() -> Result<reqwest::Client, String> {
    // Connect timeout only: an overall timeout would kill long streams. The
    // user can always cancel a hung request from the UI (`ai_chat_cancel`).
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

fn trimmed_base(base: &Option<String>, default: &str) -> String {
    let base = base
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default);
    base.trim_end_matches('/').to_string()
}

/// Build the provider-specific streaming request. Returns the prepared
/// request plus which SSE dialect the response will speak.
fn build_stream_request(
    client: &reqwest::Client,
    config: &AiConfig,
    api_key: &str,
    system: &Option<String>,
    messages: &[AiMessage],
    stream: bool,
) -> Result<(reqwest::RequestBuilder, Dialect), String> {
    let max_tokens = config.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS).max(1);
    match config.provider.as_str() {
        "anthropic" => {
            let url = format!("{}/v1/messages", trimmed_base(&config.base_url, DEFAULT_ANTHROPIC_BASE));
            let mut body = json!({
                "model": config.model,
                "max_tokens": max_tokens,
                "stream": stream,
                "messages": messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect::<Vec<_>>(),
            });
            if let Some(system) = system.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                // Block form so the stable system prompt is a prompt-cache
                // breakpoint once conversations grow past the cacheable minimum.
                body["system"] = json!([{
                    "type": "text",
                    "text": system,
                    "cache_control": {"type": "ephemeral"},
                }]);
            }
            let request = client
                .post(url)
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body);
            Ok((request, Dialect::Anthropic))
        }
        "openai-compatible" => {
            let base = config
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Base URL is required for OpenAI-compatible providers".to_string())?;
            let url = format!("{}/chat/completions", base.trim_end_matches('/'));
            let mut all_messages: Vec<Value> = Vec::with_capacity(messages.len() + 1);
            if let Some(system) = system.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                all_messages.push(json!({"role": "system", "content": system}));
            }
            all_messages.extend(messages.iter().map(|m| json!({"role": m.role, "content": m.content})));
            let body = json!({
                "model": config.model,
                "max_tokens": max_tokens,
                "stream": stream,
                "messages": all_messages,
            });
            let request = client
                .post(url)
                .header("authorization", format!("Bearer {}", api_key))
                .json(&body);
            Ok((request, Dialect::OpenAi))
        }
        other => Err(format!("Unknown AI provider: {}", other)),
    }
}

#[derive(Clone, Copy)]
enum Dialect {
    Anthropic,
    OpenAi,
}

#[derive(Default)]
struct StreamUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// What a single SSE `data:` payload means for the stream consumer.
enum SseAction {
    Delta(String),
    Done,
    Error(String),
    Ignore,
}

fn parse_anthropic_data(payload: &str, usage: &mut StreamUsage) -> SseAction {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return SseAction::Ignore;
    };
    match value["type"].as_str() {
        Some("content_block_delta") => {
            match value["delta"]["text"].as_str() {
                Some(text) if !text.is_empty() => SseAction::Delta(text.to_string()),
                _ => SseAction::Ignore,
            }
        }
        Some("message_start") => {
            usage.input_tokens = value["message"]["usage"]["input_tokens"].as_u64();
            SseAction::Ignore
        }
        Some("message_delta") => {
            if let Some(output) = value["usage"]["output_tokens"].as_u64() {
                usage.output_tokens = Some(output);
            }
            SseAction::Ignore
        }
        Some("message_stop") => SseAction::Done,
        Some("error") => SseAction::Error(
            value["error"]["message"].as_str().unwrap_or("Provider stream error").to_string(),
        ),
        _ => SseAction::Ignore,
    }
}

fn parse_openai_data(payload: &str, usage: &mut StreamUsage) -> SseAction {
    if payload == "[DONE]" {
        return SseAction::Done;
    }
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return SseAction::Ignore;
    };
    if let Some(message) = value["error"]["message"].as_str() {
        return SseAction::Error(message.to_string());
    }
    // Some providers (e.g. DeepSeek) attach usage to the final chunk.
    if value["usage"].is_object() {
        usage.input_tokens = value["usage"]["prompt_tokens"].as_u64().or(usage.input_tokens);
        usage.output_tokens = value["usage"]["completion_tokens"].as_u64().or(usage.output_tokens);
    }
    match value["choices"][0]["delta"]["content"].as_str() {
        Some(text) if !text.is_empty() => SseAction::Delta(text.to_string()),
        _ => SseAction::Ignore,
    }
}

/// Extract the assistant text from a non-streaming response body.
fn parse_full_response(value: &Value, dialect: Dialect) -> Result<String, String> {
    match dialect {
        Dialect::Anthropic => {
            // content is an array of blocks; concatenate the text ones.
            let text: String = value["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            Ok(text)
        }
        Dialect::OpenAi => Ok(value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string()),
    }
}

async fn error_from_response(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let mut excerpt = body.trim().to_string();
    if excerpt.len() > MAX_ERROR_BODY {
        let mut cut = MAX_ERROR_BODY;
        while !excerpt.is_char_boundary(cut) {
            cut -= 1;
        }
        excerpt.truncate(cut);
        excerpt.push('…');
    }
    if excerpt.is_empty() {
        format!("Provider returned HTTP {}", status.as_u16())
    } else {
        format!("Provider returned HTTP {}: {}", status.as_u16(), excerpt)
    }
}

/// Consume the SSE response, emitting `delta` events until the stream ends.
/// Line-based parsing is sufficient for both dialects: each `data:` payload is
/// a complete single-line JSON document.
async fn consume_stream(
    app: &AppHandle,
    request_id: &str,
    response: reqwest::Response,
    dialect: Dialect,
) -> Result<(), String> {
    let mut usage = StreamUsage::default();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Stream read failed: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline) = buffer.find('\n') {
            let line: String = buffer.drain(..=newline).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            let Some(payload) = line.strip_prefix("data:") else { continue };
            let payload = payload.trim();
            if payload.is_empty() {
                continue;
            }
            let action = match dialect {
                Dialect::Anthropic => parse_anthropic_data(payload, &mut usage),
                Dialect::OpenAi => parse_openai_data(payload, &mut usage),
            };
            match action {
                SseAction::Delta(text) => emit_delta(app, request_id, text),
                SseAction::Done => {
                    done = true;
                }
                SseAction::Error(message) => return Err(message),
                SseAction::Ignore => {}
            }
        }
        if done {
            break;
        }
    }

    // Treat clean EOF without an explicit end marker as done as well — some
    // OpenAI-compatible servers simply close the connection.
    emit_done(app, request_id, &usage);
    Ok(())
}

async fn run_chat(
    app: AppHandle,
    request_id: String,
    messages: Vec<AiMessage>,
    system: Option<String>,
) -> Result<(), String> {
    let config = settings::get_settings(app.clone())?.ai_config;
    let api_key = load_api_key(&app)?;
    let client = http_client()?;
    let (request, dialect) = build_stream_request(&client, &config, &api_key, &system, &messages, true)?;

    let response = request
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    if !response.status().is_success() {
        return Err(error_from_response(response).await);
    }
    consume_stream(&app, &request_id, response, dialect).await
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Start a streaming chat request. Returns the request id immediately; the
/// reply arrives as `ai-stream` events carrying that id.
#[tauri::command]
pub fn ai_chat_start(
    app: AppHandle,
    state: State<'_, AiState>,
    messages: Vec<AiMessage>,
    system: Option<String>,
) -> Result<String, String> {
    if messages.is_empty() {
        return Err("Messages must not be empty".to_string());
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let active = state.active.clone();
    let task_app = app.clone();
    let task_request_id = request_id.clone();

    let handle = tauri::async_runtime::spawn(async move {
        if let Err(message) = run_chat(
            task_app.clone(),
            task_request_id.clone(),
            messages,
            system,
        )
        .await
        {
            emit_error(&task_app, &task_request_id, message);
        }
        if let Ok(mut active) = active.lock() {
            active.remove(&task_request_id);
        }
    });

    if let Ok(mut active) = state.active.lock() {
        active.insert(request_id.clone(), handle);
    }
    Ok(request_id)
}

/// Abort an in-flight request. Emits a terminal `cancelled` event so every
/// listener state machine converges regardless of who initiated the cancel.
#[tauri::command]
pub fn ai_chat_cancel(
    app: AppHandle,
    state: State<'_, AiState>,
    request_id: String,
) -> Result<(), String> {
    let handle = state
        .active
        .lock()
        .ok()
        .and_then(|mut active| active.remove(&request_id));
    if let Some(handle) = handle {
        handle.abort();
        emit_stream_event(&app, AiStreamEvent {
            request_id,
            kind: "cancelled",
            text: None,
            message: None,
            input_tokens: None,
            output_tokens: None,
        });
    }
    Ok(())
}

/// One-shot, non-streaming completion. Used by the input bar's natural-language
/// command generation, where a streaming UI adds no value — the caller just
/// wants the final text (a single command) back.
#[tauri::command]
pub async fn ai_complete(
    app: AppHandle,
    messages: Vec<AiMessage>,
    system: Option<String>,
) -> Result<String, String> {
    if messages.is_empty() {
        return Err("Messages must not be empty".to_string());
    }
    let config = settings::get_settings(app.clone())?.ai_config;
    let api_key = load_api_key(&app)?;
    let client = http_client()?;
    let (request, dialect) = build_stream_request(&client, &config, &api_key, &system, &messages, false)?;

    let response = request
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    if !response.status().is_success() {
        return Err(error_from_response(response).await);
    }
    let value: Value = response.json().await.map_err(|e| format!("Invalid response: {}", e))?;
    parse_full_response(&value, dialect)
}

/// Fire a minimal non-streaming request to validate key + endpoint + model.
#[tauri::command]
pub async fn ai_test_connection(app: AppHandle) -> Result<(), String> {
    let config = settings::get_settings(app.clone())?.ai_config;
    let api_key = load_api_key(&app)?;
    let client = http_client()?;
    let probe = vec![AiMessage { role: "user".to_string(), content: "ping".to_string() }];
    let mut probe_config = config;
    probe_config.max_tokens = Some(1);
    let (request, _) = build_stream_request(&client, &probe_config, &api_key, &None, &probe, false)?;

    let response = request
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    if !response.status().is_success() {
        return Err(error_from_response(response).await);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_delta_and_usage_parse() {
        let mut usage = StreamUsage::default();
        assert!(matches!(
            parse_anthropic_data(
                r#"{"type":"message_start","message":{"usage":{"input_tokens":42}}}"#,
                &mut usage,
            ),
            SseAction::Ignore
        ));
        assert_eq!(usage.input_tokens, Some(42));

        match parse_anthropic_data(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            &mut usage,
        ) {
            SseAction::Delta(text) => assert_eq!(text, "hi"),
            _ => panic!("expected delta"),
        }

        assert!(matches!(
            parse_anthropic_data(
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
                &mut usage,
            ),
            SseAction::Ignore
        ));
        assert_eq!(usage.output_tokens, Some(7));

        assert!(matches!(
            parse_anthropic_data(r#"{"type":"message_stop"}"#, &mut usage),
            SseAction::Done
        ));
    }

    #[test]
    fn openai_delta_done_and_error_parse() {
        let mut usage = StreamUsage::default();
        match parse_openai_data(
            r#"{"choices":[{"delta":{"content":"hello"}}]}"#,
            &mut usage,
        ) {
            SseAction::Delta(text) => assert_eq!(text, "hello"),
            _ => panic!("expected delta"),
        }

        assert!(matches!(parse_openai_data("[DONE]", &mut usage), SseAction::Done));

        match parse_openai_data(r#"{"error":{"message":"bad key"}}"#, &mut usage) {
            SseAction::Error(message) => assert_eq!(message, "bad key"),
            _ => panic!("expected error"),
        }

        assert!(matches!(
            parse_openai_data(
                r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3}}"#,
                &mut usage,
            ),
            SseAction::Ignore
        ));
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(3));
    }

    #[test]
    fn full_response_parse_both_dialects() {
        let anthropic = serde_json::json!({
            "content": [
                {"type": "text", "text": "df -h"},
                {"type": "text", "text": " /"},
            ],
        });
        assert_eq!(parse_full_response(&anthropic, Dialect::Anthropic).unwrap(), "df -h /");

        let openai = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "ls -la"}}],
        });
        assert_eq!(parse_full_response(&openai, Dialect::OpenAi).unwrap(), "ls -la");

        // Missing content degrades to empty string rather than erroring.
        assert_eq!(parse_full_response(&serde_json::json!({}), Dialect::Anthropic).unwrap(), "");
    }

    #[test]
    fn openai_provider_requires_base_url() {
        let client = reqwest::Client::new();
        let config = AiConfig {
            enabled: true,
            provider: "openai-compatible".to_string(),
            base_url: None,
            model: "deepseek-chat".to_string(),
            max_tokens: None,
        };
        let result = build_stream_request(&client, &config, "k", &None, &[], true);
        assert!(result.is_err());
    }
}
