# weftos-scene-builder

Host-side ergonomics for the WeftOS vector-first leaf display (Phase E).

Wraps `weftos-leaf-scene`'s POD wire types in:

- **`SceneBuilder`** — fluent producer API. Insert nodes by string path
  (e.g. `"ps.row[0]"`); the builder maps paths to deterministic
  `NodeId`s via `path_to_id`.
- **`diff(old, new)`** — minimal `SceneOp` sequence transitioning one
  `SceneStore` to another. Used by producers in steady state.
- **`to_envelope(store, display)`** — wrap a store into a
  `SceneEnvelope { ops: [Replace(Scene)] }`. Used on first run, mesh
  reconnect, and every ~5 s as self-healing cadence.

Typical producer shape:

```rust,ignore
use weftos_scene_builder::{SceneBuilder, diff, to_envelope, Layer, Rgba};
use weftos_leaf_scene::codec;

// Build a frame.
let mut b = SceneBuilder::new("kernel.ps", 0);
b.viewport(800, 480).bg(Rgba::opaque(0x10, 0x10, 0x18));
b.insert("header", b.text(Layer::Text, "PID  AGENT  STATE", 4, 12, Rgba::WHITE));
for (i, row) in process_rows.iter().enumerate() {
    let path = format!("row[{i}]");
    b.insert(&path, b.text(Layer::Text, row.to_string(), 4, 28 + (i as i32) * 16, row.color));
}
let store = b.build();

// Emit either a snapshot (first run / reconnect) or a delta.
let envelope = if prev_store.is_none() {
    to_envelope(&store, 0)
} else {
    let ops = diff(prev_store.as_ref().unwrap(), &store, 0);
    weftos_scene_builder::snapshot::ops_envelope(0, ops)
};

let cbor = codec::encode(&envelope).unwrap();
// publish cbor on mesh.leaf.<pk>.push
```

`SceneBuilder` is std-only; it's deliberately a host-side tool. The
leaf-side `SceneStore` (in `weftos-leaf-scene`) is `no_std + alloc`.
