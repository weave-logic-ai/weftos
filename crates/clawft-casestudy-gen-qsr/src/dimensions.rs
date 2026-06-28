//! Static dimension tables: stores, people, positions, promotions.

use crate::config::GeneratorConfig;
use crate::rng::{stable_hash_hex, subseed};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub label: String,
    pub brand: String,
    pub store_number: String,
    pub region_code: String,
    pub metro_code: String,
    pub franchise_model: String,
    pub opened_year: u32,
    pub baseline_daily_sales: f64,
    pub timezone: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonStatus {
    Active,
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingClaim {
    pub cert_name: String,
    pub claimed_day: u32,
    /// Whether the LMS has a corroborating record. `false` = unverified gap.
    pub lms_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub label: String,
    pub subtype: String,
    pub employee_id_hashed: String,
    pub hire_year: u32,
    pub home_store_ref: String,
    pub turnover_risk_score: f64,
    #[serde(default = "default_person_status")]
    pub status: PersonStatus,
    #[serde(default)]
    pub termination_day: Option<u32>,
    /// Certification → expiration day_index. Phase-3 pattern #2 detects certs
    /// whose expiration falls within the corpus window without a renewal event.
    #[serde(default)]
    pub cert_expirations: BTreeMap<String, u32>,
    /// Self-reported trainings. Phase-3 pattern #3 checks for LMS corroboration.
    #[serde(default)]
    pub claimed_trainings: Vec<TrainingClaim>,
}

fn default_person_status() -> PersonStatus {
    PersonStatus::Active
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub label: String,
    pub store_ref: String,
    pub role_template: String,
    pub filled_by_ref: Option<String>,
    pub critical: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Promotion {
    pub label: String,
    pub brand: String,
    pub true_lift: f64,
    pub discount_pct: f64,
    pub start_day: u32,
    pub end_day: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimensions {
    pub stores: Vec<Store>,
    pub people: Vec<Person>,
    pub positions: Vec<Position>,
    pub promotions: Vec<Promotion>,
}

const POSITION_TEMPLATES: &[&str] = &["general_manager", "shift_lead", "crew_supervisor"];

pub fn generate(config: &GeneratorConfig) -> Dimensions {
    let stores = generate_stores(config);
    let positions = generate_positions(config, &stores);
    let people = generate_people(config, &stores);
    let promotions = generate_promotions(config);
    Dimensions {
        stores,
        people,
        positions,
        promotions,
    }
}

fn generate_stores(config: &GeneratorConfig) -> Vec<Store> {
    let total = config.scale_tier.store_count();
    let per_brand = total.div_ceil(config.brands.len().max(1));
    let mut stores = Vec::with_capacity(total);
    let mut idx: u64 = 0;

    for brand in &config.brands {
        for _ in 0..per_brand {
            if stores.len() >= total {
                break;
            }
            let mut rng = subseed(config.seed, "store", idx);
            // Round-robin metro assignment keeps the distribution deterministic
            // across seeds so scenarios scoped to `brand × metro` always hit
            // at least one store at small tiers.
            let metro = &config.metros[idx as usize % config.metros.len()];
            let franchise_model = if rng.gen_bool(0.7) {
                "franchised"
            } else {
                "corporate"
            };
            let store_num = format!("{:04}", idx);
            let baseline_noise: f64 = rng.gen_range(0.7..1.4);
            stores.push(Store {
                label: format!("store:{}:{}_{}", brand.code, metro.code, store_num),
                brand: brand.code.clone(),
                store_number: store_num,
                region_code: metro.region.clone(),
                metro_code: metro.code.clone(),
                franchise_model: franchise_model.into(),
                opened_year: 2000 + rng.gen_range(0u32..=25),
                baseline_daily_sales: (brand.baseline_daily_sales * baseline_noise * 100.0).round()
                    / 100.0,
                timezone: "UTC-05".into(),
            });
            idx += 1;
        }
    }
    stores
}

fn generate_positions(config: &GeneratorConfig, stores: &[Store]) -> Vec<Position> {
    let mut out = Vec::with_capacity(stores.len() * POSITION_TEMPLATES.len());
    let mut idx: u64 = 0;
    for store in stores {
        for template in POSITION_TEMPLATES {
            let mut rng = subseed(config.seed, "position", idx);
            let filled = rng.gen_bool(1.0 - config.vacancy_rate);
            out.push(Position {
                label: format!(
                    "position:{}_{}_{}",
                    store.brand, store.store_number, template
                ),
                store_ref: store.label.clone(),
                role_template: (*template).into(),
                filled_by_ref: if filled {
                    Some(format!(
                        "person:employee:{}_{}_e{:03}",
                        store.brand,
                        store.store_number,
                        idx % 100
                    ))
                } else {
                    None
                },
                critical: *template == "general_manager",
            });
            idx += 1;
        }
    }
    out
}

fn generate_people(config: &GeneratorConfig, stores: &[Store]) -> Vec<Person> {
    let mut out = Vec::new();
    let days = config.scale_tier.days();
    for (sidx, store) in stores.iter().enumerate() {
        let mut rng_head = subseed(config.seed, "empcount", sidx as u64);
        let employee_count = rng_head.gen_range(15usize..30);
        for e in 0..employee_count {
            let mut rng = subseed(config.seed, "person", (sidx as u64) * 1000 + e as u64);
            let emp_id_plain = format!("{}_{}_e{:03}", store.brand, store.store_number, e);
            let emp_id_hashed = format!(
                "blake3:{}",
                stable_hash_hex(&format!("salt-tenant-qsr::{}", emp_id_plain))
            );
            let subtype = match rng.gen_range(0u32..10) {
                0 => "manager",
                1..=2 => "salaried",
                _ => "hourly",
            };
            let turnover_risk = (rng.gen_range(0.0f64..0.6) * 100.0).round() / 100.0;

            // Phase-3: synthetic terminations. Higher turnover_risk → more
            // likely to be terminated sometime during the corpus window.
            let (status, termination_day) = if rng
                .gen_bool((turnover_risk * config.turnover_rate_multiplier).clamp(0.0, 0.9))
            {
                (PersonStatus::Terminated, Some(rng.gen_range(0..days)))
            } else {
                (PersonStatus::Active, None)
            };

            // Cert expirations — every hourly/salaried/manager needs food-safety.
            let mut cert_expirations = BTreeMap::new();
            let expires_during_window = rng.gen_bool(config.cert_expiration_rate);
            let expiration_day = if expires_during_window {
                rng.gen_range(0..days)
            } else {
                days + rng.gen_range(30..365) // well beyond window
            };
            cert_expirations.insert("food-safety-national".to_string(), expiration_day);
            if subtype == "manager" {
                cert_expirations.insert(
                    format!("{}-gm-cert", store.brand),
                    if rng.gen_bool(0.10) {
                        rng.gen_range(0..days)
                    } else {
                        days + rng.gen_range(30..365)
                    },
                );
            }

            // Claimed trainings — some fraction lack LMS corroboration.
            let mut claimed_trainings = Vec::new();
            if rng.gen_bool(config.training_claim_rate) {
                claimed_trainings.push(TrainingClaim {
                    cert_name: "food-safety-national".into(),
                    claimed_day: rng.gen_range(0..days),
                    lms_verified: rng.gen_bool(1.0 - config.training_missing_lms_rate),
                });
            }

            out.push(Person {
                label: format!("person:employee:{}", emp_id_plain),
                subtype: subtype.into(),
                employee_id_hashed: emp_id_hashed,
                hire_year: 2020 + rng.gen_range(0u32..=5),
                home_store_ref: store.label.clone(),
                turnover_risk_score: turnover_risk,
                status,
                termination_day,
                cert_expirations,
                claimed_trainings,
            });
        }
    }
    out
}

fn generate_promotions(config: &GeneratorConfig) -> Vec<Promotion> {
    let days = config.scale_tier.days();
    let mut out = Vec::with_capacity(config.promo_catalog.len());
    for (i, def) in config.promo_catalog.iter().enumerate() {
        let mut rng = subseed(config.seed, "promo", i as u64);
        let start = if days > 14 {
            rng.gen_range(0..days.saturating_sub(14))
        } else {
            0
        };
        let duration = rng.gen_range(7..=21);
        out.push(Promotion {
            label: format!("promotion:{}", def.id),
            brand: def.brand.clone(),
            true_lift: def.true_lift,
            discount_pct: def.discount_pct,
            start_day: start,
            end_day: (start + duration).min(days),
        });
    }
    out
}
