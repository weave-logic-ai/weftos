//! Generator configuration. All anonymized — synthetic brands, metros, promos.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScaleTier {
    Tiny,
    Small,
    Medium,
    Full,
}

impl ScaleTier {
    pub fn store_count(self) -> usize {
        match self {
            ScaleTier::Tiny => 10,
            ScaleTier::Small => 500,
            ScaleTier::Medium => 5_000,
            ScaleTier::Full => 30_000,
        }
    }

    pub fn days(self) -> u32 {
        match self {
            ScaleTier::Tiny => 30,
            ScaleTier::Small => 90,
            ScaleTier::Medium => 365,
            ScaleTier::Full => 365 * 5,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tiny" => Some(Self::Tiny),
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrandConfig {
    pub code: String,
    pub baseline_daily_sales: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetroConfig {
    pub code: String,
    pub region: String,
    pub weather_rain_penalty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromoDef {
    pub id: String,
    pub brand: String,
    pub true_lift: f64,
    pub discount_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorConfig {
    pub seed: u64,
    pub scale_tier: ScaleTier,
    pub brands: Vec<BrandConfig>,
    pub metros: Vec<MetroConfig>,
    pub promo_catalog: Vec<PromoDef>,
    /// Monday..Sunday revenue multipliers.
    pub weekly_seasonality: [f64; 7],
    /// Amplitude of yearly sinusoidal component (0.0 = none).
    pub yearly_amplitude: f64,
    /// Gaussian noise sigma applied multiplicatively (1.0 + N(0, sigma)).
    pub noise_sigma: f64,
    /// Budget is set at baseline * (1 + budget_optimism) * seasonality.
    pub budget_optimism: f64,
    /// Fraction of positions intentionally left vacant (0.0 = all filled).
    pub vacancy_rate: f64,
    pub start_date: String,

    // --- Phase 3: gap-generation knobs --------------------------------------
    /// Multiplier on per-person turnover risk → termination probability.
    #[serde(default = "default_turnover_multiplier")]
    pub turnover_rate_multiplier: f64,
    /// Fraction of employees whose food-safety cert expires during the window.
    #[serde(default = "default_cert_expiration_rate")]
    pub cert_expiration_rate: f64,
    /// Fraction of employees with any claimed training record.
    #[serde(default = "default_training_claim_rate")]
    pub training_claim_rate: f64,
    /// Fraction of claimed trainings that have NO corroborating LMS record.
    #[serde(default = "default_training_missing_lms_rate")]
    pub training_missing_lms_rate: f64,
    /// Inventory cadence in days (every N days, a count should happen).
    #[serde(default = "default_inventory_cadence")]
    pub inventory_cadence_days: u32,
    /// Probability that any given scheduled inventory is skipped.
    #[serde(default = "default_inventory_skip_rate")]
    pub inventory_skip_rate: f64,
    /// Audit cadence in days.
    #[serde(default = "default_audit_cadence")]
    pub audit_cadence_days: u32,
    /// Probability that any given scheduled audit is skipped.
    #[serde(default = "default_audit_skip_rate")]
    pub audit_skip_rate: f64,
    /// Fraction of expiring certs that get renewed before expiration.
    #[serde(default = "default_cert_renewal_rate")]
    pub cert_renewal_rate: f64,
    /// Fraction of shifts where labor_hours fall below the daypart requirement.
    #[serde(default = "default_shift_gap_rate")]
    pub shift_gap_rate: f64,
}

fn default_turnover_multiplier() -> f64 {
    0.15
}
fn default_cert_expiration_rate() -> f64 {
    0.25
}
fn default_training_claim_rate() -> f64 {
    0.35
}
fn default_training_missing_lms_rate() -> f64 {
    0.15
}
fn default_inventory_cadence() -> u32 {
    7
}
fn default_inventory_skip_rate() -> f64 {
    0.20
}
fn default_audit_cadence() -> u32 {
    10
}
fn default_audit_skip_rate() -> f64 {
    0.20
}
fn default_cert_renewal_rate() -> f64 {
    0.70
}
fn default_shift_gap_rate() -> f64 {
    0.10
}

impl GeneratorConfig {
    /// Default tiny-tier config: 10 stores × 1 brand × 4 metros × 30 days.
    pub fn tiny(seed: u64) -> Self {
        Self::default_for_tier(seed, ScaleTier::Tiny)
    }

    pub fn default_for_tier(seed: u64, tier: ScaleTier) -> Self {
        Self {
            seed,
            scale_tier: tier,
            brands: default_brands(),
            metros: default_metros(),
            promo_catalog: default_promos(),
            weekly_seasonality: [0.90, 0.85, 0.92, 1.00, 1.18, 1.22, 0.95],
            yearly_amplitude: 0.08,
            noise_sigma: 0.05,
            budget_optimism: 0.03,
            vacancy_rate: 0.08,
            start_date: "2026-01-01".into(),
            turnover_rate_multiplier: default_turnover_multiplier(),
            cert_expiration_rate: default_cert_expiration_rate(),
            training_claim_rate: default_training_claim_rate(),
            training_missing_lms_rate: default_training_missing_lms_rate(),
            inventory_cadence_days: default_inventory_cadence(),
            inventory_skip_rate: default_inventory_skip_rate(),
            audit_cadence_days: default_audit_cadence(),
            audit_skip_rate: default_audit_skip_rate(),
            cert_renewal_rate: default_cert_renewal_rate(),
            shift_gap_rate: default_shift_gap_rate(),
        }
    }
}

fn default_brands() -> Vec<BrandConfig> {
    vec![
        BrandConfig {
            code: "brand-a".into(),
            baseline_daily_sales: 5400.0,
        },
        BrandConfig {
            code: "brand-b".into(),
            baseline_daily_sales: 4200.0,
        },
        BrandConfig {
            code: "brand-c".into(),
            baseline_daily_sales: 4800.0,
        },
        BrandConfig {
            code: "brand-d".into(),
            baseline_daily_sales: 3900.0,
        },
    ]
}

fn default_metros() -> Vec<MetroConfig> {
    vec![
        MetroConfig {
            code: "metro-alpha".into(),
            region: "region-1".into(),
            weather_rain_penalty: 0.08,
        },
        MetroConfig {
            code: "metro-beta".into(),
            region: "region-1".into(),
            weather_rain_penalty: 0.05,
        },
        MetroConfig {
            code: "metro-gamma".into(),
            region: "region-2".into(),
            weather_rain_penalty: 0.06,
        },
        MetroConfig {
            code: "metro-delta".into(),
            region: "region-2".into(),
            weather_rain_penalty: 0.10,
        },
    ]
}

fn default_promos() -> Vec<PromoDef> {
    vec![
        PromoDef {
            id: "promo-2for6-signature".into(),
            brand: "brand-a".into(),
            true_lift: 0.12,
            discount_pct: 0.15,
        },
        PromoDef {
            id: "promo-app-exclusive-25".into(),
            brand: "brand-a".into(),
            true_lift: 0.04,
            discount_pct: 0.25,
        },
        PromoDef {
            id: "promo-family-meal-deal".into(),
            brand: "brand-b".into(),
            true_lift: 0.08,
            discount_pct: 0.10,
        },
    ]
}
