//! Governance gate helper shared by daemon RPC families that must ask
//! the kernel gate before mutating state (`app.*`, `workload.*`).
//!
//! ADR-099 section 4: `workload.*` actions are default-deny, so a
//! missing gate refuses them (`fail_closed = true`). The `app.*` family
//! keeps its pre-existing posture (`AppManager` installs when no gate is
//! attached), so it passes `fail_closed = false`.

#[cfg(feature = "exochain")]
use clawft_kernel::{GateBackend, GateDecision};

/// Principal used for operator-initiated daemon RPCs. Matches the
/// principal `AppManager` uses for its own gate check (`app.rs`).
pub const DAEMON_PRINCIPAL: &str = "kernel";

/// Sink for audit events. Production wires this to
/// `ChainManager::append`; tests pass a recording closure.
pub type Audit<'a> = &'a dyn Fn(&str, serde_json::Value);

#[cfg(feature = "exochain")]
/// Ask `gate` whether `action` may proceed.
///
/// Returns `Ok(())` only on an explicit `Permit`. `Deny` and `Defer`
/// are both refusals (the daemon has no interactive reviewer on this
/// path). With no gate configured the result depends on `fail_closed`.
pub fn decide(
    gate: Option<&dyn GateBackend>,
    action: &str,
    context: &serde_json::Value,
    fail_closed: bool,
) -> Result<(), String> {
    decide_as(DAEMON_PRINCIPAL, gate, action, context, fail_closed)
}

#[cfg(feature = "exochain")]
/// [`decide`] as `principal` (the gate's agent id, which permit rules can
/// match on). The workload catalog verbs decide as `catalog`.
pub fn decide_as(
    principal: &str,
    gate: Option<&dyn GateBackend>,
    action: &str,
    context: &serde_json::Value,
    fail_closed: bool,
) -> Result<(), String> {
    let Some(gate) = gate else {
        return if fail_closed {
            Err(format!(
                "governance denied '{action}': no governance gate configured \
                 (default-deny, ADR-099)"
            ))
        } else {
            Ok(())
        };
    };
    match gate.check(principal, action, context) {
        GateDecision::Permit { .. } => Ok(()),
        GateDecision::Deny { reason, .. } => {
            Err(format!("governance denied '{action}': {reason}"))
        }
        GateDecision::Defer { reason } => Err(format!(
            "governance deferred '{action}' (needs review; refused on this path): {reason}"
        )),
        // `GateDecision` is non_exhaustive: an unknown future variant is
        // not a permit, so refuse.
        _ => Err(format!("governance returned an unrecognised decision for '{action}'")),
    }
}

#[cfg(all(test, feature = "exochain"))]
pub(crate) mod test_support {
    use super::*;
    use std::sync::Mutex;

    /// Gate that returns a fixed decision and records every check.
    pub struct FixedGate {
        pub decision: GateDecision,
        pub seen: Mutex<Vec<(String, String, serde_json::Value)>>,
    }

    impl FixedGate {
        pub fn permit() -> Self {
            Self::with(GateDecision::Permit { token: None })
        }
        pub fn deny(reason: &str) -> Self {
            Self::with(GateDecision::Deny {
                reason: reason.into(),
                receipt: None,
            })
        }
        pub fn defer(reason: &str) -> Self {
            Self::with(GateDecision::Defer {
                reason: reason.into(),
            })
        }
        fn with(decision: GateDecision) -> Self {
            Self {
                decision,
                seen: Mutex::new(Vec::new()),
            }
        }
        pub fn actions(&self) -> Vec<String> {
            self.seen.lock().unwrap().iter().map(|s| s.1.clone()).collect()
        }
    }

    impl GateBackend for FixedGate {
        fn check(&self, agent_id: &str, action: &str, ctx: &serde_json::Value) -> GateDecision {
            self.seen
                .lock()
                .unwrap()
                .push((agent_id.into(), action.into(), ctx.clone()));
            self.decision.clone()
        }
    }

    /// Recording audit sink.
    #[derive(Default)]
    pub struct Recorder(pub Mutex<Vec<(String, serde_json::Value)>>);

    impl Recorder {
        pub fn sink(&self) -> impl Fn(&str, serde_json::Value) + '_ {
            move |kind, payload| self.0.lock().unwrap().push((kind.into(), payload))
        }
        pub fn kinds(&self) -> Vec<String> {
            self.0.lock().unwrap().iter().map(|e| e.0.clone()).collect()
        }
    }
}

#[cfg(all(test, feature = "exochain"))]
mod tests {
    use super::test_support::FixedGate;
    use super::*;

    #[test]
    fn permit_passes_and_uses_daemon_principal() {
        let g = FixedGate::permit();
        assert!(decide(Some(&g), "workload.install", &serde_json::json!({}), true).is_ok());
        assert_eq!(g.seen.lock().unwrap()[0].0, DAEMON_PRINCIPAL);
    }

    #[test]
    fn deny_and_defer_refuse() {
        let d = decide(Some(&FixedGate::deny("nope")), "a.b", &serde_json::json!({}), false);
        assert!(d.unwrap_err().contains("nope"));
        let f = decide(Some(&FixedGate::defer("later")), "a.b", &serde_json::json!({}), false);
        assert!(f.unwrap_err().contains("deferred"));
    }

    #[test]
    fn missing_gate_honours_fail_closed() {
        assert!(decide(None, "workload.install", &serde_json::json!({}), true).is_err());
        assert!(decide(None, "app.install", &serde_json::json!({}), false).is_ok());
    }
}
