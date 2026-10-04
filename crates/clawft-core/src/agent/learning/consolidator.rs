//! Phase 4 `MemoryConsolidator` — periodic distillation from conversation
//! turns into long-term memory (`MEMORY.md` / `HISTORY.md`).
//!
//! # Role
//!
//! Closes the system-architect boundary between
//! [`ConversationSink`](crate::agent::sink::ConversationSink) (per-turn
//! substrate / JSONL) and [`MemoryStore`](crate::agent::memory::MemoryStore)
//! (cross-conversation distilled facts). The two never share write paths;
//! this module is the only bridge (WEFT-347 / chat-agent-v1 Phase 4).
//!
//! # Cadence
//!
//! [`ConsolidationConfig`] supports two independent triggers (OR):
//! - **every K turns** — after `K` new turns since the last successful
//!   consolidation for that conversation;
//! - **every T minutes** — wall-clock interval since the last run.
//!
//! Callers drive the schedule: after each turn invoke
//! [`MemoryConsolidator::run_if_due`], and/or from a timer call
//! [`MemoryConsolidator::tick`]. There is no internal tokio task so the
//! module stays WASM-friendly and testable without a runtime clock.
//!
//! # Idempotency
//!
//! Distillation is a pure function of `(conv_id, turns)`:
//! [`distill_turns`] always emits the same markdown and fingerprint for
//! the same turn set. Before appending, the consolidator checks whether
//! `MEMORY.md` already contains the fingerprint marker
//! (`<!-- consolidator:fp=<hex> -->`). Re-running on the same turns is a
//! no-op write with [`ConsolidationOutcome::AlreadyPresent`].
//!
//! # Distillation
//!
//! Offline extractive heuristic (no LLM dependency). With
//! [`ConsolidationConfig::topic_merge`] the facts go into a topic bank with merge-or-insert
//! (WEFT-733, [`super::topics`]) instead of an appended block.
//!
//! Base distillation: offline extractive heuristic (no LLM dependency). Preference / fact
//! cues and short declarative user lines become bullet facts; tool turns
//! are ignored. A future ticket may swap in an LLM summarizer behind the
//! same pure-shape contract.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use clawft_platform::Platform;
use clawft_types::{ClawftError, Result};

use crate::agent::memory::MemoryStore;
use crate::agent::sink::{ConversationSink, Turn};

/// Default number of recent turns read when consolidating.
pub const DEFAULT_MAX_TURNS: usize = 32;
/// Default turn-count cadence (every K turns).
pub const DEFAULT_EVERY_K_TURNS: usize = 10;
/// Default wall-clock cadence in minutes.
pub const DEFAULT_EVERY_T_MINUTES: u64 = 30;
/// Cap on extracted fact bullets per consolidation block.
const MAX_FACTS: usize = 24;

/// Cadence and window knobs for [`MemoryConsolidator`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidationConfig {
    /// Maximum recent turns to load from the sink (`N` in the AC).
    pub max_turns: usize,
    /// Run after every `K` new turns since last consolidation.
    /// `None` disables the turn-count trigger.
    pub every_k_turns: Option<usize>,
    /// Run every `T` minutes of wall clock since last run.
    /// `None` disables the time trigger.
    pub every_t_minutes: Option<u64>,
    /// Master enable switch. When `false`, [`MemoryConsolidator::run_if_due`]
    /// and [`MemoryConsolidator::tick`] are no-ops; explicit
    /// [`MemoryConsolidator::consolidate`] still works for tests/tools.
    pub enabled: bool,
    /// Organize facts as topic nodes with merge-or-insert (WEFT-733, RMM prospective) instead
    /// of appending a block per consolidation. Default `false` (append, as before). See
    /// [`super::topics`].
    pub topic_merge: bool,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_TURNS,
            every_k_turns: Some(DEFAULT_EVERY_K_TURNS),
            every_t_minutes: Some(DEFAULT_EVERY_T_MINUTES),
            enabled: true,
            topic_merge: false,
        }
    }
}

impl ConsolidationConfig {
    /// Builder: only the turn-count cadence is active.
    pub fn every_k_turns(k: usize) -> Self {
        Self {
            every_k_turns: Some(k.max(1)),
            every_t_minutes: None,
            ..Self::default()
        }
    }

    /// Builder: only the wall-clock cadence is active.
    pub fn every_t_minutes(t: u64) -> Self {
        Self {
            every_k_turns: None,
            every_t_minutes: Some(t.max(1)),
            ..Self::default()
        }
    }

    /// Builder: turn on topic merge-or-insert (WEFT-733).
    pub fn with_topic_merge(mut self, on: bool) -> Self {
        self.topic_merge = on;
        self
    }

    /// Both cadences (OR). Zero / empty values are treated as disabled.
    pub fn with_cadence(every_k_turns: Option<usize>, every_t_minutes: Option<u64>) -> Self {
        Self {
            every_k_turns: every_k_turns.filter(|&k| k > 0),
            every_t_minutes: every_t_minutes.filter(|&t| t > 0),
            ..Self::default()
        }
    }
}

/// Pure distillation product of a turn window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistilledMemory {
    /// Conversation the turns belong to.
    pub conv_id: String,
    /// Stable SHA-256 hex of the canonical turn set (idempotency key).
    pub fingerprint: String,
    /// Number of input turns that contributed to the fingerprint.
    pub turn_count: usize,
    /// Extracted fact bullets (already trimmed, de-duplicated).
    pub facts: Vec<String>,
    /// Full markdown block ready to append to `MEMORY.md` (includes
    /// fingerprint markers). Empty when there are no facts.
    pub memory_block: String,
    /// One-line history summary for `HISTORY.md`.
    pub history_entry: String,
}

/// What a consolidate call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsolidationOutcome {
    /// New facts were written to `MEMORY.md` / `HISTORY.md`.
    Written,
    /// Distillation fingerprint already present — no write.
    AlreadyPresent,
    /// No extractable facts in the turn window.
    NoFacts,
    /// Conversation had zero turns.
    Empty,
    /// Cadence said "not yet" (`run_if_due` / `tick` only).
    SkippedNotDue,
    /// Config `enabled == false` (`run_if_due` / `tick` only).
    Disabled,
}

/// Result of one consolidate attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidationReport {
    pub conv_id: String,
    pub outcome: ConsolidationOutcome,
    /// Present whenever distillation ran (including AlreadyPresent / NoFacts).
    pub fingerprint: Option<String>,
    pub facts_written: usize,
    pub turns_read: usize,
}

/// Per-conversation cadence bookkeeping.
#[derive(Debug, Clone, Default)]
struct ConvState {
    /// Turn count observed at last successful consolidation attempt
    /// (Written, AlreadyPresent, or NoFacts).
    last_turn_count: usize,
    /// Wall-clock ms of that attempt. `None` until the first run.
    last_run_ms: Option<u64>,
    /// Fingerprint of the last non-empty distillation.
    last_fingerprint: Option<String>,
}

/// Periodic distillation bridge: `ConversationSink` → `MemoryStore`.
///
/// Holds no sink reference so it can be shared across backends
/// (in-memory tests, local file, substrate). Platform-generic via
/// [`MemoryStore`]'s filesystem trait.
pub struct MemoryConsolidator<P: Platform> {
    store: MemoryStore<P>,
    config: ConsolidationConfig,
    state: Mutex<HashMap<String, ConvState>>,
}

impl<P: Platform> MemoryConsolidator<P> {
    /// Create a consolidator writing into `store` with `config`.
    pub fn new(store: MemoryStore<P>, config: ConsolidationConfig) -> Self {
        Self {
            store,
            config,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Create with default cadence (every 10 turns OR every 30 minutes).
    pub fn with_defaults(store: MemoryStore<P>) -> Self {
        Self::new(store, ConsolidationConfig::default())
    }

    /// Current configuration.
    pub fn config(&self) -> &ConsolidationConfig {
        &self.config
    }

    /// Replace cadence/window knobs (keeps per-conv state).
    pub fn set_config(&mut self, config: ConsolidationConfig) {
        self.config = config;
    }

    /// Borrow the underlying memory store (diagnostics / tests).
    pub fn store(&self) -> &MemoryStore<P> {
        &self.store
    }

    /// Whether cadence says this conversation is due for consolidation.
    ///
    /// Due when **either** active trigger fires:
    /// - turn count advanced by ≥ `every_k_turns` since last run;
    /// - wall clock advanced by ≥ `every_t_minutes` since last run.
    ///
    /// First run (no state) is always due when at least one trigger is
    /// configured and there is at least one turn. When both triggers are
    /// `None`, returns `false` (caller must use [`Self::consolidate`]
    /// explicitly).
    pub fn is_due(&self, conv_id: &str, current_turn_count: usize, now_ms: u64) -> bool {
        if !self.config.enabled {
            return false;
        }
        if current_turn_count == 0 {
            return false;
        }
        let k = self.config.every_k_turns;
        let t = self.config.every_t_minutes;
        if k.is_none() && t.is_none() {
            return false;
        }

        let guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let st = guard.get(conv_id);

        let turn_due = match (k, st) {
            (Some(k), Some(s)) if s.last_run_ms.is_some() => {
                current_turn_count.saturating_sub(s.last_turn_count) >= k
            }
            (Some(_), _) => true, // first observation
            (None, _) => false,
        };

        let time_due = match (t, st) {
            (Some(mins), Some(s)) => match s.last_run_ms {
                Some(prev) => {
                    let elapsed = now_ms.saturating_sub(prev);
                    elapsed >= mins.saturating_mul(60_000)
                }
                None => true, // state row exists but never ran (shouldn't happen)
            },
            (Some(_), None) => true, // first observation
            (None, _) => false,
        };

        turn_due || time_due
    }

    /// Load recent turns from `sink` and consolidate if due.
    pub async fn run_if_due(
        &self,
        sink: &dyn ConversationSink,
        conv_id: &str,
        now_ms: u64,
    ) -> Result<ConsolidationReport> {
        if !self.config.enabled {
            return Ok(ConsolidationReport {
                conv_id: conv_id.to_string(),
                outcome: ConsolidationOutcome::Disabled,
                fingerprint: None,
                facts_written: 0,
                turns_read: 0,
            });
        }

        let turns = self.load_turns(sink, conv_id).await?;
        let turn_count = turns.len();
        if !self.is_due(conv_id, turn_count, now_ms) {
            return Ok(ConsolidationReport {
                conv_id: conv_id.to_string(),
                outcome: ConsolidationOutcome::SkippedNotDue,
                fingerprint: None,
                facts_written: 0,
                turns_read: turn_count,
            });
        }
        self.consolidate_loaded(conv_id, &turns, now_ms).await
    }

    /// Always consolidate (ignores cadence enable flag for the due check,
    /// but still respects empty/idempotent paths). Use from tests and
    /// end-of-conversation hooks.
    pub async fn consolidate(
        &self,
        sink: &dyn ConversationSink,
        conv_id: &str,
        now_ms: u64,
    ) -> Result<ConsolidationReport> {
        let turns = self.load_turns(sink, conv_id).await?;
        self.consolidate_loaded(conv_id, &turns, now_ms).await
    }

    /// Consolidate a pre-loaded turn window (no sink I/O). Useful for
    /// unit tests and for callers that already hold the history.
    pub async fn consolidate_turns(
        &self,
        conv_id: &str,
        turns: &[Turn],
        now_ms: u64,
    ) -> Result<ConsolidationReport> {
        self.consolidate_loaded(conv_id, turns, now_ms).await
    }

    /// Tick multiple conversations (timer / background worker entry).
    ///
    /// Returns one report per `conv_id`. Failures on one conv do not
    /// abort the others — they surface as `Err` elements so the caller
    /// can log and continue.
    pub async fn tick(
        &self,
        sink: &dyn ConversationSink,
        conv_ids: &[&str],
        now_ms: u64,
    ) -> Vec<Result<ConsolidationReport>> {
        let mut out = Vec::with_capacity(conv_ids.len());
        for id in conv_ids {
            out.push(self.run_if_due(sink, id, now_ms).await);
        }
        out
    }

    /// Last fingerprint recorded for `conv_id`, if any.
    pub fn last_fingerprint(&self, conv_id: &str) -> Option<String> {
        let guard = self.state.lock().ok()?;
        guard.get(conv_id).and_then(|s| s.last_fingerprint.clone())
    }

    // ── internals ────────────────────────────────────────────────────

    async fn load_turns(
        &self,
        sink: &dyn ConversationSink,
        conv_id: &str,
    ) -> Result<Vec<Turn>> {
        let window = self.config.max_turns;
        // Prefer the hydration window API (infallible, recent-N).
        let via_history = sink.history(conv_id, window).await;
        if !via_history.is_empty() {
            return Ok(via_history);
        }
        // Fallback for sinks that only implement load_history.
        match sink.load_history(conv_id).await {
            Ok(mut all) => {
                if window > 0 && all.len() > window {
                    all = all.split_off(all.len() - window);
                }
                Ok(all)
            }
            Err(e) => {
                warn!(conv_id, error = %e, "consolidator: load_history failed");
                Err(ClawftError::Provider {
                    message: format!("consolidator load_history({conv_id}): {e}"),
                })
            }
        }
    }

    async fn consolidate_loaded(
        &self,
        conv_id: &str,
        turns: &[Turn],
        now_ms: u64,
    ) -> Result<ConsolidationReport> {
        if turns.is_empty() {
            self.record_state(conv_id, 0, now_ms, None);
            return Ok(ConsolidationReport {
                conv_id: conv_id.to_string(),
                outcome: ConsolidationOutcome::Empty,
                fingerprint: None,
                facts_written: 0,
                turns_read: 0,
            });
        }

        let distilled = distill_turns(conv_id, turns);
        let fingerprint = distilled.fingerprint.clone();
        let turn_count = distilled.turn_count;

        if distilled.facts.is_empty() {
            self.record_state(conv_id, turn_count, now_ms, Some(fingerprint.clone()));
            return Ok(ConsolidationReport {
                conv_id: conv_id.to_string(),
                outcome: ConsolidationOutcome::NoFacts,
                fingerprint: Some(fingerprint),
                facts_written: 0,
                turns_read: turn_count,
            });
        }

        // Topic bank (WEFT-733): merge-or-insert each fact by topic key; idempotent because a
        // node whose text is unchanged is not rewritten.
        if self.config.topic_merge {
            let existing = self.store.read_long_term().await.unwrap_or_default();
            let (updated, stats) = super::topics::merge_into_memory(&existing, &distilled.facts, conv_id);
            self.record_state(conv_id, turn_count, now_ms, Some(fingerprint.clone()));
            if !stats.changed() {
                return Ok(ConsolidationReport {
                    conv_id: conv_id.to_string(),
                    outcome: ConsolidationOutcome::AlreadyPresent,
                    fingerprint: Some(fingerprint),
                    facts_written: 0,
                    turns_read: turn_count,
                });
            }
            self.store.write_long_term(&updated).await?;
            self.store.append_history(&distilled.history_entry).await?;
            info!(
                conv_id,
                merged = stats.merged,
                inserted = stats.inserted,
                unchanged = stats.unchanged,
                "consolidator: merged facts into the topic bank"
            );
            return Ok(ConsolidationReport {
                conv_id: conv_id.to_string(),
                outcome: ConsolidationOutcome::Written,
                fingerprint: Some(fingerprint),
                facts_written: stats.merged + stats.inserted,
                turns_read: turn_count,
            });
        }

        // Idempotency: skip if MEMORY.md already carries this fingerprint.
        let existing = self.store.read_long_term().await.unwrap_or_default();
        let marker = fingerprint_marker(&fingerprint);
        if existing.contains(&marker) {
            debug!(
                conv_id,
                fingerprint = %fingerprint,
                "consolidator: fingerprint already present — skip write"
            );
            self.record_state(conv_id, turn_count, now_ms, Some(fingerprint.clone()));
            return Ok(ConsolidationReport {
                conv_id: conv_id.to_string(),
                outcome: ConsolidationOutcome::AlreadyPresent,
                fingerprint: Some(fingerprint),
                facts_written: 0,
                turns_read: turn_count,
            });
        }

        self.store
            .append_long_term(&distilled.memory_block)
            .await?;
        self.store
            .append_history(&distilled.history_entry)
            .await?;

        info!(
            conv_id,
            facts = distilled.facts.len(),
            turns = turn_count,
            fingerprint = %fingerprint,
            "consolidator: distilled turns into MEMORY.md"
        );

        let facts_written = distilled.facts.len();
        self.record_state(conv_id, turn_count, now_ms, Some(fingerprint.clone()));
        Ok(ConsolidationReport {
            conv_id: conv_id.to_string(),
            outcome: ConsolidationOutcome::Written,
            fingerprint: Some(fingerprint),
            facts_written,
            turns_read: turn_count,
        })
    }

    fn record_state(
        &self,
        conv_id: &str,
        turn_count: usize,
        now_ms: u64,
        fingerprint: Option<String>,
    ) {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = guard.entry(conv_id.to_string()).or_default();
        entry.last_turn_count = turn_count;
        entry.last_run_ms = Some(now_ms);
        if fingerprint.is_some() {
            entry.last_fingerprint = fingerprint;
        }
    }
}

// ── pure distillation ────────────────────────────────────────────────

/// Distill a turn window into a deterministic [`DistilledMemory`].
///
/// Pure: same `(conv_id, turns)` → same fingerprint and markdown.
/// Tool-role turns are ignored for fact extraction but still included
/// in the fingerprint so the window identity is exact.
pub fn distill_turns(conv_id: &str, turns: &[Turn]) -> DistilledMemory {
    let fingerprint = fingerprint_turns(turns);
    let facts = extract_facts(turns);
    let turn_count = turns.len();

    let memory_block = if facts.is_empty() {
        String::new()
    } else {
        format_memory_block(conv_id, &fingerprint, turn_count, &facts)
    };

    let history_entry = format!(
        "consolidated conv={} turns={} facts={} fp={}",
        conv_id,
        turn_count,
        facts.len(),
        &fingerprint[..8.min(fingerprint.len())]
    );

    DistilledMemory {
        conv_id: conv_id.to_string(),
        fingerprint,
        turn_count,
        facts,
        memory_block,
        history_entry,
    }
}

/// SHA-256 hex of the canonical turn encoding.
fn fingerprint_turns(turns: &[Turn]) -> String {
    let mut hasher = Sha256::new();
    for t in turns {
        hasher.update(t.turn_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(t.role.as_bytes());
        hasher.update(b"\0");
        hasher.update(t.content.as_bytes());
        hasher.update(b"\0");
        hasher.update(t.ts_ms.to_le_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    hex_encode(&digest)
}

fn fingerprint_marker(fp: &str) -> String {
    format!("<!-- consolidator:fp={fp} -->")
}

fn format_memory_block(
    conv_id: &str,
    fingerprint: &str,
    turn_count: usize,
    facts: &[String],
) -> String {
    let mut out = String::new();
    out.push_str(&fingerprint_marker(fingerprint));
    out.push('\n');
    out.push_str(&format!(
        "### Distilled from conversation `{conv_id}` ({turn_count} turns)\n"
    ));
    for fact in facts {
        out.push_str("- ");
        out.push_str(fact);
        out.push('\n');
    }
    out.push_str("<!-- /consolidator -->");
    out
}

/// Extractive fact mining — deterministic, offline, no LLM.
fn extract_facts(turns: &[Turn]) -> Vec<String> {
    let mut facts = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for t in turns {
        let role = t.role.to_ascii_lowercase();
        if role == "tool" || role == "system" {
            continue;
        }
        let content = normalize_whitespace(&t.content);
        if content.is_empty() {
            continue;
        }

        // Prefer user preference / identity statements.
        if role == "user" {
            for line in content.lines() {
                let line = normalize_whitespace(line);
                if line.is_empty() {
                    continue;
                }
                if is_fact_like(&line) || is_preference_cue(&line) {
                    push_unique(&mut facts, &mut seen, &line);
                }
            }
            // Short declarative user messages also count as facts.
            if content.len() <= 200
                && !content.contains('?')
                && content.split_whitespace().count() >= 3
            {
                push_unique(&mut facts, &mut seen, &content);
            }
        } else if role == "assistant" {
            // Capture short assistant commitments / remembered facts.
            if is_fact_like(&content) || is_preference_cue(&content) {
                // Truncate long assistant replies to first sentence-ish chunk.
                let snippet = first_chunk(&content, 240);
                push_unique(&mut facts, &mut seen, &snippet);
            }
        }

        if facts.len() >= MAX_FACTS {
            break;
        }
    }

    facts
}

fn push_unique(facts: &mut Vec<String>, seen: &mut std::collections::HashSet<String>, line: &str) {
    if facts.len() >= MAX_FACTS {
        return;
    }
    let key = line.to_ascii_lowercase();
    if seen.insert(key) {
        facts.push(line.to_string());
    }
}

fn is_preference_cue(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    const CUES: &[&str] = &[
        "i prefer",
        "i'd rather",
        "i would rather",
        "please remember",
        "remember that",
        "from now on",
        "going forward",
        "my name is",
        "i am ",
        "i'm ",
        "call me ",
        "always ",
        "never ",
        "don't ",
        "do not ",
        "i work",
        "i live",
        "my timezone",
        "my email",
        "fact:",
        "note:",
    ];
    CUES.iter().any(|c| lower.contains(c))
}

fn is_fact_like(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    // Explicit fact markers or simple subject-copula patterns.
    if lower.starts_with("fact:") || lower.starts_with("note:") {
        return true;
    }
    // "X is Y" / "X are Y" with a short body.
    if s.len() <= 180
        && (lower.contains(" is ") || lower.contains(" are ") || lower.contains(" = "))
        && !s.contains('?')
    {
        return true;
    }
    false
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_chunk(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if let Some(idx) = trimmed.find(['.', '!', '\n']) {
        let end = (idx + 1).min(trimmed.len());
        let chunk = &trimmed[..end];
        if chunk.chars().count() <= max_chars {
            return normalize_whitespace(chunk);
        }
    }
    let mut out = String::new();
    for (i, ch) in trimmed.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    normalize_whitespace(&out)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// Current wall-clock milliseconds since UNIX epoch.
///
/// Convenience for callers that do not inject a clock; tests should
/// pass an explicit `now_ms` for determinism.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::sink::InMemorySink;
    use clawft_platform::NativePlatform;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_consol_{prefix}_{pid}_{id}"))
    }

    fn test_store(dir: &std::path::Path) -> MemoryStore<NativePlatform> {
        let platform = Arc::new(NativePlatform::new());
        MemoryStore::with_paths(dir.join("MEMORY.md"), dir.join("HISTORY.md"), platform)
    }

    fn turn(id: &str, role: &str, content: &str, ts: u64) -> Turn {
        Turn {
            turn_id: id.into(),
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            ts_ms: ts,
            voice_analysis: None,
            audio: None,
        }
    }

    fn sample_turns() -> Vec<Turn> {
        vec![
            turn("t1", "user", "My name is Ada and I prefer concise answers.", 1000),
            turn("t2", "assistant", "Got it — I'll keep replies short.", 1001),
            turn("t3", "user", "Please remember that the project codename is weftos.", 1002),
            turn("t4", "tool", r#"{"ok":true}"#, 1003),
            turn("t5", "assistant", "Noted: project codename is weftos.", 1004),
        ]
    }

    // ── pure distill ─────────────────────────────────────────────────

    #[test]
    fn distill_is_deterministic() {
        let turns = sample_turns();
        let a = distill_turns("conv-a", &turns);
        let b = distill_turns("conv-a", &turns);
        assert_eq!(a, b);
        assert!(!a.fingerprint.is_empty());
        assert_eq!(a.fingerprint.len(), 64); // sha256 hex
        assert!(!a.facts.is_empty());
        assert!(a.memory_block.contains(&a.fingerprint));
        assert!(a.memory_block.contains("<!-- consolidator:fp="));
        assert!(a.memory_block.contains("<!-- /consolidator -->"));
    }

    #[test]
    fn distill_fingerprint_changes_with_content() {
        let mut turns = sample_turns();
        let fp1 = distill_turns("c", &turns).fingerprint;
        turns[0].content = "My name is Grace.".into();
        let fp2 = distill_turns("c", &turns).fingerprint;
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn distill_extracts_preference_and_fact_cues() {
        let d = distill_turns("c", &sample_turns());
        let joined = d.facts.join(" | ").to_ascii_lowercase();
        assert!(
            joined.contains("ada") || joined.contains("concise") || joined.contains("weftos"),
            "expected preference/fact cues in {joined:?}"
        );
        // Tool payloads must not appear as facts.
        assert!(!joined.contains("{\"ok\":true}"));
    }

    #[test]
    fn distill_empty_turns_yields_empty_facts() {
        let d = distill_turns("c", &[]);
        assert!(d.facts.is_empty());
        assert!(d.memory_block.is_empty());
        // Empty window still has a stable fingerprint (of zero turns).
        assert_eq!(d.fingerprint, fingerprint_turns(&[]));
    }

    // ── cadence ──────────────────────────────────────────────────────

    #[test]
    fn cadence_every_k_turns() {
        let dir = temp_dir("cadence_k");
        let store = test_store(&dir);
        let c = MemoryConsolidator::new(store, ConsolidationConfig::every_k_turns(5));

        // First observation with turns is due.
        assert!(c.is_due("c1", 3, 1_000));
        // Simulate a run at turn_count=3.
        c.record_state("c1", 3, 1_000, None);
        // +2 turns → not due (need 5).
        assert!(!c.is_due("c1", 5, 1_001));
        // +5 turns since last → due.
        assert!(c.is_due("c1", 8, 1_002));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cadence_every_t_minutes() {
        let dir = temp_dir("cadence_t");
        let store = test_store(&dir);
        let c = MemoryConsolidator::new(store, ConsolidationConfig::every_t_minutes(10));

        assert!(c.is_due("c1", 1, 0));
        c.record_state("c1", 1, 0, None);
        // 5 minutes later — not due.
        assert!(!c.is_due("c1", 1, 5 * 60_000));
        // 10 minutes later — due.
        assert!(c.is_due("c1", 1, 10 * 60_000));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cadence_or_of_both_triggers() {
        let dir = temp_dir("cadence_or");
        let store = test_store(&dir);
        let cfg = ConsolidationConfig::with_cadence(Some(10), Some(30));
        let c = MemoryConsolidator::new(store, cfg);

        c.record_state("c1", 0, 0, None);
        // Time not elapsed, but 10 turns advanced → due via K.
        assert!(c.is_due("c1", 10, 1_000));

        c.record_state("c1", 10, 0, None);
        // Turns not advanced, but 30 min elapsed → due via T.
        assert!(c.is_due("c1", 10, 30 * 60_000));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_config_is_never_due() {
        let dir = temp_dir("disabled");
        let store = test_store(&dir);
        let mut cfg = ConsolidationConfig::every_k_turns(1);
        cfg.enabled = false;
        let c = MemoryConsolidator::new(store, cfg);
        assert!(!c.is_due("c1", 100, 999_999));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── end-to-end write + idempotency ───────────────────────────────

    #[tokio::test]
    async fn consolidate_writes_memory_and_history() {
        let dir = temp_dir("write");
        let store = test_store(&dir);
        let consolidator = MemoryConsolidator::new(
            store,
            ConsolidationConfig::every_k_turns(1),
        );

        let sink = InMemorySink::new();
        for t in sample_turns() {
            sink.append_turn("conv-1", t).await.unwrap();
        }

        let report = consolidator
            .consolidate(&sink, "conv-1", 5_000)
            .await
            .unwrap();
        assert_eq!(report.outcome, ConsolidationOutcome::Written);
        assert!(report.facts_written > 0);
        assert_eq!(report.turns_read, 5);

        let memory = consolidator.store().read_long_term().await.unwrap();
        assert!(memory.contains("<!-- consolidator:fp="));
        assert!(memory.contains("conv-1"));
        let history = consolidator.store().read_history().await.unwrap();
        assert!(history.contains("consolidated conv=conv-1"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn consolidate_is_idempotent_on_same_turns() {
        let dir = temp_dir("idem");
        let store = test_store(&dir);
        let consolidator =
            MemoryConsolidator::new(store, ConsolidationConfig::every_k_turns(1));

        let turns = sample_turns();
        let r1 = consolidator
            .consolidate_turns("c", &turns, 1_000)
            .await
            .unwrap();
        assert_eq!(r1.outcome, ConsolidationOutcome::Written);
        let mem_after_first = consolidator.store().read_long_term().await.unwrap();

        // Pure distill agrees.
        let d1 = distill_turns("c", &turns);
        let d2 = distill_turns("c", &turns);
        assert_eq!(d1.memory_block, d2.memory_block);
        assert_eq!(d1.fingerprint, r1.fingerprint.as_deref().unwrap());

        let r2 = consolidator
            .consolidate_turns("c", &turns, 2_000)
            .await
            .unwrap();
        assert_eq!(r2.outcome, ConsolidationOutcome::AlreadyPresent);
        assert_eq!(r2.fingerprint, r1.fingerprint);

        let mem_after_second = consolidator.store().read_long_term().await.unwrap();
        // No duplicate blocks.
        assert_eq!(mem_after_first, mem_after_second);
        let marker = fingerprint_marker(r1.fingerprint.as_deref().unwrap());
        assert_eq!(mem_after_second.matches(&marker).count(), 1);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn run_if_due_respects_turn_cadence() {
        let dir = temp_dir("due");
        let store = test_store(&dir);
        let consolidator =
            MemoryConsolidator::new(store, ConsolidationConfig::every_k_turns(3));

        let sink = InMemorySink::new();
        // Seed 2 turns — first run is due (no prior state) and writes.
        sink.append_turn("c", turn("a", "user", "My name is Ada.", 1))
            .await
            .unwrap();
        sink.append_turn("c", turn("b", "user", "I prefer dark mode.", 2))
            .await
            .unwrap();

        let r1 = consolidator.run_if_due(&sink, "c", 100).await.unwrap();
        assert_eq!(r1.outcome, ConsolidationOutcome::Written);

        // Still 2 turns — not enough new turns for K=3.
        let r2 = consolidator.run_if_due(&sink, "c", 200).await.unwrap();
        assert_eq!(r2.outcome, ConsolidationOutcome::SkippedNotDue);

        // Add 3 more → due again. Content change → new fingerprint → write.
        for (i, text) in ["Please remember the timezone is UTC.", "fact: repo is clawft", "note: ship 0.8"]
            .iter()
            .enumerate()
        {
            sink.append_turn(
                "c",
                turn(&format!("n{i}"), "user", text, 10 + i as u64),
            )
            .await
            .unwrap();
        }
        let r3 = consolidator.run_if_due(&sink, "c", 300).await.unwrap();
        assert_eq!(r3.outcome, ConsolidationOutcome::Written);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn tick_processes_multiple_convs() {
        let dir = temp_dir("tick");
        let store = test_store(&dir);
        let consolidator =
            MemoryConsolidator::new(store, ConsolidationConfig::every_k_turns(1));
        let sink = InMemorySink::new();
        sink.append_turn("a", turn("1", "user", "My name is Alice.", 1))
            .await
            .unwrap();
        sink.append_turn("b", turn("2", "user", "My name is Bob.", 2))
            .await
            .unwrap();

        let reports = consolidator.tick(&sink, &["a", "b"], 1_000).await;
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|r| r.as_ref().unwrap().outcome == ConsolidationOutcome::Written));

        let mem = consolidator.store().read_long_term().await.unwrap();
        assert!(mem.contains("Alice") || mem.to_ascii_lowercase().contains("alice"));
        assert!(mem.contains("Bob") || mem.to_ascii_lowercase().contains("bob"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn empty_conversation_reports_empty() {
        let dir = temp_dir("empty");
        let store = test_store(&dir);
        let consolidator = MemoryConsolidator::with_defaults(store);
        let sink = InMemorySink::new();
        let r = consolidator.consolidate(&sink, "missing", 1).await.unwrap();
        assert_eq!(r.outcome, ConsolidationOutcome::Empty);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn max_turns_window_is_honoured() {
        let dir = temp_dir("window");
        let store = test_store(&dir);
        let mut cfg = ConsolidationConfig::every_k_turns(1);
        cfg.max_turns = 2;
        let consolidator = MemoryConsolidator::new(store, cfg);
        let sink = InMemorySink::new();
        for i in 0..5 {
            sink.append_turn(
                "c",
                turn(
                    &format!("t{i}"),
                    "user",
                    &format!("Please remember fact number {i} is important."),
                    i as u64,
                ),
            )
            .await
            .unwrap();
        }
        let r = consolidator.consolidate(&sink, "c", 1).await.unwrap();
        assert_eq!(r.turns_read, 2);
        // Window is last 2 turns → facts about 3 and 4, not 0.
        let mem = consolidator.store().read_long_term().await.unwrap();
        assert!(mem.contains("number 4") || mem.contains("number 3"));
        assert!(!mem.contains("number 0"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    // ── topic bank (WEFT-733) ────────────────────────────────────────

    #[tokio::test]
    async fn topic_merge_replaces_a_corrected_fact_and_is_idempotent() {
        let dir = temp_dir("topics");
        let consolidator =
            MemoryConsolidator::new(test_store(&dir), ConsolidationConfig::default().with_topic_merge(true));
        let c1 = vec![turn("a1", "user", "I am allergic to penicillin and I live in Denver.", 1)];
        let c2 = vec![turn("b1", "user", "Correction: I am not allergic to penicillin.", 2)];

        let r1 = consolidator.consolidate_turns("conv-1", &c1, 10).await.unwrap();
        assert_eq!(r1.outcome, ConsolidationOutcome::Written);
        let r2 = consolidator.consolidate_turns("conv-2", &c2, 20).await.unwrap();
        assert_eq!(r2.outcome, ConsolidationOutcome::Written);

        let md = consolidator.store().read_long_term().await.unwrap();
        let (_, nodes) = super::super::topics::parse_bank(&md);
        let allergy: Vec<_> = nodes.iter().filter(|n| n.text.contains("penicillin")).collect();
        assert_eq!(allergy.len(), 1, "one allergy node, not two: {md}");
        assert!(allergy[0].text.contains("not allergic"), "{md}");
        assert!(!md.contains("consolidator:fp="), "topic mode does not append fingerprint blocks");

        // Same transcript again: nothing changes.
        let again = consolidator.consolidate_turns("conv-2", &c2, 30).await.unwrap();
        assert_eq!(again.outcome, ConsolidationOutcome::AlreadyPresent);
        assert_eq!(consolidator.store().read_long_term().await.unwrap(), md);
        let _ = std::fs::remove_dir_all(&dir);
    }

}
