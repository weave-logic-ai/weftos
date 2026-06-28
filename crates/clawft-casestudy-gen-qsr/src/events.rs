//! Daily-rollup event stream.
//!
//! Revenue model for each (store, day):
//!   revenue = baseline × weekly_seasonality[dow] × (1 + yearly_amplitude·sin(2πt/365))
//!            × (1 + Σ active_promo.true_lift) × (1 + N(0, noise_sigma))
//!
//! Budget is baseline × weekly × yearly × (1 + budget_optimism). Variance is
//! (revenue − budget) / budget, pre-computed so scoring can check sign/magnitude
//! against the truth manifest.

use crate::config::GeneratorConfig;
use crate::dimensions::{Dimensions, Promotion, Store};
use crate::rng::subseed;
use chrono::{Datelike, Duration, NaiveDate};
use rand::Rng;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyRollup {
    pub label: String,
    pub store_ref: String,
    pub business_date: String,
    pub day_index: u32,
    pub tickets: u32,
    pub revenue: f64,
    pub cogs: f64,
    pub labor: f64,
    pub labor_hours: f64,
    pub avg_ticket: f64,
    pub budget_revenue: f64,
    pub budget_variance_pct: f64,
    pub promo_codes_active: Vec<String>,
    pub verified: bool,
}

pub fn generate(config: &GeneratorConfig, dims: &Dimensions) -> Vec<DailyRollup> {
    let days = config.scale_tier.days();
    let start_date = NaiveDate::parse_from_str(&config.start_date, "%Y-%m-%d")
        .unwrap_or_else(|_| NaiveDate::from_ymd_opt(2026, 1, 1).expect("static date"));
    let mut out = Vec::with_capacity(dims.stores.len() * days as usize);

    let normal = Normal::new(0.0f64, config.noise_sigma.max(1e-9))
        .expect("noise_sigma produces a valid Normal");

    for (sidx, store) in dims.stores.iter().enumerate() {
        for day in 0..days {
            let rollup = synth_rollup(
                config,
                store,
                &dims.promotions,
                sidx,
                day,
                start_date,
                &normal,
            );
            out.push(rollup);
        }
    }
    out
}

fn synth_rollup(
    config: &GeneratorConfig,
    store: &Store,
    promos: &[Promotion],
    sidx: usize,
    day: u32,
    start_date: NaiveDate,
    normal: &Normal<f64>,
) -> DailyRollup {
    let mut rng = subseed(config.seed, "rollup", (sidx as u64) * 100_000 + day as u64);
    let date = start_date + Duration::days(day as i64);
    let dow = date.weekday().number_from_monday() as usize - 1;

    let weekly = config.weekly_seasonality[dow];
    let t = (day as f64 / 365.0) * std::f64::consts::TAU;
    let yearly = 1.0 + config.yearly_amplitude * t.sin();

    let active: Vec<&Promotion> = promos
        .iter()
        .filter(|p| p.brand == store.brand && day >= p.start_day && day < p.end_day)
        .collect();
    let promo_lift: f64 = active.iter().map(|p| p.true_lift).sum();

    let noise = normal.sample(&mut rng);
    let multiplier = weekly * yearly * (1.0 + promo_lift) * (1.0 + noise);
    let revenue = (store.baseline_daily_sales * multiplier).max(0.0);

    let avg_ticket = (11.0 + rng.gen_range(-1.5f64..1.5)).max(6.0);
    let tickets = (revenue / avg_ticket).round() as u32;
    let cogs = revenue * rng.gen_range(0.28f64..0.32);
    let labor_hours = (tickets as f64 / 6.0).max(24.0);
    let labor = labor_hours * rng.gen_range(14.0f64..18.0);

    let budget = store.baseline_daily_sales * weekly * yearly * (1.0 + config.budget_optimism);
    let variance = if budget.abs() > f64::EPSILON {
        (revenue - budget) / budget
    } else {
        0.0
    };

    DailyRollup {
        label: format!(
            "day:{}:{}:{}",
            store.brand,
            store.store_number,
            date.format("%Y-%m-%d")
        ),
        store_ref: store.label.clone(),
        business_date: date.to_string(),
        day_index: day,
        tickets,
        revenue: round2(revenue),
        cogs: round2(cogs),
        labor: round2(labor),
        labor_hours: (labor_hours * 10.0).round() / 10.0,
        avg_ticket: round2(avg_ticket),
        budget_revenue: round2(budget),
        budget_variance_pct: (variance * 10_000.0).round() / 10_000.0,
        promo_codes_active: active.iter().map(|p| p.label.clone()).collect(),
        verified: true,
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}
