//! Steward request signatures (ADR-106 section 4 step 3, section 7).
//!
//! Signed string, one field per line, the COG-011 field layout under its own
//! domain, plus the audience: `weft-licence-v1/request`, method, target (path
//! and query), node, `seed_device_id` (the audience: a request signed for one
//! Seed is refused by another), timestamp, nonce, sha256 of the body. Headers: `x-licence-node`,
//! `x-licence-ts` (unix MILLISECONDS, as the bridge's `x-bridge-timestamp`),
//! `x-licence-nonce` (16 to 64 alphanumerics), `x-licence-sig`.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use weft_licence_wire::hex_decode_exact;

pub use weft_licence_wire::request::{
    CLOCK_FLOOR_SECS, MAX_NONCES, MAX_TS_MS, REQUEST_DOMAIN, REQUEST_WINDOW_MS, signing_string,
    valid_nonce,
};

/// A parsed HTTP request (headers lower-cased).
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// `GET`, `POST`.
    pub method: String,
    /// Path plus `?query`.
    pub target: String,
    /// Lower-cased header names.
    pub headers: BTreeMap<String, String>,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl Request {
    /// The path without the query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    /// One query parameter.
    pub fn query(&self, key: &str) -> Option<&str> {
        let q = self.target.split_once('?')?.1;
        q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
    }
}

/// Sign a request as the steward: the headers to send. The signed string is
/// [`signing_string`], defined once in `weft-licence-wire`.
#[allow(clippy::too_many_arguments)]
pub fn sign_request(
    key: &SigningKey,
    node: &str,
    audience: &str,
    method: &str,
    target: &str,
    body: &[u8],
    ts_ms: u64,
    nonce: &str,
) -> BTreeMap<String, String> {
    let sig = weft_licence_wire::request::sign(key, node, audience, method, target, body, ts_ms, nonce);
    BTreeMap::from([
        ("x-licence-node".to_string(), node.to_string()),
        ("x-licence-ts".to_string(), ts_ms.to_string()),
        ("x-licence-nonce".to_string(), nonce.to_string()),
        ("x-licence-sig".to_string(), sig),
    ])
}

/// Why a request was not authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// A header is missing or malformed.
    Malformed,
    /// The node id is not the bound steward's.
    WrongNode,
    /// Timestamp outside the replay window.
    Stale,
    /// The signature does not verify.
    BadSignature,
}

/// What a verified request proves.
#[derive(Debug, Clone)]
pub struct Verified {
    /// The nonce, to be remembered against replay.
    pub nonce: String,
    /// The signed timestamp, unix milliseconds.
    pub ts: u64,
}

/// Check a request against the bound steward. Cheap checks come first, so a
/// forged request costs one signature check at most.
pub fn verify(
    req: &Request,
    steward_pubkey: &[u8; 32],
    steward_node: &str,
    audience: &str,
    now_ms: u64,
    window_ms: u64,
) -> Result<Verified, AuthError> {
    let h = |k: &str| req.headers.get(k).map(String::as_str).ok_or(AuthError::Malformed);
    let node = h("x-licence-node")?;
    let ts_raw = h("x-licence-ts")?;
    if ts_raw.is_empty() || ts_raw.len() > 16 || !ts_raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AuthError::Malformed);
    }
    let ts: u64 = ts_raw.parse().map_err(|_| AuthError::Malformed)?;
    if !weft_licence_wire::request::valid_ts(ts) {
        return Err(AuthError::Malformed);
    }
    let nonce = h("x-licence-nonce")?;
    let sig = h("x-licence-sig")?;
    if hex_decode_exact::<64>(sig).is_none() || !valid_nonce(nonce) {
        return Err(AuthError::Malformed);
    }
    if node != steward_node {
        return Err(AuthError::WrongNode);
    }
    if !weft_licence_wire::request::within_window(ts, now_ms, window_ms) {
        return Err(AuthError::Stale);
    }
    if !weft_licence_wire::request::verify_signature(
        steward_pubkey, node, audience, &req.method, &req.target, &req.body, ts, nonce, sig,
    ) {
        return Err(AuthError::BadSignature);
    }
    Ok(Verified { nonce: nonce.to_string(), ts })
}

/// Remember `nonce` in `seen` (nonce, ts in ms). `false` when it was already seen
/// (a replay) or the list is full of live entries. Old entries are pruned.
pub fn remember_nonce(seen: &mut Vec<(String, u64)>, nonce: &str, ts: u64, now_ms: u64, window_ms: u64) -> bool {
    seen.retain(|(_, t)| t.saturating_add(window_ms) >= now_ms);
    if seen.iter().any(|(n, _)| n == nonce) || seen.len() >= MAX_NONCES {
        return false;
    }
    seen.push((nonce.to_string(), ts));
    true
}
