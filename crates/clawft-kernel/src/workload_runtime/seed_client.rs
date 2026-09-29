//! Seed HTTP calls shared by the adapter and the Seed operations
//! ([`super::seed_ops`]): authenticated requests, the installed list and
//! per-cog start / stop.

use std::time::Duration;

use serde_json::Value;

use super::seed::SeedApiRuntime;
use super::seed_http::Method;
use super::seed_types::{API_TIMEOUT, InstalledCog, backend};
use super::types::RuntimeError;
use crate::workload_pkg::manifest::valid_cog_id;

fn cog_path(id: &str, op: &str) -> Result<String, RuntimeError> {
    if !valid_cog_id(id) {
        return Err(RuntimeError::InvalidConfig(format!("bad cog id {id:?}")));
    }
    Ok(format!("/api/v1/apps/{id}/{op}"))
}

impl SeedApiRuntime {
    /// Call the Seed API with the stored credential.
    pub(super) async fn api(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        let token = self.creds.get(&self.cfg.node_id)?;
        let (status, v) = self
            .transport
            .request(method, path, body, &token, timeout)
            .await?;
        if !(200..300).contains(&status) {
            return Err(backend(path, status, &v));
        }
        Ok(v)
    }

    /// Installed cogs (`GET /api/v1/apps`).
    pub async fn installed(&self) -> Result<Vec<InstalledCog>, RuntimeError> {
        let v = self
            .api(Method::Get, "/api/v1/apps", None, API_TIMEOUT)
            .await?;
        let list = v
            .get("installed")
            .and_then(Value::as_array)
            .ok_or_else(|| RuntimeError::Backend("seed /api/v1/apps: no installed list".into()))?;
        Ok(list
            .iter()
            .filter_map(|a| {
                Some(InstalledCog {
                    id: a.get("id")?.as_str()?.to_string(),
                    version: a
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    running: a.get("running").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect())
    }

    /// Stop a cog by id (`POST /api/v1/apps/{id}/stop`).
    pub async fn stop_cog(&self, id: &str) -> Result<(), RuntimeError> {
        self.api(Method::Post, &cog_path(id, "stop")?, None, API_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// Start a cog by id (`POST /api/v1/apps/{id}/start`).
    pub async fn start_cog(&self, id: &str) -> Result<(), RuntimeError> {
        self.api(Method::Post, &cog_path(id, "start")?, None, API_TIMEOUT)
            .await
            .map(|_| ())
    }
}
