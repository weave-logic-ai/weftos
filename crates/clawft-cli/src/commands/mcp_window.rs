//! WindowIntent MCP tools (ADR-075 G3 / WEFT-695, ADR-076 C4 / WEFT-702).
//!
//! Public wire tools (`window_spawn`, `window_focus`, `window_arrange`,
//! `window_close`, `window_summarize`, `window_attach`) map onto the shared
//! [`WindowIntent`] enum and submit via [`ExternalIntentTx::submit_mcp`].
//!
//! No Grok-specific layout paths: MCP is just another producer on the same
//! bus as keyboard, GUI chrome, and voice (ADR-073).
//!
//! When no shell is attached, intents still serialize and enqueue on an
//! in-process channel so unit tests and headless `weft mcp-server` can
//! exercise the wire (stub / log-ready). A live Agent Workspace drains the
//! same channel via [`ExternalIntentSource`].

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use clawft_services::mcp::ToolDefinition;
use clawft_services::mcp::provider::{CallToolResult, ToolError, ToolProvider};
use clawft_types::window_intent::{
    ArrangeMode, ExternalIntentSource, ExternalIntentTx, IntentEnvelope, IntentSource, PaneId,
    PaneKind, WindowIntent, WindowIntentProducer,
};
use serde_json::{Value, json};
use tracing::info;

// ── Public MCP tool names (catalog / ADR-076) ─────────────────────────

/// Public MCP tool names for the WindowIntent surface.
pub const WINDOW_TOOL_NAMES: &[&str] = &[
    "window_spawn",
    "window_focus",
    "window_arrange",
    "window_close",
    "window_summarize",
    "window_attach",
];

// ── Intent sink ───────────────────────────────────────────────────────

/// Where window tools deliver accepted intents.
///
/// Production path: [`ExternalIntentTx`] shared with the Agent Workspace.
/// Tests inject a recorder.
pub trait WindowIntentSink: Send + Sync {
    fn submit_mcp(&self, intent: WindowIntent) -> Result<(), String>;
}

impl WindowIntentSink for ExternalIntentTx {
    fn submit_mcp(&self, intent: WindowIntent) -> Result<(), String> {
        ExternalIntentTx::submit_mcp(self, intent)
    }
}

/// In-process sink that also retains a drainable source for tests / headless.
pub struct LocalWindowIntentBus {
    tx: ExternalIntentTx,
    /// Shared receive half — polled by tests or a future GUI attach bridge.
    #[allow(dead_code)] // read only by `poll`, which the non-test binary does not call yet
    source: Mutex<ExternalIntentSource>,
}

impl LocalWindowIntentBus {
    pub fn new() -> Self {
        let (tx, source) = ExternalIntentSource::channel();
        Self {
            tx,
            source: Mutex::new(source),
        }
    }

    /// Clone of the send half (for wiring into a live workspace later).
    #[allow(dead_code)] // public attach API for GUI co-process / future --attach-ui
    pub fn tx(&self) -> ExternalIntentTx {
        self.tx.clone()
    }

    /// Drain all intents submitted since the last poll (FIFO).
    #[allow(dead_code)] // test / headless drain; the GUI attach bridge will call it
    pub fn poll(&self) -> Vec<IntentEnvelope> {
        self.source
            .lock()
            .expect("window intent source lock")
            .poll_intents()
    }
}

impl Default for LocalWindowIntentBus {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowIntentSink for LocalWindowIntentBus {
    fn submit_mcp(&self, intent: WindowIntent) -> Result<(), String> {
        self.tx.submit_mcp(intent)
    }
}

// ── Arg → WindowIntent mapping ────────────────────────────────────────

/// Map a public `window_*` tool name + JSON args to a [`WindowIntent`].
pub fn map_window_tool(name: &str, args: &Value) -> Result<WindowIntent, String> {
    match name {
        "window_spawn" => map_spawn(args),
        "window_focus" => {
            let id = require_pane_id(args, "id")?;
            Ok(WindowIntent::Focus { id })
        }
        "window_arrange" => {
            let mode = parse_arrange_mode(args)?;
            Ok(WindowIntent::Arrange { mode })
        }
        "window_close" => {
            let id = require_pane_id(args, "id")?;
            let cancel_agent = args
                .get("cancel_agent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(WindowIntent::Close { id, cancel_agent })
        }
        "window_summarize" => {
            let id = require_pane_id(args, "id")?;
            let max_chars = args
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            Ok(WindowIntent::Summarize { id, max_chars })
        }
        "window_attach" => {
            let tool_id = require_pane_id(args, "tool_id")?;
            let agent_id = require_pane_id(args, "agent_id")?;
            Ok(WindowIntent::Attach { tool_id, agent_id })
        }
        other => Err(format!("unknown window tool '{other}'")),
    }
}

fn require_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing or empty required argument '{key}'"))
}

fn require_pane_id(args: &Value, key: &str) -> Result<PaneId, String> {
    Ok(PaneId::new(require_str(args, key)?))
}

fn parse_arrange_mode(args: &Value) -> Result<ArrangeMode, String> {
    let raw = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("grid")
        .to_ascii_lowercase();
    match raw.as_str() {
        "grid" => Ok(ArrangeMode::Grid),
        "cascade" => Ok(ArrangeMode::Cascade),
        "stack" => Ok(ArrangeMode::Stack),
        other => Err(format!(
            "invalid arrange mode '{other}'; expected grid|cascade|stack"
        )),
    }
}

fn map_spawn(args: &Value) -> Result<WindowIntent, String> {
    let kind_raw = args
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_ascii_lowercase();

    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(PaneId::new);

    let kind = match kind_raw.as_str() {
        "app" => {
            let app = require_str(args, "app")?;
            PaneKind::App { app }
        }
        "tool" => {
            let tool = require_str(args, "tool")?;
            let attached_to = args
                .get("attached_to")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            PaneKind::Tool { tool, attached_to }
        }
        "agent" => {
            let role = args
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("agent")
                .to_string();
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(role.as_str())
                .to_string();
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let background = args
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            PaneKind::Agent {
                session_id,
                role,
                title,
                background,
            }
        }
        other => {
            return Err(format!(
                "invalid spawn kind '{other}'; expected agent|app|tool"
            ));
        }
    };

    Ok(WindowIntent::Spawn { kind, id })
}

// ── Tool definitions ──────────────────────────────────────────────────

/// Catalog public tool definitions for the window surface.
pub fn window_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "window_spawn".into(),
            description: "Spawn a pane on the Agent Workspace (WindowIntent::Spawn). \
                Default kind is a visible agent pane. Same bus as keyboard/voice/GUI \
                (ADR-073). Session-bound when a shell is attached."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["agent", "app", "tool"],
                        "description": "Pane kind (default: agent)"
                    },
                    "id": {
                        "type": "string",
                        "description": "Optional stable pane id"
                    },
                    "role": {
                        "type": "string",
                        "description": "Agent role (agent kind)"
                    },
                    "title": {
                        "type": "string",
                        "description": "Pane title (agent kind)"
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Agent session id (filled by host when empty)"
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Background-only spawn (default false = visible)",
                        "default": false
                    },
                    "app": {
                        "type": "string",
                        "description": "App name when kind=app (e.g. terminal, chat)"
                    },
                    "tool": {
                        "type": "string",
                        "description": "Tool surface name when kind=tool"
                    },
                    "attached_to": {
                        "type": "string",
                        "description": "Optional agent pane id when kind=tool"
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "window_focus".into(),
            description: "Focus a pane on the Agent Workspace (WindowIntent::Focus)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Pane id to bring to front"
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "window_arrange".into(),
            description: "Arrange all panes (WindowIntent::Arrange). mode: grid|cascade|stack."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["grid", "cascade", "stack"],
                        "description": "Arrange strategy (default: grid)"
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "window_close".into(),
            description: "Close a pane (WindowIntent::Close)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Pane id to close"
                    },
                    "cancel_agent": {
                        "type": "boolean",
                        "description": "Also cancel the bound agent (default false)",
                        "default": false
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "window_summarize".into(),
            description: "Request a short UI/spoken summary for a pane \
                (WindowIntent::Summarize)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Pane id to summarize"
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Optional max characters for the summary"
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: "window_attach".into(),
            description: "Bind a tool pane to an agent pane (WindowIntent::Attach)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tool_id": {
                        "type": "string",
                        "description": "Tool pane id"
                    },
                    "agent_id": {
                        "type": "string",
                        "description": "Agent pane id"
                    }
                },
                "required": ["tool_id", "agent_id"],
                "additionalProperties": false
            }),
        },
    ]
}

/// Filter window tool defs by serve profile (control / default / full).
pub fn filter_window_tools_by_profile(
    tools: Vec<ToolDefinition>,
    allows: impl Fn(&str) -> bool,
) -> Vec<ToolDefinition> {
    tools.into_iter().filter(|t| allows(&t.name)).collect()
}

// ── Tool provider ─────────────────────────────────────────────────────

/// MCP [`ToolProvider`] that maps `window_*` tools onto [`WindowIntent`].
pub struct WindowIntentToolProvider {
    tools: Vec<ToolDefinition>,
    sink: Arc<dyn WindowIntentSink>,
}

impl WindowIntentToolProvider {
    pub fn new(sink: Arc<dyn WindowIntentSink>, tools: Vec<ToolDefinition>) -> Self {
        Self { tools, sink }
    }

    /// Full window surface filtered by a profile allow predicate.
    pub fn with_profile(
        sink: Arc<dyn WindowIntentSink>,
        allows: impl Fn(&str) -> bool,
    ) -> Self {
        let tools = filter_window_tools_by_profile(window_tool_definitions(), allows);
        Self::new(sink, tools)
    }

    /// Headless / standalone: local bus sink + profile filter.
    pub fn local_with_profile(allows: impl Fn(&str) -> bool) -> (Self, Arc<LocalWindowIntentBus>) {
        let bus = Arc::new(LocalWindowIntentBus::new());
        let sink: Arc<dyn WindowIntentSink> = bus.clone();
        let provider = Self::with_profile(sink, allows);
        (provider, bus)
    }
}

impl std::fmt::Debug for WindowIntentToolProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowIntentToolProvider")
            .field("tool_count", &self.tools.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolProvider for WindowIntentToolProvider {
    fn namespace(&self) -> &str {
        // Empty → CompositeToolProvider lists public names (ADR-076 §3).
        ""
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.tools.clone()
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<CallToolResult, ToolError> {
        if !self.tools.iter().any(|t| t.name == name) {
            return Err(ToolError::NotFound(name.to_string()));
        }

        let intent = match map_window_tool(name, &args) {
            Ok(i) => i,
            Err(msg) => {
                return Ok(CallToolResult::error(msg));
            }
        };

        match self.sink.submit_mcp(intent.clone()) {
            Ok(()) => {
                let body = json!({
                    "ok": true,
                    "tool": name,
                    "source": IntentSource::Mcp,
                    "op": intent.op_name(),
                    "intent": intent,
                    "note": "submitted to WindowIntent bus (MCP source); \
                             Agent Workspace applies when attached"
                });
                let text = serde_json::to_string_pretty(&body)
                    .unwrap_or_else(|_| body.to_string());
                info!(tool = name, op = intent.op_name(), "window intent submitted via MCP");
                Ok(CallToolResult::text(text))
            }
            Err(e) => Ok(CallToolResult::error(format!(
                "failed to submit WindowIntent: {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_tools_are_control_profile() {
        let p = crate::commands::mcp_profile::ProfileSet::parse("control").unwrap();
        for name in WINDOW_TOOL_NAMES {
            assert!(
                p.allows_tool(name),
                "control profile must allow `{name}` (ADR-076 C4)"
            );
        }
        let default = crate::commands::mcp_profile::ProfileSet::product_default();
        for name in WINDOW_TOOL_NAMES {
            assert!(
                default.allows_tool(name),
                "default profile must allow `{name}` (control ∪ workspace)"
            );
        }
        let media = crate::commands::mcp_profile::ProfileSet::parse("media").unwrap();
        for name in WINDOW_TOOL_NAMES {
            assert!(
                !media.allows_tool(name),
                "media-only profile must not include `{name}`"
            );
        }
    }

    #[test]
    fn media_profile_gates_voice_and_render() {
        // WEFT-702: media required for voice_*/render_ui; not on default.
        let default = crate::commands::mcp_profile::ProfileSet::product_default();
        for name in [
            "voice_listen",
            "voice_speak",
            "audio_transcribe",
            "audio_synthesize",
            "render_ui",
        ] {
            assert!(
                !default.allows_tool(name),
                "default must exclude media tool `{name}`"
            );
        }
        let media = crate::commands::mcp_profile::ProfileSet::parse("media").unwrap();
        for name in [
            "voice_listen",
            "voice_speak",
            "audio_transcribe",
            "render_ui",
        ] {
            assert!(
                media.allows_tool(name),
                "media profile must allow `{name}`"
            );
        }
        let composed =
            crate::commands::mcp_profile::ProfileSet::parse("control,media").unwrap();
        assert!(composed.allows_tool("window_spawn"));
        assert!(composed.allows_tool("voice_listen"));
        assert!(!composed.allows_tool("read_file"));
    }

    #[test]
    fn map_spawn_agent_defaults_visible() {
        let intent = map_window_tool(
            "window_spawn",
            &json!({
                "role": "coder",
                "title": "Coder-1"
            }),
        )
        .unwrap();
        match intent {
            WindowIntent::Spawn {
                kind: PaneKind::Agent {
                    role,
                    title,
                    background,
                    ..
                },
                ..
            } => {
                assert_eq!(role, "coder");
                assert_eq!(title, "Coder-1");
                assert!(!background);
            }
            other => panic!("expected agent spawn, got {other:?}"),
        }
    }

    #[test]
    fn map_all_verbs() {
        assert!(matches!(
            map_window_tool("window_focus", &json!({"id": "p1"})).unwrap(),
            WindowIntent::Focus { .. }
        ));
        assert!(matches!(
            map_window_tool("window_arrange", &json!({"mode": "cascade"})).unwrap(),
            WindowIntent::Arrange {
                mode: ArrangeMode::Cascade
            }
        ));
        assert!(matches!(
            map_window_tool("window_close", &json!({"id": "p1", "cancel_agent": true})).unwrap(),
            WindowIntent::Close {
                cancel_agent: true,
                ..
            }
        ));
        assert!(matches!(
            map_window_tool("window_summarize", &json!({"id": "p1", "max_chars": 200})).unwrap(),
            WindowIntent::Summarize {
                max_chars: Some(200),
                ..
            }
        ));
        assert!(matches!(
            map_window_tool(
                "window_attach",
                &json!({"tool_id": "t1", "agent_id": "a1"})
            )
            .unwrap(),
            WindowIntent::Attach { .. }
        ));
    }

    #[test]
    fn map_rejects_bad_arrange() {
        let err = map_window_tool("window_arrange", &json!({"mode": "spiral"})).unwrap_err();
        assert!(err.contains("invalid arrange mode"));
    }

    #[tokio::test]
    async fn provider_submits_to_bus_and_lists_tools() {
        let bus = Arc::new(LocalWindowIntentBus::new());
        let provider = WindowIntentToolProvider::with_profile(bus.clone(), |_| true);
        let listed = provider.list_tools();
        let names: Vec<_> = listed.iter().map(|t| t.name.as_str()).collect();
        for expected in WINDOW_TOOL_NAMES {
            assert!(names.contains(expected), "missing {expected}");
        }

        let r = provider
            .call_tool(
                "window_spawn",
                json!({
                    "role": "researcher",
                    "title": "R1",
                    "id": "pane-r1"
                }),
            )
            .await
            .unwrap();
        assert!(!r.is_error);

        let r2 = provider
            .call_tool(
                "window_spawn",
                json!({
                    "role": "coder",
                    "title": "C1",
                    "id": "pane-c1"
                }),
            )
            .await
            .unwrap();
        assert!(!r2.is_error);

        let polled = bus.poll();
        assert_eq!(polled.len(), 2);
        assert_eq!(polled[0].source, IntentSource::Mcp);
        assert!(matches!(polled[0].intent, WindowIntent::Spawn { .. }));
        assert!(matches!(polled[1].intent, WindowIntent::Spawn { .. }));
    }

    #[tokio::test]
    async fn provider_profile_filters_out_when_not_allowed() {
        let (provider, _) = WindowIntentToolProvider::local_with_profile(|_| false);
        assert!(provider.list_tools().is_empty());
        let err = provider
            .call_tool("window_spawn", json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
    }

    #[test]
    fn serde_intent_matches_shared_type() {
        // Round-trip through the shared type so GUI + MCP stay aligned.
        let intent = map_window_tool(
            "window_spawn",
            &json!({
                "kind": "agent",
                "role": "coder",
                "title": "C",
                "id": "x"
            }),
        )
        .unwrap();
        let s = serde_json::to_string(&intent).unwrap();
        let back: WindowIntent = serde_json::from_str(&s).unwrap();
        assert_eq!(intent, back);
        assert_eq!(intent.op_name(), "spawn");
    }
}
