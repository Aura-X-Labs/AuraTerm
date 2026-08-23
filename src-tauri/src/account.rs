//! Unified AuraXLab account coordinator.
//!
//! This is the only desktop boundary that accepts an account password. It
//! exchanges it once through the Phase A account protocol and coordinates the
//! sync credential and optional Cloud Console device enrollment by stable
//! account subject.

use crate::{cloud_bridge, cloud_sync};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, State};

pub const AURAXLAB_ORIGIN: &str = "https://auraxlab.com";

/// Production builds have exactly one account origin. Tests use provider-local
/// endpoint seams instead of allowing persisted or frontend-controlled URLs.
pub(crate) fn auraxlab_origin() -> String {
    AURAXLAB_ORIGIN.to_string()
}

#[derive(Deserialize)]
struct LoginAccount {
    subject: String,
    email: String,
    username: String,
    confirmed: bool,
}

#[derive(Deserialize)]
struct LoginCredential {
    secret: String,
    scope: String,
}

#[derive(Deserialize)]
struct LoginResponse {
    account: LoginAccount,
    sync_credential: LoginCredential,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleAccountState {
    enrolled: bool,
    connected: bool,
    device_id: Option<String>,
    device_label: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuraXLabAccountState {
    signed_in: bool,
    account_subject: Option<String>,
    email: String,
    username: String,
    confirmed: bool,
    sync_credential_set: bool,
    consistency: String,
    console: ConsoleAccountState,
    traffic: Option<cloud_sync::AccountTraffic>,
}

fn consistency(sync_subject: Option<&str>, device_subject: Option<&str>) -> &'static str {
    match (sync_subject, device_subject) {
        (None, None) => "signed_out",
        (Some(_), None) => "sync_only",
        (None, Some(_)) => "device_only",
        (Some(sync), Some(device)) if !sync.is_empty() && sync == device => "consistent",
        (Some(_), Some(_)) => "mismatch",
    }
}

fn response_error(status: reqwest::StatusCode, body: &str) -> String {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    parsed
        .get("message")
        .or_else(|| parsed.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("AuraXLab returned {status}"))
}

async fn read_state(app: &AppHandle, bridge: &cloud_bridge::CloudBridgeState, refresh_profile: bool) -> Result<AuraXLabAccountState, String> {
    cloud_bridge::restore(app, bridge)?;
    let mut sync = cloud_sync::local_sync_account(app)?;
    // Scoped credentials are only issued to confirmed accounts. A profile
    // refresh may update this along with traffic, but offline state remains
    // accurate without requiring a network round-trip.
    let mut confirmed = sync.is_some();
    let mut traffic = None;

    if sync.is_some() && (refresh_profile || sync.as_ref().is_some_and(|a| a.subject.is_empty())) {
        if let Ok(overview) = cloud_sync::fetch_account_overview(app).await {
            confirmed = overview.confirmed;
            traffic = overview.traffic;
            sync = cloud_sync::local_sync_account(app)?;
        }
    }

    let device = cloud_bridge::account_snapshot(bridge)?;
    let sync_subject = sync.as_ref().map(|a| a.subject.as_str());
    let device_subject = device.enrolled.then_some(device.account_subject.as_str());
    let relation = consistency(sync_subject, device_subject);
    let account_subject = sync.as_ref().map(|a| a.subject.clone()).filter(|value| !value.is_empty());

    Ok(AuraXLabAccountState {
        signed_in: sync.is_some(),
        account_subject,
        email: sync.as_ref().map(|a| a.email.clone()).unwrap_or_default(),
        username: sync.as_ref().map(|a| a.username.clone()).unwrap_or_default(),
        confirmed,
        sync_credential_set: sync.is_some(),
        consistency: relation.to_string(),
        console: ConsoleAccountState {
            enrolled: device.enrolled,
            connected: device.connected,
            device_id: device.device_id,
            device_label: device.device_label,
        },
        traffic,
    })
}

#[tauri::command]
pub async fn auraxlab_account_state(
    app: AppHandle,
    bridge: State<'_, cloud_bridge::CloudBridgeState>,
) -> Result<AuraXLabAccountState, String> {
    // Opening the account dialog must not wait on the network. The persisted
    // credential is the source of truth; profile data is refreshed separately.
    read_state(&app, &bridge, false).await
}

#[tauri::command]
pub async fn auraxlab_account_refresh(
    app: AppHandle,
    bridge: State<'_, cloud_bridge::CloudBridgeState>,
) -> Result<AuraXLabAccountState, String> {
    read_state(&app, &bridge, true).await
}

#[tauri::command]
pub async fn auraxlab_account_restore(app: AppHandle, bridge: State<'_, cloud_bridge::CloudBridgeState>) -> Result<AuraXLabAccountState, String> {
    let current = read_state(&app, &bridge, false).await?;
    if current.consistency == "consistent" {
        cloud_bridge::connect(&app, &bridge).await?;
    }
    read_state(&app, &bridge, false).await
}

#[tauri::command]
pub async fn auraxlab_account_login(
    app: AppHandle,
    bridge: State<'_, cloud_bridge::CloudBridgeState>,
    email: String,
    password: String,
    device_label: String,
    platform: String,
    enable_console: bool,
) -> Result<AuraXLabAccountState, String> {
    if email.trim().is_empty() || password.is_empty() {
        return Err("Email and password are required.".to_string());
    }

    cloud_bridge::restore(&app, &bridge)?;
    let old_sync = cloud_sync::local_sync_account(&app)?;
    let current_device = cloud_bridge::account_snapshot(&bridge)?;
    let needs_enrollment = enable_console
        && (!current_device.enrolled
            || old_sync
                .as_ref()
                .is_none_or(|account| account.subject.is_empty() || account.subject != current_device.account_subject));

    let enrollment = if needs_enrollment {
        Some(cloud_bridge::begin_enrollment(&bridge, device_label.clone(), platform).await?)
    } else {
        None
    };
    let enrollment_json = enrollment
        .as_ref()
        .map(|value| json!({"user_code": value.user_code, "fingerprint": value.fingerprint}));
    let response = cloud_sync::http_client()?
        .post(format!("{}/api/v1/auraterm/account/login", auraxlab_origin()))
        .basic_auth(email.trim(), Some(&password))
        .json(&json!({"device_label": device_label, "enrollment": enrollment_json}))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Sign-in failed: {}", response_error(status, &body)));
    }
    let login: LoginResponse = serde_json::from_str(&body).map_err(|e| format!("Invalid AuraXLab account response: {e}"))?;
    if !login.account.confirmed || login.sync_credential.scope != "sync" || !login.sync_credential.secret.starts_with("axsync_") {
        cloud_sync::revoke_sync_credential(&login.sync_credential.secret).await;
        return Err("AuraXLab returned an invalid account login response.".to_string());
    }

    if let Err(error) = cloud_sync::store_account_login(
        &app,
        &login.account.subject,
        &login.account.email,
        &login.account.username,
        &login.sync_credential.secret,
        &device_label,
    ) {
        cloud_sync::revoke_sync_credential(&login.sync_credential.secret).await;
        return Err(error);
    }

    if enrollment.is_some() {
        match cloud_bridge::redeem_enrollment(&app, &bridge, Some(&login.account.subject)).await {
            Ok(result) if result.status == "ok" => {}
            Ok(result) => {
                cloud_sync::clear_account_login(&app)?;
                cloud_sync::revoke_sync_credential(&login.sync_credential.secret).await;
                return Err(format!("Cloud Console enrollment is {}.", result.status));
            }
            Err(error) => {
                cloud_sync::clear_account_login(&app)?;
                cloud_sync::revoke_sync_credential(&login.sync_credential.secret).await;
                return Err(error);
            }
        }
    }

    if let Some(old) = old_sync {
        if old.token != login.sync_credential.secret {
            cloud_sync::revoke_sync_credential(&old.token).await;
        }
    }
    let state = read_state(&app, &bridge, false).await?;
    if enable_console && state.consistency == "consistent" {
        cloud_bridge::connect(&app, &bridge).await?;
    }
    read_state(&app, &bridge, false).await
}

#[tauri::command]
pub async fn auraxlab_account_logout(app: AppHandle, bridge: State<'_, cloud_bridge::CloudBridgeState>) -> Result<AuraXLabAccountState, String> {
    let sync = cloud_sync::local_sync_account(&app)?;
    let _ = cloud_bridge::pause(&bridge).await;
    let unbind_result = cloud_bridge::unbind(&app, &bridge).await;
    if let Some(account) = sync {
        cloud_sync::revoke_sync_credential(&account.token).await;
    }
    let clear_result = cloud_sync::clear_account_login(&app);
    unbind_result?;
    clear_result?;
    read_state(&app, &bridge, false).await
}

#[tauri::command]
pub async fn auraxlab_account_enable_console(
    app: AppHandle,
    bridge: State<'_, cloud_bridge::CloudBridgeState>,
    email: String,
    password: String,
    device_label: String,
    platform: String,
) -> Result<AuraXLabAccountState, String> {
    auraxlab_account_login(app, bridge, email, password, device_label, platform, true).await
}

#[tauri::command]
pub async fn auraxlab_account_pause_console(app: AppHandle, bridge: State<'_, cloud_bridge::CloudBridgeState>) -> Result<AuraXLabAccountState, String> {
    cloud_bridge::pause(&bridge).await?;
    read_state(&app, &bridge, false).await
}

#[cfg(test)]
mod tests {
    use super::consistency;

    #[test]
    fn consistency_fails_closed() {
        assert_eq!(consistency(None, None), "signed_out");
        assert_eq!(consistency(Some("acc_a"), None), "sync_only");
        assert_eq!(consistency(None, Some("acc_a")), "device_only");
        assert_eq!(consistency(Some("acc_a"), Some("acc_a")), "consistent");
        assert_eq!(consistency(Some("acc_a"), Some("acc_b")), "mismatch");
        assert_eq!(consistency(Some("acc_a"), Some("")), "mismatch");
    }
}
