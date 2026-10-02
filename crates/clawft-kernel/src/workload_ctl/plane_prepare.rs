//! Kind-dispatched package preparation for `place` (P2-B).

use clawft_types::placement::engine::WorkloadSpec;

use crate::workload_kind::peek_manifest_kind;
use crate::workload_runtime::VerifiedWorkload;

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
    pub(super) fn prepare(
        &self,
        order: &PlaceOrder,
    ) -> Result<(VerifiedWorkload, WorkloadSpec, String), PlaneError> {
        // A manifest too broken to name a valid kind falls to the cog path,
        // whose verification reports the real problem.
        let kind_id = peek_manifest_kind(&order.package_dir)
            .unwrap_or_else(|| crate::workload_pkg::KIND_COG.to_string());
        let kind = self.kinds.require(&kind_id)?;
        let w = kind.load(&order.package_dir, &self.anchors, &self.kinds)?;
        let seeded = self
            .exchange
            .seed_package_dir_in(&order.package_dir, &self.anchors, &self.kinds)
            .map_err(|e| PlaneError::Package(e.to_string()))?;
        let spec = kind.spec(&w).map_err(PlaneError::Package)?;
        Ok((w, spec, seeded.manifest_hash))
    }
}
