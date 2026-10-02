//! Hardware Dex: the Catalog's "Dex" sub-tab. Every catalog module/chip is a numbered entry; the
//! host marks one caught when a USB scan matches it or the user registers it (`/hw/dex`). Caught
//! entries show their details; the rest are "#042 ???" silhouettes with only a kind hint.

use crate::client::{Client, DexCatch, HwDexReport};
use crate::{AMBER, GREEN, GREY, WL};
use eframe::egui::{self, Color32, RichText};
use std::collections::BTreeMap;
use weftos_cog_market::dex::{self as rules, Rarity};
use weftos_cog_market::hw::HwCatalog;

const RARITIES: [&str; 5] = ["all", "common", "uncommon", "rare", "legendary"];

pub fn rarity_color(r: &str) -> Color32 {
    match r {
        "uncommon" => GREEN,
        "rare" => WL,
        "legendary" => AMBER,
        _ => GREY,
    }
}

/// `YYYY-MM-DD` (UTC) from unix seconds, without a date crate (days-from-civil inverse).
pub fn ymd(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Default)]
pub struct DexUi {
    /// 0 all, 1 caught, 2 uncaught
    filter: u8,
    rarity: usize,
    last_frame: u64,
}

struct Entry<'a> {
    number: u32,
    name: &'a str,
    kind: &'a str,
    rarity: Rarity,
    caught: Option<&'a DexCatch>,
}

fn entries<'a>(cat: &'a HwCatalog, rep: &'a HwDexReport) -> Vec<Entry<'a>> {
    let caught: BTreeMap<&str, &DexCatch> = rep.caught.iter().map(|c| (c.r.as_str(), c)).collect();
    let mk = |r: String, name: &'a str, kind: &'a str| {
        let rarity = rules::rarity(cat, &r).unwrap_or(Rarity::Common);
        Entry { number: rep.numbers.get(&r).copied().unwrap_or(0), caught: caught.get(r.as_str()).copied(), name, kind, rarity }
    };
    let mut v: Vec<Entry> = cat.modules.iter().map(|m| mk(rules::module_ref(&m.id), &m.name, &m.kind)).collect();
    v.extend(cat.chips.iter().map(|c| mk(rules::chip_ref(&c.id), &c.name, "chip")));
    v.sort_by_key(|e| e.number);
    v
}

fn bar(ui: &mut egui::Ui, label: &str, caught: usize, total: usize) {
    let f = if total == 0 { 0.0 } else { caught as f32 / total as f32 };
    ui.add(egui::ProgressBar::new(f).desired_width(260.0).text(format!("{label} {caught}/{total}")));
}

impl DexUi {
    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, client: &Client, cat: &HwCatalog) {
        // (Re)fetch whenever the tab was not on screen last frame; plus the Refresh button.
        let f = ctx.cumulative_frame_nr();
        if f > self.last_frame + 1 {
            client.hw_dex_fetch(ctx);
        }
        self.last_frame = f;
        let report = client.snapshot().hw_dex.clone();
        let rep = match report {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("opening the dex…");
                });
                return;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("dex unavailable: {e}")).color(crate::RED));
                ui.label(RichText::new("needs a weft-cog-host with /hw/dex (rebuild + restart it).").color(GREY).small());
                return;
            }
            Some(Ok(r)) => r,
        };
        let (m, c) = (&rep.totals.modules, &rep.totals.chips);
        ui.horizontal_wrapped(|ui| {
            bar(ui, "modules", m.caught, m.total);
            bar(ui, "chips", c.caught, c.total);
            if ui.button("⟳").on_hover_text("refresh").clicked() {
                client.hw_dex_fetch(ctx);
            }
        });
        ui.label(RichText::new("Items are caught by a USB scan that matches them or by you registering a device. Use 🔍 Identify hardware.").color(GREY).small());
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for b in &rep.badges {
                let t = if b.earned { RichText::new(format!("★ {}", b.name)).strong().color(AMBER) } else { RichText::new(format!("🔒 {} {}/{}", b.name, b.have, b.need)).color(GREY).small() };
                ui.label(t).on_hover_text(&b.desc);
            }
        });
        if !rep.wild.is_empty() {
            egui::CollapsingHeader::new(RichText::new(format!("Wild species ({})", rep.wild.len())).small()).show(ui, |ui| {
                for w in &rep.wild {
                    ui.label(RichText::new(format!("{} {}:{} · seen {}x", w.name, w.vid, w.pid, w.times_seen)).small());
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.filter, 0, "all");
            ui.selectable_value(&mut self.filter, 1, "caught");
            ui.selectable_value(&mut self.filter, 2, "uncaught");
            ui.separator();
            for (i, r) in RARITIES.iter().enumerate() {
                ui.selectable_value(&mut self.rarity, i, *r);
            }
        });
        ui.separator();
        let all = entries(cat, &rep);
        let shown: Vec<&Entry> = all
            .iter()
            .filter(|e| match self.filter {
                1 => e.caught.is_some(),
                2 => e.caught.is_none(),
                _ => true,
            })
            .filter(|e| self.rarity == 0 || e.rarity.label() == RARITIES[self.rarity])
            .collect();
        ui.horizontal_wrapped(|ui| {
            for e in &shown {
                card(ui, e);
            }
        });
        ui.label(RichText::new(format!("{} of {} entries", shown.len(), all.len())).color(GREY).small());
    }
}

fn card(ui: &mut egui::Ui, e: &Entry) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(170.0);
        ui.set_min_height(78.0);
        match e.caught {
            Some(c) => {
                ui.label(RichText::new(format!("#{:03}", e.number)).monospace().color(GREY));
                ui.label(RichText::new(e.name).strong());
                ui.label(RichText::new(format!("{} · {}", e.rarity.label(), e.kind)).small().color(rarity_color(e.rarity.label())));
                ui.label(RichText::new(format!("caught {} on {}", ymd(c.first_caught_at), c.caught_on)).small().color(GREY));
                ui.label(RichText::new(format!("seen {}x · via {}", c.times_seen, c.via)).small().color(GREY));
            }
            None => {
                ui.label(RichText::new(format!("#{:03} ???", e.number)).monospace().color(GREY));
                ui.label(RichText::new("████████").color(Color32::from_gray(70)));
                ui.label(RichText::new(e.kind).small().color(GREY));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::ymd;

    #[test]
    fn dates_format() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(1_759_363_200), "2025-10-02");
        assert_eq!(ymd(1_835_000_000), "2028-02-24");
    }
}
