//! "Identify hardware" modal for the Catalog tab: scans the host's USB bus (`GET /hw/usb`), shows
//! what is new / known / removed / unidentified, links matches into the catalog and can ask the
//! host's agent about unknown devices. All requests are on demand (open, Rescan, buttons) - no polling.

use crate::client::{Client, HwUsbDevice, HwUsbReport, Identify};
use crate::{AMBER, GREEN, GREY, RED};
use eframe::egui::{self, Color32, RichText};
use weftos_cog_market::hw::HwCatalog;

/// Where "catalog →" sends the Catalog tab: tab (1 modules, 2 chips) + a search string.
pub struct Jump {
    pub tab: u8,
    pub search: String,
}

/// The inline "Register species" panel for one device.
#[derive(Default)]
struct Register {
    key: String,
    search: String,
    wild_name: String,
}

#[derive(Default)]
pub struct HwIdentify {
    open: bool,
    register: Option<Register>,
}

fn badge(ui: &mut egui::Ui, text: &str, bg: Color32) {
    ui.label(RichText::new(format!(" {text} ")).small().strong().color(Color32::BLACK).background_color(bg));
}

fn is_hub(d: &HwUsbDevice) -> bool {
    d.id.as_ref().is_some_and(|i| i.kind == "hub")
}

fn lookup_url(d: &HwUsbDevice) -> String {
    format!("https://devicehunt.com/view/type/usb/vendor/{}/device/{}", d.vid.to_uppercase(), d.pid.to_uppercase())
}

/// Catalog destination for a device's id-table row, only when that id exists in the catalog.
fn catalog_jump(cat: &HwCatalog, d: &HwUsbDevice) -> Option<(String, Jump)> {
    let id = d.id.as_ref()?;
    if let Some(m) = id.module.as_deref().and_then(|m| cat.module(m)) {
        return Some((format!("module: {}", m.name), Jump { tab: 1, search: m.id.clone() }));
    }
    let c = id.chip.as_deref().and_then(|c| cat.chip(c))?;
    Some((format!("chip: {}", c.name), Jump { tab: 2, search: c.id.clone() }))
}

impl HwIdentify {
    /// Open the modal and kick off a scan.
    pub fn open(&mut self, client: &Client, ctx: &egui::Context) {
        self.open = true;
        client.hw_scan(ctx);
    }

    /// Draw the modal if open. Returns a catalog jump when the user follows a link.
    pub fn show(&mut self, ctx: &egui::Context, client: &Client, cat: &HwCatalog) -> Option<Jump> {
        if !self.open {
            return None;
        }
        // Copy out of the shared state so the UI below can call client methods without re-locking.
        let (report, age_s, answers) = {
            let sh = client.snapshot();
            (sh.hw_usb.clone(), sh.hw_usb_at.map(|t| t.elapsed().as_secs()), sh.hw_identify.clone())
        };
        let mut jump = None;
        let width = (ctx.content_rect().width() - 80.0).clamp(320.0, 860.0);
        let modal = egui::Modal::new(egui::Id::new("hw_identify_modal")).show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.heading("🔍 Identify hardware");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        self.open = false;
                    }
                });
            });
            self.toolbar(ui, ctx, client, &report, age_s);
            ui.separator();
            match &report {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("scanning…");
                    });
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(format!("scan failed: {e}")).color(RED));
                    ui.label(RichText::new("needs a weft-cog-host with /hw/usb/scan. A tailnet or local connection registers a mesh key for that.").color(GREY).small());
                }
                Some(Ok(r)) => jump = self.rows(ui, ctx, client, cat, r, &answers),
            }
        });
        if modal.should_close() {
            self.open = false;
        }
        jump.inspect(|_| self.open = false)
    }

    fn toolbar(&self, ui: &mut egui::Ui, ctx: &egui::Context, client: &Client, report: &Option<Result<HwUsbReport, String>>, age_s: Option<u64>) {
        ui.horizontal_wrapped(|ui| {
            if let Some(Ok(r)) = report {
                let when = age_s.map(|s| format!("scanned {s}s ago")).unwrap_or_default();
                ui.label(RichText::new(format!("host {} · {when}", if r.node.is_empty() { "?" } else { &r.node })).color(GREY).small());
            }
            if ui.button("⟳ Rescan").clicked() {
                client.hw_scan(ctx);
            }
            if ui.button("Mark all as known").on_hover_text("save the current devices as the baseline").clicked() {
                client.hw_baseline(None, ctx);
            }
        });
        if let Some(Ok(r)) = report {
            let (n, new) = (r.devices.len(), r.devices.iter().filter(|d| d.state == "new").count());
            let unknown = r.devices.iter().filter(|d| d.id.is_none()).count();
            ui.label(RichText::new(format!("{n} devices · {new} new · {} removed · {unknown} unidentified", r.removed.len())).strong());
            if r.baseline_missing {
                ui.label(RichText::new("No baseline yet, so every device shows as NEW. \"Mark all as known\" starts change tracking.").color(AMBER).small());
            }
        }
    }

    fn rows(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, client: &Client, cat: &HwCatalog, r: &HwUsbReport, answers: &std::collections::BTreeMap<String, Identify>) -> Option<Jump> {
        let mut jump = None;
        // Pending catches live on the host until acknowledged, so a closed window or a stale
        // response cannot lose one.
        if !r.unseen_catches.is_empty() {
            for c in &r.unseen_catches {
                ui.label(RichText::new(format!("✨ NEW CATCH! #{:03} {} ({})", c.number, c.name, c.rarity)).strong().size(15.0).color(crate::hw_dex::rarity_color(&c.rarity)));
            }
            if ui.small_button("Got it").clicked() {
                client.hw_ack(ctx);
            }
        }
        egui::ScrollArea::vertical().max_height(ctx.content_rect().height() - 220.0).auto_shrink([false, true]).show(ui, |ui| {
            if r.devices.is_empty() && r.removed.is_empty() {
                ui.label(RichText::new("no USB devices found").color(GREY));
            }
            // hubs are plumbing: list them last
            let mut devs: Vec<&HwUsbDevice> = r.devices.iter().collect();
            devs.sort_by_key(|d| is_hub(d));
            for d in devs {
                if let Some(j) = self.device_row(ui, ctx, client, cat, d, answers.get(&d.key)) {
                    jump = Some(j);
                }
                ui.separator();
            }
            for g in &r.removed {
                ui.horizontal_wrapped(|ui| {
                    badge(ui, "REMOVED", RED);
                    let name = if g.product.is_empty() { "(device)" } else { &g.product };
                    ui.label(RichText::new(name).strong());
                    ui.label(RichText::new(format!("{}:{}", g.vid, g.pid)).monospace().color(GREY));
                    ui.label(RichText::new(&g.manufacturer).color(GREY));
                });
                ui.separator();
            }
            if !r.unmatched_ports.is_empty() {
                ui.label(RichText::new("serial ports not matched to a device").color(GREY).small());
                for p in &r.unmatched_ports {
                    ui.label(RichText::new(p).monospace().small());
                }
            }
        });
        jump
    }

    fn device_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, client: &Client, cat: &HwCatalog, d: &HwUsbDevice, answer: Option<&Identify>) -> Option<Jump> {
        let mut jump = None;
        ui.horizontal_wrapped(|ui| {
            if d.state == "new" {
                badge(ui, "NEW", GREEN);
            } else {
                badge(ui, "KNOWN", GREY);
            }
            let hub = is_hub(d);
            if d.id.is_none() {
                badge(ui, "UNKNOWN", AMBER);
            }
            if d.id.as_ref().is_some_and(|i| i.claimed) {
                badge(ui, "CLAIMED", AMBER);
            }
            let name = d.id.as_ref().map(|i| i.name.as_str()).filter(|n| !n.is_empty()).unwrap_or(if d.product.is_empty() { "unnamed device" } else { &d.product });
            ui.label(if hub { RichText::new(name).color(GREY) } else { RichText::new(name).strong() });
            ui.label(RichText::new(format!("{}:{}", d.vid, d.pid)).monospace().color(GREY));
            if !d.manufacturer.is_empty() {
                ui.label(RichText::new(&d.manufacturer).color(GREY));
            }
            for p in &d.ports {
                ui.label(RichText::new(p).monospace().small());
            }
            if let Some((label, j)) = catalog_jump(cat, d)
                && ui.small_button(format!("catalog → {label}")).clicked()
            {
                jump = Some(j);
            }
        });
        egui::CollapsingHeader::new(RichText::new("More info").small()).id_salt(format!("hwi-{}", d.key)).show(ui, |ui| {
            egui::Grid::new(format!("hwg-{}", d.key)).num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                let mut row = |k: &str, v: &str| {
                    if !v.is_empty() {
                        ui.label(RichText::new(k).color(GREY).small());
                        ui.label(RichText::new(v).small());
                        ui.end_row();
                    }
                };
                row("product", &d.product);
                row("class", &d.class);
                row("speed", &d.speed);
                row("bus path", &d.bus_path);
                row("serial", d.serial_redacted.as_deref().unwrap_or(""));
                if let Some(i) = &d.id {
                    row("kind", &i.kind);
                    row("notes", &i.notes);
                    if i.claimed {
                        row("match", "claimed by product string only (a device can spoof this)");
                    }
                }
            });
            ui.hyperlink_to(RichText::new("look up vid:pid ↗").small(), lookup_url(d));
        });
        if d.id.is_none() {
            match answer {
                Some(Identify::Pending) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("asking the agent…").color(GREY).small());
                    });
                }
                Some(Identify::Done(Ok(text))) => {
                    ui.add(egui::Label::new(text.as_str()).selectable(true).wrap());
                    if ui.small_button("Ask again").clicked() {
                        client.hw_identify(&d.key, ctx);
                    }
                }
                Some(Identify::Done(Err(e))) => {
                    ui.label(RichText::new(e).color(RED).small());
                    if ui.small_button("Retry").clicked() {
                        client.hw_identify(&d.key, ctx);
                    }
                }
                None => {
                    if ui.small_button("Ask agent").clicked() {
                        client.hw_identify(&d.key, ctx);
                    }
                }
            }
            if ui.small_button("Register species…").on_hover_text("link this device to a catalog item, or add a wild species").clicked() {
                self.register = Some(Register { key: d.key.clone(), search: String::new(), wild_name: d.product.clone() });
            }
            self.register_panel(ui, ctx, client, cat, d, answer);
        }
        jump
    }

    fn register_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, client: &Client, cat: &HwCatalog, d: &HwUsbDevice, answer: Option<&Identify>) {
        let Some(reg) = self.register.as_mut().filter(|r| r.key == d.key) else { return };
        let text = match answer {
            Some(Identify::Done(Ok(t))) => t.clone(),
            _ => String::new(),
        };
        let mut done = false;
        ui.group(|ui| {
            ui.label(RichText::new("Register species").strong());
            ui.add(egui::TextEdit::singleline(&mut reg.search).hint_text("search the catalog…").desired_width(260.0));
            let q = reg.search.to_lowercase();
            if !q.is_empty() {
                let mods = cat.modules.iter().filter(|m| format!("{} {}", m.id, m.name).to_lowercase().contains(&q)).map(|m| (format!("module:{}", m.id), format!("{} (module)", m.name)));
                let chips = cat.chips.iter().filter(|c| format!("{} {}", c.id, c.name).to_lowercase().contains(&q)).map(|c| (format!("chip:{}", c.id), format!("{} (chip)", c.name)));
                for (r, label) in mods.chain(chips).take(8) {
                    if ui.small_button(label).clicked() {
                        client.hw_dex_catch(&d.key, &r, "", &text, ctx);
                        done = true;
                    }
                }
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("or wild:").small().color(GREY));
                ui.add(egui::TextEdit::singleline(&mut reg.wild_name).hint_text("species name").desired_width(180.0));
                if ui.small_button("Register as wild").clicked() {
                    client.hw_dex_catch(&d.key, "wild", &reg.wild_name, &text, ctx);
                    done = true;
                }
                if ui.small_button("Cancel").clicked() {
                    done = true;
                }
            });
        });
        if done {
            self.register = None;
        }
    }
}
