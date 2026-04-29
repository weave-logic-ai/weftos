# VP Gate Resolution

- **Status**: Closed (formally cancelled)
- **Decision date**: 2026-04-29
- **Authority**: ADR-053 (`docs/adr/adr-053-voice-stt-canonical-path.md`)
- **Plane**: closes WEFT-206

## Background

The voice planning tree (`.planning/sparc/voice/`) defines a "Validation
Prototype" gate -- five P0 pre-implementation tasks (VP1..VP5) that
were supposed to run in Week 0 before the voice sprint started:

- **VP1**: Audio pipeline prototype (mic -> VAD -> STT -> print text)
- **VP2**: Model hosting & download strategy (manifest + SHA-256)
- **VP3**: Feature flag design (voice / voice-stt / voice-tts / voice-wake)
- **VP4**: Platform audio testing (cpal across PipeWire / PulseAudio /
  CoreAudio / WASAPI / WSL2)
- **VP5**: Echo cancellation feasibility (loopback subtraction vs
  WebRTC AEC vs hardware AEC)

The gate (`04-voice-pre-implementation.md` §1) said "voice sprints
cannot begin until all VP tasks pass." None of those five tasks were
ever executed against the in-process sherpa-rs/cpal path that they
were designed to validate, because the project pivoted to a different
architecture before that work happened.

## What replaces the VP gate

ADR-053 (Accepted 2026-04-29) ratifies **substrate-side
`clawft-service-whisper` as the canonical voice STT path for 0.7.0+**.
That path:

1. Was built end-to-end during Phase 2 Track 4 (`crates/clawft-service-whisper/`).
2. Was shipped + has been running in production deployments
   (`SUBSTRATE_PCM_INPUT_PATH` -> whisper.cpp HTTP daemon ->
   `substrate/_derived/transcript/<src>/mic`).
3. Already has hermetic end-to-end tests (`service::tests::end_to_end_with_mocked_whisper`).
4. Sidesteps every VP risk-register entry: no in-process FFI build
   matrix (VP1 / VP4), no in-binary model load (VP2 still applies but
   to the substrate node, not the daemon binary), no AEC concern
   because audio capture and whisper inference run on different nodes
   (VP5).

The VP gate was a guard against shipping the in-process stack without
proof it would build and perform on target platforms. We're not
shipping the in-process stack; the guard is therefore moot.

## Decision

**The VP gate is closed by formal cancellation, not by execution.**

- VP1..VP5 are **not** going to be run.
- The five tracker rows (`05-voice-tracker.md` table at line 36+) are
  retained as historical context but marked Cancelled.
- Source-tree `// after VP` / `deferred until after VP validation`
  markers are removed (they previously implied "run VP first, then
  fill in this stub" -- with VP cancelled, the stubs are documented as
  in-process-backend placeholders deferred to 0.8.x+ instead).
- The in-process `clawft-plugin/src/voice/*` scaffolding stays in
  place per ADR-053: it remains the second `SttBackend` implementor,
  documented as deferred but kept as the extension-point example.

## What this does NOT change

- The 5 P0 voice security controls (SC-1, 4, 7, 9, 10) still need
  implementing. They attach to the substrate-side path (per ADR-053
  §Consequences). WEFT-207..211 remain open.
- A future ADR can re-open the in-process path. If it does, the VP
  exit criteria in `04-voice-pre-implementation.md` are still the
  right validation framework -- they just don't gate 0.7.0 anymore.
- Cancellation does not delete `04-voice-pre-implementation.md`. That
  file is a useful pre-flight checklist for whoever later picks up the
  in-process backend.

## Source-tree markers being removed

`grep -rn "after VP\|deferred until after VP" crates/clawft-plugin/src/voice/`
turned up seven sites:

- `capture.rs:29` (doc), `capture.rs:43` (inline)
- `playback.rs` (no marker, but parallel scaffolding)
- `stt.rs:16`, `tts.rs:42`, `vad.rs:17`
- `wake.rs:7`, `wake_daemon.rs:8`

Each is replaced with the canonical phrasing **"deferred to 0.8.x
in-process voice backend (see ADR-053)"** so future readers land on
the live decision instead of a cancelled gate.

## Plane lifecycle

- WEFT-206 closed with shipped="VP gate formally cancelled by
  ADR-053; resolution doc added; source-tree VP markers removed."
- VP1..VP5 are not separately tracked in Plane (they were
  pre-Plane-era planning rows).
