//! [`WorkloadGate`]: the governance gate for `workload.*` actions.
//!
//! Evaluation order (every step's outcome is chained, ADR-022):
//!
//! 1. The action must be one of [`GOVERNED_ACTIONS`]; an unknown
//!    `workload.*` action is refused and chained as `workload.refuse`.
//! 2. The `workload` context object must parse and validate.
//! 3. The subject revocation list must be readable, and no package id,
//!    signer key or artifact hash in the request may be revoked.
//! 4. Workloads carrying secrets need a `pinned` node (ADR-099 s8.3).
//! 5. A [`WorkloadPermitRule`] must match; otherwise the default deny holds.
//!    This is enforced in code as well as by the `WORKLOAD-DEFAULT-DENY`
//!    rule, so a rule set that lost that rule still fails closed.
//! 6. The governance engine evaluates the derived 5D effect vector with
//!    every rule except the default-deny category (effect ceiling and any
//!    other distributed rules still apply).

use std::sync::Arc;

use super::effect::{NodeTrustTier, WorkloadRequest};
use super::policy::{
    DEFAULT_DENY_CATEGORY, DEFAULT_DENY_RULE_ID, GOVERNED_ACTIONS, WORKLOAD_ACTION_PREFIX,
    WorkloadPermitRule, default_rules, is_governed_action,
};
use crate::chain::{self, ChainManager};
use crate::gate::{GateBackend, GateDecision};
use crate::governance::{
    GatePrincipal, GovernanceDecision, GovernanceEngine, GovernanceRequest, GovernanceRule,
};
use crate::revocation::{RevocationKind, RevocationList};

/// Chain source used for workload governance events.
pub const CHAIN_SOURCE: &str = "workload";

/// Default effect-magnitude threshold for workload actions.
pub const DEFAULT_THRESHOLD: f64 = 0.8;

/// Chain event kind for a governed action (`None` for unknown actions).
pub fn event_kind_for(action: &str) -> Option<&'static str> {
    Some(match action {
        "workload.install" => chain::EVENT_KIND_WORKLOAD_INSTALL,
        "workload.place" => chain::EVENT_KIND_WORKLOAD_PLACE,
        "workload.load" => chain::EVENT_KIND_WORKLOAD_LOAD,
        "workload.start" => chain::EVENT_KIND_WORKLOAD_START,
        "workload.stop" => chain::EVENT_KIND_WORKLOAD_STOP,
        "workload.unload" => chain::EVENT_KIND_WORKLOAD_UNLOAD,
        "workload.migrate" => chain::EVENT_KIND_WORKLOAD_MIGRATE,
        "workload.revoke" => chain::EVENT_KIND_WORKLOAD_REVOKE,
        "workload.node.bind" => chain::EVENT_KIND_WORKLOAD_NODE_BIND,
        _ => return None,
    })
}

/// Governance gate for workload actions.
pub struct WorkloadGate {
    /// Engine with every rule except the default-deny category.
    permitted_engine: GovernanceEngine,
    /// Rule ids evaluated when no permit matched (for the audit record).
    default_rule_ids: Vec<String>,
    permits: Vec<WorkloadPermitRule>,
    chain: Option<Arc<ChainManager>>,
    revocations: Option<Arc<RevocationList>>,
}

impl WorkloadGate {
    /// Gate with the shipped [`default_rules`] and no permits (denies all).
    pub fn new(risk_threshold: f64, human_approval: bool) -> Self {
        Self::with_rules(risk_threshold, human_approval, default_rules())
    }

    /// Gate over a distributed rule set (e.g. `RuleDistribution::active_rules`).
    ///
    /// Only rules whose selectors can match `workload.*` actions matter; the
    /// rest are carried but never match.
    pub fn with_rules(
        risk_threshold: f64,
        human_approval: bool,
        rules: Vec<GovernanceRule>,
    ) -> Self {
        let mut permitted_engine = GovernanceEngine::new(risk_threshold, human_approval);
        let mut default_rule_ids = Vec::new();
        for rule in rules {
            if rule.sop_category.as_deref() == Some(DEFAULT_DENY_CATEGORY) {
                if rule.active {
                    default_rule_ids.push(rule.id.clone());
                }
            } else {
                permitted_engine.add_rule(rule);
            }
        }
        if default_rule_ids.is_empty() {
            default_rule_ids.push(format!("{DEFAULT_DENY_RULE_ID} (built-in)"));
        }
        Self {
            permitted_engine,
            default_rule_ids,
            permits: Vec::new(),
            chain: None,
            revocations: None,
        }
    }

    /// Add a permit rule after validating it.
    pub fn with_permit(mut self, rule: WorkloadPermitRule) -> Result<Self, String> {
        rule.validate()?;
        if self.permits.iter().any(|p| p.id == rule.id) {
            return Err(format!("duplicate permit rule id '{}'", rule.id));
        }
        self.permits.push(rule);
        Ok(self)
    }

    /// Attach a chain manager for audit events.
    pub fn with_chain(mut self, cm: Arc<ChainManager>) -> Self {
        self.chain = Some(cm);
        self
    }

    /// Attach the revocation list checked on every request.
    pub fn with_revocations(mut self, list: Arc<RevocationList>) -> Self {
        self.revocations = Some(list);
        self
    }

    /// Permit rules in evaluation order.
    pub fn permits(&self) -> &[WorkloadPermitRule] {
        &self.permits
    }

    fn deny(reason: impl Into<String>) -> GateDecision {
        GateDecision::Deny {
            reason: reason.into(),
            receipt: None,
        }
    }

    fn record(&self, kind: &str, payload: serde_json::Value) {
        if let Some(cm) = &self.chain {
            cm.append(CHAIN_SOURCE, kind, Some(payload));
        }
    }

    fn decide(&self, agent_id: &str, action: &str, context: &serde_json::Value) -> Outcome {
        let mut out = Outcome::new();
        let req = match WorkloadRequest::from_context(context) {
            Ok(r) => r,
            Err(e) => {
                out.decision = Self::deny(format!("invalid workload context: {e}"));
                return out;
            }
        };
        out.workload = serde_json::to_value(&req.effect).ok();
        let vector = req.effect.to_effect_vector();
        out.effect = serde_json::to_value(&vector).ok();

        if let Some(list) = &self.revocations {
            if let Some(err) = list.subjects_error() {
                out.decision = Self::deny(format!("revocation list unreadable (fail closed): {err}"));
                return out;
            }
            let hit = list.first_revoked(
                req.refs.package_id.as_deref(),
                &req.refs.signer_keys,
                &req.refs.artifact_hashes,
            );
            if let Some(s) = hit {
                out.decision = Self::deny(format!("{} '{}' is revoked: {}", s.kind, s.id, s.reason));
                out.revoked = serde_json::to_value(&s).ok();
                return out;
            }
        }

        if req.effect.secrets && req.effect.node_tier < NodeTrustTier::Pinned {
            out.decision = Self::deny("workloads carrying secrets require a pinned node (ADR-099 s8.3)");
            return out;
        }

        let Some(permit) = self.permits.iter().find(|p| p.matches(action, &req.effect)) else {
            out.evaluated_rules = self.default_rule_ids.clone();
            out.decision = Self::deny(format!(
                "default deny: no workload permit rule matches '{action}' for kind '{}' (rule {})",
                req.effect.kind,
                self.default_rule_ids.join(", ")
            ));
            return out;
        };
        out.permit_rule = Some(permit.id.clone());

        let request = GovernanceRequest {
            agent_id: agent_id.to_owned(),
            action: action.to_owned(),
            effect: vector,
            context: req.effect.context_map(),
            node_id: None,
            principal: Some(GatePrincipal::agent(agent_id)),
        };
        let result = self.permitted_engine.evaluate(&request);
        out.evaluated_rules = result.evaluated_rules;
        out.threshold_exceeded = result.threshold_exceeded;
        out.decision = match result.decision {
            GovernanceDecision::Permit | GovernanceDecision::PermitWithWarning(_) => {
                GateDecision::Permit { token: None }
            }
            GovernanceDecision::EscalateToHuman(reason) => GateDecision::Defer { reason },
            GovernanceDecision::Deny(reason) => Self::deny(reason),
        };
        out
    }
}

/// Intermediate result, turned into the chain payload.
struct Outcome {
    decision: GateDecision,
    workload: Option<serde_json::Value>,
    effect: Option<serde_json::Value>,
    permit_rule: Option<String>,
    evaluated_rules: Vec<String>,
    threshold_exceeded: bool,
    revoked: Option<serde_json::Value>,
}

impl Outcome {
    fn new() -> Self {
        Self {
            decision: GateDecision::Deny {
                reason: "undecided".into(),
                receipt: None,
            },
            workload: None,
            effect: None,
            permit_rule: None,
            evaluated_rules: Vec::new(),
            threshold_exceeded: false,
            revoked: None,
        }
    }
}

fn decision_parts(d: &GateDecision) -> (&'static str, Option<&str>) {
    match d {
        GateDecision::Permit { .. } => ("permit", None),
        GateDecision::Defer { reason } => ("defer", Some(reason)),
        GateDecision::Deny { reason, .. } => ("deny", Some(reason)),
    }
}

impl GateBackend for WorkloadGate {
    fn check(&self, agent_id: &str, action: &str, context: &serde_json::Value) -> GateDecision {
        if !action.starts_with(WORKLOAD_ACTION_PREFIX) {
            // Not ours: refuse rather than silently permit. Not chained,
            // because the workload audit trail covers workload.* only.
            return Self::deny(format!("WorkloadGate only governs workload.* actions, got '{action}'"));
        }
        if !is_governed_action(action) {
            let reason = format!(
                "unknown workload action '{action}'; governed actions: {}",
                GOVERNED_ACTIONS.join(", ")
            );
            self.record(
                chain::EVENT_KIND_WORKLOAD_REFUSE,
                serde_json::json!({
                    "decision": "deny",
                    "agent_id": agent_id,
                    "action": action,
                    "reason": reason,
                }),
            );
            return Self::deny(reason);
        }

        let out = self.decide(agent_id, action, context);
        let (decision, reason) = decision_parts(&out.decision);
        let kind = event_kind_for(action).unwrap_or(chain::EVENT_KIND_WORKLOAD_REFUSE);
        self.record(
            kind,
            serde_json::json!({
                "decision": decision,
                "reason": reason,
                "agent_id": agent_id,
                "action": action,
                "workload": out.workload,
                "effect": out.effect,
                "permit_rule": out.permit_rule,
                "evaluated_rules": out.evaluated_rules,
                "threshold_exceeded": out.threshold_exceeded,
                "revoked": out.revoked,
            }),
        );
        out.decision
    }
}

/// Revoke a subject and chain a `workload.revoke` event when newly added.
///
/// Returns `Ok(true)` if newly revoked, `Ok(false)` if it already was.
pub fn revoke_and_record(
    list: &RevocationList,
    chain: Option<&ChainManager>,
    kind: RevocationKind,
    id: &str,
    reason: &str,
    revoked_by: &str,
) -> Result<bool, crate::revocation::RevocationError> {
    let added = list.revoke_subject(kind, id, reason)?;
    if added && let Some(cm) = chain {
        let canonical = kind.normalize(id)?;
        cm.append(
            CHAIN_SOURCE,
            chain::EVENT_KIND_WORKLOAD_REVOKE,
            Some(serde_json::json!({
                "decision": "revoked",
                "subject_kind": kind,
                "subject_id": canonical,
                "reason": reason,
                "revoked_by": revoked_by,
            })),
        );
    }
    Ok(added)
}
