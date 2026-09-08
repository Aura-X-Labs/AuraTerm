//! One-time migration of legacy `e2e-v1` sync vaults.
//!
//! Builds before the sync passphrase was removed uploaded an `AURASYNC` blob
//! encrypted under that passphrase. The server still hands such a vault back
//! verbatim (`format: "e2e-v1"`), and the regular pull refuses it so the cloud
//! copy is never overwritten by accident. This module is the only path that
//! still touches the old format:
//!
//! - with the old passphrase: decrypt, merge into local state, re-upload as
//!   `rest-v2`;
//! - without it ("overwrite"): re-upload this device's data, since the vault
//!   is only ever a mirror of some device's local state.
//!
//! **Phase 3 of `docs/plans/sync-passphrase-removal-design.md` deletes this
//! file** (together with `encryption::{encrypt,decrypt}_sync_blob`) once every
//! client in the field has migrated.

use crate::cloud_sync::{self, RemoteContent, SyncConfig, SyncResult, ERR_NOT_SIGNED_IN};
use crate::connections::SavedConnection;
use crate::encryption::{self, MasterPasswordState, StoredCredential};

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use tauri::{AppHandle, State};
use zeroize::Zeroizing;

/// Schema 1 bundle as written by the passphrase-era engine. Credentials were
/// plaintext inside the (then end-to-end encrypted) blob.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LegacyBundle {
    pub(crate) schema: u32,
    pub(crate) bookmarks: Vec<SavedConnection>,
    pub(crate) settings: Option<Value>,
    pub(crate) known_hosts: HashMap<String, String>,
    pub(crate) credentials: Vec<StoredCredential>,
}

pub(crate) fn decode_legacy_bundle(blob: &[u8], passphrase: &str) -> Result<LegacyBundle, String> {
    let plaintext = Zeroizing::new(encryption::decrypt_sync_blob(blob, passphrase)?);
    let bundle: LegacyBundle = serde_json::from_slice(&plaintext).map_err(|e| format!("Corrupt sync bundle: {e}"))?;
    if bundle.schema != 1 {
        return Err(format!("Unexpected legacy bundle schema {}", bundle.schema));
    }
    Ok(bundle)
}

/// Replace the legacy vault with a `rest-v2` one. `passphrase` merges the old
/// contents first; `overwrite` skips that and uploads this device's data.
#[tauri::command]
pub async fn cloud_sync_migrate_legacy(
    app: AppHandle,
    passphrase: Option<String>,
    overwrite: bool,
    master_state: State<'_, MasterPasswordState>,
) -> Result<SyncResult, String> {
    let mut config: SyncConfig = cloud_sync::load_config(&app)?;
    cloud_sync::ensure_signed_in(&config).map_err(|_| ERR_NOT_SIGNED_IN.to_string())?;

    let Some(remote) = cloud_sync::auraxlab_pull(&config.auraxlab).await? else {
        return Err("Your AuraXLab account has no synced data yet; nothing to migrate.".to_string());
    };
    let mut result = SyncResult::default();
    let blob = match remote.content {
        RemoteContent::LegacyBlob(blob) => blob,
        RemoteContent::Payload(_) => {
            result.message = "The cloud copy is already in the new format; nothing to migrate.".to_string();
            return Ok(result);
        }
    };

    if !overwrite {
        let passphrase = Zeroizing::new(passphrase.unwrap_or_default());
        if passphrase.is_empty() {
            return Err("Enter the old sync passphrase, or choose to overwrite the cloud copy with this device's data.".to_string());
        }
        let bundle = decode_legacy_bundle(&blob, &passphrase)?;
        cloud_sync::apply_tier1(&app, &config, bundle.bookmarks, bundle.settings.as_ref(), bundle.known_hosts, false, &mut result).await?;
        if !bundle.credentials.is_empty() {
            if encryption::credentials_accessible(&app, &master_state) {
                result.credentials_synced = cloud_sync::merge_plain_credentials(&app, &master_state, bundle.credentials)?;
            } else {
                result.credentials_skipped = Some(cloud_sync::skip::MASTER_LOCKED.to_string());
            }
        }
    }

    // Re-upload as rest-v2, based on the legacy vault's version so a
    // concurrent write from another device still conflicts.
    cloud_sync::push_current_state(&app, &master_state, &mut config, remote.version, &mut result).await?;
    result.message = if overwrite {
        "Replaced the old cloud copy with this device's data.".to_string()
    } else {
        "Migrated the cloud copy to the new format.".to_string()
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_schema_1_bundle_with_the_old_passphrase() {
        let plaintext = serde_json::json!({
            "schema": 1, "exportedAt": 1, "deviceId": "d", "deviceLabel": "l",
            "bookmarks": [crate::cloud_sync::tests::bookmark("b1", "one")],
            "knownHosts": {"h:22": "ssh-ed25519 AAAA"},
            "credentials": [{"connection_id": "b1", "password": "pw"}]
        });
        let blob = encryption::encrypt_sync_blob(plaintext.to_string().as_bytes(), "old").unwrap();
        let bundle = decode_legacy_bundle(&blob, "old").unwrap();
        assert_eq!(bundle.bookmarks.len(), 1);
        assert_eq!(bundle.known_hosts.len(), 1);
        assert_eq!(bundle.credentials[0].connection_id, "b1");
        assert!(decode_legacy_bundle(&blob, "wrong").is_err());
    }
}
