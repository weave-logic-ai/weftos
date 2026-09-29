//! A placement request: the workload plus operator overrides, the
//! governance verdict and scoring weights (ADR-099 section 3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::super::PlacementTypeError;
use super::spec::{MAX_LIST, MAX_NAME_LEN, WorkloadSpec};

/// Vocabulary id marking a developer machine: native routes there score as
/// the dev fallback, not as real hardware (ADR-099 section 3; COG-001 s2).
pub const DEV_FALLBACK_CLASS: &str = "node.class.dev-mac";

/// One `workload.place` gate verdict for one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum GateVerdict {
    /// Permitted.
    Permit,
    /// Denied, with the gate's reason.
    Deny {
        /// Why.
        reason: String,
    },
}

/// The governance decision for `workload.place`, computed and chained by the
/// caller **before** `place` runs. The engine only reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum GateInput {
    /// One verdict for every node.
    Uniform {
        /// The verdict.
        verdict: GateVerdict,
    },
    /// A verdict per node id. A node with no entry is denied (fail closed).
    PerNode {
        /// Verdicts by node id.
        verdicts: BTreeMap<String, GateVerdict>,
    },
}

impl GateInput {
    /// Permit every node.
    pub fn permit_all() -> Self {
        GateInput::Uniform {
            verdict: GateVerdict::Permit,
        }
    }

    /// Verdict for `node_id`.
    pub fn verdict_for(&self, node_id: &str) -> GateVerdict {
        match self {
            GateInput::Uniform { verdict } => verdict.clone(),
            GateInput::PerNode { verdicts } => {
                verdicts.get(node_id).cloned().unwrap_or(GateVerdict::Deny {
                    reason: "no gate decision for this node".to_string(),
                })
            }
        }
    }
}

/// Operator affinity. Overrides scoring among eligible nodes, never
/// constraints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Affinity {
    /// If any of these is eligible, choose among them only.
    #[serde(default)]
    pub prefer: Vec<String>,
    /// Choose these only if nothing else is eligible.
    #[serde(default)]
    pub avoid: Vec<String>,
}

/// Scoring weights. Placeholders that governance config tunes (ADR-099 s3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoringWeights {
    /// Native route on real hardware.
    pub native: f64,
    /// Native route on a dev-fallback node ([`DEV_FALLBACK_CLASS`]).
    pub dev_fallback: f64,
    /// Emulated route.
    pub emulated: f64,
    /// Node class id that marks the dev fallback.
    pub dev_fallback_class: String,
    /// Accelerator fit (smallest sufficient accelerator; no accelerator
    /// wasted on a workload that does not need one).
    pub accel_fit: f64,
    /// Measured performance, best measured node gets the full weight.
    pub perf: f64,
    /// Multiplier on `perf` for interactive workloads.
    pub interactive_perf_multiplier: f64,
    /// Idle node gets the full weight; fully loaded gets none.
    pub load: f64,
    /// Current node, ordinary workload.
    pub stickiness: f64,
    /// Current node, workload with `policy.sticky`.
    pub sticky: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            native: 100.0,
            dev_fallback: 40.0,
            emulated: 20.0,
            dev_fallback_class: DEV_FALLBACK_CLASS.to_string(),
            accel_fit: 10.0,
            perf: 30.0,
            interactive_perf_multiplier: 2.0,
            load: 10.0,
            stickiness: 15.0,
            sticky: 1000.0,
        }
    }
}

impl ScoringWeights {
    fn validate(&self) -> Result<(), PlacementTypeError> {
        let all = [
            self.native,
            self.dev_fallback,
            self.emulated,
            self.accel_fit,
            self.perf,
            self.interactive_perf_multiplier,
            self.load,
            self.stickiness,
            self.sticky,
        ];
        if all.iter().any(|w| !w.is_finite() || *w < 0.0) {
            return Err(PlacementTypeError::InvalidRequirement(
                "scoring weights must be finite and non-negative".into(),
            ));
        }
        Ok(())
    }
}

/// `(workload, pins/affinity, allow_emulated?)` plus the gate verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementRequest {
    /// The workload.
    pub spec: WorkloadSpec,
    /// Operator pin: place here or fail naming the constraint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// Operator affinity.
    #[serde(default)]
    pub affinity: Affinity,
    /// Operator opt-in to an emulated route. Never set automatically.
    #[serde(default)]
    pub allow_emulated: bool,
    /// Node the workload runs on now (stickiness), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_node: Option<String>,
    /// The `workload.place` gate verdict(s).
    pub gate: GateInput,
    /// Scoring weights.
    #[serde(default)]
    pub weights: ScoringWeights,
}

impl PlacementRequest {
    /// A request with no overrides, emulation off, gate permitting all.
    pub fn new(spec: WorkloadSpec) -> Self {
        Self {
            spec,
            pin: None,
            affinity: Affinity::default(),
            allow_emulated: false,
            current_node: None,
            gate: GateInput::permit_all(),
            weights: ScoringWeights::default(),
        }
    }

    /// Validate the spec, node ids and weights.
    pub fn validate(&self) -> Result<(), PlacementTypeError> {
        self.spec.validate()?;
        self.weights.validate()?;
        let ids = self
            .pin
            .iter()
            .chain(self.current_node.iter())
            .chain(self.affinity.prefer.iter())
            .chain(self.affinity.avoid.iter());
        for id in ids {
            if id.is_empty() || id.len() > MAX_NAME_LEN || id.chars().any(char::is_control) {
                return Err(PlacementTypeError::InvalidRequirement(
                    "node ids must be 1..=256 printable bytes".into(),
                ));
            }
        }
        if self.affinity.prefer.len() > MAX_LIST || self.affinity.avoid.len() > MAX_LIST {
            return Err(PlacementTypeError::InvalidRequirement(
                "too many affinity entries".into(),
            ));
        }
        Ok(())
    }
}
