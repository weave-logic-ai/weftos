//! `diff(old, new) -> Vec<SceneOp>` — produce the minimal `SceneOp`
//! sequence that transitions one [`SceneStore`] to another.
//!
//! Strategy (deliberately simple, deliberately correct):
//!
//! 1. **Removed nodes** — nodes present in `old` but not `new` →
//!    `SceneOp::Remove(id)`.
//! 2. **Added nodes** — nodes present in `new` but not `old` →
//!    `SceneOp::Insert(node)`. Walked in `new`'s `node_order` to
//!    preserve declaration-z-order on the leaf.
//! 3. **Updated nodes** — nodes present in both whose body differs →
//!    `SceneOp::Update(new_node)`.
//!
//! Order of emitted ops: removes first, then inserts/updates in `new`'s
//! `node_order`. The leaf applies them in arrival order; this ordering
//! keeps z-index intent legible across reconnects.
//!
//! ## What this is NOT
//!
//! - **Property-level diff** (`PropertyDiff`). The wire format reserves
//!   `SceneOp::Patch { id, diff }` for v1.1; v1 always sends the whole
//!   node on update. At ~hundreds of nodes per display this is fine on
//!   the wire — `Text` nodes are the largest and they're tens of bytes.
//! - **Tween emission**. Producers that animate emit `SceneOp::Tween`
//!   themselves; this differ doesn't infer tweens from frame-to-frame
//!   position deltas.
//! - **Z-order shuffling**. If a producer reorders existing paths, the
//!   diff still emits Update (the leaf preserves its original z-slot).
//!   Reordering requires Remove + Insert on the producer side.

use std::collections::BTreeSet;
use std::vec::Vec;

use weftos_leaf_scene::{DisplayId, NodeId, SceneOp, SceneStore};

/// Compute the ops that, applied to `old`, produce `new` on the given
/// `display`. Both stores must be for the same display id; the caller
/// is responsible for using the same display id their producer uses.
///
/// Returns `Vec::new()` when the stores are equivalent for the given
/// display.
pub fn diff(old: &SceneStore, new: &SceneStore, display: DisplayId) -> Vec<SceneOp> {
    let mut ops = Vec::new();

    let old_ids: BTreeSet<NodeId> = old
        .display(display)
        .map(|d| d.nodes.keys().copied().collect())
        .unwrap_or_default();
    let new_ids: BTreeSet<NodeId> = new
        .display(display)
        .map(|d| d.nodes.keys().copied().collect())
        .unwrap_or_default();

    // 1. Removes — `old - new`.
    for id in old_ids.difference(&new_ids) {
        ops.push(SceneOp::Remove(*id));
    }

    // 2/3. Walk `new`'s node_order so the emitted Inserts/Updates land
    // in the producer's declaration order — preserving intra-layer
    // z-stack intent.
    let Some(new_display) = new.display(display) else {
        return ops;
    };

    for id in &new_display.node_order {
        let Some(new_node) = new_display.nodes.get(id) else {
            continue;
        };
        if old_ids.contains(id) {
            // Possibly an update.
            let old_node = old.display(display).and_then(|d| d.nodes.get(id));
            if old_node != Some(new_node) {
                ops.push(SceneOp::Update(new_node.clone()));
            }
        } else {
            ops.push(SceneOp::Insert(new_node.clone()));
        }
    }

    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{rect_node, text_node, SceneBuilder};
    use weftos_leaf_scene::{Layer, Rgba};

    #[test]
    fn empty_to_empty_is_no_op() {
        let a = SceneBuilder::new("p", 0).build();
        let b = SceneBuilder::new("p", 0).build();
        let ops = diff(&a, &b, 0);
        assert!(ops.is_empty());
    }

    #[test]
    fn added_node_yields_insert() {
        let a = SceneBuilder::new("p", 0).build();
        let mut bb = SceneBuilder::new("p", 0);
        bb.insert("x", text_node(Layer::Text, "hi".into(), 0, 0, Rgba::WHITE));
        let b = bb.build();
        let ops = diff(&a, &b, 0);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], SceneOp::Insert(_)));
    }

    #[test]
    fn removed_node_yields_remove() {
        let mut aa = SceneBuilder::new("p", 0);
        aa.insert("x", text_node(Layer::Text, "hi".into(), 0, 0, Rgba::WHITE));
        let a = aa.build();
        let b = SceneBuilder::new("p", 0).build();
        let ops = diff(&a, &b, 0);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], SceneOp::Remove(_)));
    }

    #[test]
    fn changed_node_yields_update() {
        let mut aa = SceneBuilder::new("p", 0);
        aa.insert("x", text_node(Layer::Text, "old".into(), 0, 0, Rgba::WHITE));
        let a = aa.build();
        let mut bb = SceneBuilder::new("p", 0);
        bb.insert("x", text_node(Layer::Text, "new".into(), 0, 0, Rgba::WHITE));
        let b = bb.build();
        let ops = diff(&a, &b, 0);
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], SceneOp::Update(_)));
    }

    #[test]
    fn identical_stores_yield_nothing() {
        let mut aa = SceneBuilder::new("p", 0);
        aa.insert("x", text_node(Layer::Text, "hi".into(), 0, 0, Rgba::WHITE));
        aa.insert("y", rect_node(Layer::Widget, 0, 0, 10, 10, Rgba::RED));
        let a = aa.build();
        let b = aa.build();
        let ops = diff(&a, &b, 0);
        assert!(
            ops.is_empty(),
            "no-op diff on identical builders, got {:?}",
            ops
        );
    }

    #[test]
    fn mixed_add_remove_update() {
        let mut aa = SceneBuilder::new("p", 0);
        aa.insert(
            "keep",
            text_node(Layer::Text, "k".into(), 0, 0, Rgba::WHITE),
        );
        aa.insert(
            "drop",
            text_node(Layer::Text, "d".into(), 0, 0, Rgba::WHITE),
        );
        aa.insert(
            "change",
            text_node(Layer::Text, "c".into(), 0, 0, Rgba::WHITE),
        );
        let a = aa.build();

        let mut bb = SceneBuilder::new("p", 0);
        bb.insert(
            "keep",
            text_node(Layer::Text, "k".into(), 0, 0, Rgba::WHITE),
        );
        bb.insert(
            "change",
            text_node(Layer::Text, "c!".into(), 0, 0, Rgba::WHITE),
        );
        bb.insert("add", text_node(Layer::Text, "a".into(), 0, 0, Rgba::WHITE));
        let b = bb.build();

        let ops = diff(&a, &b, 0);
        // Expect: 1 remove + 1 update + 1 insert = 3 ops.
        assert_eq!(ops.len(), 3, "ops = {:?}", ops);

        let removes = ops
            .iter()
            .filter(|o| matches!(o, SceneOp::Remove(_)))
            .count();
        let inserts = ops
            .iter()
            .filter(|o| matches!(o, SceneOp::Insert(_)))
            .count();
        let updates = ops
            .iter()
            .filter(|o| matches!(o, SceneOp::Update(_)))
            .count();
        assert_eq!(removes, 1);
        assert_eq!(inserts, 1);
        assert_eq!(updates, 1);
    }

    #[test]
    fn apply_diff_to_old_yields_new() {
        let mut aa = SceneBuilder::new("p", 0);
        aa.insert("a", text_node(Layer::Text, "a".into(), 0, 0, Rgba::WHITE));
        aa.insert("b", text_node(Layer::Text, "b".into(), 0, 0, Rgba::WHITE));
        let mut a = aa.build();

        let mut bb = SceneBuilder::new("p", 0);
        bb.insert("a", text_node(Layer::Text, "A!".into(), 5, 5, Rgba::RED));
        bb.insert("c", text_node(Layer::Text, "c".into(), 0, 0, Rgba::WHITE));
        let b = bb.build();

        let ops = diff(&a, &b, 0);
        for op in &ops {
            a.apply_op(0, op);
        }

        // After applying the diff, a's snapshot should equal b's at the
        // node-set level. Order may differ (z-order preservation) but
        // node identity + content should match.
        let a_snap = a.to_snapshot(0);
        let b_snap = b.to_snapshot(0);
        let a_ids: BTreeSet<_> = a_snap.nodes.iter().map(|n| n.id).collect();
        let b_ids: BTreeSet<_> = b_snap.nodes.iter().map(|n| n.id).collect();
        assert_eq!(a_ids, b_ids, "node id sets must match after applying diff");
        for n in &b_snap.nodes {
            let a_node = a_snap.nodes.iter().find(|x| x.id == n.id).unwrap();
            assert_eq!(a_node, n, "node {} must match after diff apply", n.id.raw());
        }
    }
}
