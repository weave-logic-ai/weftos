//! Engine tests: table-driven over a synthetic fleet (see [`fixtures`]).

mod fixtures;
mod tests_bad_inputs;
mod tests_constraints;
mod tests_explain;
mod tests_pins;
mod tests_scoring;
mod tests_tiers;

use super::{ClusterState, Constraint, Decision, PlacementRequest, place};
use fixtures::TestNode;

/// Cluster at t = 1000 ms (fixture facts expire at 10 000 ms).
pub(crate) fn cluster() -> ClusterState {
    ClusterState {
        now_ms: 1_000,
        ..ClusterState::default()
    }
}

pub(crate) fn run(req: &PlacementRequest, nodes: &[TestNode]) -> Decision {
    place(req, nodes, &cluster()).expect("valid request")
}

/// Constraints that rejected `node` in `d`.
pub(crate) fn rejected_by(d: &Decision, node: &str) -> Vec<Constraint> {
    d.candidates
        .iter()
        .find(|c| c.node_id == node)
        .unwrap_or_else(|| panic!("{node} not in candidates"))
        .rejections
        .iter()
        .map(|r| r.constraint)
        .collect()
}

pub(crate) fn placed_on(d: &Decision) -> Option<&str> {
    d.placement.as_ref().map(|p| p.node_id.as_str())
}
