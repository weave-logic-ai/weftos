//! 5-dimensional effect vector for agent actions.
//!
//! This is a **structural mirror** of
//! [`clawft_kernel::governance::EffectVector`](../../../../clawft-kernel/src/governance.rs).
//! We define a local copy here so `clawft-core` does not pull a hard
//! dependency on `clawft-kernel`. Phase D2's kernel-backed gate impl
//! lives in the daemon (`clawft-service-agent` / `clawft-weave`) and
//! maps between the two types.
//!
//! Each dimension scores 0.0 (no impact) → 1.0 (maximum impact). The
//! magnitude (L2 norm) is what the kernel's threshold check consumes;
//! we expose [`EffectVector::magnitude`] so callers can replicate that
//! behaviour locally without re-deriving the formula.

use serde::{Deserialize, Serialize};

/// 5-dimensional effect score. See module docs for the kernel mirror
/// contract.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EffectVector {
    /// Probability of negative outcome (0.0 → 1.0).
    #[serde(default)]
    pub risk: f64,

    /// Impact on equitable treatment (0.0 → 1.0).
    #[serde(default)]
    pub fairness: f64,

    /// Impact on data privacy (0.0 → 1.0).
    #[serde(default)]
    pub privacy: f64,

    /// How unprecedented the action is (0.0 → 1.0).
    #[serde(default)]
    pub novelty: f64,

    /// Impact on system security (0.0 → 1.0).
    #[serde(default)]
    pub security: f64,
}

impl EffectVector {
    /// Compute the L2 norm of the effect vector. Matches the kernel's
    /// `EffectVector::magnitude` so a `Permit/Defer/Deny` decision
    /// here aligns with the kernel's threshold semantics.
    pub fn magnitude(&self) -> f64 {
        (self.risk * self.risk
            + self.fairness * self.fairness
            + self.privacy * self.privacy
            + self.novelty * self.novelty
            + self.security * self.security)
            .sqrt()
    }
}

/// Map a tool name + (currently unused) args JSON to its baseline
/// [`EffectVector`].
///
/// This is the v0 static table the agent loop consults before each
/// `tools.execute`. Phase D2 will swap in a richer scorer (kernel
/// EML-trained `GovernanceScorerModel`) and also begin honoring the
/// `args` argument (e.g. `write_file` outside the workspace might
/// score higher than within). Today `args` is reserved.
///
/// Default for unknown tools is the all-zero vector — a Permit under
/// any sane policy. Adding a tool to this table is the cheapest form
/// of policy authoring.
pub fn effect_for_tool(name: &str, _args: &serde_json::Value) -> EffectVector {
    match name {
        // ── Reads ──────────────────────────────────────────────────
        "read_file" => EffectVector {
            privacy: 0.1,
            ..Default::default()
        },
        "list_directory" => EffectVector {
            privacy: 0.05,
            ..Default::default()
        },

        // ── Writes ─────────────────────────────────────────────────
        "write_file" => EffectVector {
            security: 0.4,
            ..Default::default()
        },

        // ── Execution ──────────────────────────────────────────────
        "exec" => EffectVector {
            risk: 0.6,
            security: 0.7,
            ..Default::default()
        },

        // Unknown tools = neutral. New tools land in this table the
        // first time they need policy-aware behaviour; until then a
        // zero vector + the kernel's permissive default is fine.
        _ => EffectVector::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_tools_score_privacy() {
        let read = effect_for_tool("read_file", &json!({}));
        assert!((read.privacy - 0.1).abs() < f64::EPSILON);
        assert_eq!(read.security, 0.0);

        let list = effect_for_tool("list_directory", &json!({}));
        assert!((list.privacy - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn write_tool_scores_security() {
        let w = effect_for_tool("write_file", &json!({"path": "/tmp/foo"}));
        assert!((w.security - 0.4).abs() < f64::EPSILON);
        assert_eq!(w.risk, 0.0);
    }

    #[test]
    fn exec_tool_scores_both_risk_and_security() {
        let e = effect_for_tool("exec", &json!({"cmd": "ls"}));
        assert!((e.risk - 0.6).abs() < f64::EPSILON);
        assert!((e.security - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_tool_is_neutral() {
        let u = effect_for_tool("definitely_not_a_real_tool", &json!({}));
        assert_eq!(u, EffectVector::default());
        assert_eq!(u.magnitude(), 0.0);
    }

    #[test]
    fn magnitude_matches_l2_norm() {
        let ev = EffectVector {
            risk: 0.6,
            security: 0.7,
            ..Default::default()
        };
        let expected = (0.6f64.powi(2) + 0.7f64.powi(2)).sqrt();
        assert!((ev.magnitude() - expected).abs() < 1e-9);
    }

    #[test]
    fn args_are_currently_ignored() {
        // Phase D2 will use args to refine scores. Until then the
        // scorer is purely name-driven; assert that explicitly so
        // future changes can flip the assertion without surprise.
        let a = effect_for_tool("write_file", &json!({"path": "/etc/passwd"}));
        let b = effect_for_tool("write_file", &json!({"path": "/tmp/safe.log"}));
        assert_eq!(a, b);
    }
}
