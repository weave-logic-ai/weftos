# weftos-leaf-sim

Desktop simulator for the WeftOS vector-first leaf display.

Implements `weftos-leaf-renderer::SceneSurface` against an
`embedded-graphics-simulator::SimulatorDisplay<Rgb888>`. Lets developers
exercise the full scene-to-pixels pipeline (Phase A store + Phase B
renderer + this sim) without any hardware.

**Canonical design document:**
[`docs/design/vector-leaf-display.md`](../../docs/design/vector-leaf-display.md)
§6.1 ("Backend-specific behaviour" — `SimSurface` row).

## Capability profile

The sim declares:

```
ALPHA | SUBPIXEL | ANTIALIASED | BLEND_MODES
```

It "cheats" per the design doc — devs see rich rendering even before
the hardware backends catch up. The actual v1 DPI backend (Phase C)
ships `CapabilityMask::empty()`; the canvas backend (Phase D) ships the
same flags as the sim plus `BITMAP_PNG`.

## Quick start

Open a window with a representative scene:

```bash
cd crates/weftos-leaf-sim
cargo run --release --features window --example boot
```

Run the tween demo (snaps to `to` in v1; v1.1 will animate the same
envelope smoothly):

```bash
cargo run --release --features window --example tween_demo
```

Headless smoke (CI / no SDL2 display server):

```bash
WEFTOS_SIM_HEADLESS=1 cargo run --release --features window --example boot
```

## API tour

```rust
use weftos_leaf_renderer::render_damage;
use weftos_leaf_scene::{DamageSet, Rect, SceneStore};
use weftos_leaf_sim::SimSurface;

let mut store = SceneStore::new();
store.set_viewport(0, Rect::from_px(0, 0, 800, 480));
// ... apply some scene ops ...

let mut surface = SimSurface::new(800, 480, "WeftOS leaf sim");
render_damage(&store, 0, &DamageSet::full(), &mut surface).unwrap();

// Pop a window (blocks until user closes):
surface.show();

// Or sample pixels directly (golden tests, snapshots):
let p = surface.display().get_pixel(embedded_graphics::geometry::Point::new(50, 50));
```

## Constraints

- `std`-allowed (it's a dev tool, not embedded).
- `window` feature gated on SDL2. Without it, the headless render path
  still works (golden-test friendly); examples that need a window
  require `--features window`.

## Build & test

```bash
cd crates/weftos-leaf-sim
cargo build --release
cargo test --release             # headless tests pass without SDL2

# Run examples (requires SDL2 + a display server):
cargo run --release --features window --example boot
cargo run --release --features window --example tween_demo
```

The crate has its own `[workspace]` table to stay isolated from the
parent clawft workspace's target-specific toolchains.
