//! `report.services` (ADR-116 R3 add-on, contract §2): the processes each
//! registered project's process-compose runs on this machine, for the
//! dashboard's project overview. Left out of the heartbeat when there is
//! nothing to report.
//!
//! ```json
//! "services": [{"project": "shasta", "project_ulid": "<ULID or null>", "pc_port": 18110,
//!               "state": "ok" | "unreachable",
//!               "processes": [{"name": "shasta-field", "status": "Running", "ports": [18120], "restarts": 0}]}]
//! ```
//!
//! The source is each project's own process-compose HTTP API (`GET
//! /processes` on the port its `compose/ports.yaml` claims as
//! `process-compose-http`), the same read as the router index. Only a
//! project with that claim is listed. Read only: name, status and restart
//! count per process; never its command or environment. `ports` are the
//! project's claims whose `use` names the process; otherwise empty.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::router_index::{join_all, pc_state};
use crate::router_routes::ProjectInfo;
use crate::router_sources::{RouteSource, load};

/// Most projects reported.
pub const MAX_PROJECTS: usize = 16;
/// Most processes reported per project.
pub const MAX_PROCESSES: usize = 32;
/// Per process-compose probe.
pub const PC_TIMEOUT: Duration = Duration::from_millis(1500);

/// Where the services list comes from.
#[async_trait]
pub trait ServiceSource: Send + Sync {
    /// One entry per project with a process-compose claim; empty leaves the key out.
    async fn services(&self) -> Vec<Value>;
}

/// `use` and process names compared loosely: case, `_` and `-` do not matter.
fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().to_ascii_lowercase().replace('_', "-");
    norm(a) == norm(b)
}

/// Reduce one project's process-compose state to the report entry.
pub fn service_entry(p: &ProjectInfo, pc_port: u16, state: &Value) -> Value {
    let ok = state["state"] == "ok";
    let processes: Vec<Value> = if ok {
        state["processes"]
            .as_array()
            .map(|list| {
                list.iter()
                    .take(MAX_PROCESSES)
                    .map(|x| {
                        let name = x["name"].as_str().unwrap_or("-");
                        let ports: Vec<u16> = p.claims.iter().filter(|c| same_name(&c.use_, name)).map(|c| c.port).collect();
                        json!({
                            "name": name,
                            "status": x["status"].as_str().unwrap_or("-"),
                            "ports": ports,
                            "restarts": x["restarts"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    json!({
        "project": p.slug,
        "project_ulid": p.ulid,
        "pc_port": pc_port,
        "state": if ok { "ok" } else { "unreachable" },
        "processes": processes,
    })
}

/// Projects from a route source (the daemon's manifest index, or fixed
/// directories in tests), each probed on its process-compose port.
pub struct ProjectServices {
    source: Arc<dyn RouteSource>,
    timeout: Duration,
}

impl ProjectServices {
    pub fn new(source: Arc<dyn RouteSource>, timeout: Duration) -> Self {
        Self { source, timeout }
    }

    /// The daemon's registered projects.
    pub fn daemon(home: &std::path::Path) -> Self {
        let source = crate::router_sources::ManifestSource {
            manifests_dir: crate::user_daemon::manifests_dir(home),
            overlays_dir: crate::router_overlay::overlays_dir(home),
        };
        Self::new(Arc::new(source), PC_TIMEOUT)
    }
}

#[async_trait]
impl ServiceSource for ProjectServices {
    async fn services(&self) -> Vec<Value> {
        let source = self.source.clone();
        let table = tokio::task::spawn_blocking(move || load(source.as_ref())).await.unwrap_or_default();
        let projects: Vec<(ProjectInfo, u16)> = table.projects.into_iter().filter_map(|p| p.pc_http.map(|port| (p, port))).take(MAX_PROJECTS).collect();
        let timeout = self.timeout;
        let states = join_all(projects.iter().map(|(_, port)| *port).map(|port| async move { pc_state(Some(port), timeout).await })).await;
        projects.iter().zip(states).map(|((p, port), s)| service_entry(p, *port, &s)).collect()
    }
}

#[cfg(test)]
#[path = "dashboard_services_tests.rs"]
mod tests;
