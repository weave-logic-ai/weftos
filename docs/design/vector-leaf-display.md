# Vector-First Leaf Display Architecture

Status: **Proposed**, supersedes the raster `Compositor` / `LeafSurface` pair
in `weftos-leaf-display` and the raster `LeafPush` payloads in
`weftos-leaf-types`.

Author: System Architecture Designer
Revision: 2 (extension points promoted to first-class)
Date: 2026-05-15

---

## 1. Executive Summary

The current leaf-display pipeline is a **raster compositor**: each
`LeafPush` op is stored in a per-layer op-list, and every push event
re-rasterizes the full 800×480 framebuffer from scratch. With
`lgfx-bus-rgb-rs` 0.2.1's synchronous double-buffer swap this is correct
but visibly slow and tears during multi-push bursts, because cost is paid
in **whole frames** even when one row of text changed.

This document specifies a clean-slate replacement: a **retained-mode scene
graph + dirty-rect renderer**, built on the principle that drives LVGL,
Slint, and every modern compositor on bandwidth-constrained hardware. The
key shape change from revision 1 is that **seven features the v1
implementation defers are nonetheless first-class in the wire format and
trait surface**:

| Feature | Wire format | Scene graph | Renderer | v1 status |
|---|---|---|---|---|
| Touch input | `LeafInput` mesh push topic | `InputRegion` per node | `Input` callback on `SceneStore` | Raw events + AABB hit-test |
| Browser backend | Same CBOR-over-WS | Types are `Send + Sync` | `CanvasSurface` trait impl | Stub: Raw+Text only |
| Animation | `SceneOp::Tween` | Tween table in `SceneStore` | `tick(now)` on renderer | Snap-to-target |
| Sub-pixel / AA text | `Q24.8` position, `FontFace::Vector` | `Text { face, size, weight, kerning }` | `CapabilityMask::SUBPIXEL` | Mono fonts, round to int |
| Bitmap compression | `BitmapFormat` enum | `Primitive::Bitmap { format }` | `decode_bitmap` hook | Raw only; QOI/PNG return `Unsupported` |
| Per-node alpha + blend | `opacity: u8`, `BlendMode` | `Style::opacity`, `Layer::blend` | `CapabilityMask::ALPHA` | Honor 0 (hidden) / 255 (opaque) |
| Multi-display per leaf | `DisplayId` byte in NodeId | `Scene` per display | Renderer-per-display | `DisplayId(0)` implied |

Designing these slots in **now** prevents two breaking wire-format
revisions later. Each is below.

The bus crate (`lgfx-bus-rgb-rs`) remains untouched. The Noise handshake,
mesh topics, and CBOR envelope are untouched. Only payload types and the
crates between mesh and bus change.

## 2. The Three-Line Problem Statement

- **Broken**: full-frame raster re-composite on every push event causes
  whole-frame redraws for one-character changes; tearing in multi-push
  bursts.
- **Right answer**: retained-mode scene graph + dirty-rect renderer with
  extension slots for input, animation, compression, alpha, sub-pixel,
  multi-display, and a browser backend.
- **Path**: replace wire payload + leaf-side state + renderer; keep the
  bus, the mesh transport, and the host CLI's surface.

## 3. Architecture Diagram

```
                            HOST                                LEAF
 ┌─────────────────────────────────────────┐    ┌──────────────────────────────┐
 │  weaver leaf push  ─┐                   │    │  Mesh client (bidirectional) │
 │  kernel.ps          ├─► SceneProducer ─►│    │     │                        │
 │  service renderers  │   (NodeIds w/      │ ─► │     ▼                        │
 │                     │    DisplayId byte) │    │  SceneStore[DisplayId]       │
 │                     ┘   emits SceneOps   │    │     │                        │
 │                                          │    │     │ apply(op) → DamageSet  │
 │  InputConsumer ◄────────────────────────│◄── │     │ + InputEvent           │
 │  (touch → kernel)                       │    │     ▼                        │
 │                                          │    │  SceneRenderer<B>            │
 │                                          │    │     │                        │
 │                                          │    │     ▼                        │
 │                                          │    │  ┌────────────────────────┐  │
 │                                          │    │  │ SceneSurface           │  │
 │                                          │    │  │  ├ DpiSurface          │──┼─► CrowPanel
 │                                          │    │  │  ├ SimSurface          │  │
 │                                          │    │  │  └ CanvasSurface (WASM)│──┼─► browser
 │                                          │    │  └────────────────────────┘  │
 │                                          │    │     │                        │
 │                                          │    │     │ touch driver           │
 │                                          │    │     ▼                        │
 │                                          │    │  GT911 ─► hit-test ─► push   │
 │                                          │    │           topic              │
 └─────────────────────────────────────────┘    └──────────────────────────────┘
                                                            │
                                                            ▼
                                                     lgfx-bus-rgb-rs
                                                       (UNCHANGED)
```

## 4. Wire Format

### 4.1 Decision: hybrid delta + periodic snapshot, bidirectional

The leaf is the source of truth for what is currently displayed. Hosts
send **deltas**; the leaf maintains stable scene state. For self-healing
across mesh reconnects the host periodically (or on `MeshConnected`) sends
a **full snapshot** (`SceneOp::Replace(Scene)`).

The same wire is bidirectional. The leaf publishes **input events** on its
own push topic (`mesh.leaf.<pk>.input`) — symmetric to the display push
topic. This mirrors Wayland's request/event split.

### 4.2 Topic layout

| Topic | Direction | Payload |
|---|---|---|
| `mesh.leaf.<pk>.push` | host → leaf | `LeafPush` (display, audio, brightness) |
| `mesh.leaf.<pk>.input` | leaf → host | `LeafInput` (touch, future: rotary, IMU) |
| `mesh.leaf.<pk>.announce` | leaf → host | `LeafServices` (caps, on connect) |

### 4.3 Display types

```rust
// weftos-leaf-types — schema crate, no_std + alloc + serde

pub type NodeId    = u32;     // [DisplayId: u8 | PathHash: u24]
pub type DisplayId = u8;      // 0..=255 displays per leaf
pub type Seq       = u64;     // monotonic per producer

pub const WIRE_VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
pub struct SceneEnvelope {
    pub version:    u8,         // WIRE_VERSION; leaf rejects on mismatch
    pub display_id: DisplayId,  // which display on this leaf
    pub seq:        Seq,
    pub op:         SceneOp,
}

#[derive(Serialize, Deserialize)]
pub enum SceneOp {
    Replace(Scene),
    Upsert(Node),
    Patch { id: NodeId, diff: PropertyDiff },
    Remove(NodeId),
    Batch(Vec<SceneOp>),

    /// Time-based mutation. v1 implementation snaps to `to`; v1.1
    /// interpolates. The op exists in v1 so the wire format is stable.
    Tween {
        id:          NodeId,
        property:    AnimatableProperty,
        from:        PropertyValue,
        to:          PropertyValue,
        duration_ms: u32,
        curve:       EaseCurve,            // Linear | EaseIn | EaseOut | EaseInOut | Cubic{a,b,c,d}
        start_at:    StartClock,           // Immediate | LeafMs(u64) | OnAck
    },

    /// Cancel an in-flight tween (or all on `None`).
    CancelTween { id: NodeId, property: Option<AnimatableProperty> },

    SetBrightness   { on_us: u32 },
    SetLayerBlend   { layer: Layer, mode: BlendMode },
}

#[derive(Serialize, Deserialize)]
pub struct Scene {
    pub bg:    Rgb,
    pub nodes: Vec<Node>,  // declaration order is intra-layer z-order
}

#[derive(Serialize, Deserialize)]
pub struct Node {
    pub id:        NodeId,
    pub layer:     Layer,
    pub transform: Transform,        // sub-pixel Q24.8
    pub primitive: Primitive,
    pub style:     Style,
    pub input:     Option<InputRegion>,
    pub children:  Vec<NodeId>,
}

#[derive(Serialize, Deserialize)]
pub struct Transform {
    pub x: i32,                       // Q24.8 fixed-point pixels
    pub y: i32,
    pub rotation_deg_q8: i16,         // Q8.8 degrees, v1 ignores non-zero
    pub scale_q16: u32,               // Q16.16, 1.0 = 0x0001_0000; v1 ignores ≠ 1.0
}

#[derive(Serialize, Deserialize)]
pub enum Primitive {
    Text {
        content:     String,
        face:        FontFace,        // see §5.5
        size_q8:     u16,             // Q8.8 px; v1 rounds to int
        weight:      u16,             // 100..=900 (CSS-like); v1 ignores
        kerning:     KerningHint,     // Auto | None | Pairs(Vec<(char,char,i8)>)
    },
    Line   { x2: i32, y2: i32, thickness_q8: u16 },
    Rect   { w: u32, h: u32, radius_q8: u16 },
    Circle { radius_q16: u32 },
    Path   { commands: Vec<PathCmd> },
    Bitmap {
        w: u32, h: u32,
        format: BitmapFormat,         // Raw8888 | Raw565 | Qoi | Png | Rle | WebP
        data:   ByteBuf,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Style {
    pub fill:         Option<Rgba>,   // Rgba carries alpha
    pub stroke:       Option<Rgba>,
    pub stroke_width_q8: u16,
    pub opacity:      u8,             // 0..=255; multiplies fill/stroke alpha
    pub visible:      bool,
}

pub enum BlendMode { Normal, Multiply, Screen, Additive, SrcOver, DstOver }

pub enum Layer { Bg, Widget, Text, Alert }
```

`Rgba` (not `Rgb`) is now the colour type — alpha is in the wire from day
one even if the v1 renderer collapses it.

### 4.4 Input types

```rust
#[derive(Serialize, Deserialize)]
pub struct InputEnvelope {
    pub version:    u8,
    pub display_id: DisplayId,
    pub seq:        Seq,
    pub leaf_ms:    u64,              // leaf-local monotonic ms
    pub event:      LeafInput,
}

#[derive(Serialize, Deserialize)]
pub enum LeafInput {
    /// Raw pointer event in display coordinates (Q24.8).
    Pointer {
        id:    PointerId,             // u8, multi-touch slot
        phase: PointerPhase,           // Down | Move | Up | Cancel
        x:     i32, y: i32,
        pressure_q8: u16,              // 0..=0xFF00; FF00 = full pressure
    },
    /// Recognized gesture. v1 emits only Tap; richer gestures in v1.1.
    Gesture {
        kind:    GestureKind,          // Tap | LongPress | Drag | Flick | Pinch
        node_id: Option<NodeId>,       // hit-test result, if any
        x: i32, y: i32,
        meta: GestureMeta,             // start/end coords, velocity, etc.
    },
    /// Future: rotary encoder, IMU, hardware button.
    Button { id: u8, pressed: bool },
}

#[derive(Serialize, Deserialize)]
pub struct InputRegion {
    pub shape:       HitShape,         // Aabb | Circle | Path
    pub cursor_hint: CursorHint,        // None | Pointer | Text | Grab
    pub capture:     bool,              // capture drags after Down
}

pub enum HitShape {
    Aabb { w: u32, h: u32 },           // relative to node transform; v1
    Circle { radius_q16: u32 },         // v1
    Path(Vec<PathCmd>),                 // v1.1
}
```

### 4.5 Example: one-cell `kernel.ps` update

```
SceneEnvelope {
    version: 1, display_id: 0, seq: 2451,
    op: SceneOp::Patch {
        id: 0x00_03_00_02,             // [Display 0 | ps.row[3].cell[2]]
        diff: PropertyDiff::Text("13%".into()),
    }
}
```

CBOR-encodes to ~32 bytes vs. ~70 bytes today, *and* triggers ~600-byte
damage rect instead of a 768 KB framebuffer rewrite.

### 4.6 Example: tap on a button (leaf → host)

```
InputEnvelope {
    version: 1, display_id: 0, seq: 17, leaf_ms: 4_213_991,
    event: LeafInput::Gesture {
        kind: GestureKind::Tap,
        node_id: Some(0x00_01_00_07),  // hit-test resolved this NodeId
        x: 38_400, y: 51_200,           // Q24.8 → (150.0, 200.0)
        meta: GestureMeta::tap(),
    }
}
```

The leaf does the hit-test (it owns the scene state); the host receives a
fully-resolved `NodeId`.

## 5. Scene Graph Model

### 5.1 Types (leaf-side, in `weftos-leaf-scene`)

```rust
pub struct SceneStore {
    displays: BTreeMap<DisplayId, DisplayState>,
}

struct DisplayState {
    nodes:    BTreeMap<NodeId, NodeState>,
    bg:       Rgba,
    seq:      Seq,
    tweens:   Vec<ActiveTween>,         // §5.6
    layer_blend: [BlendMode; 4],
}

struct NodeState {
    node:      Node,
    last_aabb: Option<Rect>,            // for damage on Patch / Remove
}

pub struct DamageSet {
    rects: SmallVec<[Rect; 8]>,
    full_repaint: bool,
}

impl SceneStore {
    pub fn apply(&mut self, env: SceneEnvelope) -> DamageSet;
    pub fn tick(&mut self, now_ms: u64) -> DamageSet;       // animation
    pub fn hit_test(&self, d: DisplayId, x: i32, y: i32) -> Option<NodeId>;
    pub fn walk(&self, d: DisplayId, layer: Layer, f: impl FnMut(&Node));
}
```

### 5.2 NodeId namespacing

`NodeId = u32 = [DisplayId: 8][PathHash: 24]`.

- 8 bits of `DisplayId` → 256 displays per leaf (more than any realistic
  Pi-5-driving-many-heads scenario).
- 24 bits of producer path hash → ~16 M unique paths; collisions are
  vanishingly rare at the expected scale (~10³ nodes per display).

Host helper:

```rust
pub fn node_id(display: DisplayId, producer: &str, path: &[u16]) -> NodeId
```

Producers think in path terms (`"kernel.ps"`, `[3, 2]`); the wire carries
`u32`. Top-level reserved producer prefixes:

| Producer prefix | Owner |
|---|---|
| `"system."` | reserved (kernel-internal) |
| `"kernel.*"` | the kernel daemons (`kernel.ps`, `kernel.log`, …) |
| `"app.*"` | applications |
| `"user.*"` | end-user scripts / `weaver leaf push` |

### 5.3 Z-order

Four-layer z-bucket (`Bg | Widget | Text | Alert`). Within a layer,
declaration order is sibling order. Each layer carries a `BlendMode`
(default `Normal`); `SceneOp::SetLayerBlend` mutates it without touching
nodes.

### 5.4 Coordinate system

**Q24.8 fixed-point pixels internally and on the wire.** `i32` `x` /
`y` / `w` / `h` values are all Q24.8. The v1 renderer rounds to nearest
integer at the rasterization boundary; a v1.1 sub-pixel rasterizer reads
the same bits. **Integer-only is a v1 implementation detail, never a wire
constraint.**

Helpers:

```rust
pub const fn px(n: i32) -> i32 { n << 8 }
pub const fn from_px_q8(q: i32) -> i32 { (q + 128) >> 8 }   // round
```

### 5.5 Text and fonts

`FontFace` is the extension hinge:

```rust
pub enum FontFace {
    /// Built-in bitmap atlases. v1 ships `Mono10x20` + `Mono6x10`.
    Builtin(BuiltinFont),
    /// Reference into a leaf-installed face. v1.1.
    Vector { family: String, style: FontStyle },
    /// Inline font subset pushed over wire. v1.1.
    Inline { id: u32 /* see SceneOp::UploadFont */ },
}
```

v1 honors only `FontFace::Builtin(_)`; the others return
`RenderError::Unsupported { feature: "vector fonts" }`. The host can ask
the leaf's `LeafServices.display_sink.font_caps` which faces are
available.

`size_q8`, `weight`, and `kerning` ride in `Primitive::Text` from day one
so AA + variable fonts in v1.1 do not break wire format.

### 5.6 Tweens (animation slot)

`SceneStore` keeps a `Vec<ActiveTween>` per display:

```rust
struct ActiveTween {
    id:           NodeId,
    property:     AnimatableProperty,
    from:         PropertyValue,
    to:           PropertyValue,
    start_ms:     u64,             // leaf-clock
    duration_ms:  u32,
    curve:        EaseCurve,
}
```

`SceneStore::apply` for `SceneOp::Tween` appends to the table. v1
implementation in `SceneStore::tick(now)`: **for every active tween, snap
the property to `to` and remove the entry**. The behaviour is correct (end
state matches a fully-interpolated tween), only the visual is wrong. v1.1
replaces `tick`'s body with eased interpolation; nothing else changes.

The renderer calls `tick(now)` once per frame before drawing.

`AnimatableProperty` enumerates the lerpable subset of `PropertyDiff`:
`Position`, `Opacity`, `Fill` (RGBA lerp), `Stroke`, `Scale`, `Rotation`,
`TextContent` (snap-only — no lerping a string).

### 5.7 Per-node alpha and layer blend

- `Style::opacity: u8` rides on every node.
- `Layer` carries a `BlendMode` (mutable via `SceneOp::SetLayerBlend`).
- `Rgba` colour everywhere — fills, strokes, bg, tweens.

v1 renderer behaviour: opacity `0` → skip node (no draw, no damage
contribution past clip). Opacity `1..=254` → draw as if `255`.
`BlendMode::Normal` honoured; all other modes degrade to `Normal` with a
one-time `tracing::warn!`. Damage rules still consider the full AABB so
v1.1's correct alpha doesn't change damage behaviour.

### 5.8 Hit-test

`SceneStore::hit_test(display, x, y) → Option<NodeId>` walks the display's
nodes back-to-front (Alert → Text → Widget → Bg), descends into
declaration order, and returns the first node whose `InputRegion`
contains `(x, y)`. `HitShape::Aabb` and `Circle` ship in v1;
`HitShape::Path` is v1.1.

A node without an `InputRegion` is non-interactive (the most common case).

## 6. Renderer Trait — `SceneSurface`

```rust
pub trait SceneSurface {
    type Error: core::fmt::Debug;

    /// What this backend can do. Lets the renderer skip unsupported paths
    /// and lets the host plan around capability gaps.
    fn capabilities(&self) -> CapabilityMask;

    fn display_caps(&self) -> DisplaySinkCap;

    fn begin_frame(&mut self, damage: &DamageSet) -> Result<(), Self::Error>;

    fn push_clip(&mut self, rect: Rect);
    fn pop_clip(&mut self);

    fn fill_rect(&mut self, rect: Rect, color: Rgba);
    fn stroke_line(&mut self, x1q8: i32, y1q8: i32, x2q8: i32, y2q8: i32,
                   color: Rgba, thickness_q8: u16);
    fn draw_glyph(&mut self, xq8: i32, yq8: i32, glyph: &Glyph, color: Rgba);
    fn blit_bitmap(&mut self, rect: Rect, decoded: &DecodedBitmap);

    fn commit(&mut self) -> Result<(), Self::Error>;

    fn set_brightness(&mut self, _on_us: u32) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Asynchronously decode a non-Raw bitmap. v1 backends return
    /// `Err(Unsupported)` for everything but `Raw8888` / `Raw565`.
    fn decode_bitmap(&self, format: BitmapFormat, data: &[u8])
        -> Result<DecodedBitmap, RenderError> {
        match format {
            BitmapFormat::Raw8888 | BitmapFormat::Raw565 =>
                DecodedBitmap::from_raw(format, data),
            _ => Err(RenderError::Unsupported {
                feature: "bitmap codec",
            }),
        }
    }
}

bitflags! {
    pub struct CapabilityMask: u32 {
        const ALPHA           = 1 << 0;   // proper compositing
        const SUBPIXEL        = 1 << 1;   // Q24.8-aware rasterizer
        const ANTIALIASED     = 1 << 2;   // glyph + line AA
        const VECTOR_FONTS    = 1 << 3;
        const BITMAP_QOI      = 1 << 4;
        const BITMAP_PNG      = 1 << 5;
        const BLEND_MODES     = 1 << 6;
        const ANIMATION       = 1 << 7;   // tick honoured (not snap)
        const HIT_TEST_PATH   = 1 << 8;
    }
}
```

| Backend | v1 capabilities |
|---|---|
| `DpiSurface` | (empty) — snap-only, opaque-only, mono-only, Raw-only |
| `SimSurface` | (empty) — same, but trivial to upgrade for tests |
| `CanvasSurface` | `ALPHA | SUBPIXEL | ANTIALIASED | BLEND_MODES` — canvas does these natively for free |

The browser backend leapfrogs the embedded one for features Canvas2D gives
us for free.

### 6.1 Backend-specific behaviour

| Backend | `begin_frame` | `commit` |
|---|---|---|
| `DpiSurface` | Pick offscreen buffer via `bus.framebuffer_addr()`. Front→back copy of non-damaged rects. | `bus.present()` — synchronous swap. |
| `SimSurface` | Lock `Vec<Rgba>` buffer. | Increment frame counter; optional snapshot diff for golden tests. |
| `CanvasSurface` (WASM) | Begin `requestAnimationFrame`; set up clip rects via `ctx.beginPath()` + `ctx.rect()` + `ctx.clip()`. | No-op (canvas presents on next rAF). |

## 7. Damage Computation

```rust
fn apply_op(&mut self, op: &SceneOp) -> DamageSet {
    match op {
        SceneOp::Replace(_)              => DamageSet::full(),
        SceneOp::Upsert(n)               => union(old_aabb_of(n.id), aabb_of(n)),
        SceneOp::Patch { id, diff }      => damage_for_diff(*id, diff),
        SceneOp::Remove(id)              => self.aabb_of_id(*id).into(),
        SceneOp::Batch(ops)              => ops.iter().fold(DamageSet::none(), …),
        SceneOp::Tween { id, .. }        => self.aabb_of_id(*id).into(),
        SceneOp::CancelTween { .. }      => DamageSet::none(),
        SceneOp::SetBrightness { .. }    => DamageSet::none(),
        SceneOp::SetLayerBlend { layer, ..} => self.layer_aabb(*layer).into(),
    }
}
```

### 7.1 Rect merging

Fixed budget of 8 rects (LVGL uses similar). On overflow set
`full_repaint = true`. Mirrors LVGL's `lv_refr_join_area`.

### 7.2 Alpha damage cascading

When a node with `style.opacity < 255` (or a layer with non-`Normal`
blend) is mutated, damage cascades to **everything beneath it within the
damage rect** — those pixels need recomposition. In v1 this is moot
(opacity collapses to 0/255), but the damage logic ships correctly so
v1.1's real alpha doesn't change `apply_op`.

### 7.3 Animation damage

`SceneStore::tick(now)` returns a `DamageSet` covering every active
tween's AABB. v1's tick is a no-op-after-the-first-call (it snaps and
removes entries), so this fires once per tween then quiets. v1.1 fires
every frame an animation is in flight.

## 8. Crate Organization

### What dies

| Crate / file | Action |
|---|---|
| `weftos-leaf-types::{DisplayText, DisplayImage, DisplayClear, LayerEffect, LayerEffectKind}` | **Deleted** |
| `weftos-leaf-types::LeafPush` display variants | **Replaced** by `SceneEnvelope`; audio variants survive |
| `weftos-leaf-display::Compositor` | **Deleted** |
| `weftos-leaf-display::LeafSurface` | **Deleted**, replaced by `weftos-leaf-scene::SceneSurface` |
| `weftos-leaf-display::SimSurface` | **Rewritten** against `SceneSurface` |
| `clawft-edge-pad::drivers::dpi_surface::DpiSurface` | **Rewritten** against `SceneSurface` |
| `clawft-edge-pad::mesh` `LeafPush` decode path | **Rewritten** to dispatch by topic and decode to `SceneEnvelope` or `InputEnvelope` |

### What stays untouched

| Crate | Reason |
|---|---|
| `lgfx-bus-rgb-rs` | Proven substrate. Zero changes. |
| `clawft-edge-pad::mesh` transport, Noise handshake, topic routing | Wire payload is generic in this crate; only decoder map changes. |
| `clawft-weave` mesh router | Topic + envelope shape unchanged. |
| Audio path (`AudioDrop`, audio sink) | Orthogonal. |

### Net-new crates

| Crate | Role | Target |
|---|---|---|
| `weftos-leaf-scene` | `Scene`, `Node`, `Primitive`, `Style`, `SceneStore`, `DamageSet`, `SceneSurface`, hit-test, tween table. | `no_std + alloc`, `Send + Sync` types |
| `weftos-leaf-renderer` | Damage-aware walk; glyph cache; tween tick; bitmap-codec dispatch. | `no_std + alloc` |
| `weftos-leaf-input` (split off `-types`) | Input wire types + `InputRegion`; shared host/leaf. | `no_std + alloc` |
| `weftos-scene-builder` (host) | Producer ergonomics: `SceneBuilder::row(...).id_path("ps", &[3, 2])`. Emits `SceneEnvelope`. | `std` |
| `weftos-leaf-canvas` | `SceneSurface` impl over `web_sys::CanvasRenderingContext2d`. | `wasm32-unknown-unknown` |
| `weftos-leaf-touch-gt911` | Driver: GT911 over I2C → `LeafInput::Pointer`, with hit-test pulled from `SceneStore`. | `no_std`, esp-hal |

### `weftos-leaf-types` layout

```
weftos-leaf-types/
├── audio.rs       (unchanged)
├── scene.rs       (SceneEnvelope, SceneOp, Node, Primitive, Style, …)
├── input.rs       (InputEnvelope, LeafInput, InputRegion, HitShape, …)
├── caps.rs        (DisplaySinkCap with CapabilityMask, font_caps; AudioSinkCap; LeafServices)
├── color.rs       (Rgba, Rgb)
├── coord.rs       (Q24.8 helpers, px, from_px_q8)
└── topics.rs      (push_topic, input_topic, announce_topic)
```

Top-level outer envelope:

```rust
pub enum LeafPush {
    Scene(SceneEnvelope),
    Audio(AudioPush),
    Brightness { on_us: u32 },
}
```

## 9. Migration Plan

Hard-cut, not parallel-run. The spike is 48 hours old with one known
consumer; two compositors would double the leaf memory budget for no gain.

### Phase 0 — branch + freeze (0.5 d)

- Branch `feat/vector-leaf-display`.
- Tag the spike `leaf-display-raster-v0.1` for rollback.
- Open a Plane epic; one work item per phase below.

### Phase A — schema + scene store + input wire (1.5 d)

- `weftos-leaf-scene` skeleton: `Scene`, `Node`, `Primitive`, `Style`,
  `SceneOp`, `SceneEnvelope`, `SceneStore::apply`, `DamageSet`,
  `BlendMode`, `Layer`, `Transform` (Q24.8), `Rgba`.
- `weftos-leaf-input`: `InputEnvelope`, `LeafInput`, `InputRegion`,
  `HitShape`.
- `SceneStore::hit_test` (AABB + Circle).
- `SceneStore::tick` (snap-only).
- `weftos-leaf-types` reshape: drop old display variants, keep audio +
  caps + topics. Add `CapabilityMask` to `DisplaySinkCap`.
- Unit tests: CBOR round-trip every op; apply-op damage correctness;
  hit-test correctness; tween snap behaviour.

### Phase B — renderer + sim backend + glyph cache (2 d)

- `weftos-leaf-renderer`: damage walk, glyph cache (`Mono10x20`,
  `Mono6x10`), bitmap-codec dispatch (Raw only in v1).
- `SceneSurface` trait + `CapabilityMask`.
- Rewrite `SimSurface`. Golden-image tests per primitive — ratchet
  against regression.
- `weftos-scene-builder` host crate with `node_id` helper and
  `DisplayId`-aware path hashing.

### Phase C — DPI backend + touch driver (1.5 d)

- `DpiSurface` rewrite against `SceneSurface`. Keep `BusRgb` ownership +
  RGB565 swap. Front→back rect copy on partial updates.
- `weftos-leaf-touch-gt911`: GT911 I2C driver feeding `LeafInput::Pointer`
  into a leaf-side dispatcher that calls `SceneStore::hit_test` and
  publishes on `mesh.leaf.<pk>.input`.
- Tap gesture detector (down→up within 250 ms within 16 px). Drag /
  long-press / flick / pinch are v1.1.
- Hardware smoke: tap-a-rect → host receives `Tap{node_id=…}`.

### Phase D — browser backend (1.5 d) [parallelizable with C]

- `weftos-leaf-canvas` crate: `SceneSurface` impl over Canvas2D. Declares
  `CapabilityMask::ALPHA | SUBPIXEL | ANTIALIASED | BLEND_MODES`.
- Browser-side mesh client over WebSocket to the existing mesh gateway
  (use the egui-wasm crate's existing WS plumbing).
- Browser scene viewer page in `clawft-gui-egui` (or sibling): shows the
  same `kernel.ps` scene rendered to a canvas.

### Phase E — mesh + producer wiring (1 d)

- `clawft-edge-pad::mesh`: switch payload dispatch from `Compositor::apply`
  to `SceneStore::apply` + `SceneRenderer::render`. Add input publish
  path.
- Replace `kernel.ps` renderer with a `SceneBuilder` producing patches
  (steady state) + snapshots (on connect, every ~5 s).
- Rewrite `weaver leaf push` subcommands: `text`/`clear`/`brightness`
  map to `SceneOp::{Upsert, Remove, SetBrightness}`. Add
  `weaver leaf push scene <toml>` to script test scenes. Add
  `weaver leaf input subscribe` to dump input events to stdout.

### Phase F — verify + cleanup (0.5 d)

- Flash to CrowPanel; validate kernel.ps updates without tearing;
  reconnect recovers via snapshot; brightness still works; tap on a row
  fires an input event.
- Open browser; same scene renders.
- Delete `weftos-leaf-display` crate entirely.
- Update `docs/leaf-push-protocol.md` to point at this design.

**Total**: ~7 days of agent work, six phases. A blocks B; C/D/E can run
in parallel after B; F gates merge.

## 10. Implementation Work Breakdown

| Phase | Deliverable | Size | Depends on |
|---|---|---|---|
| 0 | Branch + freeze | XS | — |
| A | `weftos-leaf-scene` + `weftos-leaf-input` + types reshape | M | 0 |
| B | `weftos-leaf-renderer` + `SimSurface` + golden tests + `weftos-scene-builder` | L | A |
| C | `DpiSurface` + `weftos-leaf-touch-gt911` + tap gesture + hardware verify | L | B; hardware access |
| D | `weftos-leaf-canvas` + browser viewer | M | B |
| E | Mesh dispatch + `kernel.ps` producer + `weaver leaf` CLI | M | B |
| F | Verify + delete dead crates + docs | S | C + D + E |

Each phase is one agent dispatch. C and E need hardware; D is host-only.

## 11. v1 Implementation Status by Extension Point

A single source-of-truth table. The wire format and trait surface for
every row is shipped in v1; the *runtime behaviour* is what defers.

| Extension | Wire | Scene type | Renderer hook | v1 behaviour | v1.1 lands |
|---|---|---|---|---|---|
| **Touch input** | `mesh.leaf.<pk>.input` + `InputEnvelope` + `LeafInput::{Pointer,Gesture,Button}` | `Node.input: Option<InputRegion>`; `HitShape::{Aabb,Circle,Path}` | `SceneStore::hit_test`; touch driver pushes via mesh | `Pointer{Down,Move,Up,Cancel}` + `Gesture::Tap`; AABB + Circle hit-tests | `Drag`, `LongPress`, `Flick`, `Pinch`; `HitShape::Path` |
| **Browser backend** | Unchanged | Types are `Send + Sync` | `CanvasSurface` impl declares `ALPHA|SUBPIXEL|ANTIALIASED|BLEND_MODES` | Renders `Rect`, `Text`, `Line`, `Bitmap(Raw)`; mesh-over-WS to gateway | `Circle`, `Path`, full bitmap codecs |
| **Animation** | `SceneOp::{Tween,CancelTween}`; `EaseCurve`; `StartClock::{Immediate,LeafMs,OnAck}` | `ActiveTween` in `DisplayState`; `AnimatableProperty` | `SceneStore::tick(now)` | Snap-to-`to` immediately; tween record cleared | Eased interpolation per frame; `CapabilityMask::ANIMATION` lights up |
| **Sub-pixel / AA text** | `Q24.8` coords; `Text { face, size_q8, weight, kerning }` | `Transform` carries `i32` Q24.8 | `draw_glyph` takes Q24.8 coords; `CapabilityMask::{SUBPIXEL,ANTIALIASED,VECTOR_FONTS}` | Round to int, mono fonts only via `FontFace::Builtin` | `FontFace::{Vector,Inline}`; AA glyph renderer; kerning |
| **Bitmap compression** | `Primitive::Bitmap { format: BitmapFormat, data }`; `BitmapFormat::{Raw8888,Raw565,Qoi,Png,Rle,WebP}` | `DecodedBitmap` cache keyed on bytes hash | `SceneSurface::decode_bitmap`; `CapabilityMask::{BITMAP_QOI,BITMAP_PNG}` | Raw8888 + Raw565 supported; everything else returns `Unsupported`; renderer skips node | QOI in v1.1, PNG in v1.2, WebP punted |
| **Alpha + blend** | `Rgba` everywhere; `Style.opacity: u8`; `SceneOp::SetLayerBlend`; `BlendMode` | `Layer.blend_mode`; opacity per node | `CapabilityMask::{ALPHA,BLEND_MODES}` | DPI: 0→hidden, else opaque; Canvas: real alpha (free from `globalAlpha`) | DPI honours full alpha via per-pixel blend in damage rect |
| **Multi-display** | `DisplayId` byte in `SceneEnvelope` and high byte of `NodeId` | `SceneStore.displays: BTreeMap<DisplayId, DisplayState>` | One `SceneSurface` per display; per-display `tick` and `apply` | `DisplayId(0)` implicit, single display per leaf | `LeafServices.display_sinks: Vec<…>`; multi-`DpiSurface` leaves (e.g. Pi 5 driving N panels) |

## 12. Open Questions Remaining

These are genuinely deferred for the implementer:

1. **Snapshot cadence**: 5 s wall-clock + on-`MeshConnected`, or
   watermark-driven (every N deltas)? Recommend 5 s + on-connect for v1.
2. **NodeId hash function**: `xxh32` vs `fxhash` vs FNV. Pick at Phase A.
   All fine; matters for cross-language interop later.
3. **Glyph cache eviction**: bounded LRU vs static atlas? v1 = static
   atlas for built-in mono fonts (~19 KB / face). LRU at v1.1 with vector
   fonts.
4. **Tween clock authority**: leaf-local monotonic ms (chosen — simpler)
   or daemon-supplied keyframes (would require time-sync). Picked
   leaf-local; the wire's `StartClock::LeafMs(u64)` and `OnAck` give the
   host enough authority.
5. **Browser mesh transport**: WebSocket-to-mesh-gateway (recommended,
   leverages existing infra) vs. WebTransport (better for the future,
   no infra yet). Phase D picks one; recommend WS for v1, WT for v2.
6. **Tween coalescing**: if a `Tween` arrives mid-flight against the same
   property, cancel the old or chain? Recommend cancel-and-replace; the
   producer chains explicitly with `start_at: StartClock::OnAck`.

---

## Appendix A — Reference Mapping

| This design | LVGL | Slint | piet | Wayland |
|---|---|---|---|---|
| `Scene` / `SceneStore` | `lv_disp_t` + `lv_obj_t` tree | `ComponentHandle` graph | `RenderContext` | `wl_surface` tree |
| `SceneOp::Patch` | `lv_obj_set_*` setters | property bindings | n/a | request stream |
| `SceneOp::Tween` | `lv_anim_t` + `lv_anim_start` | declarative `animate` blocks | n/a | n/a |
| `DamageSet` | `lv_disp_drv_t.refr_area` | internal repaint set | n/a | `wl_surface.damage` |
| `SceneSurface::begin_frame(damage)` | `flush_cb(area, ...)` | platform `RenderingNotifier` | `RenderContext::start_clip` | `wl_surface.commit` |
| `InputEnvelope` | `lv_indev_*` | platform event router | n/a | `wl_pointer` / `wl_touch` events |
| `InputRegion` + `hit_test` | `lv_obj_get_child_at` | `accessibility::*` | n/a | `wl_surface` input region |
| `CapabilityMask` | n/a | feature flags | n/a | `wl_*_interface` versions |
| `BitmapFormat` | n/a (Raw only) | image decoders | image crate | `wl_shm` formats |
| Multi-display | screen objects | `WindowRequestedSize` | n/a | `wl_output` |

Closest cousin is **LVGL** for embedded shape; **Wayland** for the
bidirectional protocol shape (display push topic ≈ requests, input topic
≈ events, snapshot ≈ surface state). Implementers should read LVGL's
`lv_disp_drv_t.flush_cb` and `lv_refr_join_area`, and the relevant
sections of `wayland-protocols/stable/wayland.xml` for the
request/event symmetry, before starting Phases B and C.

## Appendix B — Why `Send + Sync` Throughout?

The browser backend, the desktop sim, and any future host-side preview
all want to share a `SceneStore` between a producer task and a renderer
task. Embedded leaf code is `no_std + alloc + Send + Sync`-clean already
(no `Rc`, no `RefCell` at the boundary). The cost is a few `Arc`s where
we'd otherwise use `Rc`; the benefit is one type system serving every
target. Specifically:

- `Scene`, `Node`, `Primitive`, `Style`, `SceneOp` are POD-like and `Send + Sync`
  for free.
- `SceneStore` is `Send + !Sync` (interior mutability via `&mut`); wrap
  in `Mutex<SceneStore>` for shared access. Standard pattern.
- `SceneSurface` is **not** required `Send + Sync` (backends own
  hardware); the renderer is single-threaded per surface.

## Appendix C — Why Not Drop `LeafPush` Entirely?

`LeafPush` survives as an outer envelope so audio + display + brightness
share one CBOR-encoded payload type on `mesh.leaf.<pk>.push`. Splitting
audio onto a separate topic is tempting but breaks the existing mesh
router's topic-fan-out semantics in `clawft-weave`. Cheaper to keep the
union type with a thinner contents.
