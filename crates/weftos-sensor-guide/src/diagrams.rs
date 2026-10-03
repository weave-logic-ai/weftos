//! Diagrams painted from `guide.toml` data: the 40-pin header with used and forbidden pins,
//! a parts-and-wires wiring diagram, body pad placements, and a left-to-right signal flow.
//! Painted with egui shapes so they look the same natively and in the browser.

use crate::bundle::{GuideDoc, Part};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

pub fn color(name: &str) -> Color32 {
    match name.to_ascii_lowercase().as_str() {
        "red" => Color32::from_rgb(220, 60, 60),
        "black" => Color32::from_rgb(150, 150, 150), // drawn light so it shows on dark themes
        "yellow" => Color32::from_rgb(240, 190, 40),
        "green" => Color32::from_rgb(60, 170, 90),
        "blue" => Color32::from_rgb(60, 130, 230),
        "orange" => Color32::from_rgb(240, 140, 40),
        "white" => Color32::from_rgb(235, 235, 235),
        "purple" => Color32::from_rgb(160, 100, 220),
        _ => Color32::from_rgb(120, 200, 220),
    }
}

const PANEL: Color32 = Color32::from_rgb(34, 36, 40);
const EDGE: Color32 = Color32::from_rgb(90, 95, 105);
const TEXT: Color32 = Color32::from_rgb(225, 228, 232);
const DIM: Color32 = Color32::from_rgb(150, 155, 162);
const BAD: Color32 = Color32::from_rgb(220, 70, 70);

// One type scale for every painted diagram, so titles, labels, pin numbers and detail text
// are sized consistently across header / wiring / flow / grid rather than ad-hoc per call.
const F_TITLE: f32 = 14.0; // board / panel title
const F_NAME: f32 = 13.0; // part name, legend name, grid title
const F_LABEL: f32 = 12.0; // primary labels and pin numbers (mono)
const F_AXIS: f32 = 11.0; // orientation / axis / note labels
const F_TINY: f32 = 10.0; // dense labels (wire labels, header row labels, pad labels)
const F_DETAIL: f32 = 9.5; // secondary detail text inside boxes
const F_CELL: f32 = 9.0; // grid cell index (mono)

/// Standard inset from a canvas edge to its text, so every diagram's margins match.
const MARGIN: f32 = 14.0;

/// Paints `text` wrapped to `width`; returns the height used.
fn wrapped(
    p: &egui::Painter,
    pos: egui::Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> f32 {
    let galley = p.layout(text.to_string(), font, color, width.max(40.0));
    let h = galley.size().y;
    p.galley(pos, galley, color);
    h
}

fn canvas(ui: &mut Ui, h: f32) -> (egui::Painter, Rect) {
    let w = ui.available_width().min(980.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, Color32::from_rgb(24, 25, 28));
    (p, rect)
}

/// Raspberry Pi 40-pin header, pin 1 top-left; odd pins on the inner (top) row.
pub fn header(ui: &mut Ui, doc: &GuideDoc) {
    let Some(h) = &doc.header else { return };
    let rows = h.used.len() + h.avoid.len();
    let (p, r) = canvas(ui, 118.0 + rows as f32 * 21.0);
    let pitch = ((r.width() - 60.0) / 20.0).min(40.0);
    let x0 = r.left() + 30.0 + pitch / 2.0;
    // Header along the top edge, ports along the bottom: the OUTER row (nearer the board edge) holds
    // the even pins 2-40 and is drawn on top; the INNER row (nearer the chip) holds the odd pins
    // 1-39 and is drawn below. Pin 1 (3.3 V) is the inner row's left pin. Matches the orientation text.
    let (y_even, y_odd) = (r.top() + 70.0, r.top() + 70.0 + pitch);
    p.text(
        pos2(r.left() + MARGIN, r.top() + 12.0),
        Align2::LEFT_TOP,
        &h.board,
        FontId::proportional(F_TITLE),
        TEXT,
    );
    p.text(
        pos2(r.left() + MARGIN, r.top() + 32.0),
        Align2::LEFT_TOP,
        &h.orientation,
        FontId::proportional(F_AXIS),
        DIM,
    );
    p.rect_stroke(
        Rect::from_min_max(
            pos2(x0 - pitch * 0.7, y_even - pitch * 0.7),
            pos2(x0 + pitch * 19.7, y_odd + pitch * 0.7),
        ),
        4.0,
        Stroke::new(1.0, EDGE),
        egui::StrokeKind::Middle,
    );
    // Row labels so the orientation is unambiguous regardless of how the board is held.
    p.text(
        pos2(x0 - pitch * 0.7, y_even - pitch * 0.95),
        Align2::LEFT_BOTTOM,
        "outer row · even 2-40 (board edge)",
        FontId::proportional(F_TINY),
        DIM,
    );
    p.text(
        pos2(x0 - pitch * 0.7, y_odd + pitch * 0.95),
        Align2::LEFT_TOP,
        "inner row · odd 1-39 (nearer the chip) — pin 1 = 3V3 at left",
        FontId::proportional(F_TINY),
        DIM,
    );
    for pin in 1..=40u8 {
        let col = f32::from((pin - 1) / 2);
        let pos = pos2(x0 + col * pitch, if pin % 2 == 1 { y_odd } else { y_even });
        let used = h.used.iter().find(|u| u.pin == pin);
        let avoid = h.avoid.iter().any(|a| a.pin == pin);
        let (fill, rad) = match (used, avoid) {
            (Some(u), _) => (color(&u.color), pitch * 0.32),
            (None, true) => (BAD, pitch * 0.22),
            _ => (Color32::from_gray(70), pitch * 0.18),
        };
        p.circle_filled(pos, rad, fill);
        if pin == 1 {
            // Ring pin 1 so the corner is obvious.
            p.circle_stroke(pos, rad + 3.0, Stroke::new(2.0, Color32::from_rgb(240, 190, 40)));
        }
        if avoid {
            p.line_segment(
                [pos - vec2(rad, rad) * 1.3, pos + vec2(rad, rad) * 1.3],
                Stroke::new(2.0, BAD),
            );
        }
        if used.is_some() || avoid || pin <= 2 {
            // Odd pins are on the lower (inner) row → number below; even on the upper row → above.
            let dy = if pin % 2 == 1 {
                pitch * 0.62
            } else {
                -pitch * 0.62
            };
            p.text(
                pos + vec2(0.0, dy),
                Align2::CENTER_CENTER,
                pin.to_string(),
                FontId::monospace(F_TINY),
                TEXT,
            );
        }
    }
    let mut y = y_even + pitch * 1.1;
    for u in &h.used {
        p.circle_filled(pos2(r.left() + 20.0, y + 6.0), 5.0, color(&u.color));
        let text = format!("pin {}  {}  ->  {}", u.pin, u.name, u.to);
        y += wrapped(
            &p,
            pos2(r.left() + 32.0, y),
            &text,
            FontId::proportional(F_LABEL),
            TEXT,
            r.width() - 46.0,
        ) + 3.0;
    }
    for a in &h.avoid {
        let text = format!("x  pin {}: never use. {}", a.pin, a.why);
        y += wrapped(
            &p,
            pos2(r.left() + MARGIN, y),
            &text,
            FontId::proportional(F_AXIS),
            BAD,
            r.width() - 28.0,
        ) + 3.0;
    }
}

fn pin_pos(parts: &[Part], rects: &[Rect], endpoint: &str) -> Option<(egui::Pos2, usize)> {
    let (pid, pin) = endpoint.split_once('.')?;
    let i = parts.iter().position(|p| p.id == pid)?;
    let k = parts[i].pins.iter().position(|x| x == pin)?;
    let r = rects[i];
    Some((pos2(r.center().x, r.top() + 48.0 + k as f32 * 22.0), i))
}

/// Parts as boxes left to right, wires between their pins.
pub fn wiring(ui: &mut Ui, doc: &GuideDoc) {
    let parts = &doc.parts;
    if parts.is_empty() {
        return;
    }
    let max_pins = parts.iter().map(|p| p.pins.len()).max().unwrap_or(1) as f32;
    let (p, r) = canvas(ui, 70.0 + max_pins * 22.0 + 40.0);
    let n = parts.len() as f32;
    let measure = |s: &str, size: f32| {
        ui.painter()
            .layout_no_wrap(s.to_string(), FontId::proportional(size), TEXT)
            .size()
            .x
    };
    let text_w = parts
        .iter()
        .map(|p| measure(&p.name, 13.0).max(measure(&p.detail, 10.0)))
        .fold(110.0_f32, f32::max)
        + 20.0;
    let box_w = text_w.min((r.width() - 40.0) / n * 0.6);
    let gap = (r.width() - 40.0 - box_w * n) / (n - 1.0).max(1.0);
    let rects: Vec<Rect> = parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let x = r.left() + 20.0 + i as f32 * (box_w + gap);
            Rect::from_min_size(
                pos2(x, r.top() + 14.0),
                vec2(box_w, 44.0 + part.pins.len() as f32 * 22.0),
            )
        })
        .collect();
    // Wires first, under the boxes' labels. Wires crossing the same gap get their own lanes.
    let gap_of = |w: &crate::bundle::Wire| {
        let a = pin_pos(parts, &rects, &w.from).map(|x| x.1);
        let b = pin_pos(parts, &rects, &w.to).map(|x| x.1);
        a.zip(b).map(|(a, b)| (a.min(b), a.max(b)))
    };
    for (wi, w) in doc.wires.iter().enumerate() {
        let lane_mates: Vec<usize> = doc
            .wires
            .iter()
            .enumerate()
            .filter(|(_, o)| gap_of(o) == gap_of(w))
            .map(|(i, _)| i)
            .collect();
        let lane = lane_mates.iter().position(|&i| i == wi).unwrap_or(0) as f32;
        let lanes = lane_mates.len() as f32;
        let (Some((a, ia)), Some((b, ib))) = (
            pin_pos(parts, &rects, &w.from),
            pin_pos(parts, &rects, &w.to),
        ) else {
            continue;
        };
        let c = color(&w.color);
        let stroke = Stroke::new(3.0, c);
        if ia == ib {
            // A jumper on the same part: loop out to the left side.
            let x = rects[ia].left() - 14.0;
            p.line(
                vec![
                    a,
                    pos2(rects[ia].left(), a.y),
                    pos2(x, a.y),
                    pos2(x, b.y),
                    pos2(rects[ia].left(), b.y),
                ],
                Stroke::new(2.0, c),
            );
            p.text(
                pos2(x - 4.0, (a.y + b.y) / 2.0),
                Align2::RIGHT_CENTER,
                &w.label,
                FontId::proportional(F_TINY),
                DIM,
            );
            continue;
        }
        let (sa, sb) = if ia < ib {
            (rects[ia].right(), rects[ib].left())
        } else {
            (rects[ia].left(), rects[ib].right())
        };
        let (pa, pb) = (pos2(sa, a.y), pos2(sb, b.y));
        let span = (pb.x - pa.x).abs();
        let mid =
            (pa.x + pb.x) / 2.0 + (lane - (lanes - 1.0) / 2.0) * (span / (lanes + 1.0)).min(14.0);
        p.line(vec![pa, pos2(mid, pa.y), pos2(mid, pb.y), pb], stroke);
        let (lx, align) = if pa.x < pb.x {
            (pa.x + 6.0, Align2::LEFT_BOTTOM)
        } else {
            (pa.x - 6.0, Align2::RIGHT_BOTTOM)
        };
        p.text(
            pos2(lx, pa.y - 2.0),
            align,
            &w.label,
            FontId::proportional(F_TINY),
            c,
        );
    }
    for (part, rect) in parts.iter().zip(&rects) {
        p.rect_filled(*rect, 6.0, PANEL);
        p.rect_stroke(*rect, 6.0, Stroke::new(1.5, EDGE), egui::StrokeKind::Middle);
        let wide = p
            .layout_no_wrap(part.name.clone(), FontId::proportional(F_NAME), TEXT)
            .size()
            .x
            > rect.width() - 8.0;
        let name_font = FontId::proportional(if wide { 10.5 } else { F_NAME });
        p.text(
            pos2(rect.center().x, rect.top() + 8.0),
            Align2::CENTER_TOP,
            &part.name,
            name_font,
            TEXT,
        );
        let detail = p.layout(
            part.detail.clone(),
            FontId::proportional(F_DETAIL),
            DIM,
            rect.width() - 6.0,
        );
        p.galley(
            pos2(rect.center().x - detail.size().x / 2.0, rect.top() + 24.0),
            detail,
            DIM,
        );
        for (k, pin) in part.pins.iter().enumerate() {
            let y = rect.top() + 48.0 + k as f32 * 22.0;
            p.text(
                pos2(rect.center().x, y),
                Align2::CENTER_CENTER,
                pin,
                FontId::monospace(F_LABEL),
                TEXT,
            );
        }
    }
}

/// A torso (front view) with the selected placement's pads; returns nothing, mutates `selected`.
pub fn placements(ui: &mut Ui, doc: &GuideDoc, selected: &mut usize) {
    if doc.placements.is_empty() {
        return;
    }
    *selected = (*selected).min(doc.placements.len() - 1);
    ui.horizontal_wrapped(|ui| {
        for (i, pl) in doc.placements.iter().enumerate() {
            ui.selectable_value(selected, i, &pl.name);
        }
    });
    let pl = &doc.placements[*selected];
    if !pl.when.is_empty() {
        ui.label(egui::RichText::new(&pl.when).italics());
    }
    let (p, r) = canvas(ui, 380.0);
    let fig = Rect::from_center_size(
        pos2(r.left() + r.width() * 0.33, r.center().y + 8.0),
        vec2(240.0, 340.0),
    );
    let at = |x: f32, y: f32| pos2(fig.left() + x * fig.width(), fig.top() + y * fig.height());
    let skin = Color32::from_rgb(70, 62, 58);
    // Head, torso, arms, legs.
    p.circle_filled(at(0.5, 0.06), 22.0, skin);
    let torso = vec![
        at(0.30, 0.15),
        at(0.70, 0.15),
        at(0.74, 0.30),
        at(0.68, 0.78),
        at(0.32, 0.78),
        at(0.26, 0.30),
    ];
    p.add(egui::Shape::convex_polygon(torso, skin, Stroke::NONE));
    for (a, b) in [
        ((0.30, 0.17), (0.10, 0.62)),
        ((0.70, 0.17), (0.90, 0.62)),
        ((0.40, 0.78), (0.38, 0.98)),
        ((0.60, 0.78), (0.62, 0.98)),
    ] {
        p.line_segment([at(a.0, a.1), at(b.0, b.1)], Stroke::new(16.0, skin));
    }
    // Heart, slightly to the subject's left (the viewer's right).
    p.circle_filled(at(0.56, 0.40), 11.0, Color32::from_rgb(170, 70, 80));
    p.text(
        at(0.5, 1.02),
        Align2::CENTER_TOP,
        "<- subject's right      subject's left ->",
        FontId::proportional(F_TINY),
        DIM,
    );
    let pad_col = |l: &str| match l {
        "RA" => Color32::from_rgb(230, 230, 230),
        "LA" => Color32::from_rgb(60, 130, 230),
        "RL" => Color32::from_rgb(60, 170, 90),
        _ => Color32::from_rgb(240, 190, 40),
    };
    let mut ly = r.top() + 40.0;
    let lx = r.left() + r.width() * 0.58;
    for pad in &pl.pads {
        let c = at(pad.x, pad.y);
        p.circle_filled(c, 11.0, pad_col(&pad.label));
        p.circle_stroke(c, 11.0, Stroke::new(1.5, Color32::BLACK));
        p.text(
            c,
            Align2::CENTER_CENTER,
            &pad.label,
            FontId::proportional(F_TINY),
            Color32::BLACK,
        );
        p.circle_filled(pos2(lx, ly + 8.0), 8.0, pad_col(&pad.label));
        let text = format!("{}: {}", pad.label, pad.note);
        let h = wrapped(
            &p,
            pos2(lx + 16.0, ly),
            &text,
            FontId::proportional(F_NAME),
            TEXT,
            r.right() - lx - 24.0,
        );
        ly += h.max(18.0) + 12.0;
    }
}

/// Signal flow as connected boxes.
pub fn flow(ui: &mut Ui, doc: &GuideDoc) {
    let steps = &doc.flow;
    if steps.is_empty() {
        return;
    }
    let (p, r) = canvas(ui, 92.0);
    let n = steps.len() as f32;
    let gap = 18.0;
    let w = (r.width() - 24.0 - gap * (n - 1.0)) / n;
    for (i, s) in steps.iter().enumerate() {
        let x = r.left() + 12.0 + i as f32 * (w + gap);
        let b = Rect::from_min_size(pos2(x, r.top() + 16.0), vec2(w, 60.0));
        p.rect_filled(b, 6.0, PANEL);
        p.rect_stroke(b, 6.0, Stroke::new(1.0, EDGE), egui::StrokeKind::Middle);
        p.text(
            pos2(b.center().x, b.top() + 10.0),
            Align2::CENTER_TOP,
            &s.name,
            FontId::proportional(F_LABEL),
            TEXT,
        );
        let detail = p.layout(
            s.detail.clone(),
            FontId::proportional(F_DETAIL),
            DIM,
            b.width() - 8.0,
        );
        p.galley(
            pos2(b.center().x - detail.size().x / 2.0, b.top() + 28.0),
            detail,
            DIM,
        );
        if i + 1 < steps.len() {
            let (a, c) = (
                pos2(b.right() + 2.0, b.center().y),
                pos2(b.right() + gap - 2.0, b.center().y),
            );
            p.arrow(a, c - a, Stroke::new(1.5, DIM));
        }
    }
}

/// A rows x cols zone map with orientation labels and the row-major index in each cell.
pub fn grid(ui: &mut Ui, doc: &GuideDoc) {
    let Some(g) = &doc.grid else { return };
    let (rows, cols) = (g.rows.max(1) as f32, g.cols.max(1) as f32);
    let cell = ((ui.available_width().min(980.0) - 160.0) / cols).clamp(18.0, 44.0);
    let (p, r) = canvas(ui, 70.0 + cell * rows + 60.0);
    p.text(
        pos2(r.left() + MARGIN, r.top() + 10.0),
        Align2::LEFT_TOP,
        &g.title,
        FontId::proportional(F_NAME),
        TEXT,
    );
    let origin = pos2(r.center().x - cell * cols / 2.0, r.top() + 50.0);
    for y in 0..g.rows {
        for x in 0..g.cols {
            let cr = Rect::from_min_size(
                origin + vec2(x as f32 * cell, y as f32 * cell),
                vec2(cell - 2.0, cell - 2.0),
            );
            let shade = 40 + ((x + y) % 2) as u8 * 10;
            p.rect_filled(cr, 2.0, Color32::from_gray(shade));
            if cell >= 24.0 {
                p.text(
                    cr.center(),
                    Align2::CENTER_CENTER,
                    (y * g.cols + x).to_string(),
                    FontId::monospace(F_CELL),
                    DIM,
                );
            }
        }
    }
    let gw = cell * cols;
    let gh = cell * rows;
    let corner = Rect::from_min_size(origin, vec2(cell - 2.0, cell - 2.0));
    p.rect_stroke(
        corner,
        2.0,
        Stroke::new(2.0, Color32::from_rgb(240, 190, 40)),
        egui::StrokeKind::Middle,
    );
    p.text(
        pos2(origin.x + gw / 2.0, origin.y - 6.0),
        Align2::CENTER_BOTTOM,
        &g.top,
        FontId::proportional(F_AXIS),
        TEXT,
    );
    p.text(
        pos2(origin.x + gw / 2.0, origin.y + gh + 4.0),
        Align2::CENTER_TOP,
        &g.bottom,
        FontId::proportional(F_AXIS),
        TEXT,
    );
    p.text(
        pos2(origin.x - 8.0, origin.y + gh / 2.0),
        Align2::RIGHT_CENTER,
        &g.left,
        FontId::proportional(F_AXIS),
        TEXT,
    );
    p.text(
        pos2(origin.x + gw + 8.0, origin.y + gh / 2.0),
        Align2::LEFT_CENTER,
        &g.right,
        FontId::proportional(F_AXIS),
        TEXT,
    );
    if !g.note.is_empty() {
        wrapped(
            &p,
            pos2(r.left() + 14.0, origin.y + gh + 22.0),
            &g.note,
            FontId::proportional(F_AXIS),
            DIM,
            r.width() - 28.0,
        );
    }
}
