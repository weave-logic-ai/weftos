//! Kernel-backed [`EffectGate`] adapter (`agent-core-v1.md` Phase D2).
//!
//! Wraps a [`clawft_kernel::gate::GateBackend`] (concretely, the
//! daemon's `GovernanceGate`) so every tool dispatch in
//! [`AgentLoop::run_tool_loop`](clawft_core::agent::loop_core::AgentLoop)
//! produces an audited Permit/Defer/Deny decision with a witness
//! chain entry.
//!
//! ## Mapping
//!
//! - [`EffectVector`](clawft_core::agent::effects::EffectVector) →
//!   kernel JSON via
//!   [`EffectVector::to_kernel_json`](clawft_core::agent::effects::EffectVector::to_kernel_json),
//!   wrapped as `{ "effect": <ev>, "agent_id": <agent>, "action":
//!   <action> }`. The kernel's `GovernanceGate` extracts the same
//!   shape via its `extract_effect` helper.
//! - Kernel-side [`GateDecision`](clawft_kernel::gate::GateDecision)
//!   → core's local [`GateDecision`](clawft_core::agent::gate::GateDecision)
//!   structurally, 1:1 per variant. The kernel's `Permit { token:
//!   Option<Vec<u8>> }` becomes core's `Permit { token: String }` —
//!   we hex-encode the bytes so the witness chain entry can carry
//!   the receipt without bringing a base64 dep into core. Kernel's
//!   `Deny { reason, receipt }` drops the receipt; the panel UI
//!   doesn't yet have a place to render it (tracked for v1.1).
//!
//! ## Why a `GovernanceGateLike` test seam?
//!
//! [`KernelEffectGate::new`] takes an `Arc<dyn
//! GateBackend>` so production wires the daemon's
//! `GovernanceGate` directly. The unit tests below stub a tiny
//! `GovernanceGateLike` trait and a blanket impl over
//! [`GateBackend`] — same pattern as
//! [`KernelSubstrateClient`](crate::substrate_sink::KernelSubstrateClient)
//! in C3. This keeps the test surface narrow without paying
//! `GovernanceGate`'s feature-gating cost (`exochain`, `ecc`, …) at
//! every test invocation.

use std::sync::Arc;

use async_trait::async_trait;

use clawft_core::agent::effects::EffectVector;
use clawft_core::agent::gate::{EffectGate, GateDecision};
use clawft_kernel::gate::{GateBackend, GateDecision as KernelGateDecision};

/// Adapter between core's [`EffectGate`] trait seam and a
/// [`GateBackend`] from `clawft-kernel`.
///
/// Hold onto an [`Arc`] so the daemon can share one gate instance
/// across every chat turn without per-dispatch cloning.
pub struct KernelEffectGate {
    gate: Arc<dyn GateBackend>,
}

impl KernelEffectGate {
    /// Construct a new adapter around an `Arc<dyn GateBackend>`.
    /// Production callers pass `kernel.governance_gate().cloned()`
    /// (when the kernel is built with the `exochain` feature).
    pub fn new(gate: Arc<dyn GateBackend>) -> Self {
        Self { gate }
    }
}

/// Map the kernel's `Permit { token: Option<Vec<u8>> }` to core's
/// `Permit { token: String }`. The token is opaque to the loop today
/// (Phase D2 plan: "ignore the token; tracked as v1.1 follow-up");
/// hex-encoding keeps the witness payload roundtrippable when a
/// future commit threads it into `tools.execute`.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // No need to depend on the `hex` crate for one helper.
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn map_decision(k: KernelGateDecision) -> GateDecision {
    match k {
        KernelGateDecision::Permit { token } => GateDecision::Permit {
            token: token
                .as_deref()
                .map(hex_encode)
                .unwrap_or_else(|| "kernel-permit".into()),
        },
        KernelGateDecision::Defer { reason } => GateDecision::Defer { reason },
        // The kernel-side `receipt` is dropped — core's local
        // GateDecision doesn't carry it and the panel UX doesn't yet
        // have anywhere to render it. Tracked as v1.1 follow-up
        // (see the module docs).
        KernelGateDecision::Deny { reason, receipt: _ } => GateDecision::Deny { reason },
        // KernelGateDecision is `#[non_exhaustive]`. Future variants
        // default to Defer with a synthetic reason so the loop can
        // re-plan rather than crash on a kernel upgrade. Treat this
        // as a strong signal to revisit the mapping during the next
        // kernel rev.
        other => GateDecision::Defer {
            reason: format!("unmapped kernel gate decision: {other:?}"),
        },
    }
}

#[async_trait]
impl EffectGate for KernelEffectGate {
    async fn check(
        &self,
        agent_id: &str,
        action: &str,
        effect: &EffectVector,
    ) -> GateDecision {
        // Build the kernel-side context JSON. The kernel's
        // GovernanceGate extracts `effect` back through serde_json
        // (see `clawft_kernel::gate::GovernanceGate::extract_effect`);
        // the additional `agent_id` / `action` fields are mirrored
        // here so any chain-logging side effect on the kernel side
        // sees the same identifiers we passed positionally.
        let context = serde_json::json!({
            "effect": effect.to_kernel_json(),
            "agent_id": agent_id,
            "action": action,
        });
        let decision = self.gate.check(agent_id, action, &context);
        map_decision(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Stub `GateBackend` that returns a configured decision and
    /// records every `(agent_id, action, context)` it observed.
    /// Same shape as the C3 substrate sink's `MemSubstrateClient`.
    struct StubBackend {
        decision: Mutex<Option<KernelGateDecision>>,
        observed: Mutex<Vec<(String, String, serde_json::Value)>>,
    }

    impl StubBackend {
        fn permit(token: Option<Vec<u8>>) -> Arc<Self> {
            Arc::new(Self {
                decision: Mutex::new(Some(KernelGateDecision::Permit { token })),
                observed: Mutex::new(Vec::new()),
            })
        }
        fn defer(reason: &str) -> Arc<Self> {
            Arc::new(Self {
                decision: Mutex::new(Some(KernelGateDecision::Defer {
                    reason: reason.into(),
                })),
                observed: Mutex::new(Vec::new()),
            })
        }
        fn deny(reason: &str, receipt: Option<Vec<u8>>) -> Arc<Self> {
            Arc::new(Self {
                decision: Mutex::new(Some(KernelGateDecision::Deny {
                    reason: reason.into(),
                    receipt,
                })),
                observed: Mutex::new(Vec::new()),
            })
        }
        fn last_context(&self) -> Option<serde_json::Value> {
            self.observed.lock().unwrap().last().map(|(_, _, c)| c.clone())
        }
        fn last_action(&self) -> Option<String> {
            self.observed.lock().unwrap().last().map(|(_, a, _)| a.clone())
        }
        fn last_agent(&self) -> Option<String> {
            self.observed.lock().unwrap().last().map(|(g, _, _)| g.clone())
        }
    }

    impl GateBackend for StubBackend {
        fn check(
            &self,
            agent_id: &str,
            action: &str,
            context: &serde_json::Value,
        ) -> KernelGateDecision {
            self.observed.lock().unwrap().push((
                agent_id.into(),
                action.into(),
                context.clone(),
            ));
            // Decisions are stamped Once; `take` preserves intent
            // while panicking loudly if a test forgets to set it.
            self.decision
                .lock()
                .unwrap()
                .clone()
                .expect("StubBackend decision unset")
        }
    }

    fn sample_effect() -> EffectVector {
        EffectVector {
            risk: 0.6,
            fairness: 0.0,
            privacy: 0.3,
            novelty: 0.0,
            security: 0.7,
        }
    }

    #[tokio::test]
    async fn permit_passes_through() {
        let backend = StubBackend::permit(Some(vec![0xAB, 0xCD]));
        let gate = KernelEffectGate::new(backend.clone());
        let dec = gate.check("agent-1", "tool.read_file", &sample_effect()).await;
        match dec {
            GateDecision::Permit { token } => {
                assert_eq!(token, "abcd", "token should be hex-encoded");
            }
            other => panic!("expected Permit, got {other:?}"),
        }
        assert_eq!(backend.last_action(), Some("tool.read_file".into()));
        assert_eq!(backend.last_agent(), Some("agent-1".into()));
    }

    #[tokio::test]
    async fn permit_with_no_token_uses_sentinel() {
        let backend = StubBackend::permit(None);
        let gate = KernelEffectGate::new(backend);
        let dec = gate.check("a", "tool.x", &EffectVector::default()).await;
        match dec {
            GateDecision::Permit { token } => assert_eq!(token, "kernel-permit"),
            other => panic!("expected Permit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn defer_propagates_reason() {
        let backend = StubBackend::defer("policy review needed");
        let gate = KernelEffectGate::new(backend);
        let dec = gate.check("a", "tool.exec", &sample_effect()).await;
        match dec {
            GateDecision::Defer { reason } => {
                assert_eq!(reason, "policy review needed");
            }
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_drops_receipt_keeps_reason() {
        let backend = StubBackend::deny("blocked by SOP-3", Some(vec![1, 2, 3]));
        let gate = KernelEffectGate::new(backend);
        let dec = gate.check("a", "tool.write_file", &sample_effect()).await;
        match dec {
            GateDecision::Deny { reason } => assert_eq!(reason, "blocked by SOP-3"),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn context_carries_effect_under_effect_key() {
        let backend = StubBackend::permit(None);
        let gate = KernelEffectGate::new(backend.clone());
        let _ = gate.check("a", "tool.exec", &sample_effect()).await;
        let ctx = backend.last_context().expect("context observed");
        let effect = ctx.get("effect").expect("`effect` key present");
        // Kernel-side `governance::EffectVector` reads these keys.
        for key in ["risk", "fairness", "privacy", "novelty", "security"] {
            assert!(
                effect.get(key).is_some(),
                "kernel context.effect missing `{key}`"
            );
        }
        assert_eq!(effect["risk"].as_f64(), Some(0.6));
        assert_eq!(effect["security"].as_f64(), Some(0.7));
        // Sibling fields the kernel may also use — present so any
        // chain-logging path sees the same identifiers we passed
        // positionally.
        assert_eq!(ctx["agent_id"].as_str(), Some("a"));
        assert_eq!(ctx["action"].as_str(), Some("tool.exec"));
    }
}
