//! Catch-up sync of the licence records (ADR-106 section 5.5, phase 1b).
//!
//! A node asks a peer, on each connect and every 30 minutes, for the
//! binding, the highest-`seq` grant per (cog, version) and the approvals it
//! holds. Floods alone leave late joiners, healed partitions and deferred
//! grants behind; this brings them to the same state.
//!
//! Limits, all enforced on both ends:
//!
//! - **Answered rarely and only to admitted peers.** One fresh sync per peer
//!   per minute; a continuation page (a request carrying a cursor) is bounded
//!   by the connection's sync bucket instead.
//! - **Capped.** At most [`SYNC_MAX_ENTRIES`] entries and [`SYNC_MAX_BYTES`]
//!   per response; the rest is paged per record kind, each with its own
//!   cursor (grants by `seq`, approvals by content key), so a large set of
//!   one kind cannot hide the other.
//! - **Cheap filters before any signature work**, then one sync-bucket token
//!   per entry verified. A response nobody asked for is dropped unread.
//! - **Strict on a bad signature.** The first one aborts the response, bans
//!   the peer from sync for 10 minutes and reports `sync_bad_signature`.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::exchange::{
    ExchangeError, Forward, GrantMsg, LicenceExchange, Receipt, Spend,
};
use super::exchange_types::Budget;
use super::{CheckoutGrant, LicenceError, LicenceEvent, SignedEnvelope};
use crate::mesh_delivery::PeerCtx;
use crate::mesh_discovery::MeshPeerEvent;
use crate::mesh_runtime::{COG_SYNC_TOPIC, PeerControlSink};

/// Most entries in one sync response.
pub const SYNC_MAX_ENTRIES: usize = 256;
/// Most payload bytes in one sync response.
pub const SYNC_MAX_BYTES: usize = 256 * 1024;
/// Sync tokens refilled per second, per connection.
pub(super) const SYNC_ENTRIES_PER_SEC: f64 = 8.0;
/// Sync tokens a connection may spend at once.
pub(super) const SYNC_BURST: f64 = 600.0;
/// How long a request stays open for its response.
const PENDING_TTL: Duration = Duration::from_secs(120);

/// Where the grant page stopped: grants are ordered by `(seq, cog, version)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GrantCursor {
    /// `seq` of the last grant received.
    pub seq: u64,
    /// Its cog id.
    pub cog_id: String,
    /// Its version.
    pub version: String,
}

/// A message on `mesh.cog.sync`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SyncMsg {
    /// Ask for the records after these cursors (none: from the start).
    Request {
        /// Grants strictly after this cursor.
        grant_after: Option<GrantCursor>,
        /// Approvals with a content key strictly after this.
        approval_after: Option<String>,
    },
    /// One page of records.
    Response {
        /// The binding (first page only).
        binding: Option<SignedEnvelope>,
        /// Grants, ordered by `(seq, cog, version)`.
        grants: Vec<SignedEnvelope>,
        /// Approvals, ordered by content key.
        approvals: Vec<SignedEnvelope>,
        /// More grants follow this page.
        more_grants: bool,
        /// More approvals follow this page.
        more_approvals: bool,
    },
}

/// A request in flight.
pub(super) struct Pending {
    sent: Instant,
    pages: u32,
    grant_after: Option<GrantCursor>,
    approval_after: Option<String>,
}

fn size(e: &SignedEnvelope) -> usize {
    e.payload.len() + e.public_key.len() + e.signature.len()
}

/// Take entries while both caps hold. Returns the entries and whether any
/// were left over.
fn take_page<T>(
    items: impl Iterator<Item = (SignedEnvelope, T)>,
    max_entries: usize,
    max_bytes: usize,
) -> (Vec<SignedEnvelope>, usize, bool) {
    let (mut out, mut bytes, mut more) = (Vec::new(), 0usize, false);
    for (env, _) in items {
        if out.len() >= max_entries || bytes + size(&env) > max_bytes {
            more = true;
            break;
        }
        bytes += size(&env);
        out.push(env);
    }
    (out, bytes, more)
}

impl LicenceExchange {
    /// The page of records after the given cursors. Grants get half the
    /// caps, approvals the rest after the binding and the grants.
    pub(super) fn build_response(
        &self,
        grant_after: &Option<GrantCursor>,
        approval_after: &Option<String>,
    ) -> SyncMsg {
        let binding = match grant_after {
            None => self.store.held_binding().map(|(_, b)| b),
            Some(_) => None,
        };
        let used = binding.as_ref().map(size).unwrap_or(0);
        let grants = self.store.sync_grants().into_iter().filter(|(seq, cog, ver, _)| {
            grant_after.as_ref().is_none_or(|c| (*seq, cog, ver) > (c.seq, &c.cog_id, &c.version))
        });
        let (grants, g_bytes, more_grants) = take_page(
            grants.map(|(_, _, _, g)| (g, ())),
            SYNC_MAX_ENTRIES / 2,
            SYNC_MAX_BYTES / 2,
        );
        let approvals = self
            .approvals
            .sync_approvals()
            .into_iter()
            .filter(|(k, _)| approval_after.as_ref().is_none_or(|c| k > c));
        let (approvals, _, more_approvals) = take_page(
            approvals.map(|(_, a)| (a, ())),
            SYNC_MAX_ENTRIES - grants.len() - usize::from(binding.is_some()),
            SYNC_MAX_BYTES.saturating_sub(used + g_bytes),
        );
        SyncMsg::Response { binding, grants, approvals, more_grants, more_approvals }
    }

    fn is_banned(&self, peer: &str) -> bool {
        match self.banned.get(peer).map(|t| t.elapsed()) {
            Some(age) if age < self.config.sync_ban => true,
            Some(_) => {
                self.banned.remove(peer);
                false
            }
            None => false,
        }
    }

    /// Ask `peer` for its licence records. Does nothing while the local mesh
    /// id is unset, the peer is banned, or a request to it is still open.
    pub async fn sync_peer(&self, peer: &str) {
        if self.local().is_err() || self.is_banned(peer) {
            return;
        }
        if self.pending.get(peer).is_some_and(|p| p.sent.elapsed() < PENDING_TTL) {
            return;
        }
        self.pending.insert(
            peer.to_owned(),
            Pending { sent: Instant::now(), pages: 0, grant_after: None, approval_after: None },
        );
        let req = SyncMsg::Request { grant_after: None, approval_after: None };
        if let Ok(v) = serde_json::to_value(&req) {
            self.send_to(peer, COG_SYNC_TOPIC, v).await;
        }
    }

    /// [`Self::sync_peer`] with every connected peer.
    pub async fn sync_all(&self) {
        for peer in self.runtime.peer_ids() {
            self.sync_peer(&peer).await;
        }
    }

    /// The 30-minute sync and the sync on each peer connect.
    pub(super) fn spawn_sync_tasks(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (w, every) = (self.me.clone(), self.config.sync_interval);
        handle.spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(me) = w.upgrade() else { break };
                me.sync_all().await;
            }
        });
        if !self.config.sync_on_connect {
            return;
        }
        let (w, mut rx) = (self.me.clone(), self.runtime.subscribe_peer_events());
        handle.spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if let MeshPeerEvent::Joined { node_id, .. } = ev {
                    let Some(me) = w.upgrade() else { break };
                    me.sync_peer(&node_id).await;
                }
            }
        });
    }

    fn serve(&self, ctx: &PeerCtx, conn: u64, req: SyncMsg) -> Vec<serde_json::Value> {
        let SyncMsg::Request { grant_after, approval_after } = req else {
            return Vec::new();
        };
        if !self.sync_buckets.take(Budget::Sync, conn, 1.0) {
            return Vec::new();
        }
        if grant_after.is_none() && approval_after.is_none() {
            let now = Instant::now();
            if let Some(t) = self.served.get(&ctx.peer_id)
                && now.duration_since(*t) < self.config.sync_min_gap
            {
                return Vec::new();
            }
            self.served.insert(ctx.peer_id.clone(), now);
        }
        match serde_json::to_value(self.build_response(&grant_after, &approval_after)) {
            Ok(v) => vec![v],
            Err(_) => Vec::new(),
        }
    }

    fn ban(&self, peer: &str) {
        self.banned.insert(peer.to_owned(), Instant::now());
        self.sink.emit(LicenceEvent::SyncBadSignature { peer: peer.to_owned() });
        tracing::warn!(peer, "sync response carried a bad signature; peer banned from sync");
    }

    /// What one verified-or-refused entry means for the rest of the response:
    /// `Some(true)` abort and ban (bad signature), `Some(false)` stop quietly
    /// (budget spent), `None` carry on. A new record is forwarded.
    fn step(&self, peer: &str, r: Result<Receipt, ExchangeError>, fwd: Forward) -> Option<bool> {
        match r {
            Ok(Receipt::New) => self.forward(peer, fwd),
            Err(ExchangeError::Licence(LicenceError::BadSignature)) => return Some(true),
            Err(ExchangeError::RateLimited) => return Some(false),
            _ => {}
        }
        None
    }

    /// Apply one page. Returns the next request, if the peer has more.
    fn absorb(&self, ctx: &PeerCtx, conn: u64, msg: SyncMsg) -> Vec<serde_json::Value> {
        let SyncMsg::Response { binding, grants, approvals, more_grants, more_approvals } = msg
        else {
            return Vec::new();
        };
        let peer = ctx.peer_id.as_str();
        let Some((_, p)) = self.pending.remove(peer) else {
            return Vec::new();
        };
        if p.sent.elapsed() >= PENDING_TTL || !self.sync_buckets.take(Budget::Sync, conn, 1.0) {
            return Vec::new();
        }
        let (mut entries, mut bytes) = (0usize, 0usize);
        let mut within_caps = |e: &SignedEnvelope| {
            entries += 1;
            bytes += size(e);
            entries <= SYNC_MAX_ENTRIES && bytes <= SYNC_MAX_BYTES
        };
        let spend = Spend::Sync(conn);
        let mut abort = None;
        if let Some(b) = binding
            && within_caps(&b)
        {
            abort = self.step(peer, self.accept_binding(&b, spend), Forward::Binding(b));
        }
        let mut last_grant = None;
        for g in &grants {
            if abort.is_some() || !within_caps(g) {
                break;
            }
            let parsed = serde_json::from_str::<CheckoutGrant>(&g.payload).ok();
            abort = self.step(peer, self.accept_grant(g, spend), Forward::Grant(GrantMsg::Grant(g.clone())));
            last_grant = parsed.map(|c| GrantCursor { seq: c.seq, cog_id: c.cog_id, version: c.version });
        }
        let mut last_approval = None;
        for a in &approvals {
            if abort.is_some() || !within_caps(a) {
                break;
            }
            let parsed = serde_json::from_str::<super::Approval>(&a.payload).ok();
            abort = self.step(peer, self.accept_approval(a, spend), Forward::Grant(GrantMsg::Approval(a.clone())));
            last_approval = parsed.map(|c| c.content_key());
        }
        match abort {
            Some(true) => self.ban(peer),
            Some(false) => {}
            None => return self.next_page(peer, p, more_grants, more_approvals, last_grant, last_approval),
        }
        Vec::new()
    }

    /// The follow-up request for a response with more behind it, when its
    /// cursors advanced and the page limit is not reached.
    fn next_page(
        &self,
        peer: &str,
        p: Pending,
        more_grants: bool,
        more_approvals: bool,
        last_grant: Option<GrantCursor>,
        last_approval: Option<String>,
    ) -> Vec<serde_json::Value> {
        if !(more_grants || more_approvals) || p.pages + 1 >= self.config.max_pages {
            return Vec::new();
        }
        let grant_after = last_grant.or_else(|| p.grant_after.clone());
        let approval_after = last_approval.or_else(|| p.approval_after.clone());
        if grant_after == p.grant_after && approval_after == p.approval_after {
            return Vec::new(); // no progress: do not loop on a lying peer
        }
        self.pending.insert(
            peer.to_owned(),
            Pending {
                sent: Instant::now(),
                pages: p.pages + 1,
                grant_after: grant_after.clone(),
                approval_after: approval_after.clone(),
            },
        );
        serde_json::to_value(SyncMsg::Request { grant_after, approval_after })
            .map(|v| vec![v])
            .unwrap_or_default()
    }
}

/// The `mesh.cog.sync` control sink.
pub(super) struct SyncSink(pub(super) std::sync::Arc<LicenceExchange>);

impl PeerControlSink for SyncSink {
    fn on_peer_control(&self, ctx: &PeerCtx, conn: u64, payload: &serde_json::Value) -> Vec<serde_json::Value> {
        let ex = &self.0;
        if !ex.admission.admitted(ctx) || ex.local().is_err() || ex.is_banned(&ctx.peer_id) {
            return Vec::new();
        }
        let Ok(msg) = serde_json::from_value::<SyncMsg>(payload.clone()) else {
            tracing::warn!(peer = %ctx.peer_id, "malformed licence sync message");
            return Vec::new();
        };
        match msg {
            SyncMsg::Request { .. } => ex.serve(ctx, conn, msg),
            SyncMsg::Response { .. } => ex.absorb(ctx, conn, msg),
        }
    }
}
