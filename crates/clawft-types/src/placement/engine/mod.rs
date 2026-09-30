//! Pure placement engine (ADR-099 section 3; card mesh-placement-04).
//!
//! `place(request, facts[], cluster_state) -> Decision` is a pure function:
//! no clock, no I/O, no governance calls. The caller computes and chains
//! the `workload.place` gate verdict first and passes it in
//! ([`GateInput`]); the caller chains the returned decision.
//!
//! 1. **Phase A, hard constraints** ([`constraints`]): requirements (wave 1
//!    [`match_all`](super::match_all)), provenance, memory (`mem.unified`
//!    rule), trust tier, liveness, fact TTL, node and workload revocation,
//!    co-residency `excludes` (never against the workload's own instance),
//!    emulation opt-in, gate. All failures recorded.
//! 2. **Phase B, scoring** ([`score`]): execution tier; preferences
//!    (locality); accelerator fit; measured performance with a budget
//!    filter (non-finite or negative measurements count as unmeasured);
//!    load; stickiness.
//! 3. **Selection**: an operator pin wins if the node is eligible and is a
//!    hard error naming the failed constraints otherwise. Otherwise tiers
//!    are strict (ADR-099 section 3, decided 2026-09-29): native on real
//!    target hardware, then emulated, then the dev-mac fallback. Only the
//!    best tier present is considered, so no score component or affinity
//!    lifts a lower tier over a higher one. Emulated routes exist only when
//!    the operator set `allow_emulated`. Within the tier, affinity
//!    (`prefer`, `avoid`) overrides score. Ties break by node id.
//!
//! No code here branches on the workload kind: kinds speak only through
//! capabilities, variants, preferences and policy.

mod constraints;
mod decision;
mod explain;
mod facts;
mod request;
mod score;
mod spec;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

pub use decision::{
    Constraint, Decision, Flag, NodeReport, Placement, PlacementError, Rejection, ScoreBreakdown,
    Tier,
};
pub use explain::explain;
pub use facts::{ClusterState, InstanceRecord, Liveness, PlacementFacts, TrustTier, WorkloadRef};
pub use request::{
    Affinity, DEV_FALLBACK_CLASS, GateInput, GateVerdict, PlacementRequest, ScoringWeights,
};
pub use spec::{
    Better, Execution, ExecutionVariant, LatencyClass, PerfTarget, PlacementPolicy, Preference,
    WorkloadRequirements, WorkloadSpec,
};

use constraints::{Eligible, check_node};

/// Place one workload. See the module docs for the phases.
///
/// Errors only for an invalid request, duplicate node ids, or a pin that is
/// unknown or ineligible. "No node fits" is not an error: it is a
/// [`Decision`] with no placement and per-node reasons.
pub fn place<F: PlacementFacts>(
    request: &PlacementRequest,
    facts: &[F],
    cluster: &ClusterState,
) -> Result<Decision, PlacementError> {
    request.validate()?;
    let mut seen = BTreeSet::new();
    if let Some(dup) = facts
        .iter()
        .map(|f| f.node_id())
        .find(|id| !seen.insert(*id))
    {
        return Err(PlacementError::DuplicateNode(dup.to_string()));
    }
    if let Some(pin) = &request.pin
        && !seen.contains(pin.as_str())
    {
        return Err(PlacementError::UnknownPin(pin.clone()));
    }
    let revoked_ref = request
        .spec
        .policy
        .revocable_refs
        .iter()
        .find(|r| cluster.revoked_refs.contains(*r))
        .map(String::as_str);

    // Phase A.
    let mut rejected: Vec<NodeReport> = Vec::new();
    let mut nodes: Vec<&F> = Vec::new();
    let mut survivors: Vec<Eligible> = Vec::new();
    for f in facts {
        match check_node(request, f, cluster, revoked_ref) {
            Ok(e) => {
                nodes.push(f);
                survivors.push(e);
            }
            Err(r) => rejected.push(report(f.node_id(), r)),
        }
    }

    // Phase B.
    let mut eligible: Vec<NodeReport> = Vec::new();
    for ((node, e), scored) in nodes
        .iter()
        .zip(&survivors)
        .zip(score::score_all(request, &nodes, &survivors))
    {
        match scored {
            Ok((score, flags)) => eligible.push(NodeReport {
                node_id: node.node_id().to_string(),
                rejections: vec![],
                variant: Some(e.variant.name.clone()),
                tier: Some(e.tier),
                score: Some(score),
                flags,
                assigned: assigned_ids(node.capabilities(), e),
            }),
            Err(r) => rejected.push(report(node.node_id(), vec![r])),
        }
    }
    // Strict tiers first, then score, then node id.
    eligible.sort_by(|a, b| {
        tier_rank(a)
            .cmp(&tier_rank(b))
            .then_with(|| total(b).total_cmp(&total(a)))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    rejected.sort_by(|a, b| a.node_id.cmp(&b.node_id));

    let chosen = select(request, &mut eligible, &rejected)?;
    let placement = chosen.map(|i| {
        let c = &eligible[i];
        let e = &survivors[nodes
            .iter()
            .position(|n| n.node_id() == c.node_id)
            .expect("eligible node was a survivor")];
        Placement {
            node_id: c.node_id.clone(),
            variant: e.variant.name.clone(),
            execution: e.variant.execution,
            tier: e.tier,
            accelerator: c
                .assigned
                .iter()
                .flatten()
                .find(|id| id.starts_with("accel."))
                .cloned(),
            score: c.score.clone().unwrap_or_default(),
            flags: c.flags.clone(),
        }
    });
    eligible.extend(rejected);
    Ok(Decision {
        kind: request.spec.kind.clone(),
        name: request.spec.name.clone(),
        allow_emulated: request.allow_emulated,
        pin: request.pin.clone(),
        placement,
        candidates: eligible,
    })
}

fn report(node_id: &str, rejections: Vec<Rejection>) -> NodeReport {
    NodeReport {
        node_id: node_id.to_string(),
        rejections,
        variant: None,
        tier: None,
        score: None,
        flags: vec![],
        assigned: vec![],
    }
}

fn tier_rank(r: &NodeReport) -> u8 {
    r.tier.map_or(u8::MAX, Tier::rank)
}

fn total(r: &NodeReport) -> f64 {
    r.score.as_ref().map_or(0.0, ScoreBreakdown::total)
}

fn assigned_ids(caps: &[super::Capability], e: &Eligible) -> Vec<Vec<String>> {
    e.assignment
        .iter()
        .map(|v| v.iter().map(|&i| caps[i].id.to_string()).collect())
        .collect()
}

/// Pick the winner's index in `eligible` (ranked), adding selection flags.
fn select(
    request: &PlacementRequest,
    eligible: &mut [NodeReport],
    rejected: &[NodeReport],
) -> Result<Option<usize>, PlacementError> {
    if let Some(pin) = &request.pin {
        if let Some(r) = rejected.iter().find(|r| &r.node_id == pin) {
            return Err(PlacementError::PinIneligible {
                node: pin.clone(),
                rejections: r.rejections.clone(),
            });
        }
        let i = eligible
            .iter()
            .position(|c| &c.node_id == pin)
            .expect("known pin is eligible or rejected");
        add_flag(&mut eligible[i], Flag::Pinned);
        return Ok(Some(i));
    }
    // Strict tiers: only the best tier present is a candidate pool
    // (`eligible` is ranked tier first).
    let best = eligible.first().map(tier_rank);
    let pool: Vec<usize> = (0..eligible.len())
        .filter(|&i| Some(tier_rank(&eligible[i])) == best)
        .collect();
    let base = pool.first().copied();
    let aff = &request.affinity;
    let preferred: Vec<usize> = pool
        .iter()
        .copied()
        .filter(|&i| aff.prefer.contains(&eligible[i].node_id))
        .collect();
    let not_avoided: Vec<usize> = pool
        .iter()
        .copied()
        .filter(|&i| !aff.avoid.contains(&eligible[i].node_id))
        .collect();
    let chosen = preferred
        .first()
        .or(not_avoided.first())
        .or(pool.first())
        .copied();
    if let Some(i) = chosen
        && Some(i) != base
    {
        add_flag(&mut eligible[i], Flag::Affinity);
    }
    Ok(chosen)
}

fn add_flag(r: &mut NodeReport, f: Flag) {
    if !r.flags.contains(&f) {
        r.flags.push(f);
        r.flags.sort_unstable();
    }
}
