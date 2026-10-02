//! The Seed and cog connection, the same on native and wasm: ehttp requests fired from the UI
//! loop. One "Seed host" gives the agent API (`http://<seed>`, port 80) and the cog's export
//! (`http://<seed>:<port>`); both send `Access-Control-Allow-Origin: *`, so a browser build can
//! call them directly. Measured agent behaviour this relies on (cogs repo ADR-158):
//! `PUT /api/v1/apps/<id>/config` replaces the whole config and restarts the cog.

use crate::Probe;
use eframe::egui;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant};
use weftos_sensor_guide::GuideBundle;

/// A request older than this is treated as lost and may be re-sent.
const STALE: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Seed address: 169.254.42.1 (USB), its LAN IP, or its tailnet IP.
    pub seed_host: String,
    /// Optional pairing bearer token for agent writes.
    pub seed_token: String,
}

impl Settings {
    pub fn agent_base(&self) -> String {
        format!("http://{}", self.host())
    }

    pub fn export_base(&self, port: u16) -> String {
        format!("http://{}:{port}", self.host())
    }

    fn host(&self) -> &str {
        self.seed_host
            .trim()
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/')
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            seed_host: default_host(),
            seed_token: env("COGNITUM_SEED_TOKEN").unwrap_or_default(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn env(_k: &str) -> Option<String> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn default_host() -> String {
    env("SEED_HOST").unwrap_or_else(|| "169.254.42.1".into())
}

/// In the browser, `?seed=<host>` on the page URL picks the Seed.
#[cfg(target_arch = "wasm32")]
fn default_host() -> String {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| {
            q.trim_start_matches('?')
                .split('&')
                .find_map(|kv| kv.strip_prefix("seed=").map(String::from))
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "169.254.42.1".into())
}

/// One polled endpoint: at most one request in flight, re-sent when stale.
pub struct Lane {
    busy: Arc<AtomicBool>,
    fired: Option<Instant>,
    every: Duration,
}

impl Lane {
    pub fn new(every_ms: u64) -> Self {
        Self {
            busy: Arc::new(AtomicBool::new(false)),
            fired: None,
            every: Duration::from_millis(every_ms),
        }
    }

    /// True (and marks busy) when a new request should go out now.
    pub fn due(&mut self) -> bool {
        let now = Instant::now();
        let stale = self.fired.is_some_and(|f| now.duration_since(f) > STALE);
        let waited = self
            .fired
            .is_none_or(|f| now.duration_since(f) >= self.every);
        if waited && (!self.busy.load(Ordering::Acquire) || stale) {
            self.busy.store(true, Ordering::Release);
            self.fired = Some(now);
            return true;
        }
        false
    }

    /// Makes the lane due on the next tick.
    pub fn poke(&mut self) {
        self.fired = None;
    }
}

/// What the companion knows about the Seed and the cog.
#[derive(Clone, Default)]
pub struct CogState {
    pub seed_api: Probe,
    pub cog_installed: Option<bool>,
    pub cog_running: Option<bool>,
    /// `GET /api/v1/apps/<id>/config`.
    pub config: Option<serde_json::Value>,
    /// `GET /api/v1/apps/<id>/manifest` (typed config schema).
    pub manifest: Option<serde_json::Value>,
    /// The cog's latest report, `GET <export>/status`.
    pub status: Option<serde_json::Value>,
    pub export: Probe,
    pub guide: Option<Result<Arc<GuideBundle>, String>>,
    pub last_action: Option<String>,
}

impl CogState {
    pub fn status_str(&self, key: &str) -> Option<&str> {
        self.status.as_ref()?.get(key)?.as_str()
    }

    pub fn status_f64(&self, key: &str) -> Option<f64> {
        self.status.as_ref()?.get(key)?.as_f64()
    }
}

pub fn json_of(r: &ehttp::Result<ehttp::Response>) -> Result<serde_json::Value, String> {
    match r {
        Ok(resp) if resp.ok => serde_json::from_slice(&resp.bytes).map_err(|e| e.to_string()),
        Ok(resp) => Err(format!("HTTP {} {}", resp.status, resp.status_text)),
        Err(e) => Err(e.clone()),
    }
}

pub struct CogClient {
    pub cog_id: &'static str,
    pub export_port: u16,
    pub settings: Settings,
    state: Arc<Mutex<CogState>>,
    agent: Lane,
    config: Lane,
    manifest: Lane,
    status: Lane,
    guide: Lane,
    extra: HashMap<String, Lane>,
    /// Bumped on reconnect so replies from the previous Seed are dropped.
    generation: Arc<AtomicU64>,
}

impl CogClient {
    pub fn new(cog_id: &'static str, export_port: u16, settings: Settings) -> Self {
        Self {
            cog_id,
            export_port,
            settings,
            state: Arc::new(Mutex::new(CogState::default())),
            agent: Lane::new(3000),
            config: Lane::new(5000),
            manifest: Lane::new(30_000),
            status: Lane::new(1000),
            guide: Lane::new(15_000),
            extra: HashMap::new(),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn snapshot(&self) -> CogState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn export_base(&self) -> String {
        self.settings.export_base(self.export_port)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Switch to another Seed: forget everything learned from the previous one.
    pub fn reconnect(&mut self, settings: Settings) {
        self.settings = settings;
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut st) = self.state.lock() {
            *st = CogState::default();
        }
        for l in [
            &mut self.agent,
            &mut self.config,
            &mut self.manifest,
            &mut self.status,
            &mut self.guide,
        ] {
            l.poke();
        }
        self.extra.clear();
    }

    /// GET `url` and hand the parsed JSON to `on_done` (dropped if the Seed changed meanwhile).
    pub fn fetch(
        &self,
        url: String,
        busy: Option<Arc<AtomicBool>>,
        ctx: &egui::Context,
        on_done: impl 'static + Send + FnOnce(Result<serde_json::Value, String>),
    ) {
        let (generation, want, ctx) = (self.generation.clone(), self.generation(), ctx.clone());
        ehttp::fetch(ehttp::Request::get(url), move |r| {
            if let Some(b) = busy {
                b.store(false, Ordering::Release);
            }
            if generation.load(Ordering::Acquire) == want {
                on_done(json_of(&r));
                ctx.request_repaint();
            }
        });
    }

    /// App-specific polling: GET `<export><path>` every `every_ms`, one request in flight.
    pub fn poll_export(
        &mut self,
        path: &str,
        every_ms: u64,
        ctx: &egui::Context,
        on_done: impl 'static + Send + FnOnce(Result<serde_json::Value, String>),
    ) {
        let lane = self
            .extra
            .entry(path.to_string())
            .or_insert_with(|| Lane::new(every_ms));
        if !lane.due() {
            return;
        }
        let busy = lane.busy.clone();
        let url = format!("{}{path}", self.export_base());
        self.fetch(url, Some(busy), ctx, on_done);
    }

    /// Fires whichever of the standard polls are due. Call once per frame.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let agent = self.settings.agent_base();
        let id = self.cog_id;
        if self.agent.due() {
            let st = self.state.clone();
            self.fetch(
                format!("{agent}/api/v1/apps"),
                Some(self.agent.busy.clone()),
                ctx,
                move |r| {
                    let Ok(mut s) = st.lock() else { return };
                    match r {
                        Ok(v) => {
                            let cog = v["installed"]
                                .as_array()
                                .and_then(|a| a.iter().find(|c| c["id"] == id).cloned());
                            s.seed_api = Probe::Ok;
                            s.cog_installed = Some(cog.is_some());
                            s.cog_running = cog.as_ref().and_then(|c| c["running"].as_bool());
                        }
                        Err(e) => {
                            s.seed_api = Probe::Failed(e);
                            s.cog_installed = None;
                            s.cog_running = None;
                        }
                    }
                },
            );
        }
        for (lane, path, setter) in [
            (
                &mut self.config,
                "config",
                (|s: &mut CogState, v| s.config = Some(v)) as fn(&mut CogState, serde_json::Value),
            ),
            (&mut self.manifest, "manifest", |s: &mut CogState, v| {
                s.manifest = Some(v)
            }),
        ] {
            if lane.due() {
                let st = self.state.clone();
                let busy = lane.busy.clone();
                let url = format!("{agent}/api/v1/apps/{id}/{path}");
                let (generation, want, ctx2) = (
                    self.generation.clone(),
                    self.generation.load(Ordering::Acquire),
                    ctx.clone(),
                );
                ehttp::fetch(ehttp::Request::get(url), move |r| {
                    busy.store(false, Ordering::Release);
                    if generation.load(Ordering::Acquire) != want {
                        return;
                    }
                    if let (Ok(v), Ok(mut s)) = (json_of(&r), st.lock()) {
                        setter(&mut s, v);
                    }
                    ctx2.request_repaint();
                });
            }
        }
        let export = self.export_base();
        if self.status.due() {
            let st = self.state.clone();
            self.fetch(
                format!("{export}/status"),
                Some(self.status.busy.clone()),
                ctx,
                move |r| {
                    if let Ok(mut s) = st.lock() {
                        match r {
                            Ok(v) => {
                                s.status = Some(v);
                                s.export = Probe::Ok;
                            }
                            Err(e) => s.export = Probe::Failed(e),
                        }
                    }
                },
            );
        }
        self.tick_guide(ctx, &export);
    }

    /// The cog serves its guide at `/guide` (ADR-104), fetched until it loads. Natively,
    /// `COMPANION_GUIDE_DIR=<dir>` reads a local guide folder instead (for authoring).
    fn tick_guide(&mut self, ctx: &egui::Context, export: &str) {
        let loaded = self
            .state
            .lock()
            .map(|s| matches!(s.guide, Some(Ok(_))))
            .unwrap_or(true);
        if loaded || !self.guide.due() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = env("COMPANION_GUIDE_DIR") {
            self.guide.busy.store(false, Ordering::Release);
            let g = GuideBundle::from_dir(std::path::Path::new(&dir)).map(Arc::new);
            if let Ok(mut s) = self.state.lock() {
                s.guide = Some(g);
            }
            return;
        }
        let st = self.state.clone();
        self.fetch(
            format!("{export}/guide"),
            Some(self.guide.busy.clone()),
            ctx,
            move |r| {
                if let Ok(mut s) = st.lock() {
                    s.guide = Some(r.and_then(|v| GuideBundle::from_json(&v)).map(Arc::new));
                }
            },
        );
    }

    fn post(
        &mut self,
        url: String,
        method: &str,
        body: Vec<u8>,
        verb: &'static str,
        ctx: &egui::Context,
    ) {
        let mut req = ehttp::Request::post(url, body);
        req.method = method.into();
        req.headers.insert("Content-Type", "application/json");
        if !self.settings.seed_token.is_empty() {
            req.headers.insert(
                "Authorization",
                format!("Bearer {}", self.settings.seed_token),
            );
        }
        if let Ok(mut s) = self.state.lock() {
            s.last_action = Some(format!("{verb}: sent"));
        }
        self.agent.poke();
        self.config.poke();
        let (st, ctx) = (self.state.clone(), ctx.clone());
        ehttp::fetch(req, move |r| {
            let msg = match r {
                Ok(resp) => summarize_action(verb, resp.status, resp.text().unwrap_or("")),
                Err(e) => format!("{verb}: {e}"),
            };
            if let Ok(mut s) = st.lock() {
                s.last_action = Some(msg);
            }
            ctx.request_repaint();
        });
    }

    /// POST `/api/v1/apps/<cog>/<verb>` (start, stop, console).
    pub fn action(
        &mut self,
        verb: &'static str,
        body: Option<serde_json::Value>,
        ctx: &egui::Context,
    ) {
        let url = format!(
            "{}/api/v1/apps/{}/{verb}",
            self.settings.agent_base(),
            self.cog_id
        );
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        self.post(url, "POST", bytes, verb, ctx);
    }

    /// Merges `changes` into the cog's current config and PUTs the whole thing (the agent
    /// replaces the config on PUT). The cog restarts.
    pub fn put_config(
        &mut self,
        changes: &serde_json::Map<String, serde_json::Value>,
        ctx: &egui::Context,
    ) {
        let base = self.snapshot().config.unwrap_or(serde_json::Value::Null);
        let merged = merge_config(&base, changes);
        let url = format!(
            "{}/api/v1/apps/{}/config",
            self.settings.agent_base(),
            self.cog_id
        );
        self.post(url, "PUT", merged.to_string().into_bytes(), "config", ctx);
    }
}

pub fn merge_config(
    base: &serde_json::Value,
    changes: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let mut out = base.as_object().cloned().unwrap_or_default();
    for (k, v) in changes {
        out.insert(k.clone(), v.clone());
    }
    serde_json::Value::Object(out)
}

/// One line for the UI from an agent reply. Console replies carry the cog's stdout report.
pub fn summarize_action(verb: &str, code: u16, body: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    if verb == "console" {
        let report = v["stdout"]
            .as_array()
            .and_then(|a| a.iter().rev().find_map(|l| l.as_str()))
            .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok());
        return match report {
            Some(r) => format!("test run: exit {}, status {}", v["exit_code"], r["status"]),
            None => format!(
                "console: HTTP {code} {}",
                body.chars().take(160).collect::<String>()
            ),
        };
    }
    match v["status"].as_str() {
        Some(s) => format!("{verb}: {s}"),
        None => format!(
            "{verb}: HTTP {code} {}",
            body.chars().take(160).collect::<String>()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_host_field_yields_agent_and_export() {
        let s = Settings {
            seed_host: " http://192.0.2.10/ ".into(),
            seed_token: String::new(),
        };
        assert_eq!(s.agent_base(), "http://192.0.2.10");
        assert_eq!(s.export_base(8047), "http://192.0.2.10:8047");
    }

    #[test]
    fn config_changes_merge_into_the_full_config() {
        let base = serde_json::json!({"api_bind": "0.0.0.0:8047", "mode": "8", "rate_hz": 10});
        let mut ch = serde_json::Map::new();
        ch.insert("mode".into(), serde_json::json!("4"));
        let out = merge_config(&base, &ch);
        assert_eq!(out["mode"], "4");
        assert_eq!(out["api_bind"], "0.0.0.0:8047", "untouched keys survive");
    }

    #[test]
    fn replies_are_summarized() {
        assert_eq!(
            summarize_action("stop", 200, r#"{"status":"stopped"}"#),
            "stop: stopped"
        );
        let body = r#"{"exit_code":0,"stdout":["{\"status\":\"ok\"}"]}"#;
        assert_eq!(
            summarize_action("console", 200, body),
            "test run: exit 0, status \"ok\""
        );
        assert!(summarize_action("start", 401, "nope").starts_with("start: HTTP 401"));
    }

    #[test]
    fn a_lane_waits_its_interval_and_one_request_at_a_time() {
        let mut l = Lane::new(60_000);
        assert!(l.due());
        assert!(!l.due());
        l.busy.store(false, Ordering::Release);
        assert!(!l.due());
        l.poke();
        assert!(l.due());
    }
}
