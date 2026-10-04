//! Mid-stream gating analysis hooks for the 50 ms [`CognitiveTick`] path
//! (WEFT-617 eval → WEFT-714 Phase A + WEFT-715 Phase B).
//!
//! Pure **analysis → optional impulse emit**. Never owns control flow: floor
//! commit / barge-in cancel stay with [`TalkModeLoop`] / Talk-Mode. See
//! [`docs/research/weft-617-midstream-eval.md`](../../../../docs/research/weft-617-midstream-eval.md)
//! and [`.planning/ruv/integration/midstream-integration-plan.md`](../../../../.planning/ruv/integration/midstream-integration-plan.md).
//!
//! ## Phase map
//!
//! | Phase | What this module provides | Ticket |
//! |-------|---------------------------|--------|
//! | A (IU partials) | [`TemporalMidstreamAnalyzer`] (vendored DTW/edit) + [`PrefixMidstreamAnalyzer`] kill-fallback | **WEFT-714** |
//! | B (stall) | Enhanced stall (repeat + pattern-loop via `find_similar`) → [`MidstreamImpulseBridge`] | **WEFT-715** |
//! | C/D | window caps only | optional later |
//!
//! [`CognitiveTick`]: clawft_kernel::CognitiveTick
//! [`TalkModeLoop`]: clawft_kernel::TalkModeLoop

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use clawft_kernel::{ImpulseQueue, ImpulseType, StructureTag};

use crate::temporal_compare::{edit_distance_str, find_similar};

// ---------------------------------------------------------------------------
// Tick budget constraints (must stay inside CognitiveTick headroom)
// ---------------------------------------------------------------------------

/// Hard cap on token / frame windows fed to tick-hot analysis.
///
/// MidStream DTW/LCS is O(n·m); at n,m ≤ 64 cost is single-digit microseconds
/// and fits the default 50 ms tick (~15 ms soft compute budget). Larger windows
/// and O(n³) pattern miners are **forbidden** on this path (Weaver offline only).
pub const TICK_WINDOW_CAP: usize = 64;

/// Maximum haystack length for sliding-window similarity (Phase B/C).
pub const TICK_HAYSTACK_CAP: usize = 128;

/// Default max normalised character edit-distance for last-token refine
/// (relative to the longer of the two last tokens). Below this → Extend.
pub const DEFAULT_REFINE_RATIO: f64 = 0.45;

/// Default needle length for pattern-loop stall detection.
pub const DEFAULT_LOOP_NEEDLE: usize = 2;

/// Default normalised DTW threshold for pattern-loop hits.
pub const DEFAULT_LOOP_THRESHOLD: f64 = 0.15;

// ---------------------------------------------------------------------------
// Partial / IU restart-and-revise (Phase A)
// ---------------------------------------------------------------------------

/// Outcome of comparing a new STT partial against the last *stable* prefix.
///
/// Maps to speculative node lifecycle (synthesis §C.7): extend the in-flight
/// node, revise (revoke + rewrite), or mark ready to commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialDiffVerdict {
    /// Partial matches the stable prefix exactly (no graph write).
    Unchanged,
    /// Partial is a strict extension of the stable prefix (append / update).
    Extend,
    /// Partial diverges before the end of the stable prefix (restart-and-revise).
    Revise,
    /// Partial is empty or caller marks it final — ready for Frontier→Committed.
    Commit,
}

/// Stall / loop signal from a bounded recent-token window (Phase B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StallSignal {
    /// Short reason for logs / impulse payload (`"repeat"`, `"loop"`, …).
    pub reason: &'static str,
    /// How many consecutive identical tokens (or loop period hits) triggered it.
    pub run_length: usize,
}

/// Mid-stream analysis seam used by STT partial paths and tick-side analyzers.
///
/// Implementations must be pure and allocation-light on the hot path. They
/// **must not** call into TTS, the Talk-Mode controller, or mutate the causal
/// graph — only return verdicts / signals for the caller (or
/// [`MidstreamImpulseBridge`]) to act on.
pub trait MidstreamAnalyzer: Send + Sync {
    /// Diff a new streaming partial against the last stable prefix.
    fn diff_partial(&self, stable_prefix: &str, new_partial: &str) -> PartialDiffVerdict;

    /// Look for stall / repetition over a **capped** recent-token window.
    ///
    /// Callers must pass at most [`TICK_WINDOW_CAP`] tokens; implementations may
    /// ignore the rest or return `None` if the window is empty.
    fn detect_stall(&self, recent_tokens: &[String]) -> Option<StallSignal>;
}

// ---------------------------------------------------------------------------
// Token helpers
// ---------------------------------------------------------------------------

/// Whitespace-tokenize, dropping empty pieces.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn cap_window(tokens: &[String]) -> &[String] {
    if tokens.len() > TICK_WINDOW_CAP {
        &tokens[tokens.len() - TICK_WINDOW_CAP..]
    } else {
        tokens
    }
}

// ---------------------------------------------------------------------------
// PrefixMidstreamAnalyzer — kill-criterion fallback (common-prefix only)
// ---------------------------------------------------------------------------

/// Phase-A scaffold: common-prefix compare (no DTW).
///
/// Kill criterion (WEFT-617): if vendored DTW does not measurably reduce partial
/// churn / improve commit timing vs this helper, **keep this** and drop DTW.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrefixMidstreamAnalyzer {
    /// Consecutive identical tokens before a stall signal (default 4).
    pub stall_run: usize,
}

impl PrefixMidstreamAnalyzer {
    /// Default stall run length of 4 identical tokens.
    pub fn new() -> Self {
        Self { stall_run: 4 }
    }

    /// Custom stall sensitivity.
    pub fn with_stall_run(stall_run: usize) -> Self {
        Self {
            stall_run: stall_run.max(2),
        }
    }
}

impl MidstreamAnalyzer for PrefixMidstreamAnalyzer {
    fn diff_partial(&self, stable_prefix: &str, new_partial: &str) -> PartialDiffVerdict {
        prefix_diff_partial(stable_prefix, new_partial)
    }

    fn detect_stall(&self, recent_tokens: &[String]) -> Option<StallSignal> {
        detect_repeat_stall(recent_tokens, self.stall_run)
    }
}

fn prefix_diff_partial(stable_prefix: &str, new_partial: &str) -> PartialDiffVerdict {
    let stable = stable_prefix.trim();
    let partial = new_partial.trim();

    if partial.is_empty() {
        return if stable.is_empty() {
            PartialDiffVerdict::Unchanged
        } else {
            // Empty partial after content often means endpoint / revoke path.
            PartialDiffVerdict::Commit
        };
    }
    if stable.is_empty() {
        return PartialDiffVerdict::Extend;
    }
    if partial == stable {
        return PartialDiffVerdict::Unchanged;
    }
    if let Some(rest) = partial.strip_prefix(stable) {
        // Extension only when new material appears (ignore pure whitespace grow).
        return if rest.trim().is_empty() {
            PartialDiffVerdict::Unchanged
        } else {
            PartialDiffVerdict::Extend
        };
    }
    // Diverged: common prefix shorter than stable → restart-and-revise.
    PartialDiffVerdict::Revise
}

fn detect_repeat_stall(recent_tokens: &[String], stall_run: usize) -> Option<StallSignal> {
    if recent_tokens.is_empty() {
        return None;
    }
    let window = cap_window(recent_tokens);
    let last = window.last()?.as_str();
    if last.is_empty() {
        return None;
    }
    let mut run = 0usize;
    for t in window.iter().rev() {
        if t.as_str() == last {
            run += 1;
        } else {
            break;
        }
    }
    if run >= stall_run {
        Some(StallSignal {
            reason: "repeat",
            run_length: run,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// TemporalMidstreamAnalyzer — WEFT-714/715 (vendored DP)
// ---------------------------------------------------------------------------

/// IU prefix-diff + stall analyzer backed by vendored temporal-compare.
///
/// ## Partial diff (Phase A)
///
/// Beats [`PrefixMidstreamAnalyzer`] on Parakeet partial churn where the *last
/// token* is refined in place (`"runnin"` → `"running"`) while earlier tokens
/// stay stable: common-prefix compare returns `Revise`; char-level edit-distance
/// on the last token returns `Extend`. Early divergence still yields `Revise`.
///
/// ## Stall (Phase B)
///
/// 1. Consecutive identical tokens (`"repeat"`) — same as prefix scaffold.
/// 2. Sliding-window DTW loop via [`find_similar`] on a short needle
///    (`"loop"`) when a multi-token motif repeats in the capped window.
#[derive(Debug, Clone, Copy)]
pub struct TemporalMidstreamAnalyzer {
    /// Consecutive identical tokens before a stall signal (default 4).
    pub stall_run: usize,
    /// Max char-edit / max(len) for last-token refine → Extend.
    pub refine_ratio: f64,
    /// Needle length for pattern-loop detection (default 2).
    pub loop_needle: usize,
    /// Normalised DTW threshold for loop hits (default 0.15).
    pub loop_threshold: f64,
    /// Minimum number of non-overlapping loop hits (default 2).
    pub loop_min_hits: usize,
}

impl Default for TemporalMidstreamAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl TemporalMidstreamAnalyzer {
    /// Defaults tuned for STT partials + 50 ms tick windows.
    pub fn new() -> Self {
        Self {
            stall_run: 4,
            refine_ratio: DEFAULT_REFINE_RATIO,
            loop_needle: DEFAULT_LOOP_NEEDLE,
            loop_threshold: DEFAULT_LOOP_THRESHOLD,
            loop_min_hits: 2,
        }
    }

    /// Custom stall run length.
    pub fn with_stall_run(mut self, stall_run: usize) -> Self {
        self.stall_run = stall_run.max(2);
        self
    }

    /// Custom last-token refine ratio (0.0–1.0).
    pub fn with_refine_ratio(mut self, ratio: f64) -> Self {
        self.refine_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// Custom loop needle length (≥ 2).
    pub fn with_loop_needle(mut self, n: usize) -> Self {
        self.loop_needle = n.max(2);
        self
    }
}

impl MidstreamAnalyzer for TemporalMidstreamAnalyzer {
    fn diff_partial(&self, stable_prefix: &str, new_partial: &str) -> PartialDiffVerdict {
        let stable = stable_prefix.trim();
        let partial = new_partial.trim();

        // Fast path: empty / exact / pure prefix — identical to scaffold.
        if partial.is_empty() {
            return if stable.is_empty() {
                PartialDiffVerdict::Unchanged
            } else {
                PartialDiffVerdict::Commit
            };
        }
        if stable.is_empty() {
            return PartialDiffVerdict::Extend;
        }
        if partial == stable {
            return PartialDiffVerdict::Unchanged;
        }
        if let Some(rest) = partial.strip_prefix(stable) {
            return if rest.trim().is_empty() {
                PartialDiffVerdict::Unchanged
            } else {
                PartialDiffVerdict::Extend
            };
        }

        // Temporal path: token-level alignment + last-token refine.
        let st = tokenize(stable);
        let pt = tokenize(partial);
        if st.is_empty() {
            return PartialDiffVerdict::Extend;
        }
        if pt.is_empty() {
            return PartialDiffVerdict::Commit;
        }

        // Longest common token prefix.
        let mut common = 0usize;
        let lim = st.len().min(pt.len());
        while common < lim && st[common].eq_ignore_ascii_case(&pt[common]) {
            common += 1;
        }

        // All stable tokens matched and partial has more (or equal after refine).
        if common == st.len() {
            return if pt.len() > st.len() {
                PartialDiffVerdict::Extend
            } else {
                PartialDiffVerdict::Unchanged
            };
        }

        // Only the *last* stable token is open for refine, and earlier tokens match.
        if common + 1 == st.len() && common < pt.len() {
            let old = &st[common];
            let new = &pt[common];
            if tokens_are_refine(old, new, self.refine_ratio) {
                // Remaining partial after the refined token = pure extension.
                return PartialDiffVerdict::Extend;
            }
        }

        // Character-level fallback on the whole strings when tokenisation is
        // noisy (no spaces mid-word) but edit distance is tiny.
        let ed = edit_distance_str(stable, partial);
        let max_len = stable.chars().count().max(partial.chars().count()).max(1);
        let ratio = ed as f64 / max_len as f64;
        if ratio <= self.refine_ratio && partial.len() >= stable.len() {
            return PartialDiffVerdict::Extend;
        }

        PartialDiffVerdict::Revise
    }

    fn detect_stall(&self, recent_tokens: &[String]) -> Option<StallSignal> {
        // 1) Consecutive identical tokens.
        if let Some(s) = detect_repeat_stall(recent_tokens, self.stall_run) {
            return Some(s);
        }

        // 2) Pattern loop via sliding-window DTW (find_similar).
        let window = cap_window(recent_tokens);
        if window.len() < self.loop_needle * 2 {
            return None;
        }
        let needle_len = self.loop_needle.min(window.len() / 2);
        if needle_len < 2 {
            return None;
        }
        // Use the last needle_len tokens as the motif.
        let needle = &window[window.len() - needle_len..];
        // Haystack is everything before the final needle occurrence.
        let hay_end = window.len() - needle_len;
        if hay_end < needle_len {
            return None;
        }
        let haystack = &window[..hay_end];
        let hits = find_similar(haystack, needle, self.loop_threshold);
        // Count hits that do not overlap the tail (all are in haystack by construction).
        if hits.len() >= self.loop_min_hits.saturating_sub(1) {
            // min_hits includes the tail occurrence; need min_hits-1 earlier hits.
            // If loop_min_hits == 2, one earlier hit is enough.
            let earlier = hits.len();
            let total = earlier + 1; // + the tail needle itself
            if total >= self.loop_min_hits {
                return Some(StallSignal {
                    reason: "loop",
                    run_length: total,
                });
            }
        }
        // Also: exact multi-token repeat without DTW (cheap).
        // e.g. [a,b,a,b] with needle_len=2
        if window.len() >= needle_len * self.loop_min_hits {
            let tail = &window[window.len() - needle_len..];
            let mut periods = 1usize;
            let mut pos = window.len() - needle_len;
            while pos >= needle_len {
                let prev = &window[pos - needle_len..pos];
                if prev == tail {
                    periods += 1;
                    pos -= needle_len;
                } else {
                    break;
                }
            }
            if periods >= self.loop_min_hits {
                return Some(StallSignal {
                    reason: "loop",
                    run_length: periods,
                });
            }
        }

        None
    }
}

/// True when `new` is a refinement of `old` (case-insensitive char edit).
fn tokens_are_refine(old: &str, new: &str, refine_ratio: f64) -> bool {
    if old.eq_ignore_ascii_case(new) {
        return true;
    }
    let a = old.to_ascii_lowercase();
    let b = new.to_ascii_lowercase();
    let ed = edit_distance_str(&a, &b);
    let max_len = a.chars().count().max(b.chars().count()).max(1);
    (ed as f64 / max_len as f64) <= refine_ratio
}

// ---------------------------------------------------------------------------
// Impulse bridge (Phase B) — analysis result → ImpulseQueue only
// ---------------------------------------------------------------------------

/// Emits [`ImpulseType::CoherenceAlert`] when a [`MidstreamAnalyzer`] reports a
/// stall. Does not interpret floor policy; the loop reads the impulse on the
/// next CognitiveTick drain.
pub struct MidstreamImpulseBridge<A: MidstreamAnalyzer> {
    analyzer: A,
    impulses: Arc<ImpulseQueue>,
    source: [u8; 32],
}

impl<A: MidstreamAnalyzer> MidstreamImpulseBridge<A> {
    /// Bind an analyzer to a forest / session impulse queue.
    pub fn new(analyzer: A, impulses: Arc<ImpulseQueue>, source_node: [u8; 32]) -> Self {
        Self {
            analyzer,
            impulses,
            source: source_node,
        }
    }

    /// Shared analyzer (for partial-diff without emitting).
    pub fn analyzer(&self) -> &A {
        &self.analyzer
    }

    /// Diff helpers pass-through (no impulse).
    pub fn diff_partial(&self, stable_prefix: &str, new_partial: &str) -> PartialDiffVerdict {
        self.analyzer.diff_partial(stable_prefix, new_partial)
    }

    /// Run stall detection; on hit, emit `CoherenceAlert` and return the signal.
    ///
    /// `hlc` is the hybrid-logical timestamp for queue ordering (caller supplies
    /// monotone clock, same pattern as [`crate::KernelImpulseSink`]).
    pub fn check_stall_and_emit(
        &self,
        recent_tokens: &[String],
        hlc: u64,
    ) -> Option<StallSignal> {
        let signal = self.analyzer.detect_stall(recent_tokens)?;
        let tag = StructureTag::CausalGraph.as_u8();
        let payload = serde_json::json!({
            "source": "midstream",
            "reason": signal.reason,
            "run_length": signal.run_length,
            "weft": "WEFT-715",
        });
        self.impulses.emit(
            tag,
            self.source,
            tag,
            ImpulseType::CoherenceAlert,
            payload,
            hlc,
        );
        Some(signal)
    }
}

// ---------------------------------------------------------------------------
// Token ring for tick-side stall (TalkForest integration)
// ---------------------------------------------------------------------------

/// Bounded ring of recent tokens for Phase-B stall detection on the tick path.
#[derive(Debug)]
pub struct MidstreamTokenRing {
    buf: VecDeque<String>,
    cap: usize,
}

impl MidstreamTokenRing {
    /// New ring capped at `cap` (clamped to [`TICK_HAYSTACK_CAP`]).
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap.min(TICK_HAYSTACK_CAP)),
            cap: cap.clamp(4, TICK_HAYSTACK_CAP),
        }
    }

    /// Push whitespace-tokenized text, dropping oldest when over capacity.
    pub fn push_text(&mut self, text: &str) {
        for t in tokenize(text) {
            if self.buf.len() >= self.cap {
                self.buf.pop_front();
            }
            self.buf.push_back(t);
        }
    }

    /// Replace the ring with tokens from a full partial transcript
    /// (STT partials are cumulative best-so-far, not deltas).
    pub fn set_from_partial(&mut self, text: &str) {
        self.buf.clear();
        for t in tokenize(text) {
            if self.buf.len() >= self.cap {
                self.buf.pop_front();
            }
            self.buf.push_back(t);
        }
    }

    /// Snapshot as a `Vec` for analyzer calls.
    pub fn tokens(&self) -> Vec<String> {
        self.buf.iter().cloned().collect()
    }

    /// Current length.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Empty?
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Clear.
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

impl Default for MidstreamTokenRing {
    fn default() -> Self {
        Self::new(TICK_WINDOW_CAP)
    }
}

/// Thread-safe midstream stall helper for the TalkForest / CognitiveTick path.
///
/// Holds a [`TemporalMidstreamAnalyzer`], a token ring, and the impulse bridge.
/// Callers (LoopObserver partials, explicit tick hooks) feed tokens; on stall
/// a `CoherenceAlert` is emitted for the Talk-Mode loop to drain.
pub struct MidstreamTickGuard {
    bridge: MidstreamImpulseBridge<TemporalMidstreamAnalyzer>,
    ring: Mutex<MidstreamTokenRing>,
    /// Last stable prefix for IU diff (optional bookkeeping).
    stable_prefix: Mutex<String>,
}

impl MidstreamTickGuard {
    /// Bind to a forest impulse queue.
    pub fn new(impulses: Arc<ImpulseQueue>, source: [u8; 32]) -> Self {
        Self {
            bridge: MidstreamImpulseBridge::new(TemporalMidstreamAnalyzer::new(), impulses, source),
            ring: Mutex::new(MidstreamTokenRing::default()),
            stable_prefix: Mutex::new(String::new()),
        }
    }

    /// Custom analyzer.
    pub fn with_analyzer(
        analyzer: TemporalMidstreamAnalyzer,
        impulses: Arc<ImpulseQueue>,
        source: [u8; 32],
    ) -> Self {
        Self {
            bridge: MidstreamImpulseBridge::new(analyzer, impulses, source),
            ring: Mutex::new(MidstreamTokenRing::default()),
            stable_prefix: Mutex::new(String::new()),
        }
    }

    /// Access the bridge (diff + emit).
    pub fn bridge(&self) -> &MidstreamImpulseBridge<TemporalMidstreamAnalyzer> {
        &self.bridge
    }

    /// Ingest a cumulative STT partial: update the ring and optionally emit stall.
    pub fn on_partial(&self, text: &str, hlc: u64) -> Option<StallSignal> {
        let mut ring = self.ring.lock().expect("midstream ring poisoned");
        ring.set_from_partial(text);
        let tokens = ring.tokens();
        drop(ring);
        self.bridge.check_stall_and_emit(&tokens, hlc)
    }

    /// Diff `text` against the stored stable prefix (no impulse).
    pub fn diff_against_stable(&self, text: &str) -> PartialDiffVerdict {
        let stable = self
            .stable_prefix
            .lock()
            .expect("stable prefix poisoned")
            .clone();
        self.bridge.diff_partial(&stable, text)
    }

    /// Commit a new stable prefix after IU lock-in.
    pub fn set_stable_prefix(&self, text: &str) {
        *self
            .stable_prefix
            .lock()
            .expect("stable prefix poisoned") = text.to_string();
    }

    /// Run stall detection on the current ring (CognitiveTick hook).
    pub fn on_tick(&self, hlc: u64) -> Option<StallSignal> {
        let tokens = self.ring.lock().expect("midstream ring poisoned").tokens();
        self.bridge.check_stall_and_emit(&tokens, hlc)
    }

    /// Snapshot ring length (tests).
    pub fn ring_len(&self) -> usize {
        self.ring.lock().expect("midstream ring poisoned").len()
    }

    /// Clear ring + stable prefix.
    pub fn reset(&self) {
        self.ring.lock().expect("midstream ring poisoned").clear();
        self.stable_prefix
            .lock()
            .expect("stable prefix poisoned")
            .clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_diff_extend_revise_unchanged_commit() {
        let a = PrefixMidstreamAnalyzer::new();
        assert_eq!(a.diff_partial("", "hello"), PartialDiffVerdict::Extend);
        assert_eq!(
            a.diff_partial("hello", "hello"),
            PartialDiffVerdict::Unchanged
        );
        assert_eq!(
            a.diff_partial("hello", "hello world"),
            PartialDiffVerdict::Extend
        );
        assert_eq!(
            a.diff_partial("hello world", "hello there"),
            PartialDiffVerdict::Revise
        );
        assert_eq!(a.diff_partial("hello", ""), PartialDiffVerdict::Commit);
        assert_eq!(a.diff_partial("", ""), PartialDiffVerdict::Unchanged);
    }

    #[test]
    fn temporal_beats_prefix_on_last_token_refine() {
        // Parakeet-style partial churn: last word *substituted* mid-stream
        // (not pure char growth — "runnin"→"running" still strip_prefix-matches).
        let prefix = PrefixMidstreamAnalyzer::new();
        let temporal = TemporalMidstreamAnalyzer::new();

        let stable = "hello runing"; // missing 'n'
        let partial = "hello running";

        // Common-prefix strip fails → Revise.
        assert_eq!(
            prefix.diff_partial(stable, partial),
            PartialDiffVerdict::Revise
        );
        // Temporal last-token refine (char edit) → Extend (no full IU restart).
        assert_eq!(
            temporal.diff_partial(stable, partial),
            PartialDiffVerdict::Extend
        );

        // Another STT correction shape: early tokens stable, last token fixed.
        assert_eq!(
            prefix.diff_partial("weather is clowdy", "weather is cloudy"),
            PartialDiffVerdict::Revise
        );
        assert_eq!(
            temporal.diff_partial("weather is clowdy", "weather is cloudy"),
            PartialDiffVerdict::Extend
        );
    }

    #[test]
    fn temporal_still_revises_on_early_divergence() {
        let a = TemporalMidstreamAnalyzer::new();
        assert_eq!(
            a.diff_partial("hello world", "hello there"),
            PartialDiffVerdict::Revise
        );
        assert_eq!(
            a.diff_partial("the cat sat", "a dog ran"),
            PartialDiffVerdict::Revise
        );
    }

    #[test]
    fn temporal_matches_prefix_on_clean_extend() {
        let a = TemporalMidstreamAnalyzer::new();
        assert_eq!(
            a.diff_partial("hello", "hello world"),
            PartialDiffVerdict::Extend
        );
        assert_eq!(
            a.diff_partial("hello", "hello"),
            PartialDiffVerdict::Unchanged
        );
        assert_eq!(a.diff_partial("hello", ""), PartialDiffVerdict::Commit);
    }

    #[test]
    fn stall_detects_repeated_tokens_within_cap() {
        let a = PrefixMidstreamAnalyzer::with_stall_run(3);
        let tokens: Vec<String> = ["a", "b", "x", "x", "x"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let s = a.detect_stall(&tokens).expect("stall");
        assert_eq!(s.reason, "repeat");
        assert_eq!(s.run_length, 3);
        assert!(a.detect_stall(&["a".into(), "b".into()]).is_none());
    }

    #[test]
    fn temporal_stall_detects_pattern_loop() {
        let a = TemporalMidstreamAnalyzer::new()
            .with_stall_run(10) // disable simple-repeat path
            .with_loop_needle(2);
        // "I mean I mean I mean" → needle ["I","mean"] repeats
        let tokens: Vec<String> = ["I", "mean", "I", "mean", "I", "mean"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let s = a.detect_stall(&tokens).expect("loop stall");
        assert_eq!(s.reason, "loop");
        assert!(s.run_length >= 2);
    }

    #[test]
    fn stall_respects_window_cap_tail() {
        let a = PrefixMidstreamAnalyzer::with_stall_run(3);
        let mut tokens: Vec<String> = (0..TICK_WINDOW_CAP + 10)
            .map(|i| format!("t{i}"))
            .collect();
        let n = tokens.len();
        tokens[n - 3] = "loop".into();
        tokens[n - 2] = "loop".into();
        tokens[n - 1] = "loop".into();
        assert!(a.detect_stall(&tokens).is_some());
    }

    #[test]
    fn bridge_emits_coherence_alert_on_stall() {
        let q = Arc::new(ImpulseQueue::new());
        let bridge = MidstreamImpulseBridge::new(
            PrefixMidstreamAnalyzer::with_stall_run(3),
            q.clone(),
            [7u8; 32],
        );
        let tokens: Vec<String> = ["x", "x", "x"].into_iter().map(str::to_string).collect();
        let hit = bridge.check_stall_and_emit(&tokens, 42);
        assert!(hit.is_some());
        let drained = q.drain_ready();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].impulse_type, ImpulseType::CoherenceAlert);
        assert_eq!(drained[0].hlc_timestamp, 42);
        assert_eq!(drained[0].payload["source"], "midstream");
        assert_eq!(drained[0].payload["reason"], "repeat");
        assert_eq!(drained[0].payload["weft"], "WEFT-715");
    }

    #[test]
    fn bridge_silent_when_no_stall() {
        let q = Arc::new(ImpulseQueue::new());
        let bridge =
            MidstreamImpulseBridge::new(PrefixMidstreamAnalyzer::new(), q.clone(), [0u8; 32]);
        assert!(bridge
            .check_stall_and_emit(&["one".into(), "two".into()], 1)
            .is_none());
        assert!(q.drain_ready().is_empty());
    }

    #[test]
    fn tick_guard_partial_emits_on_repeat() {
        let q = Arc::new(ImpulseQueue::new());
        let guard = MidstreamTickGuard::with_analyzer(
            TemporalMidstreamAnalyzer::new().with_stall_run(3),
            q.clone(),
            [1u8; 32],
        );
        // Cumulative partials that end in repeats.
        assert!(guard.on_partial("hello", 1).is_none());
        assert!(guard.on_partial("x x x", 2).is_some());
        let drained = q.drain_ready();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].impulse_type, ImpulseType::CoherenceAlert);
    }

    #[test]
    fn tick_guard_diff_and_stable_prefix() {
        let q = Arc::new(ImpulseQueue::new());
        let guard = MidstreamTickGuard::new(q, [0u8; 32]);
        guard.set_stable_prefix("hello runing");
        assert_eq!(
            guard.diff_against_stable("hello running"),
            PartialDiffVerdict::Extend
        );
    }

    #[test]
    fn window_caps_are_tick_safe_constants() {
        const {
            assert!(TICK_WINDOW_CAP <= 64);
            assert!(TICK_HAYSTACK_CAP <= 256);
            assert!(TICK_WINDOW_CAP * TICK_WINDOW_CAP < 10_000);
        }
    }

    #[test]
    fn tokenize_splits_whitespace() {
        assert_eq!(tokenize("  a  b\tc "), vec!["a", "b", "c"]);
        assert!(tokenize("   ").is_empty());
    }
}
