//! Chat-window panel for the Explorer.
//!
//! Renders a scrollable conversation against the daemon's `llm.prompt`
//! RPC. Triggers when the selected substrate value matches a chat
//! sentinel:
//!
//! ```json
//! { "kind": "chat", "model": "local" }
//! ```
//!
//! The daemon publishes one such sentinel at boot under
//! `substrate/<daemon-node>/ui/chat` so this panel has a stable mount
//! point in the substrate tree.
//!
//! ## Wire shape
//!
//! Each user turn fires `llm.prompt` with the **full** conversation so
//! far (system + every user/assistant turn). We don't trust the daemon
//! to preserve session state — the panel is the source of truth.
//!
//! ## Scope cuts (deliberate)
//!
//! - No streaming. The daemon ships V1 sync-only; a future
//!   `llm.prompt_stream` lands as a sibling RPC, not a breaking change
//!   here.
//! - No on-disk persistence. Conversation lives only as long as the
//!   panel does (close → cleared on next selection).
//! - No system-prompt UI. The struct carries an optional `system` field
//!   for tests and forward compat, but no `TextEdit` is wired yet.
//! - No model picker. The daemon decides which `llama-server` it talks
//!   to; the model name in the sentinel is informational.
//! - No markdown rendering. Plain text only — markdown is a follow-up.

use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::live::{self, Command, Live, ReplyRx};

/// Shape-match priority for the chat sentinel. Higher than
/// `control_toggle` (25) and `workshop` (30) so a chat sentinel
/// dispatched from the same selection slot wins decisively.
pub const PRIORITY: u32 = 40;

/// Sampling temperature passed to the LLM service. Conservative — chat
/// quality over creative drift; the daemon's own default also lives in
/// this range, so this is a soft repeat-of-default that lets the panel
/// override later without a daemon change.
const DEFAULT_TEMPERATURE: f32 = 0.4;

/// Hard cap on generated tokens per turn. Matches the daemon's default
/// (512); explicit here so a daemon-side bump doesn't silently change
/// chat-window behaviour.
const DEFAULT_MAX_TOKENS: u32 = 512;

/// Shape predicate. Returns [`PRIORITY`] when `value` looks like a
/// chat sentinel:
///
/// ```json
/// { "kind": "chat", ... }
/// ```
///
/// Strict on `kind == "chat"` — we don't probe other shapes so an
/// arbitrary object with a `model` key doesn't false-match.
pub fn matches(value: &Value) -> u32 {
    let Some(obj) = value.as_object() else {
        return 0;
    };
    let kind_ok = obj
        .get("kind")
        .and_then(Value::as_str)
        .map(|s| s == "chat")
        .unwrap_or(false);
    if kind_ok {
        PRIORITY
    } else {
        0
    }
}

/// One conversation turn. Mirrors `LlmPromptMessage` on the wire but
/// kept as a local type so the panel doesn't depend on `clawft-weave`
/// (which would pull tokio + the daemon-side world into the GUI crate
/// for free).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    /// Role: `system`, `user`, or `assistant`. `error` is a UI-only
    /// pseudo-role rendered as a red bubble; it's filtered out of the
    /// wire payload before sending.
    pub role: String,
    /// Message content.
    pub content: String,
}

impl ChatMessage {
    fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
    fn error(content: impl Into<String>) -> Self {
        Self {
            role: "error".into(),
            content: content.into(),
        }
    }
}

/// Panel state. Owned by the [`Explorer`](super::Explorer) and reset
/// whenever the selection moves (via `Explorer::on_select`) so a stale
/// conversation doesn't reappear when the user re-selects the chat
/// sentinel after navigating elsewhere.
#[derive(Default)]
pub struct ChatView {
    /// Optional system prompt prepended to every wire payload. No UI
    /// to set this yet — accepted as a forward-compat field so the
    /// state machine can be tested today.
    pub system: Option<String>,
    /// Full conversation. Wire payload always starts here (with `system`
    /// prepended if set) — the daemon does not preserve session state.
    pub history: Vec<ChatMessage>,
    /// Draft input text bound to the multiline `TextEdit`.
    pub draft: String,
    /// Pending `llm.prompt` reply channel. `Some` while a request is
    /// in flight; the input + Send button disable in that window so the
    /// user can't fire a second request against `llama-server`'s single
    /// in-flight slot (the service crate's semaphore would serialize
    /// them anyway, but UI feedback is clearer).
    pending: Option<ReplyRx>,
}

impl ChatView {
    /// Whether a request is currently in flight.
    pub fn is_in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// Build the JSON params for `agent.chat` from the current history.
    ///
    /// Matches `AgentChatParams { messages, temperature, max_tokens }`.
    /// The `system` field on `LlmPromptParams` is intentionally absent —
    /// the daemon-side concierge owns the system prompt (identity +
    /// workspace + tool intro) and we don't let the panel inject one.
    ///
    /// Filters UI-only `error` pseudo-roles out of the wire payload.
    pub fn build_request_params(&self, next_user: &str) -> Value {
        let mut messages: Vec<Value> = self
            .history
            .iter()
            .filter(|m| m.role != "error")
            .map(|m| {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                })
            })
            .collect();
        messages.push(serde_json::json!({
            "role": "user",
            "content": next_user,
        }));
        let mut obj = serde_json::Map::new();
        obj.insert("messages".into(), Value::Array(messages));
        obj.insert(
            "temperature".into(),
            serde_json::json!(DEFAULT_TEMPERATURE),
        );
        obj.insert(
            "max_tokens".into(),
            serde_json::json!(DEFAULT_MAX_TOKENS),
        );
        Value::Object(obj)
    }

    /// State-machine entry: a successful `agent.chat` response landed.
    /// Appends the assistant text to history and clears the in-flight
    /// flag. Pure function over [`Value`] so tests can drive it without
    /// any RPC plumbing.
    ///
    /// Tool-call rendering as collapsible bubbles lands in commit 9
    /// (plan §11.4); the spike just shows the final assistant text.
    /// Both old `{ completion: "..." }` and new
    /// `{ assistant_text: "..." }` shapes are accepted to make rolling
    /// the wasm bundle and the daemon independently safe.
    pub fn on_response_ok(&mut self, response: &Value) {
        let text = response
            .get("assistant_text")
            .or_else(|| response.get("completion"))
            .and_then(Value::as_str)
            .unwrap_or("(empty completion)");
        self.history
            .push(ChatMessage::assistant(text.to_string()));
        self.pending = None;
    }

    /// State-machine entry: an `llm.prompt` request failed. Appends
    /// a UI-only `error` bubble and clears the in-flight flag.
    pub fn on_response_err(&mut self, err: &str) {
        self.history.push(ChatMessage::error(err.to_string()));
        self.pending = None;
    }

    /// Drain the pending reply channel. Called once per paint before
    /// rendering so the UI always reflects the freshest server state.
    fn drain_reply(&mut self) {
        let Some(rx) = self.pending.as_mut() else {
            return;
        };
        match live::try_recv_reply(rx) {
            live::TryReply::Done(Ok(value)) => self.on_response_ok(&value),
            live::TryReply::Done(Err(err)) => self.on_response_err(&err),
            live::TryReply::Closed => {
                self.on_response_err("transport closed before reply");
            }
            live::TryReply::Empty => { /* still in flight */ }
        }
    }

    /// Submit the current `draft` as a user turn and fire the RPC.
    /// No-op if the draft is empty/whitespace or a request is already
    /// in flight.
    fn submit_draft(&mut self, live: &Arc<Live>) {
        if self.is_in_flight() {
            return;
        }
        let text = self.draft.trim().to_string();
        if text.is_empty() {
            return;
        }
        let params = self.build_request_params(&text);
        self.history.push(ChatMessage::user(text));
        self.draft.clear();

        let (tx, rx) = live::reply_channel();
        self.pending = Some(rx);
        live.submit(Command::Raw {
            method: "agent.chat".into(),
            params,
            reply: Some(tx),
        });
    }
}

/// Render the chat panel. `path` is the substrate sentinel path (used
/// only for the muted footer hint). `value` is the sentinel value
/// (used only to surface the model name).
pub fn paint(ui: &mut egui::Ui, path: &str, value: &Value, view: &mut ChatView, live: &Arc<Live>) {
    view.drain_reply();

    let model = value
        .as_object()
        .and_then(|o| o.get("model"))
        .and_then(Value::as_str)
        .unwrap_or("local");

    ui.label(
        egui::RichText::new(format!("chat · {model}"))
            .color(egui::Color32::from_rgb(160, 160, 170))
            .small(),
    );
    ui.add_space(2.0);
    ui.heading("Local LLM");
    ui.add_space(6.0);

    // Reserve room for the input area at the bottom so the scroll area
    // doesn't fight it for vertical space.
    let input_h = 96.0;
    let history_h = (ui.available_height() - input_h).max(120.0);

    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .max_height(history_h)
                .show(ui, |ui| {
                    if view.history.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(24.0);
                            ui.label(
                                egui::RichText::new(
                                    "(no messages yet — type below to start)",
                                )
                                .italics()
                                .color(egui::Color32::from_rgb(160, 160, 170)),
                            );
                        });
                    }
                    for msg in &view.history {
                        paint_bubble(ui, msg);
                    }
                    if view.is_in_flight() {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(
                                egui::RichText::new("waiting for completion…")
                                    .italics()
                                    .color(egui::Color32::from_rgb(170, 170, 180)),
                            );
                        });
                    }
                });
        });

    ui.add_space(6.0);

    // Input row. Disabled while a request is in flight; Enter submits,
    // Shift+Enter inserts a newline (egui's default for multiline +
    // explicit `desired_rows`).
    ui.add_enabled_ui(!view.is_in_flight(), |ui| {
        let response = ui.add(
            egui::TextEdit::multiline(&mut view.draft)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Type a message — Enter to send, Shift+Enter for newline"),
        );

        let enter_pressed = response.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

        ui.horizontal(|ui| {
            let send_clicked = ui.button("Send").clicked();
            if (send_clicked || enter_pressed)
                && !view.draft.trim().is_empty()
            {
                view.submit_draft(live);
            }
            if !view.history.is_empty()
                && ui.button("Clear").clicked()
                && !view.is_in_flight()
            {
                view.history.clear();
            }
        });
    });

    ui.add_space(4.0);
    ui.separator();
    ui.label(
        egui::RichText::new(format!("path: {path}"))
            .small()
            .monospace()
            .color(egui::Color32::from_rgb(140, 140, 150)),
    );
}

/// Paint one history entry as a chat bubble. Roles render distinctly:
///
/// - `system`   — dim italic, full width (informational; not a turn).
/// - `user`     — right-aligned, subtle bg.
/// - `assistant`— left-aligned, subtle bg.
/// - `error`    — left-aligned, red bg + red strong text. UI-only role.
fn paint_bubble(ui: &mut egui::Ui, msg: &ChatMessage) {
    match msg.role.as_str() {
        "system" => {
            ui.label(
                egui::RichText::new(format!("[system] {}", msg.content))
                    .italics()
                    .small()
                    .color(egui::Color32::from_rgb(150, 150, 160)),
            );
            ui.add_space(2.0);
        }
        "user" => {
            ui.with_layout(
                egui::Layout::top_down(egui::Align::RIGHT),
                |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(50, 60, 80))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&msg.content)
                                    .color(egui::Color32::from_rgb(225, 230, 240)),
                            );
                        });
                },
            );
            ui.add_space(4.0);
        }
        "assistant" => {
            ui.with_layout(
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(40, 50, 50))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&msg.content)
                                    .color(egui::Color32::from_rgb(220, 230, 220)),
                            );
                        });
                },
            );
            ui.add_space(4.0);
        }
        "error" => {
            ui.with_layout(
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(70, 30, 30))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!("error: {}", msg.content))
                                    .strong()
                                    .color(egui::Color32::from_rgb(240, 170, 170)),
                            );
                        });
                },
            );
            ui.add_space(4.0);
        }
        other => {
            // Unknown role — render as plain text so we never silently
            // drop content if the wire shape grows.
            ui.label(format!("[{other}] {}", msg.content));
            ui.add_space(2.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_returns_positive_for_chat_sentinel() {
        let v = json!({ "kind": "chat", "model": "local" });
        assert_eq!(matches(&v), PRIORITY);
    }

    #[test]
    fn matches_returns_positive_without_model_field() {
        // Sentinel must work even if the daemon hasn't filled in the
        // optional `model` field — `kind` alone is the contract.
        let v = json!({ "kind": "chat" });
        assert_eq!(matches(&v), PRIORITY);
    }

    #[test]
    fn matches_returns_zero_for_other_shapes() {
        // Control intent — different `kind`.
        let control = json!({
            "enabled": true,
            "kind": "service",
            "target": "llm",
        });
        assert_eq!(matches(&control), 0);

        // Workshop — no `kind` at all.
        let workshop = json!({
            "title": "x",
            "panels": [],
        });
        assert_eq!(matches(&workshop), 0);

        // Plain scalars + arrays.
        assert_eq!(matches(&Value::Null), 0);
        assert_eq!(matches(&json!(42)), 0);
        assert_eq!(matches(&json!("chat")), 0);
        assert_eq!(matches(&json!([1, 2, 3])), 0);

        // String `kind` but wrong value.
        assert_eq!(matches(&json!({ "kind": "Chat" })), 0);
        assert_eq!(matches(&json!({ "kind": "chatroom" })), 0);

        // Non-string `kind`.
        assert_eq!(matches(&json!({ "kind": 7 })), 0);
    }

    #[test]
    fn priority_beats_control_toggle_and_workshop() {
        // Sanity that the dispatch order in `explorer/mod.rs` lands on
        // chat first when a value (somehow) shape-matches multiple
        // primitives.
        assert!(PRIORITY > 30);
        assert!(PRIORITY > 25);
    }

    #[test]
    fn serializes_messages_to_expected_wire_shape() {
        let mut view = ChatView::default();
        view.history.push(ChatMessage::user("hi"));
        view.history.push(ChatMessage::assistant("hello"));
        // Error bubbles must NOT show up on the wire.
        view.history.push(ChatMessage::error("transport blew up"));

        let params = view.build_request_params("how are you?");
        let messages = params
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages key");

        assert_eq!(messages.len(), 3, "error role filtered, new user appended");
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "hi");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "hello");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "how are you?");

        // Required tuning knobs land in the params with the panel's
        // explicit defaults so a daemon-side default change can't drift
        // chat behaviour silently.
        assert!(params.get("temperature").is_some());
        assert!(params.get("max_tokens").is_some());
        // No `prompt` field — we always send `messages`.
        assert!(params.get("prompt").is_none());
        // No `system` unless explicitly set.
        assert!(params.get("system").is_none());
    }

    #[test]
    fn serializes_messages_omits_system_field_for_agent_chat() {
        // The daemon-side concierge owns the system prompt now; the
        // panel's `system` field is intentionally not sent on the wire
        // even when set, to prevent panel-side identity injection.
        let mut view = ChatView {
            system: Some("you are concise".into()),
            ..Default::default()
        };
        view.history.push(ChatMessage::user("hi"));
        let params = view.build_request_params("again");
        assert!(params.get("system").is_none());
    }

    #[test]
    fn ok_response_accepts_assistant_text_field() {
        // `agent.chat` returns `assistant_text`; the panel must accept it
        // alongside the legacy `completion` field for cross-rev safety.
        let mut view = ChatView::default();
        view.history.push(ChatMessage::user("hi"));
        let response = json!({
            "assistant_text": "hello from the concierge",
            "tool_calls": [],
            "finish_reason": "stop",
            "iterations": 1,
            "prompt_tokens": 12,
            "completion_tokens": 5,
            "model": "local",
            "identity_source": "clawft",
        });
        view.on_response_ok(&response);
        assert_eq!(view.history.len(), 2);
        assert_eq!(view.history[1].content, "hello from the concierge");
    }

    #[test]
    fn appends_assistant_message_on_ok_response() {
        let mut view = ChatView::default();
        view.history.push(ChatMessage::user("hi"));

        // Mock the wire shape an `llm.prompt` success returns: the
        // `Response::success` body is the `LlmPromptResult` JSON.
        let response = json!({
            "completion": "hello back",
            "finish_reason": "stop",
            "prompt_tokens": 5,
            "completion_tokens": 3,
            "model": "local",
        });
        view.on_response_ok(&response);

        assert_eq!(view.history.len(), 2);
        assert_eq!(view.history[1].role, "assistant");
        assert_eq!(view.history[1].content, "hello back");
        assert!(!view.is_in_flight());
    }

    #[test]
    fn ok_response_with_missing_completion_yields_placeholder() {
        let mut view = ChatView::default();
        view.on_response_ok(&json!({ "completion_tokens": 0 }));
        assert_eq!(view.history.len(), 1);
        assert_eq!(view.history[0].role, "assistant");
        assert!(view.history[0].content.contains("empty"));
    }

    #[test]
    fn appends_error_bubble_on_err_response() {
        let mut view = ChatView::default();
        view.history.push(ChatMessage::user("hi"));
        view.on_response_err("llm.prompt: llm service is disabled");

        assert_eq!(view.history.len(), 2);
        assert_eq!(view.history[1].role, "error");
        assert!(
            view.history[1]
                .content
                .contains("llm service is disabled")
        );
        assert!(!view.is_in_flight());
    }
}
