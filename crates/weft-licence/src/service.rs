//! The HTTP-agnostic service: authentication, the two rate pools, routing and
//! the identity endpoint. Checkout, renewal, listing and transfer live in
//! `checkout.rs` and `artifact.rs`.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

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

fn binding_stamp(dir: &std::path::Path) -> Option<(SystemTime, u64)> {
    std::fs::metadata(dir.join("binding.json")).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
}

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
        /// The open cache file (opened under the state lock, so a later
        /// eviction cannot change what is sent).
        file: std::fs::File,
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
    /// The grant key; dropped from memory while unbound.
    pub key: Option<SigningKey>,
    /// mtime and length of `binding.json` when it was last read.
    pub binding_stamp: Option<(SystemTime, u64)>,
}

/// The licence proxy.
pub struct Service {
    pub(crate) cfg: Config,
    pub(crate) clock: Clock,
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
        if cfg.device_id.is_empty() {
            return Err(SvcError::Config("device_id is not set".into()));
        }
        let ops = OperatorKeys::load(&cfg.state_dir, &cfg.operator_pubkeys)?;
        let binding = bind::load(&cfg.state_dir, &ops)?;
        let key = match keys::load(&cfg.state_dir) {
            Ok(k) => Some(k),
            Err(SvcError::NoKey) if binding.as_ref().is_some_and(|b| !b.is_bound()) => None,
            Err(e) => return Err(e),
        };
        let store = Store::open(&cfg.state_dir)?;
        let binding_stamp = binding_stamp(&cfg.state_dir);
        let cache = Cache::open(&cfg.state_dir.join("cache"), cfg.limits.cache_bytes)
            .map_err(|e| SvcError::Io(format!("{e:?}")))?;
        let bound = binding.as_ref().is_some_and(|b| b.is_bound());
        let svc = Self {
            permits: Permits::new(cfg.limits.in_flight),
            cfg,
            clock,
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
                key: if bound { key } else { None },
                binding_stamp,
            }),
            on_release: Mutex::new(None),
        };
        if !bound {
            // An unbind that happened while the service was down.
            let mut inner = svc.lock();
            svc.release_all(&mut inner);
        }
        Ok(svc)
    }

    /// Re-read `binding.json` when it changed (mtime or length), so a bind or
    /// unbind applied by the CLI takes effect without a restart. Unbound,
    /// missing or corrupt means fail closed: the key leaves memory and every
    /// checkout is released.
    pub(crate) fn refresh_binding(&self, inner: &mut Inner) {
        let stamp = binding_stamp(&self.cfg.state_dir);
        if stamp == inner.binding_stamp {
            return;
        }
        inner.binding_stamp = stamp;
        let new = match bind::load(&self.cfg.state_dir, &self.ops) {
            Ok(b) => b,
            Err(e) => {
                self.log(&format!("binding.json refused, failing closed: {e}"));
                None
            }
        };
        let bound = new.as_ref().is_some_and(|b| b.is_bound());
        inner.binding = new;
        if bound {
            inner.key = keys::load(&self.cfg.state_dir).ok();
            if inner.key.is_none() {
                self.log("bound, but the grant key cannot be loaded");
            }
        } else {
            inner.key = None;
            self.release_all(inner);
        }
        self.log(&format!("binding reloaded, bound={bound}"));
    }

    /// Mark every checkout released (unbind): none is served or renewed again
    /// unless a new binding for its mesh re-creates it.
    pub(crate) fn release_all(&self, inner: &mut Inner) {
        let mut changed = false;
        for s in inner.store.slots.slots.values_mut() {
            if s.status == crate::state::SlotStatus::Active {
                s.status = crate::state::SlotStatus::Released;
                changed = true;
            }
        }
        if changed && let Err(e) = inner.store.persist_slots() {
            self.log(&format!("could not persist the release on unbind: {e}"));
        }
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
        self.refresh_binding(&mut inner);
        if let Err(e) = self.charge_unsigned(&mut inner, now) {
            return Response::err(e);
        }
        let pk = inner.key.as_ref().map(|k| k.verifying_key().to_bytes());
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
        self.refresh_binding(&mut inner);
        if !self.clock_ok(&inner, now) {
            return Err(self.refuse(&mut inner, now, ApiError::new(503, "clock_not_set", "the Seed clock is below its floor")));
        }
        let Some(b) = inner.binding.as_ref().filter(|b| b.is_bound()) else {
            return Err(self.refuse(&mut inner, now, ApiError::new(409, "seed_not_bound", "no mesh binding")));
        };
        let (pk, node) = (b.steward_key(), b.record.steward_node_id.clone());
        let (now_ms, window) = (now.saturating_mul(1000), self.cfg.request_window_secs.saturating_mul(1000));
        let verified = match request::verify(req, &pk, &node, &self.cfg.device_id, now_ms, window) {
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
        if !request::remember_nonce(&mut inner.store.nonces, &verified.nonce, verified.ts, now_ms, window) {
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
