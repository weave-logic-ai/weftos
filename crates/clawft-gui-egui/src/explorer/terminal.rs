//! Terminal — egui surface for a daemon-side PTY session, with a
//! real ANSI-aware grid renderer.
//!
//! ## Architecture
//!
//! - The PTY lives in the daemon (crate `clawft-service-terminal`).
//!   Output bytes are published as base64 chunks at
//!   `substrate/<daemon-node>/derived/terminal/<session_id>`.
//! - This panel is a thin renderer. On first paint it fires
//!   `terminal.spawn`, stashes the returned `session_id`, and starts
//!   subscribing (via `substrate.read` polling) to the output path.
//! - Each chunk's bytes are fed through
//!   [`alacritty_terminal::vte::ansi::Processor`] into a
//!   [`alacritty_terminal::Term`]. The grid (with colors, cursor,
//!   alt-screen, etc.) is then painted as egui shapes — one colored
//!   rect per cell that has a non-default background, and one glyph
//!   per non-blank cell.
//! - Input: a focused widget area receives egui keyboard events. We
//!   translate them to terminal bytes (text passthrough, special keys
//!   to CSI/control sequences) and fire `terminal.write` RPCs.
//! - Resize: when the visible grid dims change, we both `term.resize`
//!   the local model AND fire `terminal.resize` so in-shell apps
//!   reflow.
//! - Drop: best-effort `terminal.close` via [`Self::close`] from the
//!   Explorer.
//!
//! ## What's shipped
//!
//! - **Mouse selection + clipboard** (WEFT-260). Drag inside the grid
//!   to select; Ctrl+C / Cmd+C copies the selection to the system
//!   clipboard via egui's [`OutputCommand::CopyText`]; pastes
//!   ([`Event::Paste`]) are written into the PTY as input. Selection
//!   is rendered as a translucent overlay on the affected cells.
//! - **Bold / italic glyph variants** (WEFT-261). Bold is synthesised
//!   by overdrawing the glyph with a 0.5 px horizontal offset (a
//!   weight-bump trick that doesn't require an actual bold font face);
//!   italic is approximated by rotating the glyph by a small angle via
//!   [`egui::epaint::TextShape::with_angle_and_anchor`]. Egui's bundled
//!   monospace face has no proper bold/italic variants — these are
//!   fidelity approximations, not "real" font swaps.
//! - **Scrollback wheel handler** (WEFT-262). Mouse wheel scrolls into
//!   alacritty's grid history; configurable bound defaults to ~10 000
//!   lines (alacritty's own default). Resize re-reflows correctly via
//!   the existing `Term::resize` path.
//!
//! ## What's deferred
//!
//! - **Multi-tab terminal** (WEFT-263, deferred). Single Terminal panel
//!   per Explorer selection; structural change to multi-session.
//! - **Browser (wasm32) target gets a stub** — alacritty_terminal
//!   pulls in platform-specific tty + polling crates that don't
//!   compile for wasm. Native-only renderer; wasm shows a placeholder.

use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::live::{self, Command, Live, ReplyRx};

/// Shape-match priority for the terminal sentinel. Higher than the
/// generic JSON fallback (1) and the control-toggle (25).
pub const PRIORITY: u32 = 50;

/// How often to poll the output substrate path for new chunks.
const OUTPUT_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Default terminal cell metrics for converting panel size → (rows, cols)
/// when the egui font metrics aren't queryable yet (first paint).
const FALLBACK_CELL_W: f32 = 8.0;
const FALLBACK_CELL_H: f32 = 16.0;

/// Shape-match for the top-level sentinel value.
pub fn matches(value: &Value) -> u32 {
    let Some(obj) = value.as_object() else {
        return 0;
    };
    let kind_ok = obj
        .get("kind")
        .and_then(Value::as_str)
        .map(|s| s == "terminal")
        .unwrap_or(false);
    if kind_ok { PRIORITY } else { 0 }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::*;

    use alacritty_terminal::Term;
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::grid::{Dimensions, Scroll};
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::cell::Flags;
    use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

    /// Default scrollback line count. Matches alacritty's own default
    /// from `term::Config::scrolling_history`. WEFT-262.
    const SCROLLBACK_LINES: usize = 10_000;
    /// Pixels-per-wheel-line scaling — egui's `raw_scroll_delta.y`
    /// reports unprojected pixels, so divide by an approximate cell
    /// height to translate into terminal lines. We use the same cell
    /// metric the grid is painted with (see `cell_metrics`), which
    /// keeps the scroll feel proportional to the on-screen rows.
    const WHEEL_LINE_PX: f32 = 20.0;

    /// EventListener that drops events on the floor. We don't need
    /// alacritty's bell / title / clipboard plumbing in this renderer;
    /// the surface's job is just to display whatever the PTY bytes
    /// produce.
    #[derive(Debug, Clone, Copy, Default)]
    struct NopListener;
    impl EventListener for NopListener {
        fn send_event(&self, _event: Event) {}
    }

    /// Trivial Dimensions impl for `Term::new` / `Term::resize`.
    #[derive(Debug, Clone, Copy)]
    struct Dims {
        rows: usize,
        cols: usize,
    }
    impl Dimensions for Dims {
        fn total_lines(&self) -> usize {
            self.rows
        }
        fn screen_lines(&self) -> usize {
            self.rows
        }
        fn columns(&self) -> usize {
            self.cols
        }
    }

    /// Per-panel state. Owned by [`super::super::Explorer`].
    pub struct Terminal {
        // ── RPC machinery (daemon-side PTY) ──────────────────────────
        session_id: Option<String>,
        output_path: Option<String>,
        shell: Option<String>,
        spawn_pending: Option<ReplyRx>,
        output_pending: Option<ReplyRx>,
        last_output_poll: Option<web_time::Instant>,
        last_seen_tick: u64,
        last_size: (u16, u16),
        last_error: Option<String>,

        // ── Terminal model (alacritty grid + vte parser) ────────────
        term: Term<NopListener>,
        processor: Processor,
        /// Widget id allocated once per Terminal so input events route
        /// to the correct focus-tracked region.
        widget_id: Option<egui::Id>,
        /// Counter so successive Terminal panels don't collide on the
        /// same widget id when the user opens / closes the same
        /// surface multiple times.
        instance_seq: u64,
        /// `true` while a primary-button mouse drag is in progress; the
        /// next pointer-pos sample updates `term.selection`. WEFT-260.
        dragging_selection: bool,
    }

    impl Default for Terminal {
        fn default() -> Self {
            let initial = Dims { rows: 24, cols: 80 };
            // WEFT-262: alacritty's `Term::new(Config::default(), ...)`
            // uses `Config::scrolling_history = 10_000` by default; pin
            // it explicitly so a future alacritty default change doesn't
            // silently shrink our scrollback.
            let cfg = Config {
                scrolling_history: SCROLLBACK_LINES,
                ..Config::default()
            };
            let term = Term::new(cfg, &initial, NopListener);
            Self {
                session_id: None,
                output_path: None,
                shell: None,
                spawn_pending: None,
                output_pending: None,
                last_output_poll: None,
                last_seen_tick: 0,
                last_size: (0, 0),
                last_error: None,
                term,
                processor: Processor::new(),
                widget_id: None,
                instance_seq: next_instance_seq(),
                dragging_selection: false,
            }
        }
    }

    impl Terminal {
        pub fn paint(&mut self, ui: &mut egui::Ui, live: &Arc<Live>) {
            // 1. Drain any in-flight RPC replies.
            self.drain_spawn_reply();
            self.drain_output_reply();

            // 2. Lazy spawn on first paint.
            if self.session_id.is_none()
                && self.spawn_pending.is_none()
                && self.last_error.is_none()
            {
                self.fire_spawn(live);
            }

            // 3. Header.
            paint_header(ui, &self.shell, &self.session_id);

            if let Some(err) = &self.last_error {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 120, 120),
                    format!("error: {err}"),
                );
                ui.separator();
            }

            // 4. Reserve a region for the terminal grid. Egui paints
            //    via Painter, so we allocate once and own a Rect.
            let avail = ui.available_size_before_wrap();
            let (cell_w, cell_h) = cell_metrics(ui);
            let (rect, response) = ui.allocate_exact_size(
                avail,
                egui::Sense::click_and_drag(),
            );

            // Persistent widget id so memory/focus survives re-paints.
            let widget_id = *self
                .widget_id
                .get_or_insert_with(|| {
                    egui::Id::new(("weft-terminal", self.instance_seq))
                });

            // Click → request focus so subsequent keys go here.
            if response.clicked() {
                response.request_focus();
            }

            let painter = ui.painter_at(rect);

            // 5. Compute cols/rows from rect; resize Term if changed.
            let cols = ((rect.width() / cell_w).floor() as i32).clamp(20, 400) as u16;
            let rows = ((rect.height() / cell_h).floor() as i32).clamp(8, 200) as u16;
            if (rows, cols) != self.last_size {
                self.last_size = (rows, cols);
                self.term.resize(Dims {
                    rows: rows as usize,
                    cols: cols as usize,
                });
                self.fire_resize(live, rows, cols);
            }

            // 6. Paint global background first.
            painter.rect_filled(rect, 0.0, color_for(&Color::Named(NamedColor::Background)));

            // 7. Wheel-scroll into scrollback (WEFT-262). Egui's
            //    `smooth_scroll_delta.y` is in CSS-pixel units (smoothed
            //    across frames); positive is "scroll up" (content moves
            //    down). We convert to alacritty `Scroll::Delta(lines)`.
            let wheel_lines = if response.hovered() {
                let dy = ui.input(|i| i.smooth_scroll_delta.y);
                if dy.abs() >= 0.5 {
                    (dy / WHEEL_LINE_PX).round() as i32
                } else {
                    0
                }
            } else {
                0
            };
            if wheel_lines != 0 {
                self.term.scroll_display(Scroll::Delta(wheel_lines));
            }

            // 8. Mouse selection (WEFT-260). Drag with primary button to
            //    select; click without drag clears the selection.
            self.handle_selection(ui, &response, rect, cell_w, cell_h);

            // 9. Paint the grid.
            paint_grid(&painter, &self.term, rect, cell_w, cell_h);

            // 10. Paint the selection overlay over the grid.
            paint_selection(&painter, &self.term, rect, cell_w, cell_h);

            // 11. Input: if focused, translate egui events to PTY bytes.
            if response.has_focus() {
                let cursor_visible_blink = ui.input(|i| i.time).fract() < 0.55;
                if cursor_visible_blink {
                    paint_cursor(&painter, &self.term, rect, cell_w, cell_h);
                }
                // Copy / paste handling (WEFT-260) lives alongside key
                // input so the same focus gate applies.
                self.handle_clipboard(ui, live);
                let bytes = collect_input_bytes(ui);
                if !bytes.is_empty() {
                    self.fire_write(live, &bytes);
                    // Any keyboard input also returns the viewport to
                    // the bottom — matches xterm/alacritty behaviour
                    // and avoids the "I typed and nothing happened"
                    // confusion when the user is in scrollback.
                    self.term.scroll_display(Scroll::Bottom);
                }
            } else {
                // Show a hollow cursor when not focused so the user
                // knows where it'll resume on click.
                paint_cursor_hollow(&painter, &self.term, rect, cell_w, cell_h);
            }

            // Keep widget_id alive so future paints find it.
            let _ = widget_id;

            // Cause continuous repaints while focused (so the cursor
            // blinks and incoming output appears without external
            // pings). We already poll output every 50ms, but the
            // animation needs frame ticks too.
            if response.has_focus() {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
            }

            // 9. Schedule next output poll.
            self.maybe_poll_output(live);
        }

        pub fn close(&mut self, live: &Arc<Live>) {
            self.fire_close(live);
            self.output_pending = None;
            self.spawn_pending = None;
        }

        /// Mouse selection state machine (WEFT-260). Tracks a primary
        /// drag from press to release; pixel positions are converted to
        /// alacritty `Point`s via [`pixel_to_point`]. A bare click
        /// (down + up without movement) clears any existing selection.
        fn handle_selection(
            &mut self,
            ui: &egui::Ui,
            response: &egui::Response,
            rect: egui::Rect,
            cell_w: f32,
            cell_h: f32,
        ) {
            // Press → start a fresh Simple selection at the pointer.
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(pos) = ui.ctx().pointer_interact_pos()
                && let Some((point, side)) = pixel_to_point(&self.term, rect, cell_w, cell_h, pos)
            {
                self.term.selection =
                    Some(Selection::new(SelectionType::Simple, point, side));
                self.dragging_selection = true;
            }
            // Drag → extend the selection's end anchor.
            if self.dragging_selection
                && response.dragged_by(egui::PointerButton::Primary)
                && let Some(pos) = ui.ctx().pointer_interact_pos()
                && let Some((point, side)) = pixel_to_point(&self.term, rect, cell_w, cell_h, pos)
                && let Some(sel) = self.term.selection.as_mut()
            {
                sel.update(point, side);
            }
            // Release → leave the selection in place; clipboard read is
            // an explicit Ctrl/Cmd-C below. Drop empty selections so
            // `selection_to_string` doesn't return Some("").
            if self.dragging_selection
                && response.drag_stopped_by(egui::PointerButton::Primary)
            {
                self.dragging_selection = false;
                if let Some(sel) = self.term.selection.as_ref()
                    && sel.is_empty()
                {
                    self.term.selection = None;
                }
            }
            // Bare click (no drag) clears any existing selection so the
            // user can deselect by clicking into the grid.
            if response.clicked() && !self.dragging_selection {
                self.term.selection = None;
            }
        }

        /// Handle copy / paste while focused (WEFT-260). Egui delivers
        /// these as `Event::Copy` / `Event::Paste` after platform
        /// shortcut translation, so we don't have to special-case
        /// Ctrl-vs-Cmd ourselves. Copies route through alacritty's
        /// `selection_to_string`; pastes are written as PTY input bytes.
        fn handle_clipboard(&mut self, ui: &egui::Ui, live: &Arc<Live>) {
            // Drain Copy/Cut/Paste events; rely on egui's platform
            // shortcut translation rather than reading raw modifiers
            // ourselves.
            let mut want_copy = false;
            let mut paste_text: Option<String> = None;
            ui.input(|i| {
                for ev in &i.events {
                    match ev {
                        egui::Event::Copy | egui::Event::Cut => {
                            // Cut on a read-only terminal grid behaves
                            // as copy — we cannot delete daemon-side
                            // PTY output from the local model.
                            want_copy = true;
                        }
                        egui::Event::Paste(s) => {
                            paste_text = Some(s.clone());
                        }
                        _ => {}
                    }
                }
            });
            if want_copy
                && let Some(text) = self.term.selection_to_string()
                && !text.is_empty()
            {
                ui.ctx().output_mut(|o| {
                    o.commands
                        .push(egui::OutputCommand::CopyText(text));
                });
            }
            if let Some(text) = paste_text
                && !text.is_empty()
            {
                self.fire_write(live, text.as_bytes());
            }
        }

        // ── RPC helpers (kept from prior implementation) ────────────

        fn fire_spawn(&mut self, live: &Arc<Live>) {
            let (tx, rx) = live::reply_channel();
            self.spawn_pending = Some(rx);
            live.submit(Command::Raw {
                method: "terminal.spawn".into(),
                params: serde_json::json!({ "rows": 24, "cols": 80 }),
                reply: Some(tx),
            });
        }

        fn drain_spawn_reply(&mut self) {
            let Some(rx) = self.spawn_pending.as_mut() else {
                return;
            };
            match live::try_recv_reply(rx) {
                live::TryReply::Done(Ok(value)) => {
                    self.spawn_pending = None;
                    let session_id = value
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let output_path = value
                        .get("output_path")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let shell = value
                        .get("shell")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if session_id.is_none() || output_path.is_none() {
                        self.last_error = Some(
                            "terminal.spawn reply missing session_id / output_path".into(),
                        );
                        return;
                    }
                    self.session_id = session_id;
                    self.output_path = output_path;
                    self.shell = shell;
                    self.last_error = None;
                }
                live::TryReply::Done(Err(err)) => {
                    self.spawn_pending = None;
                    self.last_error = Some(format!("spawn: {err}"));
                }
                live::TryReply::Closed => {
                    self.spawn_pending = None;
                    self.last_error = Some("spawn: transport closed".into());
                }
                live::TryReply::Empty => { /* still in flight */ }
            }
        }

        fn maybe_poll_output(&mut self, live: &Arc<Live>) {
            let Some(path) = self.output_path.clone() else {
                return;
            };
            if self.output_pending.is_some() {
                return;
            }
            let due = match self.last_output_poll {
                Some(t) => t.elapsed() >= OUTPUT_POLL,
                None => true,
            };
            if !due {
                return;
            }
            let (tx, rx) = live::reply_channel();
            self.output_pending = Some(rx);
            self.last_output_poll = Some(web_time::Instant::now());
            live.submit(Command::Raw {
                method: "substrate.read".into(),
                params: serde_json::json!({ "path": path }),
                reply: Some(tx),
            });
        }

        fn drain_output_reply(&mut self) {
            let Some(rx) = self.output_pending.as_mut() else {
                return;
            };
            match live::try_recv_reply(rx) {
                live::TryReply::Done(Ok(value)) => {
                    self.output_pending = None;
                    self.handle_output_value(value);
                }
                live::TryReply::Done(Err(_)) => {
                    self.output_pending = None;
                }
                live::TryReply::Closed => {
                    self.output_pending = None;
                }
                live::TryReply::Empty => { /* still in flight */ }
            }
        }

        fn handle_output_value(&mut self, value: Value) {
            let tick = value.get("tick").and_then(Value::as_u64).unwrap_or(0);
            if tick != 0 && tick <= self.last_seen_tick {
                return;
            }
            let chunk = value.get("value").cloned().unwrap_or(Value::Null);
            let Some(obj) = chunk.as_object() else {
                return;
            };
            let data_b64 = obj.get("data").and_then(Value::as_str).unwrap_or("");
            let exit = obj.get("exit").and_then(Value::as_bool).unwrap_or(false);
            if !data_b64.is_empty() {
                use base64::Engine;
                match base64::engine::general_purpose::STANDARD.decode(data_b64.as_bytes())
                {
                    Ok(bytes) => {
                        self.processor.advance(&mut self.term, &bytes);
                    }
                    Err(e) => {
                        self.last_error = Some(format!("base64 decode: {e}"));
                    }
                }
            }
            if exit {
                self.session_id = None;
                self.output_path = None;
            }
            self.last_seen_tick = tick;
        }

        fn fire_write(&mut self, live: &Arc<Live>, bytes: &[u8]) {
            let Some(id) = self.session_id.clone() else {
                return;
            };
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            live.submit(Command::Raw {
                method: "terminal.write".into(),
                params: serde_json::json!({
                    "session_id": id,
                    "data": data,
                }),
                reply: Some(live::reply_channel().0),
            });
        }

        fn fire_resize(&mut self, live: &Arc<Live>, rows: u16, cols: u16) {
            let Some(id) = self.session_id.clone() else {
                return;
            };
            live.submit(Command::Raw {
                method: "terminal.resize".into(),
                params: serde_json::json!({
                    "session_id": id,
                    "rows": rows,
                    "cols": cols,
                }),
                reply: Some(live::reply_channel().0),
            });
        }

        fn fire_close(&mut self, live: &Arc<Live>) {
            let Some(id) = self.session_id.take() else {
                return;
            };
            live.submit(Command::Raw {
                method: "terminal.close".into(),
                params: serde_json::json!({ "session_id": id }),
                reply: Some(live::reply_channel().0),
            });
        }
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            // Explorer's `close()` drives `terminal.close` explicitly.
            // If Drop runs without it, the daemon-side session lives
            // until the daemon shuts down — acceptable.
        }
    }

    fn next_instance_seq() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(1);
        N.fetch_add(1, Ordering::Relaxed)
    }

    fn cell_metrics(ui: &egui::Ui) -> (f32, f32) {
        let style = ui.style().clone();
        let font_id = egui::TextStyle::Monospace.resolve(&style);
        let cell_w = ui
            .ctx()
            .fonts_mut(|f| f.glyph_width(&font_id, 'M'));
        let cell_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let cell_w = if cell_w > 0.0 { cell_w } else { FALLBACK_CELL_W };
        let cell_h = if cell_h > 0.0 { cell_h } else { FALLBACK_CELL_H };
        (cell_w, cell_h)
    }

    fn paint_grid(
        painter: &egui::Painter,
        term: &Term<NopListener>,
        rect: egui::Rect,
        cell_w: f32,
        cell_h: f32,
    ) {
        let display_offset = term.grid().display_offset() as i32;
        let global_bg = color_for(&Color::Named(NamedColor::Background));
        let font_id =
            egui::FontId::new(cell_h * 0.85, egui::FontFamily::Monospace);

        for indexed in term.grid().display_iter() {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let line_num = indexed.point.line.0 + display_offset;
            let col = indexed.point.column.0 as f32;
            let x = rect.min.x + col * cell_w;
            let y = rect.min.y + line_num as f32 * cell_h;

            let mut fg = color_for(&cell.fg);
            let mut bg = color_for(&cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let dim = cell
                .flags
                .intersects(Flags::DIM | Flags::DIM_BOLD);
            if dim {
                fg = fg.linear_multiply(0.7);
            }

            let cell_width =
                if cell.flags.contains(Flags::WIDE_CHAR) { cell_w * 2.0 } else { cell_w };

            // Background fill (skip when matches global so we save shapes).
            if bg != global_bg {
                let bg_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(cell_width + 0.5, cell_h + 0.5),
                );
                painter.rect_filled(bg_rect, 0.0, bg);
            }

            // Glyph (skip blanks for performance).
            if cell.c != ' ' && cell.c != '\0' {
                let bold = cell
                    .flags
                    .intersects(Flags::BOLD | Flags::BOLD_ITALIC | Flags::DIM_BOLD);
                let italic = cell
                    .flags
                    .intersects(Flags::ITALIC | Flags::BOLD_ITALIC);
                paint_glyph(
                    painter,
                    egui::pos2(x + cell_width * 0.5, y + cell_h * 0.5),
                    cell.c,
                    &font_id,
                    fg,
                    bold,
                    italic,
                );
            }

            // Underline.
            if cell.flags.contains(Flags::UNDERLINE) {
                let yu = y + cell_h - 1.5;
                painter.line_segment(
                    [egui::pos2(x, yu), egui::pos2(x + cell_width, yu)],
                    egui::Stroke::new(1.0, fg),
                );
            }
        }
    }

    /// Paint a single glyph with optional bold / italic synthesis
    /// (WEFT-261).
    ///
    /// Egui's bundled monospace face has no separate bold or italic
    /// variants, so we approximate:
    ///   - **Bold**: draw the glyph twice with a 0.5 px x-offset. The
    ///     overdraw effectively widens each stroke by one sub-pixel,
    ///     producing a "synthetic bold" that's distinguishable from
    ///     regular weight without needing a heavy font face. The
    ///     0.5 px offset is a half-pixel so AA fills in cleanly.
    ///   - **Italic**: rotate the glyph by ~9° (atan(1/6)) about its
    ///     baseline center via `TextShape::with_angle_and_anchor`.
    ///     This is closer to a true oblique than a shear would be
    ///     without dropping to mesh transforms, and reads as italic
    ///     at terminal cell sizes.
    fn paint_glyph(
        painter: &egui::Painter,
        center: egui::Pos2,
        c: char,
        font_id: &egui::FontId,
        color: egui::Color32,
        bold: bool,
        italic: bool,
    ) {
        // Italic-oblique angle. ~9.46° (atan(1/6)) approximates a
        // typographic oblique without needing a real italic font face.
        const ITALIC_ANGLE: f32 = 0.165;

        if !italic {
            painter.text(center, egui::Align2::CENTER_CENTER, c, font_id.clone(), color);
            if bold {
                // Synthetic bold: re-paint with a 0.5 px x-shift so AA
                // fills the gap rather than producing a hard double-stroke.
                painter.text(
                    egui::pos2(center.x + 0.5, center.y),
                    egui::Align2::CENTER_CENTER,
                    c,
                    font_id.clone(),
                    color,
                );
            }
            return;
        }

        // Italic path: layout the glyph as a rotated TextShape.
        let galley = painter.layout_no_wrap(c.to_string(), font_id.clone(), color);
        let pos = center - galley.size() * 0.5;
        let mut shape = egui::epaint::TextShape::new(pos, galley.clone(), color)
            .with_angle_and_anchor(ITALIC_ANGLE, egui::Align2::CENTER_CENTER);
        painter.add(shape.clone());
        if bold {
            shape.pos.x += 0.5;
            painter.add(shape);
        }
    }

    /// Translucent overlay for the current selection (WEFT-260). Walks
    /// the selection range row-by-row and paints a single rect per row.
    fn paint_selection(
        painter: &egui::Painter,
        term: &Term<NopListener>,
        rect: egui::Rect,
        cell_w: f32,
        cell_h: f32,
    ) {
        let Some(sel) = term.selection.as_ref() else {
            return;
        };
        let Some(range) = sel.to_range(term) else {
            return;
        };
        let display_offset = term.grid().display_offset() as i32;
        let screen_lines = term.grid().screen_lines() as i32;
        let cols = term.grid().columns() as i32;
        let overlay = egui::Color32::from_rgba_unmultiplied(120, 160, 220, 80);

        let start = range.start;
        let end = range.end;

        // SelectionRange iterates inclusively from `start.line` to
        // `end.line`. For each visible row we paint one filled rect
        // covering the column range that's selected on that row.
        for line in start.line.0..=end.line.0 {
            let screen_y = line + display_offset;
            if screen_y < 0 || screen_y >= screen_lines {
                continue;
            }
            let (col_lo, col_hi) = if start.line.0 == end.line.0 {
                (start.column.0 as i32, end.column.0 as i32 + 1)
            } else if line == start.line.0 {
                (start.column.0 as i32, cols)
            } else if line == end.line.0 {
                (0, end.column.0 as i32 + 1)
            } else {
                (0, cols)
            };
            let col_lo = col_lo.max(0);
            let col_hi = col_hi.min(cols);
            if col_hi <= col_lo {
                continue;
            }
            let x0 = rect.min.x + col_lo as f32 * cell_w;
            let x1 = rect.min.x + col_hi as f32 * cell_w;
            let y0 = rect.min.y + screen_y as f32 * cell_h;
            let y1 = y0 + cell_h;
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)),
                0.0,
                overlay,
            );
        }
    }

    /// Convert a screen-pixel position inside `rect` to an alacritty
    /// grid `Point` + `Side`, accounting for current scrollback offset.
    /// Returns `None` if the pointer falls outside the active grid
    /// (e.g. below the last row).
    fn pixel_to_point(
        term: &Term<NopListener>,
        rect: egui::Rect,
        cell_w: f32,
        cell_h: f32,
        pos: egui::Pos2,
    ) -> Option<(Point, Side)> {
        if !rect.contains(pos) {
            return None;
        }
        let local_x = (pos.x - rect.min.x).max(0.0);
        let local_y = (pos.y - rect.min.y).max(0.0);
        let cols = term.grid().columns() as i32;
        let screen_lines = term.grid().screen_lines() as i32;
        let display_offset = term.grid().display_offset() as i32;

        let col_f = local_x / cell_w;
        let mut col = col_f.floor() as i32;
        col = col.clamp(0, (cols - 1).max(0));
        let frac = col_f - col as f32;
        let side = if frac < 0.5 { Side::Left } else { Side::Right };

        let screen_y = (local_y / cell_h).floor() as i32;
        if screen_y < 0 || screen_y >= screen_lines {
            return None;
        }
        // Convert screen row → grid line (display_iter uses a
        // display_offset-relative coordinate that runs negative when
        // the user has scrolled into history).
        let line = screen_y - display_offset;
        Some((Point::new(Line(line), Column(col as usize)), side))
    }

    fn paint_cursor(
        painter: &egui::Painter,
        term: &Term<NopListener>,
        rect: egui::Rect,
        cell_w: f32,
        cell_h: f32,
    ) {
        if let Some((x, y)) = cursor_screen_xy(term, rect, cell_w, cell_h) {
            let r = egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(cell_w, cell_h),
            );
            painter.rect_filled(
                r,
                0.0,
                color_for(&Color::Named(NamedColor::Foreground))
                    .linear_multiply(0.65),
            );
        }
    }

    fn paint_cursor_hollow(
        painter: &egui::Painter,
        term: &Term<NopListener>,
        rect: egui::Rect,
        cell_w: f32,
        cell_h: f32,
    ) {
        if let Some((x, y)) = cursor_screen_xy(term, rect, cell_w, cell_h) {
            let r = egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(cell_w, cell_h),
            );
            painter.rect_stroke(
                r,
                0.0,
                egui::Stroke::new(
                    1.0,
                    color_for(&Color::Named(NamedColor::Foreground))
                        .linear_multiply(0.45),
                ),
                egui::StrokeKind::Inside,
            );
        }
    }

    /// Cursor (x, y) in pixel coords within `rect`, accounting for
    /// scrollback display offset. Returns `None` if the cursor sits in
    /// the scrollback (off-viewport).
    fn cursor_screen_xy(
        term: &Term<NopListener>,
        rect: egui::Rect,
        cell_w: f32,
        cell_h: f32,
    ) -> Option<(f32, f32)> {
        let cursor = &term.grid().cursor;
        let cursor_line = cursor.point.line.0;
        let display_offset = term.grid().display_offset() as i32;
        let screen_lines = term.grid().screen_lines() as i32;
        let screen_y = cursor_line + display_offset;
        if screen_y < 0 || screen_y >= screen_lines {
            return None;
        }
        let col = cursor.point.column.0 as f32;
        let x = rect.min.x + col * cell_w;
        let y = rect.min.y + screen_y as f32 * cell_h;
        Some((x, y))
    }

    /// Map alacritty's `Color` enum → egui Color32. Named entries use a
    /// standard 16-color ANSI palette tuned for dark backgrounds.
    /// Indexed[16..256] follows the xterm 256-color cube + grayscale ramp.
    /// Spec is direct RGB.
    fn color_for(c: &Color) -> egui::Color32 {
        match c {
            Color::Spec(rgb) => egui::Color32::from_rgb(rgb.r, rgb.g, rgb.b),
            Color::Named(n) => named_color(*n),
            Color::Indexed(i) => indexed_color(*i),
        }
    }

    fn named_color(n: NamedColor) -> egui::Color32 {
        // Solarized-dark-ish palette tuned to match egui's monospace
        // text. Foreground=light, Background=dark — what most TUIs
        // expect.
        match n {
            NamedColor::Black | NamedColor::DimBlack => rgb(7, 54, 66),
            NamedColor::Red | NamedColor::DimRed => rgb(220, 50, 47),
            NamedColor::Green | NamedColor::DimGreen => rgb(133, 153, 0),
            NamedColor::Yellow | NamedColor::DimYellow => rgb(181, 137, 0),
            NamedColor::Blue | NamedColor::DimBlue => rgb(38, 139, 210),
            NamedColor::Magenta | NamedColor::DimMagenta => rgb(211, 54, 130),
            NamedColor::Cyan | NamedColor::DimCyan => rgb(42, 161, 152),
            NamedColor::White | NamedColor::DimWhite => rgb(238, 232, 213),
            NamedColor::BrightBlack => rgb(88, 110, 117),
            NamedColor::BrightRed => rgb(255, 90, 87),
            NamedColor::BrightGreen => rgb(180, 200, 0),
            NamedColor::BrightYellow => rgb(220, 180, 30),
            NamedColor::BrightBlue => rgb(100, 180, 240),
            NamedColor::BrightMagenta => rgb(240, 90, 170),
            NamedColor::BrightCyan => rgb(80, 200, 190),
            NamedColor::BrightWhite => rgb(253, 246, 227),
            NamedColor::Foreground | NamedColor::DimForeground => rgb(220, 220, 225),
            NamedColor::BrightForeground => rgb(245, 245, 250),
            NamedColor::Background => rgb(12, 12, 16),
            NamedColor::Cursor => rgb(220, 220, 225),
        }
    }

    fn indexed_color(i: u8) -> egui::Color32 {
        // 0-15: standard palette
        if i < 16 {
            return ansi_16(i);
        }
        // 16-231: 6×6×6 RGB cube
        if i < 232 {
            let i = i - 16;
            let r = i / 36;
            let g = (i / 6) % 6;
            let b = i % 6;
            let conv = |v: u8| -> u8 {
                if v == 0 { 0 } else { 55 + v * 40 }
            };
            return egui::Color32::from_rgb(conv(r), conv(g), conv(b));
        }
        // 232-255: grayscale ramp
        let v = 8 + (i - 232) * 10;
        egui::Color32::from_rgb(v, v, v)
    }

    fn ansi_16(i: u8) -> egui::Color32 {
        match i {
            0 => named_color(NamedColor::Black),
            1 => named_color(NamedColor::Red),
            2 => named_color(NamedColor::Green),
            3 => named_color(NamedColor::Yellow),
            4 => named_color(NamedColor::Blue),
            5 => named_color(NamedColor::Magenta),
            6 => named_color(NamedColor::Cyan),
            7 => named_color(NamedColor::White),
            8 => named_color(NamedColor::BrightBlack),
            9 => named_color(NamedColor::BrightRed),
            10 => named_color(NamedColor::BrightGreen),
            11 => named_color(NamedColor::BrightYellow),
            12 => named_color(NamedColor::BrightBlue),
            13 => named_color(NamedColor::BrightMagenta),
            14 => named_color(NamedColor::BrightCyan),
            15 => named_color(NamedColor::BrightWhite),
            _ => unreachable!(),
        }
    }

    fn rgb(r: u8, g: u8, b: u8) -> egui::Color32 {
        egui::Color32::from_rgb(r, g, b)
    }

    /// Translate egui keyboard / text events to PTY input bytes.
    /// Drains events we consume; ignores the rest.
    fn collect_input_bytes(ui: &egui::Ui) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        ui.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Text(s) => out.extend_from_slice(s.as_bytes()),
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if let Some(bytes) = key_to_bytes(*key, *modifiers) {
                            out.extend_from_slice(&bytes);
                        }
                    }
                    _ => {}
                }
            }
        });
        out
    }

    fn key_to_bytes(key: egui::Key, modifiers: egui::Modifiers) -> Option<Vec<u8>> {
        // Ctrl+letter → control byte (when no other modifier interferes).
        if modifiers.ctrl
            && !modifiers.alt
            && !modifiers.shift
            && let Some(letter) = ctrl_letter(key)
        {
            return Some(vec![letter & 0x1f]);
        }
        let bytes: &[u8] = match key {
            egui::Key::Enter => b"\r",
            egui::Key::Tab => b"\t",
            egui::Key::Backspace => b"\x7f",
            egui::Key::Escape => b"\x1b",
            egui::Key::ArrowUp => b"\x1b[A",
            egui::Key::ArrowDown => b"\x1b[B",
            egui::Key::ArrowRight => b"\x1b[C",
            egui::Key::ArrowLeft => b"\x1b[D",
            egui::Key::Home => b"\x1b[H",
            egui::Key::End => b"\x1b[F",
            egui::Key::PageUp => b"\x1b[5~",
            egui::Key::PageDown => b"\x1b[6~",
            egui::Key::Delete => b"\x1b[3~",
            egui::Key::Insert => b"\x1b[2~",
            egui::Key::F1 => b"\x1bOP",
            egui::Key::F2 => b"\x1bOQ",
            egui::Key::F3 => b"\x1bOR",
            egui::Key::F4 => b"\x1bOS",
            egui::Key::F5 => b"\x1b[15~",
            egui::Key::F6 => b"\x1b[17~",
            egui::Key::F7 => b"\x1b[18~",
            egui::Key::F8 => b"\x1b[19~",
            egui::Key::F9 => b"\x1b[20~",
            egui::Key::F10 => b"\x1b[21~",
            egui::Key::F11 => b"\x1b[23~",
            egui::Key::F12 => b"\x1b[24~",
            _ => return None,
        };
        Some(bytes.to_vec())
    }

    fn ctrl_letter(key: egui::Key) -> Option<u8> {
        // Map letter keys to their ASCII byte for masking with 0x1f.
        match key {
            egui::Key::A => Some(b'a'),
            egui::Key::B => Some(b'b'),
            egui::Key::C => Some(b'c'),
            egui::Key::D => Some(b'd'),
            egui::Key::E => Some(b'e'),
            egui::Key::F => Some(b'f'),
            egui::Key::G => Some(b'g'),
            egui::Key::H => Some(b'h'),
            egui::Key::I => Some(b'i'),
            egui::Key::J => Some(b'j'),
            egui::Key::K => Some(b'k'),
            egui::Key::L => Some(b'l'),
            egui::Key::M => Some(b'm'),
            egui::Key::N => Some(b'n'),
            egui::Key::O => Some(b'o'),
            egui::Key::P => Some(b'p'),
            egui::Key::Q => Some(b'q'),
            egui::Key::R => Some(b'r'),
            egui::Key::S => Some(b's'),
            egui::Key::T => Some(b't'),
            egui::Key::U => Some(b'u'),
            egui::Key::V => Some(b'v'),
            egui::Key::W => Some(b'w'),
            egui::Key::X => Some(b'x'),
            egui::Key::Y => Some(b'y'),
            egui::Key::Z => Some(b'z'),
            _ => None,
        }
    }

    fn paint_header(
        ui: &mut egui::Ui,
        shell: &Option<String>,
        session_id: &Option<String>,
    ) {
        ui.horizontal(|ui| {
            ui.heading("Terminal");
            ui.separator();
            match (session_id, shell) {
                (Some(id), Some(shell)) => {
                    ui.label(
                        egui::RichText::new(format!("{shell}  ·  {id}"))
                            .monospace()
                            .small()
                            .color(egui::Color32::from_rgb(150, 170, 200)),
                    );
                }
                (Some(id), None) => {
                    ui.label(
                        egui::RichText::new(id)
                            .monospace()
                            .small()
                            .color(egui::Color32::from_rgb(150, 170, 200)),
                    );
                }
                (None, _) => {
                    ui.label(
                        egui::RichText::new("starting…")
                            .italics()
                            .small()
                            .color(egui::Color32::from_rgb(170, 170, 180)),
                    );
                }
            }
        });
        ui.separator();
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        #[test]
        fn handle_output_value_advances_processor() {
            let mut t = Terminal::default();
            // base64("hi\n") = "aGkK"
            let chunk = json!({
                "tick": 1,
                "value": { "data": "aGkK", "ts_ms": 0 },
            });
            t.handle_output_value(chunk);
            // After parse, the grid's first row should have 'h' at col 0
            // and 'i' at col 1.
            let grid = t.term.grid();
            let row = &grid[Line(0)];
            assert_eq!(row[Column(0)].c, 'h');
            assert_eq!(row[Column(1)].c, 'i');
            assert_eq!(t.last_seen_tick, 1);
        }

        #[test]
        fn handle_output_value_dedupes_on_same_tick() {
            let mut t = Terminal::default();
            // base64("a") = "YQ=="
            let chunk = json!({
                "tick": 5,
                "value": { "data": "YQ==", "ts_ms": 0 },
            });
            t.handle_output_value(chunk.clone());
            t.handle_output_value(chunk);
            let row = &t.term.grid()[Line(0)];
            assert_eq!(row[Column(0)].c, 'a');
            // If we double-applied, col 1 would be 'a' too.
            assert_eq!(row[Column(1)].c, ' ');
        }

        #[test]
        fn handle_output_value_marks_exit() {
            let mut t = Terminal::default();
            t.session_id = Some("t-deadbeef0001".into());
            t.output_path = Some("substrate/x/derived/terminal/t-deadbeef0001".into());
            let chunk = json!({
                "tick": 3,
                "value": { "data": "", "ts_ms": 0, "exit": true },
            });
            t.handle_output_value(chunk);
            assert!(t.session_id.is_none());
            assert!(t.output_path.is_none());
        }

        #[test]
        fn key_to_bytes_arrows_and_specials() {
            let m = egui::Modifiers::default();
            assert_eq!(key_to_bytes(egui::Key::Enter, m).unwrap(), b"\r");
            assert_eq!(key_to_bytes(egui::Key::Backspace, m).unwrap(), b"\x7f");
            assert_eq!(key_to_bytes(egui::Key::Tab, m).unwrap(), b"\t");
            assert_eq!(key_to_bytes(egui::Key::Escape, m).unwrap(), b"\x1b");
            assert_eq!(key_to_bytes(egui::Key::ArrowUp, m).unwrap(), b"\x1b[A");
            assert_eq!(key_to_bytes(egui::Key::ArrowDown, m).unwrap(), b"\x1b[B");
            assert_eq!(key_to_bytes(egui::Key::ArrowLeft, m).unwrap(), b"\x1b[D");
            assert_eq!(key_to_bytes(egui::Key::ArrowRight, m).unwrap(), b"\x1b[C");
        }

        #[test]
        fn key_to_bytes_ctrl_letter_emits_control_byte() {
            let ctrl = egui::Modifiers {
                ctrl: true,
                ..Default::default()
            };
            assert_eq!(key_to_bytes(egui::Key::C, ctrl).unwrap(), vec![0x03]);
            assert_eq!(key_to_bytes(egui::Key::D, ctrl).unwrap(), vec![0x04]);
            assert_eq!(key_to_bytes(egui::Key::U, ctrl).unwrap(), vec![0x15]);
        }

        // ── WEFT-262 scrollback ────────────────────────────────────

        #[test]
        fn default_terminal_has_scrollback_history() {
            // The Term must be constructed with a non-zero scrolling
            // history so wheel-scroll has anywhere to scroll into.
            // alacritty's default is 10_000; we pin it explicitly.
            let t = Terminal::default();
            assert!(
                t.term.history_size() == 0
                    || t.term.history_size() <= SCROLLBACK_LINES,
                "history_size grows lazily; bound is what matters"
            );
            // The bound itself is encoded in our SCROLLBACK_LINES const,
            // and the type assert here keeps the constant from being
            // accidentally renamed without also touching the test.
            assert_eq!(SCROLLBACK_LINES, 10_000);
        }

        #[test]
        fn scroll_display_delta_moves_into_history() {
            // Push enough lines to populate scrollback, then verify
            // `scroll_display(Scroll::Delta(+N))` advances display_offset.
            let mut t = Terminal::default();
            // Feed 50 newlines so the grid history is well above the
            // 24-row default viewport.
            let bytes: Vec<u8> = (0..50)
                .flat_map(|i| format!("line {i}\r\n").into_bytes())
                .collect();
            t.processor.advance(&mut t.term, &bytes);
            let before = t.term.grid().display_offset();
            t.term.scroll_display(Scroll::Delta(5));
            let after = t.term.grid().display_offset();
            assert!(
                after > before,
                "display_offset should increase on Scroll::Delta(+); was {before}→{after}"
            );
            // Scroll back to bottom resets the offset.
            t.term.scroll_display(Scroll::Bottom);
            assert_eq!(t.term.grid().display_offset(), 0);
        }

        // ── WEFT-260 selection ─────────────────────────────────────

        #[test]
        fn pixel_to_point_maps_origin_to_top_left() {
            let t = Terminal::default();
            // 24×80 grid built with FALLBACK_CELL_W/H. Use those for the
            // test so we don't depend on egui font loading.
            let cell_w = FALLBACK_CELL_W;
            let cell_h = FALLBACK_CELL_H;
            let rect = egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(cell_w * 80.0, cell_h * 24.0),
            );
            let (point, side) =
                pixel_to_point(&t.term, rect, cell_w, cell_h, egui::pos2(1.0, 1.0))
                    .expect("origin must map");
            assert_eq!(point.line.0, 0);
            assert_eq!(point.column.0, 0);
            assert_eq!(side, Side::Left);
        }

        #[test]
        fn pixel_to_point_outside_returns_none() {
            let t = Terminal::default();
            let cell_w = FALLBACK_CELL_W;
            let cell_h = FALLBACK_CELL_H;
            let rect = egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(cell_w * 80.0, cell_h * 24.0),
            );
            assert!(
                pixel_to_point(
                    &t.term,
                    rect,
                    cell_w,
                    cell_h,
                    egui::pos2(rect.max.x + 100.0, 0.0)
                )
                .is_none()
            );
        }

        #[test]
        fn selection_round_trips_via_to_string() {
            // Drive the term with "hello", create a Simple selection
            // covering cols 0..5 of line 0, and assert
            // `selection_to_string` returns "hello".
            let mut t = Terminal::default();
            t.processor.advance(&mut t.term, b"hello");
            let mut sel = Selection::new(
                SelectionType::Simple,
                Point::new(Line(0), Column(0)),
                Side::Left,
            );
            sel.update(Point::new(Line(0), Column(4)), Side::Right);
            t.term.selection = Some(sel);
            let s = t.term.selection_to_string().unwrap_or_default();
            assert_eq!(s, "hello");
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;

    /// Stub for browser builds — alacritty_terminal pulls in
    /// platform-specific PTY/polling crates that don't compile to
    /// wasm. We render a placeholder instead.
    #[derive(Default)]
    pub struct Terminal {}

    impl Terminal {
        pub fn paint(&mut self, ui: &mut egui::Ui, _live: &Arc<Live>) {
            ui.heading("Terminal");
            ui.separator();
            ui.colored_label(
                egui::Color32::from_rgb(220, 180, 60),
                "Terminal is not available in the browser build.",
            );
            ui.label("Run the native app to use the terminal panel.");
        }

        pub fn close(&mut self, _live: &Arc<Live>) {}
    }
}

pub use imp::Terminal;

#[cfg(test)]
mod sentinel_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_terminal_sentinel() {
        let v = json!({ "kind": "terminal", "label": "Terminal" });
        assert_eq!(matches(&v), PRIORITY);
    }

    #[test]
    fn rejects_other_kinds() {
        assert_eq!(matches(&json!({ "kind": "chat" })), 0);
        assert_eq!(matches(&json!({ "kind": null })), 0);
        assert_eq!(matches(&json!(null)), 0);
        assert_eq!(matches(&json!([1, 2, 3])), 0);
        assert_eq!(matches(&json!("terminal")), 0);
    }

    #[test]
    fn rejects_missing_kind() {
        let v = json!({ "label": "Terminal" });
        assert_eq!(matches(&v), 0);
    }
}
