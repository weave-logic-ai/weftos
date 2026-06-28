//! Phase 3 — gap analysis patterns.
//!
//! The eight cold-case detectors translated to QSR operations per the
//! mapping in `.planning/clients/qsr/weftos-implementation-analysis.md` §7.
//!
//! Each detector returns a list of [`Gap`]s. Severity:
//! - `Critical` — patient-zero / SLA-impacting (unfilled GM, food-safety lapse)
//! - `High`     — material to store performance (inventory, cert gap)
//! - `Medium`   — warning signal (shift coverage, turnover cluster)
//! - `Low`      — hygiene (unverified training claim)

use crate::config::GeneratorConfig;
use crate::dimensions::{Dimensions, PersonStatus};
use crate::ops_events::{OpsEventLedger, cert_is_renewed};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapPattern {
    VacantPosition,
    CertExpiredUnrenewed,
    TrainingClaimUnverified,
    ShiftCoverageGap,
    StaffingAntiPatternCluster,
    InventoryNotPerformed,
    TurnoverContagionCluster,
    AuditNotPerformed,
}

impl GapPattern {
    pub fn all() -> [GapPattern; 8] {
        [
            GapPattern::VacantPosition,
            GapPattern::CertExpiredUnrenewed,
            GapPattern::TrainingClaimUnverified,
            GapPattern::ShiftCoverageGap,
            GapPattern::StaffingAntiPatternCluster,
            GapPattern::InventoryNotPerformed,
            GapPattern::TurnoverContagionCluster,
            GapPattern::AuditNotPerformed,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GapPattern::VacantPosition => "vacant_position",
            GapPattern::CertExpiredUnrenewed => "cert_expired_unrenewed",
            GapPattern::TrainingClaimUnverified => "training_claim_unverified",
            GapPattern::ShiftCoverageGap => "shift_coverage_gap",
            GapPattern::StaffingAntiPatternCluster => "staffing_anti_pattern_cluster",
            GapPattern::InventoryNotPerformed => "inventory_not_performed",
            GapPattern::TurnoverContagionCluster => "turnover_contagion_cluster",
            GapPattern::AuditNotPerformed => "audit_not_performed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gap {
    pub pattern: GapPattern,
    pub severity: GapSeverity,
    pub store_ref: Option<String>,
    pub subject: String,
    pub message: String,
    pub suggested_action: String,
    pub day_index: Option<u32>,
}

pub struct GapReport {
    pub gaps: Vec<Gap>,
    pub by_pattern: BTreeMap<GapPattern, usize>,
    pub by_store: HashMap<String, Vec<Gap>>,
}

impl GapReport {
    pub fn total(&self) -> usize {
        self.gaps.len()
    }
    pub fn count(&self, pattern: GapPattern) -> usize {
        self.by_pattern.get(&pattern).copied().unwrap_or(0)
    }
    pub fn count_severity(&self, severity: GapSeverity) -> usize {
        self.gaps.iter().filter(|g| g.severity == severity).count()
    }
    pub fn for_store(&self, store_ref: &str) -> Vec<&Gap> {
        self.by_store
            .get(store_ref)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }
}

pub fn sweep(config: &GeneratorConfig, dims: &Dimensions, ledger: &OpsEventLedger) -> GapReport {
    let mut gaps = Vec::new();
    let days = config.scale_tier.days();

    gaps.extend(detect_vacant_positions(dims));
    gaps.extend(detect_cert_gaps(dims, ledger, days));
    gaps.extend(detect_training_unverified(dims));
    gaps.extend(detect_shift_coverage(ledger));
    gaps.extend(detect_staffing_anti_patterns(dims));
    gaps.extend(detect_inventory_missing(config, dims, ledger, days));
    gaps.extend(detect_turnover_clusters(dims, days));
    gaps.extend(detect_audits_missing(config, dims, ledger, days));

    // Build indices
    let mut by_pattern = BTreeMap::new();
    let mut by_store: HashMap<String, Vec<Gap>> = HashMap::new();
    for g in &gaps {
        *by_pattern.entry(g.pattern).or_insert(0) += 1;
        if let Some(store) = &g.store_ref {
            by_store.entry(store.clone()).or_default().push(g.clone());
        }
    }

    GapReport {
        gaps,
        by_pattern,
        by_store,
    }
}

// ---------------------------------------------------------------------------
// Pattern 1 — Vacant POSITION
// ---------------------------------------------------------------------------
pub fn detect_vacant_positions(dims: &Dimensions) -> Vec<Gap> {
    dims.positions
        .iter()
        .filter(|p| p.filled_by_ref.is_none())
        .map(|p| Gap {
            pattern: GapPattern::VacantPosition,
            severity: if p.critical {
                GapSeverity::Critical
            } else {
                GapSeverity::High
            },
            store_ref: Some(p.store_ref.clone()),
            subject: p.label.clone(),
            message: format!("Position {} ({}) is vacant", p.label, p.role_template),
            suggested_action: if p.critical {
                "Immediately cover GM role; escalate to DM".into()
            } else {
                "Schedule hire / backfill".into()
            },
            day_index: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pattern 2 — Cert expired, not renewed
// ---------------------------------------------------------------------------
pub fn detect_cert_gaps(dims: &Dimensions, ledger: &OpsEventLedger, days: u32) -> Vec<Gap> {
    let mut out = Vec::new();
    for person in &dims.people {
        if person.status == PersonStatus::Terminated {
            continue;
        }
        for (cert, &expiration) in &person.cert_expirations {
            if expiration >= days {
                continue;
            }
            if !cert_is_renewed(person, cert, &ledger.cert_renewals) {
                let critical = cert.ends_with("-gm-cert");
                out.push(Gap {
                    pattern: GapPattern::CertExpiredUnrenewed,
                    severity: if critical {
                        GapSeverity::Critical
                    } else {
                        GapSeverity::High
                    },
                    store_ref: Some(person.home_store_ref.clone()),
                    subject: format!("{}::{}", person.label, cert),
                    message: format!(
                        "Certification `{}` for {} expired day {} with no renewal event",
                        cert, person.label, expiration
                    ),
                    suggested_action: "Submit renewal test; suspend from role if food-safety"
                        .into(),
                    day_index: Some(expiration),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pattern 3 — Training claim without LMS corroboration
// ---------------------------------------------------------------------------
pub fn detect_training_unverified(dims: &Dimensions) -> Vec<Gap> {
    let mut out = Vec::new();
    for person in &dims.people {
        for claim in &person.claimed_trainings {
            if !claim.lms_verified {
                out.push(Gap {
                    pattern: GapPattern::TrainingClaimUnverified,
                    severity: GapSeverity::Low,
                    store_ref: Some(person.home_store_ref.clone()),
                    subject: format!("{}::{}", person.label, claim.cert_name),
                    message: format!(
                        "{} claimed `{}` on day {} but LMS has no matching record",
                        person.label, claim.cert_name, claim.claimed_day
                    ),
                    suggested_action: "Re-run training via LMS, require re-attestation".into(),
                    day_index: Some(claim.claimed_day),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pattern 4 — Shift coverage gap
// ---------------------------------------------------------------------------
pub fn detect_shift_coverage(ledger: &OpsEventLedger) -> Vec<Gap> {
    ledger
        .shift_adequacy
        .iter()
        .filter(|s| !s.adequate)
        .map(|s| Gap {
            pattern: GapPattern::ShiftCoverageGap,
            severity: GapSeverity::Medium,
            store_ref: Some(s.store_ref.clone()),
            subject: format!("{}::day-{}", s.store_ref, s.day_index),
            message: format!(
                "Day {}: labor_hours={:.1} vs required={:.1} (shortfall)",
                s.day_index, s.labor_hours, s.required_hours
            ),
            suggested_action: "Adjust schedule; call in on-call staff; flag to DM".into(),
            day_index: Some(s.day_index),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pattern 5 — Staffing anti-pattern cluster
// ---------------------------------------------------------------------------
//
// Flag the top-quintile of stores by a composite anti-pattern score:
// vacancy_rate + turnover_rate + avg_turnover_risk/2. Threshold adapts to the
// actual feature distribution — works at any scale and any generator setting.
pub fn detect_staffing_anti_patterns(dims: &Dimensions) -> Vec<Gap> {
    let features: Vec<(String, f64)> = dims
        .stores
        .iter()
        .map(|store| {
            let positions: Vec<_> = dims
                .positions
                .iter()
                .filter(|p| p.store_ref == store.label)
                .collect();
            let vacancy_rate = if positions.is_empty() {
                0.0
            } else {
                positions
                    .iter()
                    .filter(|p| p.filled_by_ref.is_none())
                    .count() as f64
                    / positions.len() as f64
            };
            let people: Vec<_> = dims
                .people
                .iter()
                .filter(|p| p.home_store_ref == store.label)
                .collect();
            let turnover_rate = if people.is_empty() {
                0.0
            } else {
                people
                    .iter()
                    .filter(|p| p.status == PersonStatus::Terminated)
                    .count() as f64
                    / people.len() as f64
            };
            let avg_risk = if people.is_empty() {
                0.0
            } else {
                people.iter().map(|p| p.turnover_risk_score).sum::<f64>() / people.len() as f64
            };
            let score = vacancy_rate + turnover_rate + avg_risk / 2.0;
            (store.label.clone(), score)
        })
        .collect();

    // Threshold: 80th percentile of the score distribution, with a floor so we
    // don't flag stores that are merely above-average in a healthy universe.
    let mut sorted_scores: Vec<f64> = features.iter().map(|(_, s)| *s).collect();
    sorted_scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((sorted_scores.len() as f64) * 0.80).ceil() as usize;
    let p80 = sorted_scores
        .get(idx.min(sorted_scores.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0);
    let threshold = p80.max(0.20);

    features
        .into_iter()
        .filter(|(_, s)| *s >= threshold)
        .map(|(label, score)| Gap {
            pattern: GapPattern::StaffingAntiPatternCluster,
            severity: GapSeverity::High,
            store_ref: Some(label.clone()),
            subject: label.clone(),
            message: format!(
                "Store scores {:.3} on composite staffing-anti-pattern index (p80 threshold {:.3})",
                score, threshold
            ),
            suggested_action: "Assign to DM for full staffing review".into(),
            day_index: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pattern 6 — Inventory counts missing on cadence
// ---------------------------------------------------------------------------
pub fn detect_inventory_missing(
    config: &GeneratorConfig,
    dims: &Dimensions,
    ledger: &OpsEventLedger,
    days: u32,
) -> Vec<Gap> {
    let mut out = Vec::new();
    for store in &dims.stores {
        let mut expected = config.inventory_cadence_days;
        while expected < days {
            let observed_near = ledger.inventory.iter().any(|iv| {
                iv.store_ref == store.label
                    && iv.day_index >= expected.saturating_sub(2)
                    && iv.day_index <= expected + 2
            });
            if !observed_near {
                out.push(Gap {
                    pattern: GapPattern::InventoryNotPerformed,
                    severity: GapSeverity::High,
                    store_ref: Some(store.label.clone()),
                    subject: format!("{}::inv-day-{}", store.label, expected),
                    message: format!(
                        "No inventory count near day {} (cadence {}d)",
                        expected, config.inventory_cadence_days
                    ),
                    suggested_action: "Schedule immediate count; audit shrinkage ledger".into(),
                    day_index: Some(expected),
                });
            }
            expected += config.inventory_cadence_days;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pattern 7 — Turnover contagion cluster
// ---------------------------------------------------------------------------
pub fn detect_turnover_clusters(dims: &Dimensions, _days: u32) -> Vec<Gap> {
    let mut out = Vec::new();
    for store in &dims.stores {
        let people: Vec<_> = dims
            .people
            .iter()
            .filter(|p| p.home_store_ref == store.label)
            .collect();
        let total = people.len();
        if total == 0 {
            continue;
        }
        let terminated = people
            .iter()
            .filter(|p| p.status == PersonStatus::Terminated)
            .count();
        let rate = terminated as f64 / total as f64;
        if rate >= 0.20 && terminated >= 3 {
            out.push(Gap {
                pattern: GapPattern::TurnoverContagionCluster,
                severity: GapSeverity::High,
                store_ref: Some(store.label.clone()),
                subject: store.label.clone(),
                message: format!(
                    "{} of {} employees terminated ({:.1}% turnover cluster)",
                    terminated,
                    total,
                    rate * 100.0
                ),
                suggested_action: "Exit-interview trend analysis; schedule DM intervention".into(),
                day_index: None,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pattern 8 — Audits missing on cadence
// ---------------------------------------------------------------------------
pub fn detect_audits_missing(
    config: &GeneratorConfig,
    dims: &Dimensions,
    ledger: &OpsEventLedger,
    days: u32,
) -> Vec<Gap> {
    let mut out = Vec::new();
    for store in &dims.stores {
        let mut expected = config.audit_cadence_days;
        while expected < days {
            let observed_near = ledger.audits.iter().any(|a| {
                a.store_ref == store.label
                    && a.day_index >= expected.saturating_sub(2)
                    && a.day_index <= expected + 2
            });
            if !observed_near {
                out.push(Gap {
                    pattern: GapPattern::AuditNotPerformed,
                    severity: GapSeverity::Critical,
                    store_ref: Some(store.label.clone()),
                    subject: format!("{}::audit-day-{}", store.label, expected),
                    message: format!(
                        "No food-safety audit near day {} (cadence {}d)",
                        expected, config.audit_cadence_days
                    ),
                    suggested_action: "Block DM sign-off until audit performed".into(),
                    day_index: Some(expected),
                });
            }
            expected += config.audit_cadence_days;
        }
    }
    out
}
