# weftos-leaf-scene

Phase A foundation for the WeftOS vector-first leaf display.

This crate implements the **retained-mode scene graph**, **wire-format
envelopes** (CBOR), **damage computation**, and **hit-testing** that
every subsequent renderer / surface backend will consume. It is the
canonical replacement for the raster `Compositor` / `LeafPush` types
that lived in `weftos-leaf-display` and `weftos-leaf-types`.

**Canonical design document:**
[`docs/design/vector-leaf-display.md`](../../docs/design/vector-leaf-display.md).
Every public type doc-cites the section it implements.

## Scope

This crate ships:

- Wire types: `SceneEnvelope`, `InputEnvelope`, `SceneOp`, `Scene`,
  `Node`, `Primitive`, `Style`, …
- Geometry: Q24.8 fixed-point `Rect`, `Point`, `Size`, `Transform`.
- Runtime: `SceneStore` (apply, tick, hit_test, to_snapshot).
- Damage: 8-rect budget, edge-merging, 50%-viewport escalation.
- Tween table: coalescing on `(node_id, property)`; v1 snap-to-`to`.
- CBOR codec: encode/decode with `WIRE_VERSION` byte rejection.

This crate does **not** ship:

- Glyph cache or font rasterization → `weftos-leaf-renderer` (Phase B).
- Surface backends (`DpiSurface`, `SimSurface`, `CanvasSurface`).
- Mesh transport, Noise handshake — unchanged elsewhere.
- `LeafServices` / capability announce — stays in `weftos-leaf-types`.

## Constraints

- `#![no_std]` + `extern crate alloc;`. Tests run under `std`.
- All public types are `Send + Sync` (compile-time asserted in
  `lib.rs`). The browser, desktop sim, and embedded leaf all share
  the same types.
- The wire format is CBOR via `ciborium`. Bytes round-trip identically
  on every target.

## Design decisions baked in

| Decision | Value |
|----------|-------|
| Snapshot cadence | producer policy; crate exposes `SceneStore::to_snapshot()` |
| NodeId hash | `rustc-hash` (FxHasher; deterministic across reboots) |
| Glyph cache eviction | LRU (lives in renderer; this crate just stabilizes NodeId hashing) |
| Tween clock | leaf-local; `SceneOp::Tween.start_at: Option<u32>` ms |
| Browser transport | WebSocket-to-mesh-gateway (codec-agnostic from this crate) |
| Tween coalescing | newest tween wins, prior cancelled; new `from` is current interpolated state |

## v1 stub status

Items present on the wire / API but with v1 stub behaviour:

- **Tween interpolation** — `SceneStore::tick` snaps to `to` and drains.
  v1.1 replaces the body with eased interpolation.
- **`HitShape::Path`** — always returns "no hit" in `SceneStore::hit_test`.
  v1.1 rasterizes and tests.
- **`Primitive::Path`** — wire-stable; renderer skips it in v1.
- **Per-node alpha 1..=254** — wire-stable; v1 DPI renderer collapses
  to opaque (Canvas backend honours alpha natively).
- **`BlendMode` non-`Normal`** — wire-stable; v1 renderer warns + falls
  back to `Normal`.

Each is marked with a `// v1.1: …` comment in source where the upgrade lands.

## Public API tour

The top-level re-exports (`use weftos_leaf_scene::*`) surface:

- Identity: `NodeId`, `DisplayId`, `path_to_id`.
- Geometry: `Rect`, `Point`, `Size`, `Transform`, `px`, `from_px_q8`.
- Colour: `Rgba`, `Rgb`.
- Primitives: `Primitive`, `Style`, `Layer`, `BlendMode`, `FontFace`,
  `BuiltinFont`, `KerningHint`, `HitShape`, `CursorHint`, `InputRegion`,
  `EaseCurve`, `BitmapFormat`, `PathCmd`.
- Tween: `AnimatableProperty`, `PropertyValue`, `ActiveTween`,
  `TweenTable`.
- Wire: `SceneOp`, `SceneEnvelope`, `InputEnvelope`, `InputEvent`,
  `WIRE_VERSION`.
- Snapshot: `Scene`.
- Damage: `DamageSet`.
- Runtime: `SceneStore`, `DisplayState`, `Node`.
- Codec: `codec::{encode, decode_scene_envelope, decode_input_envelope,
  CodecError}`.

## Build & test

```bash
cd crates/weftos-leaf-scene
cargo build --release
cargo test --release
```

The crate has its own `[workspace]` table to stay isolated from the
parent clawft workspace's target-specific toolchains, mirroring
`lgfx-bus-rgb-rs`.

## What comes next (Phase B → F)

| Phase | Crate / change |
|-------|----------------|
| B | `weftos-leaf-renderer` — damage walk, glyph cache, `SceneSurface` trait, sim backend |
| C | `DpiSurface` rewrite + `weftos-leaf-touch-gt911` |
| D | `weftos-leaf-canvas` (WASM) |
| E | `clawft-edge-pad::mesh` dispatch + producer wiring |
| F | Hardware verify + dead crate deletion |
