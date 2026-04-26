//! Terminal — egui surface for a daemon-side PTY session.
//!
//! Architecture (matches the architectural decision the task brief
//! locks in):
//!
//! - The PTY lives in the daemon (crate `clawft-service-terminal`).
//!   Output bytes are published as base64 chunks at
//!   `substrate/<daemon-node>/derived/terminal/<session_id>`.
//! - This panel is a thin renderer. On first paint it fires
//!   `terminal.spawn`, stashes the returned `session_id`, and starts
//!   subscribing (via `substrate.read` polling, same cascade as
//!   [`super::workshop::WorkshopView`]) to the output path.
//! - Input: a one-line text field at the bottom; Enter base64-encodes
//!   the line + `\n` and fires `terminal.write`.
//! - Resize: when the visible terminal area changes, we estimate cell
//!   dims from monospace font metrics and fire `terminal.resize` so
//!   in-shell apps reflow.
//! - Drop: `terminal.close` fires from `Drop::drop` so closing the
//!   panel kills the daemon-side child shell.
//!
//! ## Sentinel shape
//!
//! Top-level surface sentinels live at
//! `substrate/<daemon-node>/ui/<name>` with at least
//! `{ "kind": "<name>" }`. This panel matches on
//! `{ "kind": "terminal" }` — see [`matches`].
//!
//! ## What's deferred
//!
//! - **No ANSI parsing.** Bytes are decoded UTF-8-lossy and appended.
//!   Colors, cursor moves, alt-screen all show as literal escape
//!   sequences — the next iteration plugs `vte` in here.
//! - **No scrollback ring.** The accumulator is unbounded today; egui
//!   `ScrollArea` handles overflow. A bounded ring lands when long-
//!   running sessions force it.
//! - **No mouse / clipboard / focus management** beyond what egui
//!   gives us by default.

use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::live::{self, Command, Live, ReplyRx};

/// Shape-match priority for the terminal sentinel. Higher than the
/// generic JSON fallback (1) and the control-toggle (25) — the
/// sentinel's `kind: "terminal"` is exclusive enough that we want it
/// to win wherever it appears.
pub const PRIORITY: u32 = 50;

/// How often to poll the output substrate path for new chunks. Same
/// cadence as [`super::SELECT_POLL`] so terminal output feels
/// equally live as the rest of the Explorer.
const OUTPUT_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Default terminal cell metrics used to convert a panel size in
/// logical pixels to a (rows, cols) PTY size when the egui font
/// metrics aren't queryable yet (first paint). Conservative
/// estimates — the resize ioctl only matters after the user starts
/// running TUI apps.
const FALLBACK_CELL_W: f32 = 8.0;
const FALLBACK_CELL_H: f32 = 16.0;

/// Shape-match for the top-level sentinel value.
///
/// The Explorer's `paint_detail` calls this on the selected substrate
/// value; if it returns >0 we render the terminal panel instead of
/// the generic viewer cascade.
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

/// Per-panel state. Owned by [`super::Explorer`] (one per terminal
/// surface today; multi-tab is a follow-up that holds N of these).
///
/// Lifecycle:
/// 1. `Default::default()` — no session yet.
/// 2. First `paint` — fires `terminal.spawn`, stores the session id
///    once the reply lands.
/// 3. Subsequent paints — drains output reads, renders accumulated
///    text, dispatches resize / write.
/// 4. `Drop` — fires `terminal.close` (best-effort, no reply needed).
#[derive(Default)]
pub struct Terminal {
    /// Live session id once `terminal.spawn` has succeeded.
    session_id: Option<String>,
    /// Substrate path published by the daemon for this session's
    /// output. Set in the spawn reply alongside `session_id`.
    output_path: Option<String>,
    /// Resolved shell path (echoed by the daemon) — rendered in the
    /// header.
    shell: Option<String>,
    /// Pending `terminal.spawn` reply, while the spawn is in flight.
    spawn_pending: Option<ReplyRx>,
    /// Pending `substrate.read` reply for the most recent output poll.
    output_pending: Option<ReplyRx>,
    /// When the next output poll should fire. `None` until the first
    /// poll has fired (we fire one immediately after spawn).
    last_output_poll: Option<web_time::Instant>,
    /// Last tick we observed on a substrate.read for this output
    /// path. We only append the chunk's data when the tick advanced —
    /// substrate.read returns the latest value on every poll, so
    /// without this guard we'd duplicate every chunk on every tick.
    last_seen_tick: u64,
    /// Accumulated decoded output. Unbounded today; bounded when
    /// scrollback ring lands.
    output: String,
    /// One-line input buffer. Cleared on Enter.
    input: String,
    /// Last reported (rows, cols). Resize fires only when this changes.
    last_size: (u16, u16),
    /// Last error string (spawn failure, write failure, ...). Rendered
    /// in a small banner so the user sees *why* the terminal is dead.
    last_error: Option<String>,
}

impl Terminal {
    /// Paint the terminal surface inside `ui`. `live` is the RPC
    /// transport handle (used to fire `terminal.spawn` / `write` /
    /// `resize`).
    pub fn paint(&mut self, ui: &mut egui::Ui, live: &Arc<Live>) {
        // 1. Drain any in-flight RPC replies before painting so a
        //    just-completed spawn renders fresh state on this frame.
        self.drain_spawn_reply();
        self.drain_output_reply();

        // 2. Fire spawn lazily on first paint.
        if self.session_id.is_none() && self.spawn_pending.is_none() && self.last_error.is_none() {
            self.fire_spawn(live);
        }

        // 3. Header: shell + session id + status.
        self.paint_header(ui);

        if let Some(err) = &self.last_error {
            ui.colored_label(
                egui::Color32::from_rgb(220, 120, 120),
                format!("error: {err}"),
            );
            ui.separator();
        }

        // 4. Body: scrolling output + input field.
        let avail = ui.available_size();
        let input_height = 28.0;
        let body_height = (avail.y - input_height - 8.0).max(60.0);

        let scroll_id = ui.make_persistent_id("weft-terminal-output");
        egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .max_height(body_height)
            .show(ui, |ui| {
                // Monospace for legibility. We render bytes UTF-8-lossy;
                // ANSI sequences appear as literal `\u{1b}[...` until a
                // vte-based parser lands. Selectable so the user can
                // copy output.
                let text = if self.output.is_empty() {
                    "(connecting…)".to_string()
                } else {
                    self.output.clone()
                };
                ui.add(
                    egui::TextEdit::multiline(&mut text.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .interactive(false),
                );
            });

        ui.separator();

        // 5. Input line. Enter sends + clears.
        let input_response = ui
            .horizontal(|ui| {
                ui.label(egui::RichText::new("$").monospace());
                ui.add_sized(
                    [ui.available_width(), input_height - 4.0],
                    egui::TextEdit::singleline(&mut self.input)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("type a command, press Enter"),
                )
            })
            .inner;

        let pressed_enter = input_response.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if pressed_enter && !self.input.is_empty() {
            let mut line = std::mem::take(&mut self.input);
            line.push('\n');
            self.fire_write(live, line.as_bytes());
            // Refocus the input so the user can keep typing without
            // clicking back.
            input_response.request_focus();
        }

        // 6. Schedule next output poll.
        self.maybe_poll_output(live);

        // 7. Resize the PTY when the panel size implies a different
        //    cell grid. Skipped on the very first frame because
        //    `available_size` is the *post*-input available size; we
        //    estimate cells from total available_size, not body
        //    only.
        let cell_w = ui
            .fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), 'M'));
        let cell_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let cell_w = if cell_w > 0.0 { cell_w } else { FALLBACK_CELL_W };
        let cell_h = if cell_h > 0.0 { cell_h } else { FALLBACK_CELL_H };
        let cols = ((avail.x / cell_w).floor() as i32).clamp(20, 400) as u16;
        let rows = ((body_height / cell_h).floor() as i32).clamp(8, 200) as u16;
        if (rows, cols) != self.last_size {
            self.last_size = (rows, cols);
            self.fire_resize(live, rows, cols);
        }
    }

    fn paint_header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Terminal");
            ui.separator();
            match (&self.session_id, &self.shell) {
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

    fn fire_spawn(&mut self, live: &Arc<Live>) {
        let (tx, rx) = live::reply_channel();
        self.spawn_pending = Some(rx);
        live.submit(Command::Raw {
            method: "terminal.spawn".into(),
            // Initial geometry is a placeholder — we resize on the
            // very next frame once we know the panel size.
            params: serde_json::json!({
                "rows": 24,
                "cols": 80,
            }),
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
                    self.last_error =
                        Some("terminal.spawn reply missing session_id / output_path".into());
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
            live::TryReply::Done(Err(_err)) => {
                // Most likely "path not yet known" before the first
                // chunk lands. Don't surface to the user; the next
                // poll will retry.
                self.output_pending = None;
            }
            live::TryReply::Closed => {
                self.output_pending = None;
            }
            live::TryReply::Empty => { /* still in flight */ }
        }
    }

    /// Append the chunk's `data` (base64) to the accumulator if the
    /// `tick` is newer than what we've already seen.
    fn handle_output_value(&mut self, value: Value) {
        let tick = value.get("tick").and_then(Value::as_u64).unwrap_or(0);
        if tick != 0 && tick <= self.last_seen_tick {
            // Same value as last poll — nothing new to append.
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
            match base64::engine::general_purpose::STANDARD.decode(data_b64.as_bytes()) {
                Ok(bytes) => {
                    let s = String::from_utf8_lossy(&bytes);
                    self.output.push_str(&s);
                }
                Err(e) => {
                    self.last_error = Some(format!("base64 decode: {e}"));
                }
            }
        }
        if exit {
            self.output.push_str("\n[session exited]\n");
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

    /// Fire `terminal.close` if a session was alive. Called from
    /// [`Drop::drop`] and [`Self::close`].
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

    /// Explicit teardown — call from the Explorer when navigating
    /// away from the terminal sentinel so the daemon-side shell dies
    /// promptly rather than waiting for `Drop`.
    pub fn close(&mut self, live: &Arc<Live>) {
        self.fire_close(live);
        self.output_pending = None;
        self.spawn_pending = None;
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // `Drop` runs on the GUI thread; we don't have a Live handle
        // here so we can't fire close. The Explorer's `close()` /
        // `on_select` paths take the Live handle and call
        // [`Self::close`] explicitly. If `Drop` runs without that
        // having happened, the daemon-side session lives until the
        // daemon shuts down — acceptable since sessions don't accrue
        // unbounded background traffic on their own.
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn handle_output_value_appends_decoded_data() {
        let mut t = Terminal::default();
        // {"tick": 1, "value": { "data": base64("hi\n"), "ts_ms": 0 }}
        let chunk = json!({
            "tick": 1,
            "value": {
                "data": "aGkK",   // base64("hi\n")
                "ts_ms": 0,
            }
        });
        t.handle_output_value(chunk);
        assert_eq!(t.output, "hi\n");
        assert_eq!(t.last_seen_tick, 1);
    }

    #[test]
    fn handle_output_value_dedupes_on_same_tick() {
        let mut t = Terminal::default();
        let chunk = json!({
            "tick": 5,
            "value": { "data": "YQ==", "ts_ms": 0 }, // "a"
        });
        t.handle_output_value(chunk.clone());
        t.handle_output_value(chunk);
        assert_eq!(t.output, "a", "same tick must not double-append");
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
        assert!(t.output.contains("[session exited]"));
        assert!(t.session_id.is_none());
        assert!(t.output_path.is_none());
    }
}
