---
title: Sensor-ingestion pipeline primitive — journal from the whisper probe
created: 2026-04-23
status: living doc — WILL be updated as the second sensor ships
probe: whisper.cpp HTTP service (this spike)
next_probe: camera | ToF | IMU (one of)
companion: .planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md
---

# Sensor-ingestion pipeline primitive — whisper probe journal

The goal of this journal is not to describe **how whisper is wired** — the code
does that — but to capture **every "this should be a general thing" moment**
the build produced, and to answer the questions the spike brief queued up.

A second-sensor iteration (camera / ToF / IMU) is blocked on this journal
being honest; a primitive designed off one data point is an extrapolation, not
an observation. Each section below ends with an explicit what-does-sensor-2-
tell-us bullet.

## 0. Build shape (one paragraph, for orientation)

`clawft-service-whisper` is an in-process daemon service. It holds a clone of
the kernel's `SubstrateService` (an `Arc`-backed DashMap), subscribes via
`substrate.subscribe("substrate/sensor/mic/pcm_chunk")`, decodes the b64
payload into s16le PCM, windows 1–3 s of samples, wraps them in a 44-byte
RIFF/WAV header, POSTs them as `multipart/form-data` to whisper.cpp's
`/inference` endpoint (a **separate HTTP process on localhost:8080**), parses
the returned `{"text": "..."}`, and publishes the transcript to
`substrate/derived/transcript/mic`.

## 1. The earlier FFI framing was wrong

The first spike brief called for `whisper-rs` — a Rust FFI wrapper over
whisper.cpp linked into the daemon process. That brief was superseded before
code was written, after the operator noticed whisper was already running as a
standalone HTTP service on this machine. The HTTP path is the correct one and
this section exists to make that decision legible.

**Why HTTP is the right primitive for the spike:**

1. **Lifecycle separation.** The whisper service loads a ~2 GB model once and
   holds it. A daemon reboot doesn't cost that load. An FFI-linked whisper
   would; every cold daemon start would re-load the model.
2. **Language isolation.** whisper.cpp is C++ + CUDA. Linking it into a Rust
   crate drags in a compiler toolchain, CUDA headers, and a non-trivial build
   matrix. Every developer who ever touches the workspace pays that cost.
   HTTP service-ification makes whisper *someone else's operational concern*.
3. **Replaceability.** The `/inference` contract is narrow enough that we
   could swap out for faster-whisper, remote GPU, or a different model entirely
   without changing a line of WeftOS.
4. **Multiplexing.** One whisper instance can serve N daemons. An FFI-linked
   whisper cannot.
5. **The service model forces primitive axes to surface.** An FFI call would
   have hidden them. See §4 below on "what HTTP-as-stage teaches that
   FFI-as-stage wouldn't."

## 2. Answers to §4.3 of the spike brief (six concrete questions)

### Q1. Does `substrate.publish` accept binary payloads today?

**No.** Substrate is JSON-only — `publish(path, value: serde_json::Value)`.
Shovelling raw bytes requires either (a) b64-encoding them inside a JSON
envelope, (b) adding a parallel `substrate.publish_bytes` RPC + `Entry`-value
variant, or (c) a binary side-channel on the daemon socket.

**What this spike chose:** option (a) — `{ "pcm_b64": "...", "sample_rate": 16000, ... }`.

**Cost on the wire:** base64 inflates by ≈33%. At 16 kHz s16le mono, a 500 ms
chunk is 16 000 bytes raw → 21 333 bytes b64. At 2 Hz chunk cadence that's
42.6 kB/s per mic. Acceptable on a loopback Unix socket + in-process DashMap.
NOT acceptable on the eventual ESP32 radio link, where every kB costs ~8 ms
of WiFi airtime at 2.4 GHz — **we'll want a native binary path before audio
leaves the local host.**

**What the primitive probably wants:** payloads should be a tagged union
`enum Payload { Json(Value), Binary(Bytes) }` with the JSON case preserving
today's zero-cost path and the Binary case landing as a native pub/sub delta.
The substrate `Entry` gains a sibling field (or a refactored `value`), and
`SubscriberSink::ExternalStream` needs a framed wire format (length-prefixed,
or the kernel's existing RVF frame codec — already in the tree).

**Sensor-2 nudge:** a camera frame (1 MB at 720p JPEG, ~10 MB at 4K raw) makes
b64 untenable. ToF (depth grid, tens of kB per frame) is borderline. IMU
(tens of bytes per sample) is fine with JSON. **Camera will force this
axis off the default; ToF will wobble it; IMU won't move it.**

### Q2. Is a "stage" a new primitive, or is it just a Service that subscribes + publishes?

**From one data point: a Service is sufficient.** The whisper pipeline is
literally "tokio task that owns a subscription + a client + a publish call."
Nothing about the code begs for a `Stage` trait.

**What IS different from a generic service:** the shape of its lifecycle.
A pipeline stage wants:
- A **health probe** at startup (`wait_for_healthy`) that's common to most
  stages that talk to external processes.
- A **degraded-but-alive** state separate from "startup failed" — the whisper
  service stays subscribed even when whisper is unreachable, so the daemon
  doesn't crash when the user restarts whisper.cpp.
- A **shutdown drain** that awaits in-flight work before exiting.
- An **input/output declaration** — today expressed in source as
  `SUBSTRATE_PCM_INPUT_PATH` / `SUBSTRATE_TRANSCRIPT_OUTPUT_PATH` constants.
  A declarative pipeline config would want these as structured metadata.

**Provisional verdict:** a `Stage` trait is not required for the whisper
spike. A pattern is — a `PipelineStage` / `SensorStage` shape with the four
lifecycle hooks above, implemented as a convention on top of `Service`.

**Sensor-2 nudge:** a stage that CHAINS (VAD → whisper → punctuation) would
want the subscribe/publish wiring to be composable as a DAG rather than
hand-called per stage. Today we hard-code "sub input, pub output." Two-stage
pipelines will force us to either fan outputs into another subscribe loop, or
introduce a concept of a pipeline graph with declared edges.

### Q3. How does the service declare its substrate I/O?

**In code today** — literal `const` strings. `SUBSTRATE_PCM_INPUT_PATH` and
`SUBSTRATE_TRANSCRIPT_OUTPUT_PATH` are module-level constants; the config
struct carries mutable copies so the operator can override (useful for
multi-mic deployments publishing to differentiated paths).

**What didn't work:** nothing yet, because there's only one stage. A second
sensor will immediately hit "where does this path come from?" — a TOML file,
an Object-Type declaration, a capability-like registry. Not this spike.

**Sensor-2 nudge:** as soon as we have two stages, declaring I/O in code
becomes a smell. The config should live where the Workshop lives — in
substrate itself, per the Step 3 / Step 4 staircase in `.planning/ontology/
ADOPTION.md`. That turns the pipeline into a Workshop-Object-shaped value
and makes "pipelines are substrate" concrete.

### Q4. What's the smallest useful introspection surface?

**What we instrumented to debug the build** — this is the honest floor:
- `wait_for_healthy` outcome (true / false, with backoff count)
- `substrate: publish` tick + actor on each transcript
- `whisper service: dropped oldest window` warning counter (backpressure visibility)
- `transcription failed` error log with window start/end ms
- `base_url` of the whisper service (degrades visibility if the operator
  swaps it unknowingly)

**What the primitive should probably expose** on a `substrate/meta/service/whisper`
topic (not yet wired):
- `state`: `starting | healthy | degraded | stopped`
- `input_rate_hz`: rolling window
- `output_rate_hz`: rolling window
- `backlog_windows`: 0 or 1 with drop-oldest (always small; interesting
  only when the policy changes)
- `last_transcript_age_ms`: for staleness chip in Explorer
- `whisper_url` + `whisper_last_seen_ok_at`

**Where it lives:** a sibling topic under `substrate/meta/service/<id>/`,
owned by the service. That keeps Explorer's tree-walk uniform — every
service gets a health subtree at a predictable path.

**Sensor-2 nudge:** different sensors want different rate-kind metrics (frames
per second for camera, samples per second for IMU). A scalar `output_rate_hz`
is a lowest-common-denominator — the primitive may want per-stage metric
declarations in the capability metadata, not a fixed schema.

### Q5. In-process or supervised sidecar?

**Whisper answers both: the service client is in-process, the model is in a
sidecar.** This is not a cop-out — it's the "right" answer for this class of
stage and the distinction *is* the axis.

In-process parts:
- WhisperClient (tiny — just reqwest + a semaphore)
- WhisperService (tokio task wiring substrate ↔ client)
- Windower + WAV writer (pure data)

Out-of-process:
- whisper.cpp server (2 GB model in RAM, CUDA context, its own crash domain)

This decoupling is free in an HTTP-first design and expensive in an FFI-first
one. **This is the main lesson of the probe.**

**Sensor-2 nudge:** camera stages probably want their model (YOLO / Detectron)
in the *same* sidecar pattern for the same reasons. IMU / ToF preprocessing
is usually in-process because the per-sample math is cheap and has no model
to load. So "placement" is per-stage, not per-pipeline.

### Q6. What is the transcript Object Type?

**Shape emitted today:**
```json
{
  "text": " hello world",
  "start_ms": 0,
  "end_ms": 2000,
  "confidence": null,
  "lang": "en",
  "seq": 2
}
```

Notes:
- `confidence` is `null` because `response_format=json` doesn't carry it. A
  future path could hit `verbose_json` and populate from
  `segments[0].avg_logprob` — at the cost of one extra encoder sweep on the
  server. We judged that not worth the latency for live streaming; a
  batch-mode Object View variant is the better way to expose it.
- `seq` is the producer's chunk sequence id (the LAST chunk folded into the
  window). It lets a downstream joiner correlate against `substrate/sensor/mic/pcm_chunk`
  without timing assumptions.
- `tick` is carried by the substrate publish envelope, not the value. Object
  Types that want tick-scoped correlation read it off the subscribe line.

**Was "Object Type = JSON schema + path binding" expressive enough?** Yes,
for a single-path scalar-ish Object. The metadata that's missing is the
capability info the ADOPTION.md staircase will need — *which Viewers render
me, which Functions accept me, which Actions link out*. That lives one
layer up, not in this spike.

**Sensor-2 nudge:** a camera frame Object Type is not a JSON value any
sensible schema can describe — it's a binary blob + a manifest. The Object
Type primitive will need a "payload reference" mode: value-in-substrate for
structured/small, path-reference-plus-manifest for binary/large. This spike
doesn't light up that requirement; camera will.

## 3. Nine axes (A1–A9) — whisper's observed values + variants

| Axis | Whisper value | One-line gloss + future variant |
|---|---|---|
| **A1 Source cardinality** | 1 | One `substrate/sensor/mic/pcm_chunk` path. Variant: N mics (multi-node mesh) fan into one transcript path; substrate-path wildcards or a multi-subscribe would be needed — neither exists today. |
| **A2 Sink cardinality** | 1 | One `substrate/derived/transcript/mic`. Variant: split transcript + per-segment confidence + speaker-id into sibling paths — just more publishes, no API change. |
| **A3 Payload shape** | b64 PCM (in) + structured JSON (out) | See Q1 above. Variants: scalars (IMU), structured JSON, binary frames (camera), tensors (vision models), event envelopes. The primitive will need per-stage payload declarations. |
| **A4 Composition** | linear — mic → whisper → transcript | Variant: chain (VAD → chunker → whisper → punctuation), DAG (transcript + audio features joined). Today we hard-code the substrate paths; DAG composition needs a pipeline-graph spec. |
| **A5 Backpressure** | **drop-oldest (chose client-side semaphore + single pending-window slot)** | Variant: queue (per stage), block upstream, per-subscriber policy. Whisper's server mutex (API §1) forced the choice — no 429 means we serialize client-side with 1 permit. New windows arriving while a window is in flight replace the pending slot. See §3.1 below. |
| **A6 Reconfig** | cold restart only today | Variant: hot-swap model (whisper's `POST /load` — we don't exercise it), change chunk_ms at runtime (would need a signal path). Today changing `window_ms` requires a service respawn. |
| **A7 Observability** | inline tracing logs | See Q4 above. Primitive should fold into a `substrate/meta/service/<id>/` subtree; not done in this spike. |
| **A8 Error handling** | 5xx→retry-once; 4xx→log+drop; 503→retry-with-backoff; panic→daemon keeps running (service drops, not the process) | Variant: isolate (mark-degraded), quarantine (dead-letter path), propagate upstream (pause the pcm_chunk source). Service-level supervision is implicit in the daemon — the pipeline task can fail without taking the daemon down. |
| **A9 Placement** | in-daemon client + out-of-daemon whisper server | **The key answer.** Variant: pure in-proc (IMU preprocessing), supervised sidecar (camera YOLO), remote HTTP (whisper today), remote gRPC (future), shared GPU worker (whisper-across-all-daemons). See §4 below. |

### 3.1 Sub-note on backpressure choice (A5)

There are three plausible policies for "PCM arrives faster than whisper
drains." They differ in what they optimize.

| Policy | Optimizes | Cost |
|---|---|---|
| **Drop-oldest** (chosen) | freshness — "what's being said right now" | transcription gaps when utterances span dropped windows |
| **Queue bounded** | completeness for short backlogs | latency monotonically grows until queue clears; on sustained overload, degenerates |
| **Block upstream** | integrity — every sample is considered | back-pressure propagates into the PCM producer (ESP32 WiFi, host audio ring buffer) and can drop samples there instead — usually worse than dropping whole windows |

For live speech, drop-oldest is the common-sense choice; for offline batch
transcription, queue-bounded makes sense. **The primitive should expose the
policy as a config axis** rather than pick one.

## 4. What HTTP-as-stage teaches that FFI-as-stage would have hidden

This is the most important section for the primitive's shape, and it only
exists because we almost built the wrong thing.

### 4.1 Stage placement is its own axis (A9 gets promoted)

With an FFI-linked whisper, placement is invisible — the stage is a function
call in the same process. You'd never think to model placement as a first-
class concern. With HTTP, the placement is forced into the open on day one:

- The service has its own lifecycle (it can be up, down, loading, restarting).
- It has its own backpressure model that isn't yours to design.
- It has its own observability surface (stderr, port, timing blocks) that
  you don't control.
- It has its own failure modes (network partition, DNS failure, TLS cert
  rotation — none of which apply to an FFI call).

**The primitive needs to model placement as a first-class concern because
stages WILL migrate between placements over their lifetime.** A stage that
starts as in-proc (prototyping phase) will become a sidecar (production
phase) will become a remote worker (scale phase). If the primitive bakes
placement into the stage definition, every migration is a rewrite.

### 4.2 Health probes are not a cross-cutting nicety, they are a stage contract

The FFI version would have had `whisper::init()` as a synchronous 1–5 s
block at service startup. If init failed, the service wouldn't start.

The HTTP version has a multi-state health surface: `"ok"`, `"loading model"`,
unreachable, 4xx (you're talking to the wrong service), 5xx (it's crashed).
The service has to handle each differently — and "unreachable at startup" can't
abort the daemon or we have a boot-ordering ratrace with whisper.

**Primitive consequence:** every stage needs a declared `ready()` contract
with richer states than "started." The primitive should provide it as a
default no-op that stages override, not as an opt-in.

### 4.3 Client-side concurrency is a stage property, not a plumbing detail

Whisper's one-in-flight model is a service property, but the *mitigation*
(semaphore with permits=1) is client-side. A different whisper service
instance (bigger GPU farm) might allow parallel requests. The primitive
should let each stage declare a concurrency ceiling — and the service
registry should coordinate when multiple clients want to share a stage.

This is invisible to an FFI model: the function call is the function call.
The HTTP model forces you to think of the stage as having its own rate limit
that you must honor.

### 4.4 The test story gets better, not worse

Wiremock-backed tests (`end_to_end_with_mocked_whisper`,
`service_survives_whisper_down_at_start`, `drops_oldest_window_when_inference_slow`)
are hermetic, fast, and don't require a whisper binary, a model file, or
CUDA. With FFI-linked whisper, the equivalent tests would need a stub
whisper-rs, which is either a mocked dyn-trait layer (complex) or a real
whisper tiny.en load (slow + fragile).

**Primitive consequence:** stages that talk to a declared external
endpoint get test ergonomics for free via the endpoint mock. The primitive
should make "declare your external endpoints" part of the stage contract
so harnesses can construct appropriate mocks automatically.

## 5. Fidget-level observations (fodder, not conclusions)

- **"Write a WAV header" is 30 lines of known code.** No crate needed. If
  the only reason to pull a dep is to avoid writing `b"RIFF"`, skip the dep.
- **base64 in JSON works but feels ugly.** Every sensor stage that carries
  binary payload will touch this; the ugliness is a tax on the spike's
  choice to not change substrate. A primitive proposal that preserves the
  b64 path for flexibility but defaults to binary is probably correct.
- **`tokio::select!` with an optional in-flight future is awkward.** We use
  `std::future::pending()` to park the arm when there's no handle. The
  shape of that code wants to be `pipeline.run()` with backpressure as a
  builder option — another reason the stage primitive wants its own runtime,
  not raw tokio.
- **Whisper's "leading space" convention** (API §3) is a trap for naive
  clients; we strip it. If the primitive has a per-stage "output sanitizer"
  concept, it should default-in for known quirks of known endpoints.
- **The in-process `SubstrateService::clone`** (it's `Arc`-backed) is exactly
  the sharing pattern a pipeline wants. Tokio tasks get their own handle,
  all tasks see the same state. If the primitive ships with a well-named
  `SubstrateHandle` wrapper that hides the Arc, it's more discoverable.

## 6. Provisional primitive shape — SEEDED, NOT FINAL

**A `SensorStage` in WeftOS, circa whisper-probe:**

```rust
trait SensorStage {
    // Identity + introspection
    fn id(&self) -> &'static str;
    fn input_topics(&self) -> &'static [TopicDecl];
    fn output_topics(&self) -> &'static [TopicDecl];

    // Placement (A9)
    fn placement(&self) -> Placement;
    //   InProc | Sidecar { process_path } | RemoteHttp { url } | RemoteGrpc { addr }

    // Lifecycle
    async fn ready(&self) -> Readiness;
    //   Ready | LoadingModel | Degraded(reason) | Down(reason)

    // Backpressure (A5)
    fn input_policy(&self) -> BufferPolicy;
    //   DropOldest | BlockCapped | Refuse     (borrowed verbatim from ADR-017)

    // The pipeline body
    async fn run(self: Arc<Self>, substrate: SubstrateHandle, shutdown: Shutdown);
}

// Standalone observability — what the primitive layer publishes *for* the
// stage, without the stage opting in.
//   substrate/meta/service/<id>/state        — Ready | LoadingModel | …
//   substrate/meta/service/<id>/rates        — { in_hz, out_hz, dropped_hz }
//   substrate/meta/service/<id>/last_output  — tick + age_ms
```

Things that are **deliberately missing** from this sketch:
- Composition (multi-stage DAG) — wait for sensor 2.
- Hot-reload — wait for a use case that can't tolerate a restart.
- Permissions / governance — slot-shaped, filled by the ontology later.
- Declarative config — stays in code until a second stage demands it.
- Object-Type metadata on topics (capabilities, viewer bindings, etc.) —
  belongs to the ontology layer, not the pipeline layer.

**This sketch is wrong in some way.** Whisper is one data point. The rule
of three demands a second non-whisper stage before we promote any of this
to a published ADR.

## 7. Sensor-2 selection criteria (restatement of spike §7)

The next sensor must force at least one axis to a value whisper didn't take.
Ranked by how much each candidate changes:

| Candidate | Axis moves most | Why pick |
|---|---|---|
| **Camera** | A3 payload shape (binary frames), A1 source cardinality (multi-camera is common), A4 composition (frame + detections is always a DAG) | Maximum primitive pressure. Forces the binary substrate path out of hiding. |
| **ToF** | A3 payload shape (depth grid), A4 composition (heatmap + segmentation is a DAG) | Moderate — doesn't force the binary path as hard as camera, but forces per-frame structured payloads. |
| **IMU** | A4 composition (always needs a feature extractor before anything else), A9 placement (no reason to sidecar — forces pure in-proc variant) | Least pressure on payload, most pressure on composition + feature-extraction patterns. |

**Current lean: camera.** Biggest axis pressure on the parts the whisper
spike *couldn't* exercise — binary substrate, DAG composition, non-trivial
sink cardinality.

## 8. What this journal does NOT claim

- It does not propose an API. It notes where an API would be helpful.
- It does not justify the b64-in-JSON choice beyond "avoided touching
  substrate; acceptable on loopback." It explicitly flags the ESP32 path
  as a later problem.
- It does not export an Object Type. The transcript shape is in code, not
  registered with any typed layer — because no typed layer exists yet.
- It does not claim the whisper spike is "done." The spike is done when
  success criteria §9 of the brief are all ticked; this journal ticks them
  but leaves the second-sensor work open.

## 9. Where this journal plugs into the rest of the tree

- `.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md` — the brief this answers.
- `.planning/ontology/ADOPTION.md` §12 — the four-verbs table. This stage
  is `process` (Machinery + Functions). An eventual `ui://transcript`
  primitive is `display`. The writeback end of a transcribed command
  (imagine "turn off lights") is `govern` + Actions.
- `crates/clawft-service-whisper/src/service.rs` — the code these notes
  describe.
- `docs/handoff.md` — the INMP441 MEMS-mic context that the ESP32 bridge
  lives in; the upstream half of this pipeline's input path.
- `~/llama.cpp/docs/whisper-service-api.md` — the external contract this
  service is a client of.

---

*This is a living document. When the second sensor's journal merges, this one
gets revised into the primitive proposal — not kept as-is.*

---

## R2 revision — Source / Stage / Sink split

**Added:** 2026-04-24.
**Context:** two main-thread decisions landed after R1 was written and
invalidate two of R1's load-bearing assumptions. This revision updates
the primitive shape; everything above is preserved as the R1 record.

**Why append rather than fork:** R1 and R2 share ~90% of their observed
axes (A1–A9, backpressure policies, placement framing, health-probe
contract). Forking into a companion doc would duplicate the axis
tables and bury the diff. The decision *is* the diff, and a reader
should see it next to what it replaced.

### R2.0 What changed in the main thread

Two decisions, both load-bearing:

### D1 — Node vs Actor identity split

A **Node** is a physical thing in the mesh; it emits (sensor data,
heartbeats) and signs what it publishes with its ed25519 key. An
**Actor** is an agent / program / user; it performs *Actions*
(Foundry-style typed mutations) and has its own separate key.
**Sensing is not acting.** Every substrate path is scoped
`substrate/<node-id>/...` — no exceptions, no flat paths. Node-id is
the first-6-hex-chars of `blake3(pubkey)` with an `n-` prefix, per
`JOURNALED-NODE-ESP32.md` §2.2.

Kernel-class nodes run `clawft-kernel`; leaf nodes run
`weftos-leaf-types` + minimal firmware. Whisper runs on a kernel-class
node (the daemon host).

### D2 — Pipeline primitive splits into Source / Stage / Sink

R1 tried to collapse the whole pipeline into one `SensorStage` trait.
R2 says that was wrong, and splits it three ways:

- **Source** — one substrate-subscribe + deserialize boundary. One
  per input topic. No pure work past the deserialize.
- **Stage** — pure in-process work (windowing, inference, fusion).
  No substrate footprint. Typed channel in, typed channel out. Any
  number chained.
- **Sink** — the publish terminator. Owns its substrate path, its
  signing identity (the publishing node's key), its sensitivity
  tier, its publish cadence, and its backpressure contract.
  Exactly one per pipeline.

Rationale (taken in order):

1. **Identity/signing happens at publish, not at every stage.** R1
   had no clean place to put signing — the `SensorStage` trait was
   ambiguous about whether subscribe-time or publish-time was the
   signed boundary. A `Sink` localises signing.
2. **Sensitivity tier applies at publish.** A fused boolean
   "speech detected" derived from Capture-tier PCM may itself be
   an `Ambient`-tier signal — the fusion is where the downgrade
   happens. R1 couldn't express that without stage-local tier
   declarations and a merge rule.
3. **Mesh visibility applies only at publish.** Intermediate stages
   shouldn't clutter substrate. R1's "declare your I/O topics"
   encouraged per-stage substrate emissions; R2 forbids them.
4. **One-job-per-primitive.** R1 mixed subscribe + pure work +
   publish in one trait; debugging it in the whisper service
   (see `run_pipeline`) means reading one 100-line `tokio::select!`
   to find the bit you care about. R2's three primitives are
   separately testable.

### R2.1 R1 shape — single-primitive (preserved for comparison)

```rust
// R1 — SEEDED, NOT FINAL (as written in §6 above)
trait SensorStage {
    fn id(&self) -> &'static str;
    fn input_topics(&self) -> &'static [TopicDecl];
    fn output_topics(&self) -> &'static [TopicDecl];
    fn placement(&self) -> Placement;
    //   InProc | Sidecar { process_path } | RemoteHttp { url } | RemoteGrpc { addr }
    async fn ready(&self) -> Readiness;
    //   Ready | LoadingModel | Degraded(reason) | Down(reason)
    fn input_policy(&self) -> BufferPolicy;
    //   DropOldest | BlockCapped | Refuse
    async fn run(self: Arc<Self>, substrate: SubstrateHandle, shutdown: Shutdown);
}
```

### R2.2 R2 shape — three primitives

None of the Rust below needs to compile; it is documentation of the
fields we believe each shape wants, inferred from the whisper probe
under the D1+D2 decisions.

### R2.2.1 `Source<T>` — substrate-subscribe + deserialize boundary

```rust
trait Source<T: DeserializeOwned + Send + 'static> {
    fn id(&self) -> &'static str;

    /// Fully scoped substrate path. MUST start with `substrate/<node-id>/`.
    fn input_topic(&self) -> &str;

    /// Attach to substrate; decode each update into T; forward to out.
    /// Panics + deserialization errors are logged and the offending
    /// update is skipped — the stream does not close.
    async fn run(
        self: Arc<Self>,
        substrate: SubstrateHandle,
        out: mpsc::Sender<Framed<T>>,
        shutdown: Shutdown,
    );
}

/// Each Source emits framed payloads — value + the substrate metadata
/// the downstream might want (source_tick, source_actor, source_path).
/// Stages generally ignore the frame; Sinks may include it in lineage.
struct Framed<T> {
    value: T,
    source_path: String,
    source_tick: u64,
    source_actor: Option<String>,
}
```

**One Source per input topic.** A fusion pipeline with N inputs has N
Source primitives. (This is one of the new open questions — §R2.6 Q3.)

### R2.2.2 `Stage<I, O>` — pure in-process transform

```rust
trait Stage<I: Send + 'static, O: Send + 'static> {
    fn id(&self) -> &'static str;

    /// Pure in-process work. No substrate access. No network. No
    /// filesystem beyond read-only model load at init.
    ///
    /// Backpressure: if Stage is slow, its input channel fills; the
    /// upstream Stage (or Source) blocks. Whisper-style drop-oldest
    /// is NOT a Stage concern — it is a Sink concern; intermediate
    /// Stages preserve all data.
    async fn run(
        self: Arc<Self>,
        input: mpsc::Receiver<I>,
        output: mpsc::Sender<O>,
        shutdown: Shutdown,
    );

    /// Optional: loadable-model state. `ready()` returns
    /// `LoadingModel` until `init()` returns; pipelines gate their
    /// Sink's `publishing` state on this.
    async fn init(&self) -> Result<(), InitError> { Ok(()) }
    fn ready_state(&self) -> StageReadiness {
        StageReadiness::Ready
    }
}

enum StageReadiness { Ready, LoadingModel, FailedInit(String) }
```

Notes:

- **No `input_topics` / `output_topics`**. Stages are wired by typed
  channels, not by substrate paths. `windower.rs` already fits this
  shape exactly — it takes `&[u8]` chunks and returns `Option<PcmWindow>`
  synchronously, no substrate knowledge, no async.
- **`placement`** is not on the Stage signature. See §R2.4 for why
  the axis may collapse.
- **Init separation** lets whisper-style "I am loading a 2 GB model
  for 5 s" be expressed without blocking the Source → Stage pipe.

### R2.2.3 `Sink<T>` — the publish terminator

```rust
trait Sink<T: Send + 'static> {
    fn id(&self) -> &'static str;

    /// Fully scoped substrate path. MUST start with
    /// `substrate/<this-node-id>/` — the publishing node's prefix,
    /// not any source's.
    fn output_topic(&self) -> &str;

    /// Signing identity. This is the publishing node's keypair
    /// handle; the Sink does not choose it, the pipeline runtime
    /// injects it at construction.
    fn signer(&self) -> &dyn NodeSigner;

    /// Sensitivity tier of the emitted payload. May be lower than
    /// the tier of any upstream Source (fusion can downgrade).
    /// Never silently raised — a Sink that wants to EMIT Capture
    /// from Ambient inputs is a programmer bug; the pipeline
    /// runtime asserts `tier_out >= max(tier_in)` is NEVER true
    /// without an explicit override.
    fn sensitivity_tier(&self) -> Sensitivity;
    //   Ambient | Capture | Privileged

    /// When to publish.
    fn cadence(&self) -> Cadence;

    /// What to do when inputs arrive faster than we publish.
    fn buffer_policy(&self) -> BufferPolicy;
    //   DropOldest | BlockCapped { n: usize } | Refuse
    //   (R1's A5 taxonomy, relocated from Stage to Sink)

    /// Optional: how the Sink annotates its emissions with lineage
    /// back to its Source paths. Used by the Explorer's lineage
    /// view (see EXPLORER-MANAGEMENT-SURFACE.md #14).
    fn lineage(&self) -> LineageMode {
        LineageMode::Inline
    }

    /// Consume typed values; publish each (or each tick, depending
    /// on cadence) via the signer.
    async fn run(
        self: Arc<Self>,
        input: mpsc::Receiver<T>,
        substrate: SubstrateHandle,
        shutdown: Shutdown,
    );
}

enum Cadence {
    /// Publish on every input (whisper: one transcript per completed
    /// inference, gated by the Stage's emit-when-done behaviour).
    Event,
    /// Publish every N ms regardless of input rate. The Sink holds
    /// the most-recent value; on tick it emits that value. Good for
    /// summary gauges, mic RMS chip, etc.
    Periodic { ms: u64 },
    /// Publish on input, but at most once per `min_ms`. Debounced
    /// event mode — useful when a Stage is bursty but downstream
    /// consumers want a bounded refresh rate.
    EventDebounced { min_ms: u64 },
}

enum LineageMode {
    /// Publish lineage metadata inline on each emission.
    Inline,
    /// Publish lineage once at a sibling `<topic>/meta/lineage` path,
    /// not on each emission. Matches the EXPLORER-MANAGEMENT-SURFACE
    /// §6 Q3 proposal.
    Sibling,
    None,
}
```

### R2.2.4 `Pipeline` — the composition

A Pipeline is the declarative wiring of `N Sources → Stage chain → 1 Sink`:

```rust
struct Pipeline {
    id: &'static str,
    node_id: NodeId,            // runs on this node; fills Sink.signer()
    sources: Vec<Box<dyn SourceAny>>,
    stages: Vec<Box<dyn StageAny>>,  // chained, typed at construction
    sink: Box<dyn SinkAny>,
}
```

Construction verifies:

- All Source output types match the first Stage input type (or all
  match, for fan-in — see §R2.6 Q3).
- Adjacent Stage input/output types match.
- Final Stage output type matches Sink input type.
- Sink's `output_topic` starts with `substrate/<node_id>/`.
- `Sink::sensitivity_tier() >= max(every Source's tier)` is checked
  at construction unless explicitly overridden (and the override is
  an auditable decision, not a silent allow).

### R2.3 Whisper pipeline — R1 vs R2 side-by-side

### R1 framing (single-primitive)

```text
 substrate/sensor/mic/pcm_chunk   substrate/derived/transcript/mic
          ▼                                      ▲
    ┌──────────────────────────────────────────┐
    │ WhisperSensorStage : SensorStage         │
    │   subscribe + deserialize                │
    │   windower (pure)                        │
    │   HTTP POST /inference                   │
    │   publish                                │
    └──────────────────────────────────────────┘
```

All five jobs in one trait implementation. This is what
`crates/clawft-service-whisper/src/service.rs::run_pipeline` is today:
one `tokio::select!` loop with subscribe, decode, window, spawn
inference, publish-result arms.

### R2 framing (three primitives)

With post-migration scoped paths:

```text
 substrate/<esp32-node>/sensor/mic/pcm_chunk
                  │
                  ▼
           ┌─────────────┐
           │  Source<    │   (substrate-subscribe + b64 decode)
           │  PcmChunk>  │
           └─────────────┘
                  │  mpsc<PcmChunk>
                  ▼
           ┌─────────────┐
           │ PcmWindower │   (pure state machine — already exists
           │  : Stage<   │    at windower.rs; no substrate I/O)
           │  PcmChunk,  │
           │  PcmWindow> │
           └─────────────┘
                  │  mpsc<PcmWindow>
                  ▼
           ┌─────────────┐
           │  Whisper    │   (HTTP POST /inference; init() loads
           │  Inference  │    model by priming the remote service's
           │  : Stage<   │    health probe; placement is hidden here
           │  PcmWindow, │    — see §R2.4)
           │  Transcript>│
           └─────────────┘
                  │  mpsc<Transcript>
                  ▼
           ┌─────────────┐
           │  Sink<      │   (publish-terminator, daemon's signer,
           │  Transcript>│    cadence: Event, drop-oldest buffer)
           └─────────────┘
                  │
                  ▼
 substrate/<daemon-node>/derived/transcript/<esp32-node>/mic
```

### R2.4 Transcript path contradiction — resolved

The R1 journal and early Phase 2 text used
`substrate/derived/transcript/mic` — unscoped. `JOURNALED-SENSOR-MIC.md`
§6 notes the same path but flags it as needing clarification:

> "The Whisper service is an *actor*, not a node. It does not sign
> emissions; it performs Actions. Where does its output land?"

**Resolution under R2 (combined with D1):**

- Whisper is **not** an Actor in the Action-Types sense. It is a
  Sink running on the daemon-node. Sinks sign with the node's key.
- Therefore the transcript publishes under the **daemon-node's**
  prefix, not the ESP32's, not an actor's.
- The ESP32 is encoded in the *path structure* (it is the thing this
  transcript is derived FROM), not in the signing identity.

**Canonical shape:**

```text
substrate/<daemon-node-id>/derived/transcript/<esp32-node-id>/mic
```

General rule this instance of:

```text
substrate/<publishing-node-id>/derived/<kind>/<source-node-id>/<source-family>
```

That shape:

- Makes it obvious which node is authoritative for the data
  (the prefix).
- Makes it trivially filterable in the Explorer by source
  (everything derived *from* ESP32 mic, regardless of which kernel
  node computed it).
- Makes redundancy explicit: if two daemons both run whisper on the
  same ESP32 mic, they publish to different prefix paths and a
  downstream consumer can pick one (or merge).

(Which leads to §R2.6 Q1 — is that redundancy correct or wasteful?)

### R2.5 What the split costs and what it buys

### Costs

1. **Three primitive types instead of one.** A trivial pipeline like
   mic-summary-to-gauge ("pass the AudioStream envelope through
   unchanged") is a `Source` + `Sink` with no intermediate Stage.
   That's still two types, not one. For the simplest pipelines
   R2 has more ceremony than R1.
2. **Explicit pipeline-construction step.** R1 spawned a single
   `SensorStage::run` on a tokio task. R2 needs a Pipeline struct
   that wires channels between N things. More code in the shared
   runtime, less in each pipeline.
3. **Typed channels require known payload types at construction.**
   R1 was bytes-in, bytes-out at every boundary (substrate is JSON;
   stage-internal types are whatever the stage defines). R2 forces
   you to declare the intermediate types up front. For the two-stage
   whisper pipeline that's 2 extra named types (`PcmChunk`,
   `PcmWindow`, `Transcript` — all three already exist in
   `windower.rs` and `service.rs`; no cost).

### Buys

1. **Identity unambiguous.** Signing happens at one place. Review of
   "who owns this path" is trivial — the Sink knows, nobody else
   does.
2. **Governance localised.** Sensitivity-tier checks, audit-sink
   hooks, edit-visibility tags — all plug into the Sink. R1 had no
   natural home for any of them.
3. **Composition clean for fusion.** Multi-Source fan-in is N
   Sources, shared first Stage, unchanged Sink contract. Under R1
   we had hand-coded pipeline-graph composition as deferred work;
   R2 makes it a construction detail.
4. **In-process stages never touch substrate.** Explorer doesn't
   see intermediate stage activity. Fewer topics to subscribe to,
   fewer spurious updates. R1's "declare your output topics" was
   pulling us the wrong way.
5. **Per-stage testing free.** `windower.rs` is already a pure
   function with its own test suite — R2 formalises that pattern
   across every Stage. Each Stage is testable with `channel in,
   channel out, assert outputs`. Sinks and Sources have their own
   mock patterns (wiremock for HTTP sinks, mock-substrate for
   substrate-facing).

### R1 fields mapped into R2

| R1 field | R2 location | Notes |
| --- | --- | --- |
| `id` | All three (Source.id, Stage.id, Sink.id) | Needed per-primitive |
| `input_topics` | `Source::input_topic` (one per Source) | Cardinality moves from N-on-stage to N-Sources |
| `output_topics` | `Sink::output_topic` (exactly one) | Sink cardinality is always 1 in R2; see R2.6 Q-also-added |
| `placement` | collapses (§R2.4 below) | Was the R1 key insight; R2 makes it a Stage-internal choice |
| `ready()` | `Stage::ready_state()` + Sink publishing-gated | Becomes per-Stage, not per-pipeline |
| `input_policy` / `BufferPolicy` | `Sink::buffer_policy()` | Moves from Stage to Sink — drop-oldest is a publish-side concern |
| `run` | Three `run`s, one per primitive | Split by concern |

### Placement axis (A9) revisited

R1 §4.1 promoted placement (InProc / Sidecar / RemoteHttp / RemoteGrpc)
to a first-class primitive concern. **R2 dissolves that axis at the
primitive level** and pushes it inside the Stage:

- A `WhisperInference` Stage that talks to whisper.cpp over HTTP is
  a Stage with an HTTP client in its state.
- A `WhisperInference` Stage that links whisper-rs via FFI is a
  Stage with a model handle in its state.
- The *pipeline* doesn't care; the Stage trait is identical in both
  cases.

Placement is no longer a pipeline-primitive axis — it is an
implementation choice inside a Stage, invisible to the pipeline
runtime. A Stage can declare `init()` if it has expensive startup;
that's the only externally-visible signal.

**What R1 got right and R2 preserves:** the *tests* for external
dependencies still benefit from the declared-endpoint pattern (R1
§4.4). In R2 that lives as a per-Stage convention, not a primitive
slot.

### R2.6 New open questions created by the split

### Q1. Redundant kernel-class nodes running whisper

If two kernel-class nodes both run the whisper pipeline against the
same ESP32's mic, each publishes its own transcript under its own
prefix:

```text
substrate/<daemon-A>/derived/transcript/<esp32>/mic
substrate/<daemon-B>/derived/transcript/<esp32>/mic
```

**Tradeoff:**

- **Redundancy is a feature.** Either daemon's transcript is
  usable; if one dies the other carries. Downstream consumers
  (e.g. a subscription that says "give me any transcript of
  `<esp32>/mic`") filter `derived/transcript/<esp32>/mic`
  across all daemon-prefixes and pick the freshest or highest
  confidence.
- **Redundancy is wasteful.** Two 2 GB model loads, two GPU
  contexts, two copies of identical work. A leader election or
  assignment layer (only one daemon takes responsibility for
  each mic) saves resources but adds consensus machinery.

**Not deciding.** Both readings are defensible. Worth noting that
the ontology already implies consumers do shape-matching across
paths — so filtering "all transcripts of this mic" is not new
code. Leader election would be, and it pulls in the chain /
consensus layer that Phase 2 has deferred.

### Q2. Sink cadence shape

R2.2.3 sketches `Cadence::{Event | Periodic{ms} | EventDebounced{min_ms}}`.

**Does this taxonomy cover the actual cases?**

- Whisper: `Event` — publishes iff inference succeeded iff the
  window had speech. ✓
- Mic summary gauge: `Periodic{ms: 500}` — the gauge wants a steady
  tick, independent of whether RMS changed. ✓ (but maybe the right
  thing is "publish on tick OR on change, whichever first" — not
  representable as-drawn)
- ToF depth frame: `Event`? `Periodic`? Both make sense.
- Node-level health: `Periodic{ms: 5000}`. ✓
- An edge-triggered alarm ("door just opened"): `Event`. ✓

**Uncovered cases I can think of:**

- **On-change** (publish only when the value differs from the last
  publish) — common enough that `Cadence::OnChange` is probably
  its own variant.
- **Periodic + on-change** ("heartbeat every 5 s OR sooner if
  anything changes") — expressible as two Sinks on the same path
  but that violates "exactly one Sink per pipeline"; or as a new
  variant `PeriodicOrChange{ms}`.

**Proposal**: extend to `Cadence::{Event | Periodic{ms} |
EventDebounced{min_ms} | OnChange | PeriodicOrChange{ms}}`.
Five variants feels right — each covers a distinct downstream
subscriber need. **Sign-off needed** on the shape.

### Q3. Multi-input Sources — N primitives or one with N topics

For a fusion pipeline that takes mic + camera + ToF:

**Option A:** N Source primitives, each with one `input_topic()`,
all feeding the same first Stage:

```rust
let srcA = MicSource::new("substrate/<esp32>/sensor/mic/summary");
let srcB = CamSource::new("substrate/<esp32>/sensor/camera/frame");
let srcC = TofSource::new("substrate/<esp32>/sensor/tof/grid");
// First Stage has three inputs — typed as enum or as triple-mpsc
```

**Option B:** One Source primitive declaring N input topics,
producing one framed union value:

```rust
let src = MultiSensorSource::new(&[
    "substrate/<esp32>/sensor/mic/summary",
    "substrate/<esp32>/sensor/camera/frame",
    "substrate/<esp32>/sensor/tof/grid",
]);
// Source output type is `FrameUnion::Mic(_) | Cam(_) | Tof(_)`
```

**Argument for A:** each Source is simple. Each has its own
deserialize logic (a mic chunk and a camera frame don't share a
payload shape). Errors in one Source don't contaminate the others
(one mic deserialize-error logged, the camera stream unaffected).
The pipeline construction sees N typed edges; types are honest.
Matches the "one job per primitive" rationale that drove the split.

**Argument for B:** the downstream Stage often *needs* time
correlation — "the frame that happened at the same tick as this
mic window." Merging in the Source keeps that correlation local.
Option A defers the merge to the Stage, which either gets a
combined-mpsc via `tokio::select!` (awkward typing) or a custom
merge primitive.

**Lean: A** — keep Sources one-topic-each, introduce a
**`MergeStage`** primitive in the Stage catalog that takes N
typed inputs and produces one typed output. That keeps each
Source small; it puts the correlation logic in a dedicated
named Stage where it belongs; it is what the "pure in-process
work" rationale for Stage was aiming at.

**Still an open question** — the counter-arg for B is real when
upstream publishes are bursty and you need a single deserialize
pass to keep up.

### Q4. Placement axis — did it actually collapse?

R2 claims placement dissolves into Stage-internal choice (§R2.4).
But:

- A Stage with an HTTP client CAN fail in ways a pure-Rust Stage
  cannot (network, DNS, TLS).
- A Stage-that-is-really-a-sidecar CAN have a non-trivial startup
  latency (the sidecar's own launch).
- These matter for observability and for operator intent.

**Is "the Stage secretly talks to a service" a primitive concern
again, just differently named?**

One answer: no — `Stage::init()` is the escape hatch. A Stage with
a remote dependency uses `init()` to probe the dependency and
returns `StageReadiness::FailedInit` if the remote is unreachable.
The pipeline runtime sees one state (`LoadingModel` or
`FailedInit`) and doesn't care *why*.

Another answer: the Explorer's NodeViewer / SensorViewer / pipeline
introspection UI wants to know "what are my external deps" — Stage
with a remote WHATEVER is materially different from a pure Stage
for debugging purposes. This wants an optional
`Stage::external_deps() -> &[DepDescriptor]` — descriptive only, no
lifecycle semantics.

**Lean: add `external_deps()` as an optional descriptor, keep
placement collapsed.** The descriptor is a forward slot for the
Explorer's "what is this stage connected to" panel; it is not a
primitive differentiator.

### Q5. Sink ordering guarantees

R2 says "exactly one Sink per pipeline." What if a pipeline wants
to publish two correlated products from a single inference — e.g.
whisper's transcript text AND the per-segment confidence as a
sibling path?

**Options:**

- **Split into two pipelines** — same Sources, same Stage chain,
  two Sinks. Each pipeline is a `N-Sources → Stages → 1 Sink`
  shape. Stages are shared (could be deduplicated by the runtime
  or simply re-instantiated).
- **Allow Sink fan-out** — relax "exactly one Sink" to "one Sink
  per emission family" where the Sink accepts a tuple and
  publishes each element to a declared sibling path.
- **Keep 1 Sink and combine the payload** — whisper emits
  `{ text, confidence }` in one envelope, consumers split
  client-side.

**Lean: keep one-Sink-per-pipeline, split into two pipelines when
products diverge.** The cost is duplicated Stage wiring; the
benefit is the primitive stays clean. Combine-the-payload is
the fallback for small cases.

**Still open** — worth revisiting when sensor #2 materializes a
multi-product pipeline. Camera (frame + detections) would force
this.

### R2.7 Concrete impact on `clawft-service-whisper`

**Position: this is a SMALL REFACTOR, not a redesign.** Defending
that position:

What already matches R2:

- `windower.rs` is already a pure state machine with no substrate
  knowledge. **It is already a `Stage<PcmChunk, PcmWindow>` in
  spirit.** Moving it to R2 is renaming the trait implementation;
  no code change to the module itself.
- `client.rs` has no substrate knowledge. It is the "talks to
  whisper.cpp" half of a `Stage<PcmWindow, Transcript>`. Wrapping
  it in a Stage trait is a thin adapter.
- `wav.rs` is pure data; consumed by the inference Stage, not a
  primitive.

What needs to move:

- **`service.rs::run_pipeline`** — the ~100-line tokio::select! —
  becomes three thin pieces:
  1. A `PcmChunkSource` (20-ish lines: subscribe, decode_update_line,
     decode_pcm_chunk, forward on mpsc).
  2. A `WhisperInferenceStage` wrapping `WhisperClient` with init()
     → `wait_for_healthy`, run() → `transcribe` in a loop, managing
     the one-in-flight semaphore (this is the actual backpressure
     boundary for the Stage — the Sink's `BufferPolicy::DropOldest`
     handles the pipeline-level concern separately).
  3. A `TranscriptSink` (30-ish lines: take a Transcript, build the
     publish payload, call `substrate.publish` with the daemon's
     signing identity).
- **`SUBSTRATE_PCM_INPUT_PATH` / `SUBSTRATE_TRANSCRIPT_OUTPUT_PATH`
  constants** in `lib.rs` — both move. Input becomes config-driven
  (it's a function of the ESP32's node-id, known at runtime not
  compile time). Output likewise depends on the daemon's node-id.
- **The one-in-flight semaphore** currently held implicitly by
  `WhisperClient` through the service's `in_flight: Option<JoinHandle>`
  pattern — becomes explicit Stage backpressure.

What stays the same:

- The tests (`end_to_end_with_mocked_whisper`,
  `service_survives_whisper_down_at_start`,
  `drops_oldest_window_when_inference_slow`). They test publish-to-
  publish behaviour; under R2 they test `Source → Stage → Sink`
  wiring, which is the same observable contract. Only the
  construction boilerplate in each test changes.

**Time estimate:** a developer who knows this code well can turn
it into R2 shape in one focused session. The big conceptual moves
(splitting subscribe from pure from publish, identity into the
Sink) are shallow in the file structure — they just name boundaries
the code already has.

The thing that makes this cheap is D1 + D2 landed BEFORE any second
sensor shipped. Refactoring one pipeline to R2 establishes the
pattern; sensor #2 is written to R2 from day one.

---

*R2 status: proposal. When the pipeline-primitive ADR lands, it
consumes R1 + R2 and commits to the R2 shape with the open
questions in §R2.6 explicitly addressed. Until then this section
is the working record, and R1 above is kept for the history of how
we got here.*

---

## R3 revision — tier split and governance amendment

**Added:** 2026-04-24.
**Context:** R2 said "every substrate path lives under `substrate/<node-id>/`."
That was too rigid. Mesh-canonical facts (transcripts, fusion outputs,
chain head, consensus cluster membership) need to outlive node
failover — the path can't change because the producer changed. R3 splits
the rule into two tiers and lays in the governance shape that follows.

### R3.0 The two-tier path rule

Two distinct kinds of paths, both signed by a node, but scoped
differently:

- **Node-private** — `substrate/<node-id>/...`. Facts the node owns
  exclusively. Raw sensors, own health, own meta, own kernel state, own
  chain replica, own cluster view. Only that node may write here. Gate
  rule: strict prefix match (this is exactly what `publish_gated`
  enforces today).
- **Mesh-canonical** — `substrate/_derived/...`. Facts the mesh owns
  collectively. The leading underscore marks "not a node id" cleanly —
  no real node-id (hex pubkey fingerprint) ever collides with a word
  starting `_`. Any eligible kernel-class node may write here, subject
  to the pipeline's coordination contract. Gate rule: capability +
  eligibility (see §R3.3).

Both tiers preserve attribution — every write is signed by some node,
and the audit envelope captures who. The tiers differ on path
*scoping*, not on whether the writer is identified.

### R3.1 Subsystem placement under the two tiers

| Subsystem | Tier | Path | Notes |
| --- | --- | --- | --- |
| Raw sensors | node-private | `substrate/<node>/sensor/...` | Only the source node writes |
| Health (raw counters) | node-private | `substrate/<node>/health/...` | Per-node self-report |
| Meta (label, hardware, capabilities) | node-private | `substrate/<node>/meta/...` | Per-node identity card |
| Kernel state | node-private | `substrate/<node>/kernel/...` | Each kernel-class node has its own |
| Chain replica | node-private | `substrate/<node>/chain/replica` | Each node's local view |
| Cluster view | node-private | `substrate/<node>/cluster/view` | Each node's peer list as it sees it |
| Chain head (canonical tip) | mesh-canonical | `substrate/_derived/chain/head` | One ledger, one tip |
| Cluster membership (consensus) | mesh-canonical | `substrate/_derived/cluster/members` | The mesh's agreed peer list |
| Derived outputs (transcripts, fusion) | mesh-canonical | `substrate/_derived/<kind>/<source-attribution>/...` | Whoever currently runs the pipeline writes |

Resolves the earlier "where does chain live" ambiguity: replica is
node-private, head is mesh-canonical. Same shape for cluster.

### R3.2 Whisper transcript — final path

Under R2 we said
`substrate/<daemon-node>/derived/transcript/<esp32>/mic`. Wrong. That
ties the path to the producer, so a leader handoff would break
subscribers.

Final shape:

```text
substrate/_derived/transcript/<esp32-node-id>/mic
```

- Source node embedded in the path as attribution (this transcript is
  derived from THAT mic).
- Producer node signs the publish (audit trail says who actually did
  it).
- Path is stable across leader handoff — if the daemon dies and a Pi
  takes over whisper, subscribers see continuous output at the same
  path.

### R3.3 Sink gate splits by tier

The Sink primitive's gate becomes tier-aware:

```rust
fn may_publish(&self, ctx: &PublishContext) -> Result<(), GateDenied> {
    match self.target_tier() {
        Tier::NodePrivate => {
            // Existing publish_gated rule.
            require_prefix_match(ctx.path, ctx.node_id)
        }
        Tier::MeshCanonical => {
            // Capability + eligibility.
            require_grant(ctx.node_id, self.pipeline_id())?;
            require_elected(ctx.node_id, self.pipeline_id())
        }
    }
}
```

Both branches still verify the node signature; they differ only on
what path-level checks apply.

### R3.4 Sink primitive grows three fields

Up from R2's shape:

- `pipeline_id: PipelineId` — opaque identifier the mesh uses to issue
  grants and resolve elections. Format: derived hash of the pipeline's
  declared inputs + outputs + version, so the same logical pipeline
  has a stable id across nodes.
- `process_id: ProcessId` — local-to-node label identifying which
  process inside the node is producing this output. For the daemon
  today: a single value per service (e.g. `"clawft-service-whisper"`).
  For future WASM apps: per-app instance.
- `target_tier: Tier` — declared at construction; selects the gate
  branch above. A Sink can't switch tiers at runtime.

### R3.5 Q1 (federation) resolved — election

Under N=1 (today): trivially elected, no coordination code runs.

Under N>1: a coordination layer (RAFT-lite, lease-based, or
first-claim lock — TBD per pipeline class) picks one eligible node as
the active producer for a given `pipeline_id`. The Sink primitive
shape doesn't change between N=1 and N>1; only the eligibility-check
implementation changes. Substrate path is stable across leader handoff
(§R3.2).

Closes R2.6 Q1.

### R3.6 Governance — `_derived/` requires a separate permission

Writing to `_derived/` is not a free-by-default consequence of holding
a node key. It requires an explicit `DerivedWriteGrant`:

```rust
struct DerivedWriteGrant {
    node_id: NodeId,
    pipeline_id: PipelineId,
    output_pattern: PathPattern,  // e.g. "_derived/transcript/*"
    issued_at: DateTime<Utc>,
    revoked: bool,
}
```

Grants are:

- **Issued at pipeline registration.** Defining a pipeline (via
  Workshop spec or service manifest) declares its outputs and registers
  the needed grants for nodes that will run it.
- **Bounded to a path pattern.** A whisper grant covers
  `_derived/transcript/*`, not `_derived/**`. Compromising one
  pipeline doesn't grant write access to all mesh-canonical paths.
- **Revocable.** Removed when a pipeline is decommissioned or when a
  node is ejected from the mesh.
- **Per (node_id, pipeline_id) pair.** Each kernel-class node that may
  potentially produce the pipeline holds its own grant; coordination
  picks which one is currently active.

Naturally extends `clawft-app/src/manifest.rs::Permission` — that enum
already declares per-app capabilities (Mic, etc.). A new variant
`Permission::DerivedWrite { pipeline_id, output_pattern }` fits the
existing shape and the existing capability-check seam.

### R3.7 Audit envelope — process-level attribution

Every write to `_derived/` carries:

```text
(node_id, process_id, pipeline_id, ts, signature)
```

The signature covers the whole envelope (path + value + node_id +
process_id + pipeline_id + ts), so the node key attests "process P on
me did this." Tamper-evident — anyone without the node's private key
cannot forge any field, including process_id.

Both the substrate fan-out line and the chain audit log record the
full envelope. Subscribers see (`node_id`, `pipeline_id`) for routing;
auditors see all five fields for forensics.

Node-private writes carry the same envelope, but `pipeline_id` is
optional (a raw sensor publish doesn't have one).

### R3.8 Attest vs. authenticate — chosen: attest, this phase

**Attest (this phase):** `process_id` is a label. The node key signs
the whole envelope, so the binding "this process wrote this" is only
as strong as the node's key custody. If the node key is compromised,
an attacker can lie about which process wrote what. Audit value is
real (any honest read sees the truth) but defense-in-depth between
processes on one node is not.

**Authenticate (future):** `process_id` is a separate identity. Each
long-running process holds its own ed25519 keypair; the node key
*delegates* to process keys (signed delegation certificates). Each
write is signed by the process key directly; the node key only signs
delegations. Compromising one process key does not let it impersonate
other processes on the same node.

The future upgrade does not change the envelope shape (process_id
stays a string field); it changes the signature scheme. Becomes
load-bearing when WASM apps share a daemon — without per-app keys, a
misbehaving app could attribute its writes to another app on the same
daemon.

For MVP: attest. Note left here for the future authenticate upgrade.

### R3.9 New open questions created by the tier split

- **Q1 — Grants registry location.** Does the mesh's
  `DerivedWriteGrant` set live in the kernel (sibling to
  `AgentRegistry` and `NodeRegistry`) or in a new
  `governance::GrantsRegistry` crate? Probably the latter once
  governance has more than one type of grant; for one type, kernel is
  fine.
- **Q2 — Pipeline-id format.** Hash of (declared inputs, outputs,
  pipeline version)? Manifest-author-supplied opaque string? Stable
  across reformat-of-pipeline-spec or not? Affects how grants survive
  pipeline-spec edits.
- **Q3 — `_derived/` sub-namespacing convention.** Today proposed:
  `_derived/<kind>/<source-attribution>/...`. Alternative:
  `_derived/<pipeline-id>/...` (each pipeline gets its own subtree).
  The second is more uniform but harder to discover ("what is this
  pipeline's output?" requires reading metadata). The first is easier
  to read but leaves room for collisions across pipelines that
  produce similar outputs.
- **Q4 — Revoked-grant cleanup.** When a grant is revoked mid-publish,
  what happens to in-flight writes? Reject immediately? Drain? Both
  positions are defensible; affects the Sink's failure modes.

### R3.10 Diff summary — what changed from R2

- R2 said "all substrate paths under `substrate/<node-id>/`" —
  corrected to two-tier rule (R3.0).
- R2.6 Q1 (redundancy vs. election) — resolved in favor of election
  (R3.5).
- R2 Sink shape (`{ id, output_topic, sensitivity, cadence,
  buffer_policy, sign }`) gains `pipeline_id`, `process_id`,
  `target_tier` (R3.4).
- R2 Sink gate (single rule) splits by tier (R3.3).
- New: governance for `_derived/` (R3.6), audit envelope (R3.7),
  attest-vs-authenticate (R3.8).
- The transcript path final form (R3.2) supersedes both R1's
  `substrate/derived/transcript/mic` and R2's
  `substrate/<daemon-node>/derived/transcript/<esp32>/mic`.

---

*R3 status: proposal, layered on R2. The pipeline-primitive ADR will
consume R1 + R2 + R3 once the governance crate's grant-registry shape
is committed. R3.9 questions are inputs to that.*
