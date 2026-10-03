//! Steward to `weft-licence` request signing (ADR-106 sections 4 and 7).
//!
//! The layout matches `weft-licence`'s server (`crates/weft-licence/src/request.rs`
//! on the phase 2 branch) exactly, which follows the COG-011 bridge: one field
//! per line under its own domain,
//!
//! ```text
//! weft-licence-v1/request \n METHOD \n target \n node \n ts_ms \n nonce \n sha256(body)
//! ```
//!
//! `target` is the path and query. The timestamp is unix MILLISECONDS. The
//! nonce is 16 to 64 ASCII alphanumerics. The request headers are
//! `x-licence-node`, `x-licence-ts`, `x-licence-nonce` and `x-licence-sig`;
//! the verifier already knows the bound steward key, so none is sent.
//!
//! The builder is duplicated here rather than shared: `weft-licence-wire`
//! carries only the grant wire types, and the kernel must not depend on the
//! Seed service crate. [`tests::golden_vector_matches_weft_licence`] pins the
//! bytes so a change on either side shows up. Moving [`signing_string`] into
//! `weft-licence-wire` would make this one definition.
//!
//! Open: phase 2 will add `seed_device_id` (the audience) as one more signed
//! line. Add it to [`signing_string`] and the client when that lands.

use std::collections::HashMap;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use super::sha256_hex;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};

/// Domain tag of steward to licence requests.
pub const REQUEST_DOMAIN: &str = "weft-licence-v1/request";
/// A request timestamp may differ from the verifier's clock by this much.
pub const REQUEST_WINDOW_MS: u64 = 120_000;
/// Latest accepted timestamp (year 2100), in ms.
pub const MAX_TS_MS: u64 = 4_102_444_800_000;
/// A signer whose clock is below this (unix seconds) is not set: it refuses
/// to sign, as the COG-011 bridge does.
pub const CLOCK_FLOOR_SECS: u64 = 1_780_000_000;
/// Largest request body the signer will produce or a verifier accept.
pub const MAX_REQUEST_BODY: usize = 16 * 1024;
/// Replay memory of one verifier.
const MAX_NONCES: usize = 4096;

/// The signature headers of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAuth {
    /// The steward's node id (`x-licence-node`).
    pub node: String,
    /// Signing time, unix milliseconds (`x-licence-ts`).
    pub timestamp_ms: u64,
    /// 16 to 64 alphanumerics (`x-licence-nonce`).
    pub nonce: String,
    /// Ed25519 signature over [`signing_string`], 128 hex (`x-licence-sig`).
    pub signature: String,
}

/// One request in HTTP shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenceRequest {
    /// `GET` or `POST`.
    pub method: String,
    /// Path and query, for example `/licence/v1/checkout`.
    pub path: String,
    /// `None` only for the unauthenticated identity endpoint.
    pub auth: Option<RequestAuth>,
    /// Body bytes (empty for `GET`).
    pub body: Vec<u8>,
}

/// The bytes the steward key signs.
pub fn signing_string(
    method: &str,
    target: &str,
    node: &str,
    ts_ms: u64,
    nonce: &str,
    body: &[u8],
) -> String {
    format!("{REQUEST_DOMAIN}\n{method}\n{target}\n{node}\n{ts_ms}\n{nonce}\n{}", sha256_hex(body))
}

/// A nonce the server accepts: 16 to 64 ASCII alphanumerics.
pub fn valid_nonce(n: &str) -> bool {
    (16..=64).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Sign a request with the steward key. `nonce` must satisfy [`valid_nonce`].
pub fn sign_request(
    key: &SigningKey,
    node: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
    ts_ms: u64,
    nonce: &str,
) -> LicenceRequest {
    let sig = key.sign(signing_string(method, path, node, ts_ms, nonce, &body).as_bytes());
    LicenceRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        auth: Some(RequestAuth {
            node: node.to_owned(),
            timestamp_ms: ts_ms,
            nonce: nonce.to_owned(),
            signature: hex_encode(&sig.to_bytes()),
        }),
        body,
    }
}

/// Why a verifier refused a request. All are refused before any work is done.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestRefused {
    /// No signature headers.
    #[error("request is not signed")]
    Unsigned,
    /// Body over [`MAX_REQUEST_BODY`].
    #[error("request body too large")]
    TooLarge,
    /// A node id other than the bound steward's.
    #[error("request names a node that is not the bound steward")]
    WrongNode,
    /// Timestamp outside the window.
    #[error("request timestamp is outside the window")]
    Stale,
    /// A malformed header value.
    #[error("malformed request header")]
    Malformed,
    /// The signature does not verify.
    #[error("request signature does not verify")]
    BadSignature,
    /// This nonce was already used inside the window.
    #[error("request nonce was already used")]
    Replay,
}

/// Verifier-side replay memory: nonce to its signed timestamp (ms). Entries
/// older than the window are evicted by timestamp; when the memory is full of
/// live entries a new request is refused, never the whole set forgotten.
#[derive(Debug, Default)]
pub struct ReplayGuard {
    seen: HashMap<String, u64>,
}

impl ReplayGuard {
    /// Record `nonce` signed at `ts_ms`; false when already seen or when the
    /// memory is full of entries still inside the window.
    pub fn first_use(&mut self, nonce: &str, ts_ms: u64, now_ms: u64) -> bool {
        self.seen.retain(|_, t| t.saturating_add(REQUEST_WINDOW_MS) >= now_ms);
        if self.seen.contains_key(nonce) || self.seen.len() >= MAX_NONCES {
            return false;
        }
        self.seen.insert(nonce.to_owned(), ts_ms);
        true
    }
}

/// Verify `req` against the bound steward. Cheap checks first; the signature
/// last; only a request whose signature verified is charged a nonce.
pub fn verify_request(
    req: &LicenceRequest,
    steward_pubkey: &[u8; 32],
    steward_node: &str,
    now_ms: u64,
    replay: &mut ReplayGuard,
) -> Result<(), RequestRefused> {
    let auth = req.auth.as_ref().ok_or(RequestRefused::Unsigned)?;
    if req.body.len() > MAX_REQUEST_BODY {
        return Err(RequestRefused::TooLarge);
    }
    if auth.timestamp_ms > MAX_TS_MS || !valid_nonce(&auth.nonce) {
        return Err(RequestRefused::Malformed);
    }
    let sig = hex_decode_exact::<64>(&auth.signature).ok_or(RequestRefused::Malformed)?;
    if auth.node != steward_node {
        return Err(RequestRefused::WrongNode);
    }
    if auth.timestamp_ms.abs_diff(now_ms) > REQUEST_WINDOW_MS {
        return Err(RequestRefused::Stale);
    }
    let vk = VerifyingKey::from_bytes(steward_pubkey).map_err(|_| RequestRefused::BadSignature)?;
    let s = signing_string(&req.method, &req.path, &auth.node, auth.timestamp_ms, &auth.nonce, &req.body);
    vk.verify_strict(s.as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| RequestRefused::BadSignature)?;
    if !replay.first_use(&auth.nonce, auth.timestamp_ms, now_ms) {
        return Err(RequestRefused::Replay);
    }
    Ok(())
}
