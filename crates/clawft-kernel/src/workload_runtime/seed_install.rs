//! Seed install bookkeeping for [`super::SeedApiRuntime::load`]: an install
//! call whose outcome is uncertain is reconciled against the Seed's
//! installed list, and an install whose follow-up stop fails is undone.

use serde_json::json;

use super::{API_TIMEOUT, Instance, SEED_ID, SeedApiRuntime, SeedPin};
use crate::workload_runtime::seed_http::Method;
use crate::workload_runtime::types::{InstanceHandle, RuntimeError, VerifiedWorkload};

impl SeedApiRuntime {
    /// `load` installed `w` but could not stop the auto-started cog: try to
    /// uninstall it. If that fails too, keep the instance tracked (as
    /// installed here) so `unload` can remove it later.
    pub(super) async fn undo_install(
        &self,
        w: &VerifiedWorkload,
        iid: &str,
        pin: &SeedPin,
        cause: RuntimeError,
    ) -> RuntimeError {
        let handle = Box::new(InstanceHandle {
            runtime: SEED_ID.into(),
            instance_id: iid.into(),
            workload_id: w.id.clone(),
            store_installed: true,
        });
        let path = format!("/api/v1/apps/{}", w.id);
        match self.api(Method::Delete, &path, None, API_TIMEOUT).await {
            Ok(_) => RuntimeError::StrandedInstall {
                handle,
                rolled_back: true,
                reason: format!("stop after install: {cause}"),
            },
            Err(undo) => {
                self.instances.lock().await.insert(
                    iid.to_string(),
                    Instance {
                        cog_id: w.id.clone(),
                        installed_here: true,
                        console_commands: pin.console_commands.clone(),
                        last: None,
                    },
                );
                RuntimeError::StrandedInstall {
                    handle,
                    rolled_back: false,
                    reason: format!("stop after install: {cause}; uninstall: {undo}"),
                }
            }
        }
    }

    /// POST the install. If the call fails (an error or a timeout: the
    /// Seed may still have finished the install), re-read the installed
    /// list: a cog that is there now was installed by this call and is
    /// tracked like any other install, so it is stopped and can be
    /// unloaded; otherwise the original error stands.
    pub(super) async fn install_reconciled(&self, id: &str) -> Result<(), RuntimeError> {
        let Err(e) = self
            .api(
                Method::Post,
                "/api/v1/apps/install",
                Some(&json!({ "id": id })),
                API_TIMEOUT * 6,
            )
            .await
        else {
            return Ok(());
        };
        match self.installed().await {
            Ok(now) if now.iter().any(|c| c.id == id) => {
                tracing::warn!(cog = id, error = %e, "seed install call failed but the cog is installed; tracking it");
                Ok(())
            }
            _ => Err(e),
        }
    }
}
