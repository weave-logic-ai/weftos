//! In-memory case graph built from a QSR corpus.
//!
//! This is the Phase-2 engine's working substrate. The node and edge types
//! deliberately mirror the data model in §3 of the analysis so the engine's
//! propagation math is equivalent to what the full `causal_predict.rs` path
//! would do once kernel integration lands (Phase 5 — client data replay).

use crate::dimensions::Dimensions;
use crate::events::DailyRollup;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type NodeId = u32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Brand,
    Region,
    Metro,
    Store,
    Promotion,
    Position,
    Person,
    DailyRollup,
    WeekRollup,
    QuarterRollup,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    BrandOf,
    LocatedIn,
    OperatesIn,
    ClosedDay,
    AggregatesTo,
    Causes,
    Inhibits,
    FillsPosition,
    PositionIn,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    GroundTruth,
    AbTest,
    QuasiExperiment,
    Observational,
    Structural,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub label: String,
    pub node_type: NodeType,
    pub metric: Option<f64>,
    pub brand: Option<String>,
    pub metro: Option<String>,
    pub region: Option<String>,
    pub day_index: Option<u32>,
    pub week_index: Option<u32>,
    pub store_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub edge_type: EdgeType,
    pub weight: f64,
    pub provenance: Provenance,
}

pub struct CaseGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub label_index: HashMap<String, NodeId>,
    pub fwd: HashMap<NodeId, Vec<usize>>,
    pub rev: HashMap<NodeId, Vec<usize>>,
}

impl CaseGraph {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn by_label(&self, label: &str) -> Option<NodeId> {
        self.label_index.get(label).copied()
    }

    pub fn outgoing(&self, id: NodeId) -> &[usize] {
        self.fwd.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn stores_matching(
        &self,
        brand: Option<&str>,
        metro: Option<&str>,
        region: Option<&str>,
    ) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|n| n.node_type == NodeType::Store)
            .filter(|n| brand.is_none_or(|b| n.brand.as_deref() == Some(b)))
            .filter(|n| metro.is_none_or(|m| n.metro.as_deref() == Some(m)))
            .filter(|n| region.is_none_or(|r| n.region.as_deref() == Some(r)))
            .map(|n| n.id)
            .collect()
    }

    pub fn dailies_for_store(&self, store_id: NodeId) -> Vec<NodeId> {
        self.outgoing(store_id)
            .iter()
            .map(|&i| &self.edges[i])
            .filter(|e| e.edge_type == EdgeType::ClosedDay)
            .map(|e| e.target)
            .collect()
    }
}

pub fn build(dims: &Dimensions, events: &[DailyRollup]) -> CaseGraph {
    let mut b = Builder::default();

    // --- Structural nodes: Brand / Region / Metro -----------------------------
    for brand in dims
        .stores
        .iter()
        .map(|s| &s.brand)
        .collect::<std::collections::BTreeSet<_>>()
    {
        b.push_node(Node {
            id: 0,
            label: format!("brand:{}", brand),
            node_type: NodeType::Brand,
            metric: None,
            brand: Some(brand.clone()),
            metro: None,
            region: None,
            day_index: None,
            week_index: None,
            store_ref: None,
        });
    }
    for region in dims
        .stores
        .iter()
        .map(|s| &s.region_code)
        .collect::<std::collections::BTreeSet<_>>()
    {
        b.push_node(Node {
            id: 0,
            label: format!("region:{}", region),
            node_type: NodeType::Region,
            metric: None,
            brand: None,
            metro: None,
            region: Some(region.clone()),
            day_index: None,
            week_index: None,
            store_ref: None,
        });
    }
    for metro in dims
        .stores
        .iter()
        .map(|s| &s.metro_code)
        .collect::<std::collections::BTreeSet<_>>()
    {
        b.push_node(Node {
            id: 0,
            label: format!("metro:{}", metro),
            node_type: NodeType::Metro,
            metric: None,
            brand: None,
            metro: Some(metro.clone()),
            region: None,
            day_index: None,
            week_index: None,
            store_ref: None,
        });
    }

    // --- Store nodes ----------------------------------------------------------
    for store in &dims.stores {
        let store_id = b.push_node(Node {
            id: 0,
            label: store.label.clone(),
            node_type: NodeType::Store,
            metric: Some(store.baseline_daily_sales),
            brand: Some(store.brand.clone()),
            metro: Some(store.metro_code.clone()),
            region: Some(store.region_code.clone()),
            day_index: None,
            week_index: None,
            store_ref: Some(store.label.clone()),
        });
        let brand_id = b.label_index[&format!("brand:{}", store.brand)];
        let region_id = b.label_index[&format!("region:{}", store.region_code)];
        let metro_id = b.label_index[&format!("metro:{}", store.metro_code)];
        b.push_edge(Edge {
            source: store_id,
            target: brand_id,
            edge_type: EdgeType::BrandOf,
            weight: 1.0,
            provenance: Provenance::Structural,
        });
        b.push_edge(Edge {
            source: store_id,
            target: region_id,
            edge_type: EdgeType::OperatesIn,
            weight: 1.0,
            provenance: Provenance::Structural,
        });
        b.push_edge(Edge {
            source: store_id,
            target: metro_id,
            edge_type: EdgeType::LocatedIn,
            weight: 1.0,
            provenance: Provenance::Structural,
        });
    }

    // --- Promotion nodes ------------------------------------------------------
    for promo in &dims.promotions {
        b.push_node(Node {
            id: 0,
            label: promo.label.clone(),
            node_type: NodeType::Promotion,
            metric: Some(promo.true_lift),
            brand: Some(promo.brand.clone()),
            metro: None,
            region: None,
            day_index: None,
            week_index: None,
            store_ref: None,
        });
    }

    // --- Daily rollup nodes ---------------------------------------------------
    for ev in events {
        let store_id = b.label_index[&ev.store_ref];
        let store = b.nodes[store_id as usize].clone();
        let week_index = ev.day_index / 7;
        let daily_id = b.push_node(Node {
            id: 0,
            label: ev.label.clone(),
            node_type: NodeType::DailyRollup,
            metric: Some(ev.revenue),
            brand: store.brand.clone(),
            metro: store.metro.clone(),
            region: store.region.clone(),
            day_index: Some(ev.day_index),
            week_index: Some(week_index),
            store_ref: Some(ev.store_ref.clone()),
        });
        b.push_edge(Edge {
            source: store_id,
            target: daily_id,
            edge_type: EdgeType::ClosedDay,
            weight: 1.0,
            provenance: Provenance::Structural,
        });

        // Attach Causes edge from each active promotion whose brand and day window match.
        for promo in &dims.promotions {
            if promo.brand != store.brand.clone().unwrap_or_default() {
                continue;
            }
            if ev.day_index < promo.start_day || ev.day_index >= promo.end_day {
                continue;
            }
            let Some(&promo_id) = b.label_index.get(&promo.label) else {
                continue;
            };
            b.push_edge(Edge {
                source: promo_id,
                target: daily_id,
                edge_type: EdgeType::Causes,
                weight: promo.true_lift,
                provenance: Provenance::GroundTruth,
            });
        }
    }

    // --- Build adjacency ------------------------------------------------------
    let mut fwd: HashMap<NodeId, Vec<usize>> = HashMap::new();
    let mut rev: HashMap<NodeId, Vec<usize>> = HashMap::new();
    for (i, e) in b.edges.iter().enumerate() {
        fwd.entry(e.source).or_default().push(i);
        rev.entry(e.target).or_default().push(i);
    }

    CaseGraph {
        nodes: b.nodes,
        edges: b.edges,
        label_index: b.label_index,
        fwd,
        rev,
    }
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    label_index: HashMap<String, NodeId>,
}

impl Builder {
    fn push_node(&mut self, mut n: Node) -> NodeId {
        let id = self.nodes.len() as NodeId;
        n.id = id;
        self.label_index.insert(n.label.clone(), id);
        self.nodes.push(n);
        id
    }
    fn push_edge(&mut self, e: Edge) {
        self.edges.push(e);
    }
}
