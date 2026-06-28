use eframe::egui;

use super::DemoState;

pub fn show(ui: &mut egui::Ui, state: &mut DemoState) {
    ui.heading("Button");
    ui.label("Variants + disabled + a click counter wired to shared state.");
    ui.separator();

    ui.horizontal(|ui| {
        if ui.button("Primary").clicked() {
            state.counter += 1;
        }
        if ui
            .add(egui::Button::new("Secondary").fill(egui::Color32::from_gray(40)))
            .clicked()
        {
            state.counter += 1;
        }
        if ui.add(egui::Button::new("Ghost").frame(false)).clicked() {
            state.counter += 1;
        }
        ui.add_enabled(false, egui::Button::new("Disabled"));
    });

    ui.add_space(10.0);
    ui.label(format!("Clicks: {}", state.counter));
    if ui.small_button("reset").clicked() {
        state.counter = 0;
    }
}
