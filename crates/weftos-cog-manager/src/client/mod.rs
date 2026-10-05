//! The manager's connection to a WeftOS cog-host (`weft-cog-host`, default :9480) and to the two
//! marketplace registries. Same code on native and wasm: ehttp requests fired from the UI loop,
//! results dropped into shared state. The host and the registries send `Access-Control-Allow-Origin:
//! *`, so a browser build calls them directly.

use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant};
use weftos_cog_market::{Catalog, CognitumRegistry, Source};
use weftos_cog_repo::Registry as WlRegistry;

const STATUS_EVERY: Duration = Duration::from_millis(1500);
const STALE: Duration = Duration::from_secs(8);

mod fetch;
mod http;
mod hw;
mod install;
mod console_token;
pub mod params;
mod settings;
mod types;

pub use console_token::{MintState, Minted};
pub use hw::*;
pub use settings::*;
pub use types::*;
use http::*;

pub struct Client {
    pub s: Settings,
    shared: Arc<Mutex<Shared>>,
    status_busy: Arc<AtomicBool>,
    status_fired: Option<Instant>,
    net_busy: Arc<AtomicBool>,
    net_fired: Option<Instant>,
    fleet_busy: Arc<AtomicBool>,
    fleet_fired: Option<Instant>,
    seeds_fired: Option<Instant>,
    /// Seed URLs with a read in flight (and since when), so a silent address never piles up requests.
    seeds_busy: Arc<Mutex<std::collections::BTreeMap<String, Instant>>>,
    catalog_started: bool,
    /// Gateway-issued console token (memory only) and the (gateway, project) it was asked for.
    mint: Arc<Mutex<MintState>>,
    mint_key: (String, String),
}

impl Client {
    pub fn new(s: Settings) -> Self {
        let mint_key = (s.gateway.trim().to_owned(), s.project.clone());
        Self {
            mint_key,
            mint: Arc::new(Mutex::new(MintState::Idle)),
            s,
            shared: Arc::new(Mutex::new(Shared::default())),
            status_busy: Arc::new(AtomicBool::new(false)),
            status_fired: None,
            net_busy: Arc::new(AtomicBool::new(false)),
            net_fired: None,
            fleet_busy: Arc::new(AtomicBool::new(false)),
            fleet_fired: None,
            seeds_fired: None,
            seeds_busy: Arc::new(Mutex::new(Default::default())),
            catalog_started: false,
        }
    }

    pub fn snapshot(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap()
    }

    /// Reconnect to a (possibly new) host/registries: clear state and refetch.
    pub fn reconnect(&mut self, s: Settings) {
        self.s = s;
        // A new gateway or project needs its own token; a failed ask is retried on "Use".
        let key = (self.s.gateway.trim().to_owned(), self.s.project.clone());
        let mut m = self.mint.lock().unwrap();
        if key != self.mint_key || matches!(*m, MintState::Failed(_) | MintState::Pending) {
            *m = MintState::Idle;
        }
        drop(m);
        self.mint_key = key;
        {
            let mut sh = self.shared.lock().unwrap();
            let epoch = sh.epoch + 1;
            *sh = Shared { epoch, ..Shared::default() };
        }
        self.status_fired = None;
        self.status_busy.store(false, Ordering::Release);
        self.net_fired = None;
        self.net_busy.store(false, Ordering::Release);
        self.fleet_fired = None;
        self.seeds_fired = None;
        self.fleet_busy.store(false, Ordering::Release);
        self.catalog_started = false;
    }

    /// Call every frame: poll the host + network, and (once) the registries, then build the catalog.
    pub fn tick(&mut self, ctx: &eframe::egui::Context) {
        self.poll_status(ctx);
        self.poll_network(ctx);
        self.poll_mint(ctx);
        self.poll_fleet(ctx);
        self.poll_seeds(ctx);
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
        let epoch = epoch_of(&shared);
        let busy = Arc::clone(&self.net_busy);
        let ctx = ctx.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            apply(&shared, epoch, |sh| sh.net = Some(parse_json::<Net>(&res)));
            busy.store(false, Ordering::Release);
            ctx.request_repaint();
        });
    }

    /// The daemon's `fleet.snapshot` through the ADR-102 gateway, every 5 s, when a gateway is set.
    fn poll_fleet(&mut self, ctx: &eframe::egui::Context) {
        let Some(url) = crate::fleet::snapshot_url(&self.s.gateway) else { return };
        // Wait for the gateway-issued token rather than fetch once without one.
        if self.s.gateway_token.trim().is_empty() && matches!(self.mint_state(), MintState::Pending) {
            return;
        }
        let now = Instant::now();
        let stale = self.fleet_fired.is_some_and(|f| now.duration_since(f) > STALE);
        let waited = self.fleet_fired.is_none_or(|f| now.duration_since(f) >= Duration::from_secs(5));
        if !(waited && (!self.fleet_busy.load(Ordering::Acquire) || stale)) {
            return;
        }
        self.fleet_busy.store(true, Ordering::Release);
        self.fleet_fired = Some(now);
        let shared = Arc::clone(&self.shared);
        let epoch = epoch_of(&shared);
        let busy = Arc::clone(&self.fleet_busy);
        let ctx = ctx.clone();
        let token = self.gateway_token();
        let minted = self.s.gateway_token.trim().is_empty();
        let mint = Arc::clone(&self.mint);
        ehttp::fetch(get_req(url, &token), move |res| {
            let parsed = match &res {
                Ok(r) if r.status == 401 || r.status == 403 => {
                    if minted {
                        let mut m = mint.lock().unwrap();
                        if matches!(*m, MintState::Done(_)) {
                            *m = MintState::Failed("gateway rejected the issued console token".into());
                        }
                    }
                    Err("the gateway wants a token: weft token issue --read-only, then set it as the gateway token".into())
                }
                _ => parse_json::<serde_json::Value>(&res),
            };
            apply(&shared, epoch, |sh| {
                if let Ok(v) = &parsed {
                    crate::fleet::push_history(&mut sh.fleet_hist, v, 120);
                }
                sh.fleet = Some(parsed);
            });
            busy.store(false, Ordering::Release);
            ctx.request_repaint();
        });
    }

    /// Read every Seed agent (listed ones, plus the connected host's own on :80) every 10 s:
    /// identity, status, firmware and thermal. All four are open, CORS-enabled agent routes.
    fn poll_seeds(&mut self, ctx: &eframe::egui::Context) {
        let now = Instant::now();
        if self.seeds_fired.is_some_and(|f| now.duration_since(f) < Duration::from_secs(10)) {
            return;
        }
        self.seeds_fired = Some(now);
        let mut targets: Vec<(String, bool)> = self.s.seeds.iter().map(|u| (u.clone(), false)).collect();
        let host = crate::fleet_unify::host_of(&base(&self.s.host));
        let auto = format!("http://{host}");
        if !host.is_empty() && !targets.iter().any(|(u, _)| crate::fleet_unify::host_of(u) == host) {
            targets.push((auto, true));
        }
        let shared = Arc::clone(&self.shared);
        let epoch = epoch_of(&shared);
        {
            let mut sh = shared.lock().unwrap();
            sh.seeds.retain(|u, _| targets.iter().any(|(t, _)| t == u));
            for (u, auto) in &targets {
                sh.seeds.entry(u.clone()).or_insert_with(|| crate::fleet_unify::SeedView { url: u.clone(), auto: *auto, ..Default::default() });
            }
        }
        for (url, _) in targets {
            {
                let mut busy = self.seeds_busy.lock().unwrap();
                if busy.get(&url).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(60)) {
                    continue;
                }
                busy.insert(url.clone(), now);
            }
            for (route, slot) in [("identity", 0u8), ("status", 1), ("firmware/status", 2), ("thermal/state", 3)] {
                let shared = Arc::clone(&shared);
                let ctx = ctx.clone();
                let key = url.clone();
                let busy = Arc::clone(&self.seeds_busy);
                let req = ehttp::Request::get(format!("{url}/api/v1/{route}"));
                ehttp::fetch(req, move |res| {
                    if slot == 0 {
                        busy.lock().unwrap().remove(&key);
                    }
                    let parsed = parse_json::<serde_json::Value>(&res);
                    apply(&shared, epoch, |sh| {
                        if let Some(sv) = sh.seeds.get_mut(&key) {
                            match parsed {
                                Ok(v) => {
                                    let cell = match slot { 0 => &mut sv.identity, 1 => &mut sv.status, 2 => &mut sv.firmware, _ => &mut sv.thermal };
                                    *cell = Some(v);
                                    if slot == 0 {
                                        sv.error = None;
                                    }
                                }
                                // Only the identity call decides whether the agent is there.
                                Err(e) if slot == 0 => {
                                    sv.identity = None;
                                    sv.error = Some(e);
                                }
                                Err(_) => {}
                            }
                        }
                    });
                    ctx.request_repaint();
                });
            }
        }
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
        let epoch = epoch_of(&shared);
        let busy = Arc::clone(&self.status_busy);
        let ctx = ctx.clone();
        // with the host token the rows also carry output-log stats and listening ports
        ehttp::fetch(get_req(url, &self.s.token), move |res| {
            let parsed = parse_json::<HostStatus>(&res);
            apply(&shared, epoch, |sh| sh.host = Some(parsed));
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
        post_headers(&mut req, &self.s.token);
        ehttp::fetch(req, move |res| {
            let msg = match &res {
                Ok(r) if r.ok => format!("{action} {id}: ok"),
                Ok(r) => format!("{action} {id}: {}", http_err(r)),
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
    fn set_action(&self, msg: String, ctx: &eframe::egui::Context) {
        self.shared.lock().unwrap().last_action = Some(msg);
        ctx.request_repaint();
    }

    /// Export base for a cog on this host: the host's address with the cog's export port, e.g.
    /// host `http://100.64.0.10:9480` + port 8050 -> `http://100.64.0.10:8050`.
    pub fn export_base(&self, port: u16) -> String {
        let h = base(&self.s.host);
        let rest = h.strip_prefix("http://").or_else(|| h.strip_prefix("https://")).unwrap_or(&h);
        let hostname = rest.split('/').next().unwrap_or(rest).rsplit_once(':').map(|(a, _)| a).unwrap_or(rest);
        format!("http://{hostname}:{port}")
    }
}
