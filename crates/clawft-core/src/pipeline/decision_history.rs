//! In-memory ring buffer of recent pipeline routing decisions (WEFT-40).
//!
//! ## Why this exists
//!
//! Security review §2.3 / release-gate task 03-19: operators need a
//! retroactive view of per-tier dispatch counts, fallback rates, and
//! budget exhaustion events. The panel already shows current tier /
//! budget remaining; this buffer holds the last N decisions for the
//! admin HTTP endpoint (`GET /api/admin/routing/decisions`).
//!
//! ## Privacy (pairs with WEFT-30 / 03-09)
//!
//! Entries never store the free-text [`RoutingDecision::reason`]
//! (which embeds user identifiers and policy detail). Only the
//! stable [`RoutingDecision::redacted_reason`] category is retained.
//! `principal` is kept for operator filter-by-user on the auth-gated
//! admin surface; it is never embedded in free-text reason fields.
//!
//! ## Retention
//!
//! Bounded to [`DEFAULT_DECISION_HISTORY_CAPACITY`] (last-N ring).
//! Oldest entries are dropped when the cap is exceeded. Not durable
//! across restarts — substrate/chain persistence remains a follow-up.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::traits::RoutingDecision;

/// Default last-N capacity for the decision ring buffer.
///
/// Sized for a live operator window (roughly hours of traffic on a
/// single node) without unbounded growth.
pub const DEFAULT_DECISION_HISTORY_CAPACITY: usize = 1_000;

/// One recorded pipeline [`RoutingDecision`] for admin history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingDecisionEntry {
    /// ISO-8601 (UTC) wall-clock when the decision was recorded.
    pub ts: String,
    /// Provider name (e.g. "openai", "anthropic").
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Tier name when tiered routing was used.
    pub tier: Option<String>,
    /// Redacted reason category (never free-text — see module docs).
    pub reason_category: String,
    /// Whether the request was escalated to a higher tier.
    pub escalated: bool,
    /// Whether budget constraints affected the routing.
    pub budget_constrained: bool,
    /// Estimated cost in USD when known.
    pub cost_estimate_usd: Option<f64>,
    /// Originating principal (sender_id) for filter-by-user.
    pub principal: Option<String>,
    /// Channel when known (from auth context).
    pub channel: Option<String>,
    /// True when the decision used a fallback path.
    pub fallback_used: bool,
}

impl RoutingDecisionEntry {
    /// Build a privacy-safe entry from a live routing decision.
    pub fn from_decision(decision: &RoutingDecision, channel: Option<&str>) -> Self {
        let reason_category = decision.redacted_reason().to_string();
        let fallback_used = reason_category == "fallback_chosen"
            || decision.reason.contains("fallback");
        Self {
            ts: Utc::now().to_rfc3339(),
            provider: decision.provider.clone(),
            model: decision.model.clone(),
            tier: decision.tier.clone(),
            reason_category,
            escalated: decision.escalated,
            budget_constrained: decision.budget_constrained,
            cost_estimate_usd: decision.cost_estimate_usd,
            principal: decision.sender_id.clone(),
            channel: channel.map(|s| s.to_string()),
            fallback_used,
        }
    }
}

/// Filter for querying the decision ring buffer.
#[derive(Debug, Clone, Default)]
pub struct DecisionHistoryFilter {
    /// Max entries to return (newest first). `None` = capacity.
    pub limit: Option<usize>,
    /// Exact match on principal (sender_id).
    pub principal: Option<String>,
    /// Exact match on tier name.
    pub tier: Option<String>,
    /// Inclusive lower bound on `ts` (RFC3339 string compare works for
    /// ISO-8601 timestamps with fixed offset `Z` / `+00:00`).
    pub since: Option<String>,
}

/// Aggregate counters derived from the current ring contents.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RoutingAggregateStats {
    /// Number of decisions currently retained.
    pub count: u64,
    /// Ring capacity.
    pub capacity: u64,
    /// Dispatch counts keyed by tier name (`"(none)"` when un-tiered).
    pub by_tier: HashMap<String, u64>,
    /// Counts keyed by redacted reason category.
    pub by_reason: HashMap<String, u64>,
    /// Escalation count.
    pub escalated: u64,
    /// Budget-constrained count.
    pub budget_constrained: u64,
    /// Fallback path count.
    pub fallback: u64,
}

/// Thread-safe last-N ring buffer of routing decisions (WEFT-40).
#[derive(Debug)]
pub struct RoutingDecisionHistory {
    entries: Mutex<VecDeque<RoutingDecisionEntry>>,
    capacity: usize,
}

impl RoutingDecisionHistory {
    /// Empty history with the default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_DECISION_HISTORY_CAPACITY)
    }

    /// Empty history with an explicit capacity (`0` means unbounded —
    /// prefer a finite cap in production).
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            capacity,
        }
    }

    /// Ring capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of retained entries.
    pub fn len(&self) -> usize {
        self.entries.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append a decision (privacy-safe). Drops oldest when over capacity.
    pub fn record(&self, decision: &RoutingDecision) {
        self.record_with_channel(decision, None);
    }

    /// Append a decision with optional channel from auth context.
    pub fn record_with_channel(&self, decision: &RoutingDecision, channel: Option<&str>) {
        let entry = RoutingDecisionEntry::from_decision(decision, channel);
        let Ok(mut guard) = self.entries.lock() else {
            tracing::warn!("routing decision history mutex poisoned; drop record");
            return;
        };
        guard.push_back(entry);
        if self.capacity > 0 {
            while guard.len() > self.capacity {
                guard.pop_front();
            }
        }
        tracing::debug!(
            count = guard.len(),
            capacity = self.capacity,
            "routing decision history: recorded"
        );
    }

    /// Recent entries, newest first, up to `limit`.
    pub fn recent(&self, limit: usize) -> Vec<RoutingDecisionEntry> {
        self.query(DecisionHistoryFilter {
            limit: Some(limit),
            ..Default::default()
        })
    }

    /// Query with optional principal / tier / since filters.
    ///
    /// Returns newest-first.
    pub fn query(&self, filter: DecisionHistoryFilter) -> Vec<RoutingDecisionEntry> {
        let Ok(guard) = self.entries.lock() else {
            return Vec::new();
        };
        let limit = filter.limit.unwrap_or(self.capacity);
        guard
            .iter()
            .rev()
            .filter(|e| {
                if let Some(ref p) = filter.principal
                    && e.principal.as_deref() != Some(p.as_str())
                {
                    return false;
                }
                if let Some(ref t) = filter.tier
                    && e.tier.as_deref() != Some(t.as_str())
                {
                    return false;
                }
                if let Some(ref since) = filter.since
                    && e.ts.as_str() < since.as_str()
                {
                    return false;
                }
                true
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// Aggregate stats over the current ring contents.
    pub fn aggregate_stats(&self) -> RoutingAggregateStats {
        let Ok(guard) = self.entries.lock() else {
            return RoutingAggregateStats {
                capacity: self.capacity as u64,
                ..Default::default()
            };
        };
        let mut stats = RoutingAggregateStats {
            count: guard.len() as u64,
            capacity: self.capacity as u64,
            ..Default::default()
        };
        for e in guard.iter() {
            let tier_key = e.tier.clone().unwrap_or_else(|| "(none)".into());
            *stats.by_tier.entry(tier_key).or_insert(0) += 1;
            *stats
                .by_reason
                .entry(e.reason_category.clone())
                .or_insert(0) += 1;
            if e.escalated {
                stats.escalated += 1;
            }
            if e.budget_constrained {
                stats.budget_constrained += 1;
            }
            if e.fallback_used {
                stats.fallback += 1;
            }
        }
        stats
    }
}

impl Default for RoutingDecisionHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_decision(n: usize, tier: &str, sender: &str) -> RoutingDecision {
        RoutingDecision {
            provider: "openai".into(),
            model: format!("model-{n}"),
            reason: format!(
                "tiered routing: complexity=0.5, tier={tier}, level=1, user={sender}"
            ),
            tier: Some(tier.into()),
            cost_estimate_usd: Some(0.01 * n as f64),
            escalated: n % 3 == 0,
            budget_constrained: n % 5 == 0,
            sender_id: Some(sender.into()),
        }
    }

    #[test]
    fn record_redacts_reason() {
        let hist = RoutingDecisionHistory::new();
        let d = sample_decision(1, "standard", "alice@example.com");
        hist.record(&d);
        let entries = hist.recent(10);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        // Free-text reason must not leak into the entry.
        assert_eq!(e.reason_category, "tiered_routing");
        assert!(!e.reason_category.contains("alice"));
        assert_eq!(e.principal.as_deref(), Some("alice@example.com"));
        assert_eq!(e.tier.as_deref(), Some("standard"));
        assert!(!e.ts.is_empty());
    }

    #[test]
    fn ring_evicts_oldest() {
        let hist = RoutingDecisionHistory::with_capacity(3);
        for i in 0..5 {
            hist.record(&sample_decision(i, "free", "u"));
        }
        assert_eq!(hist.len(), 3);
        let models: Vec<_> = hist
            .query(DecisionHistoryFilter {
                limit: Some(10),
                ..Default::default()
            })
            .into_iter()
            .map(|e| e.model)
            .collect();
        // Newest first: 4, 3, 2
        assert_eq!(models, vec!["model-4", "model-3", "model-2"]);
    }

    #[test]
    fn filter_by_principal_and_tier() {
        let hist = RoutingDecisionHistory::with_capacity(100);
        hist.record(&sample_decision(0, "free", "alice"));
        hist.record(&sample_decision(1, "premium", "alice"));
        hist.record(&sample_decision(2, "free", "bob"));

        let alice_free = hist.query(DecisionHistoryFilter {
            principal: Some("alice".into()),
            tier: Some("free".into()),
            limit: Some(10),
            ..Default::default()
        });
        assert_eq!(alice_free.len(), 1);
        assert_eq!(alice_free[0].model, "model-0");

        let bob = hist.query(DecisionHistoryFilter {
            principal: Some("bob".into()),
            limit: Some(10),
            ..Default::default()
        });
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0].principal.as_deref(), Some("bob"));
    }

    #[test]
    fn filter_by_since() {
        let hist = RoutingDecisionHistory::with_capacity(10);
        hist.record(&sample_decision(0, "free", "u"));
        // Future since → empty
        let future = hist.query(DecisionHistoryFilter {
            since: Some("2999-01-01T00:00:00Z".into()),
            limit: Some(10),
            ..Default::default()
        });
        assert!(future.is_empty());
        // Past since → all
        let past = hist.query(DecisionHistoryFilter {
            since: Some("2000-01-01T00:00:00Z".into()),
            limit: Some(10),
            ..Default::default()
        });
        assert_eq!(past.len(), 1);
    }

    #[test]
    fn aggregate_stats_counts() {
        let hist = RoutingDecisionHistory::with_capacity(100);
        for i in 0..10 {
            hist.record(&sample_decision(i, if i < 7 { "free" } else { "premium" }, "u"));
        }
        // Force a fallback entry
        let mut fb = sample_decision(99, "free", "u");
        fb.reason = "fallback: primary unavailable".into();
        hist.record(&fb);

        let stats = hist.aggregate_stats();
        assert_eq!(stats.count, 11);
        assert_eq!(stats.capacity, 100);
        assert_eq!(stats.by_tier.get("free").copied(), Some(8)); // 0..6 + fallback
        assert_eq!(stats.by_tier.get("premium").copied(), Some(3));
        assert!(stats.fallback >= 1);
        // sample i%3==0 for i in 0..10 → 0,3,6,9; plus fallback n=99 → 5
        assert_eq!(stats.escalated, 5);
    }

    #[test]
    fn fallback_and_rate_limited_categories() {
        let hist = RoutingDecisionHistory::new();
        let mut d = sample_decision(0, "free", "u");
        d.reason = "rate limited: using free tier".into();
        hist.record_with_channel(&d, Some("telegram"));
        let e = &hist.recent(1)[0];
        assert_eq!(e.reason_category, "rate_limited");
        assert_eq!(e.channel.as_deref(), Some("telegram"));
    }

    #[test]
    fn one_hundred_decisions_produce_one_hundred_entries() {
        let hist = RoutingDecisionHistory::with_capacity(1_000);
        for i in 0..100 {
            hist.record(&sample_decision(i, "standard", "batch"));
        }
        assert_eq!(hist.len(), 100);
        assert_eq!(hist.recent(100).len(), 100);
    }
}
