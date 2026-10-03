//! The ADR-106 run gate in a `workload-host` (phase 3).
//!
//! Before a Cognitum-origin cog (`GrantOrigin::Cognitum`, from the signed
//! manifest) is installed, and again before every start, the host asks the
//! node's [`CognitumRunGate`] with the sha256 and BLAKE3 computed from the
//! bytes of the binary that will run. In a Seed-bound mesh that needs a valid
//! checkout grant and an operator hash approval; a refusal is a governance
//! refusal whose reason carries the stable code (`licence run gate:
//! [no_approval] ...`), chained as `workload.refuse` and shown by `weaver
//! workload place --explain`. A permit is chained as `cog.run.permit` with
//! the grant and approval ids. Signed WeftOS and private packages (`OptIn`,
//! `NotFlagged`) never reach the gate: their package trust path is unchanged.

use serde_json::json;

use super::host_service::{WorkloadHostService, refuse};
use super::msg::{CtlRequest, Refusal, RefusalCode};
use crate::licence::{CognitumRunGate, RunRequest, RunVerdict, sha256_hex};
use crate::mesh_swarm_state::GrantOrigin;
use crate::workload_pkg::codec::hex_encode;
use crate::workload_pkg::VerifiedPackage;
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
    /// The arch of the binary that runs.
    pub arch: String,
}

/// The binary arch of a variant (`aarch64-native` is `aarch64`).
fn arch_of(variant: &str) -> &str {
    variant.split_once('-').map_or(variant, |(a, _)| a)
}

/// sha256 and BLAKE3 of the `arch` binary, computed from its bytes.
fn hashes(w: &VerifiedWorkload, arch: &str) -> Option<(String, String)> {
    let WorkloadSource::SignedPackage(p) = &w.source else { return None };
    let b = p.binaries.get(arch)?;
    Some((sha256_hex(&b.bytes), hex_encode(blake3::hash(&b.bytes).as_bytes())))
}

impl WorkloadHostService {
    /// Use `gate` for Cognitum-origin cogs (first call wins).
    pub fn set_licence_gate(&self, gate: std::sync::Arc<dyn CognitumRunGate>) -> bool {
        self.licence_gate.set(gate).is_ok()
    }

    fn licence_decide(&self, run: &LicensedRun, w: &VerifiedWorkload, phase: &str, req: &CtlRequest) -> Result<(), Refusal> {
        let Some(gate) = self.licence_gate.get() else { return Ok(()) };
        let (sha256, blake3) = hashes(w, &run.arch).ok_or_else(|| {
            refuse(RefusalCode::Governance, format!("licence run gate: [no_binary] the package has no {} binary", run.arch))
        })?;
        let r = RunRequest { cog_id: &run.cog_id, version: &run.version, sha256: &sha256, blake3: &blake3 };
        match gate.check(&r) {
            Ok(RunVerdict::NotSeedBound) => Ok(()),
            Ok(RunVerdict::Permit(p)) => {
                self.record(
                    EVENT_KIND_RUN_PERMIT,
                    json!({
                        "node": self.node_id(), "phase": phase, "requester": req.requester,
                        "decision_id": req.decision_id, "cog_id": run.cog_id, "version": run.version,
                        "arch": run.arch, "sha256": sha256, "blake3": p.blake3,
                        "grant_id": p.grant_id, "approval_id": p.approval_id,
                    }),
                );
                Ok(())
            }
            Err(e) => Err(refuse(
                RefusalCode::Governance,
                format!(
                    "licence run gate: [{}] {} ({} {} {}, sha256 {}); remedy: {}",
                    e.code(),
                    e,
                    run.cog_id,
                    run.version,
                    run.arch,
                    &sha256[..16],
                    e.remedy(&run.cog_id, &run.version)
                ),
            )),
        }
    }

    /// The gate at `place` / `load`. `Ok(None)`: not a Cognitum-origin cog,
    /// the gate does not apply.
    pub(super) fn licence_check_place(
        &self,
        verified: &VerifiedPackage,
        w: &VerifiedWorkload,
        variant: &str,
        req: &CtlRequest,
    ) -> Result<Option<LicensedRun>, Refusal> {
        let GrantOrigin::Cognitum { cog_id, version } = crate::mesh_artifact_pkg::grant_origin(verified) else {
            return Ok(None);
        };
        let run = LicensedRun { cog_id, version, arch: arch_of(variant).to_owned() };
        self.licence_decide(&run, w, "place", req)?;
        Ok(Some(run))
    }

    /// The gate again at `start` (a lapse refuses a restart; a running
    /// instance is left alone).
    pub(super) async fn licence_check_start(
        &self,
        run: &LicensedRun,
        host: &WorkloadHost,
        h: &InstanceHandle,
        req: &CtlRequest,
    ) -> Result<(), Refusal> {
        if self.licence_gate.get().is_none() {
            return Ok(());
        }
        let w = host
            .workload_for(h)
            .await
            .ok_or_else(|| refuse(RefusalCode::UnknownInstance, "instance has no loaded workload"))?;
        self.licence_decide(run, &w, "start", req)
    }
}

#[cfg(test)]
mod tests {
    use super::arch_of;

    #[test]
    fn the_variant_names_the_binary_arch() {
        assert_eq!(arch_of("aarch64-native"), "aarch64");
        assert_eq!(arch_of("x86_64-container"), "x86_64");
        assert_eq!(arch_of("armv7-emulated"), "armv7");
        assert_eq!(arch_of("aarch64"), "aarch64");
    }
}
