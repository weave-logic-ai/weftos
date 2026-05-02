---
title: Voice (W-VOICE)
slug: voice
workstream_id: "10"
release: 0.7.0
audit_type: comprehensive
status: stub-with-cloud-edges
last_updated: 2026-04-28
---

# Voice (W-VOICE)

## General Description

W-VOICE covers the entire voice I/O pipeline for ClawFT: microphone capture
(cpal), voice activity detection (Silero VAD via sherpa-rs), speech-to-text
(sherpa-rs streaming + cloud Whisper fallback), text-to-speech (sherpa-rs
piper + cloud OpenAI/ElevenLabs fallback), wake-word detection (rustpotter),
echo cancellation, noise suppression, audio quality metrics, a `VoiceChannel`
ChannelAdapter, a `TalkModeController`, per-agent `VoicePersonality`, voice
command shortcuts with Levenshtein fuzzy matching, transcript logging, and
the corresponding UI surface (status bar, talk overlay, settings, push-to-
talk). The plan splits work into three phases — VS1 core pipeline (weeks
1-3), VS2 wake word + platform integration (weeks 4-6), VS3 advanced
features + cloud + UI (weeks 7-9) — with a Week-0 VP gate (`voice-proto/`)
that was supposed to validate sherpa-rs / cpal / model-hosting / platform
audio / AEC feasibility before sprint VS1.1 began.

There is also a parallel, **separate** voice-adjacent track on the kernel
side: `clawft-service-whisper` and `clawft-service-classify` are HTTP-based
substrate sensor pipelines (mic PCM → windower → whisper.cpp HTTP server →
transcript published to `substrate/_derived/transcript/<src>/mic`). That
track is **not** part of W-VOICE per the SPARC plan; it is a Phase-2 Track-4
sensor-ingestion pipeline-primitive spike (see
`.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md`). It is included in this
audit because every search the orchestrator pointed at — "voice / STT / TTS
/ Whisper / classify" — surfaces both surfaces, and a release-gate audit
must be honest that `clawft-plugin/voice/*` and
`clawft-service-{whisper,classify}` are two unrelated implementations of the
same concept that have not been reconciled.

## Status & Timeline

| Phase | Plan | Implementation Reality | Tests |
|-------|------|------------------------|-------|
| **VP** (week 0) | Standalone `voice-proto/` validates sherpa-rs + cpal + model hosting + AEC + platform audio on Linux/macOS/Windows | **NOT DONE.** No `voice-proto/` directory exists. The gate was skipped. All VS1.1 stub work proceeded without VP exit criteria being met. | n/a |
| **VS1.1** (week 1) | Audio foundation — cpal capture/playback, Silero VAD, model download, VoiceConfig | Module skeleton + stubs only. Cargo deps for sherpa-rs / cpal **not added**. `AudioCapture::start` just sets `active=true`. SHA-256 hashes are `PLACEHOLDER_SHA256_*`. | 73 plugin tests on stubs |
| **VS1.2** (week 2) | Streaming STT/TTS, voice_listen + voice_speak tools, CLI test commands | Tool stubs return `status: "stub_not_implemented"`. CLI commands print stub messages. | 7 new tool tests |
| **VS1.3** (week 3) | VoiceChannel adapter, Talk Mode, interruption detection, WS voice events | `VoiceChannel::start()` waits for cancellation. `send()` logs but doesn't synthesize. `TalkModeController::run()` is a thin pass-through. WS event types defined but no real broadcast wired. | 17 new voice tests, 90 total |
| **VS2.1** (week 4) | rustpotter wake word, "Hey Weft" model, CPU budget, custom training | Stub `WakeWordDetector` always returns `false` from `process_frame`. No rustpotter dep. No model file. No CPU enforcement. CLI `weft voice wake` waits for Ctrl+C. | 14 new wake tests |
| **VS2.2** (week 5) | Software AEC, RNNoise, multi-language STT, audio quality metrics | EchoCanceller has a circular reference buffer but the `process()` impl is a passthrough. NoiseSuppressor tracks RMS noise floor via EMA but doesn't filter. AudioMetrics (RMS / peak / clipping / SNR clamped to [-20,80]) is **real and works** — pure computation, no external deps. Multi-language STT is **only** `language: String` field in config. | 18 new tests |
| **VS2.3** (week 6) | systemd / launchd / Windows daemons, mic permission, privacy indicator, Discord voice bridge, platform test suite | `scripts/clawft-wake.service` (systemd) and `scripts/com.clawft.wake.plist` (launchd) exist, embedded into the binary via `include_str!`. `weft voice install-service` auto-detects platform. **Windows uses manual instructions only**. No mic permission code. No privacy indicator. No Discord voice bridge. No platform CI test suite. | n/a (DevOps files) |
| **VS3.1** (week 7) | UI status bar, talk overlay, waveform, settings, push-to-talk, partial-transcription WS, TTS word highlighting, Tauri mic | All five UI components exist (`clawft-ui/src/components/voice/{push-to-talk,settings,status-bar,talk-overlay}.tsx`, `voice-store.ts`, `voice.tsx` route). `clawft-ui/src/lib/audio.ts` (357 LoC) **does** call `navigator.mediaDevices.getUserMedia` — the browser side is the most "real" surface in the entire workstream. No partial-transcription stream, no TTS word-highlighting, no Tauri integration. MSW mocks back the API. | TS check + Vite build pass |
| **VS3.2** (week 8) | OpenAI Whisper STT fallback, OpenAI/ElevenLabs TTS fallback, fallback chain, speaker diarization, transcript logging | **Cloud providers are real wire-level implementations** — `WhisperSttProvider` actually POSTs multipart to `api.openai.com/v1/audio/transcriptions`, `OpenAiTtsProvider` and `ElevenLabsTtsProvider` actually call their HTTP endpoints. `SttFallbackChain` (threshold 0.60) and `TtsFallbackChain` are real Rust glue. **But the local engines they fall back from are still stubs**, so the chain is half-real. Speaker diarization not implemented. `TranscriptLogger` (JSONL append) is real and works. | 27 new tests |
| **VS3.3** (week 9) | Per-agent voice personality, voice commands, audio file I/O tools, latency/WER benchmarks, CPU profiling, voice permissions, E2E tests | `VoicePersonality` config + validation is real. `VoiceCommandRegistry` (prefix + Levenshtein-2 fuzzy) is real and tested. `audio_transcribe` / `audio_synthesize` tools exist as stubs. **Latency benchmarks, WER benchmarks, CPU profiling, voice permission integration, and E2E tests are NOT done.** | 14 commands tests, 8 personality tests, 12 audio-file tests |

**Plan total**: 75 items across VP + VS1-VS3 (21 P0, 33 P1, 21 P2).
**Done in spirit**: ~40 (config + stub + cloud-API + UI scaffold + service files).
**Done end-to-end**: 0. There is no path from microphone-PCM to spoken-TTS-response anywhere in the codebase. The substrate-side `clawft-service-whisper` does have an end-to-end path, but it is a separate workstream and is itself characterized as a "Phase 2 Track 4 spike".

Per project memory: "VS1 spec complete, VS2/VS3 being written" is **inaccurate** as of the file system state on 2026-04-28 — VS1, VS2, and VS3 spec docs all exist (`01-phase-VS1-audio-foundation.md` 2617 lines, `02-phase-VS2-wake-word-platform.md` 2867 lines, `03-phase-VS3-advanced-ui-integration.md` 2653 lines). What is incomplete is the **implementation**, not the spec.

## Released Features

For 0.7.0 the following voice surfaces compile, run, and pass tests; none of them produce real audio I/O:

- **Feature flags**: `voice` (umbrella, implies `voice-vad` + `voice-wake` + tokio), `voice-stt`, `voice-tts`, `voice-vad`, `voice-wake` — wired across `clawft-plugin`, `clawft-tools`, `clawft-cli` (`scripts/build.sh native --features voice` is the documented build line).
- **Config types**: `VoiceConfig`, `AudioConfig`, `SttConfig`, `TtsConfig`, `VadConfig`, `WakeConfig`, `CloudFallbackConfig`, `VoicePersonality` — full serde, snake-case + camelCase aliases, defaults (16 kHz mono, 1.5 s silence timeout, "hey weft" phrase, threshold 0.5).
- **Voice plugin module**: capture / playback / vad / stt / tts / wake / wake_daemon / channel / talk_mode / events / models / commands / cloud_stt / cloud_tts / fallback / transcript_log / echo / noise / quality (21 modules, 4189 LoC).
- **Cloud STT/TTS** (real HTTP): `WhisperSttProvider` (OpenAI Whisper), `OpenAiTtsProvider` (6 voices alloy/echo/fable/onyx/nova/shimmer), `ElevenLabsTtsProvider` (4 voices). All multipart-correct, language hint supported.
- **Fallback chains**: `SttFallbackChain` (local-first, cloud on err or confidence < 0.60, takes higher-confidence winner) and `TtsFallbackChain` (local-first, cloud on err). Source attribution via `SttSource::{Local, Cloud(name)}`.
- **Audio quality metrics**: RMS, peak, clipping detection (threshold 0.99), SNR estimation clamped [-20, 80] dB. Pure compute, no platform deps, compiles on WASM.
- **Voice command registry**: `VoiceCommandRegistry::with_defaults()` ships three commands (`stop listening`, `what time is it`, `list files`); custom registries supported. Levenshtein DP, edit-distance threshold 2. Confirmation flag per command.
- **Transcript logger**: append-only JSONL at `<workspace>/.clawft/transcripts/<session>.jsonl` with `TranscriptEntry` (timestamp, speaker, text, source, confidence, language, duration_ms).
- **Per-agent voice personality**: `HashMap<String, VoicePersonality>` on `VoiceConfig`. Validation enforces speed ∈ [0.5, 2.0], pitch ∈ [-1.0, 1.0], non-empty `voice_id`.
- **CLI**: `weft voice setup | test-mic | test-speak | talk | wake | install-service` (all behind `voice` feature). `install-service` auto-detects Linux/macOS, prints manual steps on Windows.
- **Tools registered**: `voice_listen`, `voice_speak`, `audio_transcribe`, `audio_synthesize` (in `clawft-tools::register_all` when `voice` feature is on). All currently return stub status strings.
- **Platform service files**: `scripts/clawft-wake.service`, `scripts/com.clawft.wake.plist` — embedded via `include_str!`, copied to `~/.config/systemd/user/` or `~/Library/LaunchAgents/` by the install command.
- **UI components** (`clawft-ui`): `VoiceStatusBar` (mic-state badge, color-coded, pulse anim), `TalkModeOverlay` (full-screen, CSS waveform, transcript + response), `VoiceSettings` (toggles + 7-language dropdown), `PushToTalk` (hold-to-record circular button), `voice-store.ts` (zustand: state / settings / talk mode / transcript). `audio.ts` does real `getUserMedia` for mic-level metering. MSW mocks back `/api/voice/{status,settings,test-mic,test-speaker}`.
- **WebSocket scaffolding**: `VoiceWsEvent` type defined; `VoiceChannel` reports status changes via mpsc; UI subscribes to `voice:status`. The actual WS broadcast wire is not connected to a real backend handler.
- **Substrate-side STT** (separate track but voice-relevant): `clawft-service-whisper` is a real, working substrate pipeline subscribing to `substrate/sensor/mic/pcm_chunk`, windowing PCM into 1-3 s frames, posting multipart to `whisper.cpp` HTTP at `localhost:8080`, publishing transcripts to `substrate/_derived/transcript/<src>/mic` with retry, drop-oldest backpressure, and dual-publish migration window. `clawft-service-classify` mirrors the shape with an `EnergyClassifier` (RMS dBFS, default -45 dB threshold) and a `ClassifierBackend` trait sized to swap in a llama.cpp classifier emitting `{class: "speech"|"music"|"noise"|...}`.

## What's Left — Total Depth

### TODOs / FIXMEs in voice source

`grep -rEn 'TODO|FIXME|HACK|todo!|unimplemented'` over
`crates/clawft-service-whisper`, `crates/clawft-service-classify`,
`crates/clawft-plugin/src/voice`, and `clawft-ui/src/components/voice`
returns **zero hits**. There are no inline TODO markers; the deferred work
is encoded only in module-level doc comments and in the `.planning/`
documents. The notable doc-comment markers are:

- `crates/clawft-plugin/src/voice/capture.rs:28` — "Currently a stub -- real cpal integration will be added after VP".
- `crates/clawft-plugin/src/voice/playback.rs:23` — same wording for cpal output.
- `crates/clawft-plugin/src/voice/vad.rs:17` — "stub -- real sherpa-rs integration after VP".
- `crates/clawft-plugin/src/voice/stt.rs:16` — same.
- `crates/clawft-plugin/src/voice/tts.rs:42` — same.
- `crates/clawft-plugin/src/voice/echo.rs:88` — `// STUB: Real AEC would use NLMS or frequency-domain adaptive filter here.`
- `crates/clawft-plugin/src/voice/noise.rs:68` — `// STUB: Real noise suppression would use spectral subtraction or RNNoise`.
- `crates/clawft-plugin/src/voice/wake.rs:7` — "stub implementation -- real rustpotter integration is deferred until after VP validation".
- `crates/clawft-plugin/src/voice/wake_daemon.rs:7` — same.
- `crates/clawft-plugin/src/voice/channel.rs:6` — "Currently a stub implementation -- real audio processing deferred until sherpa-rs/cpal VP completes".
- `crates/clawft-plugin/src/voice/models.rs:54,66,78` — `sha256: "PLACEHOLDER_SHA256_STT".into()` (and `_TTS`, `_VAD`).
- `crates/clawft-service-whisper/src/service.rs:80` — `// REMOVE AFTER PHASE 4: dual-publish for migration`.

### Deferred items (from `.planning/` and dev notes)

**VP gate (week 0) — entirely deferred**:
- VP1 audio pipeline prototype binary in `voice-proto/` (mic → VAD → STT → text). Directory does not exist.
- VP2 model hosting + GitHub Releases + HuggingFace mirror + SHA-256 manifest. SHA-256 entries in `models.rs` are placeholders.
- VP3 feature flag design — partially done (flags exist in Cargo.toml).
- VP4 platform audio testing on Linux PipeWire/PulseAudio, macOS CoreAudio, Windows WASAPI, WSL2.
- VP5 echo cancellation feasibility study (loopback subtraction vs WebRTC AEC vs hardware AEC).

**VS1 — core pipeline real implementations**:
- sherpa-rs and cpal **not added** to `clawft-plugin/Cargo.toml` (only async-trait, serde, thiserror, tokio-util, tokio, chrono, tracing, semver, reqwest are present).
- `AudioCapture` real cpal stream; capture-thread → ring buffer → consumer plumbing.
- `AudioPlayback` real cpal output; back-pressure model.
- `VoiceActivityDetector` real Silero V5 ONNX inference.
- `SpeechToText::process` and `finalize` real sherpa-rs streaming recognizer; partial-result callback wiring.
- `TextToSpeech::synthesize` real piper synthesis; streaming-playback-before-complete.
- `TtsAbortHandle` exists as type but synthesize() never checks it (always returns immediately with empty samples).
- Model download manager real HTTP fetch + SHA-256 verify; `is_cached` only checks file existence today.
- VoiceChannel real audio loop in `start()` (today: just awaits cancel).
- VoiceChannel `send()` real TTS synthesis + playback (today: logs and toggles status).
- Talk Mode interruption detection (stop TTS when user starts speaking).
- WS `voice:status` real broadcast — `VoiceWsEvent` type exists; no service emits it.
- `voice_listen` and `voice_speak` tools wire to real STT/TTS (today: return `"status": "stub_not_implemented"`).
- `weft voice setup` real model download (today: prints stub).

**VS2 — wake word + platform**:
- rustpotter dependency in Cargo.toml.
- "Hey Weft" model file under `models/voice/wake/hey-weft.rpw` (referenced in default config; file does not exist in repo).
- `WakeWordDetector::process_frame` real MFCC+DTW/neural matching (today: returns `false`).
- Wake → VAD pipeline activation glue.
- `weft voice train-wake "hey weft"` guided training command (today: not even an enum variant).
- Custom wake words at `~/.clawft/models/wake/`.
- CPU budget auto-throttle (< 2 % wake daemon).
- Software AEC real impl in `EchoCanceller::process` (today: passthrough).
- Noise suppression real spectral subtraction or RNNoise (today: only updates EMA, returns input unchanged).
- Adaptive silence timeout (learn user speech patterns).
- Multi-language STT — language auto-detection routing (config field exists; no logic consumes it).
- Voice selection — multiple TTS voices wired to per-agent config.
- Linux systemd, macOS launchd unit files exist; **Windows scheduled task is manual instructions only**.
- Microphone permission requests (macOS TCC `AVCaptureDevice.requestAccess`, Windows privacy settings).
- Privacy indicator (tray / menu-bar / terminal badge when mic active).
- PipeWire native integration (today's plan-only assumption is "cpal handles it").
- Discord voice channel bridge (`clawft-channels` voice → STT → agent → TTS → VC audio).
- Platform audio CI test suite.

**VS3 — UI + cloud + advanced**:
- UI partial-transcription streaming over WS.
- UI TTS word-highlighting during playback.
- Tauri-side native mic access (browser-side `getUserMedia` is the only real path).
- Speaker diarization (multi-speaker sherpa-rs).
- Conversation mode (multi-party sessions).
- `audio_transcribe` real WAV/MP3/OGG/WebM decode → STT pipeline (today: stub).
- `audio_synthesize` real WAV writer (today: stub).
- Latency benchmark suite (speech-end → first-response-byte).
- WER benchmark against standard English corpus.
- CPU/battery profiling: wake < 2 %, full pipeline < 10 %.
- Voice permission integration: restrict voice-triggered tool execution by Level 0/1/2 (security review §SC-4 P0).
- Playwright + audio-simulation E2E test suite.

**Security controls (from `.planning/sparc/voice/06-voice-security-review.md`)** — none of the P0 controls are implemented:
- SC-1 visual mic indicator across platforms.
- SC-1 OS-level permission prompts (macOS, Windows, Linux portal).
- SC-1 hardware mic-mute respect.
- SC-2 audio buffer zeroization (`zeroize` crate).
- SC-2 `voice.audio_retention` config option (none/session/persist).
- SC-3 cloud provider transparency log line (`Cloud fallback active: sending audio to ...`).
- SC-4 voice-specific permission flags (`voice_listen`, `voice_speak`, `wake_word`, `talk_mode`, `voice_exec_shell`, `voice_delegate`).
- SC-4 destructive-action voice confirmation gate.
- SC-6 anti-replay nonce ("Say 'confirm delta' to proceed.").
- SC-6 transcription-echo confirmation pattern.
- SC-7 Ed25519-signed model manifest verification.
- SC-7 SHA-256 verification on download (placeholder hashes today).
- SC-8 rate limiting (10 commands/min, 5 wake activations/min, post-fail cooldown).
- SC-9 voice command transcription audit logging with permission-check trail.
- SC-10 plugin manifest `capabilities: ["voice"]` gate; `voice.listen` / `voice.speak` / `voice.raw_audio` sub-permissions; per-plugin `VoiceHandle` isolation.

### Open questions

- **Two voice STT stacks coexist.** `clawft-plugin/voice/*` (sherpa-rs in-process plan, all stubbed) vs `clawft-service-whisper` (whisper.cpp HTTP service, real and substrate-wired). The SPARC voice plan does not mention the substrate pipeline; the substrate pipeline journal is explicit that whisper-rs/FFI was the original brief and was superseded by HTTP. **No ADR reconciles the two.** Which one is the canonical Talk-Mode STT for 0.7.0+? Does `VoiceChannel` consume substrate transcript events, or does it own its own sherpa-rs recognizer?
- **`voice` umbrella feature implies `voice-wake` + `voice-vad` + tokio but NOT `voice-stt` / `voice-tts`.** `clawft-plugin/Cargo.toml:20` reads `voice = ["voice-vad", "voice-wake", "dep:tokio"]`. So `--features voice` builds a pipeline with no STT and no TTS. Intentional? The dev-note in step 1 says "voice-stt and voice-tts are empty until VP" — they remain empty, but the umbrella doesn't pull them in.
- **`AudioConfig` is duplicated.** `clawft-types/src/config/voice.rs::AudioConfig` (sample_rate, chunk_size, channels, input_device, output_device) and `clawft-plugin/src/voice/capture.rs::CaptureConfig` (same fields minus `output_device`) and `…/playback.rs::PlaybackConfig` (sample_rate, channels, device_name) — three near-identical types. Which is canonical?
- **`VoiceConfig::tts.provider` defaults to `"browser"`** (Web Speech API) but `clawft-plugin/voice/tts.rs` has no browser dispatch; the only TTS providers wired are local-stub, `OpenAiTtsProvider`, and `ElevenLabsTtsProvider`. `browser` is the default and is unimplemented.
- **`CloudFallbackConfig::stt_provider`** is documented as `"whisper"` but the fallback chain takes `Box<dyn CloudSttProvider>` directly — no string-to-provider router exists. Where does config → provider instantiation happen?
- **Wake word sensitivity vs threshold.** `WakeConfig.sensitivity` (in `clawft-types`) and `WakeWordConfig.threshold` (in `clawft-plugin`) are two names for the same knob with no shared mapping.
- **Substrate paths and session correlation.** `TranscriptLogger` writes to `<workspace>/.clawft/transcripts/<session_key>.jsonl`. `clawft-service-whisper` publishes to `substrate/_derived/transcript/<source-node-id>/mic`. There is no documented join key between the two.
- **Model SHA-256 hashes are placeholders.** The model download manager will accept any payload because it only checks file existence, not the (placeholder) hash. This is a P0 security gap (SC-7) hiding behind a stub interface.
- **`VoicePersonality.greeting_prefix`** is defined but never consumed by any TTS path.
- **`VoiceCommand.confirm: bool`** — registry stores it; nothing in the codebase reads it. Confirmation flow per security §SC-4 / §SC-6 unimplemented.
- **`audio_transcribe` / `audio_synthesize` validate file extensions** (.wav/.mp3/.ogg/.webm; .wav out) but cannot actually decode/encode any of those formats.
- **`weft voice install-service` on Windows** prints manual Task Scheduler instructions. Is that the long-term plan, or is automated `schtasks` invocation pending?

### Orphaned work

- `clawft-service-whisper/examples/publish_wav.rs` — referenced from Cargo.toml; not part of any release surface and not executed by CI; useful for hand-validating the substrate pipeline. Keep or delete?
- `clawft-service-classify` exists as an end-to-end working pipeline whose only consumer today is "Explorer GUI subscribers" (per its lib doc). It is not connected to the W-VOICE pipeline, the agent loop, or any tool.
- `clawft-ui/src/lib/voice-chat.ts` (sendVoiceMessage) sends transcribed text to `sessions:voice` and waits for an agent reply over WS. It works in isolation but no UI component currently calls it; the `voice-store` only tracks state, and `talk-overlay.tsx` does not invoke `sendVoiceMessage`.
- `WakeWordEvent::Error { message }` variant — emitted nowhere in the codebase.
- `VoiceStatus::Transcribing` — defined in the enum but the stub never transitions through it (only Idle / Listening / Speaking).
- `clawft-plugin/src/voice/events.rs` defines `VoiceWsEvent` with timestamp + status; no service constructs and broadcasts it. The UI listens on `voice:status` topic that is never published to.
- `mocks/handlers.ts` provides four MSW endpoints (`/api/voice/status`, `/api/voice/settings`, `/api/voice/test-mic`, `/api/voice/test-speaker`) — these have no Rust counterpart in `clawft-services` (they don't exist as real backend routes), so the UI works only against MSW mocks, not the daemon.
- `WakeConfig.model_path: Option<String>` (clawft-types) vs `WakeWordConfig.model_path: PathBuf` (clawft-plugin) — two model-path fields with different types and no bridge.
- `EchoCanceller.feed_reference` is wired to a circular buffer that `process()` ignores. The buffer is dead code until real AEC lands.
- `NoiseSuppressor.noise_floor` is computed but never read by `process()`; the EMA is observable only through the `noise_floor()` accessor used by tests.

## Task List

### P0 (release-critical regardless of 0.7.0 scope)

1. Decide and document (ADR) the canonical voice STT path: in-process sherpa-rs (`clawft-plugin/voice/stt.rs`) vs substrate `clawft-service-whisper`. Resolve the duplication or formally split the two surfaces with named contracts.
2. Run the VP gate or formally cancel it. If cancelled, replace the doc-comment "after VP" deferrals with a real status label so the next reader doesn't assume a gate is still pending.
3. Replace `PLACEHOLDER_SHA256_*` hashes in `models.rs` with real ones, or gate `is_cached` to refuse to load a model with a placeholder hash. Today the manager will silently accept tampered models.
4. Add `sherpa-rs` and `cpal` to `clawft-plugin/Cargo.toml`, or rename the `voice` feature to `voice-stubs` so consumers know they don't get audio I/O.
5. Implement SC-1 mic privacy indicator (at minimum: terminal badge + WS event when capture is active) before any real cpal capture lands.
6. Implement SC-4 voice permission flags (`voice_listen`, `voice_speak`, `wake_word`, `talk_mode`, `voice_exec_shell`) and gate voice-triggered tool execution. Today voice transcription would route to the agent loop with full Level-2 access.
7. Resolve the `voice` umbrella → `voice-stt` / `voice-tts` feature gap (or document that `--features voice` is intentionally I/O-only).
8. Reconcile `AudioConfig` / `CaptureConfig` / `PlaybackConfig` triplet into one canonical type.

### P1 (needed for any honest "voice MVP" claim)

9. Wire `voice_listen` / `voice_speak` tools to actual STT/TTS implementations (local + cloud fallback chain).
10. Wire `weft voice setup` to real model download with SHA-256 verify and progress UI.
11. Wire `WakeWordDetector::process_frame` to rustpotter or document an alternative.
12. Implement `EchoCanceller::process` (NLMS or frequency-domain) and `NoiseSuppressor::process` (RNNoise or spectral subtraction). Currently both are deceptive passthroughs.
13. Wire `VoiceChannel::start` to real capture+VAD+STT loop and `send` to real TTS+playback.
14. Connect the WS `voice:status` topic to a real backend broadcaster (today UI listens, nothing emits).
15. Replace MSW-only `/api/voice/*` endpoints with real handlers in `clawft-services`.
16. Add Windows automated `schtasks` install path (or document the manual route as final).
17. Add interruption detection (mic VAD trips while TTS speaking → abort handle).
18. Wire `VoicePersonality` lookup in TTS dispatch (per-agent voice routing).

### P2 (advanced / nice-to-have)

19. Speaker diarization via sherpa-rs.
20. Tauri-side native mic capture.
21. Latency + WER benchmark suite.
22. CPU profiling harness with hard 2 % wake-budget enforcement.
23. Adaptive silence timeout learning.
24. UI partial-transcription streaming + TTS word highlighting.
25. Discord voice bridge (depends on stream 09 channel work).
26. Audio file `audio_transcribe` / `audio_synthesize` real codecs.
27. Confirmation flow with anti-replay nonce (SC-6).
28. Rate limiting (SC-8).
29. Voice command transcription audit logging (SC-9).
30. Ed25519 model manifest signing (SC-7).
31. Plugin `voice` capability gate (SC-10) for WASM plugins.

### Cleanup / orphans

32. Either consume `WakeWordEvent::Error` and `VoiceStatus::Transcribing`, or remove them.
33. Either call `voice-chat.ts::sendVoiceMessage` from `talk-overlay.tsx`, or delete it.
34. Either bridge `WakeConfig.model_path: Option<String>` ↔ `WakeWordConfig.model_path: PathBuf`, or remove the unused field.
35. Either consume `VoicePersonality.greeting_prefix` and `VoiceCommand.confirm`, or document them as forward-compat.
36. `clawft-service-whisper/src/service.rs:80` — drop the legacy dual-publish path once Phase-4 migration window closes.
37. Decide `clawft-service-classify` adoption: connect to W-VOICE, leave as Explorer-only sensor pipeline, or delete.

## Sources

- `crates/clawft-types/src/config/voice.rs` — VoiceConfig + AudioConfig + SttConfig + TtsConfig + VadConfig + WakeConfig + CloudFallbackConfig (262 lines).
- `crates/clawft-types/src/config/personality.rs` — VoicePersonality (172 lines).
- `crates/clawft-plugin/src/voice/{mod,config,capture,playback,vad,stt,tts,wake,wake_daemon,channel,talk_mode,events,models,commands,cloud_stt,cloud_tts,fallback,transcript_log,echo,noise,quality}.rs` — full voice plugin module (~4189 lines).
- `crates/clawft-plugin/Cargo.toml` — voice feature flags (lines 17-25).
- `crates/clawft-tools/src/{voice_listen,voice_speak,audio_transcribe,audio_synthesize}.rs` — voice-gated tools.
- `crates/clawft-cli/src/commands/voice.rs` — `weft voice {setup,test-mic,test-speak,talk,wake,install-service}`.
- `crates/clawft-service-whisper/src/{lib,client,service,wav,windower}.rs` — substrate-side whisper.cpp HTTP pipeline (~2172 lines).
- `crates/clawft-service-classify/src/{lib,classifier,service}.rs` — substrate-side energy VAD / classifier (~930 lines).
- `clawft-ui/src/components/voice/{push-to-talk,settings,status-bar,talk-overlay}.tsx` — UI components (955 lines).
- `clawft-ui/src/stores/voice-store.ts`, `clawft-ui/src/lib/{audio,voice-chat}.ts`, `clawft-ui/src/routes/voice.tsx` — UI state, getUserMedia metering, WS chat helper.
- `scripts/clawft-wake.service`, `scripts/com.clawft.wake.plist` — platform daemon units.
- `.planning/sparc/voice/00-orchestrator.md` (342 lines) — phase plan + exit criteria + risks.
- `.planning/sparc/voice/01-phase-VS1-audio-foundation.md` (2617 lines).
- `.planning/sparc/voice/02-phase-VS2-wake-word-platform.md` (2867 lines).
- `.planning/sparc/voice/03-phase-VS3-advanced-ui-integration.md` (2653 lines).
- `.planning/sparc/voice/04-voice-pre-implementation.md` (1402 lines) — VP1-VP5 gate spec.
- `.planning/sparc/voice/05-voice-tracker.md` (294 lines) — 75-item tracker, all "Not Started" status.
- `.planning/sparc/voice/06-voice-security-review.md` (509 lines) — 10 threats + 10 security controls + platform matrix.
- `.planning/development_notes/step1-vs1.1-voice-module.md` through `step7-vs3.2-vs3.3-cloud-voice-advanced.md` — per-step implementation notes (all explicit that work landed as stubs).
- `.planning/development_notes/orchestrator-log.md` — workstream cross-status; "Voice module uses stubs — real sherpa-rs/cpal deferred to VP validation".
- `.planning/sparc/00-master-plan.md` — W-VOICE integration with W-UI, W-BROWSER (lines 5-76).
- `.planning/sensors/PIPELINE-PRIMITIVE-SPIKE.md`, `PIPELINE-PRIMITIVE-JOURNAL.md` — substrate-side rationale for `clawft-service-whisper`.
- `docs/adr/` — searched for `voice|whisper|sherpa|cpal|wake|stt|tts`; only adr-008 (cloud-side), adr-016 (multi-target theming), adr-021 (CLI kernel compliance) mention voice and only in passing. **No voice-specific ADR exists.**
- `docs/handoff.md` — only voice mention is `TurnContent` enum forward-compat for voice (lines 322-325, 446-448).
- `scripts/build.sh` — `--features voice` build documentation (line 425, 469, 478).

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws10-voice` label.

- **Range**: WEFT-205 … WEFT-241 (37 items)
- **Per cycle**: 0.7.x: 9, 0.8.x: 23, 0.9.x: 5
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
