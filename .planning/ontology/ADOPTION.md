---
title: Ontology adoption — Foundry-shaped, substrate-native, composition-first
created: 2026-04-23
status: architectural direction, approved in chat 2026-04-23; pending ADRs
scope: WeftOS ontology + composition + governance-slot direction
bridge_target: Palantir Foundry (ontology model)
first_concrete_drivers: vector-synth ⊃μBus; home-security composition example
governing_principle: "Data shape defines the interface."
---

# Ontology adoption

## 0. Where this comes from

WeftOS has a substrate layer (path-keyed KV + pub/sub) and a growing family of adapters publishing into it. It does not yet have a typed layer above that — no Object Types, no Link Types, no typed Actions, no composition primitive. The practical consequence is that *every time someone wants to "collect, process, display, and govern" a new kind of data*, they re-invent the scaffolding ad hoc. vector-synth / ⊃μBus hit this wall first — it has `docs/modules/bus/` full of typed module specs in Markdown, but no runtime-level ontology for patch composition, param UIs, or live reconfiguration. The home-security example would hit the same wall with cameras + sensors + alarms + notifiers.

The decision recorded here is to **adopt Palantir Foundry's ontology architecture as the shape of that missing layer**, sitting above the substrate, with a composition story on top. This document captures the adoption direction, the naming policy, the scoping anchor, the core inference rule, the staircase from current state to the "think it up and it's made" ambition, and the bridge-to-Foundry fidelity we owe.

## 1. The problem, one line

There is no ontology describing how WeftOS collects, processes, displays, and governs data, and that absence is visible in every domain that tries to build on it (vector-synth, sensor rigs, hypothetical home-security dashboards).

## 2. The decision

**Adopt the Palantir Foundry ontology architecture.** Keep its vocabulary intact — Object, Object Type, Property, Link Type, Interface, Action, Function, Ontology Manager, Vertex, Machinery — except where a local rename is motivated by WeftOS specifics. Build on top of substrate, not in place of it: substrate stays the permissive core; the ontology is the strict facade above.

Supporting reasons:

1. **Foundry's layering is the right shape** — semantic (types) → kinetic (actions, functions) → governance (manager, permissions) → apps (Explorer, Vertex, Machinery, Workshop). It maps cleanly onto the four verbs we already use informally: **collect, process, display, govern.**
2. **Vocabulary transfer is cheap** — naming the same thing the same way as the largest existing deployment of this model saves us a translation layer and makes cross-team / cross-project vocabulary (WeftOS ↔ vector-synth ↔ any future integrator) coherent.
3. **Bridging to Foundry is a concrete future option** — WeftOS as a gathering layer *beneath* Foundry is a plausible integration story. If our ontology is structurally isomorphic, bridge engineering is translation, not redesign.
4. **The research (see `palantir-foundry-research.md`) found the model partially fits** — it fits cleanly through the semantic + kinetic layers, and diverges at closed-world identity (Foundry) vs open-namespace substrate (WeftOS). We resolve that tension at §5 below.

## 3. Naming policy

**Preserve Foundry vocabulary.** Do not invent a weaving-metaphor parallel for every concept. Object, Object Type, Property, Link Type, Interface, Action Type, Function, Ontology Manager, Vertex, Machinery, Writeback, Object View, Workshop — all stay.

Exceptions are **motivated local renames only**, where the WeftOS-specific meaning deserves a concrete name:

| WeftOS concept | Name | Why |
|---|---|---|
| Top-level root Object (one per mesh network) | **Mesh** | Makes the identity anchor introspectable as a typed thing rather than a conceptual one. Everything else is scoped inside a Mesh. |

No other renames are committed by this document. If a genuinely motivated local rename arises later, it needs its own ADR.

## 4. Layered architecture

```
┌───────────────────────────────────────────────────────┐
│ Apps                                                   │
│   Explorer · Vertex (ui://graph) · Workshop ·          │
│   Object Views · Machinery (pipeline runner)          │
├───────────────────────────────────────────────────────┤
│ Governance (slots, not fills — see §7)                 │
│   Pre-commit hook · edit-visibility axis · audit sink  │
├───────────────────────────────────────────────────────┤
│ Kinetic                                                │
│   Action Types · Functions · Ontology Edits            │
├───────────────────────────────────────────────────────┤
│ Semantic                                               │
│   Object Types · Properties · Link Types · Interfaces  │
├───────────────────────────────────────────────────────┤
│ Substrate (permissive core)                            │
│   path-keyed KV + pub/sub; no schema enforcement       │
└───────────────────────────────────────────────────────┘
```

Substrate never changes as a result of this adoption. It is the ground truth. The ontology layers above it are typed projections, validation facades, and governed write paths.

## 5. Scoping anchor: one mesh = one Object

**A single WeftOS mesh network IS one Object instance — a Mesh.** Every other Object lives inside that Mesh's namespace. Identity is path-scoped to the Mesh; from Foundry's external point of view, a Mesh has a single Object ID.

This resolves the research-flagged tension between Foundry's closed object-graph and substrate's open path-keyed namespace:

- **Within** a Mesh: paths are open, anyone can publish to any substrate path, no central registry gates writes. Permissive core preserved.
- **Between** Meshes / **to Foundry**: each Mesh presents as a single Object with typed Properties (top-level substrate sections) and sub-Objects (nested typed paths). Closed-world enough for Foundry to consume via bridge.

Cross-mesh federation is deferred; when it lands, it is modeled as Link Types between Mesh Objects, not as a flattened identity space.

## 6. Core inference rule: data shape defines the interface

This is the principle that makes the whole stack cheap:

> The shape of a value at a substrate path determines which Object Type it instantiates, which Viewers render it, which Actions apply to it, which Functions accept it, and which Workshop slots can host it.

No schema declaration is required for data to be usefully rendered; shape-matching handles the default case. Schema declarations (Object Types) **promote** the default into a typed, governed, composable entity — they don't gate it.

This principle is already in flight at the UI layer: Phase 1 Explorer's viewer registry uses `fn matches(value: &Value) -> u32` to pick renderers by shape. The same pattern lifts up:

- **Shape → Object Type inference** (same `matches/priority` cascade, one level up)
- **Shape → default Object View** (viewer registry; JsonFallback as priority-1 catch-all)
- **Shape → Explorer facets** (auto from declared Properties when the Object Type is known)
- **Shape → bridge mapping** (structural Foundry-Object-Type correspondence where available)

Every Object Type must carry enough **capability metadata** — *what renders me, what Actions accept me as input, what events I emit, what Functions transform me* — that a composer (human drag-drop, LLM, script) can reason about composition without bespoke knowledge of each type. This is what separates a typed ontology from a typed schema: types carry their own affordances.

## 7. Governance as slot, not fill

Governance is the layer we explicitly do not build first. But every kinetic primitive carries the **shape** of its governance from day one:

- Every Action Type exposes a **pre-commit hook point** for validation + permission checks. Initial implementation: `allow_all()`. Real implementation: policy engine plugs in here without any kinetic refactor.
- Every write carries an **edit-visibility tag** slot (the decoupled permission axis the Foundry research flagged). Empty today; honored by consumers tomorrow.
- The **Ontology Edit** primitive exists and carries each write through a **single atomic commit path** from day one — `{ edits: [...], actor, motive, timestamp }`. "Transaction" is a single publish initially; expands to N-publish atomic groups later without changing the shape.
- An **audit-log sink** is a named output channel from the edit path. Today's sink is `/dev/null`. Tomorrow's sink is the audit store.

Slots everywhere, fills nowhere. The architecture carries governance in its bones; the governance *code* is later.

## 8. The composition & surfacing staircase

Each step is independently useful. Each step is foundation for the next. The "think it up and it's made" ambition sits at the top; nothing below it is useless without it.

```
Step 1  Phase 1 Explorer — see raw substrate
Step 2  Object Types + first concrete types (mic, mesh, chain, ⊃μBus modules)
Step 3  CONFIG-DRIVEN HOT-RELOAD COMPOSITION (unlocks vector-synth iteration)
Step 4  Manual drag-drop composition (Workshop as saved Object)
Step 5  Event wiring (typed Action-linked pub/sub over Object Types)
Step 6  Usage telemetry + surfacer (MRU / recency / recommender; non-AI)
Step 7  LLM composer (natural-language intent → proposed composition)
```

### Step 1 — Phase 1 Explorer (landed)

Tree of substrate paths + shape-sniffed Viewers + JsonFallback catch-all. Delivers "I can see everything WeftOS knows about" with no type system needed.

### Step 2 — Object Types + first concrete types

Promote the most-touched substrate shapes into declared Object Types with Properties and capability metadata. First targets:

- **Mesh** (root Object, one per WeftOS instance)
- **Mic / AudioStream** (already shape-matched by `AudioMeterViewer`; promote to typed)
- **MeshNode / Cluster** (today's `$substrate/cluster/*`)
- **ChainEvent** (today's `$substrate/chain/*`)
- **⊃μBus modules** (NodeKind::AudioIn, oscillators, filters, etc. — ~200 specs already in `~/dev/vector-synth/docs/modules/bus/`)

This is where the ontology becomes runtime-real.

### Step 3 — Config-driven hot-reload composition *(the unblocker)*

**Today's pain:** changing the GUI — layout, panel composition, what's shown where — requires a rebuild-and-reload cycle. This is intolerable for iteration velocity, and it blocks the vector-synth team's ability to probe UI shape against their module specs.

**The fix:** UI composition state lives **in substrate**, not in compiled Rust. Workshop specs (which Objects get rendered, with which Viewers, in what layout) are values at paths like `substrate/ui/workshop/<name>`. The GUI subscribes to those paths. Any publish causes a **live reconfiguration** — no reload, no rebuild.

Implications:

- The GUI becomes a **shape interpreter** over Workshop-Object values, not a statically-composed surface.
- **Any writer** to substrate is equivalently a UI composer: a TOML-file watcher, a script, an LLM, a drag-drop UI, a script in a remote session, a CI pipeline, a rollback from git history — all push their proposed composition to the Workshop path and the GUI picks it up.
- Vector-synth iteration becomes: edit a TOML that describes the patch's param-editor layout → file-watcher publishes to `substrate/ui/workshop/vs-oscillator-params` → GUI reconfigures live. No rebuild.
- This is the **"pipelines-as-substrate" pattern applied to UI state**: the configuration that drives runtime behavior IS substrate data, editable through the same tooling as everything else.

What Step 3 does NOT need: a drag-drop composition UI (that's Step 4), an LLM (that's Step 7), or any governance fill beyond the slot (that's never on this staircase). It only needs (a) Workshop Object Type with a layout schema, (b) GUI-side subscribe + reconfigure, (c) at least one writer that isn't the GUI itself (a file watcher is enough to start).

This step is the one that matters most for near-term development velocity.

### Step 4 — Manual drag-drop composition

A composition UI inside WeftOS for constructing Workshop Objects graphically. Drop a viewer, select a substrate path, save. The output is a Workshop Object at a substrate path, identical in shape to what Step 3 consumes — so Step 4 is *a writer* into Step 3's runtime, not a replacement for it.

### Step 5 — Event wiring

Typed if-this-then-that over Object state changes + Actions. "When `DoorSensor.state → open` AND time-of-day in after-hours, invoke `Notifier.notify(family_phones)`." The wiring itself is a Workshop-Object-shaped value; the event-wire engine subscribes to state-change substrate paths and dispatches typed Actions. Pre-commit hook slots ensure governance plugs in cleanly later.

### Step 6 — Usage telemetry + surfacer

Record which Objects users touch, which Views they open, which Actions they trigger. Feed a ranker (MRU / frecency / co-access). UI slots for "recommended" / "recently used" / "users also viewed." No AI — classical recommender.

### Step 7 — LLM composer

Natural-language intent → proposed Workshop Object. "Build me a dashboard when anyone's at the door" → LLM reads the ontology (Object Types, capability metadata), selects relevant types (DoorSensor, Camera, Notifier), proposes a Workshop composition, publishes it to `substrate/ui/workshop/door-watch`. User approves. The composition materializes live via Step 3's hot-reload.

The "think it up and it's made" endgame is Step 3 + Step 7 composed.

## 9. Canon UI primitives

These are shape-driven rendering primitives that Workshop Objects compose. Each is registered with a `matches/paint` pair and lives in `clawft-gui-egui`'s canon.

| Primitive | Status | Role |
|---|---|---|
| `ui://heatmap` | exists (commit 613b58a) | 2D grid values → color grid |
| `ui://gauge` | exists | scalar + range → bar / dial |
| `ui://waveform` | exists (commit 613b58a) | sample array → scrolling line |
| `ui://graph` | planned | graph-shaped value → node-graph canvas. **Vertex analog.** Serves Explorer's graph-view toggle AND vector-synth patch UI from the same primitive. Spike pending. |
| `ui://workshop` | planned | composition primitive; itself an Object Type; hosts child Viewers and layout |
| `ui://json-fallback` | landed (Phase 1) | catch-all; always priority 1 |
| AudioMeter / ConnectionBadge / DepthMap | landed (Phase 1) | shape-matched specialized viewers |

`ui://graph` and `ui://workshop` are the two that this document commits to adding beyond Phase 1.

## 10. Bridge-to-Foundry fidelity

For each Foundry primitive, fidelity class is **adopt** (1:1 semantics), **adopt-with-slot** (carry the shape now, governance/permissions empty, fill later), or **adapt** (substrate-open-namespace divergence; identity translation at bridge time).

| Foundry primitive | WeftOS fidelity | Notes |
|---|---|---|
| Object, Object Type | adopt | 1:1 semantics. Shape-inferred default, schema-declared preferred. |
| Property | adopt | 1:1. Carries type, range, default, capability metadata. |
| Link Type | adopt | 1:1. Typed edges between Object Types. |
| Interface | adopt-with-slot | 1:1 shape; polymorphism-over-Actions limitation inherited from Foundry. |
| Action Type | adopt-with-slot | Schema 1:1; pre-commit governance slot empty today. |
| Function | adopt | 1:1. Machinery stages are Functions. |
| Ontology Edit (transaction) | adopt-with-slot | `{ edits, actor, motive, timestamp }` shape from day one; single-publish implementation initially, N-atomic later. |
| Ontology Manager | adopt-with-slot | Shape 1:1; change workflow, migration, audit empty today. |
| Vertex | adopt | `ui://graph` primitive. |
| Machinery | adopt | Pipeline primitive; whisper spike is first probe (`.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md`). |
| Object Views | adopt | Viewer registry pattern (Phase 1 Explorer). |
| Workshop | adopt | Step 3 config-driven + Step 4 drag-drop. |
| Writebacks | adapt | Substrate paths are the writeback surface. Open-namespace means writeback semantics are path-publish, not row-update. |
| Permissions | adopt-with-slot | Object-level + property-level + edit-visibility axes reserved in schema; enforcement empty today. |

Fidelity classes decide what the eventual WeftOS↔Foundry bridge adapter has to do per primitive: `adopt` → trivial translation, `adopt-with-slot` → trivial now + enforcement later, `adapt` → identity/namespace translation layer.

## 11. First concrete drivers

The ontology shape is driven by two concrete domains rather than designed in the abstract:

### 11.1 vector-synth / ⊃μBus

`~/dev/vector-synth/docs/modules/bus/` contains ~200 module specifications as Markdown files with YAML frontmatter. Each is already *structurally* an Object Type declaration:

- Typed slots with ranges + defaults (= Properties)
- Typed input/output ports (= Link Types between modules)
- ADR grounding (= provenance / lineage)
- Taxonomy + tags (= Interface candidates)
- Canonical citation (= semantic grounding)

Adoption formalizes these markdown specs into runtime Object Types. Patch compositions become Vertex graphs over them. Module param editing becomes shape-driven Object Views. Step 3 hot-reload is specifically the unblocker that lets vector-synth iterate on these surfaces without a rebuild cycle.

### 11.2 Home-security composition example (second driver)

Cameras + door/window sensors + alarm controllers + notification services. Pressure-tests the ontology on axes vector-synth doesn't:

- **Source cardinality**: many cameras, fan-in to single dashboard
- **Payload shape**: frame tensors (binary, high-volume) vs ⊃μBus's scalar + CV
- **Composition**: DAG with joined event streams (camera + motion sensor jointly trigger alarm) vs ⊃μBus's mostly-linear patch
- **Actions with external side effects**: notifications leave the mesh (SMS, push, email) — first real test of Action governance slots being load-bearing

Even if we never ship a home-security app, the example is a useful design pressure-test for pipeline primitive axes (see `.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md`).

## 12. Four verbs → layer map

| Verb | Layer | Status in WeftOS today |
|---|---|---|
| Collect | Substrate adapters (mic, tof, mesh, chain, bluetooth, network, etc.) | ✓ have |
| Process | Machinery + Functions | whisper spike pending |
| Display | Object Views + Explorer + Vertex | Phase 1 Explorer landed; `ui://graph` pending |
| Govern | Actions + Ontology Manager | slot-shaped only; not built |

This table is the most compact statement of what's present, what's partial, and what's deferred.

## 13. Non-goals (explicit)

- Full governance workflow implementation — shape-reserved only
- Closed-world object identity — substrate paths stay open within a Mesh
- Sweeping weaving-metaphor rename of Foundry concepts — explicit decision, not overreach
- LLM composer (Step 7) before manual + config composers (Steps 3–5) exist
- Replacing substrate with a typed store — the permissive core is load-bearing
- Bridging to Foundry in the initial milestones — fidelity classes are reserved for when the bridge is actually built
- Resolving Foundry's Interface-vs-Action polymorphism limitation — inherited tradeoff, out of scope until we hit it

## 14. Expected ADRs

This direction produces several ADR-sized decisions, each deferred to its own document when implementation time comes:

- Object Type primitive + promotion of viewer registry to type registry
- Action Type minimum viable with governance-slot schema
- Ontology Edit transaction shape (single-publish → N-atomic migration path)
- `ui://graph` primitive (Vertex analog; spike brief forthcoming)
- `ui://workshop` + Step 3 config-driven hot-reload UI
- Mesh as root Object (identity anchor; implicates `substrate/` root namespace)
- Capability metadata schema on Object Types (what renders me, accepts me, etc.)

## 15. References

- `.planning/ontology/palantir-foundry-research.md` — architecture research backing the adoption
- `.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md` — whisper probe for Machinery + Functions
- `.planning/explorer/PROJECT-PLAN.md` — Explorer MVP; Phase 1 landed and is Step 1 of this staircase
- `.planning/explorer/PHASE-2-PLAN.md` — Phase 2 sequencing across five tracks
- `.planning/sensors/index.md` — ESP32 sensor/module catalog (200+ modules, 11 categories)
- `~/dev/vector-synth/docs/modules/bus/` — ~200 ⊃μBus module specs (first concrete Object Types)
- `~/dev/vector-synth/crates/vector-synth-core/src/patch_graph/` — patch-graph data model (consumer of `ui://graph`)
- `docs/handoff.md` — session state including the INMP441 MEMS swap and mic race fix context
