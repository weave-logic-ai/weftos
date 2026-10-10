//! Bench self-tests drawn straight onto the `LeafSurface`, bypassing the
//! compositor: a display cycle that runs once at boot (colour bars, full
//! fields, 1-px border + grid, corner text, a sweep for tearing) and a
//! touch-target screen used while the leaf has no provisioned identity.
//!
//! Each phase logs its name so a camera capture can be matched to the
//! serial log by time. Added for the first hardware bring-up 2026-10-10;
//! the whole cycle costs ~17 s of boot and is cheap insurance against
//! shipping a panel that only "looks right" in one corner.

use std::time::{Duration, Instant};

use embedded_graphics::mono_font::{ascii::FONT_10X20, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use log::info;
use weftos_leaf_display::LeafSurface;
use weftos_leaf_scene::InputEvent;

use crate::board::{SCREEN_HEIGHT, SCREEN_WIDTH};
use crate::display::{DisplayError, DpiDisplay};

const W: i32 = SCREEN_WIDTH as i32;
const H: i32 = SCREEN_HEIGHT as i32;
const BLACK: Rgb888 = Rgb888::new(0, 0, 0);
const WHITE: Rgb888 = Rgb888::new(255, 255, 255);
const RED: Rgb888 = Rgb888::new(255, 0, 0);
const GREEN: Rgb888 = Rgb888::new(0, 255, 0);
const BLUE: Rgb888 = Rgb888::new(0, 0, 255);
const GREY: Rgb888 = Rgb888::new(96, 96, 96);
const CYAN: Rgb888 = Rgb888::new(0, 255, 255);

type R = Result<(), DisplayError>;

fn full_rect() -> Rectangle {
    Rectangle::new(Point::zero(), Size::new(W as u32, H as u32))
}

fn fill(surface: &mut DpiDisplay, color: Rgb888) -> R {
    let mut f = surface.frame();
    full_rect().into_styled(PrimitiveStyle::with_fill(color)).draw(&mut f)?;
    drop(f);
    surface.present()
}

fn hold(secs: u64) {
    std::thread::sleep(Duration::from_secs(secs));
}

/// Five vertical bars, 160 px each: R, G, B, white, black.
fn colour_bars(surface: &mut DpiDisplay) -> R {
    let mut f = surface.frame();
    for (i, c) in [RED, GREEN, BLUE, WHITE, BLACK].iter().enumerate() {
        Rectangle::new(Point::new(i as i32 * 160, 0), Size::new(160, H as u32))
            .into_styled(PrimitiveStyle::with_fill(*c))
            .draw(&mut f)?;
    }
    drop(f);
    surface.present()
}

/// 1-px white border on the outermost pixels plus a grid: grey every
/// 50 px, white every 100 px. Any horizontal shift or wrap shows as a
/// missing or doubled border edge; any vertical shift as a missing
/// top/bottom line.
fn edge_grid(surface: &mut DpiDisplay) -> R {
    let mut f = surface.frame();
    full_rect().into_styled(PrimitiveStyle::with_fill(BLACK)).draw(&mut f)?;
    let mut x = 50;
    while x < W {
        let c = if x % 100 == 0 { WHITE } else { GREY };
        Line::new(Point::new(x, 0), Point::new(x, H - 1)).into_styled(PrimitiveStyle::with_stroke(c, 1)).draw(&mut f)?;
        x += 50;
    }
    let mut y = 50;
    while y < H {
        let c = if y % 100 == 0 { WHITE } else { GREY };
        Line::new(Point::new(0, y), Point::new(W - 1, y)).into_styled(PrimitiveStyle::with_stroke(c, 1)).draw(&mut f)?;
        y += 50;
    }
    // Border last so it is never overdrawn.
    Rectangle::new(Point::zero(), Size::new(W as u32, H as u32))
        .into_styled(PrimitiveStyle::with_stroke(WHITE, 1))
        .draw(&mut f)?;
    drop(f);
    surface.present()
}

/// Text flush against all four corners plus a centred label and cross.
/// FONT_10X20 is 10 px wide; `Text::new` takes the baseline, so the top
/// row sits at y = 16 and the bottom row at y = H - 5.
fn corner_text(surface: &mut DpiDisplay) -> R {
    let mut f = surface.frame();
    full_rect().into_styled(PrimitiveStyle::with_fill(BLACK)).draw(&mut f)?;
    let style = MonoTextStyle::new(&FONT_10X20, WHITE);
    let cyan = MonoTextStyle::new(&FONT_10X20, CYAN);
    let tl = "TL 0,0";
    let tr = "TR 799,0";
    let bl = "BL 0,479";
    let br = "BR 799,479";
    Text::new(tl, Point::new(0, 16), style).draw(&mut f)?;
    Text::new(tr, Point::new(W - 10 * tr.len() as i32, 16), style).draw(&mut f)?;
    Text::new(bl, Point::new(0, H - 5), style).draw(&mut f)?;
    Text::new(br, Point::new(W - 10 * br.len() as i32, H - 5), style).draw(&mut f)?;
    let mid = "clawft-edge-pad-idf selftest 800x480";
    Text::new(mid, Point::new(W / 2 - 5 * mid.len() as i32, H / 2 - 30), cyan).draw(&mut f)?;
    Line::new(Point::new(W / 2 - 40, H / 2), Point::new(W / 2 + 40, H / 2)).into_styled(PrimitiveStyle::with_stroke(WHITE, 1)).draw(&mut f)?;
    Line::new(Point::new(W / 2, H / 2 - 40), Point::new(W / 2, H / 2 + 40)).into_styled(PrimitiveStyle::with_stroke(WHITE, 1)).draw(&mut f)?;
    drop(f);
    surface.present()
}

/// A 24-px white bar sweeping left to right for `secs`; tearing shows as
/// a broken bar on camera. Every frame is a full repaint (the surface is
/// double-buffered, so the back buffer is two frames stale) and
/// `present` waits for VSYNC, so the logged rate is the real flip rate.
fn sweep(surface: &mut DpiDisplay, secs: u64) -> R {
    let start = Instant::now();
    let mut frames = 0u32;
    while start.elapsed() < Duration::from_secs(secs) {
        let x = ((start.elapsed().as_millis() as i64 / 4) % (W as i64 - 24)) as i32;
        let mut f = surface.frame();
        f.clear(BLACK)?;
        Rectangle::new(Point::new(x, 0), Size::new(24, H as u32)).into_styled(PrimitiveStyle::with_fill(WHITE)).draw(&mut f)?;
        drop(f);
        surface.present()?;
        frames += 1;
    }
    info!("[selftest] sweep: {frames} frames in {secs} s ({:.1} fps)", frames as f32 / secs as f32);
    Ok(())
}

/// The boot-time display cycle. ~17 s.
pub fn run_display_cycle(surface: &mut DpiDisplay) -> R {
    info!("[selftest] colour bars R/G/B/white/black (3 s)");
    colour_bars(surface)?;
    hold(3);
    for (name, c) in [("red", RED), ("green", GREEN), ("blue", BLUE), ("white", WHITE), ("black", BLACK)] {
        info!("[selftest] full field {name} (1 s)");
        fill(surface, c)?;
        hold(1);
    }
    info!("[selftest] 1-px border + 50 px grid (3 s)");
    edge_grid(surface)?;
    hold(3);
    info!("[selftest] corner text (3 s)");
    corner_text(surface)?;
    hold(3);
    info!("[selftest] sweep (3 s)");
    sweep(surface, 3)?;
    info!("[selftest] display cycle done");
    Ok(())
}

const TARGET_R: i32 = 45;
const DRAG_Y: i32 = 330;
const DRAG_X0: i32 = 120;
const DRAG_SEG: i32 = 70;
const DRAG_SEGS: usize = 8;

/// Owner-run touch test: five targets (centre + corners) and a drag line
/// of eight segments. Each turns green when the pointer hits it; the
/// caller logs the raw events. Everything is proven when all are green.
pub struct TouchTargets {
    targets: [(Point, bool); 5],
    drag: [bool; DRAG_SEGS],
    last: Option<(&'static str, i32, i32)>,
}

impl TouchTargets {
    pub fn new() -> Self {
        let p = |x, y| (Point::new(x, y), false);
        Self {
            targets: [p(W / 2, H / 2), p(60, 60), p(W - 60, 60), p(60, H - 60), p(W - 60, H - 60)],
            drag: [false; DRAG_SEGS],
            last: None,
        }
    }

    pub fn all_hit(&self) -> bool {
        self.targets.iter().all(|t| t.1) && self.drag.iter().all(|s| *s)
    }

    /// Feed one event (Q24.8 coordinates). Returns true when the screen
    /// needs redrawing.
    pub fn feed(&mut self, event: InputEvent) -> bool {
        let (kind, x, y) = match event {
            InputEvent::PointerDown { x, y, .. } => ("down", x >> 8, y >> 8),
            InputEvent::PointerMove { x, y, .. } => ("move", x >> 8, y >> 8),
            InputEvent::PointerUp { x, y, .. } => ("up", x >> 8, y >> 8),
            _ => return false,
        };
        self.last = Some((kind, x, y));
        for (i, (c, hit)) in self.targets.iter_mut().enumerate() {
            if !*hit && (c.x - x).pow(2) + (c.y - y).pow(2) <= (TARGET_R + 12).pow(2) {
                *hit = true;
                info!("[selftest] target {i} hit at x={x} y={y}");
            }
        }
        if (y - DRAG_Y).abs() <= 40 {
            let i = (x - DRAG_X0) / DRAG_SEG;
            if x >= DRAG_X0 && (i as usize) < DRAG_SEGS && !self.drag[i as usize] {
                self.drag[i as usize] = true;
                info!("[selftest] drag segment {i} hit at x={x} y={y}");
            }
        }
        true
    }

    pub fn draw(&self, surface: &mut DpiDisplay, header: &str) -> R {
        let mut f = surface.frame();
        full_rect().into_styled(PrimitiveStyle::with_fill(BLACK)).draw(&mut f)?;
        let style = MonoTextStyle::new(&FONT_10X20, WHITE);
        let cyan = MonoTextStyle::new(&FONT_10X20, CYAN);
        Text::new(header, Point::new(120, 30), style).draw(&mut f)?;
        Text::new("touch test: tap the 5 circles, drag along the bar", Point::new(120, 56), cyan).draw(&mut f)?;
        for (c, hit) in &self.targets {
            let d = (TARGET_R * 2) as u32;
            let top_left = Point::new(c.x - TARGET_R, c.y - TARGET_R);
            let st = if *hit { PrimitiveStyle::with_fill(GREEN) } else { PrimitiveStyle::with_stroke(RED, 3) };
            Circle::new(top_left, d).into_styled(st).draw(&mut f)?;
        }
        for (i, hit) in self.drag.iter().enumerate() {
            let x = DRAG_X0 + i as i32 * DRAG_SEG;
            let st = if *hit { PrimitiveStyle::with_fill(GREEN) } else { PrimitiveStyle::with_stroke(RED, 2) };
            Rectangle::new(Point::new(x, DRAG_Y - 8), Size::new(DRAG_SEG as u32 - 4, 16)).into_styled(st).draw(&mut f)?;
        }
        let status = match self.last {
            Some((k, x, y)) => format!("last: {k} x={x} y={y}"),
            None => String::from("last: (no touch yet)"),
        };
        Text::new(&status, Point::new(120, 400), cyan).draw(&mut f)?;
        if self.all_hit() {
            Text::new("ALL TARGETS HIT -- touch verified", Point::new(120, 426), MonoTextStyle::new(&FONT_10X20, GREEN)).draw(&mut f)?;
        }
        drop(f);
        surface.present()
    }
}
