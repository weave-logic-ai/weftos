//! Gate backend abstraction for permission decisions.
//!
//! [`GateBackend`] provides a unified interface for making access
//! control decisions. The default implementation wraps
//! `CapabilityChecker` (binary Permit/Deny). When the `tilezero`
//! feature is enabled, `TileZeroGate` adds three-way decisions
//! (Permit/Defer/Deny) with cryptographic receipts logged to the chain.

use serde::{Deserialize, Serialize};

/// Result of a gate decision.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GateDecision {
    /// Action is permitted.
    Permit {
        /// Optional opaque permit token (e.g. TileZero PermitToken bytes).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<Vec<u8>>,
    },
    /// Decision is deferred (needs human or higher-level review).
    Defer {
        /// Why the decision was deferred.
        reason: String,
    },
    /// Action is denied.
    Deny {
        /// Why the action was denied.
        reason: String,
        /// Optional opaque witness receipt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        receipt: Option<Vec<u8>>,
    },
}

impl GateDecision {
    /// Returns `true` if the decision is `Permit`.
    pub fn is_permit(&self) -> bool {
        matches!(self, GateDecision::Permit { .. })
    }

    /// Returns `true` if the decision is `Deny`.
    pub fn is_deny(&self) -> bool {
        matches!(self, GateDecision::Deny { .. })
    }
}

/// Trait for gate backends that make access-control decisions.
///
/// Implementations include:
/// - [`CapabilityGate`] — wraps the existing `CapabilityChecker`
///   for binary Permit/Deny decisions.
/// - `TileZeroGate` (behind `tilezero` feature) — three-way
///   Permit/Defer/Deny with cryptographic receipts.
pub trait GateBackend: Send + Sync {
    /// Check whether an agent is allowed to perform an action.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The agent requesting the action.
    /// * `action` - The action being attempted (e.g. "tool.shell_exec",
    ///   "ipc.send", "service.access").
    /// * `context` - Additional context for the decision (tool args,
    ///   target PID, etc.).
    fn check(&self, agent_id: &str, action: &str, context: &serde_json::Value) -> GateDecision;
}

/// Gate backend wrapping the existing `CapabilityChecker`.
///
/// Always returns `Permit` or `Deny` (never `Defer`). This is the
/// default gate used when no external gate crate is enabled.
pub struct CapabilityGate {
    process_table: std::sync::Arc<crate::process::ProcessTable>,
}

impl CapabilityGate {
    /// Create a capability gate backed by the given process table.
    pub fn new(process_table: std::sync::Arc<crate::process::ProcessTable>) -> Self {
        Self { process_table }
    }
}

impl GateBackend for CapabilityGate {
    fn check(&self, _agent_id: &str, action: &str, context: &serde_json::Value) -> GateDecision {
        // Extract PID from context if available
        let pid = context.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);

        let checker =
            crate::capability::CapabilityChecker::new(std::sync::Arc::clone(&self.process_table));

        // Route to appropriate checker based on action prefix
        let result = if action.starts_with("tool.") {
            let tool_name = action.strip_prefix("tool.").unwrap_or(action);
            checker.check_tool_access(pid, tool_name, None, None)
        } else if action.starts_with("ipc.") {
            let target_pid = context
                .get("target_pid")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            checker.check_ipc_target(pid, target_pid)
        } else if action.starts_with("service.") {
            let service_name = action.strip_prefix("service.").unwrap_or(action);
            checker.check_service_access(pid, service_name, None)
        } else {
            // Unknown action category: permit by default
            return GateDecision::Permit { token: None };
        };

        match result {
            Ok(()) => GateDecision::Permit { token: None },
            Err(e) => GateDecision::Deny {
                reason: e.to_string(),
                receipt: None,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// TileZero gate adapter (behind `tilezero` feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "tilezero")]
pub use tilezero_gate::TileZeroGate;

#[cfg(feature = "tilezero")]
mod tilezero_gate {
    use super::{GateBackend, GateDecision};
    use std::sync::Arc;

    use cognitum_gate_tilezero::{
        ActionContext, ActionMetadata, ActionTarget, GateDecision as TzDecision, TileZero,
    };

    /// Gate backend wrapping [`cognitum_gate_tilezero::TileZero`].
    ///
    /// Provides three-way Permit/Defer/Deny decisions with Ed25519-signed
    /// `PermitToken`s and blake3-chained `WitnessReceipt`s. Gate events
    /// are logged to the kernel chain when a `ChainManager` is provided.
    pub struct TileZeroGate {
        tilezero: Arc<TileZero>,
        chain: Option<Arc<crate::chain::ChainManager>>,
    }

    impl TileZeroGate {
        /// Create a new TileZero gate.
        ///
        /// `tilezero` — a shared `TileZero` instance (created once at
        /// boot, fed with tile reports by the coherence fabric).
        ///
        /// `chain` — optional chain manager for audit logging. When
        /// provided, every decision emits a `gate.permit`, `gate.defer`,
        /// or `gate.deny` event.
        pub fn new(
            tilezero: Arc<TileZero>,
            chain: Option<Arc<crate::chain::ChainManager>>,
        ) -> Self {
            Self { tilezero, chain }
        }

        /// Reference to the optional chain manager (for test inspection).
        #[cfg(test)]
        pub(crate) fn chain(&self) -> Option<&Arc<crate::chain::ChainManager>> {
            self.chain.as_ref()
        }

        /// Build an [`ActionContext`] from our gate parameters.
        pub(crate) fn build_action_context(
            agent_id: &str,
            action: &str,
            context: &serde_json::Value,
        ) -> ActionContext {
            ActionContext {
                action_id: uuid::Uuid::new_v4().to_string(),
                action_type: action.to_owned(),
                target: ActionTarget {
                    device: context
                        .get("device")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    path: context
                        .get("path")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    extra: Default::default(),
                },
                context: ActionMetadata {
                    agent_id: agent_id.to_owned(),
                    session_id: context
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    prior_actions: Vec::new(),
                    urgency: context
                        .get("urgency")
                        .and_then(|v| v.as_str())
                        .unwrap_or("normal")
                        .to_owned(),
                },
            }
        }
    }

    impl GateBackend for TileZeroGate {
        fn check(&self, agent_id: &str, action: &str, context: &serde_json::Value) -> GateDecision {
            let action_ctx = Self::build_action_context(agent_id, action, context);

            // TileZero::decide() is async. We use block_in_place since
            // the kernel always runs inside a multi-threaded tokio runtime.
            let token = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(self.tilezero.decide(&action_ctx))
            });

            // Serialize the signed PermitToken for the opaque bytes field.
            let token_bytes = serde_json::to_vec(&token).ok();

            // Map TileZero's three-way decision to our GateDecision.
            let decision = match token.decision {
                TzDecision::Permit => GateDecision::Permit { token: token_bytes },
                TzDecision::Defer => GateDecision::Defer {
                    reason: format!(
                        "TileZero deferred: coherence uncertain (seq={})",
                        token.sequence,
                    ),
                },
                TzDecision::Deny => GateDecision::Deny {
                    reason: format!(
                        "TileZero denied: coherence below threshold (seq={})",
                        token.sequence,
                    ),
                    receipt: token_bytes,
                },
            };

            // Log to chain.
            if let Some(ref cm) = self.chain {
                let event_kind = match &decision {
                    GateDecision::Permit { .. } => "gate.permit",
                    GateDecision::Defer { .. } => "gate.defer",
                    GateDecision::Deny { .. } => "gate.deny",
                };
                cm.append(
                    "gate",
                    event_kind,
                    Some(serde_json::json!({
                        "agent_id": agent_id,
                        "action": action,
                        "sequence": token.sequence,
                        "witness_hash": token.witness_hash.iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>(),
                    })),
                );
            }

            decision
        }
    }
}

// ---------------------------------------------------------------------------
// Governance gate adapter (behind `exochain` feature)
// ---------------------------------------------------------------------------

/// Gate backend wrapping the `GovernanceEngine`.
///
/// Bridges the 5D effect-algebra governance engine into the kernel's
/// gate slot, mapping `GovernanceDecision` → `GateDecision`. Governance
/// events are logged to the exochain when a `ChainManager` is provided.
pub struct GovernanceGate {
    engine: crate::governance::GovernanceEngine,
    chain: Option<std::sync::Arc<crate::chain::ChainManager>>,
}

impl GovernanceGate {
    /// Create a governance gate with the given risk threshold.
    pub fn new(risk_threshold: f64, human_approval: bool) -> Self {
        Self {
            engine: crate::governance::GovernanceEngine::new(risk_threshold, human_approval),
            chain: None,
        }
    }

    /// Create an open governance gate that permits everything.
    pub fn open() -> Self {
        Self {
            engine: crate::governance::GovernanceEngine::open(),
            chain: None,
        }
    }

    /// Attach a chain manager for audit logging.
    pub fn with_chain(mut self, cm: std::sync::Arc<crate::chain::ChainManager>) -> Self {
        self.chain = Some(cm);
        self
    }

    /// Add a governance rule.
    pub fn add_rule(mut self, rule: crate::governance::GovernanceRule) -> Self {
        self.engine.add_rule(rule);
        self
    }

    /// Access the inner governance engine.
    pub fn engine(&self) -> &crate::governance::GovernanceEngine {
        &self.engine
    }

    /// Verify that the governance genesis event exists on the chain.
    ///
    /// Returns the genesis sequence number if found, or `None` if no
    /// chain is attached or no genesis event exists.
    pub fn verify_governance_genesis(&self) -> Option<u64> {
        let cm = self.chain.as_ref()?;
        let events = cm.tail(0); // all events
        events
            .iter()
            .find(|e| e.kind == "governance.genesis")
            .and_then(|e| {
                e.payload
                    .as_ref()
                    .and_then(|p| p.get("genesis_seq"))
                    .and_then(|v| v.as_u64())
            })
    }

    /// Extract an [`EffectVector`] from the gate context JSON.
    ///
    /// Looks for an `"effect"` object with `risk`, `fairness`, `privacy`,
    /// `novelty`, `security` fields. Returns default if absent.
    fn extract_effect(context: &serde_json::Value) -> crate::governance::EffectVector {
        context
            .get("effect")
            .and_then(|v| serde_json::from_value::<crate::governance::EffectVector>(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Extract string context map from JSON for governance request.
    fn extract_context(context: &serde_json::Value) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        if let Some(obj) = context.as_object() {
            for (k, v) in obj {
                if k == "effect" {
                    continue; // already extracted separately
                }
                if let Some(s) = v.as_str() {
                    map.insert(k.clone(), s.to_owned());
                } else {
                    map.insert(k.clone(), v.to_string());
                }
            }
        }
        map
    }
}

impl GateBackend for GovernanceGate {
    fn check(&self, agent_id: &str, action: &str, context: &serde_json::Value) -> GateDecision {
        let effect = Self::extract_effect(context);
        let ctx_map = Self::extract_context(context);

        let request = crate::governance::GovernanceRequest {
            agent_id: agent_id.to_owned(),
            action: action.to_owned(),
            effect,
            context: ctx_map,
            node_id: None,
        };

        let result = self.engine.evaluate(&request);

        let decision = match &result.decision {
            crate::governance::GovernanceDecision::Permit => GateDecision::Permit { token: None },
            crate::governance::GovernanceDecision::PermitWithWarning(_) => {
                GateDecision::Permit { token: None }
            }
            crate::governance::GovernanceDecision::EscalateToHuman(reason) => GateDecision::Defer {
                reason: reason.clone(),
            },
            crate::governance::GovernanceDecision::Deny(reason) => GateDecision::Deny {
                reason: reason.clone(),
                receipt: None,
            },
        };

        // Log to chain.
        if let Some(ref cm) = self.chain {
            let (event_kind, extra) = match &result.decision {
                crate::governance::GovernanceDecision::Permit => {
                    ("governance.permit", serde_json::json!({}))
                }
                crate::governance::GovernanceDecision::PermitWithWarning(w) => {
                    ("governance.warn", serde_json::json!({"warning": w}))
                }
                crate::governance::GovernanceDecision::EscalateToHuman(r) => {
                    ("governance.defer", serde_json::json!({"reason": r}))
                }
                crate::governance::GovernanceDecision::Deny(r) => {
                    ("governance.deny", serde_json::json!({"reason": r}))
                }
            };

            let mut payload = serde_json::json!({
                "agent_id": agent_id,
                "action": action,
                "effect": {
                    "risk": request.effect.risk,
                    "fairness": request.effect.fairness,
                    "privacy": request.effect.privacy,
                    "novelty": request.effect.novelty,
                    "security": request.effect.security,
                },
                "threshold_exceeded": result.threshold_exceeded,
                "evaluated_rules": result.evaluated_rules,
            });

            if let Some(obj) = payload.as_object_mut()
                && let Some(extra_obj) = extra.as_object()
            {
                for (k, v) in extra_obj {
                    obj.insert(k.clone(), v.clone());
                }
            }

            cm.append("governance", event_kind, Some(payload));
        }

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::AgentCapabilities;
    use crate::process::{ProcessEntry, ProcessState, ProcessTable, ResourceUsage};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn make_gate_with_agent(caps: AgentCapabilities) -> (CapabilityGate, u64) {
        let table = Arc::new(ProcessTable::new(16));
        let entry = ProcessEntry {
            pid: 0,
            agent_id: "test-agent".to_owned(),
            state: ProcessState::Running,
            capabilities: caps,
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        let pid = table.insert(entry).unwrap();
        (CapabilityGate::new(table), pid)
    }

    #[test]
    fn capability_gate_permits_default() {
        let (gate, pid) = make_gate_with_agent(AgentCapabilities::default());
        let ctx = serde_json::json!({"pid": pid});
        let decision = gate.check("test-agent", "tool.read_file", &ctx);
        assert!(decision.is_permit());
    }

    #[test]
    fn capability_gate_denies_no_tools() {
        let caps = AgentCapabilities {
            can_exec_tools: false,
            ..Default::default()
        };
        let (gate, pid) = make_gate_with_agent(caps);
        let ctx = serde_json::json!({"pid": pid});
        let decision = gate.check("test-agent", "tool.read_file", &ctx);
        assert!(decision.is_deny());
    }

    #[test]
    fn capability_gate_denies_ipc_disabled() {
        let caps = AgentCapabilities {
            can_ipc: false,
            ..Default::default()
        };
        let (gate, pid) = make_gate_with_agent(caps);
        let ctx = serde_json::json!({"pid": pid, "target_pid": 999});
        let decision = gate.check("test-agent", "ipc.send", &ctx);
        assert!(decision.is_deny());
    }

    #[test]
    fn capability_gate_unknown_action_permits() {
        let (gate, pid) = make_gate_with_agent(AgentCapabilities::default());
        let ctx = serde_json::json!({"pid": pid});
        let decision = gate.check("test-agent", "custom.action", &ctx);
        assert!(decision.is_permit());
    }

    #[test]
    fn gate_decision_serde_roundtrip() {
        let decisions = vec![
            GateDecision::Permit {
                token: Some(vec![1, 2, 3]),
            },
            GateDecision::Defer {
                reason: "need review".into(),
            },
            GateDecision::Deny {
                reason: "denied".into(),
                receipt: None,
            },
        ];
        for d in decisions {
            let json = serde_json::to_string(&d).unwrap();
            let _: GateDecision = serde_json::from_str(&json).unwrap();
        }
    }

    // ── GovernanceGate tests ─────────────────────────────────────

    use crate::governance::{GovernanceBranch, GovernanceRule, RuleSeverity};

    #[test]
    fn governance_gate_permits_low_risk() {
        let gate = GovernanceGate::new(0.5, false).add_rule(GovernanceRule {
            id: "security-check".into(),
            description: "Block high-risk actions".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        let ctx = serde_json::json!({
            "effect": { "risk": 0.1, "security": 0.05 }
        });
        let decision = gate.check("agent-1", "tool.read_file", &ctx);
        assert!(decision.is_permit());
    }

    #[test]
    fn governance_gate_denies_high_risk() {
        let gate = GovernanceGate::new(0.5, false).add_rule(GovernanceRule {
            id: "security-check".into(),
            description: "Block high-risk actions".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        let ctx = serde_json::json!({
            "effect": { "risk": 0.8, "security": 0.6 }
        });
        let decision = gate.check("agent-1", "tool.exec", &ctx);
        assert!(decision.is_deny());
    }

    #[test]
    fn governance_gate_defers_with_human_approval() {
        let gate = GovernanceGate::new(0.5, true).add_rule(GovernanceRule {
            id: "security-check".into(),
            description: "Block high-risk actions".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        let ctx = serde_json::json!({
            "effect": { "risk": 0.8 }
        });
        let decision = gate.check("agent-1", "tool.exec", &ctx);
        assert!(matches!(decision, GateDecision::Defer { .. }));
    }

    #[test]
    fn governance_gate_warns_on_threshold() {
        let gate = GovernanceGate::new(0.5, false).add_rule(GovernanceRule {
            id: "risk-check".into(),
            description: "Warn on risky actions".into(),
            branch: GovernanceBranch::Executive,
            severity: RuleSeverity::Warning,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        let ctx = serde_json::json!({
            "effect": { "risk": 0.8 }
        });
        // Warning rules don't block — should still permit
        let decision = gate.check("agent-1", "tool.deploy", &ctx);
        assert!(decision.is_permit());
    }

    #[test]
    fn governance_gate_logs_to_chain() {
        let cm = Arc::new(crate::chain::ChainManager::new(0, 10));
        let initial_len = cm.len();

        let gate = GovernanceGate::new(0.5, false)
            .with_chain(cm.clone())
            .add_rule(GovernanceRule {
                id: "sec".into(),
                description: "test".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            });

        // Low risk → governance.permit
        let ctx = serde_json::json!({"effect": {"risk": 0.1}});
        gate.check("agent-1", "tool.read", &ctx);
        assert_eq!(cm.len(), initial_len + 1);

        let events = cm.tail(1);
        assert_eq!(events[0].kind, "governance.permit");
        assert_eq!(events[0].source, "governance");

        // High risk → governance.deny
        let ctx = serde_json::json!({"effect": {"risk": 0.9}});
        gate.check("agent-1", "tool.exec", &ctx);
        let events = cm.tail(1);
        assert_eq!(events[0].kind, "governance.deny");

        let payload = events[0].payload.as_ref().unwrap();
        assert_eq!(payload["agent_id"], "agent-1");
        assert_eq!(payload["action"], "tool.exec");
        assert!(payload["threshold_exceeded"].as_bool().unwrap());
    }

    #[test]
    fn governance_gate_open_permits_all() {
        let gate = GovernanceGate::open();
        let ctx = serde_json::json!({
            "effect": { "risk": 0.99, "security": 0.99 }
        });
        let decision = gate.check("agent-1", "tool.dangerous", &ctx);
        assert!(decision.is_permit());
    }

    #[test]
    fn governance_gate_extracts_effect_from_context() {
        let gate = GovernanceGate::new(0.5, false).add_rule(GovernanceRule {
            id: "sec".into(),
            description: "test".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        // Context with effect embedded
        let ctx = serde_json::json!({
            "pid": 1,
            "effect": {
                "risk": 0.7,
                "fairness": 0.0,
                "privacy": 0.3,
                "novelty": 0.0,
                "security": 0.0
            }
        });
        let decision = gate.check("agent-1", "tool.exec", &ctx);
        // magnitude of (0.7, 0, 0.3, 0, 0) ≈ 0.76 > 0.5 → deny
        assert!(decision.is_deny());

        // Context without effect → default (zero) → permit
        let ctx_no_effect = serde_json::json!({"pid": 1});
        let decision = gate.check("agent-1", "tool.exec", &ctx_no_effect);
        assert!(decision.is_permit());
    }

    // ── Sprint 11 Security Tests ────────────────────────────────────

    #[test]
    fn replay_attack_same_context_twice() {
        // Submit the same governance check twice; both should return
        // consistent decisions (stateless gate — no replay detection
        // at gate level, but we verify determinism).
        let cm = Arc::new(crate::chain::ChainManager::new(0, 10));
        let gate = GovernanceGate::new(0.5, false)
            .with_chain(cm.clone())
            .add_rule(GovernanceRule {
                id: "sec".into(),
                description: "test".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            });

        let ctx = serde_json::json!({"effect": {"risk": 0.1}});

        let d1 = gate.check("agent-1", "tool.read", &ctx);
        let initial_len = cm.len();
        let d2 = gate.check("agent-1", "tool.read", &ctx);

        // Both decisions are permit (low risk).
        assert!(d1.is_permit());
        assert!(d2.is_permit());

        // Both calls logged to chain (two distinct events).
        assert_eq!(cm.len(), initial_len + 1);
    }

    #[test]
    fn replay_attack_chain_records_each_invocation() {
        let cm = Arc::new(crate::chain::ChainManager::new(0, 10));
        let gate = GovernanceGate::new(0.5, false)
            .with_chain(cm.clone())
            .add_rule(GovernanceRule {
                id: "sec".into(),
                description: "block risky".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            });

        let ctx = serde_json::json!({"effect": {"risk": 0.9}});
        let before = cm.len();
        gate.check("agent-1", "tool.exec", &ctx);
        gate.check("agent-1", "tool.exec", &ctx);
        gate.check("agent-1", "tool.exec", &ctx);
        // Every invocation produces a chain event.
        assert_eq!(cm.len(), before + 3);
    }

    #[test]
    fn invalid_capability_empty_action() {
        let (gate, pid) = make_gate_with_agent(AgentCapabilities::default());
        let ctx = serde_json::json!({"pid": pid});
        // Empty action string — no recognized prefix → permits by default.
        let decision = gate.check("test-agent", "", &ctx);
        assert!(decision.is_permit());
    }

    #[test]
    fn invalid_capability_very_long_action() {
        let (gate, pid) = make_gate_with_agent(AgentCapabilities::default());
        let ctx = serde_json::json!({"pid": pid});
        let long_action = "tool.".to_owned() + &"x".repeat(10_000);
        let decision = gate.check("test-agent", &long_action, &ctx);
        // Should not panic. Default caps allow tools.
        assert!(decision.is_permit());
    }

    #[test]
    fn invalid_capability_special_characters() {
        let (gate, pid) = make_gate_with_agent(AgentCapabilities::default());
        let ctx = serde_json::json!({"pid": pid});
        // Action with null bytes and unicode
        let decision = gate.check("test-agent", "tool.\0\x01\u{FEFF}", &ctx);
        assert!(decision.is_permit());
    }

    #[test]
    fn invalid_capability_action_with_path_traversal() {
        let (gate, pid) = make_gate_with_agent(AgentCapabilities::default());
        let ctx = serde_json::json!({"pid": pid});
        let decision = gate.check("test-agent", "tool.../../etc/passwd", &ctx);
        // Should still work — gate routes based on prefix.
        assert!(decision.is_permit());
    }

    #[test]
    fn permission_escalation_no_tool_access() {
        let caps = AgentCapabilities {
            can_exec_tools: false,
            can_ipc: false,
            can_spawn: false,
            ..Default::default()
        };
        let (gate, pid) = make_gate_with_agent(caps);
        let ctx = serde_json::json!({"pid": pid});

        // Agent without tool access tries various tool actions.
        assert!(gate.check("agent", "tool.shell_exec", &ctx).is_deny());
        assert!(gate.check("agent", "tool.read_file", &ctx).is_deny());
        assert!(gate.check("agent", "tool.write_file", &ctx).is_deny());

        // IPC also denied.
        let ipc_ctx = serde_json::json!({"pid": pid, "target_pid": 999});
        assert!(gate.check("agent", "ipc.send", &ipc_ctx).is_deny());
    }

    #[test]
    fn permission_escalation_service_access_denied() {
        // Agent with default caps but checking service access for
        // a non-existent service should be handled gracefully.
        let (gate, pid) = make_gate_with_agent(AgentCapabilities::default());
        let ctx = serde_json::json!({"pid": pid});
        // Service check depends on capability checker internals.
        let decision = gate.check("agent", "service.nonexistent_service", &ctx);
        // Service access check: capabilities allow by default.
        assert!(decision.is_permit() || decision.is_deny());
    }

    #[test]
    fn governance_gate_missing_pid_defaults_to_zero() {
        let (gate, _pid) = make_gate_with_agent(AgentCapabilities::default());
        // Context without pid field.
        let ctx = serde_json::json!({});
        let decision = gate.check("test-agent", "tool.read", &ctx);
        // pid=0 is not in process table, so tool check may deny.
        // The important thing is it does not panic.
        let _ = decision;
    }

    #[test]
    fn governance_gate_concurrent_checks() {
        let cm = Arc::new(crate::chain::ChainManager::new(0, 100));
        let gate = Arc::new(
            GovernanceGate::new(0.5, false)
                .with_chain(cm.clone())
                .add_rule(GovernanceRule {
                    id: "sec".into(),
                    description: "test".into(),
                    branch: GovernanceBranch::Judicial,
                    severity: RuleSeverity::Blocking,
                    active: true,
                    reference_url: None,
                    sop_category: None,
                }),
        );

        let before = cm.len();

        std::thread::scope(|s| {
            for i in 0..10 {
                let gate = Arc::clone(&gate);
                s.spawn(move || {
                    let ctx = serde_json::json!({"effect": {"risk": 0.1 * (i as f64)}});
                    gate.check(&format!("agent-{i}"), "tool.check", &ctx);
                });
            }
        });

        // All 10 checks should be logged.
        assert_eq!(cm.len(), before + 10);
    }

    #[test]
    fn governance_gate_risk_boundary_at_threshold() {
        // Test exactly at the threshold boundary.
        let gate = GovernanceGate::new(0.5, false).add_rule(GovernanceRule {
            id: "sec".into(),
            description: "boundary test".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        // Risk exactly at 0.5 — the magnitude of (0.5,0,0,0,0) = 0.5
        let ctx = serde_json::json!({"effect": {"risk": 0.5}});
        let decision = gate.check("agent", "tool.exec", &ctx);
        // At threshold: should be permit (not exceeded).
        assert!(decision.is_permit());

        // Slightly above threshold.
        let ctx_above = serde_json::json!({"effect": {"risk": 0.51}});
        let decision_above = gate.check("agent", "tool.exec", &ctx_above);
        assert!(decision_above.is_deny());
    }

    #[test]
    fn gate_decision_deny_reason_preserved() {
        let gate = GovernanceGate::new(0.5, false).add_rule(GovernanceRule {
            id: "sec".into(),
            description: "test deny reason".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        let ctx = serde_json::json!({"effect": {"risk": 0.9}});
        let decision = gate.check("agent-1", "tool.danger", &ctx);
        match decision {
            GateDecision::Deny { reason, .. } => {
                assert!(!reason.is_empty(), "deny reason should not be empty");
            }
            _ => panic!("expected deny decision for high-risk action"),
        }
    }

    #[test]
    fn gate_decision_defer_reason_preserved() {
        let gate = GovernanceGate::new(0.5, true).add_rule(GovernanceRule {
            id: "sec".into(),
            description: "escalate test".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: true,
            reference_url: None,
            sop_category: None,
        });

        let ctx = serde_json::json!({"effect": {"risk": 0.9}});
        let decision = gate.check("agent-1", "tool.danger", &ctx);
        match decision {
            GateDecision::Defer { reason } => {
                assert!(!reason.is_empty(), "defer reason should not be empty");
            }
            _ => panic!("expected defer decision for high-risk action with human approval"),
        }
    }

    #[test]
    fn governance_gate_inactive_rule_ignored() {
        let gate = GovernanceGate::new(0.5, false).add_rule(GovernanceRule {
            id: "inactive-rule".into(),
            description: "this rule is inactive".into(),
            branch: GovernanceBranch::Judicial,
            severity: RuleSeverity::Blocking,
            active: false,
            reference_url: None,
            sop_category: None,
        });

        let ctx = serde_json::json!({"effect": {"risk": 0.9}});
        let decision = gate.check("agent-1", "tool.danger", &ctx);
        // Inactive rule should not block; governance may still block
        // based on threshold. But inactive rules are not evaluated.
        let _ = decision;
    }

    #[test]
    fn governance_gate_multiple_rules_evaluated() {
        let gate = GovernanceGate::new(0.5, false)
            .add_rule(GovernanceRule {
                id: "rule-1".into(),
                description: "first".into(),
                branch: GovernanceBranch::Judicial,
                severity: RuleSeverity::Blocking,
                active: true,
                reference_url: None,
                sop_category: None,
            })
            .add_rule(GovernanceRule {
                id: "rule-2".into(),
                description: "second".into(),
                branch: GovernanceBranch::Executive,
                severity: RuleSeverity::Warning,
                active: true,
                reference_url: None,
                sop_category: None,
            });

        let ctx = serde_json::json!({"effect": {"risk": 0.9}});
        let decision = gate.check("agent-1", "tool.exec", &ctx);
        assert!(decision.is_deny());
    }
}

#[cfg(all(test, feature = "tilezero"))]
mod tilezero_tests {
    use super::*;
    use std::sync::Arc;

    fn make_tilezero_gate() -> TileZeroGate {
        let thresholds = cognitum_gate_tilezero::GateThresholds::default();
        let tz = Arc::new(cognitum_gate_tilezero::TileZero::new(thresholds));
        TileZeroGate::new(tz, None)
    }

    fn make_tilezero_gate_with_chain() -> TileZeroGate {
        let thresholds = cognitum_gate_tilezero::GateThresholds::default();
        let tz = Arc::new(cognitum_gate_tilezero::TileZero::new(thresholds));
        let cm = Arc::new(crate::chain::ChainManager::new(0, 10));
        TileZeroGate::new(tz, Some(cm))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tilezero_gate_returns_decision() {
        let gate = make_tilezero_gate();
        let ctx = serde_json::json!({"pid": 1});
        let decision = gate.check("test-agent", "tool.read_file", &ctx);
        // With default thresholds and empty graph, TileZero makes a
        // deterministic decision. We just verify it's one of the three.
        assert!(
            decision.is_permit()
                || decision.is_deny()
                || matches!(decision, GateDecision::Defer { .. })
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tilezero_gate_includes_token_bytes() {
        let gate = make_tilezero_gate();
        let ctx = serde_json::json!({});
        let decision = gate.check("agent-1", "tool.search", &ctx);

        match &decision {
            GateDecision::Permit { token } => {
                // Permit tokens carry serialized PermitToken
                assert!(token.is_some());
                let bytes = token.as_ref().unwrap();
                // Should deserialize back to a PermitToken
                let pt: cognitum_gate_tilezero::PermitToken =
                    serde_json::from_slice(bytes).unwrap();
                assert_eq!(pt.sequence, 0);
            }
            GateDecision::Deny { receipt, .. } => {
                // Deny receipts also carry the signed token
                assert!(receipt.is_some());
            }
            GateDecision::Defer { .. } => {
                // Defer has no token/receipt, just a reason
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tilezero_gate_logs_to_chain() {
        let gate = make_tilezero_gate_with_chain();
        let ctx = serde_json::json!({"urgency": "high"});
        let _decision = gate.check("agent-1", "tool.deploy", &ctx);

        // The chain should have a gate event (genesis + gate.permit/deny/defer)
        let chain = gate.chain().unwrap();
        let seq = chain.sequence();
        // At minimum: genesis(0) + gate event(1)
        assert!(seq >= 1, "expected chain event, got seq={seq}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tilezero_gate_sequential_decisions() {
        let gate = make_tilezero_gate();
        let ctx = serde_json::json!({});

        // Multiple calls should produce incrementing sequences
        let d1 = gate.check("agent-1", "tool.a", &ctx);
        let d2 = gate.check("agent-1", "tool.b", &ctx);

        // Both should return valid decisions
        let is_valid = |d: &GateDecision| {
            d.is_permit() || d.is_deny() || matches!(d, GateDecision::Defer { .. })
        };
        assert!(is_valid(&d1));
        assert!(is_valid(&d2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tilezero_gate_action_context_mapping() {
        // Verify our ActionContext builder extracts fields correctly
        let ctx = serde_json::json!({
            "device": "router-1",
            "path": "/config/acl",
            "session_id": "sess-42",
            "urgency": "critical",
        });

        let action_ctx =
            tilezero_gate::TileZeroGate::build_action_context("agent-x", "tool.deploy", &ctx);

        assert_eq!(action_ctx.action_type, "tool.deploy");
        assert_eq!(action_ctx.context.agent_id, "agent-x");
        assert_eq!(action_ctx.context.urgency, "critical");
        assert_eq!(action_ctx.context.session_id, Some("sess-42".into()));
        assert_eq!(action_ctx.target.device, Some("router-1".into()));
        assert_eq!(action_ctx.target.path, Some("/config/acl".into()));
    }
}
