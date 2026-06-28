//! Phase 1 — shard routing.
//!
//! Routes impulses to `(brand, region, quarter)` shards per the analysis §4.
//! Keeps a global routing table and lazy-creates shards on first write.

use crate::impulse::Impulse;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ShardKey {
    pub brand: String,
    pub region: String,
    pub year: u32,
    pub quarter: u8, // 1..=4
}

impl ShardKey {
    pub fn path(&self) -> String {
        format!(
            "ops/{}/{}/{}-Q{}.rvf",
            self.brand, self.region, self.year, self.quarter
        )
    }
}

/// Routing table: decides which shard(s) a given impulse touches.
pub struct ShardRouter {
    /// Business-date start used to derive quarter boundaries.
    pub epoch_year: u32,
}

impl ShardRouter {
    pub fn new() -> Self {
        Self { epoch_year: 2026 }
    }

    pub fn route(&self, impulse: &Impulse) -> ShardKey {
        // Parse "2026-04-20" to year/quarter.
        let (year, quarter) =
            parse_year_quarter(&impulse.business_date).unwrap_or((self.epoch_year, 1));
        ShardKey {
            brand: impulse.brand.clone(),
            region: impulse.region.clone(),
            year,
            quarter,
        }
    }
}

impl Default for ShardRouter {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_year_quarter(business_date: &str) -> Option<(u32, u8)> {
    let d = chrono::NaiveDate::parse_from_str(business_date, "%Y-%m-%d").ok()?;
    let y = d.format("%Y").to_string().parse::<u32>().ok()?;
    let month: u32 = d.format("%m").to_string().parse().ok()?;
    let q = ((month - 1) / 3 + 1) as u8;
    Some((y, q))
}

/// Per-shard accumulated state. In production this would be backed by an RVF
/// file; for the Phase-1 test harness we hold rollups in memory so the engine
/// can compare stream-built vs. batch-built graphs directly.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ShardState {
    pub impulses: u64,
    pub bytes_written: u64,
    pub daily_rollups: Vec<StoredRollup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRollup {
    pub store_ref: String,
    pub day_index: u32,
    pub business_date: String,
    pub revenue: f64,
    pub budget_revenue: f64,
    pub reconstructed_from_lake: bool,
}

#[derive(Default)]
pub struct ShardSet {
    pub shards: BTreeMap<ShardKey, ShardState>,
}

impl ShardSet {
    pub fn ensure(&mut self, key: &ShardKey) -> &mut ShardState {
        self.shards.entry(key.clone()).or_default()
    }

    pub fn total_rollups(&self) -> usize {
        self.shards.values().map(|s| s.daily_rollups.len()).sum()
    }

    pub fn total_impulses(&self) -> u64 {
        self.shards.values().map(|s| s.impulses).sum()
    }
}
