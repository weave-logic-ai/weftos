//! Phase 1 — impulse types + HLC + queue.
//!
//! Modelled on `crates/clawft-kernel/src/impulse.rs`. Each DailyRollup from the
//! corpus becomes one `Impulse` with an HLC timestamp. The queue dedupes on
//! `(store_ref, day_index)` so duplicate or retried deltas are idempotent.

use crate::events::DailyRollup;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Hybrid Logical Clock: (physical_ms, logical_tick).
/// Monotone non-decreasing; ties broken by the logical field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hlc {
    pub physical_ms: u64,
    pub logical: u32,
}

impl Hlc {
    pub fn from_business_date(date: &str, day_index: u32) -> Self {
        // Anchor HLC physical_ms on the business date's Unix ms, with logical =
        // day_index so events within a day keep a stable ordering.
        let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .unwrap_or_else(|_| chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
        let dt = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        Hlc {
            physical_ms: dt.timestamp_millis() as u64,
            logical: day_index,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpulseType {
    /// New observation not seen before.
    NoveltyDetected,
    /// Revision to previously observed data (late arrival or correction).
    BeliefUpdate,
    /// Structural coherence violation — missing window, gap, anomaly.
    CoherenceAlert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Impulse {
    pub hlc: Hlc,
    pub kind: ImpulseType,
    pub store_ref: String,
    pub brand: String,
    pub region: String,
    pub metro: String,
    pub day_index: u32,
    pub business_date: String,
    pub revenue: f64,
    pub budget_revenue: f64,
    pub budget_variance_pct: f64,
    pub promo_codes_active: Vec<String>,
    /// True if this impulse was reconstructed from a backup (e.g. data lake)
    /// rather than delivered by the live stream.
    pub reconstructed_from_lake: bool,
    /// Idempotency key — `(store_ref, day_index)` concatenated.
    pub idempotency_key: String,
}

impl Impulse {
    pub fn from_rollup_with_context(
        ev: &DailyRollup,
        brand: &str,
        region: &str,
        metro: &str,
    ) -> Self {
        let idempotency_key = format!("{}::{}", ev.store_ref, ev.day_index);
        Self {
            hlc: Hlc::from_business_date(&ev.business_date, ev.day_index),
            kind: ImpulseType::NoveltyDetected,
            store_ref: ev.store_ref.clone(),
            brand: brand.to_string(),
            region: region.to_string(),
            metro: metro.to_string(),
            day_index: ev.day_index,
            business_date: ev.business_date.clone(),
            revenue: ev.revenue,
            budget_revenue: ev.budget_revenue,
            budget_variance_pct: ev.budget_variance_pct,
            promo_codes_active: ev.promo_codes_active.clone(),
            reconstructed_from_lake: false,
            idempotency_key,
        }
    }
}

/// Bounded FIFO impulse queue with idempotency dedupe and late-arrival
/// detection. Late-arriving impulses are rewritten to `BeliefUpdate` so
/// downstream processing knows they are revisions, not first-observations.
pub struct ImpulseQueue {
    queue: VecDeque<Impulse>,
    seen: HashMap<String, Hlc>,
    dropped_duplicates: u64,
    late_arrivals: u64,
    watermark: HashMap<String, Hlc>, // per-store max-hlc seen
    late_watermark_ms: u64,          // how far back "late" is still acceptable
}

impl ImpulseQueue {
    /// Default 24h late-arrival watermark.
    pub fn new() -> Self {
        Self::with_watermark_ms(24 * 3600 * 1000)
    }

    pub fn with_watermark_ms(ms: u64) -> Self {
        Self {
            queue: VecDeque::new(),
            seen: HashMap::new(),
            dropped_duplicates: 0,
            late_arrivals: 0,
            watermark: HashMap::new(),
            late_watermark_ms: ms,
        }
    }

    /// Emit one impulse into the queue. Applies dedupe + late-arrival policy.
    /// Returns true if enqueued, false if dropped as duplicate.
    pub fn emit(&mut self, mut impulse: Impulse) -> bool {
        // Duplicate detection: same idempotency_key already seen at >= this HLC.
        if let Some(prev_hlc) = self.seen.get(&impulse.idempotency_key)
            && *prev_hlc >= impulse.hlc
        {
            self.dropped_duplicates += 1;
            return false;
        }

        // Late arrival: HLC older than (store_watermark − watermark_ms)
        let store_watermark = self.watermark.get(&impulse.store_ref).copied();
        if let Some(wm) = store_watermark
            && wm.physical_ms > impulse.hlc.physical_ms + self.late_watermark_ms
        {
            impulse.kind = ImpulseType::BeliefUpdate;
            impulse.reconstructed_from_lake = true;
            self.late_arrivals += 1;
        }

        self.seen
            .insert(impulse.idempotency_key.clone(), impulse.hlc);
        self.watermark
            .entry(impulse.store_ref.clone())
            .and_modify(|w| {
                if impulse.hlc > *w {
                    *w = impulse.hlc;
                }
            })
            .or_insert(impulse.hlc);
        self.queue.push_back(impulse);
        true
    }

    /// Drain up to `max` ready impulses for processing.
    pub fn drain_ready(&mut self, max: usize) -> Vec<Impulse> {
        let n = max.min(self.queue.len());
        self.queue.drain(..n).collect()
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    pub fn stats(&self) -> ImpulseQueueStats {
        ImpulseQueueStats {
            pending: self.queue.len(),
            dropped_duplicates: self.dropped_duplicates,
            late_arrivals: self.late_arrivals,
        }
    }

    /// Detect missing-window coherence alerts: for each store, if
    /// consecutive day_indices are missing, emit an alert.
    pub fn detect_missing_windows(&self, store_refs: &[String]) -> Vec<MissingWindow> {
        let mut per_store: HashMap<&str, Vec<u32>> = HashMap::new();
        for imp in &self.queue {
            per_store
                .entry(imp.store_ref.as_str())
                .or_default()
                .push(imp.day_index);
        }
        let mut out = Vec::new();
        for store in store_refs {
            let seen: HashSet<u32> = per_store
                .get(store.as_str())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            if seen.is_empty() {
                continue;
            }
            let min = *seen.iter().min().unwrap();
            let max = *seen.iter().max().unwrap();
            for d in min..=max {
                if !seen.contains(&d) {
                    out.push(MissingWindow {
                        store_ref: store.clone(),
                        day_index: d,
                    });
                }
            }
        }
        out
    }
}

impl Default for ImpulseQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpulseQueueStats {
    pub pending: usize,
    pub dropped_duplicates: u64,
    pub late_arrivals: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingWindow {
    pub store_ref: String,
    pub day_index: u32,
}
