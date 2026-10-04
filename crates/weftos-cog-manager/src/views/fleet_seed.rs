//! The Seed detail panel: what a Cognitum Seed's own agent API reports (identity, status,
//! firmware slots, thermal). Every value is the device's own report over plain HTTP, so the whole
//! panel is labelled `self_reported`; nothing here is signed or verified by WeftOS.

use super::fleet::prov_pill;
use crate::fleet_unify::SeedView;
use crate::style;
use crate::{AMBER, GREEN, RED};
use eframe::egui::{self, RichText, Ui};
use serde_json::Value;

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(x) => x.clone(),
        other => other.to_string(),
    }
}

fn rows(ui: &mut Ui, salt: &str, rows: &[(&str, String)]) {
    egui::Grid::new(salt).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
        for (k, v) in rows {
            ui.label(style::dim(ui, *k));
            ui.label(style::body(v));
            ui.end_row();
        }
    });
}

fn uptime(secs: Option<u64>) -> String {
    match secs {
        Some(t) if t >= 86_400 => format!("{}d {}h", t / 86_400, (t % 86_400) / 3600),
        Some(t) if t >= 3600 => format!("{}h {}m", t / 3600, (t % 3600) / 60),
        Some(t) => format!("{}m", t / 60),
        None => "-".into(),
    }
}

pub(crate) fn seed_detail(ui: &mut Ui, sv: &SeedView) {
    ui.horizontal_wrapped(|ui| {
        ui.label(style::dim(ui, format!("agent at {}", sv.url)));
        prov_pill(ui, Some("self_reported"));
        if sv.auto {
            ui.label(style::dim(ui, "(found on the connected host)"));
        }
    });
    if let Some(e) = &sv.error {
        ui.colored_label(RED, format!("last read failed: {e}"));
    }
    let id = sv.identity.clone().unwrap_or(Value::Null);
    let st = sv.status.clone().unwrap_or(Value::Null);
    let fw = sv.firmware.clone().unwrap_or(Value::Null);
    let th = sv.thermal.clone().unwrap_or(Value::Null);

    style::subhead(ui, "Identity and status");
    let roles = st["roles"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_else(|| "-".into());
    rows(
        ui,
        "seed_id",
        &[
            ("device id", s(&id["device_id"])),
            ("paired", s(&st["paired"])),
            ("roles", roles),
            ("uptime", uptime(st["uptime_secs"].as_u64())),
            ("vectors", s(&st["total_vectors"])),
            ("witness chain", s(&st["witness_chain_length"])),
            ("epoch", s(&id["epoch"])),
        ],
    );

    style::subhead(ui, "Firmware");
    rows(
        ui,
        "seed_fw",
        &[
            ("version", s(&id["firmware_version"])),
            ("active slot", s(&fw["active_slot"])),
            ("last good slot", s(&fw["last_good_slot"])),
            ("inactive slot", s(&fw["inactive_slot"])),
            ("boot count", s(&fw["boot_count"])),
            ("dm-verity", s(&fw["dm_verity"])),
            ("build sha256", s(&id["build_sha256"])),
        ],
    );
    if fw["dm_verity"] == false {
        ui.label(RichText::new("dm-verity is off on this Seed").color(AMBER).small());
    }

    style::subhead(ui, "Thermal");
    let zone = s(&th["zone"]);
    rows(
        ui,
        "seed_th",
        &[
            ("temperature", th["temp_c"].as_f64().map_or_else(|| "-".into(), |t| format!("{t:.1} °C"))),
            ("zone", zone.clone()),
            ("cpu frequency", th["freq_mhz"].as_u64().map_or_else(|| "-".into(), |f| format!("{f} MHz"))),
            ("throttle flags", s(&th["throttle_flags"])),
            ("undervolt count", s(&th["undervolt_count"])),
        ],
    );
    if zone == "Cool" || zone == "Normal" {
        ui.label(RichText::new("thermal zone is fine").color(GREEN).small());
    }

    ui.add_space(style::GAP_S);
    egui::CollapsingHeader::new("Raw (all details)").id_salt(("seed_raw", &sv.url)).show(ui, |ui| {
        let all = serde_json::json!({ "identity": id, "status": st, "firmware": fw, "thermal": th });
        let text = serde_json::to_string_pretty(&all).unwrap_or_default();
        if ui.small_button("copy JSON").clicked() {
            ui.ctx().copy_text(text.clone());
        }
        egui::ScrollArea::vertical().max_height(320.0).id_salt(("seed_raw_scroll", &sv.url)).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).monospace().small()).selectable(true));
        });
    });
}
