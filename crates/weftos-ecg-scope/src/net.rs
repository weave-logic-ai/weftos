//! Networking, the same on native and wasm: ehttp requests fired from the UI loop. It polls
//! the cog's signal export (`http://<seed>:8046/raw` every 250 ms, `/status` every second)
//! and the Seed agent's app API (`http://<seed>/api/v1/apps` every 3 s), and sends agent
//! actions (start, stop, a simulate test run).
//!
//! The Seed agent serves plain HTTP on port 80 over USB, LAN and the tailnet with
//! `Access-Control-Allow-Origin: *`, and the cog's export sends the same header, so a browser
//! build can call both directly.

use crate::model::{Probe, State, parse_raw};
use eframe::egui;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant};

pub const COG_ID: &str = "sen0213-ecg";
pub const EXPORT_PORT: u16 = 8046;
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

    pub fn export_base(&self) -> String {
        format!("http://{}:{EXPORT_PORT}", self.host())
    }

    fn host(&self) -> &str {
        let h = self
            .seed_host
            .trim()
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        h.trim_end_matches('/')
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
fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

#[cfg(target_arch = "wasm32")]
fn env(_k: &str) -> Option<String> {
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
struct Lane {
    busy: Arc<AtomicBool>,
    fired: Option<Instant>,
    every: Duration,
}

impl Lane {
    fn new(every_ms: u64) -> Self {
        Self {
            busy: Arc::new(AtomicBool::new(false)),
            fired: None,
            every: Duration::from_millis(every_ms),
        }
    }

    /// True (and marks busy) when a new request should go out now.
    fn due(&mut self) -> bool {
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
}

pub struct Net {
    state: Arc<Mutex<State>>,
    raw: Lane,
    status: Lane,
    agent: Lane,
    config: Lane,
    /// Bumped when settings change, so replies to an old Seed are dropped.
    generation: Arc<AtomicU64>,
}

fn json_of(r: &ehttp::Result<ehttp::Response>) -> Result<serde_json::Value, String> {
    match r {
        Ok(resp) if resp.ok => {
            serde_json::from_slice::<serde_json::Value>(&resp.bytes).map_err(|e| e.to_string())
        }
        Ok(resp) => Err(format!("HTTP {} {}", resp.status, resp.status_text)),
        Err(e) => Err(e.clone()),
    }
}

impl Net {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        Self {
            state,
            raw: Lane::new(250),
            status: Lane::new(1000),
            agent: Lane::new(3000),
            config: Lane::new(5000),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Forget everything learned from the previous Seed.
    pub fn reset(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut st) = self.state.lock() {
            *st = State::default();
        }
    }

    /// Called every frame; fires whichever polls are due.
    pub fn tick(&mut self, s: &Settings, ctx: &egui::Context) {
        if self.raw.due() {
            let url = format!("{}/raw?seconds=2", s.export_base());
            self.get(url, &self.raw.busy.clone(), ctx, |st, r| {
                match json_of(&r) {
                    Ok(body) => {
                        let (samples, peaks, fs, source) = parse_raw(&body);
                        st.signal.merge(&samples, &peaks);
                        st.signal.sample_rate_hz = fs;
                        st.signal.source = source;
                        st.export = Probe::Ok;
                        st.last_poll = Some(Instant::now());
                    }
                    Err(e) => st.export = Probe::Failed(e),
                }
            });
        }
        if self.status.due() {
            let url = format!("{}/status", s.export_base());
            self.get(url, &self.status.busy.clone(), ctx, |st, r| {
                if let Ok(v) = json_of(&r) {
                    st.report = Some(v);
                }
            });
        }
        self.poll_config(s, ctx);
        if self.agent.due() {
            let url = format!("{}/api/v1/apps", s.agent_base());
            self.get(url, &self.agent.busy.clone(), ctx, |st, r| {
                match json_of(&r) {
                    Ok(v) => {
                        let cog = v["installed"]
                            .as_array()
                            .and_then(|a| a.iter().find(|c| c["id"] == COG_ID).cloned());
                        st.seed_api = Probe::Ok;
                        st.cog_installed = Some(cog.is_some());
                        st.cog_running = cog.as_ref().and_then(|c| c["running"].as_bool());
                    }
                    Err(e) => {
                        st.seed_api = Probe::Failed(e);
                        st.cog_installed = None;
                        st.cog_running = None;
                    }
                }
            });
        }
    }

    fn poll_config(&mut self, s: &Settings, ctx: &egui::Context) {
        if self.config.due() {
            let url = format!("{}/api/v1/apps/{COG_ID}/config", s.agent_base());
            self.get(url, &self.config.busy.clone(), ctx, |st, r| {
                if let Ok(v) = json_of(&r) {
                    st.cog_config = Some(v);
                }
            });
        }
    }

    /// PUT the cog config; the agent restarts the cog with it.
    pub fn put_config(&mut self, s: &Settings, cfg: serde_json::Value, ctx: &egui::Context) {
        let url = format!("{}/api/v1/apps/{COG_ID}/config", s.agent_base());
        let mut req = ehttp::Request::post(url, cfg.to_string().into_bytes());
        req.method = "PUT".into();
        req.headers.insert("Content-Type", "application/json");
        if !s.seed_token.is_empty() {
            req.headers
                .insert("Authorization", format!("Bearer {}", s.seed_token));
        }
        let (state, ctx) = (self.state.clone(), ctx.clone());
        self.config.fired = None;
        self.agent.fired = None;
        ehttp::fetch(req, move |r| {
            let msg = match r {
                Ok(resp) => summarize_action("config", resp.status, resp.text().unwrap_or("")),
                Err(e) => format!("config: {e}"),
            };
            if let Ok(mut st) = state.lock() {
                st.last_action = Some(msg);
                st.signal.clear();
            }
            ctx.request_repaint();
        });
    }

    fn get(
        &self,
        url: String,
        busy: &Arc<AtomicBool>,
        ctx: &egui::Context,
        apply: impl 'static + Send + FnOnce(&mut State, ehttp::Result<ehttp::Response>),
    ) {
        let (state, busy, ctx, generation, want) = (
            self.state.clone(),
            busy.clone(),
            ctx.clone(),
            self.generation.clone(),
            self.generation.load(Ordering::Acquire),
        );
        ehttp::fetch(ehttp::Request::get(url), move |r| {
            busy.store(false, Ordering::Release);
            if generation.load(Ordering::Acquire) != want {
                return;
            }
            if let Ok(mut st) = state.lock() {
                apply(&mut st, r);
            }
            ctx.request_repaint();
        });
    }

    /// POST /api/v1/apps/<cog>/<verb> and report the outcome in `last_action`.
    pub fn action(
        &mut self,
        s: &Settings,
        verb: &'static str,
        body: Option<serde_json::Value>,
        ctx: &egui::Context,
    ) {
        let url = format!("{}/api/v1/apps/{COG_ID}/{verb}", s.agent_base());
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let mut req = ehttp::Request::post(url, bytes);
        req.headers.insert("Content-Type", "application/json");
        if !s.seed_token.is_empty() {
            req.headers
                .insert("Authorization", format!("Bearer {}", s.seed_token));
        }
        if let Ok(mut st) = self.state.lock() {
            st.last_action = Some(format!("{verb}: sent"));
        }
        let (state, ctx) = (self.state.clone(), ctx.clone());
        self.agent.fired = None; // refresh the cog's running state right after
        ehttp::fetch(req, move |r| {
            let msg = match r {
                Ok(resp) => summarize_action(verb, resp.status, resp.text().unwrap_or("")),
                Err(e) => format!("{verb}: {e}"),
            };
            if let Ok(mut st) = state.lock() {
                st.last_action = Some(msg);
            }
            ctx.request_repaint();
        });
    }
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
            Some(r) => format!(
                "test run: exit {}, status {}, {} bpm, {} beats",
                v["exit_code"], r["status"], r["heart_rate_bpm"], r["beats"]
            ),
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
    fn console_reply_is_summarized_from_the_cog_report() {
        let body = r#"{"exit_code":0,"stdout":["{\"status\":\"ok\",\"heart_rate_bpm\":72.1,\"beats\":10}"]}"#;
        assert_eq!(
            summarize_action("console", 200, body),
            "test run: exit 0, status \"ok\", 72.1 bpm, 10 beats"
        );
    }

    #[test]
    fn start_stop_and_errors_are_summarized() {
        assert_eq!(
            summarize_action("stop", 200, r#"{"id":"x","status":"stopped"}"#),
            "stop: stopped"
        );
        assert!(summarize_action("start", 401, "unauthorized").starts_with("start: HTTP 401"));
    }

    #[test]
    fn one_host_field_yields_both_endpoints() {
        let s = Settings {
            seed_host: " http://192.0.2.10/ ".into(),
            seed_token: String::new(),
        };
        assert_eq!(s.agent_base(), "http://192.0.2.10");
        assert_eq!(s.export_base(), "http://192.0.2.10:8046");
    }

    #[test]
    fn a_lane_waits_its_interval_and_one_request_at_a_time() {
        let mut l = Lane::new(60_000);
        assert!(l.due());
        assert!(!l.due(), "busy and not yet due");
        l.busy.store(false, Ordering::Release);
        assert!(!l.due(), "idle but the interval has not passed");
    }
}
