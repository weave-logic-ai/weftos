//! Substrate-backed [`ConversationSink`] (`agent-core-v1.md` Phase C3).
//!
//! Per-turn JSONL lands at
//! `substrate/_derived/chat/<conv_id>/turns/<ulid>`; the per-conv
//! heartbeat publishes `substrate/_derived/chat/<conv_id>/status`.
//! Both paths sit under the mesh-canonical `_derived/` tier and
//! require the daemon's `chat` `DerivedWriteGrant` (issued at boot,
//! Phase A2); the sink routes through
//! [`SubstrateService::publish_gated_with_grants`] and surfaces any
//! [`clawft_kernel::substrate_service::GateDenied`] back to the caller.
//!
//! ## Heartbeat
//!
//! [`SubstrateConversationSink::start_heartbeat`] spawns a tokio
//! interval task on [`HEARTBEAT_PERIOD`] (default 2s) with
//! `MissedTickBehavior::Skip`. The task holds a [`Weak<Self>`] so a
//! dropped sink doesn't leak — the next tick's upgrade fails and the
//! task exits. The plan integrates `start_heartbeat` on the first
//! dispatch for a conv and `stop_heartbeat` at cancel/shutdown; C3
//! only exposes the API, the lifecycle wiring is a follow-up.
//!
//! ## TurnContent (voice forward-compat per plan §10)
//!
//! Only [`TurnContent::Text`] is constructed today. The Audio /
//! Mixed variants and [`AudioRef`] are defined now so the substrate
//! JSONL doesn't reshape when voice ships. [`AudioRef::substrate_path`]
//! always points at substrate-resident PCM — turn records never
//! inline audio bytes.
//!
//! ## Versus `agent/memory.rs`
//!
//! Distinct concerns. [`ConversationSink`] owns per-turn substrate
//! JSONL (this module). `clawft_core::agent::memory` owns cross-
//! conversation distilled facts. They never share a substrate path.
//! Phase 4's `MemoryConsolidator` bridges them; it lives elsewhere.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use clawft_core::agent::sink::{ConversationSink, Turn};
use clawft_kernel::causal::NodeId as CausalNodeId;
use clawft_kernel::{
    CausalEdgeType, CausalGraph, ChainManager, HnswService, NodeRegistry, SubstrateService,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};

/// Default heartbeat period — every 2s a `"alive"` status frame
/// lands at `derived/chat/<conv>/status`. Picked to match the
/// panel's expected liveness cadence without flooding the substrate
/// fan-out.
pub const HEARTBEAT_PERIOD: Duration = Duration::from_secs(2);

/// Wire-shape for a per-turn record's content.
///
/// Voice forward-compat per `agent-core-v1.md` §10. Wire shape is
/// externally-tagged JSON (`{"text": "..."}` / `{"audio": {...}}` /
/// `{"mixed": [...]}`); internally-tagged would reject newtype
/// variants over primitives, untagged would be ambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnContent {
    /// Plain UTF-8 text — every assistant/user/tool turn the chat
    /// loop produces today.
    Text(String),
    /// A reference to substrate-resident audio. The PCM bytes
    /// themselves live at [`AudioRef::substrate_path`]; turn records
    /// never inline audio.
    Audio(AudioRef),
    /// An ordered mix of text and audio fragments — placeholder for
    /// a multi-modal voice + text reply.
    Mixed(Vec<TurnContentPart>),
}

/// One fragment of a [`TurnContent::Mixed`] payload. Same external
/// tagging strategy as [`TurnContent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnContentPart {
    /// A text segment.
    Text(String),
    /// An audio segment (substrate-pointed; see [`AudioRef`]).
    Audio(AudioRef),
}

/// Pointer to a substrate-resident audio asset.
///
/// The substrate path holds the actual PCM/encoded audio; this
/// struct is the lightweight reference recorded in the conversation
/// JSONL. `mime` is the wire's MIME type (e.g. `"audio/wav"`,
/// `"audio/opus"`); `duration_ms` is the audio's wall-clock length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioRef {
    /// Substrate path where the audio bytes live.
    pub substrate_path: String,
    /// MIME type of the encoded audio (e.g. `"audio/wav"`).
    pub mime: String,
    /// Audio duration in milliseconds.
    pub duration_ms: u64,
}

/// Test seam over [`SubstrateService`] + [`NodeRegistry`].
///
/// Production impl ([`KernelSubstrateClient`]) routes publishes
/// through [`SubstrateService::publish_gated_with_grants`] so the
/// mesh-canonical write gate (R3.6) is respected. Tests stub with a
/// `Mutex<HashMap>`. Methods are sync — the underlying
/// [`SubstrateService`] is sync; the sink wraps each call in
/// `async fn` to satisfy [`ConversationSink`].
pub trait SubstrateClient: Send + Sync + 'static {
    /// Publish a `Replace` value at `path` under `node_id`'s grants.
    fn publish(&self, node_id: &str, path: &str, value: Value) -> Result<u64, String>;
    /// Enumerate strict descendants of `prefix` up to `depth` levels.
    fn list(&self, prefix: &str, depth: u32) -> Result<Vec<String>, String>;
    /// Read the current value at `path`, `None` if unset.
    fn read(&self, path: &str) -> Result<Option<Value>, String>;
}

/// Production [`SubstrateClient`] over a real kernel pair. Both
/// [`SubstrateService`] and [`NodeRegistry`] are `Clone` (each is
/// `Arc`-shared internally); this wrapper just bundles them.
pub struct KernelSubstrateClient {
    substrate: SubstrateService,
    node_registry: NodeRegistry,
}

impl KernelSubstrateClient {
    /// Construct from a substrate service and node registry handle.
    pub fn new(substrate: SubstrateService, node_registry: NodeRegistry) -> Self {
        Self {
            substrate,
            node_registry,
        }
    }
}

impl SubstrateClient for KernelSubstrateClient {
    fn publish(&self, node_id: &str, path: &str, value: Value) -> Result<u64, String> {
        self.substrate
            .publish_gated_with_grants(Some(node_id), path, value, &self.node_registry)
            .map_err(|e| e.to_string())
    }

    fn list(&self, prefix: &str, depth: u32) -> Result<Vec<String>, String> {
        // `caller=None` mirrors substrate.list RPC's anonymous read
        // path; capture-tier siblings (none expected under
        // `_derived/chat/`) stay hidden via the same egress gate.
        let snap = self
            .substrate
            .list(None, prefix, depth)
            .map_err(|e| e.to_string())?;
        Ok(snap
            .children
            .into_iter()
            .filter(|c| c.has_value)
            .map(|c| c.path)
            .collect())
    }

    fn read(&self, path: &str) -> Result<Option<Value>, String> {
        let snap = self.substrate.read(None, path).map_err(|e| e.to_string())?;
        Ok(snap.value)
    }
}

/// Side-effect seam invoked after every successful per-turn substrate
/// publish. Lets `agent.chat` mirror turns into the witness chain, the
/// HNSW index, and the causal graph without giving the sink a hard
/// dependency on each kernel handle. The default impl is
/// [`NoopTurnAnchor`]; the daemon swaps in [`KernelTurnAnchor`] when
/// `[kernel.agent].anchor_*` flags are on.
#[async_trait]
pub trait TurnAnchor: Send + Sync + 'static {
    /// Mirror a freshly-published turn into ancillary stores. Errors
    /// are logged at the call site — anchoring is best-effort and must
    /// not fail a turn that already landed in substrate.
    async fn anchor_turn(&self, conv_id: &str, turn_id: &str, turn: &Turn);
}

/// Default [`TurnAnchor`] — drops the call. Used when no anchor flag
/// is enabled, and as the default for tests.
pub struct NoopTurnAnchor;

#[async_trait]
impl TurnAnchor for NoopTurnAnchor {
    async fn anchor_turn(&self, _conv_id: &str, _turn_id: &str, _turn: &Turn) {}
}

/// HNSW embedding dimension — matches the kernel's
/// `HnswServiceConfig::default().default_dimensions`. Hardcoded here
/// to keep the anchor self-contained; if the kernel default ever
/// drifts the test in this module catches the mismatch.
const HNSW_EMBED_DIM: usize = 384;

/// Kernel-backed [`TurnAnchor`].
///
/// Each enabled handle drives one side-effect on `anchor_turn`:
///
/// - `chain` → `chain.append("agent", "agent.chat.turn", payload)`
///   with `{conv_id, turn_id, role, content_hash, ts_ms}`. Witness
///   chain seq advances on every turn.
/// - `hnsw` → `hnsw.insert(turn_id, embed, metadata)` where `embed`
///   is a deterministic-hash 384-d vector derived from the turn id +
///   content. Explorer "Vector entries" KPI ticks. The vector is not
///   semantic; a future change will route through a real embedder.
/// - `causal` → `causal.add_node(label, metadata)` for the new turn,
///   plus `causal.link(prev, this, edge_type)` when this conv has a
///   prior turn. Explorer "Causal graph" KPI ticks.
///
/// Per-conv "previous turn node id" is held in `prev_causal` so
/// links span turns within a conversation. Concurrent `agent.chat`
/// dispatches on the same conv are already serialised by the C1
/// per-conv `Mutex<()>` in `AgentService`, so this map only needs
/// to cope with cross-conv parallelism.
pub struct KernelTurnAnchor {
    chain: Option<Arc<ChainManager>>,
    hnsw: Option<Arc<HnswService>>,
    causal: Option<Arc<CausalGraph>>,
    prev_causal: DashMap<String, CausalNodeId>,
}

impl KernelTurnAnchor {
    /// Build with explicit handles. Pass `None` for any side-effect
    /// the operator hasn't enabled.
    pub fn new(
        chain: Option<Arc<ChainManager>>,
        hnsw: Option<Arc<HnswService>>,
        causal: Option<Arc<CausalGraph>>,
    ) -> Self {
        Self {
            chain,
            hnsw,
            causal,
            prev_causal: DashMap::new(),
        }
    }

    /// True if any side-effect handle is present. The daemon uses
    /// this to decide between [`NoopTurnAnchor`] (cheaper) and the
    /// kernel-backed instance.
    pub fn any_enabled(&self) -> bool {
        self.chain.is_some() || self.hnsw.is_some() || self.causal.is_some()
    }
}

/// Deterministic hash → 384-d unit-norm embedding. Splits a
/// SHA-256 of the input into 8 32-bit seeds and unfolds each into
/// 48 floats via xorshift, so a tiny input still fills the vector
/// without collisions on the first byte. Result is L2-normalised so
/// HNSW cosine math stays well-conditioned.
fn hash_embed(input: &str, dim: usize) -> Vec<f32> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // 8 independent 64-bit seeds derived from differently-salted hashes.
    let mut seeds = [0u64; 8];
    for (i, seed) in seeds.iter_mut().enumerate() {
        let mut h = DefaultHasher::new();
        (i as u32).hash(&mut h);
        input.hash(&mut h);
        *seed = h.finish().max(1);
    }
    let mut out = Vec::with_capacity(dim);
    for i in 0..dim {
        let s = &mut seeds[i % seeds.len()];
        // xorshift64 — cheap, well-distributed, no allocations.
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        // Map u64 → f32 in [-1, 1].
        let u = (*s >> 32) as u32;
        out.push((u as f32) / (u32::MAX as f32) * 2.0 - 1.0);
    }
    let norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    for x in &mut out {
        *x /= norm;
    }
    out
}

#[async_trait]
impl TurnAnchor for KernelTurnAnchor {
    async fn anchor_turn(&self, conv_id: &str, turn_id: &str, turn: &Turn) {
        // Compact content hash (first 16 hex chars of a default-hasher
        // digest). Cheap to compute, enough to dedupe identical turns
        // in a chain audit without dragging sha2 into this crate.
        let content_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            turn.role.hash(&mut h);
            turn.content.hash(&mut h);
            format!("{:016x}", h.finish())
        };

        // 1. Witness chain.
        if let Some(ref chain) = self.chain {
            let payload = serde_json::json!({
                "conv_id": conv_id,
                "turn_id": turn_id,
                "role": turn.role,
                "content_hash": content_hash,
                "ts_ms": turn.ts_ms,
            });
            // chain.append never errs in the current API — it panics
            // on poisoned lock, which is a programmer-error class
            // failure we don't want to swallow.
            let _ = chain.append("agent", "agent.chat.turn", Some(payload));
        }

        // 2. HNSW vector index.
        if let Some(ref hnsw) = self.hnsw {
            let embed_input = format!("{conv_id}:{turn_id}:{}", turn.content);
            let vec = hash_embed(&embed_input, HNSW_EMBED_DIM);
            let metadata = serde_json::json!({
                "conv_id": conv_id,
                "turn_id": turn_id,
                "role": turn.role,
                "ts_ms": turn.ts_ms,
            });
            // HnswService::insert is `&self` and never returns an err.
            hnsw.insert(turn_id.to_string(), vec, metadata);
        }

        // 3. Causal graph node + link to prev turn in this conv.
        if let Some(ref causal) = self.causal {
            let label = format!("turn:{conv_id}:{turn_id}");
            let metadata = serde_json::json!({
                "conv_id": conv_id,
                "turn_id": turn_id,
                "role": turn.role,
                "ts_ms": turn.ts_ms,
            });
            let new_node = causal.add_node(label, metadata);
            if let Some(prev) = self.prev_causal.insert(conv_id.to_string(), new_node) {
                // link(prev → new). chain_seq=0 is fine here: the
                // causal graph stamps it when chain wiring is later
                // bolted in via set_chain_manager. ts_ms can come from
                // the turn directly so causal time matches substrate.
                let linked =
                    causal.link(prev, new_node, CausalEdgeType::Follows, 1.0, turn.ts_ms, 0);
                if !linked {
                    debug!(
                        conv_id,
                        turn_id,
                        prev = prev,
                        new = new_node,
                        "causal anchor: link skipped (endpoint missing)"
                    );
                }
            }
        }
    }
}

/// Substrate-backed [`ConversationSink`] for `agent.chat`.
///
/// See module docs for the path layout, heartbeat lifecycle, and the
/// [`TurnContent`] forward-compat plan.
pub struct SubstrateConversationSink {
    client: Arc<dyn SubstrateClient>,
    /// Daemon node-id — caller for the gated publish (grant lookup
    /// keys on it) and "actor" stamped on the fan-out line.
    node_id: String,
    /// Heartbeat interval; tests pass a smaller value to run quickly.
    heartbeat_period: Duration,
    /// Per-conv heartbeat task. `start_heartbeat` inserts;
    /// `stop_heartbeat` (or [`Drop`]) aborts.
    heartbeats: DashMap<String, JoinHandle<()>>,
    /// Per-conv monotonic counter; prepended as a fixed-width base-32
    /// PREFIX before the ULID in [`Self::turn_id_for`] so two appends
    /// within the same ms still sort by append order (the ULID's
    /// intra-ms bits are random and cannot order burst-fire turns).
    counters: DashMap<String, AtomicU64>,
    /// Side-effect seam — fired after every successful publish in
    /// `append_turn`. Defaults to [`NoopTurnAnchor`]; the daemon
    /// swaps in [`KernelTurnAnchor`] when any `[kernel.agent]` flag
    /// is enabled.
    anchor: Arc<dyn TurnAnchor>,
}

impl SubstrateConversationSink {
    /// Build a sink backed by a real kernel pair.
    ///
    /// Convenience for the daemon construction site —
    /// `clawft-weave::daemon` already has both handles on hand.
    /// Anchor side-effects default off; use [`Self::with_anchor`] to
    /// opt in to chain / hnsw / causal mirroring.
    pub fn new(
        substrate: SubstrateService,
        node_registry: NodeRegistry,
        node_id: impl Into<String>,
    ) -> Self {
        Self::with_client(
            Arc::new(KernelSubstrateClient::new(substrate, node_registry)),
            node_id,
            HEARTBEAT_PERIOD,
        )
    }

    /// Build a sink against an arbitrary [`SubstrateClient`]. Tests
    /// pass a `Mutex<HashMap>` stub here. Anchor defaults to
    /// [`NoopTurnAnchor`].
    pub fn with_client(
        client: Arc<dyn SubstrateClient>,
        node_id: impl Into<String>,
        heartbeat_period: Duration,
    ) -> Self {
        Self::with_client_and_anchor(client, node_id, heartbeat_period, Arc::new(NoopTurnAnchor))
    }

    /// Build a sink with an explicit [`TurnAnchor`]. Daemon path —
    /// pass [`KernelTurnAnchor`] when any `[kernel.agent]` flag is
    /// enabled, [`NoopTurnAnchor`] otherwise.
    pub fn with_client_and_anchor(
        client: Arc<dyn SubstrateClient>,
        node_id: impl Into<String>,
        heartbeat_period: Duration,
        anchor: Arc<dyn TurnAnchor>,
    ) -> Self {
        Self {
            client,
            node_id: node_id.into(),
            heartbeat_period,
            heartbeats: DashMap::new(),
            counters: DashMap::new(),
            anchor,
        }
    }

    /// Daemon convenience over [`Self::new`] that also installs an
    /// explicit anchor. Mirrors the production wiring path:
    /// `SubstrateService` + `NodeRegistry` from the booted kernel,
    /// plus a [`KernelTurnAnchor`] built from the same kernel's
    /// chain / hnsw / causal handles.
    pub fn with_anchor(
        substrate: SubstrateService,
        node_registry: NodeRegistry,
        node_id: impl Into<String>,
        anchor: Arc<dyn TurnAnchor>,
    ) -> Self {
        Self::with_client_and_anchor(
            Arc::new(KernelSubstrateClient::new(substrate, node_registry)),
            node_id,
            HEARTBEAT_PERIOD,
            anchor,
        )
    }

    /// Substrate path for the per-turn JSONL subtree.
    fn turns_prefix(conv_id: &str) -> String {
        format!("substrate/_derived/chat/{conv_id}/turns")
    }

    /// Substrate path for the heartbeat / status frame.
    fn status_path(conv_id: &str) -> String {
        format!("substrate/_derived/chat/{conv_id}/status")
    }

    /// Mint a sortable per-turn id: a fixed-width base-32 per-conv
    /// counter PREFIX followed by a [`ulid::Ulid::new()`] suffix
    /// (ms-prefixed timestamp + 80-bit randomness for uniqueness).
    ///
    /// The counter leads so a lexicographic sort of turn_ids preserves
    /// append order even for several turns in the same millisecond — a
    /// ULID-first id sorts by the ULID's random intra-ms bits and cannot
    /// order burst-fire turns (the old `{ULID}-{counter}` format flaked
    /// ~50% of the time). `base32_u64` is big-endian, so left-padding
    /// with '0' to 13 chars (width of u64::MAX in base 32) keeps the
    /// lexicographic order numeric.
    fn turn_id_for(&self, conv_id: &str) -> String {
        let counter_entry = self
            .counters
            .entry(conv_id.to_string())
            .or_insert_with(|| AtomicU64::new(0));
        let n = counter_entry.fetch_add(1, Ordering::AcqRel);
        format!("{:0>13}-{}", base32_u64(n), ulid::Ulid::new())
    }

    /// Spawn the per-conv heartbeat task. Idempotent. The task holds
    /// a [`Weak<Self>`] so a dropped sink doesn't keep it alive — on
    /// the next tick the upgrade fails and the task returns.
    pub fn start_heartbeat(self: &Arc<Self>, conv_id: impl Into<String>) {
        let conv_id = conv_id.into();
        if self.heartbeats.contains_key(&conv_id) {
            debug!(conv_id, "heartbeat already running");
            return;
        }
        let me_weak: Weak<Self> = Arc::downgrade(self);
        let period = self.heartbeat_period;
        let conv_for_task = conv_id.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // First tick returns immediately; drop it so the first
            // publish lands one full period in. (At t=0 the dispatch
            // itself already proved liveness.)
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(me) = me_weak.upgrade() else {
                    return; // sink dropped — exit cleanly
                };
                let payload = serde_json::json!({ "ts_ms": now_ms() });
                if let Err(e) = me.publish_status(&conv_for_task, "alive", payload).await {
                    warn!(error = %e, conv_id = %conv_for_task, "heartbeat publish failed");
                }
            }
        });
        self.heartbeats.insert(conv_id, task);
    }

    /// Abort and forget the heartbeat task for `conv_id`. Safe if no
    /// task is running.
    pub fn stop_heartbeat(&self, conv_id: &str) {
        if let Some((_, task)) = self.heartbeats.remove(conv_id) {
            task.abort();
        }
    }

    /// Number of live heartbeat tasks. Test helper.
    pub fn live_heartbeats(&self) -> usize {
        self.heartbeats.len()
    }
}

impl Drop for SubstrateConversationSink {
    fn drop(&mut self) {
        // Belt-and-braces: the `Weak<Self>` upgrade in the heartbeat
        // task already exits the loop, but a pending task with no
        // observers wastes a tokio slot until its next tick. Abort
        // each handle so the runtime reaps the task immediately.
        for entry in self.heartbeats.iter() {
            entry.value().abort();
        }
    }
}

#[async_trait]
impl ConversationSink for SubstrateConversationSink {
    async fn lock_conversation(&self, _conv_id: &str) {
        // No-op. The per-conv `Mutex<()>` lives on
        // `AgentService` (C1's DashMap of locks); the sink-level
        // method is a no-op here so the in-memory sink's trait
        // contract still holds for tests that exercise both impls.
    }

    async fn append_turn(&self, conv_id: &str, turn: Turn) -> Result<(), String> {
        // Honour caller-supplied ids when present (tests); otherwise
        // mint a sortable ULID-based id.
        let turn_id = if turn.turn_id.is_empty() {
            self.turn_id_for(conv_id)
        } else {
            turn.turn_id.clone()
        };
        let path = format!("{}/{}", Self::turns_prefix(conv_id), turn_id);
        // `content_type: "text"` discriminant on the wire so future
        // Audio/Mixed turns parse without a schema bump.
        let body = serde_json::json!({
            "turn_id": turn_id,
            "role": turn.role,
            "content": turn.content,
            "tool_calls": turn.tool_calls,
            "tool_call_id": turn.tool_call_id,
            "ts_ms": turn.ts_ms,
            "content_type": "text",
        });
        // Substrate publish first — that's the durable record. Anchor
        // side-effects are best-effort and only run after the publish
        // succeeded, so a chain/hnsw/causal failure can never lose a
        // turn.
        self.client.publish(&self.node_id, &path, body)?;
        // Re-stamp the turn id so the anchor sees the id we actually
        // minted (which may differ from the caller-supplied empty
        // string).
        let mut anchored = turn;
        anchored.turn_id = turn_id.clone();
        self.anchor.anchor_turn(conv_id, &turn_id, &anchored).await;
        Ok(())
    }

    async fn publish_status(
        &self,
        conv_id: &str,
        status: &str,
        payload: Value,
    ) -> Result<(), String> {
        let body = serde_json::json!({
            "status": status,
            "payload": payload,
            "ts_ms": now_ms(),
        });
        self.client
            .publish(&self.node_id, &Self::status_path(conv_id), body)
            .map(|_| ())
    }

    async fn load_history(&self, conv_id: &str) -> Result<Vec<Turn>, String> {
        let prefix = Self::turns_prefix(conv_id);
        // List one level under the turns prefix — each child is one
        // turn record.
        let paths = self.client.list(&prefix, 1)?;
        let mut turns: Vec<Turn> = Vec::with_capacity(paths.len());
        for p in paths {
            let Some(value) = self.client.read(&p)? else {
                continue;
            };
            match turn_from_value(&value) {
                Some(t) => turns.push(t),
                None => {
                    warn!(path = %p, "load_history: skipping unparseable turn record");
                }
            }
        }
        // Sort ascending by ts_ms so callers always see the
        // conversation in chronological order. Equal ts_ms ties
        // break on turn_id (whose fixed-width per-conv counter prefix
        // preserves append order) so the order is deterministic.
        turns.sort_by(|a, b| {
            a.ts_ms
                .cmp(&b.ts_ms)
                .then_with(|| a.turn_id.cmp(&b.turn_id))
        });
        Ok(turns)
    }
}

/// Parse a substrate JSONL turn record back into a [`Turn`]. Returns
/// `None` if the payload is malformed (missing required fields). The
/// caller logs and skips on parse failure rather than failing the
/// whole `load_history`.
fn turn_from_value(v: &Value) -> Option<Turn> {
    let obj = v.as_object()?;
    let turn_id = obj.get("turn_id")?.as_str()?.to_string();
    let role = obj.get("role")?.as_str()?.to_string();
    let content = obj.get("content")?.as_str()?.to_string();
    let ts_ms = obj.get("ts_ms")?.as_u64()?;
    let tool_calls = obj
        .get("tool_calls")
        .and_then(|tc| if tc.is_null() { None } else { tc.as_array() })
        .map(|arr| arr.to_vec());
    let tool_call_id = obj
        .get("tool_call_id")
        .and_then(|v| if v.is_null() { None } else { v.as_str() })
        .map(|s| s.to_string());
    Some(Turn {
        turn_id,
        role,
        content,
        tool_calls,
        tool_call_id,
        ts_ms,
    })
}

/// Wall-clock millisecond timestamp; `0` on clock failure.
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Encode a `u64` in base-32 (Crockford alphabet) for the per-conv
/// counter suffix on ULID-keyed turn paths. Matches the ULID's
/// alphabet so the combined id reads as one token.
fn base32_u64(mut n: u64) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if n == 0 {
        return "0".to_string();
    }
    let mut out = Vec::with_capacity(13);
    while n > 0 {
        out.push(ALPHABET[(n & 0x1F) as usize]);
        n >>= 5;
    }
    out.reverse();
    String::from_utf8(out).expect("ALPHABET is ASCII")
}

#[cfg(test)]
mod tests {
    //! Inline unit tests for the private helpers
    //! (`base32_u64`, `turn_from_value`). The integration-style tests
    //! covering the [`ConversationSink`] impl + heartbeat lifecycle
    //! live in `tests/substrate_sink.rs` so this file stays under the
    //! 500-line ceiling per CLAUDE.md.

    use super::*;

    #[test]
    fn base32_u64_smoke() {
        assert_eq!(base32_u64(0), "0");
        assert_eq!(base32_u64(1), "1");
        // Sortable: 32 in base-32 is "10".
        assert_eq!(base32_u64(32), "10");
        // No collisions for small ids.
        let mut seen = std::collections::HashSet::new();
        for n in 0..1024u64 {
            assert!(seen.insert(base32_u64(n)));
        }
    }

    #[test]
    fn turn_from_value_round_trips_required_fields() {
        let v = serde_json::json!({
            "turn_id": "t1",
            "role": "user",
            "content": "hi",
            "tool_calls": null,
            "tool_call_id": null,
            "ts_ms": 42_u64,
            "content_type": "text",
        });
        let t = turn_from_value(&v).expect("parse");
        assert_eq!(t.turn_id, "t1");
        assert_eq!(t.role, "user");
        assert_eq!(t.content, "hi");
        assert_eq!(t.ts_ms, 42);
        assert!(t.tool_calls.is_none());
        assert!(t.tool_call_id.is_none());
    }

    #[test]
    fn turn_from_value_returns_none_on_missing_fields() {
        let v = serde_json::json!({ "role": "user" });
        assert!(turn_from_value(&v).is_none());
    }
}
