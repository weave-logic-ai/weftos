//! Steward to `weft-licence` request signing (ADR-106 sections 4 and 7).
//!
//! Every endpoint of `weft-licence` except `GET /licence/v1/identity` needs a
//! valid steward signature, `GET /licence/v1/grants` included. The signed
//! string uses the COG-011 field layout under its own domain tag:
//!
//! ```text
//! weft-licence-v1/request \n METHOD \n path \n node \n timestamp \n nonce \n sha256(body)
//! ```
//!
//! `path` includes the query string. The transport is plain HTTP in
//! production (phase 2); this module owns only the bytes that are signed, so
//! the stub proxy in the tests and the real service share one definition.

use std::collections::HashSet;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use super::sha256_hex;
use crate::workload_pkg::codec::{hex_decode_exact, hex_encode};

/// Domain tag of steward to licence requests.
pub const REQUEST_DOMAIN: &str = "weft-licence-v1/request";
/// A request timestamp may differ from the verifier's clock by this much.
pub const REQUEST_WINDOW_SECS: u64 = 300;
/// Largest request body the signer will produce or a verifier accept.
pub const MAX_REQUEST_BODY: usize = 16 * 1024;
/// Replay memory of one verifier: nonces newer than the window, at most this many.
const MAX_NONCES: usize = 4096;

/// The signature headers of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAuth {
    /// The steward's node id.
    pub node: String,
    /// The steward's Ed25519 public key, 64 hex chars.
    pub public_key: String,
    /// Signing time, unix seconds.
    pub timestamp: u64,
    /// 32 hex chars, unique per request.
    pub nonce: String,
    /// Ed25519 signature over [`signing_bytes`], 128 hex chars.
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
pub fn signing_bytes(
    method: &str,
    path: &str,
    node: &str,
    timestamp: u64,
    nonce: &str,
    body: &[u8],
) -> Vec<u8> {
    format!(
        "{REQUEST_DOMAIN}\n{method}\n{path}\n{node}\n{timestamp}\n{nonce}\n{}",
        sha256_hex(body)
    )
    .into_bytes()
}

/// Sign a request with the steward key.
pub fn sign_request(
    key: &SigningKey,
    node: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
    timestamp: u64,
    nonce: [u8; 16],
) -> LicenceRequest {
    let nonce = hex_encode(&nonce);
    let sig = key.sign(&signing_bytes(method, path, node, timestamp, &nonce, &body));
    LicenceRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        auth: Some(RequestAuth {
            node: node.to_owned(),
            public_key: hex_encode(&key.verifying_key().to_bytes()),
            timestamp,
            nonce,
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
    /// Signed by a key other than the bound steward key.
    #[error("request is signed by a key that is not the bound steward")]
    WrongKey,
    /// Timestamp outside [`REQUEST_WINDOW_SECS`].
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

/// Verifier-side replay memory: remembers recent nonces.
#[derive(Debug, Default)]
pub struct ReplayGuard {
    seen: HashSet<String>,
}

impl ReplayGuard {
    /// Record `nonce`; false when it was already seen. When the memory is
    /// full it is cleared: the timestamp window still bounds any replay.
    pub fn first_use(&mut self, nonce: &str) -> bool {
        if self.seen.len() >= MAX_NONCES {
            self.seen.clear();
        }
        self.seen.insert(nonce.to_owned())
    }
}

/// Verify `req` against the bound `steward_pubkey`. The cheap checks (header
/// present, size, key, window) come first; the signature is checked last, and
/// only a request whose signature verified is charged a nonce.
pub fn verify_request(
    req: &LicenceRequest,
    steward_pubkey: &[u8; 32],
    now: u64,
    replay: &mut ReplayGuard,
) -> Result<(), RequestRefused> {
    let auth = req.auth.as_ref().ok_or(RequestRefused::Unsigned)?;
    if req.body.len() > MAX_REQUEST_BODY {
        return Err(RequestRefused::TooLarge);
    }
    let pk = hex_decode_exact::<32>(&auth.public_key).ok_or(RequestRefused::Malformed)?;
    if &pk != steward_pubkey {
        return Err(RequestRefused::WrongKey);
    }
    if auth.timestamp.abs_diff(now) > REQUEST_WINDOW_SECS {
        return Err(RequestRefused::Stale);
    }
    if hex_decode_exact::<16>(&auth.nonce).is_none() {
        return Err(RequestRefused::Malformed);
    }
    let sig = hex_decode_exact::<64>(&auth.signature).ok_or(RequestRefused::Malformed)?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|_| RequestRefused::BadSignature)?;
    let bytes =
        signing_bytes(&req.method, &req.path, &auth.node, auth.timestamp, &auth.nonce, &req.body);
    vk.verify_strict(&bytes, &Signature::from_bytes(&sig))
        .map_err(|_| RequestRefused::BadSignature)?;
    if !replay.first_use(&auth.nonce) {
        return Err(RequestRefused::Replay);
    }
    Ok(())
}
