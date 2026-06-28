//! Effect gate seam.
//!
//! The agent loop calls [`EffectGate::check`] before each tool
//! dispatch with an [`EffectVector`](super::effects::EffectVector)
//! computed by [`effect_for_tool`](super::effects::effect_for_tool).
//! The gate decides whether the tool runs:
//!
//! - [`GateDecision::Permit`] — proceed with the tool execute.
//! - [`GateDecision::Defer`] — surface the reason as the tool result;
//!   the loop continues so the model can re-plan. Real interactive
//!   defer is a v1.1 follow-up needing panel UI (see `chat-agent-v1.md`
//!   risk register).
//! - [`GateDecision::Deny`]  — same handling as defer; the reason
//!   becomes the tool result and the loop continues.
//!
//! Phase D2 wires a kernel-backed implementation that calls
//! [`clawft_kernel::gate::GateBackend::check`](../../../../clawft-kernel/src/gate.rs)
//! and maps the kernel's `EffectVector` ↔ ours. Until then the default
//! attached to [`AgentLoop`](super::loop_core::AgentLoop) is
//! [`NoopGate`], which always permits.
//!
//! The shape mirrors `clawft-kernel`'s
//! [`GateDecision`](../../../../clawft-kernel/src/gate.rs:14-34) so
//! the Phase D2 mapping is a one-liner per variant.

use async_trait::async_trait;

use super::effects::EffectVector;

/// Outcome of an [`EffectGate::check`] call.
///
/// The `Permit` token is opaque — Phase D2's kernel-backed gate puts
/// the TileZero permit bytes here (encoded as base64 for readability).
/// Today the only producer is [`NoopGate`], which uses the literal
/// string `"noop"`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum GateDecision {
    /// Action permitted; carry the opaque token forward to the
    /// witness chain entry the loop will write at audit time.
    Permit {
        /// Opaque permit token — implementation defined.
        token: String,
    },
    /// Decision deferred; carry the human-readable reason so the
    /// agent can summarise why for the model.
    Defer {
        /// Why the gate deferred.
        reason: String,
    },
    /// Action denied; carry the human-readable reason.
    Deny {
        /// Why the gate denied.
        reason: String,
    },
}

impl GateDecision {
    /// Returns `true` if the decision is `Permit`.
    pub fn is_permit(&self) -> bool {
        matches!(self, GateDecision::Permit { .. })
    }
}

/// Pre-execute policy gate. Implementations decide whether a tool
/// dispatch is allowed given the agent identity, the action name
/// (e.g. `tool.write_file`), and the action's effect vector.
#[async_trait]
pub trait EffectGate: Send + Sync + 'static {
    /// Inspect a proposed action and return the gate decision.
    ///
    /// Implementations MUST be cancel-safe — the caller can drop the
    /// future without leaving the gate in a broken state.
    async fn check(&self, agent_id: &str, action: &str, effect: &EffectVector) -> GateDecision;
}

/// Trivial gate that permits every action.
///
/// Default for [`AgentLoop`](super::loop_core::AgentLoop) so existing
/// callers see no behaviour change. Phase D2 swaps in the kernel-
/// backed implementation that enforces real policy.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopGate;

#[async_trait]
impl EffectGate for NoopGate {
    async fn check(&self, _agent_id: &str, _action: &str, _effect: &EffectVector) -> GateDecision {
        GateDecision::Permit {
            token: "noop".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn _coerce(_gate: &dyn EffectGate) {}

    #[tokio::test]
    async fn noop_gate_permits_everything() {
        let gate = NoopGate;
        let ev = EffectVector {
            risk: 0.99,
            security: 0.99,
            ..Default::default()
        };
        let decision = gate.check("agent-1", "tool.exec", &ev).await;
        assert!(decision.is_permit());
        match decision {
            GateDecision::Permit { token } => assert_eq!(token, "noop"),
            other => panic!("expected Permit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn noop_gate_permits_with_zero_effect() {
        let gate = NoopGate;
        let decision = gate
            .check("agent-1", "tool.read_file", &EffectVector::default())
            .await;
        assert!(decision.is_permit());
    }

    #[test]
    fn is_permit_matches_only_permit() {
        assert!(GateDecision::Permit { token: "x".into() }.is_permit());
        assert!(!GateDecision::Defer { reason: "x".into() }.is_permit());
        assert!(!GateDecision::Deny { reason: "x".into() }.is_permit());
    }
}
