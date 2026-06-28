# weftos-leaf-renderer

Phase B of the WeftOS vector-first leaf display.

This crate consumes a `SceneStore` (from Phase A's `weftos-leaf-scene`)
plus a `DamageSet`, walks the scene graph in draw order, AABB-filters
against the damage rects, and dispatches one `draw_primitive` call per
visible node to a backend-supplied `SceneSurface`.

**Canonical design document:**
[`docs/design/vector-leaf-display.md`](../../docs/design/vector-leaf-display.md)
(§6 Renderer Trait, §7 Damage Computation).

## Scope

This crate ships:

- `SceneSurface` trait — every backend implements this.
- `CapabilityMask` bitflags — what each backend can natively render.
- `GlyphCache` — bounded LRU keyed on `(FontFace, char, size_q8)`.
- `decode_bitmap` — `Raw8888` + `Raw565` happy path; everything else
  returns `BitmapError::Unsupported` for v1.
- `render_damage` — the canonical entry point.
- `color::{to_rgb888, to_rgb565, to_rgb565_be}` — colour conversion
  shared by every backend, including the byte-swapped RGB565 the
  CrowPanel DIS08070H pin map needs in Phase C.

This crate does **not** ship:

- Any concrete `SceneSurface` implementation — those live in
  `weftos-leaf-sim` (this phase), the upcoming DPI backend (Phase C),
  and the canvas backend (Phase D).
- Vector-font rasterization — v1.1, gated on `CapabilityMask::VECTOR_FONTS`.
- QOI / PNG decode — v1.1.

## Constraints

- `#![no_std]` + `extern crate alloc;`. Tests run under `std`.
- All wire-shaped public types are `Send + Sync` (compile-time asserted
  in `lib.rs`). `SceneSurface` is intentionally **not** required to be
  `Send + Sync` — backends own hardware handles.
- `embedded-graphics` is only used for its built-in mono font glyph
  data (Mono6x10 / Mono10x20). The renderer itself does not hand draw
  calls to embedded-graphics — that's a backend choice.

## Public API tour

```rust
use weftos_leaf_renderer::{
    // Trait
    SceneSurface,
    // Entry point
    render_damage, RenderError,
    // Capability flags
    CapabilityMask,
    // Glyph cache
    GlyphCache, GlyphKey, Glyph,
    // Bitmap decode
    decode_bitmap, BitmapError, DecodedBitmap,
    // Color helpers
    to_rgb888, to_rgb565, to_rgb565_be,
};
```

The canonical usage from a backend:

```rust
fn render_one_frame(
    store: &SceneStore,
    damage: &DamageSet,
    display_id: DisplayId,
    surface: &mut MyBackend,
) {
    let stats = render_damage(store, display_id, damage, surface)
        .expect("render");
    log::debug!("drew {} primitives, skipped {} offscreen",
                stats.drawn, stats.skipped_offscreen);
}
```

A minimal `SceneSurface` impl shape:

```rust
impl SceneSurface for MyBackend {
    type Error = MyError;

    fn capabilities(&self) -> CapabilityMask {
        CapabilityMask::empty() // v1 baseline
    }

    fn begin_frame(&mut self, damage: &DamageSet, viewport: Rect)
        -> Result<(), Self::Error> { /* set clip + scissor */ Ok(()) }

    fn draw_primitive(&mut self, prim: &Primitive,
                      style: &Style, transform: &Transform)
        -> Result<(), Self::Error> { /* rasterize */ Ok(()) }

    fn end_frame(&mut self) -> Result<(), Self::Error> {
        /* commit / present */ Ok(())
    }
}
```

## Build & test

```bash
cd crates/weftos-leaf-renderer
cargo build --release
cargo test --release
```

The crate has its own `[workspace]` table to stay isolated from the
parent clawft workspace's target-specific toolchains, mirroring
`weftos-leaf-scene` and `lgfx-bus-rgb-rs`.

## Resolved deferrable shape questions from Phase A

- **Q1 (`SceneStore::display(id)` accessor)** — present in Phase A;
  wired through `render_damage` here. No upstream change needed.
- **Q2 (`Primitive::Patch` per-property diff variant)** — deferred to
  Phase E. The renderer assumes whole-`Node` updates and verifies that
  by routing `draw_primitive` on the resolved node, not on a diff.
- **Q3 (`StartClock` enum vs `Option<u32>`)** — deferred. The renderer
  reads tweens via `SceneStore::tick` (Phase A) and does not care about
  the clock representation.

## What comes next

| Phase | Crate / change |
|-------|----------------|
| B | **THIS** — `weftos-leaf-renderer` + `weftos-leaf-sim` |
| C | `DpiSurface` rewrite + `weftos-leaf-touch-gt911` |
| D | `weftos-leaf-canvas` (WASM) |
| E | `clawft-edge-pad::mesh` dispatch + producer wiring |
| F | Hardware verify + dead crate deletion |
