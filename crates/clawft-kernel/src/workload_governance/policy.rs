//! Workload governance actions, the default-deny rule set and permit rules
//! (ADR-099 section 4, ADR-092 distribution).

use serde::{Deserialize, Serialize};

use super::effect::{NetworkPolicy, NodeTrustTier, PackageTrust, WorkloadEffect};
use crate::governance::{
    GovernanceBranch, GovernanceRule, GovernanceRuleType, RuleSeverity, selector_matches,
};

/// Prefix shared by every workload governance action.
pub const WORKLOAD_ACTION_PREFIX: &str = "workload.";

/// Install a package on a node.
pub const ACTION_INSTALL: &str = "workload.install";
/// Place an instance on a node.
pub const ACTION_PLACE: &str = "workload.place";
/// Load an instance into a runtime.
pub const ACTION_LOAD: &str = "workload.load";
/// Start a loaded instance.
pub const ACTION_START: &str = "workload.start";
/// Stop a running instance.
pub const ACTION_STOP: &str = "workload.stop";
/// Unload an instance.
pub const ACTION_UNLOAD: &str = "workload.unload";
/// Migrate an instance to another node.
pub const ACTION_MIGRATE: &str = "workload.migrate";
/// Revoke a package, signer key or artifact hash.
pub const ACTION_REVOKE: &str = "workload.revoke";
/// Bind a fleet identity to a mesh node.
pub const ACTION_NODE_BIND: &str = "workload.node.bind";

/// Every action the workload gate governs. Any other `workload.*` string is
/// refused as unknown.
pub const GOVERNED_ACTIONS: &[&str] = &[
    ACTION_INSTALL,
    ACTION_PLACE,
    ACTION_LOAD,
    ACTION_START,
    ACTION_STOP,
    ACTION_UNLOAD,
    ACTION_MIGRATE,
    ACTION_REVOKE,
    ACTION_NODE_BIND,
];

/// Rule id of the `workload.*` default-deny rule.
pub const DEFAULT_DENY_RULE_ID: &str = "WORKLOAD-DEFAULT-DENY";
/// Rule id of the effect-magnitude ceiling that still applies after a permit.
pub const EFFECT_CEILING_RULE_ID: &str = "WORKLOAD-EFFECT-CEILING";
/// SOP category marking rules that a matching permit rule lifts.
pub const DEFAULT_DENY_CATEGORY: &str = "workload.default_deny";

/// Whether `action` is one of [`GOVERNED_ACTIONS`].
pub fn is_governed_action(action: &str) -> bool {
    GOVERNED_ACTIONS.contains(&action)
}

/// The workload rules shipped in the governance rule set.
///
/// - [`DEFAULT_DENY_RULE_ID`]: blocks every `workload.*` action regardless of
///   effect magnitude (`force_on_match`). This closes the gap where
///   `CapabilityGate` and a plain `GovernanceGate` permit unknown actions.
/// - [`EFFECT_CEILING_RULE_ID`]: blocks a `workload.*` action whose effect
///   magnitude exceeds the engine threshold, even when a permit rule matched.
///
/// Both are ordinary [`GovernanceRule`]s, so they travel through
/// [`crate::rule_distribution::RuleDistribution`] like any other rule.
pub fn default_rules() -> Vec<GovernanceRule> {
    let rule = |id: &str, desc: &str, category: &str, force: bool| GovernanceRule {
        id: id.into(),
        description: desc.into(),
        branch: GovernanceBranch::Legislative,
        severity: RuleSeverity::Blocking,
        active: true,
        reference_url: None,
        sop_category: Some(category.into()),
        rule_type: GovernanceRuleType::General,
        action_selector: Some(format!("{WORKLOAD_ACTION_PREFIX}*")),
        tool_selector: None,
        force_on_match: force,
    };
    vec![
        rule(
            DEFAULT_DENY_RULE_ID,
            "workload.* actions are denied unless an explicit workload permit rule matches (ADR-099 s4)",
            DEFAULT_DENY_CATEGORY,
            true,
        ),
        rule(
            EFFECT_CEILING_RULE_ID,
            "workload.* actions above the effect threshold are denied even when permitted",
            "workload.effect_ceiling",
            false,
        ),
    ]
}

/// Seed the workload rules into a cluster rule store (ADR-092).
pub fn install_default_rules(
    dist: &mut crate::rule_distribution::RuleDistribution,
    now_unix: u64,
) {
    for rule in default_rules() {
        dist.upsert_local(rule, now_unix);
    }
}

fn default_min_trust() -> PackageTrust {
    PackageTrust::PinnedSigner
}
fn default_min_tier() -> NodeTrustTier {
    NodeTrustTier::Paired
}
fn default_max_network() -> NetworkPolicy {
    NetworkPolicy::Lan
}
fn default_max_cost() -> f64 {
    1.0
}

/// An explicit allowance that lifts the `workload.*` default deny.
///
/// Every condition must hold. Defaults are the strict ones from ADR-099
/// section 8: pinned signer, paired node, LAN at most, no secrets, no
/// emulation, no accelerator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkloadPermitRule {
    /// Unique id (recorded in the chain event when it matches).
    pub id: String,
    /// Why this permit exists.
    #[serde(default)]
    pub description: String,
    /// Action selectors (`workload.place`, or `workload.*`).
    pub actions: Vec<String>,
    /// Workload kinds allowed (`cog`, `inference`; `*` for any).
    pub kinds: Vec<String>,
    /// Least package trust allowed.
    #[serde(default = "default_min_trust")]
    pub min_package_trust: PackageTrust,
    /// Least node trust tier allowed.
    #[serde(default = "default_min_tier")]
    pub min_node_tier: NodeTrustTier,
    /// Most network exposure allowed.
    #[serde(default = "default_max_network")]
    pub max_network: NetworkPolicy,
    /// Whether secrets may be delivered.
    #[serde(default)]
    pub allow_secrets: bool,
    /// Whether emulated placement is allowed (operator opt-in).
    #[serde(default)]
    pub allow_emulated: bool,
    /// Accelerator id selectors allowed (`accel.gpu.*`). Empty: none.
    #[serde(default)]
    pub accelerators: Vec<String>,
    /// Highest resource cost allowed.
    #[serde(default = "default_max_cost")]
    pub max_resource_cost: f64,
}

impl WorkloadPermitRule {
    /// Strict permit for `actions` on workloads of `kinds`.
    pub fn new(
        id: impl Into<String>,
        actions: impl IntoIterator<Item = impl Into<String>>,
        kinds: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            description: String::new(),
            actions: actions.into_iter().map(Into::into).collect(),
            kinds: kinds.into_iter().map(Into::into).collect(),
            min_package_trust: default_min_trust(),
            min_node_tier: default_min_tier(),
            max_network: default_max_network(),
            allow_secrets: false,
            allow_emulated: false,
            accelerators: Vec::new(),
            max_resource_cost: default_max_cost(),
        }
    }

    /// Reject rules that are empty, malformed, or reach outside `workload.*`.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("permit rule id must not be empty".into());
        }
        if self.actions.is_empty() || self.kinds.is_empty() {
            return Err(format!("permit rule '{}' needs at least one action and one kind", self.id));
        }
        for a in &self.actions {
            let known = is_governed_action(a)
                || a == "workload.*"
                || (a.ends_with('*') && a.starts_with(WORKLOAD_ACTION_PREFIX));
            if !known {
                return Err(format!(
                    "permit rule '{}': action selector '{a}' is not a governed workload action",
                    self.id
                ));
            }
        }
        if self.kinds.iter().any(|k| k.is_empty()) || self.accelerators.iter().any(|a| a.is_empty()) {
            return Err(format!("permit rule '{}': empty kind or accelerator selector", self.id));
        }
        if !self.max_resource_cost.is_finite() || !(0.0..=1.0).contains(&self.max_resource_cost) {
            return Err(format!("permit rule '{}': max_resource_cost must be in [0, 1]", self.id));
        }
        Ok(())
    }

    /// Whether this rule permits `action` with `effect`.
    pub fn matches(&self, action: &str, effect: &WorkloadEffect) -> bool {
        let action_ok = self.actions.iter().any(|s| selector_matches(s, action));
        let kind_ok = self.kinds.iter().any(|k| k == "*" || k == &effect.kind);
        let accel_ok = match &effect.accelerator {
            None => true,
            Some(acc) => self.accelerators.iter().any(|s| selector_matches(s, acc)),
        };
        action_ok
            && kind_ok
            && accel_ok
            && effect.package_trust >= self.min_package_trust
            && effect.node_tier >= self.min_node_tier
            && effect.network <= self.max_network
            && (self.allow_secrets || !effect.secrets)
            && (self.allow_emulated || !effect.emulated)
            && effect.resource_cost <= self.max_resource_cost
    }
}
