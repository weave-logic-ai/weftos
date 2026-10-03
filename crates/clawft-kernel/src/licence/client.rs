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
/// Path of the renewal call (signs a new grant for every active checkout).
pub const RENEW_PATH: &str = "/licence/v1/renew";

/// One page of `GET /licence/v1/grants?since=<ctr>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GrantsPage {
    /// The latest grant per checkout issued after the cursor.
    pub grants: Vec<SignedGrant>,
    /// The cursor for the next page (the Seed's issue counter).
    pub next: u64,
    /// More pages follow.
    pub more: bool,
}
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
    /// `POST /licence/v1/renew`: a new grant (or a withdrawal) for every
    /// active checkout, at most one batch. `next` is the Seed's issue counter
    /// after the renewal (the bound for the catch-up cursor).
    async fn renew(&self) -> Result<GrantsPage, LicenceClientError> {
        Err(LicenceClientError::BadResponse("renew is not supported by this client".into()))
    }
    /// [`Self::grants_since`] with the paging cursor.
    async fn grants_page(&self, since: u64) -> Result<GrantsPage, LicenceClientError> {
        let grants = self.grants_since(since).await?;
        Ok(GrantsPage { grants, next: since, more: false })
    }
    /// `POST /licence/v1/renew` with `{"release": [{cog_id, version}]}`: the
    /// Seed withdraws that checkout (a renewal whose `expires_at <=
    /// issued_at`) and, as for any renewal, renews every other active one.
    async fn release(&self, _cog_id: &str, _version: &str) -> Result<GrantsPage, LicenceClientError> {
        Err(LicenceClientError::BadResponse("release is not supported by this client".into()))
    }
}

/// The body of a release (ADR-106 renewal endpoint; weft-licence `RenewReq`).
#[derive(Serialize)]
struct ReleaseBody<'a> {
    release: [ReleaseRef<'a>; 1],
}

#[derive(Serialize)]
struct ReleaseRef<'a> {
    cog_id: &'a str,
    version: &'a str,
}

#[derive(Deserialize)]
struct GrantBody {
    grant: SignedGrant,
}

#[derive(Deserialize)]
struct GrantsBody {
    grants: Vec<SignedGrant>,
    #[serde(default)]
    next: u64,
    #[serde(default)]
    more: bool,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// [`LicenceClient`] that signs every request with the steward key.
pub struct SignedLicenceClient<T: LicenceTransport> {
    key: SigningKey,
    node: String,
    /// The bound Seed's device id: the audience every request is signed for.
    seed_device_id: String,
    transport: T,
    clock: ClockMs,
}

impl<T: LicenceTransport> SignedLicenceClient<T> {
    /// A client for the steward `node` (its node id) holding `key`, talking
    /// to the Seed `seed_device_id` (from the binding).
    pub fn new(
        key: SigningKey,
        node: impl Into<String>,
        seed_device_id: impl Into<String>,
        transport: T,
        clock: ClockMs,
    ) -> Arc<Self> {
        Arc::new(Self { key, node: node.into(), seed_device_id: seed_device_id.into(), transport, clock })
    }

    /// A client for the bound Seed: the audience is the binding's `device_id`
    /// and the node is its `steward_node_id`.
    pub fn for_binding(
        key: SigningKey,
        binding: &super::BindingRecord,
        transport: T,
        clock: ClockMs,
    ) -> Arc<Self> {
        Self::new(key, binding.steward_node_id.clone(), binding.device_id.clone(), transport, clock)
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
        let nonce = request_nonce();
        let req = sign_request(&self.key, &self.node, &self.seed_device_id, method, path, body, ts_ms, &nonce);
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
        Ok(self.grants_page(since).await?.grants)
    }

    async fn grants_page(&self, since: u64) -> Result<GrantsPage, LicenceClientError> {
        let path = format!("{GRANTS_PATH}?since={since}");
        let out = self.signed("GET", &path, Vec::new(), MAX_RESPONSE_BODY).await?;
        let b = serde_json::from_slice::<GrantsBody>(&out).map_err(bad)?;
        Ok(GrantsPage { grants: b.grants, next: b.next, more: b.more })
    }

    async fn renew(&self) -> Result<GrantsPage, LicenceClientError> {
        let out = self.signed("POST", RENEW_PATH, Vec::new(), MAX_RESPONSE_BODY).await?;
        let b = serde_json::from_slice::<GrantsBody>(&out).map_err(bad)?;
        Ok(GrantsPage { grants: b.grants, next: b.next, more: false })
    }

    async fn release(&self, cog_id: &str, version: &str) -> Result<GrantsPage, LicenceClientError> {
        if !valid_token(cog_id) || !valid_token(version) || version == "latest" {
            return Err(LicenceClientError::BadResponse("release needs a cog id and an exact version".into()));
        }
        let body = serde_json::to_vec(&ReleaseBody { release: [ReleaseRef { cog_id, version }] }).map_err(bad)?;
        let out = self.signed("POST", RENEW_PATH, body, MAX_RESPONSE_BODY).await?;
        let b = serde_json::from_slice::<GrantsBody>(&out).map_err(bad)?;
        Ok(GrantsPage { grants: b.grants, next: b.next, more: false })
    }
}

/// A fresh request nonce: 32 lower-case hex chars (inside the server's 16 to 64 alphanumerics).
#[cfg(test)]
pub(super) fn request_nonce_for_tests() -> String {
    request_nonce()
}

fn request_nonce() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    crate::workload_pkg::codec::hex_encode(&b)
}
