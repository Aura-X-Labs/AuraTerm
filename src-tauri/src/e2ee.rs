//! Application-layer E2EE envelope shared by Cloud Console (agent ↔ browser)
//! and Remote Assist (host ↔ guest).
//!
//! One AES-256-GCM key per peer connection; every frame carries a fresh
//! random nonce and a strictly increasing per-direction counter that is bound
//! into the AAD together with the session id, the relay connection id and the
//! sender's direction label. The relay only ever sees `E2EE_FRAME` envelopes:
//! it can drop or reorder them, but never read, forge or replay them.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::Rng;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Wire-level direction labels. They are part of the AAD, so a frame
/// encrypted by one side can never be replayed back to it as its own output.
pub const DIRECTION_AGENT: &str = "agent";
pub const DIRECTION_BROWSER: &str = "browser";
#[allow(dead_code)] // Remote Assist host/guest modules (design Phase 2/4).
pub const DIRECTION_HOST: &str = "host";
#[allow(dead_code)]
pub const DIRECTION_GUEST: &str = "guest";

#[derive(Clone)]
pub struct PeerCipher {
    pub key: [u8; 32],
    counters: Arc<Mutex<PeerCounters>>,
    /// Serialises encrypt+send so counters hit the wire in order.
    pub send_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct PeerCounters {
    sent: u64,
    received: u64,
}

impl PeerCipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            counters: Arc::new(Mutex::new(PeerCounters::default())),
            send_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Wrap `frame` for the peer. `direction` is *our* label.
    pub fn encrypt(
        &self,
        session_id: &str,
        connection_id: &str,
        direction: &str,
        frame: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| "invalid E2EE key".to_string())?;
        let mut nonce = [0_u8; 12];
        rand::thread_rng().fill(&mut nonce);
        let counter = {
            let mut counters = self.counters.lock().map_err(|e| e.to_string())?;
            counters.sent += 1;
            counters.sent
        };
        let aad = format!("{session_id}|{connection_id}|{direction}|{counter}");
        let plaintext = serde_json::to_vec(frame).map_err(|e| e.to_string())?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "E2EE encryption failed".to_string())?;
        Ok(json!({
            "kind": "E2EE_FRAME", "connection_id": connection_id,
            "counter": counter,
            "nonce": URL_SAFE_NO_PAD.encode(nonce),
            "ciphertext": URL_SAFE_NO_PAD.encode(ciphertext),
        }))
    }

    /// Unwrap an envelope sent by the peer. `peer_direction` is *their*
    /// label. Counters must arrive in strict order; anything else is treated
    /// as a replay and rejected without advancing state.
    pub fn decrypt(
        &self,
        session_id: &str,
        connection_id: &str,
        peer_direction: &str,
        envelope: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let nonce = URL_SAFE_NO_PAD
            .decode(
                envelope
                    .get("nonce")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing E2EE nonce".to_string())?,
            )
            .map_err(|_| "invalid E2EE nonce".to_string())?;
        if nonce.len() != 12 {
            return Err("invalid E2EE nonce length".into());
        }
        let ciphertext = URL_SAFE_NO_PAD
            .decode(
                envelope
                    .get("ciphertext")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing E2EE ciphertext".to_string())?,
            )
            .map_err(|_| "invalid E2EE ciphertext".to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| "invalid E2EE key".to_string())?;
        let counter = envelope
            .get("counter")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| "missing E2EE counter".to_string())?;
        {
            let counters = self.counters.lock().map_err(|e| e.to_string())?;
            if counter != counters.received + 1 {
                return Err("replayed or out-of-order E2EE frame".into());
            }
        }
        let aad = format!("{session_id}|{connection_id}|{peer_direction}|{counter}");
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "E2EE authentication failed".to_string())?;
        let decoded = serde_json::from_slice(&plaintext).map_err(|_| "invalid E2EE payload".to_string())?;
        self.counters.lock().map_err(|e| e.to_string())?.received = counter;
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_and_rejects_replay_and_wrong_direction() {
        let key = [7_u8; 32];
        let host = PeerCipher::new(key);
        let guest = PeerCipher::new(key);
        let frame = json!({"kind": "INPUT", "data_hex": "41"});
        let envelope = guest.encrypt("s", "c", DIRECTION_GUEST, &frame).unwrap();
        assert_eq!(host.decrypt("s", "c", DIRECTION_GUEST, &envelope).unwrap(), frame);
        // Replay: the counter did not advance.
        assert!(host.decrypt("s", "c", DIRECTION_GUEST, &envelope).is_err());
        // A host-labelled decrypt of a guest-labelled frame must fail (AAD).
        let host2 = PeerCipher::new(key);
        assert!(host2.decrypt("s", "c", DIRECTION_HOST, &envelope).is_err());
        // Wrong connection id in AAD.
        let host3 = PeerCipher::new(key);
        assert!(host3.decrypt("s", "other", DIRECTION_GUEST, &envelope).is_err());
    }
}
