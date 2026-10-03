//! Fold-replay for causal node lifecycle state (ADR-067 P1-graph / WEFT-629).
//!
//! [`set_node_state`](crate::causal::CausalGraph::set_node_state) emits a
//! [`causal.node.state`](crate::chain::EVENT_KIND_CAUSAL_NODE_STATE) chain event
//! on every real transition. Folding those events (optionally plus
//! `causal.node.add` / `causal.node.remove`) reconstructs
//! `node_id → state` at any chain sequence without requiring a full graph
//! snapshot.
//!
//! This is the **chain-event fold** half of P1-graph. ADR-067 also describes a
//! per-commit snapshot-diff path (more faithful for un-instrumented mutations);
//! both can coexist — fold is the small kernel ask named by WEFT-629.

use std::collections::HashMap;

use crate::causal::NodeId;
use crate::chain::{
    EVENT_KIND_CAUSAL_NODE_ADD, EVENT_KIND_CAUSAL_NODE_REMOVE, EVENT_KIND_CAUSAL_NODE_STATE,
    ChainEvent, ChainManager,
};

/// A single parsed `causal.node.state` transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStateTransition {
    /// Chain sequence of the event.
    pub sequence: u64,
    /// Causal node id.
    pub node_id: NodeId,
    /// Previous state tag (`None` if the node had no `metadata.state` yet).
    pub from: Option<String>,
    /// New state tag.
    pub to: String,
}

/// Parse a chain event into a [`NodeStateTransition`] when kind matches.
pub fn parse_node_state_event(ev: &ChainEvent) -> Option<NodeStateTransition> {
    if ev.kind != EVENT_KIND_CAUSAL_NODE_STATE {
        return None;
    }
    let payload = ev.payload.as_ref()?;
    let node_id = payload.get("node_id")?.as_u64()?;
    let to = payload.get("to")?.as_str()?.to_string();
    let from = payload
        .get("from")
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_str().map(|s| s.to_string())
            }
        });
    Some(NodeStateTransition {
        sequence: ev.sequence,
        node_id,
        from,
        to,
    })
}

/// Fold a sequence of chain events into `node_id → current state`.
///
/// Rules:
/// - `causal.node.add` seeds the node with optional `state` from payload, else
///   leaves it unmapped until a state event arrives.
/// - `causal.node.state` sets `to` (last write wins in event order).
/// - `causal.node.remove` drops the node from the map.
///
/// Events of other kinds are ignored. Order is the fold order — pass chain
/// order (ascending sequence).
pub fn fold_node_states<'a, I>(events: I) -> HashMap<NodeId, String>
where
    I: IntoIterator<Item = &'a ChainEvent>,
{
    let mut map: HashMap<NodeId, String> = HashMap::new();
    for ev in events {
        match ev.kind.as_str() {
            k if k == EVENT_KIND_CAUSAL_NODE_ADD => {
                if let Some(payload) = ev.payload.as_ref()
                    && let Some(id) = payload.get("node_id").and_then(|v| v.as_u64())
                    && let Some(state) = payload.get("state").and_then(|v| v.as_str())
                {
                    map.insert(id, state.to_string());
                }
            }
            k if k == EVENT_KIND_CAUSAL_NODE_STATE => {
                if let Some(t) = parse_node_state_event(ev) {
                    map.insert(t.node_id, t.to);
                }
            }
            k if k == EVENT_KIND_CAUSAL_NODE_REMOVE => {
                if let Some(payload) = ev.payload.as_ref()
                    && let Some(id) = payload.get("node_id").and_then(|v| v.as_u64())
                {
                    map.remove(&id);
                }
            }
            _ => {}
        }
    }
    map
}

/// Fold all events on a [`ChainManager`] (via [`ChainManager::tail`]).
pub fn fold_node_states_from_chain(cm: &ChainManager) -> HashMap<NodeId, String> {
    let events = cm.tail(cm.len());
    fold_node_states(&events)
}

/// Fold only events with `sequence <= at_seq` (scrub to chain time T).
pub fn fold_node_states_until(cm: &ChainManager, at_seq: u64) -> HashMap<NodeId, String> {
    let events = cm.tail(cm.len());
    let filtered: Vec<&ChainEvent> = events.iter().filter(|e| e.sequence <= at_seq).collect();
    fold_node_states(filtered)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::causal::CausalGraph;
    use std::sync::Arc;

    fn make_event(seq: u64, kind: &str, payload: serde_json::Value) -> ChainEvent {
        // Build via a real chain so hashes/fields are valid.
        let cm = ChainManager::new(99, 1000);
        // Burn sequences up to `seq` if needed by appending dummies first.
        // Simpler: just append once and patch sequence for pure fold tests.
        let mut ev = cm.append("test", kind, Some(payload));
        // For pure unit tests we only need kind + payload + sequence.
        ev.sequence = seq;
        ev
    }

    #[test]
    fn parse_node_state_event_round_trip_fields() {
        let ev = make_event(
            7,
            EVENT_KIND_CAUSAL_NODE_STATE,
            serde_json::json!({
                "node_id": 42u64,
                "from": "frontier",
                "to": "committed",
            }),
        );
        let t = parse_node_state_event(&ev).expect("parse");
        assert_eq!(t.node_id, 42);
        assert_eq!(t.from.as_deref(), Some("frontier"));
        assert_eq!(t.to, "committed");
    }

    #[test]
    fn fold_applies_last_write_wins() {
        let events = vec![
            make_event(
                1,
                EVENT_KIND_CAUSAL_NODE_STATE,
                serde_json::json!({"node_id": 1u64, "from": null, "to": "speculative"}),
            ),
            make_event(
                2,
                EVENT_KIND_CAUSAL_NODE_STATE,
                serde_json::json!({"node_id": 1u64, "from": "speculative", "to": "frontier"}),
            ),
            make_event(
                3,
                EVENT_KIND_CAUSAL_NODE_STATE,
                serde_json::json!({"node_id": 1u64, "from": "frontier", "to": "committed"}),
            ),
            make_event(
                4,
                EVENT_KIND_CAUSAL_NODE_STATE,
                serde_json::json!({"node_id": 2u64, "from": null, "to": "frontier"}),
            ),
        ];
        let map = fold_node_states(&events);
        assert_eq!(map.get(&1).map(String::as_str), Some("committed"));
        assert_eq!(map.get(&2).map(String::as_str), Some("frontier"));
    }

    #[test]
    fn fold_remove_drops_node() {
        let events = vec![
            make_event(
                1,
                EVENT_KIND_CAUSAL_NODE_STATE,
                serde_json::json!({"node_id": 5u64, "from": null, "to": "committed"}),
            ),
            make_event(
                2,
                EVENT_KIND_CAUSAL_NODE_REMOVE,
                serde_json::json!({"node_id": 5u64, "label": "x"}),
            ),
        ];
        let map = fold_node_states(&events);
        assert!(map.is_empty());
    }

    /// End-to-end: CausalGraph.set_node_state emits chain events; fold reconstructs.
    #[test]
    fn emit_and_fold_round_trip() {
        let cm = Arc::new(ChainManager::new(1, 1000));
        let mut g = CausalGraph::new();
        g.set_chain_manager(Arc::clone(&cm));

        let id = g.add_node("turn-1".into(), serde_json::json!({"conv_id": "c1"}));
        assert!(g.set_node_state(id, "frontier"));
        assert!(g.set_node_state(id, "committed"));
        assert!(g.set_node_state(id, "stale"));
        // No-op: same state must not emit another event.
        let len_before = cm.len();
        assert!(g.set_node_state(id, "stale"));
        assert_eq!(cm.len(), len_before, "identical state must not append");

        let map = fold_node_states_from_chain(&cm);
        assert_eq!(
            map.get(&id).map(String::as_str),
            Some("stale"),
            "fold must recover final lifecycle state"
        );

        // Live graph metadata agrees with the fold.
        let node = g.get_node(id).expect("node");
        assert_eq!(
            node.metadata.get("state").and_then(|v| v.as_str()),
            Some("stale")
        );

        // Scrub to the sequence of the *first* state event → should be "frontier".
        let all = cm.tail(cm.len());
        let first_state_seq = all
            .iter()
            .find(|e| e.kind == EVENT_KIND_CAUSAL_NODE_STATE)
            .map(|e| e.sequence)
            .expect("at least one state event");
        let at_first = fold_node_states_until(&cm, first_state_seq);
        assert_eq!(
            at_first.get(&id).map(String::as_str),
            Some("frontier"),
            "scrub to first state event"
        );
    }

    #[test]
    fn set_node_state_without_chain_manager_still_writes_metadata() {
        let g = CausalGraph::new();
        let id = g.add_node("n".into(), serde_json::json!({}));
        assert!(g.set_node_state(id, "committed"));
        assert_eq!(
            g.get_node(id)
                .unwrap()
                .metadata
                .get("state")
                .and_then(|v| v.as_str()),
            Some("committed")
        );
    }
}
