//! Steward to `weft-licence` request signing (ADR-106 sections 4 and 7).
//!
//! The signed string, its constants and the field rules are defined once, in
//! `weft-licence-wire::request`, which `weft-licence` (the Seed service) uses
//! too: domain `weft-licence-v1/request`, then method, target, node,
//! `seed_device_id` (the audience, from the binding's `device_id`), a
//! millisecond timestamp, a 16 to 64 alphanumeric nonce and the body sha256,
//! one per line. This module is the kernel's request shape ([`LicenceRequest`]),
//! its signing and verification over those shared pieces, and the verifier's
//! replay memory. The tests verify what the client signs under
//! `weft-licence`'s own verifier.

use std::collections::HashMap;

use ed25519_dalek::SigningKey;

use crate::workload_pkg::codec::hex_decode_exact;

pub use weft_licence_wire::request::{
    CLOCK_FLOOR_SECS, REQUEST_DOMAIN, REQUEST_WINDOW_MS, signing_string, valid_nonce,
};

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

/// Sign a request with the steward key. `nonce` must satisfy [`valid_nonce`].
#[allow(clippy::too_many_arguments)]
pub fn sign_request(
    key: &SigningKey,
    node: &str,
    audience: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
    ts_ms: u64,
    nonce: &str,
) -> LicenceRequest {
    let sig = weft_licence_wire::request::sign(key, node, audience, method, path, &body, ts_ms, nonce);
    LicenceRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        auth: Some(RequestAuth {
            node: node.to_owned(),
            timestamp_ms: ts_ms,
            nonce: nonce.to_owned(),
            signature: sig,
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
    audience: &str,
    now_ms: u64,
    replay: &mut ReplayGuard,
) -> Result<(), RequestRefused> {
    let auth = req.auth.as_ref().ok_or(RequestRefused::Unsigned)?;
    if req.body.len() > MAX_REQUEST_BODY {
        return Err(RequestRefused::TooLarge);
    }
    if !weft_licence_wire::request::valid_ts(auth.timestamp_ms) || !valid_nonce(&auth.nonce) {
        return Err(RequestRefused::Malformed);
    }
    if hex_decode_exact::<64>(&auth.signature).is_none() {
        return Err(RequestRefused::Malformed);
    }
    if auth.node != steward_node {
        return Err(RequestRefused::WrongNode);
    }
    if !weft_licence_wire::request::within_window(auth.timestamp_ms, now_ms, REQUEST_WINDOW_MS) {
        return Err(RequestRefused::Stale);
    }
    if !weft_licence_wire::request::verify_signature(
        steward_pubkey,
        &auth.node,
        audience,
        &req.method,
        &req.path,
        &req.body,
        auth.timestamp_ms,
        &auth.nonce,
        &auth.signature,
    ) {
        return Err(RequestRefused::BadSignature);
    }
    if !replay.first_use(&auth.nonce, auth.timestamp_ms, now_ms) {
        return Err(RequestRefused::Replay);
    }
    Ok(())
}
