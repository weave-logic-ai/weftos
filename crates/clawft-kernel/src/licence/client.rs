//! The steward's client of `weft-licence` (ADR-106 sections 4 and 7).
//!
//! [`LicenceClient`] is what the checkout relay needs. [`SignedLicenceClient`]
//! implements it over a [`LicenceTransport`] (one signed request in, one
//! status and body out): the real transport is plain HTTP and arrives with
//! `weft-licence` in phase 2. Tests run it against a stub that implements the
//! same request and grant contract.

use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::request::{CLOCK_FLOOR_SECS, LicenceRequest, MAX_REQUEST_BODY, sign_request};
use super::{SignedGrant, valid_token};

/// Source of unix milliseconds. Injected so tests control the clock.
pub type ClockMs = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The wall clock in milliseconds.
pub fn system_clock_ms() -> ClockMs {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    })
}

/// Path of the checkout endpoint.
pub const CHECKOUT_PATH: &str = "/licence/v1/checkout";
/// Path prefix of the artifact endpoint (BLAKE3 hex follows).
pub const ARTIFACT_PATH: &str = "/licence/v1/artifact/";
/// Path of the renewal listing (`?since=<seq>`).
pub const GRANTS_PATH: &str = "/licence/v1/grants";
/// Largest response body the client reads, other than artifact bytes.
pub const MAX_RESPONSE_BODY: usize = 256 * 1024;

/// A request for a cog version, as sent to `weft-licence` and by members to
/// the steward. Every string is a short token; `version` may be `latest`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutWire {
    /// Correlates a reply with this request.
    pub request_id: String,
    /// Cog id.
    pub cog_id: String,
    /// Exact version, or `latest`.
    pub version: String,
    /// Target architecture.
    pub arch: String,
}

impl CheckoutWire {
    /// Boundary validation of a request from the network.
    pub fn validate(&self) -> Result<(), String> {
        for (what, v) in [
            ("request_id", &self.request_id),
            ("cog_id", &self.cog_id),
            ("version", &self.version),
            ("arch", &self.arch),
        ] {
            if !valid_token(v) {
                return Err(format!("{what} is not a valid token"));
            }
        }
        Ok(())
    }
}

/// What `weft-licence` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenceResponse {
    /// HTTP status.
    pub status: u16,
    /// Body bytes.
    pub body: Vec<u8>,
}

/// Why a licence call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LicenceClientError {
    /// The link to `weft-licence` failed.
    #[error("licence link: {0}")]
    Transport(String),
    /// `weft-licence` refused with a stable code (`cog_unlicensed`,
    /// `licence_expired`, `clock_not_set`, `seed_bound_elsewhere`, ...).
    #[error("licence refused ({status}): {code}")]
    Refused {
        /// HTTP status.
        status: u16,
        /// The stable error code.
        code: String,
    },
    /// The answer did not have the expected shape.
    #[error("bad licence response: {0}")]
    BadResponse(String),
}

/// One signed round trip to `weft-licence`.
#[async_trait]
pub trait LicenceTransport: Send + Sync + 'static {
    /// Send `req` and return the status and body.
    async fn call(&self, req: LicenceRequest) -> Result<LicenceResponse, LicenceClientError>;
}

/// What the checkout relay needs from `weft-licence`.
#[async_trait]
pub trait LicenceClient: Send + Sync + 'static {
    /// `POST /licence/v1/checkout`: the signed grant, with `seq` persisted
    /// by the Seed before release.
    async fn checkout(&self, req: &CheckoutWire) -> Result<SignedGrant, LicenceClientError>;
    /// `GET /licence/v1/artifact/<blake3>`: the cached binary. At most
    /// `max_len` bytes are accepted.
    async fn artifact(&self, blake3_hex: &str, max_len: u64) -> Result<Vec<u8>, LicenceClientError>;
    /// `GET /licence/v1/grants?since=<seq>`: renewal batch.
    async fn grants_since(&self, since: u64) -> Result<Vec<SignedGrant>, LicenceClientError>;
}

#[derive(Deserialize)]
struct GrantBody {
    grant: SignedGrant,
}

#[derive(Deserialize)]
struct GrantsBody {
    grants: Vec<SignedGrant>,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// [`LicenceClient`] that signs every request with the steward key.
pub struct SignedLicenceClient<T: LicenceTransport> {
    key: SigningKey,
    node: String,
    transport: T,
    clock: ClockMs,
}

impl<T: LicenceTransport> SignedLicenceClient<T> {
    /// A client for the steward `node` (its node id) holding `key`.
    pub fn new(key: SigningKey, node: impl Into<String>, transport: T, clock: ClockMs) -> Arc<Self> {
        Arc::new(Self { key, node: node.into(), transport, clock })
    }

    async fn signed(
        &self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        max_body: usize,
    ) -> Result<Vec<u8>, LicenceClientError> {
        if body.len() > MAX_REQUEST_BODY {
            return Err(LicenceClientError::BadResponse("request body too large".into()));
        }
        let ts_ms = (self.clock)();
        // A clock that was never set signs nothing (COG-011 floor).
        if ts_ms / 1000 < CLOCK_FLOOR_SECS {
            return Err(LicenceClientError::Refused { status: 0, code: "clock_not_set".into() });
        }
        let nonce = super::request_nonce();
        let req = sign_request(&self.key, &self.node, method, path, body, ts_ms, &nonce);
        let resp = self.transport.call(req).await?;
        if resp.status != 200 {
            let code = serde_json::from_slice::<ErrorBody>(&resp.body)
                .map(|e| e.error)
                .unwrap_or_else(|_| "unknown_error".into());
            return Err(LicenceClientError::Refused { status: resp.status, code });
        }
        if resp.body.len() > max_body {
            return Err(LicenceClientError::BadResponse("response body too large".into()));
        }
        Ok(resp.body)
    }
}

fn bad(e: serde_json::Error) -> LicenceClientError {
    LicenceClientError::BadResponse(e.to_string())
}

#[async_trait]
impl<T: LicenceTransport> LicenceClient for SignedLicenceClient<T> {
    async fn checkout(&self, req: &CheckoutWire) -> Result<SignedGrant, LicenceClientError> {
        let body = serde_json::to_vec(req).map_err(bad)?;
        let out = self.signed("POST", CHECKOUT_PATH, body, MAX_RESPONSE_BODY).await?;
        Ok(serde_json::from_slice::<GrantBody>(&out).map_err(bad)?.grant)
    }

    async fn artifact(&self, blake3_hex: &str, max_len: u64) -> Result<Vec<u8>, LicenceClientError> {
        if !super::valid_hex32(blake3_hex) {
            return Err(LicenceClientError::BadResponse("artifact id is not a blake3".into()));
        }
        let path = format!("{ARTIFACT_PATH}{blake3_hex}");
        self.signed("GET", &path, Vec::new(), usize::try_from(max_len).unwrap_or(usize::MAX)).await
    }

    async fn grants_since(&self, since: u64) -> Result<Vec<SignedGrant>, LicenceClientError> {
        let path = format!("{GRANTS_PATH}?since={since}");
        let out = self.signed("GET", &path, Vec::new(), MAX_RESPONSE_BODY).await?;
        Ok(serde_json::from_slice::<GrantsBody>(&out).map_err(bad)?.grants)
    }
}
