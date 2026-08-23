//! Remote Assist building blocks shared by the host and guest sides: the
//! assist code format, identity strings, and the SPAKE2 → session-key
//! schedule. Design: `docs/plans/remote-assist-design.md`.
//!
//! An assist code is `RRRR-SSSS-SSSS`: a 4-character *route* segment the
//! control plane assigns and a guest submits to find the host, plus an
//! 8-character *secret* segment generated on the host that is never sent
//! anywhere — both sides prove knowledge of it to each other with SPAKE2.

// Consumed by the assist host/guest modules (design Phase 2/4).
#![allow(dead_code)]

use crate::pake::{reduce_wide, Role, Spake2, Spake2Keys};
use pbkdf2::pbkdf2_hmac_array;
use rand::Rng;
use sha2::Sha256;
use zeroize::Zeroizing;

/// No look-alike glyphs; identical to the device-enrollment user code.
pub const CODE_ALPHABET: &[u8; 28] = b"BCDFGHJKLMNPQRSTVWXZ23456789";
pub const ROUTE_LEN: usize = 4;
pub const SECRET_LEN: usize = 8;
pub const PROTOCOL_VERSION: u32 = 1;
pub const PBKDF2_ROUNDS: u32 = 100_000;
const PBKDF2_SALT_PREFIX: &str = "auraxlab-assist-v1|";
const GUEST_IDENTITY_PREFIX: &str = "auraxlab-assist-guest|";
const HOST_IDENTITY_PREFIX: &str = "auraxlab-assist-host|";
const SESSION_KEY_INFO_PREFIX: &str = "auraxlab-assist|e2ee-v1|";
const FINGERPRINT_LABEL: &[u8] = b"auraxlab-assist-sas|";

pub struct AssistCode {
    pub route: String,
    pub secret: Zeroizing<String>,
}

impl AssistCode {
    pub fn formatted(&self) -> String {
        format_code(&self.route, &self.secret)
    }
}

/// CSPRNG secret segment from the code alphabet.
pub fn generate_secret() -> Zeroizing<String> {
    let mut rng = rand::rngs::OsRng;
    Zeroizing::new(
        (0..SECRET_LEN)
            .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
            .collect(),
    )
}

pub fn format_code(route: &str, secret: &str) -> String {
    format!("{}-{}-{}", route, &secret[..4], &secret[4..])
}

/// Normalise user input: a bare code, a code with any separators, or an
/// assist link (`https://auraxlab.com/assist#CODE`). Case-insensitive.
pub fn parse_code(input: &str) -> Result<AssistCode, String> {
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
        return Err("assist code must have 12 characters".into());
    }
    if !normalised.bytes().all(|b| CODE_ALPHABET.contains(&b)) {
        return Err("assist code contains an invalid character".into());
    }
    Ok(AssistCode {
        route: normalised[..ROUTE_LEN].to_string(),
        secret: Zeroizing::new(normalised[ROUTE_LEN..].to_string()),
    })
}

/// RFC 9382 `w = MHF(pw) mod p`: PBKDF2-HMAC-SHA256 (64-byte output) with a
/// per-session salt, reduced modulo the P-256 group order.
pub fn derive_w(secret: &str, assist_id: &str) -> Zeroizing<[u8; 32]> {
    let salt = format!("{PBKDF2_SALT_PREFIX}{assist_id}");
    let wide: [u8; 64] = pbkdf2_hmac_array::<Sha256, 64>(secret.as_bytes(), salt.as_bytes(), PBKDF2_ROUNDS);
    let wide = Zeroizing::new(wide);
    reduce_wide(&wide)
}

pub fn guest_identity(assist_id: &str, connection_id: &str) -> Vec<u8> {
    format!("{GUEST_IDENTITY_PREFIX}{assist_id}|{connection_id}").into_bytes()
}

pub fn host_identity(assist_id: &str, connection_id: &str) -> Vec<u8> {
    format!("{HOST_IDENTITY_PREFIX}{assist_id}|{connection_id}").into_bytes()
}

/// Start the host (role B) side of the handshake for one guest connection.
pub fn host_pake(w: &[u8; 32], assist_id: &str, connection_id: &str) -> Result<Spake2, String> {
    Spake2::new(Role::B, w, &guest_identity(assist_id, connection_id), &host_identity(assist_id, connection_id))
        .map_err(|e| e.to_string())
}

/// Start the guest (role A) side of the handshake.
pub fn guest_pake(w: &[u8; 32], assist_id: &str, connection_id: &str) -> Result<Spake2, String> {
    Spake2::new(Role::A, w, &guest_identity(assist_id, connection_id), &host_identity(assist_id, connection_id))
        .map_err(|e| e.to_string())
}

/// AES-256-GCM key for the E2EE envelope on this connection.
pub fn session_key(keys: &Spake2Keys, assist_id: &str, connection_id: &str) -> Zeroizing<[u8; 32]> {
    keys.derive_key(format!("{SESSION_KEY_INFO_PREFIX}{assist_id}|{connection_id}").as_bytes())
}

/// `XXXX-XXXX` shown on both ends for optional out-of-band comparison.
pub fn fingerprint(keys: &Spake2Keys) -> String {
    let raw = keys.fingerprint(FINGERPRINT_LABEL);
    format!("{}-{}", &raw[..4], &raw[4..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pake::{Role, Spake2};

    fn hex(value: &str) -> Vec<u8> {
        (0..value.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex32(value: &str) -> [u8; 32] {
        let mut out = [0_u8; 32];
        out.copy_from_slice(&hex(value));
        out
    }

    #[test]
    fn code_parsing_normalises_links_separators_and_case() {
        let code = parse_code("https://auraxlab.com/assist#bcdf-ghjk-lmnp").unwrap();
        assert_eq!(code.route, "BCDF");
        assert_eq!(code.secret.as_str(), "GHJKLMNP");
        assert_eq!(code.formatted(), "BCDF-GHJK-LMNP");
        assert_eq!(parse_code(" bcdfghjklmnp ").unwrap().route, "BCDF");
        assert_eq!(parse_code("BCDF GHJK LMNP").unwrap().formatted(), "BCDF-GHJK-LMNP");
        assert!(parse_code("BCDF-GHJK-LMN").is_err());
        assert!(parse_code("BCDF-GHJK-LMNO").is_err()); // O is not in the alphabet
        assert!(parse_code("BCDF-GHJK-LMN1").is_err());
        let secret = generate_secret();
        assert_eq!(secret.len(), SECRET_LEN);
        assert!(secret.bytes().all(|b| CODE_ALPHABET.contains(&b)));
    }

    /// Cross-language fixture shared with AuraXLab's browser implementation
    /// (`tests/fixtures/spake2_interop.json` in both repositories).
    #[test]
    fn interop_fixture_reproduces_every_field() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/spake2_interop.json")).unwrap();
        let field = |name: &str| fixture[name].as_str().unwrap().to_string();
        let assist_id = field("assist_id");
        let connection_id = field("connection_id");

        let code = parse_code(&field("code")).unwrap();
        assert_eq!(code.route, field("route"));
        assert_eq!(code.secret.as_str(), field("secret"));

        let w = derive_w(&code.secret, &assist_id);
        assert_eq!(w.to_vec(), hex(&field("w")));
        assert_eq!(guest_identity(&assist_id, &connection_id), field("identity_a").into_bytes());
        assert_eq!(host_identity(&assist_id, &connection_id), field("identity_b").into_bytes());

        let guest = Spake2::with_scalar(
            Role::A,
            &w,
            &hex32(&field("x")),
            &guest_identity(&assist_id, &connection_id),
            &host_identity(&assist_id, &connection_id),
        )
        .unwrap();
        let host = Spake2::with_scalar(
            Role::B,
            &w,
            &hex32(&field("y")),
            &guest_identity(&assist_id, &connection_id),
            &host_identity(&assist_id, &connection_id),
        )
        .unwrap();
        assert_eq!(guest.share().to_vec(), hex(&field("pa")));
        assert_eq!(host.share().to_vec(), hex(&field("pb")));
        let pa = *guest.share();
        let pb = *host.share();
        let guest_keys = guest.finish(&pb).unwrap();
        let host_keys = host.finish(&pa).unwrap();
        assert_eq!(guest_keys.transcript().to_vec(), hex(&field("tt")));
        assert_eq!(guest_keys.own_confirmation().to_vec(), hex(&field("confirm_a")));
        assert_eq!(host_keys.own_confirmation().to_vec(), hex(&field("confirm_b")));
        assert!(host_keys.verify_peer_confirmation(guest_keys.own_confirmation()));
        assert!(guest_keys.verify_peer_confirmation(host_keys.own_confirmation()));
        assert_eq!(
            session_key(&guest_keys, &assist_id, &connection_id).to_vec(),
            hex(&field("session_key"))
        );
        assert_eq!(
            session_key(&host_keys, &assist_id, &connection_id).to_vec(),
            hex(&field("session_key"))
        );
        assert_eq!(fingerprint(&guest_keys), field("fingerprint"));
        assert_eq!(fingerprint(&host_keys), field("fingerprint"));
    }

    #[test]
    fn host_and_guest_helpers_agree_with_random_scalars() {
        let w = derive_w("GHJKLMNP", "assist-1");
        let guest = guest_pake(&w, "assist-1", "conn-1").unwrap();
        let host = host_pake(&w, "assist-1", "conn-1").unwrap();
        let pa = *guest.share();
        let pb = *host.share();
        let g = guest.finish(&pb).unwrap();
        let h = host.finish(&pa).unwrap();
        assert!(h.verify_peer_confirmation(g.own_confirmation()));
        assert_eq!(
            session_key(&g, "assist-1", "conn-1").as_slice(),
            session_key(&h, "assist-1", "conn-1").as_slice()
        );
        // A different connection id changes the identities and therefore the key.
        let other = host_pake(&w, "assist-1", "conn-2").unwrap();
        let guest = guest_pake(&w, "assist-1", "conn-1").unwrap();
        let pa = *guest.share();
        let pb = *other.share();
        let g = guest.finish(&pb).unwrap();
        let o = other.finish(&pa).unwrap();
        assert!(!g.verify_peer_confirmation(o.own_confirmation()));
    }
}
