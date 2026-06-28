//! Wrap a [`SceneStore`] into a single-op [`SceneEnvelope`].
//!
//! Used by producers on first run, on mesh reconnect, and every ~5 s as
//! self-healing cadence. See `docs/design/vector-leaf-display.md` §4.1
//! "hybrid delta + periodic snapshot".

use std::vec;

use weftos_leaf_scene::{DisplayId, SceneEnvelope, SceneOp, SceneStore};

/// Wrap `store`'s state for `display` into a [`SceneEnvelope`] whose
/// single op is `SceneOp::Replace(Scene)`. Applying this envelope on
/// the leaf is the canonical "I've been disconnected, here's a fresh
/// authoritative view".
///
/// Returns an envelope with an empty `Replace(Scene::empty(display))`
/// when the store has no state for `display` — the leaf will clear
/// its display state on apply, which is the desired behaviour.
pub fn to_envelope(store: &SceneStore, display: DisplayId) -> SceneEnvelope {
    let scene = store.to_snapshot(display);
    SceneEnvelope::new(display, vec![SceneOp::Replace(scene)])
}

/// Wrap a `Vec<SceneOp>` (typically from [`super::diff`]) into a
/// `SceneEnvelope`. Producers usually call this verbatim:
///
/// ```ignore
/// let ops = diff(&prev, &next, 0);
/// let env = ops_envelope(0, ops);
/// let cbor = weftos_leaf_scene::codec::encode(&env).unwrap();
/// // publish cbor via mesh
/// ```
pub fn ops_envelope(display: DisplayId, ops: std::vec::Vec<SceneOp>) -> SceneEnvelope {
    SceneEnvelope::new(display, ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{text_node, SceneBuilder};
    use weftos_leaf_scene::{codec, Layer, Rgba};

    #[test]
    fn envelope_carries_replace_scene() {
        let mut b = SceneBuilder::new("p", 0);
        b.viewport(800, 480).bg(Rgba::opaque(0x10, 0x10, 0x18));
        b.insert("x", text_node(Layer::Text, "hi".into(), 0, 0, Rgba::WHITE));
        let store = b.build();
        let env = to_envelope(&store, 0);
        assert_eq!(env.display_id, 0);
        assert_eq!(env.ops.len(), 1);
        match &env.ops[0] {
            SceneOp::Replace(scene) => {
                assert_eq!(scene.display_id, 0);
                assert_eq!(scene.nodes.len(), 1);
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn envelope_roundtrips_through_codec() {
        let mut b = SceneBuilder::new("p", 0);
        b.viewport(800, 480);
        b.insert("x", text_node(Layer::Text, "hi".into(), 0, 0, Rgba::WHITE));
        let store = b.build();
        let env = to_envelope(&store, 0);
        let bytes = codec::encode(&env).expect("encode");
        let back = codec::decode_scene_envelope(&bytes).expect("decode");
        assert_eq!(back, env);
    }

    #[test]
    fn ops_envelope_packs_ops() {
        let env = ops_envelope(
            0,
            vec![
                SceneOp::Clear,
                SceneOp::Remove(weftos_leaf_scene::NodeId::from_raw(0)),
            ],
        );
        assert_eq!(env.ops.len(), 2);
    }

    #[test]
    fn empty_store_envelopes_to_empty_scene() {
        let store = SceneStore::new();
        let env = to_envelope(&store, 0);
        match &env.ops[0] {
            SceneOp::Replace(scene) => assert!(scene.nodes.is_empty()),
            _ => panic!(),
        }
    }
}
