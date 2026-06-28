use eframe::egui;

use super::DemoState;

const TABS: &[(&str, &str)] = &[
    (
        "Overview",
        "Summary of the running kernel. 3 peers, 12 topics, uptime 4h13m.",
    ),
    (
        "Processes",
        "Mock process list — pid 0 (kernel), pid 1 (daemon), pid 42 (coder-agent).",
    ),
    (
        "Topics",
        "a2a.broadcast, mesh.leaf.abc.push, kernel.heartbeat, gate.decisions…",
    ),
    (
        "Logs",
        "[INFO] mesh listener on 0.0.0.0:9470\n[INFO] peer connected: leaf-abc",
    ),
];

pub fn show(ui: &mut egui::Ui, state: &mut DemoState) {
    ui.heading("Tabs");
    ui.label("Simple tab strip — selection persists in shared DemoState.");
    ui.separator();

    ui.horizontal(|ui| {
        for (i, (label, _)) in TABS.iter().enumerate() {
            if ui.selectable_label(state.tab_idx == i, *label).clicked() {
                state.tab_idx = i;
            }
        }
    });
    ui.separator();

    let (_, body) = TABS.get(state.tab_idx).copied().unwrap_or(("", ""));
    egui::Frame::new()
        .fill(egui::Color32::from_gray(22))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.label(body);
        });
}
