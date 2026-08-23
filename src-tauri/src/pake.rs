//! SPAKE2 (RFC 9382) over NIST P-256 with SHA-256 / HKDF-SHA256 / HMAC-SHA256.
//!
//! Used by Remote Assist so that a host and a guest who both know the
//! assist-code secret can authenticate each other and agree on a session
//! key *through* an untrusted relay and control plane. The secret never
//! leaves either endpoint; an observer of the exchange learns nothing it can
//! test offline, and an active attacker gets exactly one online guess per
//! session (the one committed in its own share).
//!
//! The implementation follows the RFC byte for byte (constants, transcript
//! layout, key schedule) and is locked by the RFC's published P-256 test
//! vector plus a cross-language interop fixture shared with the browser
//! implementation in AuraXLab.

// Consumed by the assist host/guest modules (design Phase 2/4).
#![allow(dead_code)]

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use p256::elliptic_curve::{
    group::Group,
    ops::Reduce,
    sec1::{FromEncodedPoint, ToEncodedPoint},
    Field, PrimeField,
};
use p256::{AffinePoint, EncodedPoint, ProjectivePoint, Scalar, U256};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

/// RFC 9382 §4, P-256 "M" point (compressed SEC1).
const M_COMPRESSED: &str = "02886e2f97ace46e55ba9dd7242579f2993b64e16ef3dcab95afd497333d8fa12f";
/// RFC 9382 §4, P-256 "N" point (compressed SEC1).
const N_COMPRESSED: &str = "03d8bbd6c639c62937b04d997f38c3770719c629d7014d49a24b4f98baa1292b49";
const CONFIRMATION_INFO: &[u8] = b"ConfirmationKeys";
pub const SHARE_LEN: usize = 65;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// The party that sends its share first (the Remote Assist guest).
    A,
    /// The party that replies with its share plus confirmation (the host).
    B,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PakeError {
    InvalidShare,
    InvalidScalar,
}

impl std::fmt::Display for PakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PakeError::InvalidShare => write!(f, "invalid PAKE share"),
            PakeError::InvalidScalar => write!(f, "invalid PAKE scalar"),
        }
    }
}

fn decode_hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).expect("valid hex constant"))
        .collect()
}

fn fixed_point(compressed_hex: &str) -> ProjectivePoint {
    let encoded = EncodedPoint::from_bytes(decode_hex(compressed_hex)).expect("valid SEC1 constant");
    let affine = AffinePoint::from_encoded_point(&encoded).expect("constant is on the curve");
    ProjectivePoint::from(affine)
}

fn m_point() -> ProjectivePoint {
    fixed_point(M_COMPRESSED)
}

fn n_point() -> ProjectivePoint {
    fixed_point(N_COMPRESSED)
}

fn encode_point(point: &ProjectivePoint) -> [u8; SHARE_LEN] {
    let encoded = point.to_affine().to_encoded_point(false);
    let mut out = [0_u8; SHARE_LEN];
    out.copy_from_slice(encoded.as_bytes());
    out
}

fn decode_share(bytes: &[u8]) -> Result<ProjectivePoint, PakeError> {
    if bytes.len() != SHARE_LEN || bytes[0] != 0x04 {
        return Err(PakeError::InvalidShare);
    }
    let encoded = EncodedPoint::from_bytes(bytes).map_err(|_| PakeError::InvalidShare)?;
    let affine: Option<AffinePoint> = AffinePoint::from_encoded_point(&encoded).into();
    let point = ProjectivePoint::from(affine.ok_or(PakeError::InvalidShare)?);
    if bool::from(point.is_identity()) {
        return Err(PakeError::InvalidShare);
    }
    Ok(point)
}

fn scalar_from_bytes(bytes: &[u8; 32]) -> Result<Scalar, PakeError> {
    let repr = p256::FieldBytes::from(*bytes);
    let scalar: Option<Scalar> = Scalar::from_repr(repr).into();
    let scalar = scalar.ok_or(PakeError::InvalidScalar)?;
    if bool::from(scalar.is_zero()) {
        return Err(PakeError::InvalidScalar);
    }
    Ok(scalar)
}

fn scalar_bytes(scalar: &Scalar) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(scalar.to_bytes().as_slice());
    out
}

/// Reduce a 64-byte big-endian integer modulo the group order: RFC 9382
/// §3.2 asks for `MHF(pw) mod p` computed from a hash at least 64 bits
/// longer than `p`, so the result is statistically uniform.
pub fn reduce_wide(wide: &[u8; 64]) -> Zeroizing<[u8; 32]> {
    let hi = Scalar::reduce(U256::from_be_slice(&wide[..32]));
    let lo = Scalar::reduce(U256::from_be_slice(&wide[32..]));
    // 2^256 mod n == (2^256 - 1 mod n) + 1.
    let two_pow_256 = Scalar::reduce(U256::MAX) + Scalar::ONE;
    let w = hi * two_pow_256 + lo;
    Zeroizing::new(scalar_bytes(&w))
}

fn length_prefixed(out: &mut Vec<u8>, chunk: &[u8]) {
    out.extend_from_slice(&(chunk.len() as u64).to_le_bytes());
    out.extend_from_slice(chunk);
}

/// In-flight SPAKE2 state for one party. Consumed by [`Spake2::finish`].
pub struct Spake2 {
    role: Role,
    w: Scalar,
    secret: Scalar,
    share: [u8; SHARE_LEN],
    id_a: Vec<u8>,
    id_b: Vec<u8>,
}

impl Drop for Spake2 {
    fn drop(&mut self) {
        self.w = Scalar::ZERO;
        self.secret = Scalar::ZERO;
    }
}

impl Spake2 {
    /// Start with a fresh random ephemeral scalar.
    pub fn new(role: Role, w: &[u8; 32], id_a: &[u8], id_b: &[u8]) -> Result<Self, PakeError> {
        let secret = Scalar::random(&mut rand::rngs::OsRng);
        Self::with_scalar(role, w, &scalar_bytes(&secret), id_a, id_b)
    }

    /// Start with a caller-supplied ephemeral scalar (test vectors only).
    pub fn with_scalar(role: Role, w: &[u8; 32], secret: &[u8; 32], id_a: &[u8], id_b: &[u8]) -> Result<Self, PakeError> {
        let w = scalar_from_bytes(w)?;
        let secret = scalar_from_bytes(secret)?;
        let blind = match role {
            Role::A => m_point(),
            Role::B => n_point(),
        };
        let share = encode_point(&(blind * w + ProjectivePoint::GENERATOR * secret));
        Ok(Self {
            role,
            w,
            secret,
            share,
            id_a: id_a.to_vec(),
            id_b: id_b.to_vec(),
        })
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Our public share (`pA` or `pB`), uncompressed SEC1.
    pub fn share(&self) -> &[u8; SHARE_LEN] {
        &self.share
    }

    /// Combine with the peer's share and derive the key schedule.
    pub fn finish(self, peer_share: &[u8]) -> Result<Spake2Keys, PakeError> {
        let peer = decode_share(peer_share)?;
        let peer_blind = match self.role {
            Role::A => n_point(),
            Role::B => m_point(),
        };
        let k = (peer - peer_blind * self.w) * self.secret;
        if bool::from(k.is_identity()) {
            return Err(PakeError::InvalidShare);
        }
        let (pa, pb) = match self.role {
            Role::A => (self.share.to_vec(), peer_share.to_vec()),
            Role::B => (peer_share.to_vec(), self.share.to_vec()),
        };
        let mut transcript = Vec::with_capacity(8 * 6 + self.id_a.len() + self.id_b.len() + 3 * SHARE_LEN + 32);
        length_prefixed(&mut transcript, &self.id_a);
        length_prefixed(&mut transcript, &self.id_b);
        length_prefixed(&mut transcript, &pa);
        length_prefixed(&mut transcript, &pb);
        length_prefixed(&mut transcript, &encode_point(&k));
        length_prefixed(&mut transcript, &scalar_bytes(&self.w));

        let digest = Sha256::digest(&transcript);
        let mut ke = Zeroizing::new([0_u8; 16]);
        let mut ka = Zeroizing::new([0_u8; 16]);
        ke.copy_from_slice(&digest[..16]);
        ka.copy_from_slice(&digest[16..]);
        let mut confirmation_keys = Zeroizing::new([0_u8; 32]);
        Hkdf::<Sha256>::new(None, ka.as_slice())
            .expand(CONFIRMATION_INFO, confirmation_keys.as_mut_slice())
            .expect("32-byte HKDF output");
        let mut kc_a = Zeroizing::new([0_u8; 16]);
        let mut kc_b = Zeroizing::new([0_u8; 16]);
        kc_a.copy_from_slice(&confirmation_keys[..16]);
        kc_b.copy_from_slice(&confirmation_keys[16..]);
        let confirm_a = hmac_sha256(kc_a.as_slice(), &transcript);
        let confirm_b = hmac_sha256(kc_b.as_slice(), &transcript);
        Ok(Spake2Keys {
            role: self.role,
            ke,
            transcript,
            confirm_a,
            confirm_b,
        })
    }
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// Output of a completed SPAKE2 run. Confirmation values are kept so the
/// caller can verify the peer's and send its own.
pub struct Spake2Keys {
    role: Role,
    ke: Zeroizing<[u8; 16]>,
    transcript: Vec<u8>,
    confirm_a: [u8; 32],
    confirm_b: [u8; 32],
}

impl std::fmt::Debug for Spake2Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spake2Keys").field("role", &self.role).finish_non_exhaustive()
    }
}

impl Drop for Spake2Keys {
    fn drop(&mut self) {
        self.transcript.zeroize();
        self.confirm_a.zeroize();
        self.confirm_b.zeroize();
    }
}

impl Spake2Keys {
    /// The confirmation MAC we send (`cA` for role A, `cB` for role B).
    pub fn own_confirmation(&self) -> &[u8; 32] {
        match self.role {
            Role::A => &self.confirm_a,
            Role::B => &self.confirm_b,
        }
    }

    /// Constant-time check of the peer's confirmation MAC.
    pub fn verify_peer_confirmation(&self, value: &[u8]) -> bool {
        let expected = match self.role {
            Role::A => &self.confirm_b,
            Role::B => &self.confirm_a,
        };
        value.len() == expected.len() && bool::from(value.ct_eq(expected))
    }

    /// Derive an application key from `Ke`: HKDF-SHA256 with the transcript
    /// hash as salt so the key is bound to everything both sides agreed on.
    pub fn derive_key(&self, info: &[u8]) -> Zeroizing<[u8; 32]> {
        let salt = Sha256::digest(&self.transcript);
        let mut key = Zeroizing::new([0_u8; 32]);
        Hkdf::<Sha256>::new(Some(&salt), self.ke.as_slice())
            .expand(info, key.as_mut_slice())
            .expect("32-byte HKDF output");
        key
    }

    /// Transcript hash-derived short string both sides can display.
    pub fn fingerprint(&self, label: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(label);
        hasher.update(&self.transcript);
        let digest = hasher.finalize();
        base32_no_pad(&digest[..5])
    }

    #[cfg(test)]
    pub(crate) fn transcript(&self) -> &[u8] {
        &self.transcript
    }

    #[cfg(test)]
    pub(crate) fn ke(&self) -> &[u8; 16] {
        &self.ke
    }
}

/// RFC 4648 base32 (uppercase, no padding) of the first bytes; 5 bytes → 8 chars.
fn base32_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex32(value: &str) -> [u8; 32] {
        let mut out = [0_u8; 32];
        out.copy_from_slice(&decode_hex(value));
        out
    }

    /// RFC 9382 Appendix B, SPAKE2-P256-SHA256-HKDF-HMAC, first vector
    /// (A = "server", B = "client"; w, x, y supplied directly).
    #[test]
    fn rfc9382_p256_vector() {
        let w = hex32("2ee57912099d31560b3a44b1184b9b4866e904c49d12ac5042c97dca461b1a5f");
        let x = hex32("43dd0fd7215bdcb482879fca3220c6a968e66d70b1356cac18bb26c84a78d729");
        let y = hex32("dcb60106f276b02606d8ef0a328c02e4b629f84f89786af5befb0bc75b6e66be");
        let a = Spake2::with_scalar(Role::A, &w, &x, b"server", b"client").unwrap();
        let b = Spake2::with_scalar(Role::B, &w, &y, b"server", b"client").unwrap();
        assert_eq!(
            a.share().to_vec(),
            decode_hex("04a56fa807caaa53a4d28dbb9853b9815c61a411118a6fe516a8798434751470f9010153ac33d0d5f2047ffdb1a3e42c9b4e6be662766e1eeb4116988ede5f912c")
        );
        assert_eq!(
            b.share().to_vec(),
            decode_hex("0406557e482bd03097ad0cbaa5df82115460d951e3451962f1eaf4367a420676d09857ccbc522686c83d1852abfa8ed6e4a1155cf8f1543ceca528afb591a1e0b7")
        );
        let pa = *a.share();
        let pb = *b.share();
        let ka = a.finish(&pb).unwrap();
        let kb = b.finish(&pa).unwrap();
        assert_eq!(ka.transcript(), kb.transcript());
        assert_eq!(
            Sha256::digest(ka.transcript()).to_vec(),
            decode_hex("0e0672dc86f8e45565d338b0540abe6915bdf72e2b35b5c9e5663168e960a91b")
        );
        assert_eq!(ka.ke().to_vec(), decode_hex("0e0672dc86f8e45565d338b0540abe69"));
        assert_eq!(
            ka.own_confirmation().to_vec(),
            decode_hex("58ad4aa88e0b60d5061eb6b5dd93e80d9c4f00d127c65b3b35b1b5281fee38f0")
        );
        assert_eq!(
            kb.own_confirmation().to_vec(),
            decode_hex("d3e2e547f1ae04f2dbdbf0fc4b79f8ecff2dff314b5d32fe9fcef2fb26dc459b")
        );
        assert!(ka.verify_peer_confirmation(kb.own_confirmation()));
        assert!(kb.verify_peer_confirmation(ka.own_confirmation()));
        assert!(!ka.verify_peer_confirmation(ka.own_confirmation()));
        assert_eq!(ka.derive_key(b"info").as_slice(), kb.derive_key(b"info").as_slice());
        assert_eq!(ka.fingerprint(b"sas"), kb.fingerprint(b"sas"));
        assert_eq!(ka.fingerprint(b"sas").len(), 8);
    }

    #[test]
    fn wrong_password_fails_confirmation_and_random_runs_agree() {
        let w_good = reduce_wide(&[3_u8; 64]);
        let w_bad = reduce_wide(&[4_u8; 64]);
        let a = Spake2::new(Role::A, &w_good, b"guest", b"host").unwrap();
        let b = Spake2::new(Role::B, &w_bad, b"guest", b"host").unwrap();
        let pa = *a.share();
        let pb = *b.share();
        let ka = a.finish(&pb).unwrap();
        let kb = b.finish(&pa).unwrap();
        assert!(!ka.verify_peer_confirmation(kb.own_confirmation()));
        assert!(!kb.verify_peer_confirmation(ka.own_confirmation()));

        let a = Spake2::new(Role::A, &w_good, b"guest", b"host").unwrap();
        let b = Spake2::new(Role::B, &w_good, b"guest", b"host").unwrap();
        let pa = *a.share();
        let pb = *b.share();
        let ka = a.finish(&pb).unwrap();
        let kb = b.finish(&pa).unwrap();
        assert!(ka.verify_peer_confirmation(kb.own_confirmation()));
        assert!(kb.verify_peer_confirmation(ka.own_confirmation()));
        assert_eq!(ka.derive_key(b"k").as_slice(), kb.derive_key(b"k").as_slice());
    }

    #[test]
    fn malformed_shares_are_rejected() {
        let w = reduce_wide(&[9_u8; 64]);
        let a = Spake2::new(Role::A, &w, b"a", b"b").unwrap();
        assert_eq!(a.finish(&[0x04; 10]).unwrap_err(), PakeError::InvalidShare);
        let a = Spake2::new(Role::A, &w, b"a", b"b").unwrap();
        let mut off_curve = [0x04_u8; SHARE_LEN];
        off_curve[1..].fill(0x01);
        assert_eq!(a.finish(&off_curve).unwrap_err(), PakeError::InvalidShare);
        let a = Spake2::new(Role::A, &w, b"a", b"b").unwrap();
        let mut compressed = [0_u8; SHARE_LEN];
        compressed[0] = 0x02;
        assert_eq!(a.finish(&compressed).unwrap_err(), PakeError::InvalidShare);
    }

    #[test]
    fn reduce_wide_matches_modular_arithmetic() {
        // 2^256 mod n must equal the known constant n' = 2^256 - n.
        let mut wide = [0_u8; 64];
        wide[31] = 1; // hi = 1, lo = 0 → 2^256
        let reduced = reduce_wide(&wide);
        assert_eq!(
            reduced.to_vec(),
            decode_hex("00000000ffffffff00000000000000004319055258e8617b0c46353d039cdaaf")
        );
        assert_eq!(base32_no_pad(&[0, 0, 0, 0, 0]), "AAAAAAAA");
    }
}
