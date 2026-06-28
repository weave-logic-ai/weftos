//! Phase 3 — operational event ledger.
//!
//! These are the non-sales events that the gap-analysis engine checks cadence
//! against: inventory counts, food-safety audits, cert renewals, and the
//! per-day shift-coverage adequacy flags derived from the daily rollups.

use crate::config::GeneratorConfig;
use crate::dimensions::{Dimensions, Person};
use crate::events::DailyRollup;
use crate::rng::subseed;
use rand::Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryEvent {
    pub store_ref: String,
    pub day_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub store_ref: String,
    pub day_index: u32,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertRenewalEvent {
    pub person_ref: String,
    pub cert_name: String,
    pub day_index: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpsEventLedger {
    pub inventory: Vec<InventoryEvent>,
    pub audits: Vec<AuditEvent>,
    pub cert_renewals: Vec<CertRenewalEvent>,
    /// Per (store, day) flag: `true` if the shift was adequately staffed.
    /// Derived from labor_hours at generation time; gap-detector flips this
    /// inside-out to find uncovered shifts.
    pub shift_adequacy: Vec<ShiftAdequacy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShiftAdequacy {
    pub store_ref: String,
    pub day_index: u32,
    pub adequate: bool,
    pub labor_hours: f64,
    pub required_hours: f64,
}

pub fn generate(
    config: &GeneratorConfig,
    dims: &Dimensions,
    events: &[DailyRollup],
) -> OpsEventLedger {
    let days = config.scale_tier.days();

    let mut inventory = Vec::new();
    let mut audits = Vec::new();
    for (sidx, store) in dims.stores.iter().enumerate() {
        let mut rng = subseed(config.seed, "inventory", sidx as u64);
        let mut day = 0u32;
        while day < days {
            day += config.inventory_cadence_days;
            if day >= days {
                break;
            }
            if !rng.gen_bool(config.inventory_skip_rate) {
                inventory.push(InventoryEvent {
                    store_ref: store.label.clone(),
                    day_index: day,
                });
            }
        }
        let mut rng = subseed(config.seed, "audit", sidx as u64);
        let mut day = 0u32;
        while day < days {
            day += config.audit_cadence_days;
            if day >= days {
                break;
            }
            if !rng.gen_bool(config.audit_skip_rate) {
                audits.push(AuditEvent {
                    store_ref: store.label.clone(),
                    day_index: day,
                    kind: "food_safety".into(),
                });
            }
        }
    }

    // Cert renewals: for each expiration happening during the window, flip a
    // biased coin — `cert_renewal_rate` of them actually produce a renewal
    // event a few days prior. Missing renewals become gaps.
    let mut cert_renewals = Vec::new();
    for (pidx, person) in dims.people.iter().enumerate() {
        for (cert_name, &expiration) in &person.cert_expirations {
            if expiration >= days {
                continue;
            }
            let mut rng = subseed(config.seed, &format!("renewal::{}", cert_name), pidx as u64);
            if rng.gen_bool(config.cert_renewal_rate) {
                let renewal_day = expiration.saturating_sub(rng.gen_range(1..7));
                cert_renewals.push(CertRenewalEvent {
                    person_ref: person.label.clone(),
                    cert_name: cert_name.clone(),
                    day_index: renewal_day,
                });
            }
        }
    }

    // Shift adequacy: per (store, day) compare labor_hours to a required
    // baseline derived from the store's baseline sales. Inject random gaps
    // per `shift_gap_rate`.
    let mut shift_adequacy = Vec::with_capacity(events.len());
    let store_ref_to_required: std::collections::HashMap<String, f64> = dims
        .stores
        .iter()
        .map(|s| (s.label.clone(), (s.baseline_daily_sales / 90.0).max(24.0)))
        .collect();

    for (eidx, ev) in events.iter().enumerate() {
        let required = *store_ref_to_required.get(&ev.store_ref).unwrap_or(&48.0);
        let mut rng = subseed(config.seed, "shift", eidx as u64);
        let inject_gap = rng.gen_bool(config.shift_gap_rate);
        let labor = if inject_gap {
            ev.labor_hours * rng.gen_range(0.50..0.72)
        } else {
            ev.labor_hours
        };
        let adequate = labor >= required * 0.75;
        shift_adequacy.push(ShiftAdequacy {
            store_ref: ev.store_ref.clone(),
            day_index: ev.day_index,
            adequate,
            labor_hours: (labor * 10.0).round() / 10.0,
            required_hours: (required * 10.0).round() / 10.0,
        });
    }

    OpsEventLedger {
        inventory,
        audits,
        cert_renewals,
        shift_adequacy,
    }
}

/// True if this person's cert has a renewal after or equal to its expiration.
pub fn cert_is_renewed(person: &Person, cert_name: &str, renewals: &[CertRenewalEvent]) -> bool {
    let Some(&expiration) = person.cert_expirations.get(cert_name) else {
        return true; // no cert tracked → vacuously fine
    };
    renewals.iter().any(|r| {
        r.person_ref == person.label
            && r.cert_name == cert_name
            && r.day_index >= expiration.saturating_sub(14) // renewed within 14d of expiry
            && r.day_index <= expiration + 7
    })
}
