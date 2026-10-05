//! On-demand fetches for the detail panel and the Sensors tab: guides, cog output, the mesh view
//! and node facts. Each is epoch-guarded so a reply from a host the console has left is dropped.

use super::*;

/// What to say when a guarded read has no usable bearer yet.
fn mesh_auth_gap(host: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = host;
        return "needs the host token (set it in the top bar)".into();
    }
    #[cfg(not(target_arch = "wasm32"))]
    match crate::address_book::classify(host) {
        crate::address_book::Reach::ThisMachine | crate::address_book::Reach::Tailnet => "registering a mesh key for this node".into(),
        _ => "the mesh registers a key on a tailnet or local connection".into(),
    }
}

fn guarded_read_gap(shared: &Arc<Mutex<Shared>>, host: &str) -> String {
    if shared.lock().unwrap().mesh_unsupported {
        "this host does not register mesh keys yet".into()
    } else {
        mesh_auth_gap(host)
    }
}

impl Client {
    /// Fetch a running cog's `/guide` bundle from its export port into `Shared.guide`. The Sensors
    /// tab renders it with `weftos-sensor-guide`. Guides need no Seed agent — only the cog's export.
    pub fn fetch_guide(&self, id: &str, port: u16, ctx: &eframe::egui::Context) {
        {
            let mut sh = self.shared.lock().unwrap();
            sh.guide = Some(GuideFetch { id: id.to_string(), port, result: None });
        }
        // The node's cog-host serves an installed package's guide (no running cog needed); the
        // cog's own export is the fallback for hosts or packages without one.
        let host_url = format!("{}/cogs/{id}/guide", base(&self.s.host));
        let export_url = (port != 0).then(|| format!("{}/guide", self.export_base(port)));
        let shared = Arc::clone(&self.shared);
        let epoch = epoch_of(&shared);
        let id = id.to_string();
        let ctx = ctx.clone();
        let finish = move |parsed: Result<serde_json::Value, String>| {
            // Only apply if this is still the guide we're waiting on, on the same connection.
            apply(&shared, epoch, |sh| {
                if let Some(g) = sh.guide.as_mut().filter(|g| g.id == id && g.port == port) {
                    g.result = Some(parsed);
                }
            });
            ctx.request_repaint();
        };
        let to_json = |res: &ehttp::Result<ehttp::Response>| -> Result<serde_json::Value, String> {
            match res {
                Ok(r) if r.ok => serde_json::from_slice(&r.bytes).map_err(|e| e.to_string()),
                Ok(r) => Err(format!("HTTP {} {}", r.status, r.status_text)),
                Err(e) => Err(e.clone()),
            }
        };
        ehttp::fetch(ehttp::Request::get(host_url), move |res| match (to_json(&res), export_url) {
            (Ok(v), _) => finish(Ok(v)),
            (Err(_), Some(url)) => ehttp::fetch(ehttp::Request::get(url), move |r2| finish(to_json(&r2))),
            (Err(e), None) => finish(Err(e)),
        });
    }

    /// Keep a running cog's latest output line fresh while its detail panel is open: fetches the
    /// cog export's `/status` at most every 3 s per cog. The panel calls this each frame it shows.
    pub fn ensure_cog_output(&self, id: &str, port: u16, ctx: &eframe::egui::Context) {
        {
            let mut sh = self.shared.lock().unwrap();
            let due = sh.cog_out.get(id).is_none_or(|f| f.fired.elapsed() >= Duration::from_secs(3) && (!f.in_flight || f.fired.elapsed() > STALE));
            if !due {
                return;
            }
            let prev = sh.cog_out.get(id).and_then(|f| f.result.clone());
            sh.cog_out.insert(id.to_string(), CogOutFetch { fired: Instant::now(), in_flight: true, result: prev });
        }
        // Primary: the host's own last-output route (token-guarded; health fields only; works
        // whatever the cog's export is bound to). Only a host that predates the route ("not found")
        // falls back to the cog's export `/status`; "no output yet" is a real answer, not a miss.
        let host_url = format!("{}/cogs/{id}/last", base(&self.s.host));
        let export_url = (port != 0).then(|| format!("{}/status", self.export_base(port)));
        let token = self.s.token.clone();
        let host = base(&self.s.host);
        let shared = Arc::clone(&self.shared);
        let epoch = epoch_of(&shared);
        let id = id.to_string();
        let ctx = ctx.clone();
        let finish = move |parsed: Result<serde_json::Value, String>| {
            apply(&shared, epoch, |sh| {
                if let Some(f) = sh.cog_out.get_mut(&id) {
                    f.in_flight = false;
                    f.result = Some(parsed);
                }
            });
            ctx.request_repaint();
        };
        ehttp::fetch(get_req(host_url, &token), move |res| match &res {
            Ok(r) if r.ok => finish(serde_json::from_slice(&r.bytes).map_err(|e| e.to_string())),
            Ok(r) if r.status == 401 || r.status == 403 => finish(Err(mesh_auth_gap(&host))),
            Ok(r) if r.status == 404 && !route_missing(&r.bytes) => finish(Err("no output yet".into())),
            Ok(r) if export_url.is_none() => finish(Err(format!("HTTP {} {}", r.status, r.status_text))),
            Err(e) if export_url.is_none() => finish(Err(e.clone())),
            _ => {
                if let Some(url) = export_url {
                    ehttp::fetch(ehttp::Request::get(url), move |r2| {
                        finish(match &r2 {
                            Ok(r) if r.ok => serde_json::from_slice(&r.bytes).map_err(|e| e.to_string()),
                            Ok(r) => Err(format!("HTTP {} {}", r.status, r.status_text)),
                            Err(e) => Err(e.clone()),
                        })
                    });
                }
            }
        });
    }

    /// Keep the mesh-wide cog view fresh (`GET /mesh/cogs` on the connected host, which does the
    /// peer fan-out itself). Throttled to one request per 6 s; callers invoke it each frame.
    pub fn ensure_mesh(&self, ctx: &eframe::egui::Context) {
        {
            let mut sh = self.shared.lock().unwrap();
            let due = sh.mesh.as_ref().is_none_or(|f| f.fired.elapsed() >= Duration::from_secs(6) && (!f.in_flight || f.fired.elapsed() > STALE));
            if !due {
                return;
            }
            let prev = sh.mesh.as_ref().and_then(|f| f.result.clone());
            sh.mesh = Some(MeshFetch { fired: Instant::now(), in_flight: true, result: prev });
        }
        let url = format!("{}/mesh/cogs", base(&self.s.host));
        let host = base(&self.s.host);
        let have_token = !self.s.token.trim().is_empty();
        let shared = Arc::clone(&self.shared);
        let epoch = epoch_of(&shared);
        let ctx = ctx.clone();
        ehttp::fetch(get_req(url, &self.s.token), move |res| {
            let parsed = match &res {
                Ok(r) if r.status == 404 => Err("unsupported".to_string()),
                Ok(r) if r.status == 401 && have_token => Err("the host refused the mesh key".to_string()),
                Ok(r) if r.status == 401 || r.status == 403 => Err(guarded_read_gap(&shared, &host)),
                _ => parse_json::<MeshCogs>(&res),
            };
            let refused = matches!(&parsed, Err(e) if e == "the host refused the mesh key");
            apply(&shared, epoch, |sh| {
                if refused {
                    sh.mesh_rejected = true;
                }
                if let Some(f) = sh.mesh.as_mut() {
                    f.in_flight = false;
                    f.result = Some(parsed);
                }
            });
            ctx.request_repaint();
        });
    }

    /// Fetch the connected host's bus facts for the pre-install check. At most every 8 s; callers
    /// invoke it each frame the check is showing. A tailnet or local console fills the bearer.
    pub fn ensure_node_facts(&self, ctx: &eframe::egui::Context) {
        {
            let mut sh = self.shared.lock().unwrap();
            if sh.node_facts_at.is_some_and(|t| t.elapsed() < Duration::from_secs(8)) {
                return;
            }
            sh.node_facts_at = Some(Instant::now());
        }
        let host = base(&self.s.host);
        let url = format!("{host}/hw/buses");
        let have_token = !self.s.token.trim().is_empty();
        let (shared, ctx) = (Arc::clone(&self.shared), ctx.clone());
        let epoch = epoch_of(&shared);
        ehttp::fetch(get_req(url, &self.s.token), move |res| {
            let parsed = match &res {
                Ok(r) if r.status == 401 && have_token => Err("the host refused the mesh key".to_string()),
                Ok(r) if r.status == 401 || r.status == 403 => Err(guarded_read_gap(&shared, &host)),
                Ok(r) if r.status == 404 => Err("this host predates the pre-install check".to_string()),
                _ => parse_json::<NodeFacts>(&res),
            };
            let refused = matches!(&parsed, Err(e) if e == "the host refused the mesh key");
            apply(&shared, epoch, |sh| {
                if refused {
                    sh.mesh_rejected = true;
                    sh.mesh_note = "the host refused the mesh key; registering again".into();
                }
                sh.node_facts = Some(parsed);
            });
            ctx.request_repaint();
        });
    }

    /// Close the open guide.
    pub fn clear_guide(&self) {
        self.shared.lock().unwrap().guide = None;
    }
}
