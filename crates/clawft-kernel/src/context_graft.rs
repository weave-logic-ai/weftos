//! Session-scoped context graft layer (ADR-058 Phase 3).
//!
//! The agent loop never gets a parallel store; it gets a **query/graft layer**
//! over ExoChain's ECC indexes, scoped to one conversation. This module is the
//! kernel side of that layer:
//!
//! - **3.1 [`SessionView`]** — a session-scoped, *ephemeral* semantic index
//!   (a per-session HNSW). Because the index holds only this conversation's
//!   chunks, scoping is structural: a query can only return this session's
//!   chunks. The view is disposable and **rebuildable from the chain**
//!   (re-embed this session's events), so killing/rebuilding a loop costs only
//!   time — ExoChain remains the source of truth.
//! - **3.2 graft API** — query → candidate branches → COW-reference graft,
//!   keyed by chain sequence, large payloads as content-addressed blobs.
//! - **3.3 index population** — embed `agent.chat.turn` / tool-output events
//!   with the ADR-059 embedder, store f32 vectors keyed by chain sequence.
//!
//! ## This IS the ECC causal model
//!
//! Graft / prune / promote map onto the node lifecycle ([`NodeState`]):
//! `Speculative → Frontier → Committed` (collapse to trunk) or `→ Stale →
//! Pruned` (evicted off `main_line`). v1 is **single semantic index** per
//! conversation (ADR-058 phasing); fusion (causal + temporal + BVH) is v2 and
//! additive — every chunk is already keyed by chain sequence.
//!
//! The session view is deliberately separate from the durable global
//! [`CausalGraph`](crate::causal): it is the L2 "warm, this-run" tier. L2→L3
//! promotion onto the durable trunk is Phase 4.

use std::collections::HashSet;

use dashmap::DashMap;
use serde_json::json;
use tracing::warn;

use crate::artifact_store::{ArtifactStore, ArtifactType};
use crate::embedding::{EmbeddingError, EmbeddingProvider};
use crate::hnsw_service::{HnswService, HnswServiceConfig};

// Node lifecycle state ([`NodeState`]) + cross-substrate [`mirror_state`] live
// in a sibling file to keep this module under the 500-line cap.
#[path = "context_graft_state.rs"]
mod state;
pub use state::{NodeState, mirror_state};

/// Metadata for one chunk indexed in a [`SessionView`].
///
/// Keyed by `chain_seq` (the universal ExoChain key). `content_hash` is the
/// content-addressed identity used for dedup and provenance; large chunk text
/// is held as a `blob_hash` (RVF content-addressed blob), small text inline.
#[derive(Debug, Clone)]
pub struct ChunkMeta {
    /// ExoChain sequence this chunk originates from.
    pub chain_seq: u64,
    /// BLAKE3 hex of the chunk text — content-addressed identity.
    pub content_hash: String,
    /// Causal-model state of this chunk.
    pub state: NodeState,
    /// Originating event kind, e.g. `"agent.chat.turn"` / `"tool.output"`.
    pub kind: String,
    /// Content-addressed blob hash when the text was externalized (large).
    pub blob_hash: Option<String>,
    /// Inline chunk text when small enough to keep in the manifest.
    pub inline: Option<String>,
}

/// A hit from a scoped query: the matched chunk plus its similarity score.
#[derive(Debug, Clone)]
pub struct ScopedHit {
    /// ExoChain sequence of the matched chunk.
    pub chain_seq: u64,
    /// Cosine similarity (higher is closer).
    pub score: f32,
    /// Full chunk metadata (provenance + content reference).
    pub meta: ChunkMeta,
}

/// How a grafted item's content is carried into the working set.
///
/// The graft is **by COW reference** — the origin chain entry is never removed.
/// Small text rides inline; large text is referenced by its content-addressed
/// blob hash (load via [`ArtifactStore`](crate::artifact_store::ArtifactStore));
/// a chunk whose text was not retained in this view is a bare reference to be
/// recovered from the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraftContent {
    /// Inline chunk text (small).
    Inline(String),
    /// RVF content-addressed blob hash (large); load via the artifact store.
    Blob(String),
    /// Indexed but text not retained here; recover from the origin chain.
    Reference,
}

/// One item grafted into the working set (ADR-058 Phase 3.2).
///
/// Keyed by chain sequence and carrying a verifiable provenance backref
/// (`chain_seq` + `content_hash`): the agent can produce the witness chain for
/// any grafted context.
#[derive(Debug, Clone)]
pub struct GraftedItem {
    /// ExoChain sequence — the universal key / provenance backref.
    pub chain_seq: u64,
    /// BLAKE3 content identity — dedup key + provenance.
    pub content_hash: String,
    /// Originating event kind.
    pub kind: String,
    /// Causal-model state of the grafted node.
    pub state: NodeState,
    /// Similarity score that selected this item (higher is closer).
    pub score: f32,
    /// COW reference to the item's content.
    pub content: GraftContent,
}

/// Compute the content-addressed identity of a chunk's text.
pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// Resolve a chunk's content into a COW reference (blob hash > inline > bare ref).
fn graft_content(meta: &ChunkMeta) -> GraftContent {
    if let Some(ref h) = meta.blob_hash {
        GraftContent::Blob(h.clone())
    } else if let Some(ref t) = meta.inline {
        GraftContent::Inline(t.clone())
    } else {
        GraftContent::Reference
    }
}

/// A session-scoped, ephemeral semantic view over this conversation's chain
/// chunks (ADR-058 Phase 3.1).
///
/// Holds a per-session HNSW (so queries are structurally scoped) plus a
/// manifest of `chain_seq → [`ChunkMeta`]`. Disposable: drop it at session end;
/// rebuild it from the chain by re-inserting this session's events.
pub struct SessionView {
    session_id: String,
    dims: usize,
    index: HnswService,
    /// chain_seq → chunk metadata (this session's manifest).
    chunks: DashMap<u64, ChunkMeta>,
    /// chain_seq → number of times grafted/recalled (promotion signal, 4.2).
    ref_counts: DashMap<u64, u32>,
    /// Chunks explicitly marked "important to remember" (promotion signal, 4.2).
    important: DashMap<u64, ()>,
}

impl SessionView {
    /// Create an empty session view sized to `embedder`'s output dimension
    /// (ADR-059 Qwen3 = 512; Mock fallback = its own dim). Preferred
    /// constructor so the index and the embedder can never disagree on width.
    pub fn for_embedder(session_id: impl Into<String>, embedder: &dyn EmbeddingProvider) -> Self {
        Self::new(session_id, embedder.dimensions())
    }

    /// Create an empty session view whose ephemeral index expects `dims`-wide
    /// vectors (use the embedder's [`dimensions`](crate::embedding::EmbeddingProvider::dimensions);
    /// ADR-059 Qwen3 = 512, Mock fallback = its own dim).
    pub fn new(session_id: impl Into<String>, dims: usize) -> Self {
        let config = HnswServiceConfig {
            ef_search: 100,
            ef_construction: 200,
            default_dimensions: dims,
        };
        Self {
            session_id: session_id.into(),
            dims,
            index: HnswService::new(config),
            chunks: DashMap::new(),
            ref_counts: DashMap::new(),
            important: DashMap::new(),
        }
    }

    /// The conversation/session id this view is scoped to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Vector dimensionality of the ephemeral index.
    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Number of chunks indexed in this session view.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// `true` when no chunks have been indexed yet.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// The chain sequences indexed in this view (the rebuild manifest).
    pub fn chain_seqs(&self) -> Vec<u64> {
        let mut seqs: Vec<u64> = self.chunks.iter().map(|e| *e.key()).collect();
        seqs.sort_unstable();
        seqs
    }

    /// Fetch a chunk's metadata by chain sequence.
    pub fn chunk(&self, chain_seq: u64) -> Option<ChunkMeta> {
        self.chunks.get(&chain_seq).map(|e| e.value().clone())
    }

    /// Insert a pre-embedded chunk into the ephemeral index, keyed by chain
    /// sequence. Low-level building block: 3.3 wraps this with the embedder and
    /// blob externalization. Replaces any existing chunk for the same
    /// `chain_seq` (idempotent re-graft / rebuild).
    ///
    /// Returns `false` (and does not index) when `vector` does not match the
    /// view's dimensionality — callers must embed with the configured model.
    pub fn insert_vector(&self, vector: Vec<f32>, meta: ChunkMeta) -> bool {
        if vector.len() != self.dims {
            return false;
        }
        let chain_seq = meta.chain_seq;
        let metadata = json!({
            "chain_seq": chain_seq,
            "content_hash": meta.content_hash,
            "state": meta.state.as_str(),
            "kind": meta.kind,
            "session_id": self.session_id,
        });
        self.index.insert(chain_seq.to_string(), vector, metadata);
        self.chunks.insert(chain_seq, meta);
        true
    }

    /// Query the session-scoped index for the `top_k` nearest chunks.
    ///
    /// Because the index holds only this session's chunks, results are
    /// structurally scoped — a hit can only be from this conversation.
    pub fn query(&self, query: &[f32], top_k: usize) -> Vec<ScopedHit> {
        if query.len() != self.dims {
            return Vec::new();
        }
        self.index
            .search(query, top_k)
            .into_iter()
            .filter_map(|hit| {
                let chain_seq: u64 = hit.id.parse().ok()?;
                let meta = self.chunk(chain_seq)?;
                Some(ScopedHit {
                    chain_seq,
                    score: hit.score,
                    meta,
                })
            })
            .collect()
    }

    /// Graft the `top_k` most relevant chunks into the working set (ADR-058
    /// Phase 3.2).
    ///
    /// Scoped query → candidate branches → **COW-reference** graft: returns
    /// chain-sequence-keyed [`GraftedItem`]s with provenance backrefs
    /// (`chain_seq` + `content_hash`) and a content reference (inline / blob /
    /// bare). Candidates are **deduplicated by content hash** — identical tool
    /// outputs or repeated file reads collapse to one item (the highest-scoring
    /// occurrence). The origin is never removed; this only references it.
    pub fn graft(&self, query: &[f32], top_k: usize) -> Vec<GraftedItem> {
        if top_k == 0 {
            return Vec::new();
        }
        // Over-fetch so dedup-by-content does not starve the result below top_k.
        let raw = self.query(query, top_k.saturating_mul(2));
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<GraftedItem> = Vec::with_capacity(top_k);
        // `query` returns hits sorted by descending score, so the first time a
        // content hash is seen is its best-scoring occurrence.
        for hit in raw {
            if !seen.insert(hit.meta.content_hash.clone()) {
                continue; // duplicate content — already grafted at a higher score
            }
            // Recall is a promotion signal (4.2): count every graft.
            *self.ref_counts.entry(hit.chain_seq).or_insert(0) += 1;
            out.push(GraftedItem {
                chain_seq: hit.chain_seq,
                content_hash: hit.meta.content_hash.clone(),
                kind: hit.meta.kind.clone(),
                state: hit.meta.state,
                score: hit.score,
                content: graft_content(&hit.meta),
            });
            if out.len() == top_k {
                break;
            }
        }
        out
    }

    /// Embed and index one chain event into the session view (ADR-058 Phase
    /// 3.3).
    ///
    /// Embeds `text` with the ADR-059 `embedder` (store **f32**; Qwen3 = @512)
    /// and indexes the vector keyed by `chain_seq`. When `text` exceeds
    /// `inline_max` bytes and an [`ArtifactStore`] is supplied, the full text is
    /// written as an RVF **content-addressed blob** and the chunk holds only the
    /// blob hash (no chain bloat, free dedup); otherwise the text rides inline.
    /// The chunk enters as [`NodeState::Frontier`] (live in the window).
    ///
    /// Returns `Ok(false)` only if the embedder produced a vector whose width
    /// disagrees with the view (guarded against; use [`for_embedder`]).
    ///
    /// [`for_embedder`]: Self::for_embedder
    pub async fn index_chunk(
        &self,
        embedder: &dyn EmbeddingProvider,
        store: Option<&ArtifactStore>,
        chain_seq: u64,
        kind: impl Into<String>,
        text: &str,
        inline_max: usize,
    ) -> Result<bool, EmbeddingError> {
        // Default: embed the same string that is stored.
        self.index_chunk_with_embed(embedder, store, chain_seq, kind, text, text, inline_max)
            .await
    }

    /// Like [`index_chunk`](Self::index_chunk), but embeds `embed_input` while
    /// storing / hashing `text` (WEFT-640).
    ///
    /// Used when the passage lane is a **verbalized** atom (text +
    /// classification + voice cues) but the grafted payload must remain the
    /// original turn text for the prompt. Content hash is over `text` so
    /// dedup stays content-stable across verbalizer format bumps.
    #[allow(clippy::too_many_arguments)] // embed vs store text split needs distinct params
    pub async fn index_chunk_with_embed(
        &self,
        embedder: &dyn EmbeddingProvider,
        store: Option<&ArtifactStore>,
        chain_seq: u64,
        kind: impl Into<String>,
        text: &str,
        embed_input: &str,
        inline_max: usize,
    ) -> Result<bool, EmbeddingError> {
        let vector = embedder.embed(embed_input).await?;
        let hash = content_hash(text);

        // Externalize large payloads to a content-addressed blob; fall back to
        // inline if there is no store or the write fails (chunk stays usable).
        let (inline, blob_hash) = if text.len() > inline_max {
            match store {
                Some(s) => match s.store(text.as_bytes(), ArtifactType::Generic) {
                    Ok(h) => (None, Some(h)),
                    Err(e) => {
                        warn!(
                            chain_seq,
                            error = %e,
                            "context_graft: blob store failed; keeping chunk inline"
                        );
                        (Some(text.to_string()), None)
                    }
                },
                None => (Some(text.to_string()), None),
            }
        } else {
            (Some(text.to_string()), None)
        };

        let meta = ChunkMeta {
            chain_seq,
            content_hash: hash,
            state: NodeState::Frontier,
            kind: kind.into(),
            blob_hash,
            inline,
        };
        Ok(self.insert_vector(vector, meta))
    }

    /// Convenience: embed a free-text query with `embedder` and graft the
    /// `top_k` nearest chunks (ADR-058 Phase 3.2 + 3.3, query side).
    ///
    /// Uses [`EmbeddingProvider::embed_query`] (not plain `embed`) so asymmetric
    /// providers (e5 `query:` prefix, Qwen3 instruction) land queries in the
    /// correct half of the space (WEFT-640). Symmetric backends default
    /// `embed_query` → `embed`.
    pub async fn graft_text(
        &self,
        embedder: &dyn EmbeddingProvider,
        query_text: &str,
        top_k: usize,
    ) -> Result<Vec<GraftedItem>, EmbeddingError> {
        let query = embedder.embed_query(query_text).await?;
        Ok(self.graft(&query, top_k))
    }

    /// Set a chunk's causal-model [`NodeState`] unconditionally. Returns
    /// `false` if the chunk is unknown. Prefer [`transition`](Self::transition)
    /// on the per-turn path so illegal lifecycle jumps are rejected.
    pub fn set_state(&self, chain_seq: u64, state: NodeState) -> bool {
        match self.chunks.get_mut(&chain_seq) {
            Some(mut e) => {
                e.state = state;
                true
            }
            None => false,
        }
    }

    /// The current causal-model state of a chunk, if known.
    pub fn state(&self, chain_seq: u64) -> Option<NodeState> {
        self.chunks.get(&chain_seq).map(|e| e.value().state)
    }

    /// Mark a chunk [`NodeState::Speculative`] — above the wavefront, mutable,
    /// not yet hash-chained (ADR-062 D2). This is the per-turn entry state the
    /// kernel never assigned before P0.2. Returns `false` if unknown.
    pub fn set_speculative(&self, chain_seq: u64) -> bool {
        self.set_state(chain_seq, NodeState::Speculative)
    }

    /// Advance a chunk through a **validated** lifecycle transition
    /// ([`NodeState::can_transition_to`], ADR-062 D2). Returns `false` if the
    /// chunk is unknown or the transition is illegal (the chunk is left
    /// unchanged in that case).
    pub fn transition(&self, chain_seq: u64, next: NodeState) -> bool {
        match self.chunks.get_mut(&chain_seq) {
            Some(mut e) if e.state.can_transition_to(next) => {
                e.state = next;
                true
            }
            _ => false,
        }
    }

    /// Commit a chunk to the durable trunk ([`NodeState::Committed`]) — the
    /// Frontier→Committed step fired at EOU (ADR-062 D5). Validated; returns
    /// `false` if unknown or not currently `Frontier`.
    pub fn commit(&self, chain_seq: u64) -> bool {
        self.transition(chain_seq, NodeState::Committed)
    }

    /// Prune a chunk from the live window (ADR-058 Phase 4.1).
    ///
    /// Marks it [`NodeState::Stale`] but **keeps it in the index** — the origin
    /// stays on the chain and the chunk is re-graftable via retrieval. This is
    /// the application-level eviction that bounds L1 with no engine
    /// `--context-shift`. Returns `false` if the chunk is unknown.
    pub fn prune(&self, chain_seq: u64) -> bool {
        self.set_state(chain_seq, NodeState::Stale)
    }

    /// Re-graft a previously pruned chunk back into the live window
    /// ([`NodeState::Frontier`]). The chunk was never removed from the index, so
    /// this only flips its state. Returns `false` if unknown.
    pub fn regraft(&self, chain_seq: u64) -> bool {
        self.set_state(chain_seq, NodeState::Frontier)
    }

    /// Chain sequences currently live in the window ([`NodeState::Frontier`]),
    /// sorted ascending.
    pub fn live_seqs(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .chunks
            .iter()
            .filter(|e| e.value().state == NodeState::Frontier)
            .map(|e| *e.key())
            .collect();
        v.sort_unstable();
        v
    }

    /// Mark a chunk "important to remember" — an explicit promotion signal
    /// (ADR-058 Phase 4.2, e.g. from a `memory_store` tool call). Returns
    /// `false` if the chunk is unknown.
    pub fn mark_important(&self, chain_seq: u64) -> bool {
        if self.chunks.contains_key(&chain_seq) {
            self.important.insert(chain_seq, ());
            true
        } else {
            false
        }
    }

    /// Whether a chunk was explicitly marked important.
    pub fn is_important(&self, chain_seq: u64) -> bool {
        self.important.contains_key(&chain_seq)
    }

    /// How many times a chunk has been grafted/recalled (a promotion signal).
    pub fn ref_count(&self, chain_seq: u64) -> u32 {
        self.ref_counts
            .get(&chain_seq)
            .map(|e| *e.value())
            .unwrap_or(0)
    }
}

// ── Tests (no model required; split to a sibling file for the <500-line rule) ─

#[cfg(test)]
#[path = "context_graft_tests.rs"]
mod tests;
