//! The ADR-106 run gate in a `workload-host` (phase 3).
//!
//! A package is Cognitum-origin when its signed manifest says so
//! (`GrantOrigin::Cognitum`) or when any of its binaries is bytes the gate
//! knows as Cognitum bytes (listed by a held grant, or a revoked artifact
//! hash): a re-pack under another id does not escape the gate.
//!
//! For such a package the host asks the node's [`CognitumRunGate`]:
//!
//! - **at `place` / `load`**, for EVERY binary in the package (hashes computed
//!   from its bytes), so no binary the gate does not permit is ever staged.
//!   The arch that will run is the one the runtime's own admission picks
//!   (native: the host arch; container: its arch order), and the placement is
//!   refused when that is not the arch of the chosen variant;
//! - **at every `start`**, for the binary that runs, rehashed from the file
//!   it was staged to when the adapter runs it from disk (native), else from
//!   the loaded bytes (a container image is tagged by the binary's BLAKE3).
//!
//! In a Seed-bound mesh that needs a valid checkout grant and an operator
//! hash approval. A refusal is a governance refusal whose reason carries the
//! stable code (`licence run gate: [no_approval] ...`), chained as
//! `workload.refuse` and shown by `weaver workload place --explain`. A permit
//! is chained as `cog.run.permit` with the grant and approval ids. Other
//! signed packages never reach the gate.

use serde_json::json;

use super::cog_kind::arch_of;
use super::host_service::{WorkloadHostService, refuse, runtime_refusal};
use super::msg::{CtlRequest, Refusal, RefusalCode};
use crate::licence::{CognitumRunGate, RunRequest, RunVerdict, sha256_hex};
use crate::mesh_swarm_state::GrantOrigin;
use crate::workload_pkg::VerifiedPackage;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_runtime::{InstanceHandle, VerifiedWorkload, WorkloadHost, WorkloadSource};

/// Chain kind of a permitted Cognitum run.
pub const EVENT_KIND_RUN_PERMIT: &str = "cog.run.permit";

/// What a placed Cognitum cog is, for the check at each start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicensedRun {
    /// Cog id.
    pub cog_id: String,
    /// Version.
    pub version: String,
    /// The arch of the binary that runs (from the runtime's admission).
    pub arch: String,
}

fn hash(bytes: &[u8]) -> (String, String) {
    (sha256_hex(bytes), hex_encode(blake3::hash(bytes).as_bytes()))
}

/// (arch, sha256, blake3) of every binary, computed from its bytes.
fn binaries(w: &VerifiedWorkload) -> Vec<(String, String, String)> {
    let WorkloadSource::SignedPackage(p) = &w.source else { return Vec::new() };
    p.binaries
        .iter()
        .map(|(arch, b)| {
            let (s, b3) = hash(&b.bytes);
            (arch.clone(), s, b3)
        })
        .collect()
}

fn gate_refusal(code: &str, why: String) -> Refusal {
    refuse(RefusalCode::Governance, format!("licence run gate: [{code}] {why}"))
}

impl WorkloadHostService {
    /// Use `gate` for Cognitum-origin cogs (first call wins).
    pub fn set_licence_gate(&self, gate: std::sync::Arc<dyn CognitumRunGate>) -> bool {
        self.licence_gate.set(gate).is_ok()
    }

    /// Ask the gate for one binary; `record` chains a permit.
    fn licence_decide(
        &self,
        gate: &dyn CognitumRunGate,
        run: &LicensedRun,
        arch: &str,
        (sha256, blake3): (&str, &str),
        record: Option<(&str, &CtlRequest)>,
    ) -> Result<(), Refusal> {
        let r = RunRequest { cog_id: &run.cog_id, version: &run.version, sha256, blake3 };
        match gate.check(&r) {
            Ok(RunVerdict::NotSeedBound) => Ok(()),
            Ok(RunVerdict::Permit(p)) => {
                if let Some((phase, req)) = record {
                    self.record(
                        EVENT_KIND_RUN_PERMIT,
                        json!({
                            "node": self.node_id(), "phase": phase, "requester": req.requester,
                            "decision_id": req.decision_id, "cog_id": run.cog_id, "version": run.version,
                            "arch": arch, "sha256": sha256, "blake3": p.blake3,
                            "grant_id": p.grant_id, "approval_id": p.approval_id,
                        }),
                    );
                }
                Ok(())
            }
            Err(e) => Err(gate_refusal(
                e.code(),
                format!(
                    "{e} ({} {} {arch}, sha256 {}); remedy: {}",
                    run.cog_id,
                    run.version,
                    &sha256[..16],
                    e.remedy(&run.cog_id, &run.version)
                ),
            )),
        }
    }

    /// The gate at `place` / `load`. `Ok(None)`: not a Cognitum-origin cog
    /// (or no gate on this node), the gate does not apply.
    pub(super) async fn licence_check_place(
        &self,
        verified: &VerifiedPackage,
        w: &VerifiedWorkload,
        variant: &str,
        host: &WorkloadHost,
        req: &CtlRequest,
    ) -> Result<Option<LicensedRun>, Refusal> {
        let Some(gate) = self.licence_gate.get() else { return Ok(None) };
        let bins = binaries(w);
        let (cog_id, version) = match crate::mesh_artifact_pkg::grant_origin(verified) {
            GrantOrigin::Cognitum { cog_id, version } => (cog_id, version),
            _ if bins.iter().any(|(_, s, b)| gate.claims(s, b)) => (w.id.clone(), w.version.clone()),
            _ => return Ok(None),
        };
        // The arch the runtime will actually run, not the one the controller assumed.
        let adm = host.runtime().admit(w).await.map_err(|e| runtime_refusal(&e))?;
        if adm.arch != arch_of(variant) {
            return Err(gate_refusal(
                "arch_mismatch",
                format!("this node's runtime would run the {} binary, the placement chose {variant}", adm.arch),
            ));
        }
        let run = LicensedRun { cog_id, version, arch: adm.arch };
        // Every binary in the package, the one that runs last (it is chained).
        let (runs, others): (Vec<_>, Vec<_>) = bins.iter().partition(|(a, _, _)| *a == run.arch);
        for (arch, s, b) in others {
            self.licence_decide(gate.as_ref(), &run, arch, (s, b), None)?;
        }
        let (_, s, b) = runs
            .first()
            .ok_or_else(|| gate_refusal("no_binary", format!("the package has no {} binary", run.arch)))?;
        self.licence_decide(gate.as_ref(), &run, &run.arch, (s, b), Some(("place", req)))?;
        Ok(Some(run))
    }

    /// The gate again at `start` (a lapse refuses a restart; a running
    /// instance is left alone). The binary is rehashed from the staged file
    /// when there is one.
    pub(super) async fn licence_check_start(
        &self,
        run: &LicensedRun,
        host: &WorkloadHost,
        h: &InstanceHandle,
        req: &CtlRequest,
    ) -> Result<(), Refusal> {
        let Some(gate) = self.licence_gate.get() else { return Ok(()) };
        let hashes = match host.runtime().staged_payload(h).await {
            Some(path) => {
                let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
                    .await
                    .map_err(|e| refuse(RefusalCode::Runtime, e.to_string()))?
                    .map_err(|e| gate_refusal("staged_unreadable", format!("the staged binary cannot be read: {e}")))?;
                hash(&bytes)
            }
            None => {
                let w = host
                    .workload_for(h)
                    .await
                    .ok_or_else(|| refuse(RefusalCode::UnknownInstance, "instance has no loaded workload"))?;
                let (_, s, b) = binaries(&w)
                    .into_iter()
                    .find(|(a, _, _)| *a == run.arch)
                    .ok_or_else(|| gate_refusal("no_binary", format!("the package has no {} binary", run.arch)))?;
                (s, b)
            }
        };
        self.licence_decide(gate.as_ref(), run, &run.arch, (&hashes.0, &hashes.1), Some(("start", req)))
    }
}
