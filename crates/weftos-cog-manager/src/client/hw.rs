//! Hardware identify and Dex: the `/hw/*` shapes and the calls that fetch them (token-guarded).

use super::*;

// ---- /hw/usb shapes (mirror of weftos_cog_host::usb::report) ----

#[derive(Deserialize, Clone, Default)]
pub struct HwUsbId {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub chip: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub notes: String,
    /// Matched on a product string only, which any device on a shared VID can spoof.
    #[serde(default)]
    pub claimed: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct HwUsbDevice {
    pub key: String,
    #[serde(default)]
    pub vid: String,
    #[serde(default)]
    pub pid: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub serial_redacted: Option<String>,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub speed: String,
    #[serde(default)]
    pub bus_path: String,
    #[serde(default)]
    pub ports: Vec<String>,
    /// "new" | "known"
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub id: Option<HwUsbId>,
}

#[derive(Deserialize, Clone, Default)]
pub struct HwUsbRemoved {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub vid: String,
    #[serde(default)]
    pub pid: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub product: String,
}

#[derive(Deserialize, Clone, Default)]
pub struct HwUsbReport {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub baseline_missing: bool,
    #[serde(default)]
    pub devices: Vec<HwUsbDevice>,
    #[serde(default)]
    pub removed: Vec<HwUsbRemoved>,
    #[serde(default)]
    pub unmatched_ports: Vec<String>,
    /// Catches not yet acknowledged ("NEW CATCH!"); persisted on the host until acked.
    #[serde(default)]
    pub unseen_catches: Vec<DexCatch>,
}

/// A caught catalog item (`GET /hw/dex` `caught[]`, and `unseen_catches[]`).
#[derive(Deserialize, Clone, Default)]
pub struct DexCatch {
    /// `module:<id>` | `chip:<id>`
    #[serde(default, rename = "ref")]
    pub r: String,
    #[serde(default)]
    pub number: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rarity: String,
    #[serde(default)]
    pub first_caught_at: u64,
    #[serde(default)]
    pub caught_on: String,
    #[serde(default)]
    pub times_seen: u32,
    #[serde(default)]
    pub via: String,
    /// Sensor grade C/B/A/S, when the item has one.
    #[serde(default)]
    pub grade: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
pub struct DexTypeMember {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub grade: String,
    #[serde(default)]
    pub caught: bool,
}

/// One sensor type row of `GET /hw/dex` `types[]`.
#[derive(Deserialize, Clone, Default)]
pub struct DexTypeRow {
    #[serde(default, rename = "type")]
    pub type_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub caught: bool,
    #[serde(default)]
    pub best_caught_grade: Option<String>,
    #[serde(default)]
    pub best_available_grade: Option<String>,
    #[serde(default)]
    pub upgrade_available: bool,
    #[serde(default)]
    pub members: Vec<DexTypeMember>,
}

#[derive(Deserialize, Clone, Default)]
pub struct DexWild {
    #[serde(default)]
    pub vid: String,
    #[serde(default)]
    pub pid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub times_seen: u32,
}

#[derive(Deserialize, Clone, Default)]
pub struct DexBadge {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub earned: bool,
    #[serde(default)]
    pub have: usize,
    #[serde(default)]
    pub need: usize,
}

#[derive(Deserialize, Clone, Default)]
pub struct DexCount {
    #[serde(default)]
    pub caught: usize,
    #[serde(default)]
    pub total: usize,
}

#[derive(Deserialize, Clone, Default)]
pub struct DexTotals {
    #[serde(default)]
    pub modules: DexCount,
    #[serde(default)]
    pub chips: DexCount,
}

#[derive(Deserialize, Clone, Default)]
pub struct HwDexReport {
    #[serde(default)]
    pub caught: Vec<DexCatch>,
    #[serde(default)]
    pub wild: Vec<DexWild>,
    #[serde(default)]
    pub totals: DexTotals,
    #[serde(default)]
    pub badges: Vec<DexBadge>,
    #[serde(default)]
    pub types: Vec<DexTypeRow>,
    /// ref -> dex number, for every catalog item (so uncaught ones show "#042 ???").
    #[serde(default)]
    pub numbers: std::collections::BTreeMap<String, u32>,
}

/// Progress of an "Ask agent" call for one device key.
#[derive(Clone)]
pub enum Identify {
    Pending,
    /// Ok(answer) | Err(error + hint)
    Done(Result<String, String>),
}

impl Client {
    /// Scan the host's USB bus (`GET /hw/usb`). On demand: the modal calls this on open and Rescan.
    pub fn hw_scan(&self, ctx: &eframe::egui::Context) {
        self.shared.lock().unwrap().hw_usb = None;
        // POST: a scan the user asked for records sightings in the dex (GET /hw/usb is read-only).
        let url = format!("{}/hw/usb/scan", base(&self.s.host));
        let mut req = ehttp::Request::post(url, Vec::new());
        post_headers(&mut req, &self.s.token);
        let shared = Arc::clone(&self.shared);
        let ctx = ctx.clone();
        ehttp::fetch(req, move |res| {
            let mut sh = shared.lock().unwrap();
            sh.hw_usb = Some(parse_json::<HwUsbReport>(&res));
            sh.hw_usb_at = Some(Instant::now());
            ctx.request_repaint();
        });
    }

    /// Accept the current scan (`keys = None`) or just `keys` as the known baseline, then rescan.
    pub fn hw_baseline(&self, keys: Option<Vec<String>>, ctx: &eframe::egui::Context) {
        let url = format!("{}/hw/usb/baseline", base(&self.s.host));
        let body = match keys {
            Some(k) => serde_json::json!({ "keys": k }).to_string().into_bytes(),
            None => Vec::new(),
        };
        let mut req = ehttp::Request::post(url, body);
        post_headers(&mut req, &self.s.token);
        let (shared, host, ctx2, token) = (Arc::clone(&self.shared), self.s.host.clone(), ctx.clone(), self.s.token.clone());
        ehttp::fetch(req, move |res| {
            let msg = match &res {
                Ok(r) if r.ok => "baseline saved".to_string(),
                Ok(r) => format!("baseline: {}", http_err(r)),
                Err(e) => format!("baseline: {e}"),
            };
            shared.lock().unwrap().last_action = Some(msg);
            let url = format!("{}/hw/usb", base(&host));
            let (shared, ctx3) = (Arc::clone(&shared), ctx2.clone());
            ehttp::fetch(get_req(url, &token), move |res| {
                let mut sh = shared.lock().unwrap();
                sh.hw_usb = Some(parse_json::<HwUsbReport>(&res));
                sh.hw_usb_at = Some(Instant::now());
                ctx3.request_repaint();
            });
        });
    }

    /// Ask the host's agent what a device is (`POST /hw/usb/identify {key}`); the answer lands in
    /// `hw_identify[key]`. The host blocks up to ~90 s, so the UI shows a spinner meanwhile.
    pub fn hw_identify(&self, key: &str, ctx: &eframe::egui::Context) {
        self.shared.lock().unwrap().hw_identify.insert(key.to_string(), Identify::Pending);
        let url = format!("{}/hw/usb/identify", base(&self.s.host));
        let mut req = ehttp::Request::post(url, serde_json::json!({ "key": key }).to_string().into_bytes());
        post_headers(&mut req, &self.s.token);
        let (shared, ctx, key) = (Arc::clone(&self.shared), ctx.clone(), key.to_string());
        ehttp::fetch(req, move |res| {
            // Error bodies (503 no agent / 404 rescan) are JSON too: surface their message + hint.
            let out = match &res {
                Ok(r) => match serde_json::from_slice::<serde_json::Value>(&r.bytes) {
                    Ok(v) if v["ok"] == true => Ok(v["answer"].as_str().unwrap_or("").to_string()),
                    Ok(v) => {
                        let e = v["error"].as_str().unwrap_or("identify failed");
                        Err(match v["hint"].as_str() {
                            Some(h) => format!("{e} ({h})"),
                            None => e.to_string(),
                        })
                    }
                    Err(_) => Err(http_err(r)),
                },
                Err(e) => Err(e.clone()),
            };
            shared.lock().unwrap().hw_identify.insert(key, Identify::Done(out));
            ctx.request_repaint();
        });
    }

    /// Acknowledge the "NEW CATCH!" banners (`POST /hw/dex/ack`), then refresh the scan and dex.
    pub fn hw_ack(&self, ctx: &eframe::egui::Context) {
        let url = format!("{}/hw/dex/ack", base(&self.s.host));
        let mut req = ehttp::Request::post(url, Vec::new());
        post_headers(&mut req, &self.s.token);
        let (shared, ctx, host, token) = (Arc::clone(&self.shared), ctx.clone(), self.s.host.clone(), self.s.token.clone());
        ehttp::fetch(req, move |res| {
            let msg = match &res {
                Ok(r) if r.ok => "acknowledged".to_string(),
                Ok(r) => format!("ack: {}", http_err(r)),
                Err(e) => format!("ack: {e}"),
            };
            shared.lock().unwrap().last_action = Some(msg);
            for path in ["hw/usb", "hw/dex"] {
                let (shared, ctx) = (Arc::clone(&shared), ctx.clone());
                let url = format!("{}/{path}", base(&host));
                ehttp::fetch(get_req(url, &token), move |res| {
                    let mut sh = shared.lock().unwrap();
                    if path == "hw/usb" {
                        sh.hw_usb = Some(parse_json::<HwUsbReport>(&res));
                    } else {
                        sh.hw_dex = Some(parse_json::<HwDexReport>(&res));
                    }
                    ctx.request_repaint();
                });
            }
        });
    }

    /// Fetch the Hardware Dex (`GET /hw/dex`).
    pub fn hw_dex_fetch(&self, ctx: &eframe::egui::Context) {
        let url = format!("{}/hw/dex", base(&self.s.host));
        let (shared, ctx) = (Arc::clone(&self.shared), ctx.clone());
        ehttp::fetch(get_req(url, &self.s.token), move |res| {
            shared.lock().unwrap().hw_dex = Some(parse_json::<HwDexReport>(&res));
            ctx.request_repaint();
        });
    }

    /// "Register species": link an attached device to a catalog ref (`module:<id>`/`chip:<id>`) or
    /// register it as `wild` (with `name`). Rescans and refreshes the dex afterwards.
    pub fn hw_dex_catch(&self, key: &str, catalog_id: &str, name: &str, answer: &str, ctx: &eframe::egui::Context) {
        let url = format!("{}/hw/dex/catch", base(&self.s.host));
        let body = serde_json::json!({ "key": key, "catalog_id": catalog_id, "name": name, "answer": answer });
        let mut req = ehttp::Request::post(url, body.to_string().into_bytes());
        post_headers(&mut req, &self.s.token);
        let (shared, ctx) = (Arc::clone(&self.shared), ctx.clone());
        let this = (self.s.clone(), catalog_id.to_string());
        ehttp::fetch(req, move |res| {
            let msg = match &res {
                Ok(r) => match serde_json::from_slice::<serde_json::Value>(&r.bytes) {
                    Ok(v) if v["ok"] == true => format!("registered {}", this.1),
                    Ok(v) => format!("register failed: {}", v["error"].as_str().unwrap_or("error")),
                    Err(_) => format!("register: {}", http_err(r)),
                },
                Err(e) => format!("register: {e}"),
            };
            {
                let mut sh = shared.lock().unwrap();
                sh.last_action = Some(msg);
            }
            for path in ["hw/usb", "hw/dex"] {
                let (shared, ctx) = (Arc::clone(&shared), ctx.clone());
                let url = format!("{}/{path}", base(&this.0.host));
                ehttp::fetch(get_req(url, &this.0.token), move |res| {
                    let mut sh = shared.lock().unwrap();
                    if path == "hw/usb" {
                        sh.hw_usb = Some(parse_json::<HwUsbReport>(&res));
                        sh.hw_usb_at = Some(Instant::now());
                    } else {
                        sh.hw_dex = Some(parse_json::<HwDexReport>(&res));
                    }
                    ctx.request_repaint();
                });
            }
        });
    }
}
