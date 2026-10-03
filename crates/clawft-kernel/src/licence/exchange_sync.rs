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
pub(super) const PENDING_TTL: Duration = Duration::from_secs(120);

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
    pub(super) sent: Instant,
    pub(super) pages: u32,
    pub(super) grant_after: Option<GrantCursor>,
    pub(super) approval_after: Option<String>,
}

/// A sync this node is serving to a peer: opened by a fresh request, and the
/// only thing a continuation request is answered against.
pub(super) struct ServeSession {
    pub(super) opened: Instant,
    pub(super) pages: u32,
    pub(super) grant_last: Option<GrantCursor>,
    pub(super) approval_last: Option<String>,
}

/// A page and where it stopped.
pub(super) struct Page {
    pub(super) msg: SyncMsg,
    pub(super) last_grant: Option<GrantCursor>,
    pub(super) last_approval: Option<String>,
}

fn size(e: &SignedEnvelope) -> usize {
    super::store_sync::env_size(e)
}

/// True when `req` is not behind `last` (`None` is the start).
fn not_behind<T: Ord>(req: &Option<T>, last: &Option<T>) -> bool {
    match (req, last) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(r), Some(l)) => r >= l,
    }
}

impl LicenceExchange {
    /// The page of records after the given cursors. Grants get half the
    /// caps, approvals the rest after the binding and the grants.
    pub(super) fn build_page(
        &self,
        grant_after: &Option<GrantCursor>,
        approval_after: &Option<String>,
    ) -> Page {
        let binding = match grant_after {
            None => self.store.held_signed_binding().map(|(_, b)| b),
            Some(_) => None,
        };
        let used = binding.as_ref().map(size).unwrap_or(0);
        let after = grant_after.as_ref().map(|c| (c.seq, c.cog_id.as_str(), c.version.as_str()));
        let (g, more_grants) =
            self.store.sync_grants_page(after, SYNC_MAX_ENTRIES / 2, SYNC_MAX_BYTES / 2);
        let last_grant = g
            .last()
            .map(|(seq, cog, ver, _)| GrantCursor { seq: *seq, cog_id: cog.clone(), version: ver.clone() });
        let grants: Vec<SignedEnvelope> = g.into_iter().map(|(_, _, _, e)| e).collect();
        let g_bytes: usize = grants.iter().map(size).sum();
        let (a, more_approvals) = self.approvals.sync_approvals_page(
            approval_after.as_deref(),
            SYNC_MAX_ENTRIES - grants.len() - usize::from(binding.is_some()),
            SYNC_MAX_BYTES.saturating_sub(used + g_bytes),
        );
        let last_approval = a.last().map(|(k, _)| k.clone());
        let approvals = a.into_iter().map(|(_, e)| e).collect();
        Page {
            msg: SyncMsg::Response { binding, grants, approvals, more_grants, more_approvals },
            last_grant,
            last_approval,
        }
    }

    /// [`Self::build_page`]'s message.
    #[cfg(test)]
    pub(super) fn build_response(
        &self,
        grant_after: &Option<GrantCursor>,
        approval_after: &Option<String>,
    ) -> SyncMsg {
        self.build_page(grant_after, approval_after).msg
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
        let w = self.me.clone();
        handle.spawn(async move {
            loop {
                tokio::time::sleep(super::exchange_retry::RETRY_SCAN).await;
                let Some(me) = w.upgrade() else { break };
                me.retry_expired().await;
            }
        });
        if !self.config.sync_on_connect {
            return;
        }
        let Some(mut rx) = self.runtime.subscribe_peer_events() else { return };
        let w = self.me.clone();
        handle.spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if let MeshPeerEvent::Joined { node_id, .. } = ev {
                    let Some(me) = w.upgrade() else { break };
                    me.sync_peer(&node_id).await;
                }
            }
        });
    }

    /// Answer a request. A fresh one (no cursor) is answered once per peer
    /// per minute and opens a session. A continuation is answered only
    /// against that session: within [`PENDING_TTL`] of opening, under
    /// `max_pages`, with cursors not behind the last page served. Anything
    /// else gets no answer.
    pub(super) fn serve(&self, ctx: &PeerCtx, conn: u64, req: SyncMsg) -> Vec<serde_json::Value> {
        let SyncMsg::Request { grant_after, approval_after } = req else {
            return Vec::new();
        };
        if !self.sync_buckets.take(Budget::Sync, conn, 1.0) {
            return Vec::new();
        }
        let peer = &ctx.peer_id;
        let fresh = grant_after.is_none() && approval_after.is_none();
        if fresh {
            let now = Instant::now();
            if let Some(t) = self.served.get(peer)
                && now.duration_since(*t) < self.config.sync_min_gap
            {
                return Vec::new();
            }
            self.served.insert(peer.clone(), now);
        } else {
            let ok = self.sessions.get(peer).is_some_and(|s| {
                s.opened.elapsed() < PENDING_TTL
                    && s.pages < self.config.max_pages
                    && not_behind(&grant_after, &s.grant_last)
                    && not_behind(&approval_after, &s.approval_last)
            });
            if !ok {
                return Vec::new();
            }
        }
        let page = self.build_page(&grant_after, &approval_after);
        if fresh {
            self.sessions.insert(
                peer.clone(),
                ServeSession {
                    opened: Instant::now(),
                    pages: 1,
                    grant_last: page.last_grant,
                    approval_last: page.last_approval,
                },
            );
        } else if let Some(mut s) = self.sessions.get_mut(peer) {
            s.pages += 1;
            s.grant_last = page.last_grant.or_else(|| s.grant_last.take());
            s.approval_last = page.last_approval.or_else(|| s.approval_last.take());
        }
        serde_json::to_value(page.msg).map(|v| vec![v]).unwrap_or_default()
    }

    fn ban(&self, peer: &str) {
        self.banned.insert(peer.to_owned(), Instant::now());
        self.sink.emit(LicenceEvent::SyncBadSignature { peer: peer.to_owned() });
        tracing::warn!(peer, "sync response carried a bad signature; peer banned from sync");
    }

    /// What one verified-or-refused entry means for the rest of the response:
    /// `Some(true)` abort and ban (bad signature), `Some(false)` stop quietly
    /// (budget spent), `None` carry on (a deferred or refused entry does not
    /// stop the page). A new binding or restrictive record is forwarded.
    fn step(&self, peer: &str, r: Result<Receipt, ExchangeError>, fwd: Option<Forward>) -> Option<bool> {
        match r {
            Ok(Receipt::New) => {
                if let Some(f) = fwd {
                    self.forward(peer, f);
                }
            }
            Err(ExchangeError::Licence(LicenceError::BadSignature)) => return Some(true),
            Err(ExchangeError::RateLimited) => return Some(false),
            _ => {}
        }
        None
    }

    /// Apply one page. Returns the next request, if the peer has more.
    pub(super) fn absorb(&self, peer: &str, conn: u64, p: Pending, msg: SyncMsg) -> Vec<serde_json::Value> {
        let SyncMsg::Response { binding, grants, approvals, more_grants, more_approvals } = msg
        else {
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
            abort = self.step(peer, self.accept_binding(&b, spend), Some(Forward::Binding(b.clone())));
        }
        let mut last_grant = None;
        for g in &grants {
            if abort.is_some() || !within_caps(g) {
                break;
            }
            let parsed = serde_json::from_str::<CheckoutGrant>(&g.payload).ok();
            // Neighbours sync for themselves: only a withdrawal is passed on.
            let fwd = parsed
                .as_ref()
                .filter(|c| c.is_withdrawal())
                .map(|_| Forward::Grant(GrantMsg::Grant(g.clone())));
            abort = self.step(peer, self.accept_grant(g, spend), fwd);
            last_grant = parsed.map(|c| GrantCursor { seq: c.seq, cog_id: c.cog_id, version: c.version });
        }
        let mut last_approval = None;
        for a in &approvals {
            if abort.is_some() || !within_caps(a) {
                break;
            }
            let parsed = serde_json::from_str::<super::Approval>(&a.payload).ok();
            abort = self.step(peer, self.accept_approval(a, spend), None);
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
        match payload.get("op").and_then(|o| o.as_str()) {
            Some("request") => match SyncMsg::deserialize(payload) {
                Ok(req) => ex.serve(ctx, conn, req),
                Err(_) => {
                    tracing::warn!(peer = %ctx.peer_id, "malformed licence sync request");
                    Vec::new()
                }
            },
            Some("response") => {
                // Unsolicited or late: dropped before it is even parsed.
                let open = ex.pending.get(&ctx.peer_id).is_some_and(|p| p.sent.elapsed() < PENDING_TTL);
                if !open {
                    return Vec::new();
                }
                let Ok(msg) = SyncMsg::deserialize(payload) else {
                    tracing::warn!(peer = %ctx.peer_id, "malformed licence sync response");
                    return Vec::new();
                };
                if let Some((_, p)) = ex.pending.remove(&ctx.peer_id) {
                    ex.retries.remove(&ctx.peer_id);
                    self.apply(ctx.peer_id.clone(), conn, p, msg);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

impl SyncSink {
    /// Verify and apply a page off the dispatch path: every entry is a
    /// signature check and a fsync'd save. The follow-up request, if any, is
    /// sent when it is done.
    fn apply(&self, peer: String, conn: u64, p: Pending, msg: SyncMsg) {
        let ex = self.0.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            ex.absorb(&peer, conn, p, msg);
            return;
        };
        handle.spawn(async move {
            let (e2, peer2) = (ex.clone(), peer.clone());
            match tokio::task::spawn_blocking(move || e2.absorb(&peer2, conn, p, msg)).await {
                Ok(next) => {
                    for v in next {
                        ex.send_to(&peer, COG_SYNC_TOPIC, v).await;
                    }
                }
                Err(e) => tracing::warn!(peer, error = %e, "licence sync page failed"),
            }
        });
    }
}
