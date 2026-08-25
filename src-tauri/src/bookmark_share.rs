//! Bookmark share codes: pack a group, encrypt it, hand out a short code —
//! and open one on the other side.
//!
//! A share code is `RRRR-SSSS-SSSS-SSSS`: a 4-character *route* segment
//! AuraXLab assigns and looks up, plus a 12-character *secret* segment that is
//! generated here and never sent anywhere. The server stores ciphertext it
//! cannot open, exactly as it stores a sync vault.
//!
//! **Why the secret is 12 characters and a Live Share code's is 8.** Both draw
//! from the same 28-glyph alphabet (~4.81 bits each), but the threat is not the
//! same. A Live Share secret is proven *online* with SPAKE2, where the server
//! counts failures and locks the session, so 8 characters (~38.5 bits) is
//! plenty. A bookmark share is an offline drop: the sharer is usually asleep by
//! the time it is redeemed, and anyone who fetched the blob can grind it at
//! their own pace. 38.5 bits behind Argon2id (16 MiB, t=3) is roughly a
//! few GPU-days for a determined attacker — shorter than the default 7-day
//! lifetime of the share. 12 characters (~57.7 bits) moves that out of reach.
//! Four extra characters is the cheapest part of this whole design.
//!
//! The route segment is *not* part of that budget: 4 characters is ~614k
//! values and must be assumed enumerable. It is an address, and the server
//! rate-limits it and answers every unusable code identically.

use crate::account::auraxlab_origin;
use crate::assist::CODE_ALPHABET;
use crate::{cloud_sync, connections, encryption};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, Manager};
use zeroize::Zeroizing;

pub const ROUTE_LEN: usize = 4;
pub const SECRET_LEN: usize = 12;
const SHARE_LINK_BASE: &str = "https://auraxlab.com/b";
/// Local sidecar for the labels the server deliberately never learns.
const SHARE_LABELS_FILE: &str = "bookmark_shares.json";

pub struct ShareCode {
    pub route: String,
    pub secret: Zeroizing<String>,
}

/// CSPRNG secret segment from the shared code alphabet.
pub fn generate_secret() -> Zeroizing<String> {
    let mut rng = rand::rngs::OsRng;
    Zeroizing::new(
        (0..SECRET_LEN)
            .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
            .collect(),
    )
}

/// `RRRR-SSSS-SSSS-SSSS` — grouped in fours because people retype these.
pub fn format_code(route: &str, secret: &str) -> String {
    let mut out = String::with_capacity(ROUTE_LEN + SECRET_LEN + 3);
    out.push_str(route);
    for chunk in secret.as_bytes().chunks(4) {
        out.push('-');
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    }
    out
}

/// The link form. The code sits in the URL *fragment*, which browsers never
/// send to the server — the same trick the Live Share join link uses.
pub fn share_link(route: &str, secret: &str) -> String {
    format!("{}#{}", SHARE_LINK_BASE, format_code(route, secret))
}

/// Normalise user input: a bare code, a code with any separators, or a share
/// link. Case-insensitive, because the alphabet is upper-case by convention.
pub fn parse_code(input: &str) -> Result<ShareCode, String> {
    let raw = input.trim();
    let raw = match raw.rfind('#') {
        Some(index) => &raw[index + 1..],
        None => raw,
    };
    let normalised: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if normalised.len() != ROUTE_LEN + SECRET_LEN {
        return Err(format!(
            "A share code has {} characters",
            ROUTE_LEN + SECRET_LEN
        ));
    }
    if !normalised.bytes().all(|b| CODE_ALPHABET.contains(&b)) {
        return Err("That share code contains an invalid character".to_string());
    }
    Ok(ShareCode {
        route: normalised[..ROUTE_LEN].to_string(),
        secret: Zeroizing::new(normalised[ROUTE_LEN..].to_string()),
    })
}

// ── local labels ────────────────────────────────────────────────────────────
// The server stores no plaintext, not even what a share is called, so the
// readable name lives here. Losing this file costs labels, nothing else.

fn labels_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join(SHARE_LABELS_FILE))
}

fn load_labels(app: &AppHandle) -> HashMap<String, String> {
    let Ok(path) = labels_path(app) else {
        return HashMap::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_labels(app: &AppHandle, labels: &HashMap<String, String>) {
    let Ok(path) = labels_path(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(labels) {
        // A label is a convenience; failing to write one must not fail a share.
        let _ = crate::util::write_atomic(&path, &text);
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────────

fn sync_credential(app: &AppHandle) -> Result<String, String> {
    let token = cloud_sync::load_config(app)?.auraxlab.token;
    if !token.starts_with("axsync_") {
        return Err("Sign in to your AuraXLab account before sharing".to_string());
    }
    Ok(token)
}

fn shares_url() -> String {
    format!("{}/api/v1/auraterm/shares", auraxlab_origin())
}

#[derive(Deserialize)]
struct ShareResponse {
    route_code: String,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    max_redeems: u32,
    #[serde(default)]
    blob: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

async fn api_error(response: reqwest::Response, fallback: &str) -> String {
    let status = response.status();
    match response.json::<ApiError>().await {
        Ok(body) if !body.message.is_empty() => body.message,
        _ => format!("{} ({})", fallback, status),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareTicket {
    route: String,
    /// The full `RRRR-SSSS-SSSS-SSSS` code, shown once and never recoverable.
    code: String,
    link: String,
    expires_at: Option<String>,
    max_redeems: u32,
}

/// Pack a group, encrypt it under a fresh code, and upload the ciphertext.
///
/// The returned code is the only copy of the secret segment: it exists in this
/// process and in whatever the user pastes it into. Losing it means revoking
/// the share and making a new one — the server cannot help.
#[tauri::command]
pub async fn create_bookmark_share(
    app: AppHandle,
    root: String,
    explicit_groups: Vec<String>,
    label: Option<String>,
    note: Option<String>,
    ttl_hours: Option<u32>,
    max_redeems: Option<u32>,
) -> Result<ShareTicket, String> {
    let display_label = label.clone().unwrap_or_else(|| root.clone());
    let bundle = connections::build_share_bundle(&app, root, explicit_groups, label, note)?;
    let secret = generate_secret();
    let blob = encryption::encrypt_share_blob(bundle.as_bytes(), &secret)?;
    let token = sync_credential(&app)?;

    let response = cloud_sync::http_client()?
        .post(shares_url())
        .basic_auth(&token, Some(""))
        .json(&serde_json::json!({
            "blob": STANDARD.encode(&blob),
            "ttlHours": ttl_hours,
            "maxRedeems": max_redeems,
        }))
        .send()
        .await
        .map_err(|e| format!("Could not reach AuraXLab: {e}"))?;
    if !response.status().is_success() {
        return Err(api_error(response, "Could not create the share").await);
    }
    let created: ShareResponse = response
        .json()
        .await
        .map_err(|e| format!("Unexpected reply from AuraXLab: {e}"))?;

    let mut labels = load_labels(&app);
    labels.insert(created.route_code.clone(), display_label);
    save_labels(&app, &labels);

    Ok(ShareTicket {
        code: format_code(&created.route_code, &secret),
        link: share_link(&created.route_code, &secret),
        route: created.route_code,
        expires_at: created.expires_at,
        max_redeems: created.max_redeems,
    })
}

/// Fetch and decrypt a share. Returns the bundle JSON, which goes straight into
/// `preview_bookmark_import` — a redeemed share is external input like any
/// other file, and gets the same review before it lands.
#[tauri::command]
pub async fn redeem_bookmark_share(code: String) -> Result<String, String> {
    let parsed = parse_code(&code)?;
    let response = cloud_sync::http_client()?
        .get(format!("{}/{}", shares_url(), parsed.route))
        .send()
        .await
        .map_err(|e| format!("Could not reach AuraXLab: {e}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // The server cannot tell us more than this, by design.
        return Err("That share code is not valid, or it has expired".to_string());
    }
    if !response.status().is_success() {
        return Err(api_error(response, "Could not open the share").await);
    }

    let body: ShareResponse = response
        .json()
        .await
        .map_err(|e| format!("Unexpected reply from AuraXLab: {e}"))?;
    let blob = STANDARD
        .decode(body.blob.unwrap_or_default())
        .map_err(|_| "The shared data is corrupt".to_string())?;
    let plaintext = encryption::decrypt_share_blob(&blob, &parsed.secret)?;
    String::from_utf8(plaintext).map_err(|_| "The shared data is corrupt".to_string())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRecord {
    route_code: String,
    state: String,
    #[serde(default)]
    redeem_count: u32,
    #[serde(default)]
    max_redeems: u32,
    #[serde(default)]
    redeemable: bool,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    last_redeemed_at: Option<String>,
    /// From the local sidecar — the server never learned it.
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
struct ShareList {
    shares: Vec<ShareRecord>,
}

/// The account's shares. Codes are *not* here and cannot be: the secret
/// segment was never uploaded, so a share whose code is lost can only be
/// revoked and replaced.
#[tauri::command]
pub async fn list_bookmark_shares(app: AppHandle) -> Result<Vec<ShareRecord>, String> {
    let token = sync_credential(&app)?;
    let response = cloud_sync::http_client()?
        .get(shares_url())
        .basic_auth(&token, Some(""))
        .send()
        .await
        .map_err(|e| format!("Could not reach AuraXLab: {e}"))?;
    if !response.status().is_success() {
        return Err(api_error(response, "Could not list your shares").await);
    }
    let listed: ShareList = response
        .json()
        .await
        .map_err(|e| format!("Unexpected reply from AuraXLab: {e}"))?;

    let labels = load_labels(&app);
    Ok(listed
        .shares
        .into_iter()
        .map(|mut share| {
            share.label = labels.get(&share.route_code).cloned();
            share
        })
        .collect())
}

/// Revoke a share: the ciphertext is deleted server-side, so a link already
/// sent to somebody stops working.
#[tauri::command]
pub async fn revoke_bookmark_share(app: AppHandle, route: String) -> Result<(), String> {
    let token = sync_credential(&app)?;
    let route = route.trim().to_ascii_uppercase();
    let response = cloud_sync::http_client()?
        .delete(format!("{}/{}", shares_url(), route))
        .basic_auth(&token, Some(""))
        .send()
        .await
        .map_err(|e| format!("Could not reach AuraXLab: {e}"))?;
    if !response.status().is_success() {
        return Err(api_error(response, "Could not revoke the share").await);
    }
    let mut labels = load_labels(&app);
    if labels.remove(&route).is_some() {
        save_labels(&app, &labels);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_a_route_plus_three_groups_of_four() {
        assert_eq!(format_code("BCDF", "GHJKLMNPQRST"), "BCDF-GHJK-LMNP-QRST");
        assert_eq!(
            share_link("BCDF", "GHJKLMNPQRST"),
            "https://auraxlab.com/b#BCDF-GHJK-LMNP-QRST"
        );
    }

    #[test]
    fn parsing_accepts_whatever_the_user_pastes() {
        for input in [
            "BCDF-GHJK-LMNP-QRST",
            "bcdfghjklmnpqrst",
            "  BCDF GHJK LMNP QRST  ",
            "https://auraxlab.com/b#BCDF-GHJK-LMNP-QRST",
        ] {
            let parsed = parse_code(input).expect(input);
            assert_eq!(parsed.route, "BCDF");
            assert_eq!(&*parsed.secret, "GHJKLMNPQRST");
        }
    }

    #[test]
    fn parsing_rejects_wrong_lengths_and_lookalike_glyphs() {
        assert!(parse_code("BCDF-GHJK-LMNP").is_err(), "too short");
        assert!(parse_code("BCDF-GHJK-LMNP-QRST-VWXZ").is_err(), "too long");
        // I, O, U, A, E, Y are absent from the alphabet on purpose.
        assert!(parse_code("BCDF-GHJK-LMNP-QRSI").is_err());
    }

    #[test]
    fn generated_secrets_are_long_enough_to_resist_offline_grinding() {
        let secret = generate_secret();
        assert_eq!(secret.len(), SECRET_LEN);
        assert!(secret.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        // The whole point of the 12/8 split: a share is attacked offline.
        assert!(SECRET_LEN > crate::assist::SECRET_LEN);
    }

    #[test]
    fn a_bundle_survives_the_envelope_and_a_wrong_code_does_not_open_it() {
        let secret = generate_secret();
        let blob = encryption::encrypt_share_blob(b"{\"format\":\"auraterm-bookmarks\"}", &secret)
            .expect("encrypt");
        assert_eq!(
            encryption::decrypt_share_blob(&blob, &secret).expect("decrypt"),
            b"{\"format\":\"auraterm-bookmarks\"}"
        );

        let other = generate_secret();
        assert!(encryption::decrypt_share_blob(&blob, &other).is_err());
        // A sync vault must not open as a share bundle, or vice versa.
        let sync = encryption::encrypt_sync_blob(b"payload", &secret).expect("sync");
        assert!(encryption::decrypt_share_blob(&sync, &secret).is_err());
    }
}
