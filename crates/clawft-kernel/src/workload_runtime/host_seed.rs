//! Governed Seed operations outside the cog lifecycle: firmware upgrade
//! and `writes_gated` recovery, each gated as `workload.install` with kind
//! `seed-firmware` and chained with the backup summary.

use serde_json::json;

use super::host::WorkloadHost;
use super::seed::{SEED_ID, SeedApiRuntime};
use super::seed_ops::{KIND_SEED_FIRMWARE, SeedBackup, UpgradeOutcome};
use super::types::{RuntimeError, VerifiedWorkload};
use crate::chain;

impl WorkloadHost {
    /// Governed, backed-up Seed firmware upgrade: gated as
    /// `workload.install` with kind `seed-firmware`, outcome chained.
    pub async fn upgrade_seed_firmware(
        &self,
        seed: &SeedApiRuntime,
        backup: &SeedBackup,
    ) -> Result<UpgradeOutcome, RuntimeError> {
        let w = VerifiedWorkload::store_pin("cognitum", "seed-firmware", "current", None)?;
        self.check("workload.install", &w, KIND_SEED_FIRMWARE, false)?;
        let r = seed.upgrade_firmware(backup).await;
        let mut payload = json!({
            "runtime": SEED_ID, "phase": "firmware-upgrade", "backup": backup.audit(),
        });
        if let Ok(o) = &r {
            payload["result"] = json!(format!("{o:?}"));
        }
        self.outcome(chain::EVENT_KIND_WORKLOAD_INSTALL, payload, &r);
        r
    }

    /// Governed `writes_gated` recovery (truncate-confirm after a backup).
    pub async fn recover_seed_writes(
        &self,
        seed: &SeedApiRuntime,
        backup: &SeedBackup,
    ) -> Result<(), RuntimeError> {
        let w = VerifiedWorkload::store_pin("cognitum", "seed-firmware", "current", None)?;
        self.check("workload.install", &w, KIND_SEED_FIRMWARE, false)?;
        let r = seed.recover_writes_gated(backup).await;
        let payload = json!({
            "runtime": SEED_ID, "phase": "writes-gated-recovery", "backup": backup.audit(),
        });
        self.outcome(chain::EVENT_KIND_WORKLOAD_INSTALL, payload, &r);
        r
    }
}
