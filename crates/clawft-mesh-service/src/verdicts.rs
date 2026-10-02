//! Cluster-owner verdicts (plan 1.2 `verdict.request`/`verdict.reply`, D-3).
//!
//! The service never evaluates governance. It forwards each admission or
//! publish/subscribe question to the registration of the cluster-owner uid
//! (whose daemon evaluates its `GateBackend`) and caches the answer for the
//! `ttl_s` the daemon returned. Every failure mode closes:
//!
//! - no owner registered, queue full, or no reply within `verdict_timeout`:
//!   a peer that was already granted a permit keeps it for `stale_grace`
//!   (default 10 minutes) from the grant; anyone else is
//!   [`Verdict::Unavailable`] (refused like a denial, never cached by the
//!   kernel gate);
//! - a `verdict.reply` is accepted only from the connection that was asked, so
//!   another uid's daemon cannot answer for the owner;
//! - the cache key includes the peer's public key, class and platform, never
//!   the node id alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use clawft_kernel::mesh_admit::{Verdict, VerdictRequest as KernelVerdictRequest, VerdictSource};
use clawft_mesh_local::hexser;
use clawft_mesh_local::proto::{
    Frame, Message, PeerInfo, VerdictRequest, VerdictSubject, SERVICE_ID_FLAG,
};
use tokio::sync::oneshot;

use crate::registry::{Registration, Registry};
use crate::state::PolicyCell;

/// Longest a daemon may make us cache an allow.
pub const MAX_ALLOW_TTL: Duration = Duration::from_secs(3600);
/// Longest a deny stays cached: an owner who changes their mind should not
/// make a peer wait long.
pub const MAX_DENY_TTL: Duration = Duration::from_secs(30);
const MAX_CACHE: usize = 1024;

/// The daemon's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub allow: bool,
    pub ttl_s: u64,
    pub reason: String,
    pub rule_hash: String,
}

/// Outcome of asking the cluster owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow { rule_hash: String, stale: bool },
    Deny(String),
    Unavailable(String),
}

type CacheKey = (VerdictSubjectKey, String, [u8; 32], String, String, Option<String>);
type VerdictSubjectKey = u8;

#[derive(Clone)]
struct Entry {
    at: Instant,
    ttl: Duration,
    allow: bool,
    reason: String,
    rule_hash: String,
}

fn subject_key(s: &VerdictSubject) -> VerdictSubjectKey {
    match s {
        VerdictSubject::PeerAdmit => 0,
        VerdictSubject::ClusterJoin => 1,
        VerdictSubject::Publish => 2,
        VerdictSubject::Subscribe => 3,
        VerdictSubject::Unknown => 4,
    }
}

/// Real asks (cache misses) allowed per minute in total, and per peer node id.
/// A flood of unauthenticated peers can therefore not turn the cluster
/// owner's daemon into the bottleneck: past the budget they get the
/// fail-closed answer.
pub const ASKS_PER_MINUTE: u32 = 120;
pub const ASKS_PER_NODE_PER_MINUTE: u32 = 10;
const MAX_DENY_CACHE: usize = 256;

struct AskBudget {
    window_start: Instant,
    total: u32,
    per_node: HashMap<String, u32>,
}

type Waiters = Vec<oneshot::Sender<Decision>>;

/// Removes this ask's single-flight slot if the leader is dropped before it
/// finished, so waiters fail closed instead of hanging.
struct Flight<'a> {
    broker: &'a VerdictBroker,
    key: CacheKey,
    done: bool,
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.broker.inflight.lock().expect("inflight lock").remove(&self.key);
        }
    }
}

/// Asks the cluster-owner registration and caches what it says.
///
/// Allows and denies live in separate bounded caches: denies are cheap for a
/// hostile peer to generate, and must not be able to evict the allows the
/// stale-grace rule depends on. Concurrent asks of one key share one request.
pub struct VerdictBroker {
    registry: Arc<Registry>,
    policy: Arc<PolicyCell>,
    timeout: Duration,
    stale_grace: Duration,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, (u64, oneshot::Sender<Reply>)>>,
    allows: Mutex<HashMap<CacheKey, Entry>>,
    denies: Mutex<HashMap<CacheKey, Entry>>,
    inflight: Mutex<HashMap<CacheKey, Waiters>>,
    budget: Mutex<AskBudget>,
    /// Last `rule_hash` an allow carried per node id (journalled with `peer.admit`).
    last_rule: Mutex<HashMap<String, String>>,
}

impl VerdictBroker {
    pub fn new(
        registry: Arc<Registry>,
        policy: Arc<PolicyCell>,
        timeout: Duration,
        stale_grace: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            policy,
            timeout,
            stale_grace,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            allows: Mutex::new(HashMap::new()),
            denies: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            budget: Mutex::new(AskBudget { window_start: Instant::now(), total: 0, per_node: HashMap::new() }),
            last_rule: Mutex::new(HashMap::new()),
        })
    }

    fn owner(&self) -> Option<Arc<Registration>> {
        let uid = self.policy.owner_uid()?;
        self.registry.by_principal(&clawft_mesh_local::Principal::Uid(uid))
    }

    /// The rule hash of the newest allow for `node_id`, if any.
    pub fn rule_hash_for(&self, node_id: &str) -> Option<String> {
        self.last_rule.lock().expect("rule lock").get(node_id).cloned()
    }

    /// Forget every cached verdict (owner change, admission-mode change,
    /// revocation or rebind of the owner): an answer given under the old
    /// authority must not outlive it. Stale grace goes with it.
    pub fn clear(&self) {
        self.allows.lock().expect("cache lock").clear();
        self.denies.lock().expect("cache lock").clear();
        self.last_rule.lock().expect("rule lock").clear();
    }

    fn cached(&self, key: &CacheKey) -> Option<Decision> {
        for map in [&self.allows, &self.denies] {
            if let Some(e) = map.lock().expect("cache lock").get(key)
                && e.at.elapsed() < e.ttl
            {
                return Some(decision(e, false));
            }
        }
        None
    }

    fn allow_ask(&self, node: &str) -> bool {
        let mut b = self.budget.lock().expect("budget lock");
        if b.window_start.elapsed() >= Duration::from_secs(60) {
            b.window_start = Instant::now();
            b.total = 0;
            b.per_node.clear();
        }
        if b.total >= ASKS_PER_MINUTE {
            return false;
        }
        if b.per_node.len() >= MAX_CACHE && !b.per_node.contains_key(node) {
            return false;
        }
        let n = b.per_node.entry(node.to_string()).or_insert(0);
        if *n >= ASKS_PER_NODE_PER_MINUTE {
            return false;
        }
        *n += 1;
        b.total += 1;
        true
    }

    /// Ask (or answer from cache) whether `req` is allowed.
    pub async fn ask(&self, req: VerdictRequest) -> Decision {
        let key: CacheKey = (
            subject_key(&req.subject),
            req.peer.node_id.clone(),
            hexser::decode::<32>(&req.peer.pubkey).unwrap_or([0u8; 32]),
            req.peer.platform.clone(),
            req.peer.capabilities.join(","),
            req.topic.clone(),
        );
        if let Some(d) = self.cached(&key) {
            return d;
        }
        // Single flight: later askers of the same key wait for the leader.
        let waiter = {
            let mut f = self.inflight.lock().expect("inflight lock");
            match f.get_mut(&key) {
                Some(v) => {
                    let (tx, rx) = oneshot::channel();
                    v.push(tx);
                    Some(rx)
                }
                None => {
                    f.insert(key.clone(), Vec::new());
                    None
                }
            }
        };
        if let Some(rx) = waiter {
            return match rx.await {
                Ok(d) => d,
                Err(_) => self.fallback(&key, "the concurrent ask was abandoned".into()),
            };
        }
        let mut flight = Flight { broker: self, key: key.clone(), done: false };
        let d = self.decide(&key, &req).await;
        let waiters = self.inflight.lock().expect("inflight lock").remove(&key).unwrap_or_default();
        flight.done = true;
        for w in waiters {
            let _ = w.send(d.clone());
        }
        d
    }

    async fn decide(&self, key: &CacheKey, req: &VerdictRequest) -> Decision {
        if !self.allow_ask(&req.peer.node_id) {
            return self.fallback(key, "ask rate limit reached".into());
        }
        match self.ask_owner(req).await {
            Ok(reply) => {
                let ttl = Duration::from_secs(reply.ttl_s)
                    .min(if reply.allow { MAX_ALLOW_TTL } else { MAX_DENY_TTL });
                let e = Entry {
                    at: Instant::now(),
                    ttl,
                    allow: reply.allow,
                    reason: reply.reason,
                    rule_hash: reply.rule_hash,
                };
                if e.allow {
                    self.last_rule
                        .lock()
                        .expect("rule lock")
                        .insert(req.peer.node_id.clone(), e.rule_hash.clone());
                }
                let d = decision(&e, false);
                self.store(key.clone(), e);
                d
            }
            Err(why) => self.fallback(key, why),
        }
    }

    fn store(&self, key: CacheKey, e: Entry) {
        let (map, cap) = if e.allow { (&self.allows, MAX_CACHE) } else { (&self.denies, MAX_DENY_CACHE) };
        let mut c = map.lock().expect("cache lock");
        if c.len() >= cap {
            let grace = self.stale_grace;
            c.retain(|_, v| v.at.elapsed() < v.ttl.max(if v.allow { grace } else { Duration::ZERO }));
            while c.len() >= cap {
                let Some(old) = c.iter().min_by_key(|(_, v)| v.at).map(|(k, _)| k.clone()) else { break };
                c.remove(&old);
            }
        }
        c.insert(key, e);
    }

    /// D-3: a peer already granted keeps its permit for `stale_grace` from
    /// the grant; new peers (and previously denied ones) are refused.
    fn fallback(&self, key: &CacheKey, why: String) -> Decision {
        if let Some(e) = self.allows.lock().expect("cache lock").get(key)
            && e.at.elapsed() < self.stale_grace
        {
            return decision(e, true);
        }
        Decision::Unavailable(format!("cluster owner unavailable: {why}"))
    }

    async fn ask_owner(&self, req: &VerdictRequest) -> Result<Reply, String> {
        let owner = self.owner().ok_or_else(|| "no cluster-owner daemon is registered".to_string())?;
        let id = SERVICE_ID_FLAG | self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(id, (owner.conn_id, tx));
        let frame = Frame::with_id(id, Message::VerdictRequest(req.clone()));
        if owner.try_queue_priority(frame).is_err() {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err("cluster-owner daemon is not draining its queue".into());
        }
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(reply)) => Ok(reply),
            _ => {
                self.pending.lock().expect("pending lock").remove(&id);
                Err(format!("no verdict within {}s", self.timeout.as_secs()))
            }
        }
    }

    /// A `verdict.reply` arrived on connection `conn_id`. Only the connection
    /// that was asked can answer; anything else is dropped.
    pub fn on_reply(&self, conn_id: u64, id: u64, reply: Reply) -> bool {
        let mut p = self.pending.lock().expect("pending lock");
        match p.get(&id) {
            Some((asked, _)) if *asked == conn_id => {
                let (_, tx) = p.remove(&id).expect("present");
                tx.send(reply).is_ok()
            }
            _ => false,
        }
    }

    /// Drop requests waiting on a connection that closed.
    pub fn forget_conn(&self, conn_id: u64) {
        self.pending.lock().expect("pending lock").retain(|_, (c, _)| *c != conn_id);
    }

    /// Number of verdicts currently cached (tests, status).
    pub fn cache_len(&self) -> usize {
        self.allows.lock().expect("cache lock").len() + self.denies.lock().expect("cache lock").len()
    }
}

fn decision(e: &Entry, stale: bool) -> Decision {
    if e.allow {
        Decision::Allow { rule_hash: e.rule_hash.clone(), stale }
    } else {
        Decision::Deny(if e.reason.is_empty() { "denied by the cluster owner".into() } else { e.reason.clone() })
    }
}

/// The kernel's `VerdictSource` view of the broker (`peer.admit`).
pub struct OwnerVerdicts(pub Arc<VerdictBroker>);

#[async_trait]
impl VerdictSource for OwnerVerdicts {
    async fn verdict(&self, req: &KernelVerdictRequest) -> Verdict {
        let peer = PeerInfo {
            node_id: req.node_id.clone(),
            pubkey: hexser::encode(&req.pubkey),
            platform: req.platform.clone(),
            capabilities: vec![format!("{:?}", req.class).to_lowercase()],
            genesis_hash: String::new(),
            chain_seq: 0,
        };
        let q = VerdictRequest { subject: VerdictSubject::PeerAdmit, peer, topic: None };
        match self.0.ask(q).await {
            Decision::Allow { .. } => Verdict::Permit,
            Decision::Deny(r) => Verdict::Deny(r),
            Decision::Unavailable(r) => Verdict::Unavailable(r),
        }
    }
}

#[cfg(test)]
#[path = "verdicts_tests.rs"]
mod tests;
