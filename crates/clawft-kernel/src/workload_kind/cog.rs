//! The `cog` kind (ADR-100), behind [`WorkloadKind`]. Behaviour is the
//! pre-registry cog path unchanged.

#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use std::path::Path;

#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_ctl::{PlaneError, cog_workload_spec};
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_pkg::{DirSource, TrustAnchors, VerifyPolicy, verify_dir_in};
use crate::workload_pkg::{ManifestEnvelope, ManifestError};
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_runtime::VerifiedWorkload;
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use clawft_types::placement::engine::WorkloadSpec;

use super::WorkloadKind;
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use super::{KindRegistry, MAX_KIND_LEN};

/// Native, container and Seed-hosted cog packages.
#[derive(Debug, Clone, Copy, Default)]
pub struct CogKind;

impl WorkloadKind for CogKind {
    fn id(&self) -> &'static str {
        crate::workload_pkg::KIND_COG
    }

    fn validate(&self, envelope: &ManifestEnvelope) -> Result<(), ManifestError> {
        envelope.cog_body().map(|_| ())
    }

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn load(
        &self,
        package_dir: &Path,
        anchors: &TrustAnchors,
        kinds: &KindRegistry,
    ) -> Result<VerifiedWorkload, PlaneError> {
        load_cog_shaped(self.id(), package_dir, anchors, kinds)
    }

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn spec(&self, workload: &VerifiedWorkload) -> Result<WorkloadSpec, String> {
        cog_workload_spec(workload)
    }

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    fn adapters(&self) -> &'static [&'static str] {
        &["native", "container", "emulated", "remote.api"]
    }
}

/// Verify and load a package whose verified body is cog-shaped, refusing it
/// unless the verified envelope's kind is `expected` (closes the window
/// between peeking the kind and verifying the package).
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
pub fn load_cog_shaped(
    expected: &str,
    package_dir: &Path,
    anchors: &TrustAnchors,
    kinds: &KindRegistry,
) -> Result<VerifiedWorkload, PlaneError> {
    let verified = verify_dir_in(package_dir, anchors, &VerifyPolicy::default(), kinds)
        .map_err(|e| PlaneError::Package(e.to_string()))?;
    if verified.envelope.kind != expected {
        return Err(PlaneError::Package(format!(
            "manifest kind {:?} is not {expected:?}",
            verified
                .envelope
                .kind
                .chars()
                .take(MAX_KIND_LEN)
                .collect::<String>()
        )));
    }
    VerifiedWorkload::from_package(&verified, &DirSource::new(package_dir))
        .map_err(|e| PlaneError::Package(e.to_string()))
}
