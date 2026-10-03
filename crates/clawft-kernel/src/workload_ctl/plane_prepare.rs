//! Kind-dispatched package preparation for `place` (P2-B).

use clawft_types::placement::engine::WorkloadSpec;

use crate::workload_kind::peek_kind_in;
use crate::workload_pkg::verify::read_bounded;
use crate::workload_pkg::{MANIFEST_FILE, MAX_MANIFEST_BYTES};
use crate::workload_runtime::{VerifiedWorkload, WorkloadSource};

use super::plane::{PlacementControlPlane, PlaneError};
use super::plane_place::PlaceOrder;

impl PlacementControlPlane {
    /// Verify the package and build its spec through its registered kind;
    /// seed it for serving.
    ///
    /// The kind is peeked from the (unauthenticated) manifest only to pick
    /// the registered kind; that kind's `load` verifies the package against
    /// the registry and refuses a verified kind that differs. Nothing is
    /// seeded for an unregistered kind.
    ///
    /// The manifest is read once here and those bytes serve both the kind
    /// peek and the seeding (the kind's `load` reads the directory itself).
    /// The package the kind loaded must be the package that was seeded: a
    /// directory swapped between the two reads changes the package id and is
    /// refused.
    pub(super) fn prepare(
        &self,
        order: &PlaceOrder,
    ) -> Result<(VerifiedWorkload, WorkloadSpec, String), PlaneError> {
        // A manifest too broken to name a valid kind falls to the cog path,
        // whose verification reports the real problem.
        let manifest = read_bounded(
            &order.package_dir.join(MANIFEST_FILE),
            MANIFEST_FILE,
            MAX_MANIFEST_BYTES as u64,
        )
        .ok();
        let kind_id = manifest
            .as_deref()
            .and_then(peek_kind_in)
            .unwrap_or_else(|| crate::workload_pkg::KIND_COG.to_string());
        let kind = self.kinds.require(&kind_id)?;
        let w = kind.load(&order.package_dir, &self.anchors, &self.kinds)?;
        let manifest = manifest
            .ok_or_else(|| PlaneError::Package(format!("{MANIFEST_FILE} could not be read")))?;
        let seeded = self
            .exchange
            .seed_package_dir_manifest(&order.package_dir, &manifest, &self.anchors, &self.kinds)
            .map_err(|e| PlaneError::Package(e.to_string()))?;
        match &w.source {
            WorkloadSource::SignedPackage(p) if p.package_id == seeded.package_id => {}
            WorkloadSource::SignedPackage(_) => {
                return Err(PlaneError::Package(
                    "the package changed between verification and seeding".into(),
                ));
            }
            // A package kind must load a signed package; anything else
            // cannot be tied to what was seeded, so it is not seeded.
            _ => {
                return Err(PlaneError::Package(
                    "the kind loaded a workload that is not a signed package".into(),
                ));
            }
        }
        let spec = kind.spec(&w).map_err(PlaneError::Package)?;
        Ok((w, spec, seeded.manifest_hash))
    }
}
