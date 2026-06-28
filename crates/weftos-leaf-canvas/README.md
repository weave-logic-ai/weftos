# weftos-leaf-canvas

Browser backend for the WeftOS vector-first leaf display. Implements
`weftos_leaf_renderer::SceneSurface` against
`web_sys::CanvasRenderingContext2d` so a browser tab can render the
same `kernel.ps` scene that ships to the embedded CrowPanel leaf.

Phase D of the vector-leaf rollout. See
[`docs/design/vector-leaf-display.md`](../../docs/design/vector-leaf-display.md)
(§4.6 Browser Backend, §6 Renderer Trait, §11 capability matrix) for
the design.

## Capability profile

| Flag           | Native? | Source                                                |
|----------------|---------|-------------------------------------------------------|
| `ALPHA`        | yes     | Canvas2D `globalAlpha` + straight RGBA `ImageData`    |
| `SUBPIXEL`     | yes     | Canvas2D accepts `f64` coordinates                    |
| `ANTIALIASED`  | yes     | Native glyph + path AA                                |
| `BLEND_MODES`  | yes     | `globalCompositeOperation` covers every v1 mode       |
| `BITMAP_PNG`   | declared| `createImageBitmap` wiring lands in v1.1              |

Declared mask: `ALPHA | SUBPIXEL | ANTIALIASED | BLEND_MODES | BITMAP_PNG`.

## What this crate is

A `SceneSurface` impl that turns a populated `SceneStore` into pixels
inside a `<canvas id="…">` element. Construction looks up the canvas
by DOM id; `render_damage` dispatches one `draw_primitive` per visible
node per frame.

## What this crate is **not**

- **The mesh transport.** Phase E lands the browser-side WebSocket
  client that feeds `SceneEnvelope`s into a `SceneStore`. This crate
  is the receiving end of that pipeline — it knows nothing about the
  network.
- **A windowing harness.** The `examples/boot.html` page wires a
  canvas into a static demo scene; it doesn't run a `requestAnimation\
Frame` loop, drive input, or open a window.

## Build

This crate's `.cargo/config.toml` defaults the target to
`wasm32-unknown-unknown`:

```bash
cd crates/weftos-leaf-canvas
cargo build --release
```

The explicit form (e.g., from CI without a per-crate config):

```bash
cargo build --release --target wasm32-unknown-unknown
```

The crate's own workspace stanza keeps it isolated from the parent
clawft workspace so the wasm target doesn't poison the rest of the
build.

## Running the example

`examples/boot.html` is a static HTML page that loads the wasm-pack
output and draws the same scene as `weftos-leaf-sim`'s `boot.rs`
example. Build with `wasm-pack` (not part of `cargo build`), then
serve the directory over any static-file server:

```bash
wasm-pack build --target web --out-dir examples/pkg
cd examples
python3 -m http.server 8000
# open http://localhost:8000/boot.html
```

The `boot.html` page is intentionally minimal — it instantiates a
canvas, calls `CanvasSurface::new("leaf-canvas")`, and lets the
embedded Rust code populate + render a static demo scene. Production
producers feed the surface through `weftos-leaf-renderer::render_damage`
once Phase E's mesh transport is wired up.

## Primitive coverage (v1)

| Primitive   | v1 behaviour                                              |
|-------------|-----------------------------------------------------------|
| `Rect`      | `fill_rect` / `stroke_rect` (or manual rounded path)      |
| `Line`      | `begin_path` + `move_to` + `line_to` + `stroke`           |
| `Circle`    | `arc` + `fill` / `stroke`                                 |
| `Text`      | `set_font` (`monospace`) + `fill_text` at `top` baseline  |
| `Bitmap`    | Raw8888 + Raw565 via `put_image_data`; PNG deferred       |
| `Path`      | **Unsupported** in v1 — `Path2d` translator lands in v1.1 |

Capability flag for path rendering and PNG decode is reserved today;
the dispatch shape is in place so v1.1 doesn't churn the trait surface.
