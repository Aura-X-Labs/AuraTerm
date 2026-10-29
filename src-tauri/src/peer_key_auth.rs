//! Control-plane authorization of a viewer's E2EE public key.
//!
//! Live Console browsers and Live Relay peers reach this device through the
//! relay, which forwards each one's E2EE public key in `E2EE_INIT`. The relay
//! is trusted to route frames, not to introduce viewers: if this device took
//! the forwarded key at face value, a compromised relay could hand it a key of
//! its own and read — or, with a controller role, type into — the session.
//! (Live Share is not affected: its SPAKE2 handshake proves the guest knows
//! the code, which the relay never sees.)
//!
//! So the control plane signs `{session, this device, sha256(key), role}`
//! with the console authority key when it mints the viewer's ticket, and the
//! relay carries that token next to the key. The relay only ever holds
//! verification keys, so it can forward a token but never mint or alter one.
//! This module checks it with the authority keys cached at enrollment — the
//! same material TX leases are verified with — before any key agreement.
//!
//! Rollout: an unsigned key is refused outright. The server signs every
//! ticket that carries a viewer key, so it must be deployed before a client
//! with this check ships (against an older server, Live Console viewers and
//! Live Relay peers cannot join; Live Share is unaffected). There is
//! deliberately no "accept unsigned keys unless the server advertises
//! signing" mode: the server and client ship from one team with the server
//! first, and such a mode would leave a state in which the protection is
//! silently absent.

use crate::cloud_bridge::{ed25519_verify, sha256_hex};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use std::collections::HashMap;

/// Domain separator; must match `PURPOSE_PEER_KEY_AUTHORIZATION` server-side.
pub(crate) const PURPOSE: &str = "peer-key-authorization";

/// What the control plane vouched for. `consumer_*` are present for Live
/// Relay peers: the account's other device and its label, covered by the
/// same signature (unlike the unauthenticated `RELAY_GRANT_ISSUED` hint).
#[derive(Debug, Deserialize)]
pub(crate) struct PeerKeyClaims {
    pub(crate) session_id: String,
    pub(crate) device_id: String,
    pub(crate) peer_key_hash: String,
    pub(crate) role: String,
    pub(crate) exp: i64,
    #[serde(default)]
    pub(crate) consumer_device_id: Option<String>,
    #[serde(default)]
    pub(crate) consumer_label: Option<String>,
}

/// Verify a `cav1.{kid}.{payload}.{sig}` capability signed for `purpose` and
/// return its decoded claims payload. Fails closed on anything malformed.
pub(crate) fn verify_capability(
    authority_keys: &HashMap<String, Vec<u8>>,
    purpose: &str,
    token: &str,
) -> Result<Vec<u8>, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 || parts[0] != "cav1" {
        return Err("malformed authorization".into());
    }
    let key = authority_keys
        .get(parts[1])
        .ok_or_else(|| "authorization signed by an unknown key".to_string())?;
    let signature = URL_SAFE_NO_PAD
        .decode(parts[3])
        .map_err(|_| "malformed authorization signature".to_string())?;
    let signed = format!("{purpose}.{}", parts[2]);
    if !ed25519_verify(key, signed.as_bytes(), &signature) {
        return Err("invalid authorization signature".into());
    }
    URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| "malformed authorization claims".to_string())
}

/// Accept the viewer key in an `E2EE_INIT` frame only if the control plane
/// authorized exactly this key, for this session on this device, in the
/// role the frame claims. Returns the signed claims; callers take the role
/// and any peer identity from them, never from the frame.
pub(crate) fn verify_e2ee_init(
    authority_keys: &HashMap<String, Vec<u8>>,
    device_id: &str,
    frame: &serde_json::Value,
    now_unix: i64,
) -> Result<PeerKeyClaims, String> {
    let field = |name: &str| frame.get(name).and_then(|v| v.as_str()).unwrap_or("");
    let token = field("peer_key_authorization");
    if token.is_empty() {
        return Err("viewer key is not authorized by the control plane".into());
    }
    let payload = verify_capability(authority_keys, PURPOSE, token)?;
    let claims: PeerKeyClaims =
        serde_json::from_slice(&payload).map_err(|_| "invalid authorization claims".to_string())?;
    let peer_public = field("peer_public_key");
    if claims.exp < now_unix {
        return Err("viewer key authorization expired".into());
    }
    if claims.device_id != device_id
        || claims.session_id.is_empty()
        || claims.session_id != field("session_id")
        || peer_public.is_empty()
        || claims.peer_key_hash != sha256_hex(peer_public)
        || claims.role.is_empty()
        || claims.role != field("role")
    {
        return Err("viewer key authorization does not match this handshake".into());
    }
    Ok(claims)
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A stand-in for the control plane's authority key, for tests that
    //! drive E2EE_INIT through the real verification.
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    pub(crate) const TEST_KID: &str = "k1";
    const TEST_AUTHORITY_SEED: [u8; 32] = [42_u8; 32];

    pub(crate) fn authority_keys() -> HashMap<String, Vec<u8>> {
        let public = SigningKey::from_bytes(&TEST_AUTHORITY_SEED).verifying_key().to_bytes().to_vec();
        HashMap::from([(TEST_KID.to_string(), public)])
    }

    /// Sign arbitrary claims as the control plane would.
    pub(crate) fn sign_claims(claims: &serde_json::Value) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
        let signature = SigningKey::from_bytes(&TEST_AUTHORITY_SEED).sign(format!("{PURPOSE}.{payload}").as_bytes());
        format!("cav1.{TEST_KID}.{payload}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    /// The authorization the server attaches to a ticket for `peer_public`.
    pub(crate) fn authorize(session_id: &str, device_id: &str, peer_public: &str, role: &str) -> String {
        sign_claims(&json!({
            "session_id": session_id, "device_id": device_id,
            "peer_key_hash": sha256_hex(peer_public), "role": role,
            "consumer_device_id": "device-b", "consumer_label": "Travel laptop",
            "iat": unix_now(), "exp": unix_now() + 300,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use serde_json::json;

    const DEVICE: &str = "device-test";
    const SESSION: &str = "cloud-session-1";
    const KEY: &str = "BPeerPublicKey";

    fn init(authorization: Option<String>) -> serde_json::Value {
        let mut frame = json!({"kind": "E2EE_INIT", "session_id": SESSION,
            "connection_id": "conn-1", "peer_public_key": KEY, "role": "shared_viewer"});
        if let Some(token) = authorization {
            frame["peer_key_authorization"] = json!(token);
        }
        frame
    }

    #[test]
    fn a_signed_key_for_this_handshake_is_accepted() {
        let frame = init(Some(authorize(SESSION, DEVICE, KEY, "shared_viewer")));
        let claims = verify_e2ee_init(&authority_keys(), DEVICE, &frame, unix_now()).unwrap();
        assert_eq!(claims.role, "shared_viewer");
        assert_eq!(claims.consumer_label.as_deref(), Some("Travel laptop"));
    }

    #[test]
    fn an_unsigned_key_is_refused() {
        assert!(verify_e2ee_init(&authority_keys(), DEVICE, &init(None), unix_now()).is_err());
        assert!(verify_e2ee_init(&authority_keys(), DEVICE, &init(Some(String::new())), unix_now()).is_err());
    }

    /// The relay's attack: keep a real authorization, swap in its own key.
    #[test]
    fn a_substituted_key_is_refused() {
        let mut frame = init(Some(authorize(SESSION, DEVICE, KEY, "shared_viewer")));
        frame["peer_public_key"] = json!("BRelayHeldKey");
        assert!(verify_e2ee_init(&authority_keys(), DEVICE, &frame, unix_now()).is_err());
    }

    #[test]
    fn session_device_and_role_are_bound() {
        let keys = authority_keys();
        let now = unix_now();
        let other_session = init(Some(authorize("other-session", DEVICE, KEY, "shared_viewer")));
        assert!(verify_e2ee_init(&keys, DEVICE, &other_session, now).is_err());
        let other_device = init(Some(authorize(SESSION, "device-other", KEY, "shared_viewer")));
        assert!(verify_e2ee_init(&keys, DEVICE, &other_device, now).is_err());
        // A relay upgrading the role in the frame does not get the upgrade.
        let mut upgraded = init(Some(authorize(SESSION, DEVICE, KEY, "shared_viewer")));
        upgraded["role"] = json!("controller");
        assert!(verify_e2ee_init(&keys, DEVICE, &upgraded, now).is_err());
    }

    /// Wire compatibility with the server: this token was minted by
    /// AuraXLabs' own `ConsoleAuthority.authorize_peer_key` under the key
    /// ring `{"k1": "auraterm-interop-fixture"}` (50-year lifetime), so a
    /// change on either side of the format fails here.
    #[test]
    fn verifies_a_token_minted_by_the_server() {
        const SERVER_PUBLIC_K1: &str = "nXQ_sTEVuj1FkLWBh5RpmytvFSQZCOwRofYH4TWd8cg";
        const VIEWER_KEY: &str =
            "BGsX0fLhLEJH-Lzm5WOkQPJ3A32BLeszoPShOUXYmMKWT-NC4v4af5uO5-tKfA-eFivOM1drMV7Oy7ZAaDe_UfU";
        const TOKEN: &str = concat!(
            "cav1.k1.eyJjb25zdW1lcl9kZXZpY2VfaWQiOiJkZXZpY2UtYiIsImNvbnN1bWVyX2xhYmVsIjoiVHJhdmVsIGxh",
            "cHRvcCIsImRldmljZV9pZCI6ImRldmljZS1pbnRlcm9wIiwiZXhwIjozMzcwMTE4NDAwLCJpYXQiOjE3OTMzMTg0",
            "MDAsInBlZXJfa2V5X2hhc2giOiJjMzYzOWNkZjNmMDY1NzAzYTgyY2U0OGFmZmY3MDJhNWI3M2JkYjhiYTc4YTM0",
            "OWQ2YzBiODA0MTM2NTAwMTA4Iiwicm9sZSI6InJlbGF5X2NvbnRyb2xsZXIiLCJzZXNzaW9uX2lkIjoic2Vzcy1p",
            "bnRlcm9wIn0.gUNepZh3ZguFdSSTkqbtwufgkL-JpmvuKTFWTpITTbx2cliY-Milv-68vKaoY8GnsQyka0Q5rs55",
            "Yat6q292Aw",
        );
        let keys = HashMap::from([("k1".to_string(), URL_SAFE_NO_PAD.decode(SERVER_PUBLIC_K1).unwrap())]);
        let frame = json!({"kind": "E2EE_INIT", "session_id": "sess-interop",
            "connection_id": "conn-1", "peer_public_key": VIEWER_KEY,
            "role": "relay_controller", "peer_key_authorization": TOKEN});
        let claims = verify_e2ee_init(&keys, "device-interop", &frame, unix_now()).unwrap();
        assert_eq!(claims.role, "relay_controller");
        assert_eq!(claims.consumer_device_id.as_deref(), Some("device-b"));
        assert_eq!(claims.consumer_label.as_deref(), Some("Travel laptop"));
        assert!(verify_e2ee_init(&keys, "device-other", &frame, unix_now()).is_err());
    }

    #[test]
    fn expired_forged_and_foreign_signatures_are_refused() {
        let keys = authority_keys();
        let now = unix_now();
        let expired = sign_claims(&json!({"session_id": SESSION, "device_id": DEVICE,
            "peer_key_hash": sha256_hex(KEY), "role": "shared_viewer", "exp": now - 1}));
        assert!(verify_e2ee_init(&keys, DEVICE, &init(Some(expired)), now).is_err());
        // Missing exp (a token minted without a lifetime) fails closed.
        let no_exp = sign_claims(&json!({"session_id": SESSION, "device_id": DEVICE,
            "peer_key_hash": sha256_hex(KEY), "role": "shared_viewer"}));
        assert!(verify_e2ee_init(&keys, DEVICE, &init(Some(no_exp)), now).is_err());
        // Claims edited after signing.
        let token = authorize(SESSION, DEVICE, KEY, "shared_viewer");
        let parts: Vec<&str> = token.split('.').collect();
        let forged_claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"session_id": SESSION, "device_id": DEVICE,
                "peer_key_hash": sha256_hex("BRelayHeldKey"), "role": "shared_viewer",
                "exp": now + 300})).unwrap(),
        );
        let forged = format!("{}.{}.{}.{}", parts[0], parts[1], forged_claims, parts[3]);
        let mut frame = init(Some(forged));
        frame["peer_public_key"] = json!("BRelayHeldKey");
        assert!(verify_e2ee_init(&keys, DEVICE, &frame, now).is_err());
        // Signed by a key this device never cached.
        assert!(verify_e2ee_init(&HashMap::new(), DEVICE, &init(Some(token.clone())), now).is_err());
        // Same signature presented for another purpose (a TX lease).
        assert!(verify_capability(&keys, "lease-grant", &token).is_err());
    }
}
