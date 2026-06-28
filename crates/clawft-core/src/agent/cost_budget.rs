//! Per-conversation cost circuit-breaker (WEFT-322 / agent-core-v1.1).
//!
//! Implements the per-conversation cap requested in chat-agent-v1 §18:
//!
//! > "cost budget is per-LLM-call, not per-conversation: a confused
//! > loop on user permission can burn the daily budget in one turn."
//!
//! The agent loop checks [`ConversationBudget::check_can_call`] BEFORE
//! issuing each LLM call. On trip, the conversation is marked
//! `circuit_open` in the underlying [`BudgetStore`]; subsequent calls
//! fail-fast with [`ClawftError::ConversationBudgetExceeded`] until
//! [`ConversationBudget::reset`] is invoked (`agent.chat.reset_budget`
//! daemon RPC).
//!
//! ## Layering
//!
//! - [`CostBudgetConfig`] in `clawft-types::config` carries the caps.
//! - [`BudgetUsage`] is the accumulator (one record per `conv_id`).
//! - [`BudgetStore`] is the persistence seam — [`InMemoryBudgetStore`]
//!   for tests/CLI, substrate-backed impl in `clawft-service-agent`
//!   (writes `derived/chat/<conv_id>/budget.json` per item 4).
//! - [`ConversationBudget`] wraps a config + store and is what the
//!   loop holds (`AgentLoop::with_cost_budget`).
//!
//! State that survives daemon restart lives in the [`BudgetStore`];
//! the trait shape is sync because the in-memory impl needs no I/O
//! and the substrate impl wraps the kernel's already-sync publish.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use clawft_types::config::CostBudgetConfig;
use clawft_types::error::ClawftError;

/// Cumulative usage accumulator for one conversation.
///
/// Written to substrate at `derived/chat/<conv_id>/budget.json` so the
/// circuit-state survives daemon restart (per the WEFT-322 spec
/// item 4). The fields use the `clawft_types::Usage` naming convention
/// so the wire shape lines up with the existing per-call accounting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetUsage {
    /// Cumulative input tokens across every LLM call on this conv.
    #[serde(default)]
    pub input_tokens: u64,
    /// Cumulative output tokens across every LLM call on this conv.
    #[serde(default)]
    pub output_tokens: u64,
    /// Cumulative USD spend (sum of per-call cost estimates).
    #[serde(default)]
    pub usd: f64,
    /// Number of LLM round-trips inside `run_tool_loop`, summed across
    /// every `handle_turn` for this conv.
    #[serde(default)]
    pub iterations: u32,
    /// Set to `true` when any cap tripped. Once `true`, the circuit
    /// stays open until [`ConversationBudget::reset`] clears it.
    #[serde(default)]
    pub circuit_open: bool,
    /// Which dimension tripped — `"tokens"`, `"usd"`, or `"iterations"`.
    /// `None` while the circuit is closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tripped_dimension: Option<String>,
}

impl BudgetUsage {
    /// Total tokens (input + output). Drives the `tokens` cap check.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// Add one LLM call's accounting onto the accumulator.
    pub fn add_call(&mut self, input_tokens: u32, output_tokens: u32, usd_cost: f64) {
        self.input_tokens = self.input_tokens.saturating_add(input_tokens as u64);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens as u64);
        // Float saturating-add isn't exposed by std; clamp on overflow.
        let new = self.usd + usd_cost.max(0.0);
        self.usd = if new.is_finite() { new } else { f64::MAX };
        self.iterations = self.iterations.saturating_add(1);
    }
}

/// Persistence seam for budget accumulators.
///
/// The [`InMemoryBudgetStore`] satisfies CLI / test paths. The
/// substrate-backed impl in `clawft-service-agent` writes to
/// `derived/chat/<conv_id>/budget.json` on every mutation so circuit
/// state survives daemon restart (item 4).
///
/// The methods are sync because:
/// - the in-memory impl needs no I/O,
/// - the substrate impl wraps `SubstrateService::publish_gated_with_grants`
///   which is itself sync.
pub trait BudgetStore: Send + Sync + 'static {
    /// Load the current accumulator for `conv_id`. Returns the
    /// zero-default when the conv has no record (first call).
    fn load(&self, conv_id: &str) -> BudgetUsage;

    /// Persist `usage` for `conv_id`. Replaces any prior record.
    fn save(&self, conv_id: &str, usage: &BudgetUsage) -> Result<(), String>;
}

/// `HashMap`-backed [`BudgetStore`] for CLI / test paths.
#[derive(Debug, Default)]
pub struct InMemoryBudgetStore {
    inner: Mutex<HashMap<String, BudgetUsage>>,
}

impl InMemoryBudgetStore {
    /// Empty store — no conv records yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Test helper: snapshot of every record. Cheap clone of the map.
    #[allow(dead_code)]
    pub fn snapshot(&self) -> HashMap<String, BudgetUsage> {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl BudgetStore for InMemoryBudgetStore {
    fn load(&self, conv_id: &str) -> BudgetUsage {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.get(conv_id).cloned())
            .unwrap_or_default()
    }

    fn save(&self, conv_id: &str, usage: &BudgetUsage) -> Result<(), String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "InMemoryBudgetStore mutex poisoned".to_string())?;
        guard.insert(conv_id.to_string(), usage.clone());
        Ok(())
    }
}

/// Decision returned by [`ConversationBudget::check_can_call`].
///
/// `Allowed` means the loop may issue the next LLM call; `Tripped`
/// carries the dimension that crossed its cap and is mapped 1:1 to
/// [`ClawftError::ConversationBudgetExceeded`] at the loop boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetDecision {
    /// Budget is intact; the call may proceed.
    Allowed,
    /// A cap has been reached or the circuit was already open.
    Tripped {
        /// One of `"tokens"`, `"usd"`, `"iterations"`.
        dimension: String,
        /// Configured cap value cast to `f64` for the wire.
        limit: f64,
        /// Accumulated value at the moment of the trip.
        used: f64,
    },
}

impl BudgetDecision {
    /// Convert a [`BudgetDecision::Tripped`] into the typed
    /// [`ClawftError::ConversationBudgetExceeded`] that the agent loop
    /// surfaces. `Allowed` becomes `None` so the call site can `?`.
    pub fn into_error(self, conv_id: &str) -> Option<ClawftError> {
        match self {
            BudgetDecision::Allowed => None,
            BudgetDecision::Tripped {
                dimension,
                limit,
                used,
            } => Some(ClawftError::ConversationBudgetExceeded {
                conv_id: conv_id.to_string(),
                dimension,
                limit,
                used,
            }),
        }
    }
}

/// Per-conversation budget façade — config + store + (cheap) helpers.
///
/// The agent loop holds `Arc<ConversationBudget>` and:
///   1. calls `check_can_call(conv_id)` before each `pipeline.complete`,
///   2. calls `record_call(conv_id, &usage, usd)` after each call,
///   3. on trip, calls `mark_open(conv_id, dim, limit, used)` then
///      surfaces the typed error.
///
/// The `reset(conv_id)` API matches item 3 of WEFT-322 — invoked by
/// the daemon's `agent.chat.reset_budget` RPC.
pub struct ConversationBudget {
    config: CostBudgetConfig,
    store: Arc<dyn BudgetStore>,
}

impl ConversationBudget {
    /// Build a budget façade around the given config and store.
    pub fn new(config: CostBudgetConfig, store: Arc<dyn BudgetStore>) -> Self {
        Self { config, store }
    }

    /// Convenience: an in-memory budget with the supplied caps.
    pub fn in_memory(config: CostBudgetConfig) -> Self {
        Self::new(config, Arc::new(InMemoryBudgetStore::new()))
    }

    /// Current config caps (read-only borrow).
    pub fn config(&self) -> &CostBudgetConfig {
        &self.config
    }

    /// Current accumulator for `conv_id`. Defaults to zero when unset.
    pub fn usage(&self, conv_id: &str) -> BudgetUsage {
        self.store.load(conv_id)
    }

    /// Decide whether the next LLM call on `conv_id` may proceed.
    ///
    /// Returns [`BudgetDecision::Tripped`] when:
    ///   - the circuit is already open (a prior call tripped it), OR
    ///   - any cap (tokens / usd / iterations) is at or above its
    ///     configured limit.
    ///
    /// Note: the iteration cap is checked at `iterations >= limit`
    /// (i.e. the `limit`th call is rejected) so configuring `30`
    /// allows the first 30 calls and rejects the 31st. Matches the
    /// way `max_tool_iterations` is interpreted elsewhere.
    pub fn check_can_call(&self, conv_id: &str) -> BudgetDecision {
        let u = self.store.load(conv_id);
        if u.circuit_open {
            // Re-surface the original tripping dimension so callers
            // see the same wire shape across multiple fail-fast hits.
            let dim = u
                .tripped_dimension
                .clone()
                .unwrap_or_else(|| "unknown".into());
            let (limit, used) = match dim.as_str() {
                "tokens" => (
                    self.config.max_tokens_per_conv as f64,
                    u.total_tokens() as f64,
                ),
                "usd" => (self.config.max_usd_per_conv, u.usd),
                "iterations" => (
                    self.config.max_iterations_per_conv as f64,
                    u.iterations as f64,
                ),
                _ => (0.0, 0.0),
            };
            return BudgetDecision::Tripped {
                dimension: dim,
                limit,
                used,
            };
        }
        if u.total_tokens() >= self.config.max_tokens_per_conv {
            return BudgetDecision::Tripped {
                dimension: "tokens".into(),
                limit: self.config.max_tokens_per_conv as f64,
                used: u.total_tokens() as f64,
            };
        }
        if u.usd >= self.config.max_usd_per_conv {
            return BudgetDecision::Tripped {
                dimension: "usd".into(),
                limit: self.config.max_usd_per_conv,
                used: u.usd,
            };
        }
        if u.iterations >= self.config.max_iterations_per_conv {
            return BudgetDecision::Tripped {
                dimension: "iterations".into(),
                limit: self.config.max_iterations_per_conv as f64,
                used: u.iterations as f64,
            };
        }
        BudgetDecision::Allowed
    }

    /// Record one completed LLM call's accounting against `conv_id`.
    ///
    /// Increments the in-memory accumulator and persists it via
    /// [`BudgetStore::save`]. After this call, [`Self::check_after_call`]
    /// reports whether the latest update tripped any cap so the caller
    /// can immediately surface the error without waiting for the next
    /// `check_can_call` round-trip.
    pub fn record_call(
        &self,
        conv_id: &str,
        input_tokens: u32,
        output_tokens: u32,
        usd_cost: f64,
    ) -> Result<BudgetUsage, String> {
        let mut u = self.store.load(conv_id);
        u.add_call(input_tokens, output_tokens, usd_cost);
        self.store.save(conv_id, &u)?;
        Ok(u)
    }

    /// Inspect the post-call usage and return a [`BudgetDecision`]
    /// describing whether the *latest* update crossed any cap.
    ///
    /// Distinct from [`Self::check_can_call`] in that this does NOT
    /// honour `circuit_open` — the circuit is set BY this method via
    /// [`Self::mark_open`] when the caller acts on the result.
    pub fn check_after_call(&self, usage: &BudgetUsage) -> BudgetDecision {
        if usage.total_tokens() >= self.config.max_tokens_per_conv {
            return BudgetDecision::Tripped {
                dimension: "tokens".into(),
                limit: self.config.max_tokens_per_conv as f64,
                used: usage.total_tokens() as f64,
            };
        }
        if usage.usd >= self.config.max_usd_per_conv {
            return BudgetDecision::Tripped {
                dimension: "usd".into(),
                limit: self.config.max_usd_per_conv,
                used: usage.usd,
            };
        }
        if usage.iterations >= self.config.max_iterations_per_conv {
            return BudgetDecision::Tripped {
                dimension: "iterations".into(),
                limit: self.config.max_iterations_per_conv as f64,
                used: usage.iterations as f64,
            };
        }
        BudgetDecision::Allowed
    }

    /// Mark the circuit open. Persists the flag + the dimension that
    /// tripped so a subsequent [`Self::check_can_call`] surfaces the
    /// same wire-shape error after a daemon restart.
    pub fn mark_open(&self, conv_id: &str, dimension: &str) -> Result<(), String> {
        let mut u = self.store.load(conv_id);
        u.circuit_open = true;
        u.tripped_dimension = Some(dimension.to_string());
        self.store.save(conv_id, &u)
    }

    /// Reset the circuit and zero the accumulator (item 3 of WEFT-322).
    ///
    /// Invoked by the daemon RPC `agent.chat.reset_budget`. Returns the
    /// pre-reset snapshot so the caller can audit-log what was cleared.
    pub fn reset(&self, conv_id: &str) -> Result<BudgetUsage, String> {
        let prev = self.store.load(conv_id);
        let cleared = BudgetUsage::default();
        self.store.save(conv_id, &cleared)?;
        Ok(prev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(tokens: u64, usd: f64, iters: u32) -> CostBudgetConfig {
        CostBudgetConfig {
            max_tokens_per_conv: tokens,
            max_usd_per_conv: usd,
            max_iterations_per_conv: iters,
        }
    }

    #[test]
    fn defaults_match_spec() {
        let c = CostBudgetConfig::default();
        assert_eq!(c.max_tokens_per_conv, 200_000);
        assert!((c.max_usd_per_conv - 1.00).abs() < 1e-9);
        assert_eq!(c.max_iterations_per_conv, 30);
    }

    #[test]
    fn fresh_conv_is_allowed() {
        let b = ConversationBudget::in_memory(cfg(1_000, 1.0, 5));
        assert_eq!(b.check_can_call("c1"), BudgetDecision::Allowed);
    }

    #[test]
    fn token_cap_trips() {
        let b = ConversationBudget::in_memory(cfg(100, 10.0, 100));
        // Two calls of 60+0 tokens each → 120 input tokens > 100.
        b.record_call("c1", 60, 0, 0.0).unwrap();
        let u = b.record_call("c1", 60, 0, 0.0).unwrap();
        let dec = b.check_after_call(&u);
        assert!(
            matches!(dec, BudgetDecision::Tripped { ref dimension, .. } if dimension == "tokens")
        );
        b.mark_open("c1", "tokens").unwrap();
        // Subsequent check_can_call surfaces the same dimension.
        assert!(matches!(
            b.check_can_call("c1"),
            BudgetDecision::Tripped { ref dimension, .. } if dimension == "tokens"
        ));
    }

    #[test]
    fn usd_cap_trips() {
        let b = ConversationBudget::in_memory(cfg(10_000, 0.50, 100));
        b.record_call("c1", 10, 10, 0.30).unwrap();
        let u = b.record_call("c1", 10, 10, 0.30).unwrap();
        let dec = b.check_after_call(&u);
        match dec {
            BudgetDecision::Tripped {
                dimension,
                used,
                limit,
            } => {
                assert_eq!(dimension, "usd");
                assert!(used >= 0.50);
                assert!((limit - 0.50).abs() < 1e-9);
            }
            _ => panic!("expected USD trip, got {:?}", dec),
        }
    }

    #[test]
    fn iteration_cap_trips() {
        let b = ConversationBudget::in_memory(cfg(10_000, 100.0, 3));
        for _ in 0..3 {
            let u = b.record_call("c1", 1, 1, 0.0).unwrap();
            // After the 3rd call, iterations == 3 == cap → trip.
            let _ = u;
        }
        let dec = b.check_can_call("c1");
        match dec {
            BudgetDecision::Tripped {
                dimension,
                used,
                limit,
            } => {
                assert_eq!(dimension, "iterations");
                assert_eq!(used, 3.0);
                assert_eq!(limit, 3.0);
            }
            _ => panic!("expected iterations trip, got {:?}", dec),
        }
    }

    #[test]
    fn reset_clears_circuit() {
        let b = ConversationBudget::in_memory(cfg(100, 0.10, 5));
        b.record_call("c1", 60, 60, 0.0).unwrap();
        b.mark_open("c1", "tokens").unwrap();
        assert!(matches!(
            b.check_can_call("c1"),
            BudgetDecision::Tripped { .. }
        ));
        let prev = b.reset("c1").unwrap();
        assert!(prev.circuit_open);
        assert_eq!(prev.input_tokens, 60);
        // After reset: zeroed, allowed again.
        assert_eq!(b.check_can_call("c1"), BudgetDecision::Allowed);
        assert_eq!(b.usage("c1"), BudgetUsage::default());
    }

    #[test]
    fn into_error_carries_typed_fields() {
        let dec = BudgetDecision::Tripped {
            dimension: "tokens".into(),
            limit: 200_000.0,
            used: 250_000.0,
        };
        let err = dec.into_error("conv-abc").unwrap();
        let msg = err.to_string();
        assert!(msg.contains("conv-abc"), "{}", msg);
        assert!(msg.contains("tokens"), "{}", msg);
        assert!(msg.contains("circuit_open"), "{}", msg);
        match err {
            ClawftError::ConversationBudgetExceeded {
                conv_id,
                dimension,
                limit,
                used,
            } => {
                assert_eq!(conv_id, "conv-abc");
                assert_eq!(dimension, "tokens");
                assert_eq!(limit, 200_000.0);
                assert_eq!(used, 250_000.0);
            }
            other => panic!("wrong error variant: {:?}", other),
        }
    }

    #[test]
    fn allowed_into_error_is_none() {
        assert!(BudgetDecision::Allowed.into_error("c1").is_none());
    }

    #[test]
    fn isolated_per_conv() {
        let b = ConversationBudget::in_memory(cfg(100, 1.0, 5));
        b.record_call("a", 60, 60, 0.0).unwrap();
        b.mark_open("a", "tokens").unwrap();
        assert!(matches!(
            b.check_can_call("a"),
            BudgetDecision::Tripped { .. }
        ));
        // Sibling conv is unaffected.
        assert_eq!(b.check_can_call("b"), BudgetDecision::Allowed);
    }

    #[test]
    fn store_round_trip_persists_across_handles() {
        // Simulate "daemon restart" by keeping the same Arc<dyn BudgetStore>
        // and rebuilding the ConversationBudget around it.
        let store: Arc<dyn BudgetStore> = Arc::new(InMemoryBudgetStore::new());
        {
            let b1 = ConversationBudget::new(cfg(100, 1.0, 5), Arc::clone(&store));
            b1.record_call("c1", 60, 60, 0.0).unwrap();
            b1.mark_open("c1", "tokens").unwrap();
        }
        // Restart: fresh ConversationBudget, same store.
        let b2 = ConversationBudget::new(cfg(100, 1.0, 5), Arc::clone(&store));
        let u = b2.usage("c1");
        assert!(u.circuit_open);
        assert_eq!(u.input_tokens, 60);
        assert!(matches!(
            b2.check_can_call("c1"),
            BudgetDecision::Tripped { .. }
        ));
    }
}
