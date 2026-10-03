//! Steward to `weft-licence` request signing: the one definition both the
//! steward's client (kernel) and the Seed service (`weft-licence`) use.
//!
//! The signed string is one field per line, the COG-011 bridge layout under
//! its own domain plus the audience:
//!
//! ```text
//! weft-licence-v1/request \n METHOD \n target \n node \n seed_device_id \n ts_ms \n nonce \n sha256(body)
//! ```
//!
//! `target` is the path and query. `seed_device_id` is the audience (the
//! bound binding's `device_id`): a request signed for one Seed fails at
//! another. The timestamp is unix MILLISECONDS; the nonce is 16 to 64 ASCII
//! alphanumerics. Header transport and replay memory stay with each side.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::{hex_decode_exact, hex_encode};

/// Domain tag of steward requests.
pub const REQUEST_DOMAIN: &str = "weft-licence-v1/request";
/// Latest accepted timestamp (year 2100), in ms, as the bridge.
pub const MAX_TS_MS: u64 = 4_102_444_800_000;
/// The COG-011 bridge's clock floor (`CLOCK_FLOOR_MS` / 1000), unix seconds
/// (2026-05-28 UTC). A clock below it is not set: nothing is signed or issued.
pub const CLOCK_FLOOR_SECS: u64 = 1_780_000_000;
/// Default replay window, each side of the verifier's clock.
pub const REQUEST_WINDOW_MS: u64 = 120_000;
/// Most nonces a verifier remembers inside the window.
pub const MAX_NONCES: usize = 4096;

/// The bytes the steward key signs.
pub fn signing_string(
    method: &str,
    target: &str,
    node: &str,
    audience: &str,
    ts_ms: u64,
    nonce: &str,
    body: &[u8],
) -> String {
    format!(
        "{REQUEST_DOMAIN}\n{method}\n{target}\n{node}\n{audience}\n{ts_ms}\n{nonce}\n{}",
        hex_encode(&Sha256::digest(body))
    )
}

/// A nonce the server accepts: 16 to 64 ASCII alphanumerics.
pub fn valid_nonce(n: &str) -> bool {
    (16..=64).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A timestamp inside the accepted range.
pub fn valid_ts(ts_ms: u64) -> bool {
    ts_ms <= MAX_TS_MS
}

/// Whether `ts_ms` is within `window_ms` of `now_ms`.
pub fn within_window(ts_ms: u64, now_ms: u64, window_ms: u64) -> bool {
    ts_ms.abs_diff(now_ms) <= window_ms
}

/// Sign a request: the signature as 128 lower-case hex chars.
#[allow(clippy::too_many_arguments)]
pub fn sign(
    key: &SigningKey,
    node: &str,
    audience: &str,
    method: &str,
    target: &str,
    body: &[u8],
    ts_ms: u64,
    nonce: &str,
) -> String {
    let s = signing_string(method, target, node, audience, ts_ms, nonce, body);
    hex_encode(&key.sign(s.as_bytes()).to_bytes())
}

/// Check a request's signature (strictly) against the bound steward key.
/// `sig_hex` must be 128 lower-case hex chars; false for anything else.
#[allow(clippy::too_many_arguments)]
pub fn verify_signature(
    steward_pubkey: &[u8; 32],
    node: &str,
    audience: &str,
    method: &str,
    target: &str,
    body: &[u8],
    ts_ms: u64,
    nonce: &str,
    sig_hex: &str,
) -> bool {
    let (Some(sig), Ok(vk)) =
        (hex_decode_exact::<64>(sig_hex), VerifyingKey::from_bytes(steward_pubkey))
    else {
        return false;
    };
    let s = signing_string(method, target, node, audience, ts_ms, nonce, body);
    vk.verify_strict(s.as_bytes(), &Signature::from_bytes(&sig)).is_ok()
}
