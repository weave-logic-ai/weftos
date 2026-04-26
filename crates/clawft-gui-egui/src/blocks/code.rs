use eframe::egui;

use crate::live::Snapshot;

pub fn show(ui: &mut egui::Ui, snap: &Snapshot) {
    ui.heading("Code");
    ui.label("Latest kernel.status response, pretty-printed JSON.");
    ui.separator();

    let text = match &snap.status {
        Some(v) => serde_json::to_string_pretty(v).unwrap_or_else(|e| format!("// serialize error: {e}")),
        None => "// daemon offline — no status yet".to_string(),
    };

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("kernel.status").weak().monospace());
        if ui.small_button("📋 copy").clicked() {
            ui.ctx().copy_text(text.clone());
        }
    });

    egui::Frame::new()
        .fill(egui::Color32::from_gray(18))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).monospace())
                            .selectable(true)
                            .wrap(),
                    );
                });
        });
}
