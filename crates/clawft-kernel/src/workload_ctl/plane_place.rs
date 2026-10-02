//! Placement and instance control on the controller (ADR-099 sections 3,
//! 4 and 7).
//!
//! `place`:
//!
//! 1. verify and seed the signed package (this node serves it to the
//!    target during the call), derive the kind's spec (ADR-100 s2);
//! 2. ask the gate `workload.place` for every cached node (each check is
//!    chained by the gate) and run the pure engine on the verified facts;
//! 3. chain the decision (candidates, scores, failed constraints) as
//!    `workload.place`; its chain hash is the decision id every request
//!    carries;
//! 4. send the signed `place` to the winner, then to the next eligible
//!    candidates in rank order while targets refuse (the target's
//!    admission self-check disagreeing with its advertised facts, its own
//!    governance, a fetch or verify failure); each refusal is chained as
//!    `workload.refuse`; the final placement is chained as `workload.place`.

use std::path::PathBuf;

use clawft_types::placement::engine::{
    Affinity, ClusterState, Decision, Execution, GateInput, GateVerdict, InstanceRecord,
    PlacementFacts, PlacementRequest, Tier, WorkloadRef, WorkloadSpec, explain, place,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::chain;
use crate::gate::GateDecision;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_runtime::VerifiedWorkload;

use super::facts::governance_tier;
use super::host_service::{CtlConfig, InstanceBody, PlaceBody};
use super::msg::method;
use super::plane::{
    Call, CallFailure, PLANE_CHAIN_SOURCE, PlacementControlPlane, PlacementRecord, PlaneError,
    now_ms,
};

/// What the operator asked for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceOrder {
    /// Unpacked signed package directory.
    pub package_dir: PathBuf,
    /// Instance configuration.
    pub config: CtlConfig,
    /// Place on this node or fail naming the constraint.
    #[serde(default)]
    pub pin: Option<String>,
    /// Affinity.
    #[serde(default)]
    pub prefer: Vec<String>,
    /// Anti-affinity.
    #[serde(default)]
    pub avoid: Vec<String>,
    /// Operator opt-in to emulation (never automatic).
    #[serde(default)]
    pub allow_emulated: bool,
    /// Start after loading.
    #[serde(default = "yes")]
    pub start: bool,
    /// Decide and explain only; dispatch nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Project placing the workload (the caller's verified project). The
    /// target's ingest bridge delivers a cog's vectors to this project's
    /// store; `None` delivers them to this controller's store.
    #[serde(default)]
    pub project_id: Option<String>,
}

fn yes() -> bool {
    true
}

/// One dispatch attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    /// Target.
    pub node_id: String,
    /// Variant sent.
    pub variant: String,
    /// `placed`, `refused`, `unreachable`, `indeterminate` or `gate_denied`.
    pub outcome: String,
    /// Refusal code, if refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Why, if not placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Result of `place` (and of a dry run).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceReport {
    /// The engine's decision.
    pub decision: Decision,
    /// Chain hash of the decision event.
    pub decision_id: String,
    /// Human-readable explanation (engine explain plus attempts).
    pub explain: String,
    /// Dispatch attempts, in order.
    pub attempts: Vec<Attempt>,
    /// The placement, if any target accepted.
    pub placed: Option<PlacementRecord>,
}

impl PlacementControlPlane {
    fn gate_ctx(&self, w: &VerifiedWorkload, node_id: &str, emulated: bool) -> Value {
        let tier = self
            .facts
            .get(node_id, now_ms() / 1000)
            .map(|c| governance_tier(c.trust_tier))
            .unwrap_or(crate::workload_governance::NodeTrustTier::Discovered);
        let (pid, keys, hashes, cost) = match w.signed("placement") {
            Ok(p) => (
                Some(p.package_id.clone()),
                p.signer_keys.clone(),
                p.artifact_hashes.clone(),
                (f64::from(p.spec.resources.cpu_pct) / 400.0).clamp(0.0, 1.0),
            ),
            Err(_) => (None, vec![], vec![], 0.25),
        };
        json!({ "node_id": node_id, "workload": {
            "kind": w.kind, "package_trust": "pinned_signer", "node_tier": tier,
            // A native cog has the host's network (ADR-100 s4): say so.
            "network": "egress", "secrets": false, "emulated": emulated,
            "resource_cost": cost, "package_id": pid, "signer_keys": keys,
            "artifact_hashes": hashes,
        }})
    }

    fn gate_verdict(&self, w: &VerifiedWorkload, node_id: &str, emulated: bool) -> GateVerdict {
        let ctx = self.gate_ctx(w, node_id, emulated);
        match self.gate.check(&self.node_id, "workload.place", &ctx) {
            GateDecision::Permit { .. } => GateVerdict::Permit,
            GateDecision::Deny { reason, .. } => GateVerdict::Deny { reason },
            GateDecision::Defer { reason } => GateVerdict::Deny {
                reason: format!("deferred to a human (refused on this path): {reason}"),
            },
        }
    }

    fn cluster_state(&self) -> ClusterState {
        let recs = self.placements();
        ClusterState {
            now_ms: now_ms(),
            instances: recs
                .iter()
                .map(|r| InstanceRecord {
                    node_id: r.node_id.clone(),
                    workload: Some(WorkloadRef {
                        kind: r.kind.clone(),
                        name: r.workload.clone(),
                    }),
                    ..InstanceRecord::default()
                })
                .collect(),
            ..ClusterState::default()
        }
    }

    pub(super) fn chain_event(&self, kind: &str, payload: Value) -> String {
        hex_encode(
            &self
                .chain
                .append(PLANE_CHAIN_SOURCE, kind, Some(payload))
                .hash,
        )
    }

    /// Decide (and unless `dry_run`, dispatch) one placement.
    pub async fn place(&self, order: &PlaceOrder) -> Result<PlaceReport, PlaneError> {
        order.check()?;
        let (w, spec, manifest_hash) = match self.prepare(order) {
            Ok(p) => p,
            Err(e @ PlaneError::UnknownKind(_)) => {
                self.chain_event(
                    chain::EVENT_KIND_WORKLOAD_REFUSE,
                    json!({ "phase": "kind", "error": e.to_string() }),
                );
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        let view = self.view();
        let verdicts = view
            .iter()
            .map(|n| {
                (
                    n.node_id().to_string(),
                    self.gate_verdict(&w, n.node_id(), false),
                )
            })
            .collect();
        let mut req = PlacementRequest::new(spec);
        req.pin = order.pin.clone();
        req.affinity = Affinity {
            prefer: order.prefer.clone(),
            avoid: order.avoid.clone(),
        };
        req.allow_emulated = order.allow_emulated;
        req.gate = GateInput::PerNode { verdicts };
        req.weights = self.cfg.weights.clone();
        let decided = place(&req, &view, &self.cluster_state());
        let decision = match decided {
            Ok(d) => d,
            Err(e) => {
                self.chain_event(
                    chain::EVENT_KIND_WORKLOAD_REFUSE,
                    json!({ "phase": "decision", "workload": w.id, "error": e.to_string(),
                            "constraints": e.constraints().iter().map(|c| c.as_str()).collect::<Vec<_>>() }),
                );
                return Err(PlaneError::Placement(e.to_string()));
            }
        };
        let decision_id = self.chain_event(
            chain::EVENT_KIND_WORKLOAD_PLACE,
            json!({ "phase": "decision", "dry_run": order.dry_run, "controller": self.node_id,
                    "manifest_hash": manifest_hash, "decision": decision }),
        );
        let mut report = PlaceReport {
            explain: String::new(),
            decision,
            decision_id,
            attempts: Vec::new(),
            placed: None,
        };
        if !order.dry_run && report.decision.placement.is_some() {
            self.dispatch(order, &w, &manifest_hash, &mut report).await;
        }
        report.explain = render(&report);
        Ok(report)
    }

    /// Winner first, then the other eligible candidates in rank order (a
    /// pin allows only the pinned node).
    fn dispatch_order(d: &Decision) -> Vec<(String, String, Execution)> {
        let Some(p) = &d.placement else { return vec![] };
        let mut out = vec![(p.node_id.clone(), p.variant.clone(), p.execution)];
        if d.pin.is_none() {
            for c in d
                .candidates
                .iter()
                .filter(|c| c.eligible() && c.node_id != p.node_id)
            {
                if let Some(v) = &c.variant {
                    let ex = if c.tier == Some(Tier::Emulated) {
                        Execution::Emulated
                    } else {
                        Execution::Native
                    };
                    out.push((c.node_id.clone(), v.clone(), ex));
                }
            }
        }
        out
    }

    async fn dispatch(
        &self,
        order: &PlaceOrder,
        w: &VerifiedWorkload,
        manifest: &str,
        report: &mut PlaceReport,
    ) {
        for (node, variant, ex) in Self::dispatch_order(&report.decision) {
            let attempt = |outcome: &str, code: Option<String>, reason: Option<String>| Attempt {
                node_id: node.clone(),
                variant: variant.clone(),
                outcome: outcome.to_string(),
                code,
                reason,
            };
            if ex == Execution::Emulated
                && let GateVerdict::Deny { reason } = self.gate_verdict(w, &node, true)
            {
                report
                    .attempts
                    .push(attempt("gate_denied", None, Some(reason)));
                continue;
            }
            let body = serde_json::to_value(PlaceBody {
                name: w.id.clone(),
                manifest_hash: manifest.to_string(),
                variant: variant.clone(),
                config: order.config.clone(),
                start: order.start,
                project_id: order.project_id.clone(),
            })
            .unwrap_or_default();
            let target = match self.target(&node) {
                Ok(t) => t,
                Err(e) => {
                    report
                        .attempts
                        .push(attempt("unreachable", None, Some(e.to_string())));
                    continue;
                }
            };
            let r = self
                .round_trip(Call {
                    addr: &target.0.addr,
                    target: &node,
                    expect: Some(target.1),
                    method: method::PLACE,
                    decision_id: Some(report.decision_id.clone()),
                    body,
                    serve: true,
                })
                .await;
            let template = PlacementRecord {
                instance_id: String::new(),
                node_id: node.clone(),
                kind: w.kind.clone(),
                workload: w.id.clone(),
                variant: variant.clone(),
                decision_id: report.decision_id.clone(),
                manifest_hash: manifest.to_string(),
                project_id: order.project_id.clone(),
            };
            match r {
                Ok((result, _)) => {
                    let iid = result["instance_id"].as_str().unwrap_or_default();
                    let rec = PlacementRecord {
                        instance_id: iid.to_string(),
                        ..template
                    };
                    self.chain_event(
                        chain::EVENT_KIND_WORKLOAD_PLACE,
                        json!({ "phase": "placed", "decision_id": report.decision_id, "node": node,
                                "variant": variant, "emulated": ex == Execution::Emulated,
                                "instance_id": rec.instance_id, "attempts": report.attempts.len() + 1,
                                "target_result": result }),
                    );
                    if let Ok(mut p) = self.placements.lock() {
                        p.insert(rec.instance_id.clone(), rec.clone());
                    }
                    self.persist();
                    report.attempts.push(attempt("placed", None, None));
                    report.placed = Some(rec);
                    return;
                }
                Err(CallFailure::Indeterminate(why)) => {
                    // The target may have placed it: reconcile before any
                    // other candidate, or stop (never two instances).
                    let (settled, note) = self.reconcile(&node, &report.decision_id).await;
                    self.chain_event(
                        chain::EVENT_KIND_WORKLOAD_REFUSE,
                        json!({ "phase": "dispatch", "decision_id": report.decision_id, "node": node,
                                "variant": variant, "outcome": "indeterminate", "reason": why,
                                "reconcile": note, "next": if settled { "trying the next candidate" }
                                else { "stopped: the target may hold the instance" } }),
                    );
                    report.attempts.push(attempt(
                        "indeterminate",
                        None,
                        Some(format!("{why}; {note}")),
                    ));
                    if !settled {
                        // Adopted later if the target turns out to hold it.
                        self.remember_unsettled(template);
                        return;
                    }
                }
                Err(f) => {
                    let (outcome, code, reason) = match &f {
                        CallFailure::Refused(r) => (
                            "refused",
                            Some(r.code.as_str().to_string()),
                            r.reason.clone(),
                        ),
                        CallFailure::Unreachable(e) | CallFailure::Indeterminate(e) => {
                            ("unreachable", None, e.clone())
                        }
                    };
                    self.chain_event(
                        chain::EVENT_KIND_WORKLOAD_REFUSE,
                        json!({ "phase": "dispatch", "decision_id": report.decision_id, "node": node,
                                "variant": variant, "outcome": outcome, "code": code, "reason": reason,
                                "next": "trying the next candidate" }),
                    );
                    report.attempts.push(attempt(outcome, code, Some(reason)));
                }
            }
        }
    }

    /// Gate an instance transition on the controller (chained) and send it.
    pub async fn instance(&self, m: &str, instance_id: &str) -> Result<Value, PlaneError> {
        let find = || {
            self.placements()
                .into_iter()
                .find(|r| r.instance_id == instance_id)
        };
        let rec = match find() {
            Some(r) => r,
            None => {
                // It may come from a lost `place` answer: adopt, then retry.
                self.settle_unsettled().await;
                find().ok_or_else(|| {
                    PlaneError::Unknown(format!("no placed instance {instance_id}"))
                })?
            }
        };
        if let Some(r) = self.seed_instance(&rec, m).await {
            if m == method::UNLOAD
                && r.is_ok()
                && let Ok(mut p) = self.placements.lock()
            {
                p.remove(instance_id);
            }
            return r;
        }
        self.ensure_described(&rec.node_id).await;
        let decision_id = if method::mutates(m) {
            let ctx = json!({ "workload": {
                "kind": rec.kind, "package_trust": "pinned_signer",
                "node_tier": self.facts.get(&rec.node_id, now_ms() / 1000)
                    .map(|c| governance_tier(c.trust_tier))
                    .unwrap_or(crate::workload_governance::NodeTrustTier::Discovered),
                "network": "egress", "resource_cost": 0.0,
            }});
            if let GateDecision::Deny { reason, .. } = self.gate.check(&self.node_id, m, &ctx) {
                return Err(PlaneError::Governance(reason));
            }
            Some(
                self.chain_event(
                    crate::workload_governance::event_kind_for(m)
                        .unwrap_or(chain::EVENT_KIND_WORKLOAD_REFUSE),
                    json!({ "phase": "request", "instance_id": instance_id, "node": rec.node_id }),
                ),
            )
        } else {
            None
        };
        let body = serde_json::to_value(InstanceBody {
            instance_id: Some(instance_id.to_string()),
            grace_ms: None,
        })
        .unwrap_or_default();
        let out = self.call(&rec.node_id, m, decision_id, body).await?;
        if m == method::UNLOAD
            && let Ok(mut p) = self.placements.lock()
        {
            p.remove(instance_id);
        }
        if m == method::UNLOAD {
            self.persist();
        }
        Ok(out)
    }
}

/// Engine explanation plus the dispatch attempts.
pub fn render(r: &PlaceReport) -> String {
    let mut s = explain(&r.decision);
    s.push_str(&format!("decision_id: {}\n", r.decision_id));
    if !r.attempts.is_empty() {
        s.push_str("dispatch:\n");
        for (i, a) in r.attempts.iter().enumerate() {
            s.push_str(&format!(
                "  {}. {} via {}: {}{}\n",
                i + 1,
                a.node_id,
                a.variant,
                a.outcome,
                a.reason
                    .as_deref()
                    .map(|x| format!(
                        " ({}{x})",
                        a.code
                            .as_deref()
                            .map(|c| format!("{c}: "))
                            .unwrap_or_default()
                    ))
                    .unwrap_or_default()
            ));
        }
    }
    s
}

impl PlaceOrder {
    /// Boundary validation.
    pub fn check(&self) -> Result<(), PlaneError> {
        self.config
            .mode
            .validate()
            .map_err(|e| PlaneError::Invalid(e.to_string()))?;
        if self.config.csi_port == 0 {
            return Err(PlaneError::Invalid("csi_port must be non-zero".into()));
        }
        Ok(())
    }
}
