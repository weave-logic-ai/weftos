//! Seed operations outside the cog lifecycle: pairing and firmware upgrade.
//!
//! Firmware upgrade is a governed, backed-up operation (COG-001 section 5):
//! a 0.10.x upgrade hit the witness-chain `writes_gated` state, recovered
//! with `/api/v1/store/truncate-confirm` after a backup. So:
//! 1. [`SeedApiRuntime::backup`] snapshots what the API exposes;
//! 2. [`SeedApiRuntime::upgrade_firmware`] refuses without a verified backup
//!    or while writes are gated, then applies;
//! 3. [`SeedApiRuntime::recover_writes_gated`] runs the truncate-confirm
//!    recovery only when writes are gated and the backup still verifies.
//!
//! Callers go through [`super::WorkloadHost`], which gates each step as
//! `workload.install` with kind `seed-firmware` and chains the result.

use std::path::{Path, PathBuf};

use clawft_types::secret::SecretString;
use serde_json::{Value, json};

use super::seed::{API_TIMEOUT, SeedApiRuntime};
use super::seed_http::Method;
use super::types::RuntimeError;

/// Workload kind used to govern firmware operations.
pub const KIND_SEED_FIRMWARE: &str = "seed-firmware";

/// A verified local backup of a Seed's API-visible state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedBackup {
    /// Backup directory.
    pub dir: PathBuf,
    /// Firmware version at backup time.
    pub firmware: String,
    /// `(file name, blake3)` for each captured file.
    pub files: Vec<(String, String)>,
    /// BLAKE3 of `manifest.json`.
    pub manifest_blake3: String,
}

/// Result of an upgrade request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeOutcome {
    /// Nothing pending.
    UpToDate {
        /// Current firmware.
        version: String,
    },
    /// Upgrade applied; check [`SeedApiRuntime::writes_gated`] afterwards.
    Applied {
        /// Firmware before the upgrade.
        from: String,
    },
}

fn gated(status: &Value) -> bool {
    status
        .pointer("/integrity/writes_gated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| RuntimeError::Backend(format!("backup write {}: {e}", path.display())))?;
    f.write_all(bytes)
        .map_err(|e| RuntimeError::Backend(format!("backup write: {e}")))
}

impl SeedBackup {
    /// Re-hash every file; fails if anything changed or went missing.
    pub fn verify(&self) -> Result<(), RuntimeError> {
        let m = std::fs::read(self.dir.join("manifest.json"))
            .map_err(|e| RuntimeError::AdmissionRefused(format!("backup manifest: {e}")))?;
        if blake3::hash(&m).to_hex().as_str() != self.manifest_blake3 {
            return Err(RuntimeError::AdmissionRefused(
                "backup manifest changed".into(),
            ));
        }
        for (name, hash) in &self.files {
            let b = std::fs::read(self.dir.join(name))
                .map_err(|e| RuntimeError::AdmissionRefused(format!("backup file {name}: {e}")))?;
            if blake3::hash(&b).to_hex().as_str() != hash {
                return Err(RuntimeError::AdmissionRefused(format!(
                    "backup file {name} changed"
                )));
            }
        }
        Ok(())
    }

    /// Chain-safe summary.
    pub fn audit(&self) -> Value {
        json!({
            "firmware": self.firmware,
            "files": self.files.len(),
            "manifest_blake3": self.manifest_blake3,
        })
    }
}

impl SeedApiRuntime {
    /// Pair with the Seed: open the pairing window (authorized by
    /// `bootstrap`, which may be empty over the trusted USB link), request a
    /// client token, and store it in the operator secret store.
    pub async fn pair(
        &self,
        client_name: &str,
        bootstrap: &SecretString,
    ) -> Result<(), RuntimeError> {
        if !crate::workload_pkg::manifest::valid_token(client_name, 64) {
            return Err(RuntimeError::InvalidConfig(
                "client name must be a plain token".into(),
            ));
        }
        let t = &self.transport;
        let (s, _) = t
            .request(
                Method::Post,
                "/api/v1/pair/window",
                None,
                bootstrap,
                API_TIMEOUT,
            )
            .await?;
        if !(200..300).contains(&s) {
            return Err(RuntimeError::Backend(format!("seed pair window: HTTP {s}")));
        }
        let body = json!({ "client_name": client_name });
        let (s, v) = t
            .request(
                Method::Post,
                "/api/v1/pair",
                Some(&body),
                bootstrap,
                API_TIMEOUT,
            )
            .await?;
        if !(200..300).contains(&s) {
            return Err(RuntimeError::Backend(format!("seed pair: HTTP {s}")));
        }
        let token = v
            .get("token")
            .and_then(Value::as_str)
            .filter(|t| t.len() >= 16 && t.len() <= 512 && t.bytes().all(|b| b.is_ascii_graphic()))
            .ok_or_else(|| {
                RuntimeError::Backend("seed pair: no usable token in response".into())
            })?;
        self.creds.put(&self.cfg.node_id, SecretString::new(token))
    }

    /// Current `writes_gated` flag from `/api/v1/status`.
    pub async fn writes_gated(&self) -> Result<bool, RuntimeError> {
        let s = self
            .api(Method::Get, "/api/v1/status", None, API_TIMEOUT)
            .await?;
        Ok(gated(&s))
    }

    /// Snapshot status, identity, installed apps, each app's config and the
    /// witness chain into a new private directory `dir`.
    pub async fn backup(&self, dir: &Path) -> Result<SeedBackup, RuntimeError> {
        let mut docs: Vec<(String, Value)> = Vec::new();
        for (name, path) in [
            ("status.json", "/api/v1/status"),
            ("identity.json", "/api/v1/identity"),
            ("apps.json", "/api/v1/apps"),
            ("witness-chain.json", "/api/v1/witness/chain"),
        ] {
            docs.push((
                name.into(),
                self.api(Method::Get, path, None, API_TIMEOUT).await?,
            ));
        }
        for c in self.installed().await? {
            let v = self
                .api(
                    Method::Get,
                    &format!("/api/v1/apps/{}/config", c.id),
                    None,
                    API_TIMEOUT,
                )
                .await?;
            docs.push((format!("config-{}.json", c.id), v));
        }
        let firmware = docs
            .iter()
            .find(|(n, _)| n == "identity.json")
            .and_then(|(_, v)| v.get("firmware_version")?.as_str().map(str::to_string))
            .unwrap_or_default();
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|e| RuntimeError::Backend(format!("backup dir {}: {e}", dir.display())))?;
        }
        let mut files = Vec::new();
        for (name, v) in &docs {
            let bytes = serde_json::to_vec_pretty(v).unwrap_or_default();
            write_private(&dir.join(name), &bytes)?;
            files.push((name.clone(), blake3::hash(&bytes).to_hex().to_string()));
        }
        let manifest = serde_json::to_vec_pretty(&json!({
            "node_id": self.cfg.node_id,
            "firmware": firmware,
            "files": files,
        }))
        .unwrap_or_default();
        write_private(&dir.join("manifest.json"), &manifest)?;
        Ok(SeedBackup {
            dir: dir.to_path_buf(),
            firmware,
            files,
            manifest_blake3: blake3::hash(&manifest).to_hex().to_string(),
        })
    }

    /// Apply a pending firmware upgrade. Refuses without a verified backup
    /// or while writes are gated.
    pub async fn upgrade_firmware(
        &self,
        backup: &SeedBackup,
    ) -> Result<UpgradeOutcome, RuntimeError> {
        backup.verify()?;
        if self.writes_gated().await? {
            return Err(RuntimeError::InvalidState(
                "writes are gated; recover (truncate-confirm after backup) before upgrading".into(),
            ));
        }
        let check = self
            .api(Method::Get, "/api/v1/upgrade/check", None, API_TIMEOUT)
            .await?;
        let current = check
            .get("current_version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !check
            .get("pending_update")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(UpgradeOutcome::UpToDate { version: current });
        }
        self.api(Method::Post, "/api/v1/upgrade/apply", None, API_TIMEOUT * 6)
            .await?;
        Ok(UpgradeOutcome::Applied { from: current })
    }

    /// Recover the witness-chain `writes_gated` state. Only runs when the
    /// Seed reports writes gated and `backup` still verifies.
    pub async fn recover_writes_gated(&self, backup: &SeedBackup) -> Result<(), RuntimeError> {
        backup.verify()?;
        if !self.writes_gated().await? {
            return Err(RuntimeError::InvalidState(
                "writes are not gated; nothing to recover".into(),
            ));
        }
        self.api(
            Method::Post,
            "/api/v1/store/truncate-confirm",
            Some(&json!({ "confirm": true })),
            API_TIMEOUT,
        )
        .await
        .map(|_| ())
    }
}
