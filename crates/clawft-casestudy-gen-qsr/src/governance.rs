//! Phase 1 — governance rule engine.
//!
//! A lightweight mirror of `crates/clawft-kernel/src/governance.rs` sized for
//! the synthetic test harness. Implements the two rules called out in the
//! analysis §11 (Operational risks):
//!
//! 1. **SOX-style attestation**: rollups in a sealed quarter cannot be
//!    overwritten by `NoveltyDetected`; they must come through as
//!    `BeliefUpdate` (which the impulse queue auto-rewrites when the HLC is
//!    older than the watermark).
//! 2. **Franchisee data boundary**: impulses from a franchisee denylisted for
//!    the tenant boundary are blocked outright.

use crate::impulse::{Impulse, ImpulseType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub permitted: bool,
    pub severity: Severity,
    pub rule: String,
    pub reason: String,
}

impl Decision {
    pub fn permit() -> Self {
        Self {
            permitted: true,
            severity: Severity::Warning,
            rule: "default".into(),
            reason: "permit".into(),
        }
    }
    pub fn deny(rule: &str, reason: &str) -> Self {
        Self {
            permitted: false,
            severity: Severity::Blocking,
            rule: rule.into(),
            reason: reason.into(),
        }
    }
}

/// Governance policy: a set of rules plus per-tenant configuration.
pub struct Governance {
    /// Quarters that have been sealed (year × quarter).
    sealed_quarters: BTreeSet<(u32, u8)>,
    /// Franchisee orgs denylisted from the tenant boundary.
    franchisee_denylist: BTreeSet<String>,
}

impl Governance {
    pub fn new() -> Self {
        Self {
            sealed_quarters: BTreeSet::new(),
            franchisee_denylist: BTreeSet::new(),
        }
    }

    pub fn seal_quarter(&mut self, year: u32, quarter: u8) {
        self.sealed_quarters.insert((year, quarter));
    }

    pub fn denylist_franchisee(&mut self, org_ref: &str) {
        self.franchisee_denylist.insert(org_ref.to_string());
    }

    /// Evaluate a candidate impulse. Returns a `Decision`.
    pub fn evaluate(&self, impulse: &Impulse) -> Decision {
        // Franchisee boundary
        if self
            .franchisee_denylist
            .iter()
            .any(|deny| impulse.store_ref.contains(deny))
        {
            return Decision::deny(
                "franchisee_boundary",
                "store belongs to a franchisee outside the tenant data boundary",
            );
        }

        // SOX-style sealed quarter: rollups in a sealed quarter must be
        // BeliefUpdate, not NoveltyDetected.
        if let Some((y, q)) = parse_year_quarter(&impulse.business_date)
            && self.sealed_quarters.contains(&(y, q))
            && impulse.kind == ImpulseType::NoveltyDetected
        {
            return Decision::deny(
                "sox_sealed_quarter",
                "quarter is sealed; revisions must arrive as BeliefUpdate",
            );
        }

        Decision::permit()
    }
}

impl Default for Governance {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_year_quarter(business_date: &str) -> Option<(u32, u8)> {
    let d = chrono::NaiveDate::parse_from_str(business_date, "%Y-%m-%d").ok()?;
    let y = d.format("%Y").to_string().parse::<u32>().ok()?;
    let month: u32 = d.format("%m").to_string().parse().ok()?;
    Some((y, ((month - 1) / 3 + 1) as u8))
}
