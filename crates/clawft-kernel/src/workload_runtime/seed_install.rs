//! Seed install bookkeeping for [`super::SeedApiRuntime::load`]: an install
//! call whose outcome is uncertain is reconciled against the Seed's
//! installed list, an install whose follow-up stop or version check fails
//! is undone, and a cog installed at any version but the pin is refused.

use serde_json::json;

use super::{API_TIMEOUT, InstalledCog, Instance, SEED_ID, SeedApiRuntime, SeedPin};
use crate::workload_runtime::seed_http::Method;
use crate::workload_runtime::types::{InstanceHandle, RuntimeError, VerifiedWorkload};

impl SeedApiRuntime {
    /// `load` installed `w` but a follow-up `step` failed (stopping the
    /// auto-started cog, or checking the installed version): try to
    /// uninstall it. If that fails too, keep the instance tracked (as
    /// installed here) so `unload` can remove it later.
    pub(super) async fn undo_install(
        &self,
        w: &VerifiedWorkload,
        iid: &str,
        pin: &SeedPin,
        step: &str,
        cause: RuntimeError,
    ) -> RuntimeError {
        let handle = Box::new(InstanceHandle {
            runtime: SEED_ID.into(),
            instance_id: iid.into(),
            workload_id: w.id.clone(),
            store_installed: true,
        });
        let path = format!("/api/v1/apps/{}", w.id);
        // The install auto-started it and the stop has not happened yet
        // (an unpinned version): stop it, best effort, before removal.
        if step == "version check" {
            let _ = self.stop_cog(&w.id).await;
        }
        match self.api(Method::Delete, &path, None, API_TIMEOUT).await {
            Ok(_) => RuntimeError::StrandedInstall {
                handle,
                rolled_back: true,
                reason: format!("{step} after install: {cause}"),
            },
            Err(undo) => {
                self.instances.lock().await.insert(
                    iid.to_string(),
                    Instance {
                        cog_id: w.id.clone(),
                        version: pin.version.clone(),
                        installed_here: true,
                        console_commands: pin.console_commands.clone(),
                        last: None,
                    },
                );
                RuntimeError::StrandedInstall {
                    handle,
                    rolled_back: false,
                    reason: format!("{step} after install: {cause}; uninstall: {undo}"),
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

/// `id` must be installed on the Seed at exactly the pinned `version`.
pub(super) fn pinned_on_seed(
    installed: &[InstalledCog],
    id: &str,
    version: &str,
) -> Result<(), RuntimeError> {
    match installed.iter().find(|c| c.id == id) {
        Some(c) if c.version == version => Ok(()),
        Some(c) => Err(RuntimeError::AdmissionRefused(format!(
            "the Seed has {id}@{} installed but the operator pinned {version}",
            c.version
        ))),
        None => Err(RuntimeError::NotInstalled(id.to_string())),
    }
}
