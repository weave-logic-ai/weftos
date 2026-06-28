//! Phase 1 — DEMOCRITUS-style ingest pipeline.
//!
//! Drives the loop: SENSE (drain impulses) → EMBED (n/a for test harness) →
//! SEARCH (look up routing) → UPDATE (apply to shard + case graph) → COMMIT
//! (audit counters). One `IngestWorker` per brand, backed by a shared
//! `ShardSet`, `Governance`, and `CaseGraph`.

use crate::audit::{AuditKind, HashChainAuditor};
use crate::dimensions::Dimensions;
use crate::events::DailyRollup;
use crate::governance::{Governance, Severity};
use crate::graph::{CaseGraph, EdgeType, Node, NodeType, build as build_graph};
use crate::impulse::{Impulse, ImpulseQueue};
use crate::shard::{ShardRouter, ShardSet, StoredRollup};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestStats {
    pub impulses_emitted: u64,
    pub impulses_applied: u64,
    pub impulses_blocked: u64,
    pub dropped_duplicates: u64,
    pub late_arrivals: u64,
    pub ticks: u64,
    pub wall_ms: u64,
}

pub struct IngestDriver {
    pub queue: ImpulseQueue,
    pub router: ShardRouter,
    pub shards: ShardSet,
    pub graph: CaseGraph,
    pub governance: Governance,
    pub stats: IngestStats,
    /// Max impulses drained per tick (matches DemocritusConfig.max_impulses_per_tick).
    pub max_per_tick: usize,
    /// Hash-chained audit trail (Phase 4).
    pub auditor: HashChainAuditor,
    /// Lookup: store_ref → (brand, region, metro) for impulse construction.
    store_ctx: HashMap<String, (String, String, String)>,
}

impl IngestDriver {
    /// Build a fresh driver with an empty graph seeded only with dimension nodes.
    pub fn new(dims: &Dimensions) -> Self {
        // Seed the graph with all dimension nodes but no daily rollups yet —
        // those get inserted as impulses flow through.
        let empty_events: Vec<DailyRollup> = Vec::new();
        let graph = build_graph(dims, &empty_events);

        let store_ctx = dims
            .stores
            .iter()
            .map(|s| {
                (
                    s.label.clone(),
                    (s.brand.clone(), s.region_code.clone(), s.metro_code.clone()),
                )
            })
            .collect();

        Self {
            queue: ImpulseQueue::new(),
            router: ShardRouter::new(),
            shards: ShardSet::default(),
            graph,
            governance: Governance::new(),
            stats: IngestStats::default(),
            max_per_tick: 64,
            auditor: HashChainAuditor::new(),
            store_ctx,
        }
    }

    /// Build an impulse for a rollup using the pre-indexed store context.
    pub fn impulse_for(&self, ev: &DailyRollup) -> Option<Impulse> {
        let (brand, region, metro) = self.store_ctx.get(&ev.store_ref)?.clone();
        Some(Impulse::from_rollup_with_context(
            ev, &brand, &region, &metro,
        ))
    }

    /// Emit all given rollups as impulses (feeds the stream).
    pub fn emit_stream(&mut self, events: &[DailyRollup]) {
        for ev in events {
            if let Some(imp) = self.impulse_for(ev)
                && self.queue.emit(imp)
            {
                self.stats.impulses_emitted += 1;
            }
        }
    }

    /// One tick of the DEMOCRITUS loop: drain up to `max_per_tick` impulses
    /// and process them.
    pub fn tick(&mut self) -> usize {
        let t0 = std::time::Instant::now();
        let batch = self.queue.drain_ready(self.max_per_tick);
        let n = batch.len();
        for imp in batch {
            self.apply_one(imp);
        }
        self.stats.ticks += 1;
        self.stats.wall_ms += t0.elapsed().as_millis() as u64;
        n
    }

    /// Drain the queue to completion, one tick at a time.
    pub fn run_to_completion(&mut self) {
        while self.queue.pending() > 0 {
            self.tick();
        }
        let qstats = self.queue.stats();
        self.stats.dropped_duplicates = qstats.dropped_duplicates;
        self.stats.late_arrivals = qstats.late_arrivals;
    }

    fn apply_one(&mut self, impulse: Impulse) {
        // Governance gate
        let decision = self.governance.evaluate(&impulse);
        if !decision.permitted && decision.severity == Severity::Blocking {
            self.stats.impulses_blocked += 1;
            self.auditor.record(
                AuditKind::ImpulseBlocked,
                format!("{} — rule={}", impulse.idempotency_key, decision.rule),
                &serde_json::json!({
                    "idempotency_key": impulse.idempotency_key,
                    "rule": decision.rule,
                    "reason": decision.reason,
                }),
            );
            return;
        }

        // Shard routing + write
        let key = self.router.route(&impulse);
        let shard = self.shards.ensure(&key);
        shard.impulses += 1;
        shard.bytes_written += std::mem::size_of::<Impulse>() as u64;
        shard.daily_rollups.push(StoredRollup {
            store_ref: impulse.store_ref.clone(),
            day_index: impulse.day_index,
            business_date: impulse.business_date.clone(),
            revenue: impulse.revenue,
            budget_revenue: impulse.budget_revenue,
            reconstructed_from_lake: impulse.reconstructed_from_lake,
        });

        // Case graph update: insert a DailyRollup node + ClosedDay edge +
        // Causes edges from any matching active promotions.
        self.graph_upsert_daily(&impulse);

        self.stats.impulses_applied += 1;
        self.auditor.record(
            AuditKind::ImpulseApplied,
            format!("{} → {}", impulse.idempotency_key, key.path()),
            &serde_json::json!({
                "idempotency_key": impulse.idempotency_key,
                "shard": key.path(),
                "revenue": impulse.revenue,
                "reconstructed_from_lake": impulse.reconstructed_from_lake,
            }),
        );
    }

    fn graph_upsert_daily(&mut self, impulse: &Impulse) {
        // If a node for this label already exists (BeliefUpdate revisiting
        // a rollup), update its metric in place.
        if let Some(existing_id) = self.graph.by_label(&impulse.business_date_label()) {
            if let Some(node) = self.graph.nodes.get_mut(existing_id as usize) {
                node.metric = Some(impulse.revenue);
            }
            return;
        }

        let Some(&store_id) = self.graph.label_index.get(&impulse.store_ref) else {
            return;
        };
        let store = self.graph.nodes[store_id as usize].clone();

        let daily_id = self.graph.nodes.len() as u32;
        let daily_label = impulse.business_date_label();
        self.graph.nodes.push(Node {
            id: daily_id,
            label: daily_label.clone(),
            node_type: NodeType::DailyRollup,
            metric: Some(impulse.revenue),
            brand: store.brand.clone(),
            metro: store.metro.clone(),
            region: store.region.clone(),
            day_index: Some(impulse.day_index),
            week_index: Some(impulse.day_index / 7),
            store_ref: Some(impulse.store_ref.clone()),
        });
        self.graph.label_index.insert(daily_label, daily_id);

        let edge_idx = self.graph.edges.len();
        self.graph.edges.push(crate::graph::Edge {
            source: store_id,
            target: daily_id,
            edge_type: EdgeType::ClosedDay,
            weight: 1.0,
            provenance: crate::graph::Provenance::Structural,
        });
        self.graph.fwd.entry(store_id).or_default().push(edge_idx);
        self.graph.rev.entry(daily_id).or_default().push(edge_idx);

        // Wire Causes edges from active promotions for this brand.
        let promo_labels: Vec<String> = impulse.promo_codes_active.to_vec();
        for plabel in promo_labels {
            let Some(&promo_id) = self.graph.label_index.get(&plabel) else {
                continue;
            };
            let weight = self.graph.nodes[promo_id as usize].metric.unwrap_or(0.0);
            let edge_idx = self.graph.edges.len();
            self.graph.edges.push(crate::graph::Edge {
                source: promo_id,
                target: daily_id,
                edge_type: EdgeType::Causes,
                weight,
                provenance: crate::graph::Provenance::GroundTruth,
            });
            self.graph.fwd.entry(promo_id).or_default().push(edge_idx);
            self.graph.rev.entry(daily_id).or_default().push(edge_idx);
        }
    }
}

impl Impulse {
    fn business_date_label(&self) -> String {
        // Same scheme as events.rs, so the graph is comparable between batch
        // and stream paths.
        // store_ref format: "store:<brand>:<metro>_<num>"
        let store_num = self.store_ref.rsplit('_').next().unwrap_or(&self.store_ref);
        format!("day:{}:{}:{}", self.brand, store_num, self.business_date)
    }
}
