//! Mesh liveness and round-trip time: a ping/pong between verified peers.
//!
//! Each node pings every verified peer on the [`PING_TOPIC`] control topic
//! every [`LivenessConfig::interval`]. A pong only counts when it arrives on
//! the verified connection of the peer that was pinged and echoes a nonce
//! this node actually sent and has not yet seen answered. A counted pong
//! feeds the heartbeat tracker ([`MeshRuntime::record_heartbeat`], which
//! drives `Alive`/`Recovered` and cluster `last_heartbeat`) and a smoothed
//! RTT. A pong may carry the responder's [`LoadSample`] (fleet P3); old peers
//! send none. Unverified peers are never pinged, and their pings and pongs
//! are ignored. Everything here is in memory; nothing is persisted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_delivery::PeerCtx;
use crate::mesh_load::LoadSample;
use crate::mesh_runtime::{MeshRuntime, PING_TOPIC, PeerControlSink};

/// Tunables.
#[derive(Debug, Clone, Copy)]
pub struct LivenessConfig {
    /// Time between ping rounds.
    pub interval: Duration,
    /// An unanswered ping older than this is forgotten (and counted as a miss).
    pub timeout: Duration,
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self { interval: Duration::from_secs(10), timeout: Duration::from_secs(30) }
    }
}

/// What this node knows about one peer's liveness.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerLiveness {
    /// Wall-clock time of the last counted pong.
    pub last_seen: SystemTime,
    /// Smoothed round-trip time (EWMA, alpha 0.2), milliseconds.
    pub rtt_ms: f64,
    /// Pings sent that timed out since the last counted pong.
    pub missed: u32,
    /// The load the peer attached to its last counted pong (it says so;
    /// not signed). `None` from a peer that sends none.
    pub load: Option<LoadSample>,
}

/// At most this many pings in flight per peer; a slow peer cannot grow the table.
const MAX_OUTSTANDING_PER_PEER: usize = 4;

#[derive(Default)]
struct State {
    seq: u64,
    /// (peer, nonce) -> when the ping was sent.
    outstanding: HashMap<(String, u64), Instant>,
    peers: HashMap<String, PeerLiveness>,
}

/// The liveness service for one [`MeshRuntime`].
pub struct Liveness {
    rt: Weak<MeshRuntime>,
    cfg: LivenessConfig,
    state: Mutex<State>,
}

impl Liveness {
    /// Build without starting the ping loop (tests drive [`Liveness::tick`]).
    pub fn new(rt: &Arc<MeshRuntime>, cfg: LivenessConfig) -> Arc<Self> {
        Arc::new(Self { rt: Arc::downgrade(rt), cfg, state: Mutex::new(State::default()) })
    }

    /// Install as the [`PING_TOPIC`] sink and spawn the ping loop. Call once.
    pub fn start(self: &Arc<Self>) {
        let Some(rt) = self.rt.upgrade() else { return };
        rt.set_control_sink(PING_TOPIC, self.clone());
        let me = Arc::downgrade(self);
        let interval = self.cfg.interval;
        tokio::spawn(async move {
            let mut t = tokio::time::interval(interval);
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                t.tick().await;
                let Some(me) = me.upgrade() else { break };
                me.tick().await;
            }
        });
    }

    /// One round: expire stale pings, then ping every verified peer.
    pub async fn tick(&self) {
        let Some(rt) = self.rt.upgrade() else { return };
        let targets: Vec<String> =
            rt.peer_details().into_iter().filter(|p| p.verified).map(|p| p.node_id).collect();
        let pings: Vec<(String, u64)> = {
            let mut st = self.state.lock().unwrap();
            let timeout = self.cfg.timeout;
            let expired: Vec<(String, u64)> =
                st.outstanding.iter().filter(|(_, at)| at.elapsed() > timeout).map(|(k, _)| k.clone()).collect();
            for k in expired {
                st.outstanding.remove(&k);
                if let Some(p) = st.peers.get_mut(&k.0) {
                    p.missed = p.missed.saturating_add(1);
                }
            }
            // Forget peers that are no longer connected.
            st.peers.retain(|id, _| targets.contains(id));
            let mut out = Vec::new();
            for peer in &targets {
                let inflight = st.outstanding.keys().filter(|(p, _)| p == peer).count();
                if inflight >= MAX_OUTSTANDING_PER_PEER {
                    continue;
                }
                st.seq += 1;
                let n = st.seq;
                st.outstanding.insert((peer.clone(), n), Instant::now());
                out.push((peer.clone(), n));
            }
            out
        };
        for (peer, n) in pings {
            let _ = rt.route_to_remote(&peer, control(json!({"t": "ping", "n": n}))).await;
        }
    }

    /// The liveness view of `peer`, if a pong from it has been counted.
    pub fn peer(&self, peer: &str) -> Option<PeerLiveness> {
        self.state.lock().unwrap().peers.get(peer).cloned()
    }

    /// Count a pong from a verified `peer` for nonce `n`. Returns true when it counted.
    fn on_pong(&self, peer: &str, n: u64, load: Option<LoadSample>) -> bool {
        let rtt_ms = {
            let mut st = self.state.lock().unwrap();
            let Some(sent) = st.outstanding.remove(&(peer.to_string(), n)) else { return false };
            let sample = sent.elapsed().as_secs_f64() * 1000.0;
            let e = st.peers.entry(peer.to_string()).or_insert(PeerLiveness {
                last_seen: SystemTime::now(),
                rtt_ms: sample,
                missed: 0,
                load: None,
            });
            e.rtt_ms = e.rtt_ms * 0.8 + sample * 0.2;
            e.last_seen = SystemTime::now();
            e.missed = 0;
            e.load = load;
            e.rtt_ms
        };
        if let Some(rt) = self.rt.upgrade() {
            rt.record_heartbeat(peer);
        }
        tracing::trace!(peer, rtt_ms, "mesh pong");
        true
    }
}

impl PeerControlSink for Liveness {
    fn on_peer_control(&self, ctx: &PeerCtx, _conn: u64, payload: &Value) -> Vec<Value> {
        // Only a verified peer takes part: unverified ids are claims.
        if !ctx.node_verified {
            return Vec::new();
        }
        let Some(n) = payload.get("n").and_then(Value::as_u64) else { return Vec::new() };
        match payload.get("t").and_then(Value::as_str) {
            Some("ping") => {
                let mut pong = json!({"t": "pong", "n": n});
                if let Some(l) = LoadSample::local() {
                    pong["load"] = l.to_json();
                }
                vec![pong]
            }
            Some("pong") => {
                let load = payload.get("load").and_then(LoadSample::from_json);
                self.on_pong(&ctx.peer_id, n, load);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

fn control(value: Value) -> KernelMessage {
    KernelMessage::new(0, MessageTarget::Topic(PING_TOPIC.to_string()), MessagePayload::Json(value))
}

#[cfg(test)]
#[path = "mesh_liveness_tests.rs"]
mod tests;
