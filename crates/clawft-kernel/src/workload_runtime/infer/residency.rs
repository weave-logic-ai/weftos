//! Unified-memory budget and co-residency (card mesh-placement-20; ADR-101
//! section 6). One ledger is shared by every managed adapter on a node:
//! an instance reserves its weights plus KV budget when it starts and
//! releases them when it stops or is unloaded. A start that would exceed
//! the budget, or that excludes (or is excluded by) a running role, is
//! refused as unplaceable, with the reason.
//!
//! Memory is counted in decimal gigabytes at the edges (`ram_gb` of the
//! model lab's roster, the operator's budget) and in bytes here.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::spec::InferenceSpec;

/// Bytes in the roster's and the operator's "GB".
pub const GB: u64 = 1_000_000_000;

/// Why an instance cannot be resident beside the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoResidency {
    /// Its memory does not fit in what is left of the budget.
    OverBudget {
        /// Role that was refused.
        role: String,
        /// Bytes it needs.
        need: u64,
        /// Bytes still free.
        free: u64,
        /// The whole budget.
        budget: u64,
        /// Who holds the rest: (role, bytes).
        holders: Vec<(String, u64)>,
    },
    /// Either side lists the other in `excludes`.
    Excluded {
        /// Role that was refused.
        role: String,
        /// Running role it cannot sit beside.
        with: String,
        /// Which side declared the exclusion.
        declared_by: String,
    },
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / GB as f64)
}

impl std::fmt::Display for CoResidency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OverBudget { role, need, free, budget, holders } => {
                let held = holders
                    .iter()
                    .map(|(r, b)| format!("{r} {}", gb(*b)))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "unplaceable: '{role}' needs {} but only {} of the {} unified-memory budget is free (held by {})",
                    gb(*need),
                    gb(*free),
                    gb(*budget),
                    if held.is_empty() { "nothing" } else { &held }
                )
            }
            Self::Excluded { role, with, declared_by } => write!(
                f,
                "unplaceable: '{role}' cannot be resident beside '{with}' ('{declared_by}' excludes it)"
            ),
        }
    }
}

/// Bytes a spec holds while resident.
pub fn footprint(spec: &InferenceSpec) -> u64 {
    spec.memory.weights_bytes.saturating_add(spec.memory.kv_budget_bytes)
}

fn excludes(spec: &InferenceSpec, other_role: &str) -> bool {
    spec.excludes
        .iter()
        .any(|x| x == other_role || x.strip_prefix("role:") == Some(other_role))
}

/// The node's resident instances.
pub struct ResidencyLedger {
    budget: Option<u64>,
    held: Mutex<BTreeMap<String, InferenceSpec>>,
}

impl ResidencyLedger {
    /// A ledger with `budget_bytes` of unified memory (`None`: only
    /// exclusions are checked).
    pub fn new(budget_bytes: Option<u64>) -> Self {
        Self { budget: budget_bytes, held: Mutex::new(BTreeMap::new()) }
    }

    /// The budget, if any.
    pub fn budget(&self) -> Option<u64> {
        self.budget
    }

    /// Reserve `spec`'s memory for `instance`. `Ok(true)` when newly
    /// reserved, `Ok(false)` when this instance already held it.
    pub fn reserve(&self, instance: &str, spec: &InferenceSpec) -> Result<bool, CoResidency> {
        let mut g = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if g.contains_key(instance) {
            return Ok(false);
        }
        for (id, other) in g.iter() {
            if id == instance {
                continue;
            }
            if excludes(spec, &other.role) {
                return Err(CoResidency::Excluded {
                    role: spec.role.clone(),
                    with: other.role.clone(),
                    declared_by: spec.role.clone(),
                });
            }
            if excludes(other, &spec.role) {
                return Err(CoResidency::Excluded {
                    role: spec.role.clone(),
                    with: other.role.clone(),
                    declared_by: other.role.clone(),
                });
            }
        }
        if let Some(budget) = self.budget {
            let used: u64 = g.values().map(footprint).fold(0, u64::saturating_add);
            let need = footprint(spec);
            let free = budget.saturating_sub(used);
            if need > free {
                return Err(CoResidency::OverBudget {
                    role: spec.role.clone(),
                    need,
                    free,
                    budget,
                    holders: g.values().map(|s| (s.role.clone(), footprint(s))).collect(),
                });
            }
        }
        g.insert(instance.to_string(), spec.clone());
        Ok(true)
    }

    /// Release what `instance` held (no-op if nothing).
    pub fn release(&self, instance: &str) {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).remove(instance);
    }

    /// Bytes reserved.
    pub fn used(&self) -> u64 {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(footprint)
            .fold(0, u64::saturating_add)
    }

    /// Resident roles and their bytes.
    pub fn snapshot(&self) -> Vec<(String, u64)> {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|s| (s.role.clone(), footprint(s)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload_runtime::infer::spec::InferFlavor;

    fn spec(role: &str, gb_: u64, excludes: &[&str]) -> InferenceSpec {
        let mut s = InferenceSpec::new(role, InferFlavor::MlxLm);
        s.memory.weights_bytes = gb_ * GB;
        s.excludes = excludes.iter().map(|x| x.to_string()).collect();
        s
    }

    #[test]
    fn fits_until_the_budget_is_spent_then_says_who_holds_it() {
        let l = ResidencyLedger::new(Some(96 * GB));
        assert!(l.reserve("a", &spec("coder-daily", 55, &[])).unwrap());
        assert!(l.reserve("b", &spec("coder-small", 9, &[])).unwrap());
        let e = l.reserve("c", &spec("planner", 55, &[])).unwrap_err();
        let text = e.to_string();
        assert!(text.contains("'planner' needs 55.0 GB"), "{text}");
        assert!(text.contains("32.0 GB of the 96.0 GB"), "{text}");
        assert!(text.contains("coder-daily 55.0 GB") && text.contains("coder-small 9.0 GB"), "{text}");
        assert_eq!(l.used(), 64 * GB, "a refusal reserves nothing");
        // Freeing makes room.
        l.release("a");
        assert!(l.reserve("c", &spec("planner", 55, &[])).unwrap());
    }

    #[test]
    fn exactly_the_budget_fits_and_one_byte_more_does_not() {
        let l = ResidencyLedger::new(Some(10 * GB));
        let mut s = spec("x", 10, &[]);
        assert!(l.reserve("x", &s).unwrap());
        l.release("x");
        s.memory.kv_budget_bytes = 1;
        assert!(matches!(l.reserve("x", &s), Err(CoResidency::OverBudget { .. })));
    }

    #[test]
    fn exclusions_hold_in_both_directions_and_with_the_role_prefix() {
        let l = ResidencyLedger::new(None);
        assert!(l.reserve("a", &spec("planner", 1, &["role:coder-daily"])).unwrap());
        // The newcomer is the one excluded by a running role.
        let e = l.reserve("b", &spec("coder-daily", 1, &[])).unwrap_err();
        assert_eq!(
            e,
            CoResidency::Excluded { role: "coder-daily".into(), with: "planner".into(), declared_by: "planner".into() }
        );
        assert!(e.to_string().contains("'planner' excludes it"));
        // The newcomer excludes a running role (bare name form).
        let e = l.reserve("c", &spec("swarm", 1, &["planner"])).unwrap_err();
        assert!(matches!(e, CoResidency::Excluded { ref declared_by, .. } if declared_by == "swarm"));
        // Unrelated roles coexist; no budget means no memory check.
        assert!(l.reserve("d", &spec("embed", 500, &[])).unwrap());
    }

    #[test]
    fn reserving_twice_for_one_instance_counts_once() {
        let l = ResidencyLedger::new(Some(60 * GB));
        let s = spec("coder", 55, &[]);
        assert!(l.reserve("a", &s).unwrap());
        assert!(!l.reserve("a", &s).unwrap());
        assert_eq!(l.used(), 55 * GB);
    }
}
