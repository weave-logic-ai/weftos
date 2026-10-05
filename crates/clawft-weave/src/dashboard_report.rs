//! The daemon-native dashboard reporter (`[dashboard]`, see
//! [`crate::dashboard_cfg`]): a heartbeat to `POST /api/nodes/heartbeat` with
//! the node token as a bearer, replacing the `heartbeat.sh` systemd timer.
//!
//! The report carries what the kernel itself knows: version, mesh listen
//! address, gateway URL, `systemctl --user` unit health (Linux, when present)
//! and, per registered project (by ULID), the child state from the project
//! supervisor. The token is read from its 0600 file on every beat (so a
//! rotation takes effect at once), is never logged, and never leaves the
//! `Authorization` header. Failures back off; a rejected token (401/403) is
//! logged clearly and the reporter keeps running.
//!
//! Rotation lives in [`crate::dashboard_token`]; the RPC and mesh entry points
//! in [`crate::dashboard_rpc`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::dashboard_cfg::{self, DashboardConfig};

/// First heartbeat is sent this long after start.
pub const FIRST_BEAT: Duration = Duration::from_secs(5);
/// Longest wait between attempts while failing.
pub const MAX_BACKOFF: Duration = Duration::from_secs(900);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE: usize = 64 * 1024;
const UNIT_TIMEOUT: Duration = Duration::from_secs(5);

static GLOBAL: OnceLock<Arc<Dashboard>> = OnceLock::new();

/// The running reporter, once [`crate::dashboard_rpc::start`] enabled it.
pub fn global() -> Option<Arc<Dashboard>> {
    GLOBAL.get().cloned()
}

pub(crate) fn install_global(d: Arc<Dashboard>) -> bool {
    GLOBAL.set(d).is_ok()
}

/// Where per-project child states come from.
#[async_trait]
pub trait ChildSource: Send + Sync {
    /// `(project ULID, child state)` for every registered project.
    async fn children(&self) -> Vec<(String, String)>;
}

/// The user daemon's manifest store plus its project supervisor.
pub struct SupervisorChildren {
    /// `~/.weftos/projects`.
    pub manifests_dir: PathBuf,
}

#[async_trait]
impl ChildSource for SupervisorChildren {
    async fn children(&self) -> Vec<(String, String)> {
        let dir = self.manifests_dir.clone();
        let ids: Vec<String> = tokio::task::spawn_blocking(move || {
            clawft_types::project::list_manifests(&dir)
                .map(|l| l.manifests.into_iter().map(|m| m.id).collect())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        let states: Vec<(String, String)> = match crate::project_supervisor::global() {
            Some(sup) => sup
                .status_all()
                .await
                .into_iter()
                .map(|s| {
                    let st = serde_json::to_value(s.state)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".into());
                    (s.project_id, st)
                })
                .collect(),
            None => Vec::new(),
        };
        ids.into_iter()
            .map(|id| {
                let st = states.iter().find(|(i, _)| *i == id).map(|(_, s)| s.clone());
                (id, st.unwrap_or_else(|| "not running".into()))
            })
            .collect()
    }
}

/// What the daemon knows about itself that is not per project.
#[derive(Debug, Clone, Default)]
pub struct Ambient {
    /// Mesh listen address, if the mesh is configured.
    pub mesh_listen: Option<String>,
    /// Gateway URL, if known.
    pub gateway_url: Option<String>,
}

/// Outcome of one heartbeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Beat {
    /// 2xx.
    Ok,
    /// 401 or 403: the dashboard does not accept this token.
    Rejected(u16),
    /// Anything else (transport error, other status, unusable token file).
    Failed(String),
}

/// Reporter state shown by `dashboard.status` (no token material).
#[derive(Debug, Clone, Default, Serialize)]
pub struct DashState {
    /// RFC 3339 time of the last attempt.
    pub last_heartbeat_at: Option<String>,
    /// `ok`, `rejected (401)` or `failed: ...`.
    pub last_heartbeat: Option<String>,
    /// Failed attempts since the last success.
    pub consecutive_failures: u32,
    /// RFC 3339 time of the last rotation attempt.
    pub last_rotation_at: Option<String>,
    /// `ok`, or why it failed.
    pub last_rotation: Option<String>,
}

/// The reporter.
pub struct Dashboard {
    pub(crate) cfg: DashboardConfig,
    pub(crate) http: reqwest::Client,
    pub(crate) state: Mutex<DashState>,
    /// Serialises rotation and persistence of a rotated token.
    pub(crate) token_lock: tokio::sync::Mutex<()>,
    /// A rotated token that could not be written (the old one is revoked): used
    /// in place of the file until it is persisted.
    pub(crate) pending: Mutex<Option<String>>,
    ambient: Ambient,
    children: Arc<dyn ChildSource>,
}

/// `unit_<name>` key of a systemd unit: `weftos.service` is `unit_weftos`,
/// `weftos-gateway.service` is `unit_gateway` (the keys `heartbeat.sh` sent).
pub fn unit_key(unit: &str) -> String {
    let stem = unit.split('.').next().unwrap_or(unit);
    let stem = if stem == "weftos" { stem } else { stem.strip_prefix("weftos-").unwrap_or(stem) };
    let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("unit_{clean}")
}

async fn unit_state(unit: &str) -> String {
    if !cfg!(target_os = "linux") {
        return "unknown".into();
    }
    let run = tokio::process::Command::new("systemctl")
        .args(["--user", "is-active", "--", unit])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(UNIT_TIMEOUT, run).await {
        Ok(Ok(o)) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if s.is_empty() || s.len() > 32 { "unknown".into() } else { s }
        }
        _ => "unknown".into(),
    }
}

impl Dashboard {
    /// A reporter for a validated, enabled config.
    pub fn new(cfg: DashboardConfig, ambient: Ambient, children: Arc<dyn ChildSource>) -> Result<Arc<Self>, String> {
        cfg.validate()?;
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            // The bearer must never follow a redirect to another host.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("weaver/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Arc::new(Self {
            cfg,
            http,
            state: Mutex::new(DashState::default()),
            token_lock: tokio::sync::Mutex::new(()),
            pending: Mutex::new(None),
            ambient,
            children,
        }))
    }

    /// The config this reporter runs with.
    pub fn config(&self) -> &DashboardConfig {
        &self.cfg
    }

    pub(crate) fn with_state(&self, f: impl FnOnce(&mut DashState)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// The token in use: a rotated-but-unwritten one, else the file.
    pub(crate) fn current_token(&self) -> Result<String, String> {
        if let Some(t) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(t);
        }
        dashboard_cfg::read_token(self.cfg.token_path()?)
    }

    /// POST `body` to `path` under the dashboard URL with `token` as the bearer.
    /// Returns the status and the (capped) response text.
    pub(crate) async fn post(&self, path: &str, token: &str, body: &Value) -> Result<(u16, String), String> {
        let url = format!("{}{path}", self.cfg.base_url()?);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| e.without_url().to_string())?;
        let cut = &bytes[..bytes.len().min(MAX_RESPONSE)];
        Ok((status, String::from_utf8_lossy(cut).into_owned()))
    }

    /// The heartbeat request body (what `heartbeat.sh` sent).
    pub async fn build_body(&self) -> Value {
        let version = env!("CARGO_PKG_VERSION");
        let mut unit_facts = Map::new();
        for u in &self.cfg.units {
            unit_facts.insert(unit_key(u), Value::String(unit_state(u).await));
        }
        let mut observed = Map::new();
        for (ulid, child) in self.children.children().await {
            let mut o = Map::new();
            o.insert("release".into(), json!(format!("v{version}")));
            o.extend(unit_facts.clone());
            o.insert("project_child_state".into(), json!(child));
            if let Some(m) = &self.ambient.mesh_listen {
                o.insert("mesh_listen".into(), json!(m));
            }
            if let Some(g) = &self.ambient.gateway_url {
                o.insert("gateway_url".into(), json!(g));
            }
            observed.insert(ulid, Value::Object(o));
        }
        json!({
            "node_id": self.cfg.node_id,
            "installation_id": self.cfg.installation_id,
            "report": { "host": self.cfg.installation_id, "weaver_version": version, "observed": observed },
        })
    }

    /// Send one heartbeat and record the outcome.
    pub async fn heartbeat_once(&self) -> Beat {
        let beat = self.try_beat().await;
        let text = match &beat {
            Beat::Ok => "ok".to_owned(),
            Beat::Rejected(s) => format!("rejected ({s})"),
            Beat::Failed(e) => format!("failed: {e}"),
        };
        let ok = beat == Beat::Ok;
        self.with_state(|s| {
            s.last_heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
            s.last_heartbeat = Some(text);
            s.consecutive_failures = if ok { 0 } else { s.consecutive_failures.saturating_add(1) };
        });
        beat
    }

    async fn try_beat(&self) -> Beat {
        let body = self.build_body().await;
        let mut token = match self.current_token() {
            Ok(t) => t,
            Err(e) => return Beat::Failed(e),
        };
        for attempt in 0..2 {
            match self.post("/api/nodes/heartbeat", &token, &body).await {
                Ok((s, _)) if (200..300).contains(&s) => return Beat::Ok,
                Ok((s @ (401 | 403), _)) => {
                    // A rotation may have landed between reading the token and
                    // sending: retry once with the current one before reporting.
                    match self.current_token() {
                        Ok(t) if t != token && attempt == 0 => token = t,
                        _ => return Beat::Rejected(s),
                    }
                }
                Ok((s, _)) => return Beat::Failed(format!("dashboard answered HTTP {s}")),
                Err(e) => return Beat::Failed(e),
            }
        }
        Beat::Rejected(401)
    }

    /// Delay before the next attempt after `failures` consecutive failures.
    pub fn backoff(interval: Duration, failures: u32) -> Duration {
        if failures == 0 {
            return interval;
        }
        let factor = 1u32 << failures.min(10);
        interval.saturating_mul(factor).min(MAX_BACKOFF.max(interval))
    }

    /// Run the heartbeat loop until the task is dropped.
    pub fn spawn_loop(self: &Arc<Self>, first: Duration) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(first).await;
            let interval = Duration::from_secs(me.cfg.interval_secs);
            let mut last: Option<Beat> = None;
            loop {
                me.persist_pending().await;
                let beat = me.heartbeat_once().await;
                match (&beat, &last) {
                    (Beat::Ok, Some(prev)) if *prev != Beat::Ok => tracing::info!("dashboard heartbeat recovered"),
                    (Beat::Rejected(s), _) => tracing::error!(
                        status = s,
                        "dashboard token rejected: the dashboard does not accept this node's token; \
                         issue a new one and write it to the token file (the reporter keeps running)"
                    ),
                    (Beat::Failed(e), prev) if prev.as_ref() != Some(&beat) => {
                        tracing::warn!(error = %e, "dashboard heartbeat failed (backing off)")
                    }
                    _ => {}
                }
                let failures = me.state.lock().unwrap_or_else(|e| e.into_inner()).consecutive_failures;
                last = Some(beat);
                tokio::time::sleep(Self::backoff(interval, failures)).await;
            }
        })
    }

    /// Reporter status for `dashboard.status`: no token material.
    pub fn status_json(&self) -> Value {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let token_check = match self.cfg.token_path().map(dashboard_cfg::check_token_file) {
            Ok(Ok(())) => "ok".to_owned(),
            Ok(Err(e)) | Err(e) => e,
        };
        let unpersisted = self.pending.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        json!({
            "enabled": true,
            "url": self.cfg.url,
            "node_id": self.cfg.node_id,
            "installation_id": self.cfg.installation_id,
            "interval_secs": self.cfg.interval_secs,
            "token_file": self.cfg.token_file,
            "token_file_check": token_check,
            "token_unpersisted": unpersisted,
            "allow_remote_rotate": self.cfg.allow_remote_rotate,
            "last_heartbeat_at": s.last_heartbeat_at,
            "last_heartbeat": s.last_heartbeat,
            "consecutive_failures": s.consecutive_failures,
            "last_rotation_at": s.last_rotation_at,
            "last_rotation": s.last_rotation,
        })
    }
}

#[cfg(test)]
#[path = "dashboard_report_tests.rs"]
mod tests;
