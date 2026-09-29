//! What `place` returns: a placement with its score breakdown, or
//! `Unplaceable` with per-node reasons; plus the error for an ineligible pin.

use serde::{Deserialize, Serialize};

use super::super::PlacementTypeError;
use super::spec::Execution;

/// Which constraint rejected a node. `as_str` names are stable: they appear
/// in pin errors, explain output and chain events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Constraint {
    /// A requirement is not met by the advertised capabilities.
    Requirements,
    /// A capability exists but with too low a provenance.
    Provenance,
    /// Not enough memory in the right pool.
    Memory,
    /// Node trust tier below the policy minimum.
    Trust,
    /// Node not alive.
    Liveness,
    /// Advertised facts past their TTL.
    FactsExpired,
    /// Node revoked.
    NodeRevoked,
    /// A package, signer or artifact of the workload is revoked.
    WorkloadRevoked,
    /// An instance on the node excludes this workload, or vice versa.
    CoResidency,
    /// Only an emulated route fits, and emulation was not requested.
    EmulationNotAllowed,
    /// Governance denied `workload.place` on this node.
    Gate,
    /// Measured performance misses the declared budget (phase B filter).
    PerfBudget,
}

impl Constraint {
    /// Stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Constraint::Requirements => "requirements",
            Constraint::Provenance => "provenance",
            Constraint::Memory => "memory",
            Constraint::Trust => "trust",
            Constraint::Liveness => "liveness",
            Constraint::FactsExpired => "facts_expired",
            Constraint::NodeRevoked => "node_revoked",
            Constraint::WorkloadRevoked => "workload_revoked",
            Constraint::CoResidency => "co_residency",
            Constraint::EmulationNotAllowed => "emulation_not_allowed",
            Constraint::Gate => "gate",
            Constraint::PerfBudget => "perf_budget",
        }
    }
}

/// One failed constraint on one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// Which constraint.
    pub constraint: Constraint,
    /// Human-readable detail.
    pub detail: String,
}

/// Per-component score, so the decision explains itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    /// Native / dev fallback / emulated tier.
    pub execution: f64,
    /// Preferences met (data locality and similar).
    pub locality: f64,
    /// Accelerator fit.
    pub accel_fit: f64,
    /// Measured performance.
    pub perf: f64,
    /// Load headroom.
    pub load: f64,
    /// Stickiness to the current node.
    pub stickiness: f64,
}

impl ScoreBreakdown {
    /// Sum of all components.
    pub fn total(&self) -> f64 {
        self.execution + self.locality + self.accel_fit + self.perf + self.load + self.stickiness
    }
}

/// Execution tier a node earns for its chosen route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Native on real hardware.
    Native,
    /// Native on a dev-fallback node.
    DevFallback,
    /// Emulated (operator opt-in only).
    Emulated,
}

impl Tier {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Native => "native",
            Tier::DevFallback => "dev_fallback",
            Tier::Emulated => "emulated",
        }
    }
}

/// A note attached to a candidate that did not reject it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// No measurement for the declared perf signal: scored conservatively.
    Unmeasured,
    /// Chosen route is emulated.
    Emulated,
    /// Chosen because of an operator pin.
    Pinned,
    /// Chosen because of operator affinity rather than score alone.
    Affinity,
    /// Load unknown: scored as fully loaded.
    LoadUnknown,
}

impl Flag {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Flag::Unmeasured => "unmeasured",
            Flag::Emulated => "emulated",
            Flag::Pinned => "pinned",
            Flag::Affinity => "affinity",
            Flag::LoadUnknown => "load_unknown",
        }
    }
}

/// How one node fared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeReport {
    /// Node id.
    pub node_id: String,
    /// Empty when the node is eligible.
    pub rejections: Vec<Rejection>,
    /// Chosen variant name, when eligible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Tier of the chosen variant, when eligible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// Score, when eligible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<ScoreBreakdown>,
    /// Flags.
    #[serde(default)]
    pub flags: Vec<Flag>,
    /// Capability ids assigned per requirement (common, then variant).
    #[serde(default)]
    pub assigned: Vec<Vec<String>>,
}

impl NodeReport {
    /// True if no constraint rejected the node.
    pub fn eligible(&self) -> bool {
        self.rejections.is_empty()
    }
}

/// A successful placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Chosen node.
    pub node_id: String,
    /// Chosen variant.
    pub variant: String,
    /// Native or emulated (chained as `emulated: true|false`).
    pub execution: Execution,
    /// Tier.
    pub tier: Tier,
    /// First assigned `accel.*` capability, for the governance effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accelerator: Option<String>,
    /// Score breakdown.
    pub score: ScoreBreakdown,
    /// Flags on the chosen node.
    pub flags: Vec<Flag>,
}

impl Placement {
    /// True for an emulated route.
    pub fn emulated(&self) -> bool {
        self.execution == Execution::Emulated
    }
}

/// Outcome of the pure placement function.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// Workload kind (carried, never interpreted).
    pub kind: String,
    /// Workload name.
    pub name: String,
    /// Whether the operator opted into emulation.
    pub allow_emulated: bool,
    /// The operator pin, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// `Some` when placed; `None` means unplaceable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// Every node, ranked: eligible by score, then rejected by node id.
    pub candidates: Vec<NodeReport>,
}

impl Decision {
    /// True when no node could take the workload.
    pub fn is_unplaceable(&self) -> bool {
        self.placement.is_none()
    }

    /// Per-node reasons (rejected nodes only).
    pub fn reasons(&self) -> impl Iterator<Item = &NodeReport> {
        self.candidates.iter().filter(|c| !c.eligible())
    }
}

/// `place` could not produce a decision.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlacementError {
    /// The request or spec failed validation.
    #[error("invalid placement request: {0}")]
    Invalid(#[from] PlacementTypeError),
    /// Two fact records share a node id.
    #[error("duplicate node id {0:?} in facts")]
    DuplicateNode(String),
    /// The pinned node is not in the facts.
    #[error("pinned node {0:?} is not a known node")]
    UnknownPin(String),
    /// The pinned node fails a constraint. Pins override scoring, never
    /// constraints.
    #[error("pinned node {node:?} is ineligible: {}", describe(.rejections))]
    PinIneligible {
        /// Pinned node.
        node: String,
        /// Every failed constraint.
        rejections: Vec<Rejection>,
    },
}

impl PlacementError {
    /// Constraints named by a pin error (empty for other errors).
    pub fn constraints(&self) -> Vec<Constraint> {
        match self {
            PlacementError::PinIneligible { rejections, .. } => {
                rejections.iter().map(|r| r.constraint).collect()
            }
            _ => vec![],
        }
    }
}

fn describe(r: &[Rejection]) -> String {
    r.iter()
        .map(|r| format!("constraint `{}`: {}", r.constraint.as_str(), r.detail))
        .collect::<Vec<_>>()
        .join("; ")
}
