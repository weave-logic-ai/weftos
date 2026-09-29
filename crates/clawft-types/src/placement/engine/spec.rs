//! `WorkloadSpec { kind, requirements, policy }` (ADR-099 sections 1, 3).
//!
//! A spec is what a workload kind produces from its package and config. It
//! speaks only the capability vocabulary: the engine never looks at `kind`
//! beyond carrying it into the decision and explain output. Everything a
//! kind wants (arch, runtime, accelerator, feed locality, measured speed)
//! arrives as requirements, execution variants, preferences and policy.
//!
//! Validated at the serde boundary (`try_from`), so a spec read from an RPC
//! or a file is either well formed or rejected.

use serde::{Deserialize, Serialize};

use super::super::PlacementTypeError;
use super::super::capability::{CapabilityId, Provenance};
use super::super::memory::MemoryDemand;
use super::super::requirement::Requirement;
use super::facts::TrustTier;

/// Longest kind string.
pub const MAX_KIND_LEN: usize = 64;
/// Longest workload name, label or revocable ref.
pub const MAX_NAME_LEN: usize = 256;
/// Most requirements in one list (common, or one variant).
pub const MAX_REQUIREMENTS: usize = 64;
/// Most execution variants.
pub const MAX_VARIANTS: usize = 16;
/// Most labels, excludes, preferences or refs.
pub const MAX_LIST: usize = 64;

/// How a variant executes on a node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    /// The payload runs on hardware of its own architecture (directly or in
    /// a same-arch container/VM).
    #[default]
    Native,
    /// The payload runs under instruction emulation. Never chosen unless
    /// the request sets `allow_emulated`.
    Emulated,
}

impl Execution {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Execution::Native => "native",
            Execution::Emulated => "emulated",
        }
    }
}

/// One way to run the workload: extra requirements plus how it executes.
/// A kind emits one variant per (arch, runtime) route it supports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionVariant {
    /// Short name shown in explain output (`aarch64-native`).
    pub name: String,
    /// Native or emulated.
    #[serde(default)]
    pub execution: Execution,
    /// Requirements added to the common set for this route.
    #[serde(default)]
    pub requirements: Vec<Requirement>,
}

/// Everything a node must provide.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkloadRequirements {
    /// Requirements every variant shares.
    #[serde(default)]
    pub common: Vec<Requirement>,
    /// Alternative execution routes. Empty means one implicit native route
    /// with no extra requirements.
    #[serde(default)]
    pub variants: Vec<ExecutionVariant>,
    /// Memory demand, checked against the node's pools per the
    /// `mem.unified` rule.
    #[serde(default)]
    pub memory: MemoryDemand,
}

/// Latency class a workload declares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    /// Throughput matters more than latency.
    #[default]
    Batch,
    /// A human or control loop waits on it: measured performance counts
    /// double and unmeasured nodes are flagged.
    Interactive,
}

/// Which way a measured number is better.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Better {
    /// Smaller is better (cycle time, latency).
    #[default]
    Lower,
    /// Larger is better (tokens per second).
    Higher,
}

/// A measured-performance signal: which `perf.*` capability to read and,
/// optionally, a budget a node's measurement must meet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfTarget {
    /// Capability id (`perf.cog.cycle_ms`).
    pub id: CapabilityId,
    /// Parameter attribute (`cog_id`).
    pub param: String,
    /// Parameter value (the cog or model this measurement is for).
    pub param_value: String,
    /// Direction.
    #[serde(default)]
    pub better: Better,
    /// Hard budget: a node whose measurement misses it is filtered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<f64>,
}

/// A soft preference: a requirement that adds `weight` when a node meets it
/// (data locality: `model.present`, a feed on the same LAN, a mounted tier).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preference {
    /// Short name for explain output.
    pub name: String,
    /// What the node must advertise to earn the weight.
    pub requirement: Requirement,
    /// Score added (finite, non-negative).
    pub weight: f64,
}

/// Placement policy from the package and operator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementPolicy {
    /// Lowest node trust tier allowed.
    #[serde(default = "default_tier")]
    pub min_trust: TrustTier,
    /// Lowest provenance any requirement accepts (raises weaker
    /// per-requirement minimums).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_provenance: Option<Provenance>,
    /// This workload's co-residency labels.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Labels this workload must not share a node with.
    #[serde(default)]
    pub excludes: Vec<String>,
    /// Latency class.
    #[serde(default)]
    pub latency_class: LatencyClass,
    /// Measured-performance signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perf: Option<PerfTarget>,
    /// Soft preferences (locality and similar).
    #[serde(default)]
    pub preferences: Vec<Preference>,
    /// Strong stickiness to the current node (warm state, ADR-101 s6).
    #[serde(default)]
    pub sticky: bool,
    /// Package ids, signer keys and artifact hashes; any revoked one makes
    /// the workload unplaceable everywhere.
    #[serde(default)]
    pub revocable_refs: Vec<String>,
}

fn default_tier() -> TrustTier {
    TrustTier::Paired
}

impl Default for PlacementPolicy {
    fn default() -> Self {
        Self {
            min_trust: default_tier(),
            min_provenance: None,
            labels: vec![],
            excludes: vec![],
            latency_class: LatencyClass::Batch,
            perf: None,
            preferences: vec![],
            sticky: false,
            revocable_refs: vec![],
        }
    }
}

/// A workload as the placement engine sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WorkloadSpecRaw")]
pub struct WorkloadSpec {
    /// Open kind string (`cog`, `inference`, ...). Carried, never branched on.
    pub kind: String,
    /// Human name for explain output.
    pub name: String,
    /// Hard requirements.
    #[serde(default)]
    pub requirements: WorkloadRequirements,
    /// Policy.
    #[serde(default)]
    pub policy: PlacementPolicy,
}

#[derive(Deserialize)]
struct WorkloadSpecRaw {
    kind: String,
    name: String,
    #[serde(default)]
    requirements: WorkloadRequirements,
    #[serde(default)]
    policy: PlacementPolicy,
}

impl TryFrom<WorkloadSpecRaw> for WorkloadSpec {
    type Error = PlacementTypeError;
    fn try_from(r: WorkloadSpecRaw) -> Result<Self, Self::Error> {
        let s = WorkloadSpec {
            kind: r.kind,
            name: r.name,
            requirements: r.requirements,
            policy: r.policy,
        };
        s.validate()?;
        Ok(s)
    }
}

fn bad(why: impl Into<String>) -> PlacementTypeError {
    PlacementTypeError::InvalidRequirement(why.into())
}

fn check_text(what: &str, s: &str) -> Result<(), PlacementTypeError> {
    if s.is_empty() || s.len() > MAX_NAME_LEN || s.chars().any(char::is_control) {
        return Err(bad(format!(
            "{what} must be 1..={MAX_NAME_LEN} printable bytes"
        )));
    }
    Ok(())
}

fn check_list(what: &str, v: &[String]) -> Result<(), PlacementTypeError> {
    if v.len() > MAX_LIST {
        return Err(bad(format!("too many {what} (max {MAX_LIST})")));
    }
    v.iter().try_for_each(|s| check_text(what, s))
}

fn check_reqs(v: &[Requirement]) -> Result<(), PlacementTypeError> {
    if v.len() > MAX_REQUIREMENTS {
        return Err(bad(format!(
            "too many requirements (max {MAX_REQUIREMENTS})"
        )));
    }
    v.iter().try_for_each(Requirement::validate)
}

impl WorkloadSpec {
    /// A spec with no requirements and default policy.
    pub fn new(kind: &str, name: &str) -> Self {
        Self {
            kind: kind.to_string(),
            name: name.to_string(),
            requirements: WorkloadRequirements::default(),
            policy: PlacementPolicy::default(),
        }
    }

    /// Check every bound. Called on deserialize and by `place`.
    pub fn validate(&self) -> Result<(), PlacementTypeError> {
        let k = &self.kind;
        if k.is_empty()
            || k.len() > MAX_KIND_LEN
            || !k
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(bad("kind must be 1..=64 of [a-z0-9_-]"));
        }
        check_text("name", &self.name)?;
        let r = &self.requirements;
        check_reqs(&r.common)?;
        if r.variants.len() > MAX_VARIANTS {
            return Err(bad(format!("too many variants (max {MAX_VARIANTS})")));
        }
        for v in &r.variants {
            check_text("variant name", &v.name)?;
            check_reqs(&v.requirements)?;
        }
        let p = &self.policy;
        check_list("labels", &p.labels)?;
        check_list("excludes", &p.excludes)?;
        check_list("revocable refs", &p.revocable_refs)?;
        if p.preferences.len() > MAX_LIST {
            return Err(bad(format!("too many preferences (max {MAX_LIST})")));
        }
        for pref in &p.preferences {
            check_text("preference name", &pref.name)?;
            pref.requirement.validate()?;
            if !pref.weight.is_finite() || pref.weight < 0.0 {
                return Err(bad("preference weight must be finite and non-negative"));
            }
        }
        if let Some(t) = &p.perf {
            check_text("perf param", &t.param)?;
            check_text("perf param value", &t.param_value)?;
            if t.budget.is_some_and(|b| !b.is_finite() || b < 0.0) {
                return Err(bad("perf budget must be finite and non-negative"));
            }
        }
        Ok(())
    }

    /// Variants to try: the declared ones, or one implicit native route.
    pub fn effective_variants(&self) -> Vec<ExecutionVariant> {
        if self.requirements.variants.is_empty() {
            vec![ExecutionVariant {
                name: "default".to_string(),
                execution: Execution::Native,
                requirements: vec![],
            }]
        } else {
            self.requirements.variants.clone()
        }
    }
}
