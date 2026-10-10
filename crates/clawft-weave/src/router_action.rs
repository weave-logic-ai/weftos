//! The `route` dashboard action (ADR-116 R3, contract §3): the dashboard
//! sets or removes a route for a project registered on this node. The node
//! writes the project's overlay (`~/.weftos/routes/<ULID>.yaml`, never a
//! repository file), reloads the router, and answers
//! `{op, prefix, applied, reason?}`: `applied` says whether the router serves
//! the result now; a refusal (the repository wins on that prefix, a conflict
//! with another project, the router off) is the `reason`.
//!
//! Validation is R1's: prefix, port, health path and `allow` list, plus the
//! project must be registered here. The payload is never logged or echoed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::dashboard_actions::{Action, ActionHandler, ActionOutcome};
use crate::router_overlay;
use crate::router_routes::{Route, RouteDecl, RouteTable, Source, normalize_allow, normalize_prefix, port_of, valid_health};
use crate::router_state::RouterHandle;

/// The action kind.
pub const KIND: &str = "route";

/// The payload as the dashboard sends it. Field names carry no `key`, `token`,
/// `secret` or `password`: the dashboard's payload check refuses those.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutePayload {
    /// `set` or `remove`.
    pub op: String,
    pub project_ulid: String,
    pub prefix: String,
    /// Required for `set`.
    #[serde(default)]
    pub port: Option<u64>,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub allow: Vec<String>,
}

/// Which router to reload and read back: the daemon's (looked up per call,
/// since it may start after the handlers do), a given one (tests), or none.
pub enum RouterRef {
    Global,
    Handle(Arc<RouterHandle>),
    Off,
}

impl RouterRef {
    /// The router, if one is running.
    pub fn get(&self) -> Option<Arc<RouterHandle>> {
        match self {
            RouterRef::Global => crate::router_state::global(),
            RouterRef::Handle(h) => Some(h.clone()),
            RouterRef::Off => None,
        }
    }
}

/// Everything the handler needs from the machine, injected so tests never
/// touch the real home.
pub struct RouteEnv {
    /// `~/.weftos/projects`: the project must be registered here.
    pub manifests_dir: PathBuf,
    /// `~/.weftos/routes`.
    pub overlays_dir: PathBuf,
    pub router: RouterRef,
}

impl RouteEnv {
    /// The running user's home; `None` without one.
    pub fn from_process() -> Option<Self> {
        let home = clawft_types::runtime_paths::home_dir()?;
        Some(Self {
            manifests_dir: crate::user_daemon::manifests_dir(&home),
            overlays_dir: router_overlay::overlays_dir(&home),
            router: RouterRef::Global,
        })
    }
}

/// Answers `route` actions.
pub struct RouteHandler {
    env: Option<Arc<RouteEnv>>,
}

impl RouteHandler {
    pub fn new(env: RouteEnv) -> Self {
        Self { env: Some(Arc::new(env)) }
    }

    /// For `default_handlers()`: fails clearly when there is no home directory.
    pub fn from_process() -> Self {
        Self { env: RouteEnv::from_process().map(Arc::new) }
    }
}

#[async_trait]
impl ActionHandler for RouteHandler {
    fn kind(&self) -> &str {
        KIND
    }

    async fn handle(&self, action: &Action) -> ActionOutcome {
        let Some(env) = self.env.clone() else { return ActionOutcome::failed("no home directory on this node", KIND) };
        let payload = action.payload.clone();
        tokio::task::spawn_blocking(move || apply(&env, payload))
            .await
            .unwrap_or_else(|e| ActionOutcome::failed(format!("route action did not finish: {e}"), KIND))
    }
}

fn refused(op: Option<&str>, prefix: Option<&str>, reason: impl Into<String>) -> ActionOutcome {
    let reason = reason.into();
    ActionOutcome {
        status: "failed",
        result: json!({ "op": op, "prefix": prefix, "applied": false, "reason": reason, "error": reason, "kind": KIND }),
    }
}

fn registered(manifests_dir: &Path, ulid: &str) -> bool {
    clawft_types::project::list_manifests(manifests_dir).map(|l| l.manifests.iter().any(|m| m.id == ulid)).unwrap_or(false)
}

fn decl_of(p: &RoutePayload, prefix: &str) -> Result<RouteDecl, String> {
    let port = p.port.ok_or("set needs a port")?;
    port_of(port, "route")?;
    if let Some(h) = &p.health
        && !valid_health(h)
    {
        return Err(format!("health {h:?} must be a path"));
    }
    normalize_allow(&p.allow)?;
    Ok(RouteDecl { prefix: Some(prefix.to_owned()), port, health: p.health.clone(), default: p.default, allow: p.allow.clone() })
}

/// Validate, write the overlay, reload, and read the outcome back.
pub fn apply(env: &RouteEnv, payload: Value) -> ActionOutcome {
    let p: RoutePayload = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(e) => return refused(None, None, format!("payload: {e}")),
    };
    let op = p.op.as_str();
    if !matches!(op, "set" | "remove") {
        return refused(None, None, "op must be \"set\" or \"remove\"");
    }
    if !crate::mesh_pair_requests::is_ulid(&p.project_ulid) {
        return refused(Some(op), None, "project_ulid is not a ULID");
    }
    if !registered(&env.manifests_dir, &p.project_ulid) {
        return refused(Some(op), None, format!("project {} is not registered on this node", p.project_ulid));
    }
    let prefix = match normalize_prefix(&p.prefix) {
        Ok(x) => x,
        Err(e) => return refused(Some(op), None, e),
    };
    let written = match op {
        "set" => decl_of(&p, &prefix).and_then(|d| router_overlay::set_route(&env.overlays_dir, &p.project_ulid, &d)),
        _ => router_overlay::remove_route(&env.overlays_dir, &p.project_ulid, &prefix).and_then(|found| {
            found.then_some(()).ok_or_else(|| format!("no dashboard route {prefix} for this project (repository routes are edited in compose/ports.yaml)"))
        }),
    };
    if let Err(e) = written {
        return refused(Some(op), Some(&prefix), e);
    }
    let (applied, reason) = after_reload(env, op, &prefix, &p.project_ulid);
    let mut result = json!({ "op": op, "prefix": prefix, "applied": applied });
    if let Some(r) = reason {
        result["reason"] = json!(r);
    }
    ActionOutcome { status: "succeeded", result }
}

fn is_mine(t: &RouteTable, project: &str, ulid: &str) -> bool {
    t.project(project).is_some_and(|p| p.ulid.as_deref() == Some(ulid))
}

/// Reload the router and say whether the written change is what it serves.
fn after_reload(env: &RouteEnv, op: &str, prefix: &str, ulid: &str) -> (bool, Option<String>) {
    let Some(h) = env.router.get() else {
        return (false, Some("the tailnet router is off on this node; the overlay is saved and applies once [router] enabled = true".into()));
    };
    let t = h.reload();
    let served = |r: &Route| r.prefix == prefix && r.source == Source::Dashboard && is_mine(&t, &r.project, ulid);
    let still = t.routes.iter().any(served);
    match op {
        "set" if still => (true, None),
        "set" => {
            let reason = t
                .refused
                .iter()
                .find(|x| x.prefix == prefix && x.source == Source::Dashboard && is_mine(&t, &x.project, ulid))
                .map(|x| x.reason.clone())
                .unwrap_or_else(|| "the route was written but the router did not admit it".into());
            (false, Some(reason))
        }
        _ if still => (false, Some("an overlay route with this prefix is still served".into())),
        _ => (true, None),
    }
}

#[cfg(test)]
#[path = "router_action_tests.rs"]
mod tests;
