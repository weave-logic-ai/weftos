# ADR-053: Voice STT canonical path — substrate-side `clawft-service-whisper`

- **Status**: Accepted (2026-04-29)
- **Supersedes**: open question Q-VOICE-1 in `.planning/sparc/voice/decisions.md`
- **Closes**: WEFT-205
- **Spawns**: WEFT-NEW (substrate STT → agent conversation / command input wiring)

## Context

`.planning/reviews/0.7.0-release-gate/10-voice.md` flagged that two STT
paths coexist with no decision documented:

1. **Substrate-side `clawft-service-whisper` + `clawft-service-classify`**
   — a real, working HTTP daemon running on a separate node. Sensor
   nodes capture PCM, push it through the substrate, and the whisper
   service returns transcripts. Today's deployment uses this path.
2. **In-process `sherpa-rs`** — a planned in-binary STT/TTS engine that
   would let the daemon transcribe locally. Crates under
   `clawft-plugin/voice/*` are scaffolded but `sherpa-rs` and `cpal`
   were never added to `Cargo.toml`. No real implementation exists.

The audit listed five P0 voice security controls (SC-1, 4, 7, 9, 10) all
blocked on this canonical-path decision.

## Decision

**The substrate-side `clawft-service-whisper` (and
`clawft-service-classify`) is the canonical voice STT path for WeftOS
0.7.0 and beyond.**

The library remains structured so additional STT backends can be
plugged in (the `SttBackend` trait at
`crates/clawft-service-whisper/src/lib.rs` allows it), but
**substrate-side whisper is the only blessed implementation for 0.7.0**.
In-process sherpa-rs / cpal-on-the-daemon-host is **not in scope** for
0.7.0 and is deferred to 0.8.x or beyond as a research item, not a
ship-gate.

### What "canonical" means here

- Sensor capture: `clawft-sensor-pipeline` runs on a sensor node,
  produces PCM frames over the substrate.
- STT: `clawft-service-whisper` runs on its own substrate node (today,
  the same node the user runs `weaver` on; in multi-node setups it can
  live anywhere reachable). Receives PCM, returns transcripts.
- Classification (optional): `clawft-service-classify` for wake-word /
  intent.
- The voice channel adapter (M3 WEFT-164, `clawft-channels::voice`)
  is a thin client of these substrate services.

### What this decision deliberately does NOT change

- The `SttBackend` trait stays in place. If a future contributor wants
  to add an in-process backend (sherpa-rs, whisper.cpp linked into the
  binary, etc.), the library accepts it — the only thing this ADR rules
  out is *0.7.0 shipping that path as the default*.
- A future ADR can re-open this if the deployment story changes (e.g.
  embedded targets where substrate-over-network is prohibitive).

## Consequences

### What this unblocks

- **WEFT-207 / SC-1 (mic privacy indicator)**: implement against the
  sensor node's capture surface. The indicator lives wherever the
  sensor pipeline runs, not on the daemon host.
- **WEFT-208 / SC-4 (voice permission flags)**: gate voice-triggered
  tool execution by Level 0/1/2 at the *agent* layer (when transcripts
  reach the agent loop), independent of which STT backend produced
  them.
- **WEFT-209 / SC-7 (model integrity)**: the manifest + Ed25519 verify
  applies to the whisper model files on the substrate node, not the
  daemon binary.
- **WEFT-210 / SC-9 (voice command audit logging)**: log per
  transcription at the agent boundary, regardless of STT origin.
- **WEFT-211 / SC-10 (plugin voice capability)**: WASM plugin grants
  for "may consume voice transcripts" gate at the same boundary.

### What this leaves open (and where it goes)

The audit's underlying complaint was not just "which STT backend" but
**"there is no path to bring sensor→PCM→STT output into an agent
conversation or use it as input for commands"**. The substrate
infrastructure works end-to-end (sensor publishes PCM, whisper service
publishes transcripts) but there's no consumer in `weft` that:

1. Subscribes to the transcript topic.
2. Pushes transcripts as `agent.chat` inbound messages, OR
3. Routes them as commands (e.g. transcripts matching `weft <verb>`
   patterns).

This is filed as **WEFT-NEW** (0.7.x release-gate-blocker): `ws10:
voice — wire substrate STT output into agent conversation + command
input`. Without this, voice ships as a "transcripts-to-nowhere" feature
— substrate produces them, nothing consumes them. The 5 P0 SCs above
attach to *this* boundary; they're meaningless on a wire that no agent
listens to.

### Library/code implications

- No code changes to `clawft-service-whisper` or
  `clawft-sensor-pipeline` — they're already canonical.
- The `clawft-plugin/voice/*` scaffolds (in-process placeholder) become
  documented-as-deferred. Don't delete them; they're the SttBackend
  trait's other implementor and serve as the "extension point" example.
  Mark with `// DEFERRED: in-process backend, see ADR-053. Implementor
  should treat substrate-side whisper as the default.` at the top of
  each file.
- `Cargo.toml` does NOT gain `sherpa-rs` or `cpal` as workspace deps in
  0.7.0. The `clawft-channels::voice` adapter (M3 WEFT-164) keeps cpal
  behind `voice-real-audio` as opt-in.

## Validation

- WEFT-NEW (the wiring work) covers the "transcripts reach the agent"
  acceptance test: speak into the sensor, see the transcript appear in
  the agent's conversation history, the agent acts on it.
- The 5 P0 SCs (WEFT-207..211) are then implementable against a
  defined boundary.

## Decision log

- 2026-04-28: audit (`.planning/reviews/0.7.0-release-gate/10-voice.md`)
  surfaces the unresolved canonical-path question, files WEFT-205.
- 2026-04-29: discussion confirms substrate path is the canonical
  shipping solution; in-process is research, deferred. ADR drafted +
  accepted. WEFT-205 closed; new wiring ticket filed.
