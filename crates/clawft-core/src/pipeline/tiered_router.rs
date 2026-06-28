//! Tiered router (Level 1 implementation).
//!
//! Selects models based on task complexity, user permissions, and cost
//! budgets. Implements the [`ModelRouter`] trait as a drop-in replacement
//! for `StaticRouter` when `routing.mode == "tiered"`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use clawft_types::routing::{
    AuthContext, EscalationConfig, RoutingConfig, TierSelectionStrategy, UserPermissions,
};

use super::traits::{
    BudgetResult, ChatRequest, CostTrackable, ModelRouter, RateLimitable, ResponseOutcome,
    RoutingDecision, TaskProfile,
};

// ── ModelTier runtime struct ────────────────────────────────────────────

/// Runtime representation of a model tier with routing state.
///
/// Built from [`ModelTierConfig`] during [`TieredRouter::new`].
/// The `ordinal` field tracks position in the cheapest-to-most-expensive
/// ordering for efficient comparison.
#[derive(Debug, Clone)]
pub struct ModelTier {
    /// Tier name (e.g., "free", "standard", "premium", "elite").
    pub name: String,
    /// Models available in this tier, in preference order.
    /// Format: "provider/model" (e.g., "anthropic/claude-haiku-3.5").
    pub models: Vec<String>,
    /// Complexity range this tier covers: [min, max] (both inclusive).
    pub complexity_range: [f32; 2],
    /// Approximate cost per 1K tokens (blended input/output) in USD.
    pub cost_per_1k_tokens: f64,
    /// Maximum context tokens supported by models in this tier.
    pub max_context_tokens: usize,
    /// Ordinal position in the tier list (0 = cheapest).
    pub ordinal: usize,
}

impl ModelTier {
    /// Returns true if the given complexity falls within this tier's range
    /// (inclusive on both ends).
    pub fn matches_complexity(&self, complexity: f32) -> bool {
        complexity >= self.complexity_range[0] && complexity <= self.complexity_range[1]
    }
}

// ── No-op implementations ───────────────────────────────────────────────

/// No-op cost tracker for when budget enforcement is disabled.
pub struct NoopCostTracker;

impl CostTrackable for NoopCostTracker {
    fn check_budget(
        &self,
        _sender_id: &str,
        _estimated_cost: f64,
        _daily_limit: f64,
        _monthly_limit: f64,
    ) -> BudgetResult {
        BudgetResult::Approved
    }

    fn record_estimated(&self, _sender_id: &str, _estimated_cost: f64) {}

    fn record_actual(&self, _sender_id: &str, _estimated_cost: f64, _actual_cost: f64) {}
}

/// No-op rate limiter for when rate limiting is disabled.
pub struct NoopRateLimiter;

impl RateLimitable for NoopRateLimiter {
    fn check(&self, _sender_id: &str, _limit: u32) -> bool {
        true
    }
}

// ── Internal helper types ───────────────────────────────────────────────

/// Internal result of tier selection.
struct TierSelection<'a> {
    tier: &'a ModelTier,
    escalated: bool,
}

/// Internal result of budget constraint evaluation.
struct TierBudgetResult<'a> {
    tier: &'a ModelTier,
    constrained: bool,
}

// ── TieredRouter ────────────────────────────────────────────────────────

/// Level 1 router that selects models based on task complexity,
/// user permissions, and cost budgets.
///
/// Implements [`ModelRouter`] as a drop-in replacement for `StaticRouter`
/// when `routing.mode == "tiered"`.
pub struct TieredRouter {
    /// Configured model tiers, ordered cheapest to most expensive.
    tiers: Vec<ModelTier>,
    /// Tier name -> ordinal index for O(1) lookup of max_tier boundaries.
    tier_index: HashMap<String, usize>,
    /// Model selection strategy within a tier.
    selection_strategy: TierSelectionStrategy,
    /// Round-robin counters per tier (tier ordinal -> atomic counter).
    round_robin_counters: Vec<AtomicUsize>,
    /// Escalation configuration from routing config.
    escalation_config: EscalationConfig,
    /// Fallback model when all tiers are exhausted or budget-blocked.
    /// Format: "provider/model".
    fallback_model: Option<String>,
    /// Cost tracker for budget enforcement.
    cost_tracker: Option<Arc<dyn CostTrackable>>,
    /// Rate limiter for per-user throttling.
    rate_limiter: Option<Arc<dyn RateLimitable>>,
}

impl TieredRouter {
    /// Create a TieredRouter from routing configuration.
    pub fn new(config: RoutingConfig) -> Self {
        let tiers: Vec<ModelTier> = config
            .tiers
            .iter()
            .enumerate()
            .map(|(i, t)| ModelTier {
                name: t.name.clone(),
                models: t.models.clone(),
                complexity_range: t.complexity_range,
                cost_per_1k_tokens: t.cost_per_1k_tokens,
                max_context_tokens: t.max_context_tokens,
                ordinal: i,
            })
            .collect();

        let tier_index: HashMap<String, usize> =
            tiers.iter().map(|t| (t.name.clone(), t.ordinal)).collect();

        let round_robin_counters: Vec<AtomicUsize> =
            (0..tiers.len()).map(|_| AtomicUsize::new(0)).collect();

        let selection_strategy = config
            .selection_strategy
            .unwrap_or(TierSelectionStrategy::PreferenceOrder);

        Self {
            tiers,
            tier_index,
            selection_strategy,
            round_robin_counters,
            escalation_config: config.escalation,
            fallback_model: config.fallback_model,
            cost_tracker: None,
            rate_limiter: None,
        }
    }

    /// Set a cost tracker for budget enforcement (builder pattern).
    pub fn with_cost_tracker(mut self, tracker: Arc<dyn CostTrackable>) -> Self {
        self.cost_tracker = Some(tracker);
        self
    }

    /// Set a rate limiter (builder pattern).
    pub fn with_rate_limiter(mut self, limiter: Arc<dyn RateLimitable>) -> Self {
        self.rate_limiter = Some(limiter);
        self
    }

    // ── Tier filtering ──────────────────────────────────────────────

    /// Filter tiers to only those the user is allowed to access.
    ///
    /// Returns tiers at or below the user's `max_tier`, preserving
    /// the cheapest-first ordering from config.
    fn filter_tiers_by_permissions<'a>(
        &'a self,
        permissions: &UserPermissions,
    ) -> Vec<&'a ModelTier> {
        let max_ordinal = self
            .tier_index
            .get(&permissions.max_tier)
            .copied()
            .unwrap_or(usize::MAX); // Unknown tier name -> allow all tiers

        self.tiers
            .iter()
            .filter(|t| t.ordinal <= max_ordinal)
            .collect()
    }

    // ── Tier selection ──────────────────────────────────────────────

    /// Select the best tier for the given complexity, respecting permissions.
    ///
    /// Algorithm:
    /// 1. Find all allowed tiers whose complexity_range covers the task
    ///    complexity.
    /// 2. Among matches, pick the highest-quality (highest ordinal) tier.
    /// 3. If no match, check escalation eligibility.
    /// 4. If escalation allowed, try tiers above max_tier (up to
    ///    max_escalation_tiers).
    /// 5. If still no match, fall back to the highest allowed tier.
    fn select_tier<'a>(
        &'a self,
        complexity: f32,
        allowed_tiers: &[&'a ModelTier],
        permissions: &UserPermissions,
    ) -> TierSelection<'a> {
        // Step 1: Find matching tiers within permission boundary
        let matching: Vec<&&ModelTier> = allowed_tiers
            .iter()
            .filter(|t| t.matches_complexity(complexity))
            .collect();

        // Step 2: Pick the highest-quality match
        if let Some(best) = matching.iter().max_by_key(|t| t.ordinal) {
            return TierSelection {
                tier: best,
                escalated: false,
            };
        }

        // Step 3: No allowed tier matches -- try escalation
        if permissions.escalation_allowed
            && self.escalation_config.enabled
            && complexity > permissions.escalation_threshold
        {
            let max_ordinal = self
                .tier_index
                .get(&permissions.max_tier)
                .copied()
                .unwrap_or(0);

            let max_escalation = self.escalation_config.max_escalation_tiers;

            // Look at tiers above the user's max_tier, up to max_escalation_tiers
            let escalation_candidates: Vec<&ModelTier> = self
                .tiers
                .iter()
                .filter(|t| {
                    t.ordinal > max_ordinal
                        && t.ordinal <= max_ordinal + max_escalation as usize
                        && t.matches_complexity(complexity)
                })
                .collect();

            if let Some(best) = escalation_candidates.iter().max_by_key(|t| t.ordinal) {
                tracing::info!(
                    user_max_tier = %permissions.max_tier,
                    escalated_to = %best.name,
                    complexity = %complexity,
                    "escalation applied: promoting beyond max_tier"
                );
                return TierSelection {
                    tier: best,
                    escalated: true,
                };
            }
        }

        // Step 4: No match even with escalation -- fall back to highest allowed tier
        if let Some(highest) = allowed_tiers.iter().max_by_key(|t| t.ordinal) {
            return TierSelection {
                tier: highest,
                escalated: false,
            };
        }

        // Should not reach here if tiers list is non-empty, but handle gracefully
        TierSelection {
            tier: &self.tiers[0],
            escalated: false,
        }
    }

    // ── Budget constraints ──────────────────────────────────────────

    /// Apply budget constraints to the selected tier.
    ///
    /// If the estimated cost of the selected tier would push the user over
    /// their daily budget, fall back to cheaper tiers until one fits or all
    /// tiers are exhausted.
    fn apply_budget_constraints<'a>(
        &'a self,
        selected: &'a ModelTier,
        allowed_tiers: &[&'a ModelTier],
        auth: &AuthContext,
        permissions: &UserPermissions,
    ) -> TierBudgetResult<'a> {
        let cost_tracker = match &self.cost_tracker {
            Some(ct) => ct,
            None => {
                return TierBudgetResult {
                    tier: selected,
                    constrained: false,
                };
            }
        };

        // If budget is unlimited (0.0), skip checking
        if permissions.cost_budget_daily_usd <= 0.0 && permissions.cost_budget_monthly_usd <= 0.0 {
            return TierBudgetResult {
                tier: selected,
                constrained: false,
            };
        }

        let estimated_cost = selected.cost_per_1k_tokens;

        let budget_check = cost_tracker.check_budget(
            &auth.sender_id,
            estimated_cost,
            permissions.cost_budget_daily_usd,
            permissions.cost_budget_monthly_usd,
        );

        if budget_check == BudgetResult::Approved {
            return TierBudgetResult {
                tier: selected,
                constrained: false,
            };
        }

        tracing::info!(
            user = %auth.sender_id,
            selected_tier = %selected.name,
            estimated_cost = %estimated_cost,
            daily_limit = %permissions.cost_budget_daily_usd,
            monthly_limit = %permissions.cost_budget_monthly_usd,
            "budget constraint triggered: attempting tier downgrade"
        );

        // Budget would be exceeded -- fall back to cheaper tiers
        let mut candidates: Vec<&&ModelTier> = allowed_tiers
            .iter()
            .filter(|t| t.ordinal < selected.ordinal)
            .collect();
        candidates.sort_by(|a, b| b.ordinal.cmp(&a.ordinal)); // highest first

        for candidate in candidates {
            let candidate_cost = candidate.cost_per_1k_tokens;
            let candidate_check = cost_tracker.check_budget(
                &auth.sender_id,
                candidate_cost,
                permissions.cost_budget_daily_usd,
                permissions.cost_budget_monthly_usd,
            );
            if candidate_check == BudgetResult::Approved {
                return TierBudgetResult {
                    tier: candidate,
                    constrained: true,
                };
            }
        }

        // No tier fits -- use the cheapest tier anyway (overage is recorded)
        if let Some(cheapest) = allowed_tiers.iter().min_by_key(|t| t.ordinal) {
            return TierBudgetResult {
                tier: cheapest,
                constrained: true,
            };
        }

        TierBudgetResult {
            tier: selected,
            constrained: true,
        }
    }

    // ── Model selection ─────────────────────────────────────────────

    /// Select a specific model from the tier's model list.
    ///
    /// Applies the configured selection strategy and filters by the user's
    /// model_access allowlist and model_denylist.
    fn select_model(
        &self,
        tier: &ModelTier,
        permissions: &UserPermissions,
    ) -> Option<(String, String)> {
        let available = self.filter_models_by_permissions(&tier.models, permissions);
        if available.is_empty() {
            return None;
        }

        let selected = match &self.selection_strategy {
            TierSelectionStrategy::PreferenceOrder => available[0].clone(),
            TierSelectionStrategy::RoundRobin => {
                let counter = &self.round_robin_counters[tier.ordinal];
                let idx = counter.fetch_add(1, Ordering::Relaxed) % available.len();
                available[idx].clone()
            }
            TierSelectionStrategy::LowestCost => {
                // Within a tier all models share cost_per_1k_tokens;
                // just pick the first one.
                available[0].clone()
            }
            TierSelectionStrategy::Random => {
                // Use a simple hash-based selection to avoid needing the rand crate.
                // `runtime::now_millis()` is safe on both native and browser WASM.
                let seed = crate::runtime::now_millis();
                let idx = (seed as usize) % available.len();
                available[idx].clone()
            }
            _ => available[0].clone(),
        };

        Some(split_provider_model(&selected))
    }

    /// Filter a tier's model list against user permission allowlist/denylist.
    fn filter_models_by_permissions(
        &self,
        models: &[String],
        permissions: &UserPermissions,
    ) -> Vec<String> {
        models
            .iter()
            .filter(|m| {
                // Allowlist check (empty = all allowed)
                let allowed = permissions.model_access.is_empty()
                    || permissions
                        .model_access
                        .iter()
                        .any(|p| model_matches_pattern(m, p));
                // Denylist check
                let denied = permissions
                    .model_denylist
                    .iter()
                    .any(|p| model_matches_pattern(m, p));
                allowed && !denied
            })
            .cloned()
            .collect()
    }

    // ── Fallback chain ──────────────────────────────────────────────

    /// Execute the fallback chain when the primary tier's models are all
    /// unavailable.
    ///
    /// Chain: lower tiers (descending quality) -> fallback_model -> None
    fn fallback_chain(
        &self,
        primary_tier: &ModelTier,
        allowed_tiers: &[&ModelTier],
        permissions: &UserPermissions,
    ) -> Option<(String, String, String)> {
        let max_ordinal = self
            .tier_index
            .get(&permissions.max_tier)
            .copied()
            .unwrap_or(0);

        // 1. Try lower tiers in descending quality order
        let mut lower_tiers: Vec<&&ModelTier> = allowed_tiers
            .iter()
            .filter(|t| t.ordinal < primary_tier.ordinal)
            .collect();
        lower_tiers.sort_by(|a, b| b.ordinal.cmp(&a.ordinal));

        for tier in lower_tiers {
            if tier.ordinal > max_ordinal {
                continue;
            }
            if let Some((provider, model)) = self.select_model(tier, permissions) {
                let reason = format!(
                    "fallback from tier '{}' to tier '{}'",
                    primary_tier.name, tier.name
                );
                return Some((provider, model, reason));
            }
        }

        // 2. Try the global fallback model -- but only if it belongs to a
        // permitted tier
        if let Some(ref fallback) = self.fallback_model {
            let fallback_tier_ordinal = self
                .tiers
                .iter()
                .find(|t| t.models.iter().any(|m| m == fallback))
                .map(|t| t.ordinal);

            match fallback_tier_ordinal {
                Some(ordinal) if ordinal > max_ordinal => {
                    // Fallback model is in a tier above user's permission level.
                    tracing::info!(
                        fallback_model = %fallback,
                        fallback_tier_ordinal = %ordinal,
                        user_max_ordinal = %max_ordinal,
                        "fallback chain: fallback_model denied -- tier above user max_tier"
                    );
                    // Do NOT return the fallback -- fall through to None
                }
                _ => {
                    let (provider, model) = split_provider_model(fallback);
                    let reason = format!("fallback to configured fallback_model '{}'", fallback);
                    return Some((provider, model, reason));
                }
            }
        }

        // 3. No models available
        None
    }

    // ── Decision helpers ────────────────────────────────────────────

    /// Build a RoutingDecision for rate-limited requests.
    fn rate_limited_decision(&self, permissions: &UserPermissions) -> RoutingDecision {
        let max_ordinal = self
            .tier_index
            .get(&permissions.max_tier)
            .copied()
            .unwrap_or(0);

        if let Some(ref fallback) = self.fallback_model {
            let fallback_tier_ordinal = self
                .tiers
                .iter()
                .find(|t| t.models.iter().any(|m| m == fallback))
                .map(|t| t.ordinal);

            if let Some(ordinal) = fallback_tier_ordinal
                && ordinal > max_ordinal
            {
                return RoutingDecision {
                    provider: String::new(),
                    model: String::new(),
                    reason: "rate limited: fallback model not permitted for user tier".into(),
                    ..Default::default()
                };
            }

            let (provider, model) = split_provider_model(fallback);
            RoutingDecision {
                provider,
                model,
                reason: "rate limited: using fallback model".into(),
                ..Default::default()
            }
        } else {
            RoutingDecision {
                provider: String::new(),
                model: String::new(),
                reason: "rate limited: no fallback model configured".into(),
                budget_constrained: true,
                ..Default::default()
            }
        }
    }

    /// Build a RoutingDecision when no tiers are available at all.
    ///
    /// WEFT-27: even on the no-tiers path, the global fallback model must
    /// honour the caller's `max_tier`. Without this gate, a misconfigured
    /// `routing.fallback_model: anthropic/claude-opus-4-5` would let
    /// zero-trust users hit elite models — bypassing the entire
    /// permission system. When permissions are not provided (legacy
    /// callers), we fall back to permissive behaviour (no caller context
    /// means no caller to authorise against).
    fn no_tiers_available_decision(
        &self,
        permissions: Option<&UserPermissions>,
    ) -> RoutingDecision {
        if let Some(ref fallback) = self.fallback_model {
            // Apply the same tier check the regular fallback chain uses
            // (FIX-06 / WEFT-27). If the fallback model lives in a tier
            // above the caller's max_tier, deny rather than leak access.
            if let Some(perms) = permissions {
                let max_ordinal = self.tier_index.get(&perms.max_tier).copied().unwrap_or(0);
                let fallback_tier_ordinal = self
                    .tiers
                    .iter()
                    .find(|t| t.models.iter().any(|m| m == fallback))
                    .map(|t| t.ordinal);

                if let Some(ordinal) = fallback_tier_ordinal
                    && ordinal > max_ordinal
                {
                    tracing::warn!(
                        fallback_tier_ordinal = %ordinal,
                        user_max_ordinal = %max_ordinal,
                        "no_tiers_available: fallback model denied -- tier above user max_tier"
                    );
                    return RoutingDecision {
                        provider: String::new(),
                        model: String::new(),
                        reason: "no tiers available: fallback model not permitted for user tier"
                            .into(),
                        ..Default::default()
                    };
                }
            }

            let (provider, model) = split_provider_model(fallback);
            RoutingDecision {
                provider,
                model,
                reason: "no tiers available: using fallback model".into(),
                ..Default::default()
            }
        } else {
            RoutingDecision {
                provider: String::new(),
                model: String::new(),
                reason: "no tiers or fallback model available".into(),
                ..Default::default()
            }
        }
    }
}

// ── ModelRouter trait implementation ─────────────────────────────────────

#[async_trait]
impl ModelRouter for TieredRouter {
    async fn route(&self, request: &ChatRequest, profile: &TaskProfile) -> RoutingDecision {
        // Step 1: Extract auth context
        let auth = request.auth_context.as_ref().cloned().unwrap_or_default(); // zero-trust if absent

        // Step 2: Read pre-resolved permissions
        let default_permissions = UserPermissions::default();
        let permissions = request
            .auth_context
            .as_ref()
            .map(|a| &a.permissions)
            .unwrap_or(&default_permissions);

        tracing::info!(
            sender = %auth.sender_id,
            channel = %auth.channel,
            complexity = %profile.complexity,
            max_tier = %permissions.max_tier,
            level = %permissions.level,
            "route: starting tiered routing decision"
        );

        // ── WEFT-31: model_override audit ───────────────────────────
        //
        // When a request supplies an explicit `model` AND the caller's
        // permissions grant `model_override`, the tier system is
        // bypassed entirely — the request gets the model it asked for.
        // This is the highest-blast-radius routing path: an admin
        // (or any caller with `model_override: true`) can punch through
        // tier filtering, escalation thresholds, and budget caps. To
        // keep that path auditable we emit a `routing.audit` warn line
        // every time it fires and a chain event for durable replay.
        // The event must contain enough metadata for after-the-fact
        // governance review (who, what, where) without being noisy on
        // the normal routing path.
        if let Some(ref override_model) = request.model
            && permissions.model_override
        {
            let (provider, model) = split_provider_model(override_model);
            tracing::warn!(
                target: "routing.audit",
                principal = %auth.sender_id,
                channel = %auth.channel,
                level = %permissions.level,
                model = %override_model,
                "model_override applied: tier filtering bypassed"
            );
            crate::chain_event!(
                "routing",
                "model_override_bypass",
                {
                    "principal": auth.sender_id,
                    "channel": auth.channel,
                    "level": permissions.level,
                    "model": override_model,
                }
            );
            return RoutingDecision {
                provider,
                model,
                reason: format!(
                    "model_override bypass: principal={}, channel={}, model={}",
                    auth.sender_id, auth.channel, override_model
                ),
                tier: None,
                cost_estimate_usd: None,
                escalated: false,
                budget_constrained: false,
                sender_id: Some(auth.sender_id.clone()),
            };
        }

        // Step 3: Check rate limit
        if let Some(ref limiter) = self.rate_limiter
            && permissions.rate_limit > 0
            && !limiter.check(&auth.sender_id, permissions.rate_limit)
        {
            tracing::info!(sender = %auth.sender_id, "route: rate limited");
            return self.rate_limited_decision(permissions);
        }

        // Step 4: Filter tiers by permission level
        let allowed_tiers = self.filter_tiers_by_permissions(permissions);
        if allowed_tiers.is_empty() {
            tracing::info!(sender = %auth.sender_id, "route: no tiers available");
            return self.no_tiers_available_decision(Some(permissions));
        }

        // Step 5: Select tier by complexity (with escalation)
        let tier_selection = self.select_tier(profile.complexity, &allowed_tiers, permissions);

        // Step 6: Apply budget constraints
        let budget_result =
            self.apply_budget_constraints(tier_selection.tier, &allowed_tiers, &auth, permissions);

        let final_tier = budget_result.tier;
        let escalated = tier_selection.escalated;
        let budget_constrained = budget_result.constrained;

        // Step 7: Select model from tier
        let (provider, model) = match self.select_model(final_tier, permissions) {
            Some(pm) => pm,
            None => {
                // No models available in selected tier -- try fallback chain
                match self.fallback_chain(final_tier, &allowed_tiers, permissions) {
                    Some((p, m, reason)) => {
                        return RoutingDecision {
                            provider: p,
                            model: m,
                            reason,
                            tier: Some(final_tier.name.clone()),
                            cost_estimate_usd: Some(final_tier.cost_per_1k_tokens),
                            escalated,
                            budget_constrained,
                            sender_id: Some(auth.sender_id.clone()),
                        };
                    }
                    None => return self.no_tiers_available_decision(Some(permissions)),
                }
            }
        };

        // Step 8: Record estimated cost + build decision
        let cost_estimate = final_tier.cost_per_1k_tokens;
        if let Some(ref tracker) = self.cost_tracker {
            tracker.record_estimated(&auth.sender_id, cost_estimate);
        }

        tracing::info!(
            sender = %auth.sender_id,
            provider = %provider,
            model = %model,
            tier = %final_tier.name,
            cost_estimate_usd = %cost_estimate,
            escalated = %escalated,
            budget_constrained = %budget_constrained,
            "route: decision made"
        );

        RoutingDecision {
            provider,
            model,
            reason: format!(
                "tiered routing: complexity={:.2}, tier={}, level={}, user={}",
                profile.complexity, final_tier.name, permissions.level, auth.sender_id,
            ),
            tier: Some(final_tier.name.clone()),
            cost_estimate_usd: Some(cost_estimate),
            escalated,
            budget_constrained,
            sender_id: Some(auth.sender_id.clone()),
        }
    }

    fn update(&self, decision: &RoutingDecision, _outcome: &ResponseOutcome) {
        // Record actual cost after response is received.
        // For now, cost_estimate_usd is used as a proxy for actual cost.
        // A proper cost calculation from token usage will be added later.
        if let (Some(cost), Some(sender_id)) = (decision.cost_estimate_usd, &decision.sender_id)
            && let Some(ref tracker) = self.cost_tracker
        {
            // Reconcile: actual == estimated for now (no token-based cost yet)
            tracker.record_actual(sender_id, cost, cost);
        }
    }
}

// ── Debug implementation ────────────────────────────────────────────────

impl std::fmt::Debug for TieredRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TieredRouter")
            .field(
                "tiers",
                &self.tiers.iter().map(|t| &t.name).collect::<Vec<_>>(),
            )
            .field("selection_strategy", &self.selection_strategy)
            .field("escalation_config", &self.escalation_config)
            .field("fallback_model", &self.fallback_model)
            .finish()
    }
}

// ── Helper functions ────────────────────────────────────────────────────

/// Check if a model name matches a glob-style pattern.
/// Supports exact match, prefix wildcard ("anthropic/*"), full wildcard ("*").
fn model_matches_pattern(model: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return model.starts_with(prefix);
    }
    model == pattern
}

/// Split a "provider/model" string into (provider, model).
/// If no slash, defaults provider to "openai".
fn split_provider_model(s: &str) -> (String, String) {
    if let Some(idx) = s.find('/') {
        (s[..idx].to_string(), s[idx + 1..].to_string())
    } else {
        ("openai".to_string(), s.to_string())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::traits::{LlmMessage, TaskType};
    use clawft_types::routing::ModelTierConfig;
    use std::sync::Mutex;

    // ── Test helpers ────────────────────────────────────────────────

    fn make_tier_config(
        name: &str,
        models: Vec<&str>,
        range: [f32; 2],
        cost: f64,
    ) -> ModelTierConfig {
        ModelTierConfig {
            name: name.into(),
            models: models.into_iter().map(|s| s.to_string()).collect(),
            complexity_range: range,
            cost_per_1k_tokens: cost,
            max_context_tokens: 8192,
        }
    }

    fn standard_tiers() -> Vec<ModelTierConfig> {
        vec![
            make_tier_config("free", vec!["groq/llama-3.1-8b"], [0.0, 0.3], 0.0),
            make_tier_config(
                "standard",
                vec!["anthropic/claude-haiku-3.5", "openai/gpt-4o-mini"],
                [0.0, 0.7],
                0.001,
            ),
            make_tier_config(
                "premium",
                vec!["anthropic/claude-sonnet-4", "openai/gpt-4o"],
                [0.3, 1.0],
                0.01,
            ),
            make_tier_config("elite", vec!["anthropic/claude-opus-4"], [0.7, 1.0], 0.05),
        ]
    }

    fn make_config(tiers: Vec<ModelTierConfig>) -> RoutingConfig {
        RoutingConfig {
            mode: "tiered".into(),
            tiers,
            selection_strategy: Some(TierSelectionStrategy::PreferenceOrder),
            fallback_model: Some("groq/llama-3.1-8b".into()),
            escalation: EscalationConfig {
                enabled: true,
                threshold: 0.6,
                max_escalation_tiers: 1,
            },
            ..RoutingConfig::default()
        }
    }

    fn make_request() -> ChatRequest {
        ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        }
    }

    fn make_request_with_auth(auth: AuthContext) -> ChatRequest {
        ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: Some(auth),
            complexity_boost: 0.0,
        }
    }

    fn make_profile(complexity: f32) -> TaskProfile {
        TaskProfile {
            task_type: TaskType::Chat,
            complexity,
            keywords: vec![],
        }
    }

    fn admin_permissions() -> UserPermissions {
        UserPermissions {
            level: 2,
            max_tier: "elite".into(),
            escalation_allowed: true,
            escalation_threshold: 0.0,
            rate_limit: 0,
            cost_budget_daily_usd: 0.0,
            cost_budget_monthly_usd: 0.0,
            ..UserPermissions::default()
        }
    }

    fn user_permissions() -> UserPermissions {
        UserPermissions {
            level: 1,
            max_tier: "standard".into(),
            escalation_allowed: true,
            escalation_threshold: 0.6,
            rate_limit: 60,
            cost_budget_daily_usd: 5.0,
            cost_budget_monthly_usd: 50.0,
            ..UserPermissions::default()
        }
    }

    fn make_auth(sender: &str, permissions: UserPermissions) -> AuthContext {
        AuthContext {
            sender_id: sender.into(),
            channel: "test".into(),
            permissions,
        }
    }

    /// Mock cost tracker that can be configured to deny or approve.
    struct MockCostTracker {
        deny: Mutex<bool>,
    }

    impl MockCostTracker {
        fn new(deny: bool) -> Self {
            Self {
                deny: Mutex::new(deny),
            }
        }
    }

    impl CostTrackable for MockCostTracker {
        fn check_budget(
            &self,
            _sender_id: &str,
            _estimated_cost: f64,
            _daily_limit: f64,
            _monthly_limit: f64,
        ) -> BudgetResult {
            if *self.deny.lock().unwrap() {
                BudgetResult::DailyLimitExceeded {
                    spent: 10.0,
                    limit: 5.0,
                }
            } else {
                BudgetResult::Approved
            }
        }

        fn record_estimated(&self, _sender_id: &str, _estimated_cost: f64) {}
        fn record_actual(&self, _sender_id: &str, _estimated_cost: f64, _actual_cost: f64) {}
    }

    /// Mock rate limiter that can be configured to deny or approve.
    struct MockRateLimiter {
        allow: bool,
    }

    impl RateLimitable for MockRateLimiter {
        fn check(&self, _sender_id: &str, _limit: u32) -> bool {
            self.allow
        }
    }

    // ── Tier matching tests ─────────────────────────────────────────

    #[test]
    fn tier_matches_complexity_in_range() {
        let tier = ModelTier {
            name: "standard".into(),
            models: vec![],
            complexity_range: [0.0, 0.7],
            cost_per_1k_tokens: 0.001,
            max_context_tokens: 8192,
            ordinal: 0,
        };
        assert!(tier.matches_complexity(0.5));
    }

    #[test]
    fn tier_matches_complexity_at_lower_bound() {
        let tier = ModelTier {
            name: "free".into(),
            models: vec![],
            complexity_range: [0.0, 0.3],
            cost_per_1k_tokens: 0.0,
            max_context_tokens: 8192,
            ordinal: 0,
        };
        assert!(tier.matches_complexity(0.0));
    }

    #[test]
    fn tier_matches_complexity_at_upper_bound() {
        let tier = ModelTier {
            name: "standard".into(),
            models: vec![],
            complexity_range: [0.0, 0.7],
            cost_per_1k_tokens: 0.001,
            max_context_tokens: 8192,
            ordinal: 0,
        };
        assert!(tier.matches_complexity(0.7));
    }

    #[test]
    fn tier_does_not_match_below_range() {
        let tier = ModelTier {
            name: "premium".into(),
            models: vec![],
            complexity_range: [0.3, 1.0],
            cost_per_1k_tokens: 0.01,
            max_context_tokens: 8192,
            ordinal: 0,
        };
        assert!(!tier.matches_complexity(0.0));
    }

    #[test]
    fn tier_does_not_match_above_range() {
        let tier = ModelTier {
            name: "free".into(),
            models: vec![],
            complexity_range: [0.0, 0.3],
            cost_per_1k_tokens: 0.0,
            max_context_tokens: 8192,
            ordinal: 0,
        };
        assert!(!tier.matches_complexity(1.0));
    }

    // ── Tier selection tests ────────────────────────────────────────

    #[test]
    fn select_tier_picks_highest_quality_match() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = admin_permissions();
        let allowed = router.filter_tiers_by_permissions(&perms);
        let selection = router.select_tier(0.5, &allowed, &perms);
        // Both standard [0.0-0.7] and premium [0.3-1.0] match 0.5;
        // premium is higher ordinal.
        assert_eq!(selection.tier.name, "premium");
        assert!(!selection.escalated);
    }

    #[test]
    fn select_tier_with_single_matching_tier() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = admin_permissions();
        let allowed = router.filter_tiers_by_permissions(&perms);
        let selection = router.select_tier(0.1, &allowed, &perms);
        // Only free [0.0-0.3] and standard [0.0-0.7] match 0.1
        // standard is highest ordinal.
        assert_eq!(selection.tier.name, "standard");
    }

    #[test]
    fn select_tier_falls_back_to_highest_allowed() {
        // Use tiers with non-overlapping ranges and a gap
        let config = make_config(vec![
            make_tier_config("low", vec!["a/m1"], [0.0, 0.2], 0.0),
            make_tier_config("high", vec!["a/m2"], [0.8, 1.0], 0.01),
        ]);
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "high".into(),
            escalation_allowed: false,
            ..admin_permissions()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        // Complexity 0.5 matches neither tier
        let selection = router.select_tier(0.5, &allowed, &perms);
        // Falls back to highest allowed tier
        assert_eq!(selection.tier.name, "high");
    }

    // ── Escalation tests ────────────────────────────────────────────

    #[test]
    fn escalation_promotes_to_next_tier() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "standard".into(),
            escalation_allowed: true,
            escalation_threshold: 0.6,
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        // Complexity 0.8 doesn't match standard [0.0-0.7], but does match
        // premium [0.3-1.0] which is one tier above. Escalation should fire.
        let selection = router.select_tier(0.8, &allowed, &perms);
        assert_eq!(selection.tier.name, "premium");
        assert!(selection.escalated);
    }

    #[test]
    fn escalation_respects_max_escalation_tiers() {
        let mut config = make_config(standard_tiers());
        config.escalation.max_escalation_tiers = 1;
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "standard".into(),
            escalation_allowed: true,
            escalation_threshold: 0.6,
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        // max_escalation_tiers=1 means can only go to premium (1 above standard),
        // not elite (2 above). For complexity 0.8, premium matches.
        let selection = router.select_tier(0.8, &allowed, &perms);
        assert_eq!(selection.tier.name, "premium");
        assert!(selection.escalated);
    }

    #[test]
    fn escalation_denied_when_not_allowed() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "standard".into(),
            escalation_allowed: false,
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        let selection = router.select_tier(0.9, &allowed, &perms);
        // Escalation not allowed, so falls back to highest allowed tier (standard)
        assert_eq!(selection.tier.name, "standard");
        assert!(!selection.escalated);
    }

    #[test]
    fn escalation_denied_below_threshold() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "free".into(),
            escalation_allowed: true,
            escalation_threshold: 0.9,
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        // Complexity 0.5 is below threshold 0.9, so no escalation
        let selection = router.select_tier(0.5, &allowed, &perms);
        assert!(!selection.escalated);
    }

    #[test]
    fn escalation_denied_when_config_disabled() {
        let mut config = make_config(standard_tiers());
        config.escalation.enabled = false;
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "standard".into(),
            escalation_allowed: true,
            escalation_threshold: 0.6,
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        let selection = router.select_tier(0.9, &allowed, &perms);
        assert!(!selection.escalated);
    }

    // ── Budget constraint tests ─────────────────────────────────────

    #[tokio::test]
    async fn budget_unlimited_skips_check() {
        let config = make_config(standard_tiers());
        let tracker = Arc::new(MockCostTracker::new(true)); // would deny
        let router = TieredRouter::new(config).with_cost_tracker(tracker);
        let perms = UserPermissions {
            cost_budget_daily_usd: 0.0,
            cost_budget_monthly_usd: 0.0,
            ..admin_permissions()
        };
        let auth = make_auth("alice", perms.clone());
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.5)).await;
        // Budget is unlimited (0.0), so the denial tracker is bypassed
        assert!(!decision.budget_constrained);
    }

    #[tokio::test]
    async fn budget_sufficient_allows_tier() {
        let config = make_config(standard_tiers());
        let tracker = Arc::new(MockCostTracker::new(false)); // approves
        let router = TieredRouter::new(config).with_cost_tracker(tracker);
        let perms = UserPermissions {
            cost_budget_daily_usd: 10.0,
            cost_budget_monthly_usd: 100.0,
            ..admin_permissions()
        };
        let auth = make_auth("alice", perms);
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.5)).await;
        assert!(!decision.budget_constrained);
        assert_eq!(decision.tier.as_deref(), Some("premium"));
    }

    #[tokio::test]
    async fn budget_insufficient_downgrades_tier() {
        // Create a cost tracker that denies premium but approves standard
        struct SelectiveDenyTracker;
        impl CostTrackable for SelectiveDenyTracker {
            fn check_budget(
                &self,
                _sender_id: &str,
                estimated_cost: f64,
                _daily_limit: f64,
                _monthly_limit: f64,
            ) -> BudgetResult {
                if estimated_cost > 0.005 {
                    BudgetResult::DailyLimitExceeded {
                        spent: 4.99,
                        limit: 5.0,
                    }
                } else {
                    BudgetResult::Approved
                }
            }
            fn record_estimated(&self, _: &str, _: f64) {}
            fn record_actual(&self, _: &str, _: f64, _: f64) {}
        }

        let config = make_config(standard_tiers());
        let tracker = Arc::new(SelectiveDenyTracker);
        let router = TieredRouter::new(config).with_cost_tracker(tracker);
        let perms = UserPermissions {
            cost_budget_daily_usd: 5.0,
            cost_budget_monthly_usd: 50.0,
            ..admin_permissions()
        };
        let auth = make_auth("alice", perms);
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.5)).await;
        // Premium (0.01) is too expensive, should downgrade to standard (0.001)
        assert!(decision.budget_constrained);
        assert_eq!(decision.tier.as_deref(), Some("standard"));
    }

    #[tokio::test]
    async fn budget_exhausted_uses_cheapest_tier() {
        let config = make_config(standard_tiers());
        let tracker = Arc::new(MockCostTracker::new(true)); // denies all
        let router = TieredRouter::new(config).with_cost_tracker(tracker);
        let perms = UserPermissions {
            cost_budget_daily_usd: 5.0,
            cost_budget_monthly_usd: 50.0,
            ..admin_permissions()
        };
        let auth = make_auth("alice", perms);
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.5)).await;
        assert!(decision.budget_constrained);
        assert_eq!(decision.tier.as_deref(), Some("free"));
    }

    // ── Selection strategy tests ────────────────────────────────────

    #[test]
    fn preference_order_picks_first_model() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let tier = &router.tiers[1]; // standard with 2 models
        let perms = admin_permissions();
        let (_, model) = router.select_model(tier, &perms).unwrap();
        assert_eq!(model, "claude-haiku-3.5");
    }

    #[test]
    fn round_robin_rotates_models() {
        let mut config = make_config(standard_tiers());
        config.selection_strategy = Some(TierSelectionStrategy::RoundRobin);
        let router = TieredRouter::new(config);
        let tier = &router.tiers[1]; // standard: 2 models
        let perms = admin_permissions();
        let (_, m1) = router.select_model(tier, &perms).unwrap();
        let (_, m2) = router.select_model(tier, &perms).unwrap();
        let (_, m3) = router.select_model(tier, &perms).unwrap();
        // Should cycle: m1, m2, m1
        assert_ne!(m1, m2);
        assert_eq!(m1, m3);
    }

    #[test]
    fn lowest_cost_picks_first_model() {
        let mut config = make_config(standard_tiers());
        config.selection_strategy = Some(TierSelectionStrategy::LowestCost);
        let router = TieredRouter::new(config);
        let tier = &router.tiers[1]; // standard
        let perms = admin_permissions();
        let (_, model) = router.select_model(tier, &perms).unwrap();
        // Within a tier, all models share cost, so first is picked.
        assert_eq!(model, "claude-haiku-3.5");
    }

    #[test]
    fn random_returns_valid_model() {
        let mut config = make_config(standard_tiers());
        config.selection_strategy = Some(TierSelectionStrategy::Random);
        let router = TieredRouter::new(config);
        let tier = &router.tiers[1]; // standard
        let perms = admin_permissions();
        let (provider, model) = router.select_model(tier, &perms).unwrap();
        // Should return one of the tier's models
        let full_name = format!("{}/{}", provider, model);
        assert!(
            tier.models.contains(&full_name),
            "model {} not in tier models {:?}",
            full_name,
            tier.models
        );
    }

    // ── Permission filtering tests ──────────────────────────────────

    #[test]
    fn filter_tiers_by_max_tier() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "standard".into(),
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        let names: Vec<&str> = allowed.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["free", "standard"]);
    }

    #[test]
    fn filter_tiers_unknown_max_tier_allows_all() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "nonexistent".into(),
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        // Unknown tier name -> allow all tiers (e.g. admin with "elite"
        // max_tier when config only defines free/standard/premium/elite).
        assert_eq!(allowed.len(), 4);
    }

    #[test]
    fn model_access_allowlist_filters() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            model_access: vec!["anthropic/*".into()],
            ..admin_permissions()
        };
        let tier = &router.tiers[1]; // standard: anthropic/claude-haiku-3.5, openai/gpt-4o-mini
        let available = router.filter_models_by_permissions(&tier.models, &perms);
        assert_eq!(available, vec!["anthropic/claude-haiku-3.5"]);
    }

    #[test]
    fn model_denylist_filters() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            model_denylist: vec!["openai/gpt-4o-mini".into()],
            ..admin_permissions()
        };
        let tier = &router.tiers[1]; // standard
        let available = router.filter_models_by_permissions(&tier.models, &perms);
        assert_eq!(available, vec!["anthropic/claude-haiku-3.5"]);
    }

    #[test]
    fn empty_model_access_allows_all() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            model_access: vec![],
            ..admin_permissions()
        };
        let tier = &router.tiers[1]; // standard: 2 models
        let available = router.filter_models_by_permissions(&tier.models, &perms);
        assert_eq!(available.len(), 2);
    }

    // ── Fallback chain tests ────────────────────────────────────────

    #[test]
    fn fallback_to_lower_tier() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        // Premium tier with models denied by denylist
        let primary = &router.tiers[2]; // premium
        let perms = UserPermissions {
            model_denylist: vec!["anthropic/claude-sonnet-4".into(), "openai/gpt-4o".into()],
            ..admin_permissions()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        let result = router.fallback_chain(primary, &allowed, &perms);
        assert!(result.is_some());
        let (_, _, reason) = result.unwrap();
        assert!(reason.contains("fallback"));
    }

    #[test]
    fn fallback_to_fallback_model() {
        // All tiers have denied models
        let config = make_config(vec![make_tier_config(
            "only",
            vec!["denied/model"],
            [0.0, 1.0],
            0.0,
        )]);
        let router = TieredRouter::new(config);
        let primary = &router.tiers[0];
        let perms = UserPermissions {
            model_denylist: vec!["denied/model".into()],
            max_tier: "only".into(),
            ..admin_permissions()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        let result = router.fallback_chain(primary, &allowed, &perms);
        assert!(result.is_some());
        let (provider, model, reason) = result.unwrap();
        assert_eq!(provider, "groq");
        assert_eq!(model, "llama-3.1-8b");
        assert!(reason.contains("fallback_model"));
    }

    #[test]
    fn fallback_returns_none_when_no_fallback() {
        let mut config = make_config(vec![make_tier_config(
            "only",
            vec!["denied/model"],
            [0.0, 1.0],
            0.0,
        )]);
        config.fallback_model = None;
        let router = TieredRouter::new(config);
        let primary = &router.tiers[0];
        let perms = UserPermissions {
            model_denylist: vec!["denied/model".into()],
            max_tier: "only".into(),
            ..admin_permissions()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        let result = router.fallback_chain(primary, &allowed, &perms);
        assert!(result.is_none());
    }

    #[test]
    fn fallback_model_denied_above_max_tier() {
        // Fallback model is in premium, but user only has access to free
        let mut config = make_config(standard_tiers());
        config.fallback_model = Some("anthropic/claude-sonnet-4".into());
        let router = TieredRouter::new(config);
        let primary = &router.tiers[0]; // free
        let perms = UserPermissions {
            model_denylist: vec!["groq/llama-3.1-8b".into()],
            max_tier: "free".into(),
            ..UserPermissions::default()
        };
        let allowed = router.filter_tiers_by_permissions(&perms);
        let result = router.fallback_chain(primary, &allowed, &perms);
        // Fallback model is in premium (ordinal 2), user max is free (ordinal 0)
        // Should be denied
        assert!(result.is_none());
    }

    #[test]
    fn rate_limited_fallback_denied_above_max_tier() {
        let mut config = make_config(standard_tiers());
        config.fallback_model = Some("anthropic/claude-opus-4".into());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "free".into(),
            ..UserPermissions::default()
        };
        let decision = router.rate_limited_decision(&perms);
        // Fallback model is in elite tier, user is free tier -- denied
        assert!(decision.provider.is_empty());
        assert!(decision.reason.contains("not permitted"));
    }

    // ── Full route() integration tests ──────────────────────────────

    #[tokio::test]
    async fn route_low_complexity_to_free_tier() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "free".into(),
            ..UserPermissions::default()
        };
        let auth = make_auth("user1", perms);
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.1)).await;
        assert_eq!(decision.tier.as_deref(), Some("free"));
        assert_eq!(decision.model, "llama-3.1-8b");
        assert!(!decision.escalated);
    }

    #[tokio::test]
    async fn route_high_complexity_to_premium() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let auth = make_auth("admin1", admin_permissions());
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.9)).await;
        // elite [0.7-1.0] is highest matching tier for admin
        assert_eq!(decision.tier.as_deref(), Some("elite"));
        assert!(!decision.escalated);
    }

    #[tokio::test]
    async fn route_no_auth_context_uses_zero_trust() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let req = make_request(); // no auth_context
        let decision = router.route(&req, &make_profile(0.1)).await;
        // Zero-trust defaults to max_tier="free"
        assert_eq!(decision.tier.as_deref(), Some("free"));
    }

    #[tokio::test]
    async fn route_rate_limited_returns_fallback() {
        let config = make_config(standard_tiers());
        let limiter = Arc::new(MockRateLimiter { allow: false });
        let router = TieredRouter::new(config).with_rate_limiter(limiter);
        let perms = user_permissions();
        let auth = make_auth("user1", perms);
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.5)).await;
        assert!(decision.reason.contains("rate limited"));
    }

    #[tokio::test]
    async fn route_update_does_not_panic() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let decision = RoutingDecision {
            provider: "test".into(),
            model: "test".into(),
            reason: "test".into(),
            cost_estimate_usd: Some(0.01),
            ..Default::default()
        };
        let outcome = ResponseOutcome {
            success: true,
            quality: crate::pipeline::traits::QualityScore {
                overall: 1.0,
                relevance: 1.0,
                coherence: 1.0,
            },
            latency_ms: 100,
        };
        router.update(&decision, &outcome);
    }

    // ── Edge case tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn empty_tiers_config_uses_fallback() {
        let mut config = make_config(vec![]);
        config.fallback_model = Some("groq/llama-3.1-8b".into());
        let router = TieredRouter::new(config);
        let req = make_request();
        let decision = router.route(&req, &make_profile(0.5)).await;
        assert_eq!(decision.provider, "groq");
        assert_eq!(decision.model, "llama-3.1-8b");
        assert!(decision.reason.contains("no tiers"));
    }

    #[tokio::test]
    async fn single_tier_config_always_uses_it() {
        let config = make_config(vec![make_tier_config(
            "only",
            vec!["test/model"],
            [0.0, 1.0],
            0.0,
        )]);
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            max_tier: "only".into(),
            ..admin_permissions()
        };
        let auth = make_auth("user1", perms);
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.1)).await;
        assert_eq!(decision.tier.as_deref(), Some("only"));
        let decision2 = router.route(&req, &make_profile(0.9)).await;
        assert_eq!(decision2.tier.as_deref(), Some("only"));
    }

    #[test]
    fn model_matches_pattern_exact() {
        assert!(model_matches_pattern(
            "anthropic/claude-haiku-3.5",
            "anthropic/claude-haiku-3.5"
        ));
    }

    #[test]
    fn model_matches_pattern_wildcard() {
        assert!(model_matches_pattern(
            "anthropic/claude-haiku-3.5",
            "anthropic/*"
        ));
    }

    #[test]
    fn model_matches_pattern_star() {
        assert!(model_matches_pattern("anything/at_all", "*"));
    }

    #[test]
    fn model_matches_pattern_no_match() {
        assert!(!model_matches_pattern("openai/gpt-4o", "anthropic/*"));
    }

    #[tokio::test]
    async fn missing_auth_context_defaults_to_zero_trust() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let req = make_request(); // auth_context = None
        let decision = router.route(&req, &make_profile(0.1)).await;
        // Zero-trust defaults: max_tier="free"
        assert_eq!(decision.tier.as_deref(), Some("free"));
    }

    #[tokio::test]
    async fn empty_tiers_no_fallback() {
        let mut config = make_config(vec![]);
        config.fallback_model = None;
        let router = TieredRouter::new(config);
        let req = make_request();
        let decision = router.route(&req, &make_profile(0.5)).await;
        assert!(decision.provider.is_empty());
        assert!(decision.reason.contains("no tiers"));
    }

    // ── WEFT-27: tier check on fallback model selection ─────────────
    //
    // Ensures that EVERY fallback selection branch — including the
    // no-tiers-available terminal path — gates the configured
    // `routing.fallback_model` against the caller's `max_tier`. A
    // misconfigured fallback (e.g. `anthropic/claude-opus-4-5`) must
    // never be served to a zero-trust caller just because their tier
    // filter excluded the entire tier list. This is the missing
    // half of FIX-06.

    #[test]
    fn weft27_no_tiers_available_denies_fallback_above_max_tier() {
        // Tier list contains an elite tier; user has only `free` access.
        // The configured fallback model lives in `elite`. If the user's
        // permission filter empties the allowed list (e.g. workspace
        // ceiling clamps everything away), the no-tiers path must NOT
        // hand them the elite fallback.
        let mut config = make_config(standard_tiers());
        config.fallback_model = Some("anthropic/claude-opus-4".into());
        let router = TieredRouter::new(config);

        let perms = UserPermissions {
            // `unknown_tier` is not in the tier_index → max_ordinal = 0.
            // Combined with model_denylist of every tier's primary model,
            // filter_tiers_by_permissions returns an empty list.
            max_tier: "free".into(),
            ..UserPermissions::default()
        };
        let decision = router.no_tiers_available_decision(Some(&perms));

        // The fallback (claude-opus-4 / elite ordinal 3) must be denied
        // because user max_tier is `free` (ordinal 0).
        assert!(
            decision.provider.is_empty(),
            "fallback above max_tier must NOT be returned"
        );
        assert!(decision.reason.contains("not permitted"));
    }

    #[test]
    fn weft27_no_tiers_available_allows_fallback_within_max_tier() {
        // Same setup but the fallback model lives in the `free` tier
        // (ordinal 0) — equal to the user's max_tier — so the gate
        // must permit it.
        let mut config = make_config(standard_tiers());
        config.fallback_model = Some("groq/llama-3.1-8b".into());
        let router = TieredRouter::new(config);

        let perms = UserPermissions {
            max_tier: "free".into(),
            ..UserPermissions::default()
        };
        let decision = router.no_tiers_available_decision(Some(&perms));
        assert_eq!(decision.provider, "groq");
        assert_eq!(decision.model, "llama-3.1-8b");
    }

    #[test]
    fn weft27_no_tiers_no_permissions_falls_back_permissively() {
        // Legacy callers (no permissions argument) keep the previous
        // behaviour — the fallback is returned unconditionally.
        let mut config = make_config(standard_tiers());
        config.fallback_model = Some("anthropic/claude-opus-4".into());
        let router = TieredRouter::new(config);
        let decision = router.no_tiers_available_decision(None);
        assert_eq!(decision.provider, "anthropic");
        assert_eq!(decision.model, "claude-opus-4");
    }

    // ── WEFT-31: model_override audit ───────────────────────────────

    #[tokio::test]
    async fn weft31_model_override_emits_audit_and_uses_override_model() {
        // Drain any chain events left over from earlier tests in this
        // process so the assertion below is deterministic.
        let _ = crate::chain_event::drain_pending_chain_events();

        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            level: 2,
            max_tier: "elite".into(),
            model_override: true,
            ..admin_permissions()
        };
        let auth = make_auth("admin1", perms);
        let mut req = make_request_with_auth(auth);
        // Request explicitly asks for a model that is OUTSIDE any tier.
        req.model = Some("custom-provider/exotic-model".into());

        let decision = router.route(&req, &make_profile(0.5)).await;

        // The override model is used verbatim.
        assert_eq!(decision.provider, "custom-provider");
        assert_eq!(decision.model, "exotic-model");
        // No tier is attached — we bypassed the tier selector entirely.
        assert!(decision.tier.is_none());
        // Reason captures the bypass for operator-debug logging
        // (it stays internal; redacted_reason() classifies it
        // through the catch-all fallback category).
        assert!(decision.reason.contains("model_override"));

        // A chain event was pushed for governance review.
        let events = crate::chain_event::drain_pending_chain_events();
        let bypass_event = events
            .iter()
            .find(|e| e.kind == "model_override_bypass")
            .expect("model_override_bypass event must be emitted");
        assert_eq!(bypass_event.source, "routing");
    }

    #[tokio::test]
    async fn weft31_model_override_off_does_not_bypass() {
        // Same setup but `model_override: false` — the request's
        // explicit `model` field MUST be ignored and normal tier
        // routing must run.
        let _ = crate::chain_event::drain_pending_chain_events();

        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let perms = UserPermissions {
            model_override: false,
            ..admin_permissions()
        };
        let auth = make_auth("admin2", perms);
        let mut req = make_request_with_auth(auth);
        req.model = Some("custom-provider/exotic-model".into());

        let decision = router.route(&req, &make_profile(0.9)).await;

        // Tier routing should pick `elite` — NOT the requested override.
        assert_ne!(decision.model, "exotic-model");
        assert!(decision.tier.is_some());

        // No bypass audit event was emitted.
        let events = crate::chain_event::drain_pending_chain_events();
        assert!(
            !events.iter().any(|e| e.kind == "model_override_bypass"),
            "no audit event should fire when model_override=false"
        );
    }

    // ── WEFT-52: admin user x restricted channel ────────────────────
    //
    // CONS-007 settled "channel overrides beat user overrides" as a
    // general rule (test_channel_overrides_beat_user_overrides in
    // permissions.rs). This test pins the specific edge case the
    // 0.7.0 audit flagged: an admin (level 2) hitting a channel whose
    // override clamps the level to `user`. The decision matrix says
    // the channel override wins; this test makes that contract
    // explicit at the integration level (router + resolver) so a
    // future refactor can't quietly invert the priority.
    #[tokio::test]
    async fn weft52_admin_in_restricted_channel_is_clamped_to_channel_level() {
        use crate::pipeline::permissions::PermissionResolver;
        use clawft_types::routing::{PermissionLevelConfig, PermissionsConfig, RoutingConfig};
        use std::collections::HashMap;

        // alice is configured as level=2 (admin) globally.
        let mut users = HashMap::new();
        users.insert(
            "alice".into(),
            PermissionLevelConfig {
                level: Some(2),
                ..PermissionLevelConfig::default()
            },
        );
        // The #general channel is restricted to level=1 (user) and the
        // free tier — model_override turned OFF + escalation off so the
        // channel really is a hard tier ceiling (no upgrades, period).
        let mut channels = HashMap::new();
        channels.insert(
            "general".into(),
            PermissionLevelConfig {
                level: Some(1),
                max_tier: Some("free".into()),
                model_override: Some(false),
                escalation_allowed: Some(false),
                ..PermissionLevelConfig::default()
            },
        );
        let cfg = RoutingConfig {
            permissions: PermissionsConfig {
                users,
                channels,
                ..Default::default()
            },
            ..RoutingConfig::default()
        };

        // Resolve permissions for admin in restricted channel.
        let resolver = PermissionResolver::new(&cfg, None);
        let perms = resolver.resolve("alice", "general", false);

        // Channel override wins: admin is clamped to user level + free
        // tier and model_override is denied.
        assert_eq!(
            perms.level, 1,
            "channel level override must beat user override"
        );
        assert_eq!(perms.max_tier, "free");
        assert!(
            !perms.model_override,
            "channel override must beat user override on model_override too"
        );

        // Now route the request — even with `request.model` set, the
        // bypass MUST NOT fire (because the resolved permissions have
        // model_override=false from the channel clamp).
        let _ = crate::chain_event::drain_pending_chain_events();
        let router_cfg = make_config(standard_tiers());
        let router = TieredRouter::new(router_cfg);
        let auth = make_auth("alice", perms);
        let mut req = make_request_with_auth(auth);
        req.model = Some("anthropic/claude-opus-4".into()); // elite

        // Use a complexity that maps cleanly into the `free` tier
        // (`[0.0, 0.3]`) so the test depends only on the channel-tier
        // clamp and the model_override gate, not on escalation
        // tie-breaks.
        let decision = router.route(&req, &make_profile(0.1)).await;

        // The elite override MUST be denied — the routing must pick
        // a tier within the channel-clamped max_tier (`free`).
        assert_ne!(decision.model, "claude-opus-4");
        assert_eq!(decision.tier.as_deref(), Some("free"));
        // No bypass event because model_override was clamped off.
        let events = crate::chain_event::drain_pending_chain_events();
        assert!(!events.iter().any(|e| e.kind == "model_override_bypass"));
    }

    #[tokio::test]
    async fn weft27_route_zero_trust_denied_dangerous_fallback() {
        // End-to-end: route() with a zero-trust caller whose max_tier
        // sits below the configured fallback model. The route must
        // surface a deny rather than the high-tier model.
        let mut config = make_config(standard_tiers());
        // Strip free-tier models out of the user's allowlist so the
        // primary model selection comes back empty and we descend into
        // the fallback chain. Use a denylist on every non-fallback
        // model in the free tier.
        config.fallback_model = Some("anthropic/claude-opus-4".into());
        let router = TieredRouter::new(config);

        let perms = UserPermissions {
            max_tier: "free".into(),
            // Deny every model in every tier so select_model returns None
            // and fallback_chain is invoked. After fallback_chain rejects
            // the elite fallback, the route falls through to
            // no_tiers_available_decision (or the chain's own deny path).
            model_denylist: vec![
                "groq/llama-3.1-8b".into(),
                "anthropic/claude-haiku-3.5".into(),
                "openai/gpt-4o-mini".into(),
                "anthropic/claude-sonnet-4".into(),
                "openai/gpt-4o".into(),
                "anthropic/claude-opus-4".into(),
            ],
            ..UserPermissions::default()
        };
        let auth = make_auth("zt-user", perms);
        let req = make_request_with_auth(auth);
        let decision = router.route(&req, &make_profile(0.1)).await;

        // The high-tier fallback model MUST NOT be returned.
        assert_ne!(
            decision.model, "claude-opus-4",
            "tier check on fallback path is not firing"
        );
    }

    #[test]
    fn split_provider_model_with_slash() {
        let (p, m) = split_provider_model("anthropic/claude-opus-4");
        assert_eq!(p, "anthropic");
        assert_eq!(m, "claude-opus-4");
    }

    #[test]
    fn split_provider_model_without_slash() {
        let (p, m) = split_provider_model("gpt-4o");
        assert_eq!(p, "openai");
        assert_eq!(m, "gpt-4o");
    }

    #[test]
    fn noop_cost_tracker_always_approves() {
        let tracker = NoopCostTracker;
        let result = tracker.check_budget("any", 100.0, 1.0, 1.0);
        assert_eq!(result, BudgetResult::Approved);
    }

    #[test]
    fn noop_rate_limiter_always_allows() {
        let limiter = NoopRateLimiter;
        assert!(limiter.check("any", 1));
    }

    #[test]
    fn debug_impl_does_not_panic() {
        let config = make_config(standard_tiers());
        let router = TieredRouter::new(config);
        let debug_str = format!("{:?}", router);
        assert!(debug_str.contains("TieredRouter"));
    }
}
