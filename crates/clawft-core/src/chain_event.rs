//! Lightweight chain event markers for crates without direct
//! [`ChainManager`](https://docs.rs/clawft-kernel) access.
//!
//! Since `clawft-core` does not depend on `clawft-kernel`, it cannot
//! call `ChainManager::append()` directly.  Instead, state-modifying
//! methods emit **structured tracing events** at `INFO` level using the
//! `chain_event!` macro.  The daemon layer (`clawft-weave`) subscribes
//! to the `chain_event` target and forwards matching spans to the real
//! ExoChain via `ChainManager::append()`.
//!
//! This design keeps `clawft-core` free of kernel dependencies while
//! still providing a consistent chain-logging surface.
//!
//! # Event kind constants
//!
//! These mirror the `EVENT_KIND_*` constants in `clawft-kernel::chain`
//! so that producers and consumers share a vocabulary without a compile
//! dependency.

/// Sandbox enforcement check.
pub const EVENT_KIND_SANDBOX_EXECUTE: &str = "sandbox.execute";

/// Session created.
pub const EVENT_KIND_SESSION_CREATE: &str = "session.create";

/// Conversation turn appended to a session (WEFT-85 / MW-7).
///
/// Emitted from [`crate::session::SessionManager::append_turn`] on every
/// successful append (subject to the manager's optional sample-rate cap).
/// Payload includes `key`, `role`, and `turn_count` — not message content.
pub const EVENT_KIND_SESSION_APPEND: &str = "session.append";

/// Session destroyed.
pub const EVENT_KIND_SESSION_DESTROY: &str = "session.destroy";

/// Workspace created.
pub const EVENT_KIND_WORKSPACE_CREATE: &str = "workspace.create";

/// Workspace config loaded / merged.
pub const EVENT_KIND_WORKSPACE_CONFIG: &str = "workspace.config";

/// Tool registered in the ToolRegistry.
pub const EVENT_KIND_TOOL_REGISTER: &str = "tool.register";

/// WASM plugin invocation (fuel/memory observability — WEFT-68).
///
/// Mirrors `clawft_plugin::EVENT_KIND_PLUGIN_WASM_INVOKE` so the daemon
/// chain bridge and plugin host share a vocabulary.
pub const EVENT_KIND_PLUGIN_WASM_INVOKE: &str = "plugin.wasm.invoke";

/// ObservationPack archive write (`ImpulseType::Custom(0x70)`).
pub const EVENT_KIND_OBSERVATION_ARCHIVED: &str = "observation.archived";

/// ObservationPack recall (`ImpulseType::Custom(0x71)`).
pub const EVENT_KIND_OBSERVATION_RECALLED: &str = "observation.recalled";

/// LeWM per-frame observational tuple attestation (WEFT-533).
///
/// Payload is a version-tagged SIGReg manifold reference plus
/// `(a_t, z_t, z_{t+1}, surprise)`. Mirrors
/// `weftos_worldmodel_core::EVENT_KIND_LEWM_FRAME_ATTESTATION`.
pub const EVENT_KIND_LEWM_FRAME_ATTESTATION: &str = "lewm.frame.attestation";

/// A pending chain event record for the tracing-to-ChainManager bridge.
///
/// Non-kernel crates cannot call `ChainManager::append()` directly.
/// Instead, they push records into the static [`PENDING_CHAIN_EVENTS`]
/// buffer via [`push_chain_event`]. The daemon periodically drains the
/// buffer via [`drain_pending_chain_events`] and forwards each record
/// to the real `ChainManager`.
#[derive(Debug, Clone)]
pub struct PendingChainEvent {
    /// Source subsystem (e.g. "session", "workspace", "wasm_fs").
    pub source: String,
    /// Event kind constant (e.g. "session.create").
    pub kind: String,
    /// Optional JSON payload.
    pub payload: Option<serde_json::Value>,
}

use std::sync::Mutex;

/// Global buffer of chain events emitted by non-kernel crates.
static PENDING_CHAIN_EVENTS: Mutex<Vec<PendingChainEvent>> = Mutex::new(Vec::new());

/// Push a chain event into the global pending buffer.
///
/// This is safe to call from any thread. The daemon drains this buffer
/// periodically and forwards events to `ChainManager::append()`.
pub fn push_chain_event(source: &str, kind: &str, payload: Option<serde_json::Value>) {
    if let Ok(mut buf) = PENDING_CHAIN_EVENTS.lock() {
        buf.push(PendingChainEvent {
            source: source.to_string(),
            kind: kind.to_string(),
            payload,
        });
    }
}

/// Drain all pending chain events from the global buffer.
///
/// Returns the accumulated events and clears the buffer. The daemon
/// calls this periodically to forward events to `ChainManager`.
pub fn drain_pending_chain_events() -> Vec<PendingChainEvent> {
    match PENDING_CHAIN_EVENTS.lock() {
        Ok(mut buf) => std::mem::take(&mut *buf),
        Err(_) => Vec::new(),
    }
}

/// Emit a structured tracing event on the `chain_event` target AND
/// push a record into the pending chain event buffer.
///
/// The daemon's chain event bridge drains the pending buffer and
/// forwards matching events to `ChainManager::append`.
///
/// Sets `chain_buffered = true` on the tracing record so the weaver
/// [`ChainEventLayer`] (WEFT-597) does not double-push events that the
/// macro has already buffered. Call sites that only emit tracing
/// (without this flag) are captured by the Layer instead.
///
/// # Usage
///
/// ```rust,ignore
/// chain_event!("session", "session.create", { "key": key });
/// ```
#[macro_export]
macro_rules! chain_event {
    ($source:expr, $kind:expr, { $($key:tt : $val:expr),* $(,)? }) => {
        {
            $crate::chain_event::push_chain_event(
                $source,
                $kind,
                Some(serde_json::json!({ $( stringify!($key): format!("{}", $val) ),* })),
            );
            tracing::info!(
                target: "chain_event",
                source = $source,
                kind = $kind,
                chain_buffered = true,
                $( $key = %$val, )*
                "chain"
            );
        }
    };
}

pub const EVENT_KIND_TOOL_REGISTER_METADATA: &str = "tool.register.metadata";
pub const EVENT_KIND_GRAPHIFY_QUERY_RESULT: &str = "graphify.query_result";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_expected_values() {
        assert_eq!(EVENT_KIND_SANDBOX_EXECUTE, "sandbox.execute");
        assert_eq!(EVENT_KIND_SESSION_CREATE, "session.create");
        assert_eq!(EVENT_KIND_SESSION_APPEND, "session.append");
        assert_eq!(EVENT_KIND_SESSION_DESTROY, "session.destroy");
        assert_eq!(EVENT_KIND_WORKSPACE_CREATE, "workspace.create");
        assert_eq!(EVENT_KIND_WORKSPACE_CONFIG, "workspace.config");
        assert_eq!(EVENT_KIND_TOOL_REGISTER, "tool.register");
        assert_eq!(EVENT_KIND_PLUGIN_WASM_INVOKE, "plugin.wasm.invoke");
        assert_eq!(
            EVENT_KIND_LEWM_FRAME_ATTESTATION,
            "lewm.frame.attestation"
        );
        assert_eq!(EVENT_KIND_OBSERVATION_ARCHIVED, "observation.archived");
        assert_eq!(EVENT_KIND_OBSERVATION_RECALLED, "observation.recalled");
    }

    #[test]
    fn macro_compiles_and_pushes_event() {
        // Drain any stale events from previous tests.
        drain_pending_chain_events();

        // Verify the macro expands and pushes to the buffer.
        chain_event!("test", "test.event", { "foo": "bar" });

        let events = drain_pending_chain_events();
        assert!(!events.is_empty(), "macro should push to pending buffer");
        assert_eq!(events[0].source, "test");
        assert_eq!(events[0].kind, "test.event");
    }

    #[test]
    fn push_and_drain_chain_events() {
        drain_pending_chain_events();

        push_chain_event("src", "kind.test", Some(serde_json::json!({"a": 1})));
        push_chain_event("src2", "kind.test2", None);

        let events = drain_pending_chain_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].source, "src");
        assert_eq!(events[1].kind, "kind.test2");

        // Drain again should be empty.
        assert!(drain_pending_chain_events().is_empty());
    }
}
