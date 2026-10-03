//! Types of the licence exchange: errors, admission, config, wire messages
//! and the per-connection token buckets (ADR-106 phase 1b).

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use super::{AdmissionPosture, ApprovalStore, CheckoutGrantStore, LicenceError, LicenceEventSink, SignedApproval, SignedGrant};
use crate::mesh_delivery::PeerCtx;
use crate::mesh_runtime::MeshRuntime;
use crate::workload_pkg::TrustAnchors;

/// Records accepted per second on one connection and kind.
pub const RECORDS_PER_SEC: f64 = 2.0;
/// Burst of records accepted at once on one connection and kind.
pub const RECORD_BURST: f64 = 10.0;
/// Connections whose buckets are remembered at once.
pub(super) const MAX_TRACKED_CONNS: usize = 1024;
/// Applied records remembered for de-duplication.
pub(super) const MAX_SEEN: usize = 4096;

/// Why a record was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExchangeError {
    /// The store or a cheap check refused it.
    #[error(transparent)]
    Licence(#[from] LicenceError),
    /// Too many records arrived too fast on this connection.
    #[error("licence records are arriving too fast")]
    RateLimited,
}

/// What an accepted record did here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receipt {
    /// New state; the record is forwarded.
    New,
    /// Already held, stale or already seen; nothing to do.
    Known,
    /// Ahead of the local clock: not applied, not forwarded, retried at sync.
    Deferred,
}

/// Which peers may sync with this node.
pub trait PeerAdmission: Send + Sync + 'static {
    /// True when `ctx` is an admitted full node. Phase 1c replaces the
    /// source of this answer with the service-stamped delivery origin.
    fn admitted(&self, ctx: &PeerCtx) -> bool;
}

/// Admission as the mesh runtime reports it on the [`PeerCtx`]: a verified
/// full node (never a leaf, a legacy or an unauthenticated peer).
#[derive(Debug, Default, Clone, Copy)]
pub struct CtxAdmission;

impl PeerAdmission for CtxAdmission {
    fn admitted(&self, ctx: &PeerCtx) -> bool {
        ctx.node_verified && ctx.class == crate::mesh_admit::PeerClass::Node
    }
}

/// The node's admission state, read each time a binding arrives.
pub type PostureFn = Arc<dyn Fn() -> AdmissionPosture + Send + Sync>;

/// Timing knobs of the exchange.
#[derive(Debug, Clone)]
pub struct LicenceExchangeConfig {
    /// Period of the background catch-up sync (30 min).
    pub sync_interval: Duration,
    /// Least gap between two syncs answered for one peer (1 min).
    pub sync_min_gap: Duration,
    /// How long a peer that sent a bad signature is not synced with (10 min).
    pub sync_ban: Duration,
    /// Most pages followed in one sync session.
    pub max_pages: u32,
    /// Sync with a peer as soon as it connects.
    pub sync_on_connect: bool,
}

impl Default for LicenceExchangeConfig {
    fn default() -> Self {
        Self {
            sync_interval: Duration::from_secs(30 * 60),
            sync_min_gap: Duration::from_secs(60),
            sync_ban: Duration::from_secs(10 * 60),
            max_pages: 16,
            sync_on_connect: true,
        }
    }
}

/// Everything [`LicenceExchange::start`] needs.
pub struct LicenceExchangeParts {
    /// Binding and grant store.
    pub store: Arc<CheckoutGrantStore>,
    /// Approval store.
    pub approvals: Arc<ApprovalStore>,
    /// Pinned keys.
    pub anchors: Arc<TrustAnchors>,
    /// The mesh to flood on.
    pub runtime: Arc<MeshRuntime>,
    /// Admission posture for incoming bindings.
    pub posture: PostureFn,
    /// Who may sync.
    pub admission: Arc<dyn PeerAdmission>,
    /// Where `sync_bad_signature` goes (the stores have their own sink).
    pub sink: Arc<dyn LicenceEventSink>,
    /// Timing.
    pub config: LicenceExchangeConfig,
}

/// A record on `mesh.cog.grant`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum GrantMsg {
    /// A checkout grant.
    Grant(SignedGrant),
    /// An operator hash approval.
    Approval(SignedApproval),
}

/// Whose budget a record's signature verify is charged to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spend {
    /// Nobody's: the operator's own `issue_*`.
    Exempt,
    /// The flood bucket of this connection.
    Flood(u64),
    /// The sync bucket of this connection.
    Sync(u64),
}

/// Budget classes, each with its own bucket per connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Budget {
    Binding,
    Grant,
    Approval,
    Sync,
}

/// Per-connection token buckets, one per [`Budget`].
pub(super) struct Buckets {
    rate: f64,
    burst: f64,
    map: DashMap<(Budget, u64), (f64, Instant)>,
}

impl Buckets {
    pub(super) fn new(rate: f64, burst: f64) -> Self {
        Self { rate, burst, map: DashMap::new() }
    }

    /// Spend `cost` tokens from `conn`'s bucket for `kind`.
    pub(super) fn take(&self, kind: Budget, conn: u64, cost: f64) -> bool {
        let key = (kind, conn);
        if self.map.len() >= MAX_TRACKED_CONNS && !self.map.contains_key(&key) {
            let oldest = self.map.iter().min_by_key(|b| b.1).map(|b| *b.key());
            if let Some(k) = oldest {
                self.map.remove(&k);
            }
        }
        let now = Instant::now();
        let mut b = self.map.entry(key).or_insert((self.burst, now));
        let dt = now.duration_since(b.1).as_secs_f64();
        b.1 = now;
        b.0 = (b.0 + dt * self.rate).min(self.burst);
        if b.0 < cost {
            return false;
        }
        b.0 -= cost;
        true
    }
}

