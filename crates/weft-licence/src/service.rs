//! The HTTP-agnostic service: authentication, the two rate pools, routing and
//! the identity endpoint. Checkout, renewal, listing and transfer live in
//! `checkout.rs` and `artifact.rs`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use ed25519_dalek::SigningKey;
use serde_json::json;
use weft_licence_wire::{SignedGrant, key_id};

use crate::bind::{self, BindingState};
use crate::cache::Cache;
use crate::config::Config;
use crate::error::{ApiError, SvcError};
use crate::keys;
use crate::limits::{Permit, Permits, RateWindow};
use crate::providers::{CogFetcher, DeviceSigner, LicenceProvider};
use crate::request::{self, AuthError, Request};
use crate::state::{OperatorKeys, Store};

/// Source of unix seconds. Injected so tests control the clock.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The wall clock.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// Called with each grant just before release (a test seam).
pub type ReleaseHook = Box<dyn Fn(&SignedGrant) + Send + Sync>;

/// What an answer carries.
pub enum Body {
    /// A JSON document.
    Json(Vec<u8>),
    /// A cached artifact to stream (rate limited by the HTTP layer).
    File {
        /// File to send.
        path: PathBuf,
        /// Its length.
        len: u64,
    },
}

/// An answer. The permit, when present, is held until the body is sent.
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// Body.
    pub body: Body,
    /// In-flight permit held while a transfer streams.
    pub permit: Option<Permit>,
}

impl Response {
    pub(crate) fn json(status: u16, v: serde_json::Value) -> Self {
        Self { status, body: Body::Json(serde_json::to_vec(&v).unwrap_or_default()), permit: None }
    }

    pub(crate) fn err(e: ApiError) -> Self {
        Self::json(e.status, json!({"error": e.code, "detail": e.detail}))
    }

    /// The JSON body, when it is one (tests).
    pub fn json_body(&self) -> Option<serde_json::Value> {
        match &self.body {
            Body::Json(b) => serde_json::from_slice(b).ok(),
            Body::File { .. } => None,
        }
    }
}

/// The mutable state behind one lock.
pub(crate) struct Inner {
    pub store: Store,
    pub binding: Option<BindingState>,
    pub cache: Cache,
    pub steward_rate: RateWindow,
    pub unsigned_rate: RateWindow,
}

/// The licence proxy.
pub struct Service {
    pub(crate) cfg: Config,
    pub(crate) clock: Clock,
    pub(crate) key: Option<SigningKey>,
    pub(crate) ops: OperatorKeys,
    pub(crate) licence: Box<dyn LicenceProvider>,
    pub(crate) fetcher: Box<dyn CogFetcher>,
    pub(crate) device: Box<dyn DeviceSigner>,
    pub(crate) permits: Permits,
    pub(crate) inner: Mutex<Inner>,
    pub(crate) on_release: Mutex<Option<ReleaseHook>>,
}

/// A verified steward.
pub(crate) struct Steward {
    pub key_id: String,
}

impl Service {
    /// Open the service over `cfg.state_dir`. The grant key is loaded with its
    /// permission check; a missing key is allowed only after an unbind.
    pub fn open(
        cfg: Config,
        clock: Clock,
        licence: Box<dyn LicenceProvider>,
        fetcher: Box<dyn CogFetcher>,
        device: Box<dyn DeviceSigner>,
    ) -> Result<Self, SvcError> {
        cfg.validate()?;
        let ops = OperatorKeys::load(&cfg.state_dir, &cfg.operator_pubkeys)?;
        let binding = bind::load(&cfg.state_dir, &ops)?;
        let key = match keys::load(&cfg.state_dir) {
            Ok(k) => Some(k),
            Err(SvcError::NoKey) if binding.as_ref().is_some_and(|b| !b.is_bound()) => None,
            Err(e) => return Err(e),
        };
        let store = Store::open(&cfg.state_dir)?;
        let cache = Cache::open(&cfg.state_dir.join("cache"), cfg.limits.cache_bytes)
            .map_err(|e| SvcError::Io(format!("{e:?}")))?;
        Ok(Self {
            permits: Permits::new(cfg.limits.in_flight),
            cfg,
            clock,
            key,
            ops,
            licence,
            fetcher,
            device,
            inner: Mutex::new(Inner {
                store,
                binding,
                cache,
                steward_rate: RateWindow::default(),
                unsigned_rate: RateWindow::default(),
            }),
            on_release: Mutex::new(None),
        })
    }

    /// One log line to stderr (journald under systemd). Fields are ids and
    /// counts, never keys, licence references or request bodies.
    pub(crate) fn log(&self, line: &str) {
        eprintln!("weft-licence: {line}");
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Test seam: called with every grant just before it is released, after
    /// its `seq` is on disk.
    pub fn set_release_hook(&self, f: ReleaseHook) {
        *self.on_release.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
    }

    /// Test seam: make state writes fail, as if the process died first.
    pub fn inject_persist_failure(&self, fail: bool) {
        self.lock().store.fail_persist = fail;
    }

    pub(crate) fn release(&self, grant: &SignedGrant) {
        if let Some(f) = self.on_release.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            f(grant);
        }
    }

    /// The configured limits.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn clock_ok(&self, inner: &Inner, now: u64) -> bool {
        now >= self.cfg.clock_floor && now >= inner.store.slots.last_issued_at
    }

    /// Handle one request.
    pub fn handle(&self, req: &Request) -> Response {
        let now = (self.clock)();
        if req.method == "GET" && req.path() == "/licence/v1/identity" {
            return self.identity(now);
        }
        let steward = match self.authenticate(req, now) {
            Ok(s) => s,
            Err(e) => return Response::err(e),
        };
        match (req.method.as_str(), req.path()) {
            ("POST", "/licence/v1/checkout") => self.checkout(req, now, &steward),
            ("POST", "/licence/v1/renew") => self.renew(req, now),
            ("GET", "/licence/v1/grants") => self.grants(req),
            ("GET", p) if p.starts_with("/licence/v1/artifact/") => self.artifact(req, now, &steward),
            _ => Response::err(ApiError::new(404, "not_found", "no such endpoint")),
        }
    }

    /// Charge the unsigned pool; `Err` when it is empty.
    fn charge_unsigned(&self, inner: &mut Inner, now: u64) -> Result<(), ApiError> {
        if inner.unsigned_rate.try_acquire(now, self.cfg.limits.unsigned_per_min) {
            Ok(())
        } else {
            Err(ApiError::new(429, "rate_limited_unsigned", "too many unsigned or refused requests"))
        }
    }

    /// Refuse an unauthenticated request: the refusal itself costs the
    /// unsigned pool, never the steward's budget.
    fn refuse(&self, inner: &mut Inner, now: u64, e: ApiError) -> ApiError {
        match self.charge_unsigned(inner, now) {
            Ok(()) => {
                self.log(&format!("refused {} ({})", e.code, e.status));
                e
            }
            Err(pool) => pool,
        }
    }

    fn identity(&self, now: u64) -> Response {
        let mut inner = self.lock();
        if let Err(e) = self.charge_unsigned(&mut inner, now) {
            return Response::err(e);
        }
        let pk = self.key.as_ref().map(|k| k.verifying_key().to_bytes());
        let bound = inner.binding.as_ref().filter(|b| b.is_bound());
        Response::json(
            200,
            json!({
                "service": "weft-licence",
                "v": 1,
                "device_id": self.cfg.device_id,
                "grant_key_id": pk.as_ref().map(key_id),
                "grant_pubkey": pk.as_ref().map(|p| weft_licence_wire::hex_encode(p)),
                "bound": bound.is_some(),
                "mesh_id": bound.map(|b| b.record.mesh_id.clone()),
                "clock_ok": self.clock_ok(&inner, now),
                "device_signing": self.device.label(),
            }),
        )
    }

    /// Verify the steward signature, then charge the steward's budget. Order
    /// matters: forged traffic is charged to the unsigned pool only.
    fn authenticate(&self, req: &Request, now: u64) -> Result<Steward, ApiError> {
        let mut inner = self.lock();
        if !self.clock_ok(&inner, now) {
            return Err(self.refuse(&mut inner, now, ApiError::new(503, "clock_not_set", "the Seed clock is below its floor")));
        }
        let Some(b) = inner.binding.as_ref().filter(|b| b.is_bound()) else {
            return Err(self.refuse(&mut inner, now, ApiError::new(409, "seed_not_bound", "no mesh binding")));
        };
        let (pk, node) = (b.steward_key(), b.record.steward_node_id.clone());
        let window = self.cfg.request_window_secs;
        let verified = match request::verify(req, &pk, &node, now, window) {
            Ok(v) => v,
            Err(e) => {
                let (status, code) = match e {
                    AuthError::Malformed => (400, "malformed_auth"),
                    AuthError::WrongNode => (401, "wrong_node"),
                    AuthError::Stale => (401, "stale_request"),
                    AuthError::BadSignature => (401, "bad_signature"),
                };
                return Err(self.refuse(&mut inner, now, ApiError::new(status, code, "request signature refused")));
            }
        };
        if !request::remember_nonce(&mut inner.store.nonces, &verified.nonce, verified.ts, now, window) {
            return Err(self.refuse(&mut inner, now, ApiError::new(401, "replayed", "nonce already used")));
        }
        if !inner.steward_rate.try_acquire(now, self.cfg.limits.requests_per_min) {
            return Err(ApiError::new(429, "rate_limited", "steward request budget used up"));
        }
        if inner.store.persist_nonces().is_err() {
            return Err(ApiError::new(503, "persist_failed", "could not record the nonce"));
        }
        Ok(Steward { key_id: key_id(&pk) })
    }
}
