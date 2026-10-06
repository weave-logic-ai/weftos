//! The `project` kind (ADR-103 A6, Phase 2 package G): a per-project child
//! kernel run by the user-daemon supervisor through the `logical` adapter.
//!
//! Authority comes from a project certificate chained to the user key, not
//! from a signed package, so [`WorkloadKind::load`] always refuses: there is
//! no package directory to verify. The supervisor builds the workload with
//! [`prepare_project`] instead. Governance: the kind has no permit in any
//! operator rule set; the only permit is [`project_supervisor_permit`],
//! limited to the supervisor principal, so `workload.place` of a `project`
//! stays denied for everyone else.
//!
//! [`project_supervisor_permit`]: crate::workload_governance::project_supervisor_permit

#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use std::path::Path;

#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use clawft_types::placement::engine::{PlacementPolicy, WorkloadRequirements, WorkloadSpec};
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use clawft_types::placement::{CapabilityId, MemoryDemand, Requirement};

#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_ctl::PlaneError;
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::{ManifestEnvelope, ManifestError};
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_runtime::{CAP_PROJECT_LOGICAL, ProjectPayload, VerifiedWorkload, WorkloadSource};

use super::WorkloadKind;
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use super::KindRegistry;

/// Manifest kind string.
pub const KIND_PROJECT: &str = "project";

/// Per-project kernels.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectKind;

impl WorkloadKind for ProjectKind {
    fn id(&self) -> &'static str {
        KIND_PROJECT
    }

    fn health(&self) -> super::HealthSpec {
        // A child kernel answers its handshake quickly; two misses are enough.
        super::HealthSpec {
            interval_ms: 5_000,
            miss_limit: 2,
        }
    }

    /// A project kernel is bound to its user daemon's host: never moved.
    fn migratable(&self) -> bool {
        false
    }

    fn validate(&self, _envelope: &ManifestEnvelope) -> Result<(), ManifestError> {
        Err(ManifestError::Invalid(
            "project workloads have no signed package; the user daemon supervisor starts them".into(),
        ))
    }

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn load(
        &self,
        _package_dir: &Path,
        _anchors: &TrustAnchors,
        _kinds: &KindRegistry,
    ) -> Result<VerifiedWorkload, PlaneError> {
        Err(PlaneError::Package(
            "project workloads are authorised by a project certificate, not a package; \
             use `project.start` on the user daemon"
                .into(),
        ))
    }

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn spec(&self, workload: &VerifiedWorkload) -> Result<WorkloadSpec, String> {
        let capability = match &workload.source {
            WorkloadSource::Project(p) if p.adapter == "wasmtime-project-v1" => "runtime.project.wasmtime",
            WorkloadSource::Project(p) if p.adapter == "logical" => CAP_PROJECT_LOGICAL,
            _ => return Err("unsupported project adapter".into()),
        };
        let id = CapabilityId::new(capability).map_err(|e| e.to_string())?;
        let spec = WorkloadSpec {
            kind: workload.kind.clone(),
            name: workload.id.clone(),
            requirements: WorkloadRequirements {
                common: vec![Requirement::exact(id)],
                variants: Vec::new(),
                memory: MemoryDemand::default(),
            },
            policy: PlacementPolicy::default(),
        };
        spec.validate().map_err(|e| e.to_string())?;
        Ok(spec)
    }

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn adapters(&self) -> &'static [&'static str] {
        &["logical", "wasmtime-project-v1"]
    }
}

/// Everything [`prepare_project`] checks a project against.
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix, feature = "native"))]
pub struct ProjectFacts<'a> {
    /// The certificate in force for the project. `None` before its first
    /// registration: the child registers its key on first boot, so the
    /// workload carries `key_id = "unregistered"` and serial 0.
    pub cert: Option<&'a clawft_types::project::ProjectCert>,
    /// The user public key the certificate must chain to.
    pub user_pubkey: &'a [u8; 32],
    /// The revocation view over the user chain.
    pub revocations: &'a crate::project_identity::RevocationView,
    /// The project id the manifest registered.
    pub manifest_id: &'a str,
    /// The manifest's canonical root.
    pub root: &'a Path,
    /// Hex hash of the policy in force (`parent_hash`), recorded on the
    /// workload; may be empty before the first policy export.
    pub policy_hash: &'a str,
}

/// Why a project could not be turned into a workload.
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix, feature = "native"))]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectPrepareError {
    /// Certificate signature, key id or expiry check failed.
    #[error("certificate: {0}")]
    Cert(String),
    /// The key was revoked or replaced.
    #[error("identity: {0}")]
    Identity(String),
    /// The certificate names another project than the manifest.
    #[error("certificate is for project {cert}, manifest is {manifest}")]
    WrongProject {
        /// Id in the certificate.
        cert: String,
        /// Id in the manifest.
        manifest: String,
    },
    /// The root is not a directory.
    #[error("project root {0} is not a directory")]
    RootGone(String),
}

/// Verify the certificate chain and the manifest binding, then build the
/// workload the `logical` adapter runs. Refuses a revoked or replaced key,
/// a certificate for another project and a missing root.
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix, feature = "native"))]
pub fn prepare_project(f: &ProjectFacts<'_>) -> Result<VerifiedWorkload, ProjectPrepareError> {
    let (project_id, key_id, serial, user_key_id) = match f.cert {
        Some(cert) => {
            crate::project_identity::verify_cert(cert, f.user_pubkey)
                .map_err(|e| ProjectPrepareError::Cert(e.to_string()))?;
            f.revocations
                .check_cert(cert)
                .map_err(|e| ProjectPrepareError::Identity(e.to_string()))?;
            if cert.project_id != f.manifest_id {
                return Err(ProjectPrepareError::WrongProject {
                    cert: cert.project_id.clone(),
                    manifest: f.manifest_id.to_owned(),
                });
            }
            (
                cert.project_id.clone(),
                cert.project_key_id.clone(),
                cert.serial,
                cert.user_key_id.clone(),
            )
        }
        None => (
            f.manifest_id.to_owned(),
            "unregistered".to_owned(),
            0,
            clawft_types::project::cert::key_id(f.user_pubkey),
        ),
    };
    if !f.root.is_dir() {
        return Err(ProjectPrepareError::RootGone(f.root.display().to_string()));
    }
    Ok(VerifiedWorkload {
        kind: KIND_PROJECT.into(),
        id: project_id.clone(),
        version: format!("cert-{serial}"),
        source: WorkloadSource::Project(ProjectPayload {
            adapter: "logical".into(),
            project_id,
            key_id,
            cert_serial: serial,
            user_key_id,
            policy_hash: f.policy_hash.to_owned(),
            root: f.root.to_path_buf(),
        }),
    })
}

/// Health of a running project kernel: its handshake must name the expected
/// project, and not every shared service may be down.
pub fn project_healthy(expected_id: &str, handshake_project_id: Option<&str>, shared_all_down: bool) -> bool {
    handshake_project_id == Some(expected_id) && !shared_all_down
}
