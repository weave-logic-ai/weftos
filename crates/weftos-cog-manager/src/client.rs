//! The manager's connection to a WeftOS cog-host (`weft-cog-host`, default :9480) and to the two
//! marketplace registries. Same code on native and wasm: ehttp requests fired from the UI loop,
//! results dropped into shared state. The host and the registries send `Access-Control-Allow-Origin:
//! *`, so a browser build calls them directly.

use base64::Engine as _;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant};
use weftos_cog_market::{Catalog, CognitumRegistry, Source};
use weftos_cog_repo::Registry as WlRegistry;

const STATUS_EVERY: Duration = Duration::from_millis(1500);
const STALE: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct Settings {
    /// cog-host base, e.g. `http://127.0.0.1:9480` or `http://100.109.125.18:9480`.
    pub host: String,
    /// WeaveLogic signed registry.json URL (optional; empty = skip).
    pub our_registry: String,
    /// Cognitum app-registry.json URL (optional; empty = skip).
    pub cognitum_registry: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // native: env; browser: ?host= / ?wl= / ?cog= query params; else the default.
            host: setting("WEFTOS_HOST", "host", "http://127.0.0.1:9480"),
            our_registry: setting("WEFTOS_WL_REGISTRY", "wl", ""),
            cognitum_registry: setting(
                "WEFTOS_COGNITUM_REGISTRY",
                "cog",
                "https://storage.googleapis.com/cognitum-apps/app-registry.json",
            ),
        }
    }
}

// ---- host /status shape (mirror of weftos_cog_host::supervise::CogStatus) ----

#[derive(Deserialize, Clone, Default)]
pub struct HostCog {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub restarts: u32,
    #[serde(default)]
    pub rss_kb: Option<u64>,
    #[serde(default)]
    pub uptime_s: Option<u64>,
    #[serde(default)]
    pub last_exit: Option<String>,
    #[serde(default)]
    pub signed: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct HostStatus {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub running: usize,
    #[serde(default)]
    pub cogs: Vec<HostCog>,
}

// ---- /network shape ----

#[derive(Deserialize, Clone, Default)]
pub struct NetPeer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub online: bool,
    #[serde(default, rename = "self")]
    pub is_self: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct Tailscale {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub peers: Vec<NetPeer>,
}

#[derive(Deserialize, Clone, Default)]
pub struct FleetNode {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub sensor: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub rssi: Option<i32>,
    #[serde(default)]
    pub battery: Option<f64>,
    #[serde(default)]
    pub fw: String,
    #[serde(default)]
    pub age_s: u64,
    #[serde(default)]
    pub online: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct Net {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub tailscale: Tailscale,
    /// Cognitum mesh overlay peers (count + peers), kept raw.
    #[serde(default)]
    pub cognitum_mesh: serde_json::Value,
    /// Edge nodes that checked in (COG-010 Fleet).
    #[serde(default)]
    pub fleet: Vec<FleetNode>,
}

/// A running cog's `/guide` bundle fetch (the ADR-104 guide the cog serves on its export port).
#[derive(Default)]
pub struct GuideFetch {
    pub id: String,
    pub port: u16,
    /// None while in flight; Some once the request resolves.
    pub result: Option<Result<serde_json::Value, String>>,
}

#[derive(Default)]
pub struct Shared {
    pub host: Option<Result<HostStatus, String>>,
    pub net: Option<Result<Net, String>>,
    pub our_reg: Option<Result<WlRegistry, String>>,
    pub cognitum_reg: Option<Result<CognitumRegistry, String>>,
    pub catalog: Option<Catalog>,
    pub last_action: Option<String>,
    /// The guide currently being viewed in the Sensors tab (one at a time).
    pub guide: Option<GuideFetch>,
}

pub struct Client {
    pub s: Settings,
    shared: Arc<Mutex<Shared>>,
    status_busy: Arc<AtomicBool>,
    status_fired: Option<Instant>,
    net_busy: Arc<AtomicBool>,
    net_fired: Option<Instant>,
    catalog_started: bool,
}

impl Client {
    pub fn new(s: Settings) -> Self {
        Self {
            s,
            shared: Arc::new(Mutex::new(Shared::default())),
            status_busy: Arc::new(AtomicBool::new(false)),
            status_fired: None,
            net_busy: Arc::new(AtomicBool::new(false)),
            net_fired: None,
            catalog_started: false,
        }
    }

    pub fn snapshot(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    /// Reconnect to a (possibly new) host/registries: clear state and refetch.
    pub fn reconnect(&mut self, s: Settings) {
        self.s = s;
        *self.shared.lock().unwrap() = Shared::default();
        self.status_fired = None;
        self.status_busy.store(false, Ordering::Release);
        self.net_fired = None;
        self.net_busy.store(false, Ordering::Release);
        self.catalog_started = false;
    }

    /// Call every frame: poll the host + network, and (once) the registries, then build the catalog.
    pub fn tick(&mut self, ctx: &eframe::egui::Context) {
        self.poll_status(ctx);
        self.poll_network(ctx);
        if !self.catalog_started {
            self.catalog_started = true;
            self.fetch_registries(ctx);
        }
        self.try_build_catalog();
    }

    fn poll_network(&mut self, ctx: &eframe::egui::Context) {
        let now = Instant::now();
        let stale = self.net_fired.is_some_and(|f| now.duration_since(f) > STALE);
        let waited = self.net_fired.is_none_or(|f| now.duration_since(f) >= Duration::from_millis(4000));
        if !(waited && (!self.net_busy.load(Ordering::Acquire) || stale)) {
            return;
        }
        self.net_busy.store(true, Ordering::Release);
        self.net_fired = Some(now);
        let url = format!("{}/network", base(&self.s.host));
        let shared = Arc::clone(&self.shared);
        let busy = Arc::clone(&self.net_busy);
        let ctx = ctx.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            shared.lock().unwrap().net = Some(parse_json::<Net>(&res));
            busy.store(false, Ordering::Release);
            ctx.request_repaint();
        });
    }

    fn poll_status(&mut self, ctx: &eframe::egui::Context) {
        let now = Instant::now();
        let stale = self.status_fired.is_some_and(|f| now.duration_since(f) > STALE);
        let waited = self.status_fired.is_none_or(|f| now.duration_since(f) >= STATUS_EVERY);
        if !(waited && (!self.status_busy.load(Ordering::Acquire) || stale)) {
            return;
        }
        self.status_busy.store(true, Ordering::Release);
        self.status_fired = Some(now);
        let url = format!("{}/status", base(&self.s.host));
        let shared = Arc::clone(&self.shared);
        let busy = Arc::clone(&self.status_busy);
        let ctx = ctx.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let parsed = parse_json::<HostStatus>(&res);
            shared.lock().unwrap().host = Some(parsed);
            busy.store(false, Ordering::Release);
            ctx.request_repaint();
        });
    }

    fn fetch_registries(&self, ctx: &eframe::egui::Context) {
        if !self.s.our_registry.trim().is_empty() {
            let url = self.s.our_registry.clone();
            let shared = Arc::clone(&self.shared);
            let ctx = ctx.clone();
            ehttp::fetch(ehttp::Request::get(url), move |res| {
                shared.lock().unwrap().our_reg = Some(parse_json::<WlRegistry>(&res));
                ctx.request_repaint();
            });
        } else {
            self.shared.lock().unwrap().our_reg = Some(Err("not configured".into()));
        }
        if !self.s.cognitum_registry.trim().is_empty() {
            let url = self.s.cognitum_registry.clone();
            let shared = Arc::clone(&self.shared);
            let ctx = ctx.clone();
            ehttp::fetch(ehttp::Request::get(url), move |res| {
                let parsed = match &res {
                    Ok(r) if r.ok => CognitumRegistry::parse(&r.bytes),
                    Ok(r) => Err(format!("HTTP {} {}", r.status, r.status_text)),
                    Err(e) => Err(e.clone()),
                };
                shared.lock().unwrap().cognitum_reg = Some(parsed);
                ctx.request_repaint();
            });
        } else {
            self.shared.lock().unwrap().cognitum_reg = Some(Err("not configured".into()));
        }
    }

    /// Once both registry fetches have resolved, build the unified catalog.
    fn try_build_catalog(&self) {
        let mut sh = self.shared.lock().unwrap();
        if sh.catalog.is_some() || sh.our_reg.is_none() || sh.cognitum_reg.is_none() {
            return;
        }
        let our = sh.our_reg.as_ref().and_then(|r| r.as_ref().ok());
        let cog = sh.cognitum_reg.as_ref().and_then(|r| r.as_ref().ok());
        sh.catalog = Some(Catalog::build(our, cog));
    }

    /// Start or stop a host-managed cog (`action` = "start" | "stop").
    pub fn lifecycle(&self, id: &str, action: &str, ctx: &eframe::egui::Context) {
        let url = format!("{}/cogs/{id}/{action}", base(&self.s.host));
        let shared = Arc::clone(&self.shared);
        let busy = Arc::clone(&self.status_busy);
        let ctx = ctx.clone();
        let id = id.to_string();
        let action = action.to_string();
        let mut req = ehttp::Request::post(url, Vec::new());
        req.headers.insert("content-type", "application/json");
        ehttp::fetch(req, move |res| {
            let msg = match &res {
                Ok(r) if r.ok => format!("{action} {id}: ok"),
                Ok(r) => format!("{action} {id}: HTTP {}", r.status),
                Err(e) => format!("{action} {id}: {e}"),
            };
            shared.lock().unwrap().last_action = Some(msg);
            busy.store(false, Ordering::Release); // make status refetch promptly
            ctx.request_repaint();
        });
        // force a status refresh on the next tick
        self.status_busy.store(false, Ordering::Release);
    }
}

impl Client {
    /// Install a catalog item onto the host: fetch the binary (TLS here, in the browser/native),
    /// then upload it to the host's `/install`, which verifies (Ed25519 for signed, sha256 for
    /// Cognitum) before writing. Progress/result lands in `last_action`.
    pub fn install(&self, id: &str, source: Source, version: String, ctx: &eframe::egui::Context) {
        // Resolve the binary URL + integrity material from the raw registries.
        let detail: Option<(String, String, Option<String>, bool)> = {
            let sh = self.shared.lock().unwrap();
            match source {
                Source::WeaveLogic => {
                    let b = registry_base(&self.s.our_registry);
                    sh.our_reg
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .and_then(|r| r.cogs.iter().find(|c| c.id == id).cloned())
                        .and_then(|c| c.artifacts.get("arm").or_else(|| c.artifacts.values().next()).cloned())
                        .map(|a| (format!("{b}/{}", a.path), a.sha256, Some(a.sig), true))
                }
                Source::Cognitum => sh.cognitum_reg.as_ref().and_then(|r| r.as_ref().ok()).and_then(|r| {
                    let url = r.arm_binary_url(id)?;
                    let sha = r.cogs.iter().find(|c| c.id == id).and_then(|c| c.sha256.clone())?;
                    Some((url, sha, None, false))
                }),
            }
        };
        let Some((url, sha, sig, signed)) = detail else {
            self.set_action(format!("install {id}: no binary URL (is the registry configured/loaded?)"), ctx);
            return;
        };

        self.set_action(format!("fetching {id}…"), ctx);
        let shared = Arc::clone(&self.shared);
        let host = base(&self.s.host);
        let busy = Arc::clone(&self.status_busy);
        let id = id.to_string();
        let ctx = ctx.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let bytes = match &res {
                Ok(r) if r.ok => r.bytes.clone(),
                Ok(r) => return set(&shared, &ctx, format!("fetch {id}: HTTP {}", r.status)),
                Err(e) => return set(&shared, &ctx, format!("fetch {id}: {e} (CORS? use the native console for this source)")),
            };
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let body = serde_json::json!({
                "id": id, "version": version,
                "source": if signed { "weavelogic" } else { "cognitum" },
                "signed": signed, "sha256": sha, "sig": sig,
                "binary_b64": b64, "enable": false,
            })
            .to_string();
            let mut req = ehttp::Request::post(format!("{host}/install"), body.into_bytes());
            req.headers.insert("content-type", "application/json");
            let shared2 = Arc::clone(&shared);
            let busy2 = Arc::clone(&busy);
            let ctx2 = ctx.clone();
            let id2 = id.clone();
            ehttp::fetch(req, move |r2| {
                let msg = match &r2 {
                    Ok(x) if x.ok => format!("installed {id2} ✓ ({} KB, verified)", bytes.len() / 1024),
                    Ok(x) => format!("install {id2}: {}", String::from_utf8_lossy(&x.bytes).chars().take(160).collect::<String>()),
                    Err(e) => format!("install {id2}: {e}"),
                };
                busy2.store(false, Ordering::Release); // prompt a status refresh so it shows up
                set(&shared2, &ctx2, msg);
            });
        });
    }

    fn set_action(&self, msg: String, ctx: &eframe::egui::Context) {
        self.shared.lock().unwrap().last_action = Some(msg);
        ctx.request_repaint();
    }

    /// Export base for a cog on this host: the host's address with the cog's export port, e.g.
    /// host `http://100.109.125.18:9480` + port 8050 -> `http://100.109.125.18:8050`.
    pub fn export_base(&self, port: u16) -> String {
        let h = base(&self.s.host);
        let rest = h.strip_prefix("http://").or_else(|| h.strip_prefix("https://")).unwrap_or(&h);
        let hostname = rest.split('/').next().unwrap_or(rest).rsplit_once(':').map(|(a, _)| a).unwrap_or(rest);
        format!("http://{hostname}:{port}")
    }

    /// Fetch a running cog's `/guide` bundle from its export port into `Shared.guide`. The Sensors
    /// tab renders it with `weftos-sensor-guide`. Guides need no Seed agent — only the cog's export.
    pub fn fetch_guide(&self, id: &str, port: u16, ctx: &eframe::egui::Context) {
        {
            let mut sh = self.shared.lock().unwrap();
            sh.guide = Some(GuideFetch { id: id.to_string(), port, result: None });
        }
        let url = format!("{}/guide", self.export_base(port));
        let shared = Arc::clone(&self.shared);
        let id = id.to_string();
        let ctx = ctx.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let parsed: Result<serde_json::Value, String> = match &res {
                Ok(r) if r.ok => serde_json::from_slice(&r.bytes).map_err(|e| e.to_string()),
                Ok(r) => Err(format!("HTTP {} {}", r.status, r.status_text)),
                Err(e) => Err(format!("{e} (is the cog running? CORS/port reachable?)")),
            };
            let mut sh = shared.lock().unwrap();
            // Only apply if this is still the guide we're waiting on.
            if let Some(g) = sh.guide.as_mut().filter(|g| g.id == id && g.port == port) {
                g.result = Some(parsed);
            }
            ctx.request_repaint();
        });
    }

    /// Close the open guide.
    pub fn clear_guide(&self) {
        self.shared.lock().unwrap().guide = None;
    }
}

fn set(shared: &Arc<Mutex<Shared>>, ctx: &eframe::egui::Context, msg: String) {
    shared.lock().unwrap().last_action = Some(msg);
    ctx.request_repaint();
}

/// The directory a `registry.json` URL lives in, used as the base for relative artifact paths.
fn registry_base(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    match u.rsplit_once('/') {
        Some((base, _file)) => base.to_string(),
        None => u.to_string(),
    }
}

fn base(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    if h.starts_with("http://") || h.starts_with("https://") {
        h.to_string()
    } else {
        format!("http://{h}")
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(res: &ehttp::Result<ehttp::Response>) -> Result<T, String> {
    match res {
        Ok(r) if r.ok => serde_json::from_slice::<T>(&r.bytes).map_err(|e| e.to_string()),
        Ok(r) => Err(format!("HTTP {} {}", r.status, r.status_text)),
        Err(e) => Err(e.clone()),
    }
}

// ---- setting resolution ---------------------------------------------------
// Native reads an env var; the browser reads a `?<query>=` param; else the default.

#[cfg(not(target_arch = "wasm32"))]
fn setting(env_key: &str, _query_key: &str, default: &str) -> String {
    std::env::var(env_key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

#[cfg(target_arch = "wasm32")]
fn setting(_env_key: &str, query_key: &str, default: &str) -> String {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| {
            let prefix = format!("{query_key}=");
            q.trim_start_matches('?').split('&').find_map(|kv| {
                kv.strip_prefix(&prefix).map(|v| js_decode(v))
            })
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Minimal percent-decode for query values (handles %3A %2F etc. in URLs passed as params).
#[cfg(target_arch = "wasm32")]
fn js_decode(s: &str) -> String {
    let bytes = s.replace('+', " ");
    let mut out = Vec::new();
    let mut it = bytes.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let h = (it.next(), it.next());
            if let (Some(a), Some(c)) = h {
                if let (Some(x), Some(y)) = (hexval(a), hexval(c)) {
                    out.push(x * 16 + y);
                    continue;
                }
            }
        } else {
            out.push(b);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_arch = "wasm32")]
fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
