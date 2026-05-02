---
title: Sensor ingestion pipeline — whisper probe for the primitive
created: 2026-04-23
status: draft — spike brief, not an implementation plan
drives: discovery of the pipeline primitive's shape
first_sensor_target: audio (INMP441 MEMS mic → whisper.cpp → transcript)
---

# Sensor ingestion pipeline — whisper probe for the primitive

## 0. Purpose

The whisper build is a **probe**, not a deliverable. Its explicit output is not a working transcriber (though it will be one) but **a characterization of the WeftOS sensor-ingestion-pipeline primitive** — its boundaries (what it has to span) and its axes (what varies from one pipeline to the next).

We are not proposing the primitive's API before the spike. We are proposing to build whisper straight, journaling every moment the code "wants to be general," and to harvest those moments into the primitive's axis list. A second sensor (camera or ToF) will validate or refute the shape. Rule of three, applied honestly.

## 1. Framing — where this lives

Under the Foundry-ontology adoption direction (`.planning/ontology/ADOPTION.md`, research at `.planning/ontology/palantir-foundry-research.md`), this primitive is the WeftOS analogue of Foundry's **Machinery + Functions** layer. In the stack:

```
┌─────────────────────────────────────────────────────┐
│ Strict facade (ontology)                            │
│   Object Types / Link Types / Actions / Functions   │
│   ← Pipelines live here as compositions of Functions │
├─────────────────────────────────────────────────────┤
│ Permissive core (substrate)                         │
│   path-keyed KV + pub/sub, no schema enforcement    │
│   ← Raw sensor data lives here                       │
└─────────────────────────────────────────────────────┘
```

The substrate never changes as a result of this spike. The pipeline is a typed composition of Functions above the substrate; the substrate remains the unenforced source of truth.

## 2. Spike target state

Concrete, minimum-viable, end-to-end:

1. The ESP32 sensor node publishes PCM chunks (INMP441 MEMS, 16 kHz mono i16) to `substrate/sensor/mic/pcm_chunk` at roughly 2 Hz (~500 ms per chunk).
2. A new daemon-side crate `clawft-service-whisper` wraps [`whisper-rs`](https://crates.io/crates/whisper-rs) — the Rust FFI bindings to whisper.cpp. On start it loads a small model (tiny.en or base.en) once.
3. The service subscribes to `substrate/sensor/mic/pcm_chunk`, windows incoming chunks into whisper-sized frames, and on segment completion publishes `{ text, start_ms, end_ms, confidence, tick }` to `substrate/derived/transcript/mic`.
4. The existing RMS stream at `substrate/sensor/mic` keeps publishing. PCM is added alongside, not replacing.
5. A Phase 2 `TranscriptViewer` (follow-up, not this spike) later renders the text stream in Explorer's detail pane.

## 3. Scope

**In scope:**
- Single-stage, single-source, single-sink pipeline (whisper only).
- Binary payload handling in substrate (if not supported, this spike is where we discover that and add it).
- Minimal supervision — the service starts with the daemon, logs segments, restarts on panic.

**Out of scope (deferred until second sensor validates axes):**
- A pipeline-TOML or any declarative DAG config.
- Hot-reload of model or chunk-size at runtime.
- Multi-stage composition (no VAD/chunker/punctuation/diarization).
- The Ontology Manager or governance layer.
- Action Type / writeback semantics.
- Camera, ToF, IMU, or any second sensor.

## 4. Questions the spike exists to answer

### 4.1 Boundaries — things the primitive has to span

| # | Boundary | Why whisper forces the question |
|---|---|---|
| B1 | **Binary payloads in substrate** | PCM is ~32 KB/s of i16, not a scalar. Does substrate carry bytes today, or is this the moment we add byte-payload support? If the latter: b64-in-JSON, separate binary side-channel, or native binary? |
| B2 | **Stage state across ticks** | whisper's streaming decoder carries state between chunks for context. Functions are NOT pure. How does the primitive admit stage-local memory without leaking it into substrate? |
| B3 | **FFI / external-dep lifecycle** | whisper.cpp is C++ linked via FFI. Model load is ~1–5 s, one-shot; per-call cost is cheap only after load. Service startup contract has to accommodate expensive init. |
| B4 | **Rate asymmetry** | Input at ~2 Hz, output at ~0.3 Hz (one transcript segment per ~3 s of audio). Does the primitive model stages as 1:1 per tick, or as asynchronous producer/consumer with their own rates? |
| B5 | **Latency / in-flight work** | A whisper chunk takes 200 ms–2 s to transcribe. Synchronous-per-chunk wastes the CPU; asynchronous means in-flight work across stage boundaries. How is that expressed? |

### 4.2 Axes — things that vary pipeline-to-pipeline

| # | Axis | Whisper's answer | Likely variants |
|---|---|---|---|
| A1 | Source cardinality | 1 (one mic path) | N (fan-in from multiple mics, stereo, multiple sensor nodes) |
| A2 | Sink cardinality | 1 (transcript path) | N (transcript + per-segment confidence + speaker ID) |
| A3 | Payload shape | binary PCM + structured transcript | scalars, structured JSON, frames (camera), tensors (vision), events |
| A4 | Composition | linear (mic → whisper → transcript) | chain (VAD → chunker → whisper → punctuation), DAG (transcript + audio features joined downstream) |
| A5 | Backpressure | what happens when whisper falls behind? drop oldest PCM? queue? block upstream? | drop/queue/block per-stage, settable |
| A6 | Reconfig | swap tiny.en ↔ base.en, change chunk size, change language | live vs cold restart |
| A7 | Observability | per-stage latency, input rate, output rate, error count | Explorer-visible counters, traces |
| A8 | Error handling | whisper panics → service restart (supervised) | isolate / quarantine / propagate upstream |
| A9 | Placement | in-daemon process, one thread | separate supervised process, remote node, shared GPU worker |

### 4.3 Concrete questions the spike must answer before exit

1. Does `substrate.publish` accept binary payloads today, or only JSON? If JSON-only, what's the cheapest path for PCM — b64 in a `{ "pcm_b64": "..." }` object, a new `substrate.publish_bytes` RPC, or a parallel binary channel?
2. Is a "stage" a new primitive, or is it just a service that happens to subscribe + publish? (i.e. do we need a `Stage` trait, or does `Service` cover it?)
3. How does the service declare its substrate I/O — in code (explicit subscribe/publish calls), in a TOML manifest, or via an attribute/macro?
4. What's the smallest useful introspection surface? Explorer needs to know "`clawft-service-whisper` is running, input rate X Hz, output rate Y Hz, last-segment Z ms ago." Where does that live?
5. Does the whisper service run in the daemon's process or as a supervised sidecar? Model load (~1–5 s) argues for in-process with lazy init; memory pressure (~500 MB for base) argues for sidecar when many such services exist.
6. What is the transcript Object Type? Proposed shape: `{ text: string, start_ms: u64, end_ms: u64, confidence: f32, lang: string, tick: u64 }` — the spike tests whether "Object Type = JSON schema + path binding" is expressive enough.

## 5. Probes — how the build answers each question

For every boundary and axis above, the spike's implementation forces an answer. Selected entries:

- **B1 (binary)** → will be answered the first time we try `substrate.publish("substrate/sensor/mic/pcm_chunk", pcm_as_value)` and see whether the JSON serializer chokes or silently inflates.
- **B2 (stage state)** → answered by where in the service code we hold the `WhisperContext` struct and how we keep it alive across subscribe-callback invocations.
- **B3 (lifecycle)** → answered by where we put the model-load call. Synchronous at service init vs lazy on first input vs background-loaded with a "not-ready" state — whichever survives first contact with the supervisor.
- **B4 (rate asymmetry)** → answered by whether we can emit 0, 1, or N transcript publishes per input chunk without the subscriber framework fighting us.
- **A5 (backpressure)** → answered by what happens when we deliberately slow whisper (big model, slow CPU) and pump PCM in faster than it drains. Ring-buffer? Unbounded channel? Lost data?
- **A7 (observability)** → answered by what we end up instrumenting in order to debug B4/A5 during the build. Whatever we needed = what the primitive needs to expose.

## 6. Deliverables of the spike

Two artifacts:

1. **Working whisper pipeline.** ESP32 → substrate → whisper service → substrate → (eventual) Explorer transcript view. Transcripts visible in `substrate.read substrate/derived/transcript/mic`.
2. **`.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md`** — every "ugh, this should be a general thing" moment during the build, timestamped, with the workaround chosen. This journal becomes the input to the primitive's formal proposal after the second sensor validates.

## 7. Second-sensor criteria (deferred, noted)

The primitive proposal is blocked until we land a second ingestion pipeline against a sensor category with **different** characteristics. Candidates from the catalog (`.planning/sensors/`):

- **Camera (01-vision-imaging)** — different payload (frame tensor), different composition (DAG — frame + detection + tracking).
- **ToF (02-positioning-navigation)** — different rate (faster, ~30 Hz), different shape (depth grid), already partially plumbed (`substrate/sensor/tof`).
- **IMU (02-positioning-navigation)** — highest rate (~100 Hz), small payload, typically needs feature extraction stage before anything interesting.

A sensor we pick must force at least ONE axis (A1–A9 above) to a value whisper didn't take. Otherwise it's the same spike with different payloads.

## 8. Non-goals

- Designing the pipeline API before the spike. That is the thing this spike exists to NOT do.
- Refactoring the existing mic RMS adapter. It keeps publishing what it publishes.
- Shipping a `TranscriptViewer` in the same change. Phase 1 Explorer's `JsonFallbackViewer` will render transcripts adequately until a dedicated viewer lands in Phase 2.
- Any writeback / governance / Action-Type plumbing. Those belong to the Ontology Manager spike, not this one.

## 9. Success criteria

The spike is done when:

1. Speaking near the mic produces text visible in `substrate/derived/transcript/mic` within ~2 s of the utterance ending.
2. The Phase 1 Explorer, reloaded, shows `substrate/derived/transcript/mic` in the tree; selecting it renders JSON that updates live.
3. All nine axes (A1–A9) have **explicit, written** answers in `PIPELINE-PRIMITIVE-JOURNAL.md`, even if the answer is "not relevant to whisper — defer to sensor #2."
4. All six concrete questions in §4.3 have answers.
5. A short "what the primitive looks like, provisionally, from one data point" section exists at the bottom of the journal — clearly labeled as provisional, to be reconciled when sensor #2 ships.

## 10. References

- `.planning/ontology/palantir-foundry-research.md` — research feeding the ontology adoption.
- `.planning/ontology/ADOPTION.md` — ontology adoption direction (Machinery = pipeline primitive).
- `.planning/explorer/PROJECT-PLAN.md` — the Explorer MVP that consumes pipeline outputs; Phase 2 viewers grow from pipeline products.
- `.planning/sensors/index.md` — sensor catalog (11 categories × ~200 modules) that this primitive eventually has to cover.
- `docs/handoff.md` — running session state; the mic-race and MEMS-mic swap context.
- `crates/clawft-substrate/src/*` — current substrate store; where B1 (binary payloads) will likely need work.
- `crates/clawft-services/` — existing service-registry surface; where the whisper service likely lives.
- `crates/clawft-weave/src/daemon.rs` — RPC dispatch; `substrate.list` is now wired for Explorer observability.

## 11. Explicitly deferred to the second-sensor iteration

The following are known questions that **this spike is allowed to punt on** because a single-sensor answer would be premature:

- Whether pipelines should be declared in TOML, in Rust code, in substrate itself (pipelines-as-substrate), or in some combination.
- The `Pipeline` / `Stage` trait signature — even whether `Stage` deserves to be a trait vs just a convention over `Service`.
- Hot-reload semantics across all stage types.
- Permission / governance model on stage I/O.
- Cross-node stage placement (stages running on different machines in the mesh).

Each of those is a real question. Each of them also gets easier with two concrete pipelines in the ground than with one.
