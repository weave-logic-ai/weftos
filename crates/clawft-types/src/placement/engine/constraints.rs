//! Phase A: hard constraints (ADR-099 section 3). A node failing any is
//! filtered, and **every** failing constraint is recorded, not only the
//! first, so a pin error and the explain output name them all.
//!
//! Requirement matching is wave 1's [`match_all`]; nothing here re-implements
//! it. Nothing here branches on the workload kind.

use super::super::assign::{Assignment, match_all};
use super::super::memory::{MemoryDemand, MemoryLedger, MemoryPool, MemoryShortfall};
use super::super::requirement::{MatchFailure, Requirement};
use super::decision::{Constraint, Rejection, Tier};
use super::facts::{ClusterState, Liveness, PlacementFacts};
use super::request::{GateVerdict, PlacementRequest};
use super::spec::{Execution, ExecutionVariant};

/// A node that passed phase A, with the route chosen for it.
#[derive(Debug, Clone)]
pub(super) struct Eligible {
    /// Chosen variant.
    pub variant: ExecutionVariant,
    /// Tier it earns on this node.
    pub tier: Tier,
    /// Capability indices per requirement (common, then variant).
    pub assignment: Assignment,
}

fn rej(constraint: Constraint, detail: impl Into<String>) -> Rejection {
    Rejection {
        constraint,
        detail: detail.into(),
    }
}

/// Describe a match failure for humans.
pub(super) fn describe_failure(req: &Requirement, f: &MatchFailure) -> String {
    let sel = req.selector.as_str();
    match f {
        MatchFailure::NoSuchId { .. } => format!("{sel}: not advertised"),
        MatchFailure::ProvenanceTooLow { best, need } => {
            format!("{sel}: provenance {best:?} below {need:?}").to_lowercase()
        }
        MatchFailure::PredicateFailed { attr, op } => {
            format!(
                "{sel}: predicate `{attr} {}` failed",
                format!("{op:?}").to_lowercase()
            )
        }
        MatchFailure::Unavailable { state } => {
            format!(
                "{sel}: unavailable ({})",
                format!("{state:?}").to_lowercase()
            )
        }
        MatchFailure::InsufficientCount { have, need } => {
            format!("{sel}: {have} usable, need {need}")
        }
        MatchFailure::Contended { need } => {
            format!("{sel}: contended (need {need} exclusive)")
        }
    }
}

fn failure_constraint(f: &MatchFailure) -> Constraint {
    match f {
        MatchFailure::ProvenanceTooLow { .. } => Constraint::Provenance,
        _ => Constraint::Requirements,
    }
}

fn raise_provenance(reqs: &[Requirement], req: &PlacementRequest) -> Vec<Requirement> {
    let floor = req.spec.policy.min_provenance;
    reqs.iter()
        .cloned()
        .map(|mut r| {
            if let Some(f) = floor
                && r.min_provenance.is_none_or(|p| p < f)
            {
                r.min_provenance = Some(f);
            }
            r
        })
        .collect()
}

fn match_rejections(
    prefix: &str,
    reqs: &[Requirement],
    fails: &[(usize, MatchFailure)],
) -> Vec<Rejection> {
    fails
        .iter()
        .map(|(i, f)| {
            rej(
                failure_constraint(f),
                format!("{prefix}{}", describe_failure(&reqs[*i], f)),
            )
        })
        .collect()
}

/// Choose a route: the first native variant that matches, else the first
/// emulated one if the operator allowed emulation.
fn choose_route<F: PlacementFacts>(
    req: &PlacementRequest,
    node: &F,
) -> Result<Eligible, Vec<Rejection>> {
    let caps = node.capabilities();
    let common = raise_provenance(&req.spec.requirements.common, req);
    if let Err(fails) = match_all(&common, caps) {
        return Err(match_rejections("", &common, &fails));
    }
    let variants = req.spec.effective_variants();
    let implicit = req.spec.requirements.variants.is_empty();
    let mut matched: Vec<(ExecutionVariant, Vec<Requirement>, Assignment)> = Vec::new();
    let mut failures = Vec::new();
    for v in variants {
        let mut all = common.clone();
        all.extend(raise_provenance(&v.requirements, req));
        match match_all(&all, caps) {
            Ok(a) => matched.push((v, all, a)),
            Err(fails) => {
                let prefix = if implicit {
                    String::new()
                } else {
                    format!("variant {}: ", v.name)
                };
                failures.extend(match_rejections(&prefix, &all, &fails));
            }
        }
    }
    let pick = matched
        .iter()
        .position(|(v, ..)| v.execution == Execution::Native)
        .or_else(|| {
            matched
                .iter()
                .position(|(v, ..)| v.execution == Execution::Emulated && req.allow_emulated)
        });
    match pick {
        Some(i) => {
            let (variant, _, assignment) = matched.swap_remove(i);
            let tier = match variant.execution {
                Execution::Emulated => Tier::Emulated,
                Execution::Native
                    if caps
                        .iter()
                        .any(|c| c.id.as_str() == req.weights.dev_fallback_class) =>
                {
                    Tier::DevFallback
                }
                Execution::Native => Tier::Native,
            };
            Ok(Eligible {
                variant,
                tier,
                assignment,
            })
        }
        None if !matched.is_empty() => {
            let names: Vec<&str> = matched.iter().map(|(v, ..)| v.name.as_str()).collect();
            Err(vec![rej(
                Constraint::EmulationNotAllowed,
                format!(
                    "only emulated route(s) fit ({}); emulation needs explicit allow_emulated",
                    names.join(", ")
                ),
            )])
        }
        None => Err(failures),
    }
}

fn describe_shortfall(s: &MemoryShortfall) -> String {
    let pool = match s.pool {
        MemoryPool::Unified => "unified",
        MemoryPool::System => "system",
        MemoryPool::Vram => "vram",
    };
    format!("{pool} pool: need {} bytes, free {} bytes", s.need, s.free)
}

fn check_memory<F: PlacementFacts>(
    req: &PlacementRequest,
    node: &F,
    cluster: &ClusterState,
) -> Option<Rejection> {
    let demand = req.spec.requirements.memory;
    if demand == MemoryDemand::default() {
        return None;
    }
    let total = cluster
        .instances_on(node.node_id())
        .fold(demand, |acc, i| MemoryDemand {
            host_bytes: acc.host_bytes.saturating_add(i.pending.host_bytes),
            accel_bytes: acc.accel_bytes.saturating_add(i.pending.accel_bytes),
        });
    MemoryLedger::from_capabilities(node.capabilities())
        .check(total)
        .err()
        .map(|s| rej(Constraint::Memory, describe_shortfall(&s)))
}

fn check_co_residency<F: PlacementFacts>(
    req: &PlacementRequest,
    node: &F,
    cluster: &ClusterState,
) -> Vec<Rejection> {
    let policy = &req.spec.policy;
    let mut out = Vec::new();
    for inst in cluster.instances_on(node.node_id()) {
        if let Some(l) = inst.labels.iter().find(|l| policy.excludes.contains(l)) {
            out.push(rej(
                Constraint::CoResidency,
                format!("this workload excludes `{l}`, which runs here"),
            ));
        }
        if let Some(l) = inst.excludes.iter().find(|l| policy.labels.contains(l)) {
            out.push(rej(
                Constraint::CoResidency,
                format!("an instance here excludes `{l}`"),
            ));
        }
    }
    out
}

/// Every phase A constraint for one node. `revoked_ref` is the first of the
/// workload's revoked references, computed once by the caller.
pub(super) fn check_node<F: PlacementFacts>(
    req: &PlacementRequest,
    node: &F,
    cluster: &ClusterState,
    revoked_ref: Option<&str>,
) -> Result<Eligible, Vec<Rejection>> {
    let id = node.node_id();
    let mut out = Vec::new();
    if cluster.revoked_nodes.contains(id) {
        out.push(rej(Constraint::NodeRevoked, "node is revoked"));
    }
    if node.liveness() != Liveness::Alive {
        out.push(rej(
            Constraint::Liveness,
            format!("node is {}", node.liveness().as_str()),
        ));
    }
    if let Some(exp) = node.facts_expire_at_ms()
        && exp <= cluster.now_ms
    {
        out.push(rej(
            Constraint::FactsExpired,
            format!("facts expired at {exp} ms, now {} ms", cluster.now_ms),
        ));
    }
    let min = req.spec.policy.min_trust;
    if node.trust_tier() < min {
        out.push(rej(
            Constraint::Trust,
            format!(
                "node tier {} below required {}",
                node.trust_tier().as_str(),
                min.as_str()
            ),
        ));
    }
    if let Some(r) = revoked_ref {
        out.push(rej(
            Constraint::WorkloadRevoked,
            format!("`{r}` is revoked"),
        ));
    }
    out.extend(check_co_residency(req, node, cluster));
    out.extend(check_memory(req, node, cluster));
    let route = choose_route(req, node);
    if let Err(r) = &route {
        out.extend(r.iter().cloned());
    }
    if let GateVerdict::Deny { reason } = req.gate.verdict_for(id) {
        out.push(rej(
            Constraint::Gate,
            format!("workload.place denied: {reason}"),
        ));
    }
    match route {
        Ok(e) if out.is_empty() => Ok(e),
        _ => Err(out),
    }
}
