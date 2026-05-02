//! Constitutional governance engine for WeftOS.
//!
//! Implements the three-branch governance model where:
//! - **Legislative** (SOPs, rules, manifests) defines boundaries
//! - **Executive** (agents) acts within defined boundaries
//! - **Judicial** (CGR engine) validates every action
//!
//! No branch can modify another's constraints. Governance violations
//! are type-level impossibilities, not merely audited events.
//!
//! # Design
//!
//! All types compile unconditionally. The CGR validation engine and
//! effect algebra scoring require the `governance` or `ruvector-apps`
//! feature gates. Without them, `GovernanceEngine::evaluate()` returns
//! `GovernanceDecision::Permit` (open governance).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::environment::{Environment, EnvironmentClass};

/// A governance rule that restricts agent behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceRule {
    /// Unique rule identifier.
    pub id: String,

    /// Human-readable rule description.
    pub description: String,

    /// Which branch defined this rule.
    pub branch: GovernanceBranch,

    /// Rule severity (how critical the violation is).
    pub severity: RuleSeverity,

    /// Whether this rule is currently active.
    #[serde(default = "default_true")]
    pub active: bool,

    /// SOP reference URL for agents to consult for full procedure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_url: Option<String>,

    /// SOP category tag for filtering rules by domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sop_category: Option<String>,
}

impl GovernanceRule {
    /// Get rules by SOP category from a slice of rules.
    pub fn filter_by_category<'a>(rules: &'a [GovernanceRule], category: &str) -> Vec<&'a GovernanceRule> {
        rules.iter().filter(|r| r.sop_category.as_deref() == Some(category)).collect()
    }
}

fn default_true() -> bool {
    true
}

/// Governance branch that owns a rule.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GovernanceBranch {
    /// Rules from SOPs, genesis protocol, weftapp.toml.
    Legislative,
    /// Rules from agent execution policies.
    Executive,
    /// Rules from CGR validation engine.
    Judicial,
}

impl std::fmt::Display for GovernanceBranch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GovernanceBranch::Legislative => write!(f, "legislative"),
            GovernanceBranch::Executive => write!(f, "executive"),
            GovernanceBranch::Judicial => write!(f, "judicial"),
        }
    }
}

/// Rule violation severity.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RuleSeverity {
    /// Advisory -- logged but not enforced.
    Advisory,
    /// Warning -- logged and flagged, action proceeds.
    Warning,
    /// Blocking -- action is prevented.
    Blocking,
    /// Critical -- action prevented and agent capability may be revoked.
    Critical,
}

impl std::fmt::Display for RuleSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSeverity::Advisory => write!(f, "advisory"),
            RuleSeverity::Warning => write!(f, "warning"),
            RuleSeverity::Blocking => write!(f, "blocking"),
            RuleSeverity::Critical => write!(f, "critical"),
        }
    }
}

/// 5-dimensional effect vector for scoring agent actions.
///
/// Each dimension is scored from 0.0 (no impact) to 1.0 (maximum impact).
/// The magnitude of the vector determines whether an action exceeds
/// the environment's governance threshold.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EffectVector {
    /// Risk score: probability of negative outcome.
    #[serde(default)]
    pub risk: f64,

    /// Fairness score: impact on equitable treatment.
    #[serde(default)]
    pub fairness: f64,

    /// Privacy score: impact on data privacy.
    #[serde(default)]
    pub privacy: f64,

    /// Novelty score: how unprecedented the action is.
    #[serde(default)]
    pub novelty: f64,

    /// Security score: impact on system security.
    #[serde(default)]
    pub security: f64,
}

impl EffectVector {
    /// Compute the magnitude of the effect vector (L2 norm).
    ///
    /// Preserved as the stable hardcoded scalar. The EML-backed variant
    /// is [`Self::score`], which consults an optional
    /// [`GovernanceScorerModel`](crate::eml_kernel::GovernanceScorerModel)
    /// and falls back to this value when the model is untrained.
    pub fn magnitude(&self) -> f64 {
        (self.risk * self.risk
            + self.fairness * self.fairness
            + self.privacy * self.privacy
            + self.novelty * self.novelty
            + self.security * self.security)
            .sqrt()
    }

    /// Compute the composite governance score for this effect vector.
    ///
    /// When `model` is `None` or untrained, returns
    /// [`Self::magnitude`] unchanged (bit-for-bit identical to the
    /// pre-EML behaviour). When the model is trained, delegates to
    /// [`GovernanceScorerModel::predict`] which combines the five
    /// dimensions into a learned scalar.
    ///
    /// NOTE(eml-swap): wired — Finding #5 (GovernanceScorerModel).
    pub fn score(
        &self,
        model: Option<&crate::eml_kernel::GovernanceScorerModel>,
    ) -> f64 {
        match model {
            Some(m) => m.predict(
                self.risk,
                self.fairness,
                self.privacy,
                self.novelty,
                self.security,
            ),
            None => self.magnitude(),
        }
    }

    /// Check if any dimension exceeds a threshold.
    pub fn any_exceeds(&self, threshold: f64) -> bool {
        self.risk > threshold
            || self.fairness > threshold
            || self.privacy > threshold
            || self.novelty > threshold
            || self.security > threshold
    }

    /// Get the maximum dimension value.
    pub fn max_dimension(&self) -> f64 {
        [
            self.risk,
            self.fairness,
            self.privacy,
            self.novelty,
            self.security,
        ]
        .into_iter()
        .fold(0.0_f64, f64::max)
    }
}

/// Governance decision for an action.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GovernanceDecision {
    /// Action is permitted.
    Permit,
    /// Action is permitted with advisory note.
    PermitWithWarning(String),
    /// Action requires human approval before proceeding.
    EscalateToHuman(String),
    /// Action is denied.
    Deny(String),
}

impl std::fmt::Display for GovernanceDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GovernanceDecision::Permit => write!(f, "permit"),
            GovernanceDecision::PermitWithWarning(msg) => write!(f, "permit (warning: {msg})"),
            GovernanceDecision::EscalateToHuman(msg) => write!(f, "escalate ({msg})"),
            GovernanceDecision::Deny(reason) => write!(f, "deny: {reason}"),
        }
    }
}

/// Governance evaluation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceRequest {
    /// Agent identifier making the request.
    pub agent_id: String,

    /// Action being requested.
    pub action: String,

    /// Computed effect vector for the action.
    #[serde(default)]
    pub effect: EffectVector,

    /// Additional context for the evaluator.
    #[serde(default)]
    pub context: std::collections::HashMap<String, String>,

    /// Node ID of the requesting node (for distributed governance in K6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

impl GovernanceRequest {
    /// Create a new governance request.
    pub fn new(agent_id: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            action: action.into(),
            effect: EffectVector::default(),
            context: std::collections::HashMap::new(),
            node_id: None,
        }
    }

    /// Set the node ID for distributed governance evaluation.
    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    /// Set the effect vector.
    pub fn with_effect(mut self, effect: EffectVector) -> Self {
        self.effect = effect;
        self
    }

    /// Enrich the request with tool execution context (k3:D2).
    ///
    /// Sets the `context` map with tool-specific fields so governance
    /// rules can distinguish between different tool invocations even
    /// when the action string is the generic `"tool.exec"`.
    ///
    /// # Fields set
    ///
    /// - `tool` — tool name (e.g. `"fs.read_file"`)
    /// - `gate_action` — the per-tool gate action from the catalog
    /// - `pid` — stringified PID of the requesting agent
    ///
    /// The `effect` field is set from the tool's declared effect vector,
    /// enabling threshold-based governance that varies per tool.
    pub fn with_tool_context(
        mut self,
        tool_name: impl Into<String>,
        gate_action: impl Into<String>,
        effect: EffectVector,
        pid: u64,
    ) -> Self {
        self.context.insert("tool".into(), tool_name.into());
        self.context.insert("gate_action".into(), gate_action.into());
        self.context.insert("pid".into(), pid.to_string());
        self.effect = effect;
        self
    }

    /// Add a single key-value pair to the context map.
    pub fn with_context_entry(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
}

/// Governance evaluation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceResult {
    /// The decision.
    pub decision: GovernanceDecision,

    /// Rules that were evaluated.
    pub evaluated_rules: Vec<String>,

    /// The effect vector that was scored.
    pub effect: EffectVector,

    /// Whether the effect magnitude exceeded the threshold.
    pub threshold_exceeded: bool,
}

/// Governance engine.
///
/// Evaluates actions against governance rules and the environment's
/// risk threshold. Without the `governance` feature gate, all
/// evaluations return `Permit`.
pub struct GovernanceEngine {
    rules: Vec<GovernanceRule>,
    risk_threshold: f64,
    human_approval_required: bool,
    /// Optional learned scorer (Finding #5). When `None` the engine
    /// scores effect vectors with the hardcoded L2 norm; when set and
    /// trained the scorer's learned composite is used instead.
    /// Untrained scorers fall back to L2 (see
    /// [`EffectVector::score`]), so this is drop-in safe.
    scorer: Option<crate::eml_kernel::GovernanceScorerModel>,
}

impl GovernanceEngine {
    /// Create a governance engine with the given risk threshold.
    pub fn new(risk_threshold: f64, human_approval_required: bool) -> Self {
        Self {
            rules: Vec::new(),
            risk_threshold,
            human_approval_required,
            scorer: None,
        }
    }

    /// Create an open governance engine that permits everything.
    pub fn open() -> Self {
        Self {
            rules: Vec::new(),
            risk_threshold: 1.0,
            human_approval_required: false,
            scorer: None,
        }
    }

    /// Install a learned [`GovernanceScorerModel`](crate::eml_kernel::GovernanceScorerModel).
    ///
    /// With a scorer installed, [`evaluate`](Self::evaluate) uses
    /// [`EffectVector::score`] which delegates to the model when
    /// trained and falls back to the L2 magnitude otherwise. The
    /// fallback path is bit-for-bit identical to the pre-EML
    /// behaviour.
    pub fn with_scorer(
        mut self,
        scorer: crate::eml_kernel::GovernanceScorerModel,
    ) -> Self {
        self.scorer = Some(scorer);
        self
    }

    /// Returns a reference to the installed governance scorer model,
    /// if any.
    pub fn scorer(&self) -> Option<&crate::eml_kernel::GovernanceScorerModel> {
        self.scorer.as_ref()
    }

    /// Add a governance rule.
    pub fn add_rule(&mut self, rule: GovernanceRule) {
        debug!(rule_id = %rule.id, branch = %rule.branch, "adding governance rule");
        self.rules.push(rule);
    }

    /// Get all active rules.
    pub fn active_rules(&self) -> Vec<&GovernanceRule> {
        self.rules.iter().filter(|r| r.active).collect()
    }

    /// Get rules by branch.
    pub fn rules_by_branch(&self, branch: &GovernanceBranch) -> Vec<&GovernanceRule> {
        self.rules
            .iter()
            .filter(|r| r.active && &r.branch == branch)
            .collect()
    }

    /// Evaluate a governance request.
    ///
    /// Decision logic:
    /// 1. If any blocking/critical rule applies, deny.
    /// 2. If effect magnitude exceeds threshold:
    ///    - If human_approval_required, escalate.
    ///    - Otherwise deny.
    /// 3. If any warning rule applies, permit with warning.
    /// 4. Otherwise permit.
    ///
    /// NOTE(eml-swap): wired — Finding #5 (GovernanceScorerModel).
    /// The scalar fed into the threshold check is produced by
    /// [`EffectVector::score`], which consults the engine's optional
    /// scorer and falls back to the L2 magnitude when no model is
    /// installed or the model is untrained.
    pub fn evaluate(&self, request: &GovernanceRequest) -> GovernanceResult {
        let magnitude = request.effect.score(self.scorer.as_ref());
        let threshold_exceeded = magnitude > self.risk_threshold;

        let mut evaluated_rules = Vec::new();
        let mut has_warning = false;
        let mut has_blocking = false;
        let mut blocking_reason = String::new();

        for rule in self.active_rules() {
            evaluated_rules.push(rule.id.clone());

            match rule.severity {
                RuleSeverity::Blocking | RuleSeverity::Critical => {
                    if threshold_exceeded {
                        has_blocking = true;
                        blocking_reason = format!(
                            "rule '{}': effect magnitude {magnitude:.2} > threshold {:.2}",
                            rule.id, self.risk_threshold
                        );
                    }
                }
                RuleSeverity::Warning => {
                    if threshold_exceeded {
                        has_warning = true;
                    }
                }
                RuleSeverity::Advisory => {}
            }
        }

        let decision = if has_blocking {
            if self.human_approval_required {
                GovernanceDecision::EscalateToHuman(blocking_reason)
            } else {
                GovernanceDecision::Deny(blocking_reason)
            }
        } else if threshold_exceeded && has_warning {
            GovernanceDecision::PermitWithWarning(format!(
                "effect magnitude {magnitude:.2} approaches threshold {:.2}",
                self.risk_threshold
            ))
        } else {
            GovernanceDecision::Permit
        };

        GovernanceResult {
            decision,
            evaluated_rules,
            effect: request.effect.clone(),
            threshold_exceeded,
        }
    }

    /// Get the configured risk threshold.
    pub fn risk_threshold(&self) -> f64 {
        self.risk_threshold
    }

    /// Get total rule count.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Evaluate a governance request in the context of a specific environment.
    ///
    /// Different environment classes apply different risk thresholds:
    /// - **Development**: uses the environment's own `risk_threshold` (lenient, typically 0.9).
    /// - **Staging**: uses the environment's own `risk_threshold` (moderate, typically 0.6).
    /// - **Production**: uses half the environment's `risk_threshold` (strict, typically 0.15).
    /// - **Custom**: uses the custom class's `risk_threshold` directly.
    ///
    /// After normal rule evaluation, an additional effect-magnitude check is
    /// performed against the environment-adjusted threshold. If the magnitude
    /// exceeds it, the decision is overridden to `Deny`.
    pub fn evaluate_in_environment(
        &self,
        request: &GovernanceRequest,
        env: &Environment,
    ) -> GovernanceResult {
        let adjusted_threshold = match &env.class {
            EnvironmentClass::Development => {
                // Dev: use the environment's risk threshold directly (lenient).
                env.governance.risk_threshold
            }
            EnvironmentClass::Staging => {
                // Staging: use the environment's risk threshold as-is.
                env.governance.risk_threshold
            }
            EnvironmentClass::Production => {
                // Production: halve the threshold for stricter gating.
                env.governance.risk_threshold * 0.5
            }
            EnvironmentClass::Custom { risk_threshold, .. } => {
                // Custom: use the class-level threshold.
                *risk_threshold
            }
        };

        // Run normal rule-based evaluation first.
        let mut result = self.evaluate(request);

        // Apply environment-scoped magnitude check on top.
        //
        // NOTE(eml-swap): wired — Finding #5 (GovernanceScorerModel).
        let magnitude = request.effect.score(self.scorer.as_ref());
        if magnitude > adjusted_threshold {
            result.threshold_exceeded = true;
            result.decision = GovernanceDecision::Deny(format!(
                "effect magnitude {magnitude:.2} exceeds {} environment threshold {adjusted_threshold:.2}",
                env.class,
            ));
        }

        result
    }

    /// Evaluate a governance request and log the decision to the chain.
    ///
    /// This is the recommended entry point when a `ChainManager` is
    /// available. It calls [`evaluate`](Self::evaluate) and records an
    /// `ipc.dead_letter`-style audit event via [`ChainLoggable`].
    ///
    /// If no chain manager is provided, behaves identically to `evaluate`.
    #[cfg(feature = "exochain")]
    pub fn evaluate_logged(
        &self,
        request: &GovernanceRequest,
        chain: Option<&crate::chain::ChainManager>,
    ) -> GovernanceResult {
        let result = self.evaluate(request);
        if let Some(cm) = chain {
            Self::chain_log_result(cm, request, &result);
        }
        result
    }

    /// Evaluate in an environment and log the decision to the chain.
    #[cfg(feature = "exochain")]
    pub fn evaluate_in_environment_logged(
        &self,
        request: &GovernanceRequest,
        env: &Environment,
        chain: Option<&crate::chain::ChainManager>,
    ) -> GovernanceResult {
        let result = self.evaluate_in_environment(request, env);
        if let Some(cm) = chain {
            Self::chain_log_result(cm, request, &result);
        }
        result
    }

    /// Log a governance result to the ExoChain.
    ///
    /// Can be called after any `evaluate` / `evaluate_in_environment`
    /// call to record the decision in the audit trail.
    #[cfg(feature = "exochain")]
    pub fn chain_log_result(
        cm: &crate::chain::ChainManager,
        request: &GovernanceRequest,
        result: &GovernanceResult,
    ) {
        use crate::chain::GovernanceDecisionEvent;

        let decision_str = match &result.decision {
            GovernanceDecision::Permit => "Permit".to_owned(),
            GovernanceDecision::PermitWithWarning(_) => "PermitWithWarning".to_owned(),
            GovernanceDecision::EscalateToHuman(_) => "EscalateToHuman".to_owned(),
            GovernanceDecision::Deny(_) => "Deny".to_owned(),
        };

        let event = GovernanceDecisionEvent {
            agent_id: request.agent_id.clone(),
            action: request.action.clone(),
            decision: decision_str,
            effect_magnitude: request.effect.magnitude(),
            threshold_exceeded: result.threshold_exceeded,
            evaluated_rules: result.evaluated_rules.clone(),
            timestamp: chrono::Utc::now(),
        };
        cm.append_loggable(&event);
    }
}

// ── Trajectory recording ─────────────────────────────────────
//
// Records agent decision points for learning, replay, and
// pattern extraction. Lives alongside governance because every
// trajectory point is a governed decision.

/// Outcome of an agent decision for trajectory scoring.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrajectoryOutcome {
    /// Decision succeeded with a reward signal.
    Success {
        /// Reward value (higher is better).
        reward: f64,
    },
    /// Decision failed.
    Failure {
        /// Reason for failure.
        reason: String,
    },
    /// Outcome not yet known.
    Pending,
}

/// A record of an agent's decision for learning and replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryRecord {
    /// Agent that made the decision.
    pub agent_id: String,
    /// What was decided (action name / tool call).
    pub action: String,
    /// Context at decision time (state snapshot).
    pub context: serde_json::Value,
    /// Outcome (success / failure / pending).
    pub outcome: TrajectoryOutcome,
    /// Timestamp of the decision.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Records agent trajectories for learning and pattern extraction.
///
/// Maintains a bounded FIFO buffer of [`TrajectoryRecord`]s. When the
/// buffer is full the oldest record is evicted. Callers can query by
/// agent and extract frequency patterns from successful actions.
pub struct TrajectoryRecorder {
    records: Vec<TrajectoryRecord>,
    max_records: usize,
}

impl TrajectoryRecorder {
    /// Create a recorder with the given capacity.
    pub fn new(max_records: usize) -> Self {
        Self {
            records: Vec::new(),
            max_records,
        }
    }

    /// Record a trajectory point. Evicts the oldest record on overflow.
    pub fn record(&mut self, record: TrajectoryRecord) {
        if self.records.len() >= self.max_records {
            self.records.remove(0); // FIFO eviction
        }
        self.records.push(record);
    }

    /// Get all records for a specific agent.
    pub fn agent_trajectory(&self, agent_id: &str) -> Vec<&TrajectoryRecord> {
        self.records
            .iter()
            .filter(|r| r.agent_id == agent_id)
            .collect()
    }

    /// Extract patterns: returns `(action, count)` pairs for successful
    /// actions, sorted by frequency descending.
    pub fn extract_patterns(&self) -> Vec<(String, usize)> {
        let mut action_counts: HashMap<String, usize> = HashMap::new();
        for record in &self.records {
            if matches!(record.outcome, TrajectoryOutcome::Success { .. }) {
                *action_counts.entry(record.action.clone()).or_default() += 1;
            }
        }
        let mut patterns: Vec<_> = action_counts.into_iter().collect();
        patterns.sort_by(|a, b| b.1.cmp(&a.1));
        patterns
    }

    /// Number of recorded trajectories.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the recorder is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

// ── RVF governance bridge ────────────────────────────────────
//
// Behind `exochain` feature gate: bidirectional mapping between
// WeftOS constitutional governance and RVF witness governance.
//
// WeftOS governance evaluates *whether* an action should proceed
// (effect algebra, risk thresholds, branch-based rules).
// RVF governance records *what happened* during execution
// (witness bundles, tool call traces, cost budgets).

#[cfg(feature = "exochain")]
impl GovernanceDecision {
    /// Map this decision to the equivalent RVF PolicyCheck.
    ///
    /// - `Permit` → `Allowed`
    /// - `PermitWithWarning` / `EscalateToHuman` → `Confirmed`
    /// - `Deny` → `Denied`
    pub fn to_rvf_policy_check(&self) -> rvf_types::witness::PolicyCheck {
        match self {
            GovernanceDecision::Permit => rvf_types::witness::PolicyCheck::Allowed,
            GovernanceDecision::PermitWithWarning(_) => rvf_types::witness::PolicyCheck::Confirmed,
            GovernanceDecision::EscalateToHuman(_) => rvf_types::witness::PolicyCheck::Confirmed,
            GovernanceDecision::Deny(_) => rvf_types::witness::PolicyCheck::Denied,
        }
    }
}

#[cfg(feature = "exochain")]
impl GovernanceEngine {
    /// Derive the equivalent RVF GovernanceMode from this engine's config.
    ///
    /// - `risk_threshold >= 1.0` (open) → `Autonomous`
    /// - `human_approval_required` → `Approved`
    /// - otherwise → `Restricted`
    pub fn to_rvf_mode(&self) -> rvf_types::witness::GovernanceMode {
        if self.risk_threshold >= 1.0 {
            rvf_types::witness::GovernanceMode::Autonomous
        } else if self.human_approval_required {
            rvf_types::witness::GovernanceMode::Approved
        } else {
            rvf_types::witness::GovernanceMode::Restricted
        }
    }

    /// Build an RVF GovernancePolicy from this engine's configuration.
    ///
    /// Uses the default tool lists and cost budgets for each mode.
    /// Callers can customize the returned policy further if needed.
    pub fn to_rvf_policy(&self) -> rvf_runtime::GovernancePolicy {
        match self.to_rvf_mode() {
            rvf_types::witness::GovernanceMode::Restricted => {
                rvf_runtime::GovernancePolicy::restricted()
            }
            rvf_types::witness::GovernanceMode::Approved => {
                rvf_runtime::GovernancePolicy::approved()
            }
            rvf_types::witness::GovernanceMode::Autonomous => {
                rvf_runtime::GovernancePolicy::autonomous()
            }
        }
    }
}

#[cfg(feature = "exochain")]
impl GovernanceResult {
    /// Map the decision to an RVF TaskOutcome.
    ///
    /// This is a convenience for recording the governance result in a
    /// witness bundle. The caller should override based on actual execution.
    pub fn to_rvf_task_outcome(&self) -> rvf_types::witness::TaskOutcome {
        match &self.decision {
            GovernanceDecision::Permit | GovernanceDecision::PermitWithWarning(_) => {
                rvf_types::witness::TaskOutcome::Solved
            }
            GovernanceDecision::EscalateToHuman(_) => rvf_types::witness::TaskOutcome::Skipped,
            GovernanceDecision::Deny(_) => rvf_types::witness::TaskOutcome::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(id: &str, severity: RuleSeverity, branch: GovernanceBranch) -> GovernanceRule {
        GovernanceRule {
            id: id.into(),
            description: format!("Test rule {id}"),
            branch,
            severity,
            active: true,
            reference_url: None,
            sop_category: None,
        }
    }

    #[test]
    fn effect_vector_magnitude() {
        let v = EffectVector {
            risk: 0.3,
            fairness: 0.4,
            privacy: 0.0,
            novelty: 0.0,
            security: 0.0,
        };
        assert!((v.magnitude() - 0.5).abs() < 0.001);
    }

    #[test]
    fn effect_vector_zero() {
        let v = EffectVector::default();
        assert!((v.magnitude() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn effect_score_untrained_matches_magnitude() {
        // Finding #5: untrained scorer must be bit-for-bit identical to
        // the L2 magnitude. Covers both None and Some(untrained) paths.
        let v = EffectVector {
            risk: 0.5,
            fairness: 0.3,
            privacy: 0.2,
            novelty: 0.1,
            security: 0.4,
        };

        let expected = v.magnitude();
        assert_eq!(v.score(None), expected);

        let untrained = crate::eml_kernel::GovernanceScorerModel::new();
        assert!(!untrained.is_trained());
        assert_eq!(v.score(Some(&untrained)), expected);
    }

    #[test]
    fn effect_score_trained_uses_model() {
        // With a trained scorer, score() takes the EML path instead of
        // L2. We force the trained flag via a JSON patch because the
        // underlying coordinate descent needs 50+ samples to declare
        // convergence, which would make this a slow integration test.
        let v = EffectVector {
            risk: 1.0,
            fairness: 1.0,
            privacy: 1.0,
            novelty: 1.0,
            security: 1.0,
        };

        let scorer = crate::eml_kernel::GovernanceScorerModel::new();
        let untrained_score = v.score(Some(&scorer));
        assert_eq!(untrained_score, v.magnitude());

        // Patch the serialized form to force is_trained() = true.
        let mut json = serde_json::to_value(&scorer).unwrap();
        if let Some(inner) = json.get_mut("inner").and_then(|v| v.as_object_mut()) {
            inner.insert("trained".into(), serde_json::Value::Bool(true));
        }
        let forced: crate::eml_kernel::GovernanceScorerModel =
            serde_json::from_value(json).expect("patched scorer must deserialize");
        assert!(forced.is_trained(), "patched scorer should report trained");

        // Fall-through branch runs; we don't assert a specific value
        // (params are zeros, so the EML output is deterministic but
        // implementation-defined). The contract is: `score` dispatches
        // to the model instead of the L2 shortcut.
        let trained_score = v.score(Some(&forced));
        // Untrained L2 for (1,1,1,1,1) is sqrt(5) ≈ 2.236. The model
        // predict_primary over zero-params is 0.0 (clamped at 0.0 by
        // max(0.0)). These must differ — proves the branch taken.
        assert_ne!(
            trained_score, untrained_score,
            "trained scorer must dispatch to the model, not L2"
        );
    }

    #[test]
    fn effect_any_exceeds() {
        let v = EffectVector {
            risk: 0.8,
            ..Default::default()
        };
        assert!(v.any_exceeds(0.5));
        assert!(!v.any_exceeds(0.9));
    }

    #[test]
    fn effect_max_dimension() {
        let v = EffectVector {
            risk: 0.2,
            fairness: 0.5,
            privacy: 0.3,
            novelty: 0.1,
            security: 0.4,
        };
        assert!((v.max_dimension() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn governance_branch_display() {
        assert_eq!(GovernanceBranch::Legislative.to_string(), "legislative");
        assert_eq!(GovernanceBranch::Executive.to_string(), "executive");
        assert_eq!(GovernanceBranch::Judicial.to_string(), "judicial");
    }

    #[test]
    fn rule_severity_ordering() {
        assert!(RuleSeverity::Advisory < RuleSeverity::Warning);
        assert!(RuleSeverity::Warning < RuleSeverity::Blocking);
        assert!(RuleSeverity::Blocking < RuleSeverity::Critical);
    }

    #[test]
    fn governance_decision_display() {
        assert_eq!(GovernanceDecision::Permit.to_string(), "permit");
        assert!(
            GovernanceDecision::Deny("too risky".into())
                .to_string()
                .contains("too risky")
        );
    }

    #[test]
    fn open_engine_permits_everything() {
        let engine = GovernanceEngine::open();
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "deploy".into(),
            effect: EffectVector {
                risk: 0.9,
                security: 0.9,
                ..Default::default()
            },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert_eq!(result.decision, GovernanceDecision::Permit);
    }

    #[test]
    fn blocking_rule_denies() {
        let mut engine = GovernanceEngine::new(0.5, false);
        engine.add_rule(make_rule(
            "security-check",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));

        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "deploy".into(),
            effect: EffectVector {
                risk: 0.6,
                ..Default::default()
            },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert!(matches!(result.decision, GovernanceDecision::Deny(_)));
        assert!(result.threshold_exceeded);
    }

    #[test]
    fn engine_with_untrained_scorer_preserves_behavior() {
        // Finding #5: installing an untrained GovernanceScorerModel must
        // not change evaluation outcomes (drop-in safe).
        let mut baseline = GovernanceEngine::new(0.5, false);
        baseline.add_rule(make_rule(
            "security-check",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));
        let mut wired = GovernanceEngine::new(0.5, false)
            .with_scorer(crate::eml_kernel::GovernanceScorerModel::new());
        wired.add_rule(make_rule(
            "security-check",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));

        for effect in [
            EffectVector {
                risk: 0.6,
                ..Default::default()
            },
            EffectVector {
                risk: 0.1,
                fairness: 0.1,
                ..Default::default()
            },
            EffectVector::default(),
        ] {
            let req = GovernanceRequest {
                agent_id: "a".into(),
                action: "x".into(),
                effect,
                context: Default::default(),
                node_id: None,
            };
            assert_eq!(
                baseline.evaluate(&req).decision,
                wired.evaluate(&req).decision,
                "untrained scorer must reproduce L2 behaviour"
            );
        }
    }

    #[test]
    fn engine_with_trained_scorer_uses_model_scalar() {
        // Finding #5: once the scorer is trained, the engine's
        // threshold check receives the model-derived scalar. We force
        // `is_trained` via a JSON patch so we don't have to train.
        let scorer = crate::eml_kernel::GovernanceScorerModel::new();
        let mut json = serde_json::to_value(&scorer).unwrap();
        if let Some(inner) = json.get_mut("inner").and_then(|v| v.as_object_mut()) {
            inner.insert("trained".into(), serde_json::Value::Bool(true));
        }
        let forced: crate::eml_kernel::GovernanceScorerModel =
            serde_json::from_value(json).unwrap();
        assert!(forced.is_trained());

        let effect = EffectVector {
            risk: 1.0,
            ..Default::default()
        };
        let l2 = effect.magnitude();
        let model_scalar = effect.score(Some(&forced));
        // Zero params + softmax3(0,0,0) means model_scalar is derived
        // from the EML tree and NOT equal to the L2 magnitude — this
        // is the core invariant: the trained branch actually fires.
        assert_ne!(
            model_scalar, l2,
            "trained scorer must not return L2 magnitude"
        );

        // Bonus: verify the engine uses it by picking a threshold that
        // straddles the two values. Because we don't know which scalar
        // is larger, pick between them.
        let (lo, hi) = if model_scalar < l2 {
            (model_scalar, l2)
        } else {
            (l2, model_scalar)
        };
        let mid = (lo + hi) / 2.0;
        let engine = GovernanceEngine::new(mid, false).with_scorer(forced);
        let request = GovernanceRequest {
            agent_id: "a".into(),
            action: "x".into(),
            effect,
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        // With threshold between model_scalar and l2: outcome depends
        // on which is larger. The invariant we check is that the
        // scorer-backed decision uses `model_scalar` (> mid only if
        // `model_scalar == hi`).
        assert_eq!(result.threshold_exceeded, model_scalar > mid);
    }

    #[test]
    fn blocking_with_human_approval_escalates() {
        let mut engine = GovernanceEngine::new(0.5, true);
        engine.add_rule(make_rule(
            "security-check",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));

        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "deploy".into(),
            effect: EffectVector {
                risk: 0.6,
                ..Default::default()
            },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert!(matches!(
            result.decision,
            GovernanceDecision::EscalateToHuman(_)
        ));
    }

    #[test]
    fn warning_rule_permits_with_warning() {
        let mut engine = GovernanceEngine::new(0.5, false);
        engine.add_rule(make_rule(
            "risk-check",
            RuleSeverity::Warning,
            GovernanceBranch::Executive,
        ));

        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "deploy".into(),
            effect: EffectVector {
                risk: 0.6,
                ..Default::default()
            },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert!(matches!(
            result.decision,
            GovernanceDecision::PermitWithWarning(_)
        ));
    }

    #[test]
    fn below_threshold_permits() {
        let mut engine = GovernanceEngine::new(0.5, false);
        engine.add_rule(make_rule(
            "security-check",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));

        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "read".into(),
            effect: EffectVector {
                risk: 0.1,
                ..Default::default()
            },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert_eq!(result.decision, GovernanceDecision::Permit);
        assert!(!result.threshold_exceeded);
    }

    #[test]
    fn rules_by_branch() {
        let mut engine = GovernanceEngine::new(0.5, false);
        engine.add_rule(make_rule(
            "r1",
            RuleSeverity::Warning,
            GovernanceBranch::Legislative,
        ));
        engine.add_rule(make_rule(
            "r2",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));
        engine.add_rule(make_rule(
            "r3",
            RuleSeverity::Advisory,
            GovernanceBranch::Judicial,
        ));

        let judicial = engine.rules_by_branch(&GovernanceBranch::Judicial);
        assert_eq!(judicial.len(), 2);
        let legislative = engine.rules_by_branch(&GovernanceBranch::Legislative);
        assert_eq!(legislative.len(), 1);
    }

    #[test]
    fn inactive_rules_excluded() {
        let mut engine = GovernanceEngine::new(0.5, false);
        engine.add_rule(GovernanceRule {
            id: "disabled".into(),
            description: "Disabled rule".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: false,
            reference_url: None,
            sop_category: None,
        });
        assert_eq!(engine.active_rules().len(), 0);
    }

    #[test]
    fn governance_rule_serde_roundtrip() {
        let rule = make_rule("sec-1", RuleSeverity::Critical, GovernanceBranch::Judicial);
        let json = serde_json::to_string(&rule).unwrap();
        let restored: GovernanceRule = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "sec-1");
        assert!(restored.active);
    }

    #[test]
    fn governance_request_serde_roundtrip() {
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "deploy".into(),
            effect: EffectVector {
                risk: 0.5,
                privacy: 0.3,
                ..Default::default()
            },
            context: std::collections::HashMap::from([("env".into(), "prod".into())]),
            node_id: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let restored: GovernanceRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id, "agent-1");
        assert!((restored.effect.risk - 0.5).abs() < f64::EPSILON);
        assert!(restored.node_id.is_none());
    }

    #[test]
    fn governance_request_with_node_id() {
        let request = GovernanceRequest::new("agent-1", "deploy")
            .with_node_id("node-42")
            .with_effect(EffectVector { risk: 0.1, ..Default::default() });

        assert_eq!(request.node_id.as_deref(), Some("node-42"));
        assert_eq!(request.agent_id, "agent-1");

        // Serde roundtrip preserves node_id.
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("node-42"));
        let restored: GovernanceRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_id.as_deref(), Some("node-42"));
    }

    #[test]
    fn governance_request_without_node_id_deserializes() {
        // JSON without node_id should deserialize with node_id = None (backward compat).
        let json = r#"{"agent_id":"a","action":"b","effect":{},"context":{}}"#;
        let request: GovernanceRequest = serde_json::from_str(json).unwrap();
        assert!(request.node_id.is_none());
    }

    #[test]
    fn governance_request_builder() {
        let request = GovernanceRequest::new("agent-1", "deploy");
        assert_eq!(request.agent_id, "agent-1");
        assert_eq!(request.action, "deploy");
        assert!(request.node_id.is_none());
        assert!((request.effect.magnitude() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn effect_vector_serde_roundtrip() {
        let v = EffectVector {
            risk: 0.1,
            fairness: 0.2,
            privacy: 0.3,
            novelty: 0.4,
            security: 0.5,
        };
        let json = serde_json::to_string(&v).unwrap();
        let restored: EffectVector = serde_json::from_str(&json).unwrap();
        assert!((restored.security - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn filter_rules_by_sop_category() {
        let rules = vec![
            GovernanceRule {
                id: "SOP-L001".into(),
                description: "test".into(),
                branch: GovernanceBranch::Legislative,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: Some("https://example.com".into()),
                sop_category: Some("governance".into()),
            },
            GovernanceRule {
                id: "SOP-J001".into(),
                description: "test".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: Some("https://example.com".into()),
                sop_category: Some("ethics".into()),
            },
            GovernanceRule {
                id: "GOV-001".into(),
                description: "test".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            },
        ];
        let ethics = GovernanceRule::filter_by_category(&rules, "ethics");
        assert_eq!(ethics.len(), 1);
        assert_eq!(ethics[0].id, "SOP-J001");

        let governance = GovernanceRule::filter_by_category(&rules, "governance");
        assert_eq!(governance.len(), 1);

        let none = GovernanceRule::filter_by_category(&rules, "nonexistent");
        assert!(none.is_empty());
    }

    #[test]
    fn governance_rule_with_sop_serde() {
        let rule = GovernanceRule {
            id: "SOP-L001".into(),
            description: "test".into(),
            branch: GovernanceBranch::Legislative,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: Some("https://example.com/sop".into()),
            sop_category: Some("governance".into()),
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("reference_url"));
        assert!(json.contains("sop_category"));
        let restored: GovernanceRule = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.reference_url, Some("https://example.com/sop".into()));
    }

    #[test]
    fn governance_rule_without_sop_backward_compat() {
        // Old-format JSON without new fields should deserialize fine
        let json = r#"{"id":"GOV-001","description":"test","branch":"Judicial","severity":"Blocking","active":true}"#;
        let rule: GovernanceRule = serde_json::from_str(json).unwrap();
        assert!(rule.reference_url.is_none());
        assert!(rule.sop_category.is_none());
    }

    // ── Genesis rule enforcement tests ──────────────────────────────

    /// Helper to create a GovernanceRule with optional SOP category,
    /// matching the shape used in boot.rs genesis rules.
    fn make_sop_rule(
        id: &str,
        severity: RuleSeverity,
        branch: GovernanceBranch,
        category: Option<&str>,
    ) -> GovernanceRule {
        GovernanceRule {
            id: id.into(),
            description: format!("Genesis rule {id}"),
            branch,
            severity,
            active: true,
            reference_url: None,
            sop_category: category.map(|c| c.into()),
        }
    }

    /// Build a GovernanceEngine with all 22 genesis rules matching boot.rs.
    fn genesis_engine() -> GovernanceEngine {
        let mut engine = GovernanceEngine::new(0.7, false);

        // ── Core constitutional rules (GOV-001 .. GOV-007) ──────
        // Judicial blocking
        engine.add_rule(make_sop_rule("GOV-001", RuleSeverity::Blocking, GovernanceBranch::Judicial, None));
        engine.add_rule(make_sop_rule("GOV-002", RuleSeverity::Blocking, GovernanceBranch::Judicial, None));
        // Legislative warning
        engine.add_rule(make_sop_rule("GOV-003", RuleSeverity::Warning, GovernanceBranch::Legislative, None));
        // Executive advisory
        engine.add_rule(make_sop_rule("GOV-004", RuleSeverity::Advisory, GovernanceBranch::Executive, None));
        // Legislative warning
        engine.add_rule(make_sop_rule("GOV-005", RuleSeverity::Warning, GovernanceBranch::Legislative, None));
        // Executive blocking
        engine.add_rule(make_sop_rule("GOV-006", RuleSeverity::Blocking, GovernanceBranch::Executive, None));
        // Judicial advisory
        engine.add_rule(make_sop_rule("GOV-007", RuleSeverity::Advisory, GovernanceBranch::Judicial, None));

        // ── AI-SDLC SOP rules: Legislative (6) ──────────────────
        engine.add_rule(make_sop_rule("SOP-L001", RuleSeverity::Blocking, GovernanceBranch::Legislative, Some("governance")));
        engine.add_rule(make_sop_rule("SOP-L002", RuleSeverity::Warning, GovernanceBranch::Legislative, Some("governance")));
        engine.add_rule(make_sop_rule("SOP-L003", RuleSeverity::Warning, GovernanceBranch::Legislative, Some("engineering")));
        engine.add_rule(make_sop_rule("SOP-L004", RuleSeverity::Advisory, GovernanceBranch::Legislative, Some("lifecycle")));
        engine.add_rule(make_sop_rule("SOP-L005", RuleSeverity::Blocking, GovernanceBranch::Legislative, Some("ethics")));
        engine.add_rule(make_sop_rule("SOP-L006", RuleSeverity::Warning, GovernanceBranch::Legislative, Some("governance")));

        // ── AI-SDLC SOP rules: Executive (5) ────────────────────
        engine.add_rule(make_sop_rule("SOP-E001", RuleSeverity::Warning, GovernanceBranch::Executive, Some("engineering")));
        engine.add_rule(make_sop_rule("SOP-E002", RuleSeverity::Blocking, GovernanceBranch::Executive, Some("lifecycle")));
        engine.add_rule(make_sop_rule("SOP-E003", RuleSeverity::Warning, GovernanceBranch::Executive, Some("security")));
        engine.add_rule(make_sop_rule("SOP-E004", RuleSeverity::Advisory, GovernanceBranch::Executive, Some("lifecycle")));
        engine.add_rule(make_sop_rule("SOP-E005", RuleSeverity::Advisory, GovernanceBranch::Executive, Some("governance")));

        // ── AI-SDLC SOP rules: Judicial (4) ─────────────────────
        engine.add_rule(make_sop_rule("SOP-J001", RuleSeverity::Blocking, GovernanceBranch::Judicial, Some("ethics")));
        engine.add_rule(make_sop_rule("SOP-J002", RuleSeverity::Warning, GovernanceBranch::Judicial, Some("ethics")));
        engine.add_rule(make_sop_rule("SOP-J003", RuleSeverity::Warning, GovernanceBranch::Judicial, Some("lifecycle")));
        engine.add_rule(make_sop_rule("SOP-J004", RuleSeverity::Advisory, GovernanceBranch::Judicial, Some("quality")));

        engine
    }

    #[test]
    fn genesis_has_22_rules() {
        let engine = genesis_engine();
        assert_eq!(engine.rule_count(), 22);
    }

    #[test]
    fn genesis_high_risk_operation_blocked() {
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "deploy-to-prod".into(),
            effect: EffectVector { risk: 0.9, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert!(
            matches!(result.decision, GovernanceDecision::Deny(_)),
            "high-risk operation should be denied, got {:?}", result.decision,
        );
        assert!(result.threshold_exceeded);
    }

    #[test]
    fn genesis_low_risk_operation_permitted() {
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "read-file".into(),
            effect: EffectVector { risk: 0.1, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert_eq!(result.decision, GovernanceDecision::Permit);
        assert!(!result.threshold_exceeded);
    }

    #[test]
    fn genesis_privacy_violation_triggers_enforcement() {
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "access-user-data".into(),
            effect: EffectVector { privacy: 0.8, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        // magnitude = 0.8 > 0.7 threshold; blocking rules exist -> Deny
        assert!(
            matches!(result.decision, GovernanceDecision::Deny(_) | GovernanceDecision::PermitWithWarning(_)),
            "privacy violation should trigger warning or deny, got {:?}", result.decision,
        );
        assert!(result.threshold_exceeded);
    }

    #[test]
    fn genesis_security_sensitive_blocked() {
        // GOV-002: security-sensitive actions blocked when threshold exceeded
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "modify-firewall".into(),
            effect: EffectVector { security: 0.9, risk: 0.5, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        // magnitude = sqrt(0.81 + 0.25) ~ 1.03 > 0.7
        assert!(matches!(result.decision, GovernanceDecision::Deny(_)));
    }

    #[test]
    fn genesis_fairness_bias_blocked() {
        // SOP-J001: bias/fairness evaluation blocking
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "ml-agent".into(),
            action: "evaluate-candidate".into(),
            effect: EffectVector { fairness: 0.9, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert!(
            matches!(result.decision, GovernanceDecision::Deny(_)),
            "fairness violation should be blocked by SOP-J001, got {:?}", result.decision,
        );
    }

    #[test]
    fn genesis_agent_spawn_blocked_when_risky() {
        // GOV-006 + SOP-E002: agent spawn with high risk denied
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "orchestrator".into(),
            action: "agent.spawn".into(),
            effect: EffectVector { risk: 0.8, novelty: 0.5, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        // magnitude = sqrt(0.64 + 0.25) ~ 0.94 > 0.7
        assert!(matches!(result.decision, GovernanceDecision::Deny(_)));
    }

    #[test]
    fn genesis_agent_spawn_permitted_when_safe() {
        // GOV-006: spawn with low effect should be permitted
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "orchestrator".into(),
            action: "agent.spawn".into(),
            effect: EffectVector { risk: 0.1, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert_eq!(result.decision, GovernanceDecision::Permit);
    }

    #[test]
    fn genesis_data_protection_blocks_high_privacy() {
        // SOP-L005: data protection blocking
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "data-agent".into(),
            action: "export-pii".into(),
            effect: EffectVector { privacy: 0.9, risk: 0.3, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        // magnitude = sqrt(0.81 + 0.09) ~ 0.95 > 0.7
        assert!(matches!(result.decision, GovernanceDecision::Deny(_)));
    }

    #[test]
    fn genesis_with_human_approval_escalates() {
        // Same rules but with human_approval_required = true
        let mut engine = GovernanceEngine::new(0.7, true);
        engine.add_rule(make_sop_rule("GOV-001", RuleSeverity::Blocking, GovernanceBranch::Judicial, None));

        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "high-risk-op".into(),
            effect: EffectVector { risk: 0.9, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert!(
            matches!(result.decision, GovernanceDecision::EscalateToHuman(_)),
            "with human_approval, blocking should escalate, got {:?}", result.decision,
        );
    }

    #[test]
    fn genesis_all_branches_represented() {
        let engine = genesis_engine();
        let legislative = engine.rules_by_branch(&GovernanceBranch::Legislative);
        let executive = engine.rules_by_branch(&GovernanceBranch::Executive);
        let judicial = engine.rules_by_branch(&GovernanceBranch::Judicial);

        // Legislative: GOV-003, GOV-005, SOP-L001..L006 = 8
        assert_eq!(legislative.len(), 8, "legislative should have 8 rules, got {}", legislative.len());
        // Executive: GOV-004, GOV-006, SOP-E001..E005 = 7
        assert_eq!(executive.len(), 7, "executive should have 7 rules, got {}", executive.len());
        // Judicial: GOV-001, GOV-002, GOV-007, SOP-J001..J004 = 7
        assert_eq!(judicial.len(), 7, "judicial should have 7 rules, got {}", judicial.len());
    }

    #[test]
    fn genesis_blocking_rules_count() {
        let engine = genesis_engine();
        let blocking_count = engine.active_rules().iter()
            .filter(|r| matches!(r.severity, RuleSeverity::Blocking | RuleSeverity::Critical))
            .count();
        // GOV-001, GOV-002, GOV-006, SOP-L001, SOP-L005, SOP-E002, SOP-J001 = 7
        assert_eq!(blocking_count, 7, "should have exactly 7 blocking rules");
    }

    #[test]
    fn genesis_warning_rules_count() {
        let engine = genesis_engine();
        let warning_count = engine.active_rules().iter()
            .filter(|r| matches!(r.severity, RuleSeverity::Warning))
            .count();
        // GOV-003, GOV-005, SOP-L002, SOP-L003, SOP-L006,
        // SOP-E001, SOP-E003, SOP-J002, SOP-J003 = 9
        assert_eq!(warning_count, 9, "should have exactly 9 warning rules");
    }

    #[test]
    fn genesis_advisory_rules_count() {
        let engine = genesis_engine();
        let advisory_count = engine.active_rules().iter()
            .filter(|r| matches!(r.severity, RuleSeverity::Advisory))
            .count();
        // GOV-004, GOV-007, SOP-L004, SOP-E004, SOP-E005, SOP-J004 = 6
        assert_eq!(advisory_count, 6, "should have exactly 6 advisory rules");
    }

    #[test]
    fn genesis_moderate_risk_with_only_warnings_permits_with_warning() {
        // Only warning-severity rules, no blocking: should PermitWithWarning
        let mut engine = GovernanceEngine::new(0.7, false);
        engine.add_rule(make_sop_rule("SOP-L002", RuleSeverity::Warning, GovernanceBranch::Legislative, Some("governance")));
        engine.add_rule(make_sop_rule("SOP-E001", RuleSeverity::Warning, GovernanceBranch::Executive, Some("engineering")));

        let request = GovernanceRequest {
            agent_id: "dev-agent".into(),
            action: "write-code".into(),
            effect: EffectVector { risk: 0.5, novelty: 0.5, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        // magnitude ~ 0.71 > 0.7, only warnings -> PermitWithWarning
        assert!(
            matches!(result.decision, GovernanceDecision::PermitWithWarning(_)),
            "warning-only rules above threshold should PermitWithWarning, got {:?}", result.decision,
        );
    }

    #[test]
    fn genesis_advisory_only_permits_above_threshold() {
        // Advisory rules alone never block or warn -- action is permitted
        let mut engine = GovernanceEngine::new(0.7, false);
        engine.add_rule(make_sop_rule("GOV-004", RuleSeverity::Advisory, GovernanceBranch::Executive, None));
        engine.add_rule(make_sop_rule("GOV-007", RuleSeverity::Advisory, GovernanceBranch::Judicial, None));

        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "novel-action".into(),
            effect: EffectVector { novelty: 0.9, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert_eq!(result.decision, GovernanceDecision::Permit,
            "advisory-only rules should still permit, got {:?}", result.decision);
        assert!(result.threshold_exceeded);
    }

    #[test]
    fn genesis_evaluates_all_22_rules() {
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "any-action".into(),
            effect: EffectVector { risk: 0.9, ..Default::default() },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        assert_eq!(
            result.evaluated_rules.len(), 22,
            "all 22 rules should be evaluated, got {}", result.evaluated_rules.len(),
        );
    }

    #[test]
    fn genesis_sop_categories_present() {
        let engine = genesis_engine();
        let all_rules = engine.active_rules();
        let categorized: Vec<_> = all_rules.iter()
            .filter(|r| r.sop_category.is_some())
            .collect();
        // 15 SOP rules have categories, 7 GOV rules do not
        assert_eq!(categorized.len(), 15, "15 SOP rules should have categories");

        let categories: std::collections::HashSet<_> = categorized.iter()
            .map(|r| r.sop_category.as_deref().unwrap())
            .collect();
        assert!(categories.contains("governance"));
        assert!(categories.contains("ethics"));
        assert!(categories.contains("engineering"));
        assert!(categories.contains("lifecycle"));
        assert!(categories.contains("security"));
        assert!(categories.contains("quality"));
    }

    #[test]
    fn genesis_multi_dimension_high_effect_denied() {
        // Multiple dimensions contributing to high magnitude
        let engine = genesis_engine();
        let request = GovernanceRequest {
            agent_id: "agent-1".into(),
            action: "risky-novel-private".into(),
            effect: EffectVector {
                risk: 0.4,
                privacy: 0.4,
                novelty: 0.4,
                security: 0.4,
                fairness: 0.0,
            },
            context: Default::default(),
            node_id: None,
        };
        let result = engine.evaluate(&request);
        // magnitude = sqrt(4 * 0.16) = sqrt(0.64) = 0.8 > 0.7
        assert!(
            matches!(result.decision, GovernanceDecision::Deny(_)),
            "combined multi-dimension effect should exceed threshold and deny, got {:?}",
            result.decision,
        );
        assert!(result.threshold_exceeded);
    }

    // ── Environment-scoped governance tests ─────────────────────

    fn make_env(class: crate::environment::EnvironmentClass, risk_threshold: f64) -> crate::environment::Environment {
        crate::environment::Environment {
            id: format!("{class}"),
            name: format!("{class}"),
            class,
            governance: crate::environment::GovernanceScope {
                risk_threshold,
                ..Default::default()
            },
            labels: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn dev_environment_allows_higher_risk() {
        use crate::environment::EnvironmentClass;

        // Use an open engine (threshold 1.0) so the environment scope
        // is the controlling factor, not the engine's own threshold.
        let engine = GovernanceEngine::open();

        // Dev environment with risk_threshold 0.9 -- lenient.
        let dev_env = make_env(EnvironmentClass::Development, 0.9);

        // High-risk request: magnitude 0.8
        let request = GovernanceRequest::new("agent-1", "deploy")
            .with_effect(EffectVector { risk: 0.8, ..Default::default() });

        let result = engine.evaluate_in_environment(&request, &dev_env);
        // 0.8 < 0.9 (dev threshold) => should be permitted.
        assert_eq!(
            result.decision,
            GovernanceDecision::Permit,
            "dev should allow risk 0.8 with threshold 0.9, got {:?}",
            result.decision,
        );
    }

    #[test]
    fn prod_environment_blocks_high_risk() {
        use crate::environment::EnvironmentClass;

        let mut engine = GovernanceEngine::new(1.0, false);
        engine.add_rule(make_rule(
            "sec-check",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));

        // Prod environment with risk_threshold 0.6 -- halved to 0.3.
        let prod_env = make_env(EnvironmentClass::Production, 0.6);

        // Moderate request: magnitude 0.4
        let request = GovernanceRequest::new("agent-1", "deploy")
            .with_effect(EffectVector { risk: 0.4, ..Default::default() });

        let result = engine.evaluate_in_environment(&request, &prod_env);
        // 0.4 > 0.3 (prod adjusted = 0.6 * 0.5) => denied.
        assert!(
            matches!(result.decision, GovernanceDecision::Deny(_)),
            "prod should deny risk 0.4 with adjusted threshold 0.3, got {:?}",
            result.decision,
        );
    }

    #[test]
    fn staging_uses_normal_thresholds() {
        use crate::environment::EnvironmentClass;

        let mut engine = GovernanceEngine::new(1.0, false);
        engine.add_rule(make_rule(
            "sec-check",
            RuleSeverity::Blocking,
            GovernanceBranch::Judicial,
        ));

        // Staging environment with risk_threshold 0.6.
        let staging_env = make_env(EnvironmentClass::Staging, 0.6);

        // Moderate request: magnitude 0.5
        let request = GovernanceRequest::new("agent-1", "test-deploy")
            .with_effect(EffectVector { risk: 0.5, ..Default::default() });

        let result = engine.evaluate_in_environment(&request, &staging_env);
        // 0.5 < 0.6 => permitted.
        assert_eq!(
            result.decision,
            GovernanceDecision::Permit,
            "staging should allow risk 0.5 with threshold 0.6, got {:?}",
            result.decision,
        );

        // Higher request: magnitude 0.7
        let request2 = GovernanceRequest::new("agent-1", "test-deploy")
            .with_effect(EffectVector { risk: 0.7, ..Default::default() });
        let result2 = engine.evaluate_in_environment(&request2, &staging_env);
        // 0.7 > 0.6 => denied.
        assert!(
            matches!(result2.decision, GovernanceDecision::Deny(_)),
            "staging should deny risk 0.7 with threshold 0.6, got {:?}",
            result2.decision,
        );
    }

    #[test]
    fn environment_governance_same_request_different_envs() {
        use crate::environment::EnvironmentClass;

        let engine = GovernanceEngine::open();

        let dev = make_env(EnvironmentClass::Development, 0.9);
        let prod = make_env(EnvironmentClass::Production, 0.6);

        // magnitude = 0.5
        let request = GovernanceRequest::new("agent-1", "write-file")
            .with_effect(EffectVector { risk: 0.5, ..Default::default() });

        let dev_result = engine.evaluate_in_environment(&request, &dev);
        let prod_result = engine.evaluate_in_environment(&request, &prod);

        // Dev: 0.5 < 0.9 => permit
        assert_eq!(dev_result.decision, GovernanceDecision::Permit);
        // Prod: 0.5 > 0.3 (0.6 * 0.5) => deny
        assert!(matches!(prod_result.decision, GovernanceDecision::Deny(_)));
    }

    // ── Trajectory recorder tests ───────────────────────────────

    #[test]
    fn trajectory_records_and_retrieves() {
        use chrono::Utc;
        let mut recorder = TrajectoryRecorder::new(100);
        recorder.record(TrajectoryRecord {
            agent_id: "agent-1".into(),
            action: "tool.execute".into(),
            context: serde_json::json!({"tool": "read_file"}),
            outcome: TrajectoryOutcome::Success { reward: 1.0 },
            timestamp: Utc::now(),
        });
        assert_eq!(recorder.len(), 1);
        assert!(!recorder.is_empty());
        assert_eq!(recorder.agent_trajectory("agent-1").len(), 1);
        assert_eq!(recorder.agent_trajectory("agent-2").len(), 0);
    }

    #[test]
    fn trajectory_extracts_patterns() {
        use chrono::Utc;
        let mut recorder = TrajectoryRecorder::new(100);

        for _ in 0..5 {
            recorder.record(TrajectoryRecord {
                agent_id: "a".into(),
                action: "tool.execute".into(),
                context: serde_json::json!({}),
                outcome: TrajectoryOutcome::Success { reward: 1.0 },
                timestamp: Utc::now(),
            });
        }
        for _ in 0..3 {
            recorder.record(TrajectoryRecord {
                agent_id: "a".into(),
                action: "tool.read".into(),
                context: serde_json::json!({}),
                outcome: TrajectoryOutcome::Success { reward: 0.5 },
                timestamp: Utc::now(),
            });
        }
        recorder.record(TrajectoryRecord {
            agent_id: "a".into(),
            action: "tool.fail".into(),
            context: serde_json::json!({}),
            outcome: TrajectoryOutcome::Failure {
                reason: "err".into(),
            },
            timestamp: Utc::now(),
        });

        let patterns = recorder.extract_patterns();
        assert_eq!(patterns[0].0, "tool.execute");
        assert_eq!(patterns[0].1, 5);
        assert_eq!(patterns[1].0, "tool.read");
        assert_eq!(patterns[1].1, 3);
        // Failures are not in patterns.
        assert!(patterns.iter().all(|(a, _)| a != "tool.fail"));
    }

    #[test]
    fn trajectory_max_records_eviction() {
        use chrono::Utc;
        let mut recorder = TrajectoryRecorder::new(3);
        for i in 0..5 {
            recorder.record(TrajectoryRecord {
                agent_id: format!("agent-{i}"),
                action: "act".into(),
                context: serde_json::json!({}),
                outcome: TrajectoryOutcome::Pending,
                timestamp: Utc::now(),
            });
        }
        assert_eq!(recorder.len(), 3);
        // Oldest two (agent-0, agent-1) should be evicted.
        assert!(recorder.agent_trajectory("agent-0").is_empty());
        assert!(recorder.agent_trajectory("agent-1").is_empty());
        assert_eq!(recorder.agent_trajectory("agent-2").len(), 1);
        assert_eq!(recorder.agent_trajectory("agent-4").len(), 1);
    }

    #[test]
    fn trajectory_empty_patterns() {
        let recorder = TrajectoryRecorder::new(10);
        assert!(recorder.is_empty());
        assert!(recorder.extract_patterns().is_empty());
    }

    // ── Sprint 09a: serde roundtrip + behavioral tests ─────────

    #[test]
    fn governance_branch_serde_roundtrip() {
        for branch in [
            GovernanceBranch::Legislative,
            GovernanceBranch::Executive,
            GovernanceBranch::Judicial,
        ] {
            let json = serde_json::to_string(&branch).unwrap();
            let restored: GovernanceBranch = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, branch);
        }
    }

    #[test]
    fn rule_severity_serde_roundtrip() {
        for severity in [
            RuleSeverity::Advisory,
            RuleSeverity::Warning,
            RuleSeverity::Blocking,
            RuleSeverity::Critical,
        ] {
            let json = serde_json::to_string(&severity).unwrap();
            let restored: RuleSeverity = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, severity);
        }
    }

    #[test]
    fn rule_severity_display() {
        assert_eq!(RuleSeverity::Advisory.to_string(), "advisory");
        assert_eq!(RuleSeverity::Warning.to_string(), "warning");
        assert_eq!(RuleSeverity::Blocking.to_string(), "blocking");
        assert_eq!(RuleSeverity::Critical.to_string(), "critical");
    }

    #[test]
    fn governance_decision_serde_roundtrip_all_variants() {
        let variants = vec![
            GovernanceDecision::Permit,
            GovernanceDecision::PermitWithWarning("low risk".into()),
            GovernanceDecision::EscalateToHuman("needs review".into()),
            GovernanceDecision::Deny("policy violation".into()),
        ];
        for decision in variants {
            let json = serde_json::to_string(&decision).unwrap();
            let restored: GovernanceDecision = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, decision);
        }
    }

    #[test]
    fn governance_result_serde_roundtrip() {
        let result = GovernanceResult {
            decision: GovernanceDecision::Permit,
            evaluated_rules: vec!["rule-1".into(), "rule-2".into()],
            effect: EffectVector {
                risk: 0.3,
                ..Default::default()
            },
            threshold_exceeded: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: GovernanceResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.decision, GovernanceDecision::Permit);
        assert_eq!(restored.evaluated_rules.len(), 2);
        assert!(!restored.threshold_exceeded);
    }

    #[test]
    fn governance_result_deny_serde_roundtrip() {
        let result = GovernanceResult {
            decision: GovernanceDecision::Deny("threshold exceeded".into()),
            evaluated_rules: vec![],
            effect: EffectVector {
                risk: 0.9,
                security: 0.8,
                ..Default::default()
            },
            threshold_exceeded: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: GovernanceResult = serde_json::from_str(&json).unwrap();
        assert!(restored.threshold_exceeded);
        assert!(matches!(restored.decision, GovernanceDecision::Deny(_)));
    }

    #[test]
    fn trajectory_outcome_serde_roundtrip() {
        let variants: Vec<TrajectoryOutcome> = vec![
            TrajectoryOutcome::Success { reward: 1.5 },
            TrajectoryOutcome::Failure {
                reason: "timeout".into(),
            },
            TrajectoryOutcome::Pending,
        ];
        for outcome in variants {
            let json = serde_json::to_string(&outcome).unwrap();
            let _restored: TrajectoryOutcome = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn trajectory_record_serde_roundtrip() {
        use chrono::Utc;
        let record = TrajectoryRecord {
            agent_id: "agent-1".into(),
            action: "tool.execute".into(),
            context: serde_json::json!({"tool": "read_file", "path": "/etc/hosts"}),
            outcome: TrajectoryOutcome::Success { reward: 1.0 },
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&record).unwrap();
        let restored: TrajectoryRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id, "agent-1");
        assert_eq!(restored.action, "tool.execute");
    }

    #[test]
    fn governance_rule_with_sop_fields_roundtrip() {
        let rule = GovernanceRule {
            id: "SOP-001".into(),
            description: "Do not access prod data".into(),
            branch: GovernanceBranch::Legislative,
            severity: RuleSeverity::Critical,
            active: true,
            reference_url: Some("https://sops.example.com/001".into()),
            sop_category: Some("data-access".into()),
        };
        let json = serde_json::to_string(&rule).unwrap();
        let restored: GovernanceRule = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.reference_url.unwrap(), "https://sops.example.com/001");
        assert_eq!(restored.sop_category.unwrap(), "data-access");
    }

    #[test]
    fn effect_vector_default_is_zero() {
        let v = EffectVector::default();
        assert!((v.magnitude() - 0.0).abs() < f64::EPSILON);
        assert!(!v.any_exceeds(0.0));
    }

    #[test]
    fn effect_vector_max_dimension() {
        let v = EffectVector {
            risk: 0.1,
            fairness: 0.5,
            privacy: 0.3,
            novelty: 0.9,
            security: 0.2,
        };
        assert!((v.max_dimension() - 0.9).abs() < f64::EPSILON);
    }

    #[cfg(feature = "exochain")]
    mod rvf_bridge_tests {
        use super::*;

        #[test]
        fn decision_to_policy_check() {
            use rvf_types::witness::PolicyCheck;

            assert_eq!(
                GovernanceDecision::Permit.to_rvf_policy_check(),
                PolicyCheck::Allowed,
            );
            assert_eq!(
                GovernanceDecision::PermitWithWarning("low risk".into()).to_rvf_policy_check(),
                PolicyCheck::Confirmed,
            );
            assert_eq!(
                GovernanceDecision::EscalateToHuman("needs review".into()).to_rvf_policy_check(),
                PolicyCheck::Confirmed,
            );
            assert_eq!(
                GovernanceDecision::Deny("blocked".into()).to_rvf_policy_check(),
                PolicyCheck::Denied,
            );
        }

        #[test]
        fn open_engine_maps_to_autonomous() {
            use rvf_types::witness::GovernanceMode;

            let engine = GovernanceEngine::open();
            assert_eq!(engine.to_rvf_mode(), GovernanceMode::Autonomous);
        }

        #[test]
        fn strict_engine_maps_to_restricted() {
            use rvf_types::witness::GovernanceMode;

            let engine = GovernanceEngine::new(0.5, false);
            assert_eq!(engine.to_rvf_mode(), GovernanceMode::Restricted);
        }

        #[test]
        fn human_approval_maps_to_approved() {
            use rvf_types::witness::GovernanceMode;

            let engine = GovernanceEngine::new(0.5, true);
            assert_eq!(engine.to_rvf_mode(), GovernanceMode::Approved);
        }

        #[test]
        fn to_rvf_policy_mode_matches() {
            use rvf_types::witness::GovernanceMode;

            let open = GovernanceEngine::open();
            let policy = open.to_rvf_policy();
            assert_eq!(policy.mode, GovernanceMode::Autonomous);

            let strict = GovernanceEngine::new(0.3, false);
            let policy = strict.to_rvf_policy();
            assert_eq!(policy.mode, GovernanceMode::Restricted);

            let human = GovernanceEngine::new(0.3, true);
            let policy = human.to_rvf_policy();
            assert_eq!(policy.mode, GovernanceMode::Approved);
        }

        #[test]
        fn governance_result_to_task_outcome() {
            use rvf_types::witness::TaskOutcome;

            let permit_result = GovernanceResult {
                decision: GovernanceDecision::Permit,
                evaluated_rules: vec![],
                effect: EffectVector::default(),
                threshold_exceeded: false,
            };
            assert_eq!(permit_result.to_rvf_task_outcome() as u8, TaskOutcome::Solved as u8);

            let deny_result = GovernanceResult {
                decision: GovernanceDecision::Deny("blocked".into()),
                evaluated_rules: vec![],
                effect: EffectVector::default(),
                threshold_exceeded: true,
            };
            assert_eq!(deny_result.to_rvf_task_outcome() as u8, TaskOutcome::Failed as u8);

            let escalate_result = GovernanceResult {
                decision: GovernanceDecision::EscalateToHuman("review".into()),
                evaluated_rules: vec![],
                effect: EffectVector::default(),
                threshold_exceeded: true,
            };
            assert_eq!(escalate_result.to_rvf_task_outcome() as u8, TaskOutcome::Skipped as u8);
        }
    }

    // ── ChainLoggable integration tests ────────────────────────

    #[cfg(feature = "exochain")]
    mod chain_logging_tests {
        use super::*;
        use std::sync::Arc;

        #[test]
        fn evaluate_logged_records_permit() {
            let cm = Arc::new(crate::chain::ChainManager::new(0, 100));
            let initial_len = cm.len();

            let mut engine = GovernanceEngine::new(0.5, false);
            engine.add_rule(GovernanceRule {
                id: "sec".into(),
                description: "test".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            });

            let request = GovernanceRequest {
                agent_id: "agent-1".into(),
                action: "tool.read".into(),
                effect: EffectVector { risk: 0.1, ..Default::default() },
                context: Default::default(),
                node_id: None,
            };

            let result = engine.evaluate_logged(&request, Some(&cm));
            assert!(matches!(result.decision, GovernanceDecision::Permit));
            assert_eq!(cm.len(), initial_len + 1);

            let events = cm.tail(1);
            assert_eq!(events[0].kind, "governance.permit");
            assert_eq!(events[0].source, "governance");
        }

        #[test]
        fn evaluate_logged_records_deny() {
            let cm = Arc::new(crate::chain::ChainManager::new(0, 100));

            let mut engine = GovernanceEngine::new(0.5, false);
            engine.add_rule(GovernanceRule {
                id: "sec".into(),
                description: "test".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            });

            let request = GovernanceRequest {
                agent_id: "agent-1".into(),
                action: "tool.exec".into(),
                effect: EffectVector { risk: 0.9, ..Default::default() },
                context: Default::default(),
                node_id: None,
            };

            let result = engine.evaluate_logged(&request, Some(&cm));
            assert!(matches!(result.decision, GovernanceDecision::Deny(_)));

            let events = cm.tail(1);
            assert_eq!(events[0].kind, "governance.deny");
            let payload = events[0].payload.as_ref().unwrap();
            assert_eq!(payload["agent_id"], "agent-1");
            assert!(payload["threshold_exceeded"].as_bool().unwrap());
        }

        #[test]
        fn evaluate_logged_without_chain_still_works() {
            let engine = GovernanceEngine::new(0.5, false);
            let request = GovernanceRequest {
                agent_id: "agent-1".into(),
                action: "tool.read".into(),
                effect: EffectVector::default(),
                context: Default::default(),
                node_id: None,
            };

            // Passing None should not panic, just skip logging
            let result = engine.evaluate_logged(&request, None);
            assert!(matches!(result.decision, GovernanceDecision::Permit));
        }

        #[test]
        fn chain_log_result_standalone() {
            let cm = crate::chain::ChainManager::new(0, 100);
            let initial_len = cm.len();

            let mut engine = GovernanceEngine::new(0.5, true);
            engine.add_rule(GovernanceRule {
                id: "sec".into(),
                description: "test".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            });

            let request = GovernanceRequest {
                agent_id: "agent-1".into(),
                action: "tool.exec".into(),
                effect: EffectVector { risk: 0.8, ..Default::default() },
                context: Default::default(),
                node_id: None,
            };

            let result = engine.evaluate(&request);
            GovernanceEngine::chain_log_result(&cm, &request, &result);
            assert_eq!(cm.len(), initial_len + 1);

            let events = cm.tail(1);
            assert_eq!(events[0].kind, "governance.defer");
        }
    }
}
