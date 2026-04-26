use eframe::egui;

pub fn show(ui: &mut egui::Ui) {
    ui.heading("Layout — Row / Column / Grid");
    ui.label("The three layout primitives from the web build, mapped to egui.");
    ui.separator();

    ui.label(egui::RichText::new("Row").strong());
    ui.horizontal(|ui| {
        for i in 0..4 {
            cell(ui, &format!("row-{i}"));
        }
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("Column").strong());
    ui.vertical(|ui| {
        for i in 0..3 {
            cell(ui, &format!("col-{i}"));
        }
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("Grid (3×3)").strong());
    egui::Grid::new("layout_grid")
        .num_columns(3)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            for r in 0..3 {
                for c in 0..3 {
                    cell(ui, &format!("{r},{c}"));
                }
                ui.end_row();
            }
        });
}

fn cell(ui: &mut egui::Ui, label: &str) {
    egui::Frame::new()
        .fill(egui::Color32::from_gray(30))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(70.0, 32.0));
            ui.label(egui::RichText::new(label).monospace().small());
        });
}
