//! Phase 3+ — temporal rollup.
//!
//! Aggregates a daily-grain rollup stream into weekly (or arbitrary-N-day)
//! grain. Used for historic compaction: previous-year data is rolled up to
//! weekly so the hot HNSW index only needs to carry current-year dailies.

use crate::events::DailyRollup;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyRollup {
    pub label: String,
    pub store_ref: String,
    pub brand: String,
    pub week_start_date: String,
    pub week_index: u32,
    pub days_covered: u32,
    pub tickets: u64,
    pub revenue: f64,
    pub cogs: f64,
    pub labor: f64,
    pub labor_hours: f64,
    pub avg_ticket: f64,
    pub budget_revenue: f64,
    pub budget_variance_pct: f64,
    pub promo_codes_active_union: Vec<String>,
    pub reconstructed_from_daily: bool,
}

/// Group a slice of `DailyRollup` events by (store, week_index) and produce
/// one `WeeklyRollup` per group. `week_index` is `day_index / 7`.
pub fn roll_up_to_week(events: &[DailyRollup]) -> Vec<WeeklyRollup> {
    let mut groups: BTreeMap<(String, u32), Vec<&DailyRollup>> = BTreeMap::new();
    for ev in events {
        groups
            .entry((ev.store_ref.clone(), ev.day_index / 7))
            .or_default()
            .push(ev);
    }

    groups
        .into_iter()
        .map(|((store_ref, week_index), dailies)| {
            let days_covered = dailies.len() as u32;
            let tickets: u64 = dailies.iter().map(|d| d.tickets as u64).sum();
            let revenue: f64 = dailies.iter().map(|d| d.revenue).sum();
            let cogs: f64 = dailies.iter().map(|d| d.cogs).sum();
            let labor: f64 = dailies.iter().map(|d| d.labor).sum();
            let labor_hours: f64 = dailies.iter().map(|d| d.labor_hours).sum();
            let budget_revenue: f64 = dailies.iter().map(|d| d.budget_revenue).sum();
            let avg_ticket = if tickets > 0 {
                revenue / tickets as f64
            } else {
                0.0
            };
            let budget_variance_pct = if budget_revenue.abs() > f64::EPSILON {
                (revenue - budget_revenue) / budget_revenue
            } else {
                0.0
            };
            // Brand is stable per store — pull from the first daily's label
            // convention "day:<brand>:<store_num>:<date>".
            let brand = dailies[0].label.split(':').nth(1).unwrap_or("").to_string();
            let mut promo_union: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            for d in &dailies {
                for p in &d.promo_codes_active {
                    promo_union.insert(p.clone());
                }
            }
            let week_start_date = dailies
                .iter()
                .map(|d| d.business_date.clone())
                .min()
                .unwrap_or_default();
            WeeklyRollup {
                label: format!(
                    "week:{}:{}:{:03}",
                    brand,
                    store_num_from(&store_ref),
                    week_index
                ),
                store_ref,
                brand,
                week_start_date,
                week_index,
                days_covered,
                tickets,
                revenue: round2(revenue),
                cogs: round2(cogs),
                labor: round2(labor),
                labor_hours: (labor_hours * 10.0).round() / 10.0,
                avg_ticket: round2(avg_ticket),
                budget_revenue: round2(budget_revenue),
                budget_variance_pct: (budget_variance_pct * 10_000.0).round() / 10_000.0,
                promo_codes_active_union: promo_union.into_iter().collect(),
                reconstructed_from_daily: true,
            }
        })
        .collect()
}

fn store_num_from(store_ref: &str) -> String {
    // "store:<brand>:<metro>_<num>" → <num>
    store_ref.rsplit('_').next().unwrap_or("").to_string()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

// ---------------------------------------------------------------------------
// Monthly rollup (calendar-month, by business_date)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthlyRollup {
    pub label: String,
    pub store_ref: String,
    pub brand: String,
    pub year_month: String, // "YYYY-MM"
    pub month_start_date: String,
    pub days_covered: u32,
    pub tickets: u64,
    pub revenue: f64,
    pub cogs: f64,
    pub labor: f64,
    pub labor_hours: f64,
    pub avg_ticket: f64,
    pub budget_revenue: f64,
    pub budget_variance_pct: f64,
    pub promo_codes_active_union: Vec<String>,
    pub reconstructed_from_daily: bool,
}

/// Calendar-month rollup: groups by (store, `YYYY-MM` of business_date).
pub fn roll_up_to_month(events: &[DailyRollup]) -> Vec<MonthlyRollup> {
    let mut groups: BTreeMap<(String, String), Vec<&DailyRollup>> = BTreeMap::new();
    for ev in events {
        let ym = ev
            .business_date
            .get(0..7) // "YYYY-MM"
            .unwrap_or("0000-00")
            .to_string();
        groups
            .entry((ev.store_ref.clone(), ym))
            .or_default()
            .push(ev);
    }

    groups
        .into_iter()
        .map(|((store_ref, year_month), dailies)| {
            let days_covered = dailies.len() as u32;
            let tickets: u64 = dailies.iter().map(|d| d.tickets as u64).sum();
            let revenue: f64 = dailies.iter().map(|d| d.revenue).sum();
            let cogs: f64 = dailies.iter().map(|d| d.cogs).sum();
            let labor: f64 = dailies.iter().map(|d| d.labor).sum();
            let labor_hours: f64 = dailies.iter().map(|d| d.labor_hours).sum();
            let budget_revenue: f64 = dailies.iter().map(|d| d.budget_revenue).sum();
            let avg_ticket = if tickets > 0 {
                revenue / tickets as f64
            } else {
                0.0
            };
            let budget_variance_pct = if budget_revenue.abs() > f64::EPSILON {
                (revenue - budget_revenue) / budget_revenue
            } else {
                0.0
            };
            let brand = dailies[0].label.split(':').nth(1).unwrap_or("").to_string();
            let mut promo_union: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            for d in &dailies {
                for p in &d.promo_codes_active {
                    promo_union.insert(p.clone());
                }
            }
            let month_start_date = dailies
                .iter()
                .map(|d| d.business_date.clone())
                .min()
                .unwrap_or_default();
            MonthlyRollup {
                label: format!(
                    "month:{}:{}:{}",
                    brand,
                    store_num_from(&store_ref),
                    year_month
                ),
                store_ref,
                brand,
                year_month,
                month_start_date,
                days_covered,
                tickets,
                revenue: round2(revenue),
                cogs: round2(cogs),
                labor: round2(labor),
                labor_hours: (labor_hours * 10.0).round() / 10.0,
                avg_ticket: round2(avg_ticket),
                budget_revenue: round2(budget_revenue),
                budget_variance_pct: (budget_variance_pct * 10_000.0).round() / 10_000.0,
                promo_codes_active_union: promo_union.into_iter().collect(),
                reconstructed_from_daily: true,
            }
        })
        .collect()
}
