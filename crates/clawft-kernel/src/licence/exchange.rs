//! Floods of the licence records across the mesh (ADR-106 phase 1b).
//!
//! [`LicenceExchange`] carries the Seed binding on `mesh.cog.binding` and
//! checkout grants plus operator approvals on `mesh.cog.grant`, with the same
//! discipline as [`crate::mesh_swarm_revoke::RevocationExchange`]:
//!
//! - **Cheap first, and free.** Size, a signer that is a pinned operator key
//!   (binding, approval) or the bound grant key (grant), the record naming
//!   this node's mesh, and a `seq` (or content key) the store does not
//!   already hold.
//! - **Budgeted before the verify.** A record that passes spends a token
//!   from its own connection's bucket for that record kind, so junk on one
//!   connection starves neither another connection, another kind, nor the
//!   revocation floods (those live in a different exchange with its own
//!   buckets). The operator's own `issue_*` is exempt.
//! - **Verified by the stores.** [`CheckoutGrantStore`] and
//!   [`ApprovalStore`] do the signature and rule checks; this module adds
//!   none of its own.
//! - **Forwarded once.** A record that was new here goes to every other
//!   peer; the seen-set is written only after a record verified.
//! - **Deferred, not refused.** A grant ahead of the local clock
//!   (`NotYetValid`) is neither forwarded nor remembered: catch-up sync
//!   brings it again later.
//!
//! The exchange is inert while the local mesh id is unset.

use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use dashmap::DashMap;
use ed25519_dalek::SigningKey;

use super::binding::require_operator;
pub use super::exchange_types::{
    CtxAdmission, ExchangeError, GrantMsg, LicenceExchangeConfig, LicenceExchangeParts,
    PeerAdmission, PostureFn, Receipt, Spend,
};
use super::exchange_types::{Buckets, MAX_SEEN, RECORDS_PER_SEC, RECORD_BURST};
pub(super) use super::exchange_types::Budget;
use super::{
    Approval, ApprovalStore, BindState, BindingRecord, CheckoutGrant,
    CheckoutGrantStore, LicenceError, LicenceEventSink, NoExtraChecks, Outcome, SignedApproval,
    SignedBinding, SignedEnvelope, SignedGrant, envelope_key, sign_binding,
};
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_delivery::PeerCtx;
use crate::mesh_runtime::{
    COG_BINDING_TOPIC, COG_GRANT_TOPIC, COG_SYNC_TOPIC, MeshRuntime, PeerControlSink,
};
use crate::workload_pkg::TrustAnchors;

type Seen = (std::collections::HashSet<[u8; 32]>, std::collections::VecDeque<[u8; 32]>);

/// Receives, applies, forwards and syncs the licence records of one node.
pub struct LicenceExchange {
    pub(super) store: Arc<CheckoutGrantStore>,
    pub(super) approvals: Arc<ApprovalStore>,
    pub(super) anchors: Arc<TrustAnchors>,
    pub(super) runtime: Arc<MeshRuntime>,
    pub(super) posture: PostureFn,
    pub(super) admission: Arc<dyn PeerAdmission>,
    pub(super) sink: Arc<dyn LicenceEventSink>,
    pub(super) config: LicenceExchangeConfig,
    pub(super) me: Weak<Self>,
    flood_buckets: Buckets,
    pub(super) sync_buckets: Buckets,
    seen: Mutex<Seen>,
    /// Peers with a sync request in flight.
    pub(super) pending: DashMap<String, super::exchange_sync::Pending>,
    /// When a sync was last answered for a peer.
    pub(super) served: DashMap<String, Instant>,
    /// Peers banned from sync, and since when.
    pub(super) banned: DashMap<String, Instant>,
}

impl LicenceExchange {
    /// Start the exchange: install the control sinks on the runtime and
    /// begin the periodic and on-connect catch-up sync (when inside a tokio
    /// runtime).
    pub fn start(p: LicenceExchangeParts) -> Arc<Self> {
        let rt = p.runtime.clone();
        let me = Arc::new_cyclic(|w| Self {
            store: p.store,
            approvals: p.approvals,
            anchors: p.anchors,
            runtime: p.runtime,
            posture: p.posture,
            admission: p.admission,
            sink: p.sink,
            config: p.config,
            me: w.clone(),
            flood_buckets: Buckets::new(RECORDS_PER_SEC, RECORD_BURST),
            sync_buckets: Buckets::new(super::exchange_sync::SYNC_ENTRIES_PER_SEC, super::exchange_sync::SYNC_BURST),
            seen: Default::default(),
            pending: DashMap::new(),
            served: DashMap::new(),
            banned: DashMap::new(),
        });
        rt.set_control_sink(COG_BINDING_TOPIC, Arc::new(BindingSink(me.clone())));
        rt.set_control_sink(COG_GRANT_TOPIC, Arc::new(GrantSink(me.clone())));
        rt.set_control_sink(COG_SYNC_TOPIC, Arc::new(super::exchange_sync::SyncSink(me.clone())));
        me.spawn_sync_tasks();
        me
    }

    /// The local mesh id, or [`LicenceError::NoLocalMesh`] (the exchange is
    /// inert while it is unset).
    pub(super) fn local(&self) -> Result<super::MeshId, LicenceError> {
        self.store.local_mesh_id().get().ok_or(LicenceError::NoLocalMesh)
    }

    fn record_key(tag: u8, env: &SignedEnvelope) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(&[tag]);
        h.update(env.payload.as_bytes());
        h.update(env.public_key.as_bytes());
        h.update(env.signature.as_bytes());
        *h.finalize().as_bytes()
    }

    fn already_seen(&self, key: &[u8; 32]) -> bool {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).0.contains(key)
    }

    fn remember(&self, key: [u8; 32]) {
        let mut g = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if g.0.insert(key) {
            g.1.push_back(key);
            if g.1.len() > MAX_SEEN
                && let Some(old) = g.1.pop_front()
            {
                g.0.remove(&old);
            }
        }
    }

    /// Spend a token before a signature verify (`conn` `None` is exempt).
    fn budget(&self, kind: Budget, spend: Spend) -> Result<(), ExchangeError> {
        let ok = match spend {
            Spend::Exempt => true,
            Spend::Flood(c) => self.flood_buckets.take(kind, c, 1.0),
            Spend::Sync(c) => self.sync_buckets.take(Budget::Sync, c, 1.0),
        };
        if ok { Ok(()) } else { Err(ExchangeError::RateLimited) }
    }

    fn receipt(o: Outcome) -> Receipt {
        match o {
            Outcome::Applied => Receipt::New,
            Outcome::Duplicate | Outcome::Ignored => Receipt::Known,
        }
    }

    /// Cheap pre-verify filter and apply for a binding. `spend` picks
    /// the bucket (the operator's own issue); see [`Spend`].
    pub fn accept_binding(
        &self,
        env: &SignedBinding,
        spend: Spend,
    ) -> Result<Receipt, ExchangeError> {
        let local = self.local()?;
        let pk = envelope_key(env)?;
        require_operator(&self.anchors, &pk)?;
        let rec: BindingRecord = serde_json::from_str(&env.payload)
            .map_err(|e| LicenceError::Malformed(e.to_string()))?;
        if rec.mesh_id != local.to_hex() {
            return Err(LicenceError::WrongMesh.into());
        }
        if let Some((seq, held)) = self.store.held_binding()
            && (rec.seq < seq || (rec.seq == seq && held.payload == env.payload))
        {
            return Ok(Receipt::Known);
        }
        let key = Self::record_key(1, env);
        if self.already_seen(&key) {
            return Ok(Receipt::Known);
        }
        self.budget(Budget::Binding, spend)?;
        let out = self.store.accept_binding(env, (self.posture)(), &NoExtraChecks)?;
        self.remember(key);
        Ok(Self::receipt(out))
    }

    /// Cheap pre-verify filter and apply for a grant.
    pub fn accept_grant(
        &self,
        env: &SignedGrant,
        spend: Spend,
    ) -> Result<Receipt, ExchangeError> {
        self.local()?;
        let pk = envelope_key(env)?;
        let bound = self.store.bound_grant_key().ok_or(LicenceError::NoBinding)?;
        if super::hex_encode(&pk) != bound {
            return Err(LicenceError::UntrustedKey.into());
        }
        let g: CheckoutGrant = serde_json::from_str(&env.payload)
            .map_err(|e| LicenceError::Malformed(e.to_string()))?;
        if let Some((seq, held)) = self.store.held_grant(&g.cog_id, &g.version)
            && (g.seq < seq || (g.seq == seq && held.payload == env.payload))
        {
            return Ok(Receipt::Known);
        }
        let key = Self::record_key(2, env);
        if self.already_seen(&key) {
            return Ok(Receipt::Known);
        }
        self.budget(Budget::Grant, spend)?;
        match self.store.accept_grant(env) {
            Ok(o) => {
                self.remember(key);
                Ok(Self::receipt(o))
            }
            Err(LicenceError::NotYetValid) => Ok(Receipt::Deferred),
            Err(e) => Err(e.into()),
        }
    }

    /// Cheap pre-verify filter and apply for an approval.
    pub fn accept_approval(
        &self,
        env: &SignedApproval,
        spend: Spend,
    ) -> Result<Receipt, ExchangeError> {
        let local = self.local()?;
        let pk = envelope_key(env)?;
        require_operator(&self.anchors, &pk)?;
        let a: Approval = serde_json::from_str(&env.payload)
            .map_err(|e| LicenceError::Malformed(e.to_string()))?;
        if a.mesh_id != local.to_hex() {
            return Err(LicenceError::WrongMesh.into());
        }
        if self.approvals.holds(&a.content_key()) {
            return Ok(Receipt::Known);
        }
        let key = Self::record_key(3, env);
        if self.already_seen(&key) {
            return Ok(Receipt::Known);
        }
        self.budget(Budget::Approval, spend)?;
        let out = self.approvals.accept(env)?;
        self.remember(key);
        Ok(Self::receipt(out))
    }

    /// Accept a binding here (the operator's own node, not rate limited) and
    /// send it to every peer. Used for a bind and for an unbind: any node
    /// with an operator key may issue one, so a steward cannot block it.
    pub async fn issue_binding(&self, env: SignedBinding) -> Result<Receipt, ExchangeError> {
        let r = self.accept_binding(&env, Spend::Exempt)?;
        self.flood_binding(&env, None).await;
        Ok(r)
    }

    /// As [`Self::issue_binding`] for a grant.
    pub async fn issue_grant(&self, env: SignedGrant) -> Result<Receipt, ExchangeError> {
        let r = self.accept_grant(&env, Spend::Exempt)?;
        self.flood_grant(&GrantMsg::Grant(env), None).await;
        Ok(r)
    }

    /// As [`Self::issue_binding`] for an approval.
    pub async fn issue_approval(&self, env: SignedApproval) -> Result<Receipt, ExchangeError> {
        let r = self.accept_approval(&env, Spend::Exempt)?;
        self.flood_grant(&GrantMsg::Approval(env), None).await;
        Ok(r)
    }

    pub(super) async fn flood_binding(&self, env: &SignedBinding, except: Option<&str>) {
        if let Ok(v) = serde_json::to_value(env) {
            self.send_all(COG_BINDING_TOPIC, v, except).await;
        }
    }

    pub(super) async fn flood_grant(&self, msg: &GrantMsg, except: Option<&str>) {
        if let Ok(v) = serde_json::to_value(msg) {
            self.send_all(COG_GRANT_TOPIC, v, except).await;
        }
    }

    async fn send_all(&self, topic: &str, value: serde_json::Value, except: Option<&str>) {
        for peer in self.runtime.peer_ids() {
            if Some(peer.as_str()) != except {
                self.send_to(&peer, topic, value.clone()).await;
            }
        }
    }

    pub(super) async fn send_to(&self, peer: &str, topic: &str, value: serde_json::Value) {
        let msg = KernelMessage::new(
            0,
            MessageTarget::Topic(topic.to_string()),
            MessagePayload::Json(value),
        );
        if let Err(e) = self.runtime.route_to_remote(peer, msg).await {
            tracing::debug!(peer, topic, error = %e, "licence send failed");
        }
    }

    /// Forward a record that was new here, on a task (the sink is sync and
    /// the runtime is mid-dispatch).
    pub(super) fn forward(&self, from: &str, what: Forward) {
        let Some(me) = self.me.upgrade() else { return };
        let from = from.to_owned();
        tokio::spawn(async move {
            match what {
                Forward::Binding(b) => me.flood_binding(&b, Some(&from)).await,
                Forward::Grant(m) => me.flood_grant(&m, Some(&from)).await,
            }
        });
    }
}

/// A record to pass on.
pub(super) enum Forward {
    Binding(SignedBinding),
    Grant(GrantMsg),
}

/// Sign the unbind of `prev`: same Seed and mesh, `state: unbound`, `seq`
/// one higher. Any node holding an operator key can do this and
/// [`LicenceExchange::issue_binding`] it.
pub fn sign_unbind(
    prev: &BindingRecord,
    now: u64,
    operator: &SigningKey,
) -> Result<SignedBinding, LicenceError> {
    let rec = BindingRecord {
        state: BindState::Unbound,
        seq: prev.seq.saturating_add(1),
        bound_at: now,
        ..prev.clone()
    };
    sign_binding(&rec, operator)
}

struct BindingSink(Arc<LicenceExchange>);
struct GrantSink(Arc<LicenceExchange>);

fn refused(kind: &str, ctx: &PeerCtx, e: &ExchangeError) {
    tracing::warn!(peer = %ctx.peer_id, verified = ctx.node_verified, error = %e,
        "licence {kind} refused");
}

impl PeerControlSink for BindingSink {
    fn on_peer_control(&self, ctx: &PeerCtx, conn: u64, payload: &serde_json::Value) -> Vec<serde_json::Value> {
        let Ok(env) = serde_json::from_value::<SignedBinding>(payload.clone()) else {
            tracing::warn!(peer = %ctx.peer_id, "malformed licence binding message");
            return Vec::new();
        };
        match self.0.accept_binding(&env, Spend::Flood(conn)) {
            Ok(Receipt::New) => self.0.forward(&ctx.peer_id, Forward::Binding(env)),
            Ok(_) => {}
            Err(ExchangeError::Licence(LicenceError::NoLocalMesh)) => {}
            Err(e) => refused("binding", ctx, &e),
        }
        Vec::new()
    }
}

impl PeerControlSink for GrantSink {
    fn on_peer_control(&self, ctx: &PeerCtx, conn: u64, payload: &serde_json::Value) -> Vec<serde_json::Value> {
        let Ok(msg) = serde_json::from_value::<GrantMsg>(payload.clone()) else {
            tracing::warn!(peer = %ctx.peer_id, "malformed licence grant message");
            return Vec::new();
        };
        let r = match &msg {
            GrantMsg::Grant(g) => self.0.accept_grant(g, Spend::Flood(conn)),
            GrantMsg::Approval(a) => self.0.accept_approval(a, Spend::Flood(conn)),
        };
        match r {
            Ok(Receipt::New) => self.0.forward(&ctx.peer_id, Forward::Grant(msg)),
            Ok(_) => {}
            Err(ExchangeError::Licence(LicenceError::NoLocalMesh)) => {}
            Err(e) => refused("grant", ctx, &e),
        }
        Vec::new()
    }
}
