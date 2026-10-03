//! `WindowIntent` bus — one typed control surface for voice, keys, MCP, and GUI.
//!
//! ADR-073 Phase D (WEFT-688). All layout/control verbs share this enum so
//! voice is not a special-case egui hack. Producers push intents; the
//! Agent Workspace (or stock desktop bridge) applies them.
//!
//! Shared in `clawft-types` so MCP tools (WEFT-695 / WEFT-702) and the GUI
//! shell emit the **same** serde-stable shape via
//! [`ExternalIntentTx::submit_mcp`].

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use serde::{Deserialize, Serialize};

/// Stable id for a freeform pane on the Agent Workspace stage.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub String);

impl PaneId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a pane hosts on the freeform stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PaneKind {
    /// Stock desktop app surface (sidebar target name, e.g. `"terminal"`).
    App { app: String },
    /// Long-running agent session (visible-agent rule — ADR-073 §2).
    Agent {
        session_id: String,
        role: String,
        title: String,
        /// When true, spawn is background-only (explicit opt-in). Default false.
        #[serde(default)]
        background: bool,
    },
    /// Tool surface bound to an agent (terminal / browser / log).
    Tool {
        tool: String,
        #[serde(default)]
        attached_to: Option<String>,
    },
}

/// Arrange strategy for the freeform stage.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrangeMode {
    Grid,
    Cascade,
    Stack,
}

/// One layout/control verb. Equal sources: keyboard, voice, MCP, GUI chrome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WindowIntent {
    /// Open a pane. Agent panes open by default (visible-agent rule).
    Spawn {
        kind: PaneKind,
        #[serde(default)]
        id: Option<PaneId>,
    },
    /// Bring pane to front / select.
    Focus { id: PaneId },
    /// Rearrange all panes.
    Arrange { mode: ArrangeMode },
    /// Close pane (+ optional agent cancel — cancel is a future hook).
    Close {
        id: PaneId,
        #[serde(default)]
        cancel_agent: bool,
    },
    /// Compress agent wall-of-text into short UI (and later spoken) form.
    Summarize {
        id: PaneId,
        #[serde(default)]
        max_chars: Option<usize>,
    },
    /// Bind a tool pane to an agent pane.
    Attach {
        tool_id: PaneId,
        agent_id: PaneId,
    },
}

impl WindowIntent {
    /// Short verb label for logs / MCP responses (no secrets).
    pub fn op_name(&self) -> &'static str {
        match self {
            Self::Spawn { .. } => "spawn",
            Self::Focus { .. } => "focus",
            Self::Arrange { .. } => "arrange",
            Self::Close { .. } => "close",
            Self::Summarize { .. } => "summarize",
            Self::Attach { .. } => "attach",
        }
    }
}

/// Source tag for diagnostics / rate-limit policies.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentSource {
    Keyboard,
    Gui,
    Voice,
    Mcp,
    System,
}

/// Envelope with provenance. The bus stores plain intents; producers may
/// attach source when logging.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntentEnvelope {
    pub intent: WindowIntent,
    pub source: IntentSource,
}

/// In-process intent queue. GUI/keyboard push here each frame; external
/// producers (MCP, voice) push via [`ExternalIntentSource`].
#[derive(Default)]
pub struct WindowIntentBus {
    queue: VecDeque<IntentEnvelope>,
}

impl WindowIntentBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, intent: WindowIntent, source: IntentSource) {
        self.queue.push_back(IntentEnvelope { intent, source });
    }

    pub fn push_intent(&mut self, intent: WindowIntent) {
        self.push(intent, IntentSource::System);
    }

    /// Drain all pending envelopes in FIFO order.
    pub fn drain(&mut self) -> Vec<IntentEnvelope> {
        self.queue.drain(..).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

/// Trait for non-UI producers (MCP server, voice bridge). Implementations
/// poll or receive intents and enqueue them onto a shared bus.
///
/// WEFT-688: MCP-ready stub — real MCP tools (WEFT-695) and voice S2S
/// (WEFT-690) attach by holding a [`ExternalIntentTx`] and calling
/// [`ExternalIntentTx::submit`].
pub trait WindowIntentProducer {
    fn poll_intents(&mut self) -> Vec<IntentEnvelope>;
}

/// Send half of an external intent channel. Clone-friendly for MCP handlers.
#[derive(Clone)]
pub struct ExternalIntentTx {
    tx: Sender<IntentEnvelope>,
}

impl ExternalIntentTx {
    pub fn submit(&self, intent: WindowIntent, source: IntentSource) -> Result<(), String> {
        self.tx
            .send(IntentEnvelope { intent, source })
            .map_err(|e| e.to_string())
    }

    /// Convenience: submit as MCP.
    pub fn submit_mcp(&self, intent: WindowIntent) -> Result<(), String> {
        self.submit(intent, IntentSource::Mcp)
    }

    /// Convenience: submit as voice.
    pub fn submit_voice(&self, intent: WindowIntent) -> Result<(), String> {
        self.submit(intent, IntentSource::Voice)
    }
}

/// Receive half — polled each frame by the workspace.
pub struct ExternalIntentSource {
    rx: Receiver<IntentEnvelope>,
}

impl ExternalIntentSource {
    pub fn channel() -> (ExternalIntentTx, Self) {
        let (tx, rx) = mpsc::channel();
        (ExternalIntentTx { tx }, Self { rx })
    }
}

impl WindowIntentProducer for ExternalIntentSource {
    fn poll_intents(&mut self) -> Vec<IntentEnvelope> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(env) => out.push(env),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

/// Map a keyboard chord (modifiers + key name) to an optional intent.
/// Returns `None` when the chord is not a workspace verb.
///
/// Chord table (WEFT-688 keyboard producer):
/// - `Mod+Shift+G` → Arrange Grid
/// - `Mod+Shift+C` → Arrange Cascade
/// - `Mod+Shift+S` → Arrange Stack
/// - `Mod+W`       → Close focused (caller supplies id)
/// - `Mod+Shift+N` → Spawn demo agent (caller fills kind)
pub fn intent_from_key(
    modifiers: KeyModifiers,
    key: &str,
    focused: Option<&PaneId>,
) -> Option<WindowIntent> {
    let key = key.to_ascii_lowercase();
    if modifiers.mod_key && modifiers.shift {
        return match key.as_str() {
            "g" => Some(WindowIntent::Arrange {
                mode: ArrangeMode::Grid,
            }),
            "c" => Some(WindowIntent::Arrange {
                mode: ArrangeMode::Cascade,
            }),
            "s" => Some(WindowIntent::Arrange {
                mode: ArrangeMode::Stack,
            }),
            "n" => Some(WindowIntent::Spawn {
                kind: PaneKind::Agent {
                    session_id: String::new(), // filled by apply layer
                    role: "agent".into(),
                    title: "Agent".into(),
                    background: false,
                },
                id: None,
            }),
            _ => None,
        };
    }
    if modifiers.mod_key
        && key == "w"
        && let Some(id) = focused
    {
        return Some(WindowIntent::Close {
            id: id.clone(),
            cancel_agent: false,
        });
    }
    None
}

/// Minimal modifier snapshot so we don't depend on egui in pure tests.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    /// Cmd on macOS, Ctrl elsewhere.
    pub mod_key: bool,
    pub shift: bool,
    pub alt: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_fifo() {
        let mut bus = WindowIntentBus::new();
        bus.push(
            WindowIntent::Arrange {
                mode: ArrangeMode::Grid,
            },
            IntentSource::Keyboard,
        );
        bus.push(
            WindowIntent::Focus {
                id: PaneId::new("a"),
            },
            IntentSource::Gui,
        );
        let d = bus.drain();
        assert_eq!(d.len(), 2);
        assert!(matches!(d[0].intent, WindowIntent::Arrange { .. }));
        assert!(matches!(d[1].intent, WindowIntent::Focus { .. }));
        assert!(bus.is_empty());
    }

    #[test]
    fn external_channel_mcp_ready() {
        let (tx, mut src) = ExternalIntentSource::channel();
        tx.submit_mcp(WindowIntent::Spawn {
            kind: PaneKind::Agent {
                session_id: "s1".into(),
                role: "coder".into(),
                title: "Coder".into(),
                background: false,
            },
            id: None,
        })
        .unwrap();
        let polled = src.poll_intents();
        assert_eq!(polled.len(), 1);
        assert_eq!(polled[0].source, IntentSource::Mcp);
        assert!(matches!(polled[0].intent, WindowIntent::Spawn { .. }));
    }

    #[test]
    fn key_chords() {
        let mods = KeyModifiers {
            mod_key: true,
            shift: true,
            alt: false,
        };
        assert!(matches!(
            intent_from_key(mods, "G", None),
            Some(WindowIntent::Arrange {
                mode: ArrangeMode::Grid
            })
        ));
        let focused = PaneId::new("p1");
        assert!(matches!(
            intent_from_key(
                KeyModifiers {
                    mod_key: true,
                    shift: false,
                    alt: false
                },
                "w",
                Some(&focused)
            ),
            Some(WindowIntent::Close { .. })
        ));
    }

    #[test]
    fn serde_roundtrip_spawn() {
        let intent = WindowIntent::Spawn {
            kind: PaneKind::Agent {
                session_id: "abc".into(),
                role: "researcher".into(),
                title: "R1".into(),
                background: false,
            },
            id: Some(PaneId::new("pane-1")),
        };
        let json = serde_json::to_string(&intent).unwrap();
        let back: WindowIntent = serde_json::from_str(&json).unwrap();
        assert_eq!(intent, back);
    }
}
