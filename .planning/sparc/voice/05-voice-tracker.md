# Sprint Tracker: Voice Development Sprint

**Project**: clawft
**Sprint**: Voice Development -- STT, TTS, VAD, VoiceChannel, Talk Mode, Voice Wake
**Source**: `.planning/voice_development.md`
**Orchestrator**: `.planning/sparc/voice/00-orchestrator.md`
**Engine**: sherpa-rs (sherpa-onnx) + rustpotter + cpal
**Created**: 2026-02-23

---

## Milestone Status

- [ ] **VS1 MVP (Week 3)**: Audio foundation (cpal capture/playback + Silero VAD + model download), STT/TTS tools (sherpa-rs streaming recognizer/synthesizer + voice_listen/voice_speak), VoiceChannel adapter, basic Talk Mode (listen -> transcribe -> think -> speak), CLI commands (weft voice setup/test-mic/test-speak/talk), WebSocket voice:status events
- [ ] **VS2 Complete (Week 6)**: Wake Word detection (rustpotter "Hey Weft" + custom training), Echo Cancellation (software AEC + noise suppression), multi-language STT, platform daemons (systemd/launchd/Windows startup), Discord voice bridge, privacy indicator, platform audio test suite
- [ ] **VS3 Complete (Week 9)**: UI voice integration (status bar, Talk Mode overlay, waveform visualizer, settings panel, push-to-talk), cloud STT/TTS fallback (OpenAI Whisper + ElevenLabs), speaker diarization, per-agent voice personality, voice command shortcuts, audio file I/O, latency + WER benchmarks, CPU profiling, E2E voice tests

### MVP Verification Checklist

- [ ] cpal audio capture works on Linux, macOS, Windows
- [ ] sherpa-rs STT transcribes speech with > 90% accuracy
- [ ] sherpa-rs TTS speaks text with < 200ms first-byte latency
- [ ] VoiceChannel routes audio through MessageBus correctly
- [ ] Talk Mode continuous conversation works
- [ ] voice_listen and voice_speak tools callable from agent
- [ ] `weft voice talk` starts Talk Mode session
- [ ] WebSocket `voice:status` events broadcast

---

## VP Pre-Implementation (Week 0) -- CANCELLED

**SPARC Dir**: `sparc/voice`
**Purpose**: Validate platform audio, model hosting, feature flags before sprint begins
**Status**: GATE CLOSED via formal cancellation (2026-04-29). See
`vp-gate-resolution.md` and ADR-053. The substrate-side STT path
shipped in `clawft-service-whisper` supersedes the in-process stack
this gate was meant to validate; VP1..VP5 are not going to be run.
The five rows are retained for historical context.

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VP1 | Audio pipeline prototype (mic -> VAD -> STT -> print text) | P0 | 0 | Cancelled (ADR-053) | voice-proto/ | Prototype |
| VP2 | Model hosting and download strategy (GitHub Releases + HuggingFace, SHA-256 manifest) | P0 | 0 | Cancelled (ADR-053) | -- | Decision |
| VP3 | Feature flag design (voice, voice-stt, voice-tts, voice-wake) | P0 | 0 | Cancelled (ADR-053) | clawft-plugin | Design |
| VP4 | Platform audio testing (cpal on Linux PipeWire/PulseAudio, macOS CoreAudio, Windows WASAPI, WSL2) | P0 | 0 | Cancelled (ADR-053) | -- | Testing |
| VP5 | Echo cancellation feasibility study (loopback subtraction vs WebRTC AEC vs hardware AEC) | P1 | 0 | Cancelled (ADR-053) | -- | Research |

**Pre-Implementation Summary**: 5 items, all cancelled

---

## VS1.1 Audio Foundation (Week 1)

**SPARC Dir**: `sparc/voice`
**Sprint**: VS1 -- Core Pipeline
**Deliverable**: Audio capture/playback working, VAD detecting speech boundaries, model download functional

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS1.1.1 | Add sherpa-rs, cpal to clawft-plugin/Cargo.toml behind voice feature | P0 | 1 | Not Started | clawft-plugin | Feature |
| VS1.1.2 | Create clawft-plugin/src/voice/mod.rs -- voice module structure | P0 | 1 | Not Started | clawft-plugin | Feature |
| VS1.1.3 | Implement AudioCapture -- cpal microphone input stream | P0 | 1 | Not Started | clawft-plugin | Feature |
| VS1.1.4 | Implement AudioPlayback -- cpal speaker output stream | P0 | 1 | Not Started | clawft-plugin | Feature |
| VS1.1.5 | Implement VoiceActivityDetector -- sherpa-rs Silero VAD wrapper | P0 | 1 | Not Started | clawft-plugin | Feature |
| VS1.1.6 | Model download manager -- fetch + cache + integrity check | P0 | 1 | Not Started | clawft-plugin | Feature |
| VS1.1.7 | Voice config types: VoiceConfig in clawft-types | P1 | 1 | Not Started | clawft-types | Feature |
| VS1.1.8 | Feature flag wiring: voice, voice-stt, voice-tts, voice-wake | P0 | 1 | Not Started | Multiple | Config |

**VS1.1 Summary**: 8 items

---

## VS1.2 STT + TTS (Week 2)

**Sprint**: VS1 -- Core Pipeline
**Deliverable**: STT transcribes mic input, TTS speaks text, both available as agent tools

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS1.2.1 | Implement SpeechToText -- sherpa-rs streaming recognizer | P0 | 2 | Not Started | clawft-plugin | Feature |
| VS1.2.2 | Implement TextToSpeech -- sherpa-rs streaming synthesizer | P0 | 2 | Not Started | clawft-plugin | Feature |
| VS1.2.3 | STT partial result callback -- emit intermediate transcriptions | P1 | 2 | Not Started | clawft-plugin | Feature |
| VS1.2.4 | TTS streaming playback -- start audio before full synthesis completes | P1 | 2 | Not Started | clawft-plugin | Feature |
| VS1.2.5 | TTS cancellation -- abort handle for interruption support | P1 | 2 | Not Started | clawft-plugin | Feature |
| VS1.2.6 | voice_listen tool -- on-demand transcription (non-streaming) | P0 | 2 | Not Started | clawft-tools | Feature |
| VS1.2.7 | voice_speak tool -- on-demand TTS (agent can speak text) | P0 | 2 | Not Started | clawft-tools | Feature |
| VS1.2.8 | CLI commands: weft voice setup, weft voice test-mic, weft voice test-speak | P1 | 2 | Not Started | clawft-cli | Feature |

**VS1.2 Summary**: 8 items

---

## VS1.3 VoiceChannel + Talk Mode (Week 3)

**Sprint**: VS1 -- Core Pipeline
**Deliverable**: Full voice pipeline (mic -> VAD -> STT -> agent -> TTS -> speaker), Talk Mode via CLI, WS voice events

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS1.3.1 | Implement VoiceChannel as ChannelAdapter plugin | P0 | 3 | Not Started | clawft-plugin | Feature |
| VS1.3.2 | VoiceChannel -> MessageBus integration (transcriptions as InboundMessage) | P0 | 3 | Not Started | clawft-plugin | Feature |
| VS1.3.3 | MessageBus -> VoiceChannel TTS (outbound text spoken aloud) | P0 | 3 | Not Started | clawft-plugin | Feature |
| VS1.3.4 | Basic Talk Mode controller (listen -> transcribe -> think -> speak -> listen) | P0 | 3 | Not Started | clawft-plugin | Feature |
| VS1.3.5 | Silence timeout configuration (configurable speech boundary detection) | P1 | 3 | Not Started | clawft-types | Feature |
| VS1.3.6 | Interruption detection: stop TTS when user starts speaking | P1 | 3 | Not Started | clawft-plugin | Feature |
| VS1.3.7 | CLI: weft voice talk -- start Talk Mode session | P0 | 3 | Not Started | clawft-cli | Feature |
| VS1.3.8 | WebSocket voice events: voice:status (idle/listening/processing/speaking) | P1 | 3 | Not Started | clawft-services | Feature |
| VS1.3.9 | Unit + integration tests for pipeline | P0 | 3 | Not Started | tests/ | Testing |

**VS1.3 Summary**: 9 items

---

## VS2.1 Voice Wake (Week 4)

**Sprint**: VS2 -- Wake Word + Platform Integration
**Deliverable**: "Hey Weft" activates voice pipeline, custom wake words trainable

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS2.1.1 | Add rustpotter to clawft-plugin/Cargo.toml behind voice-wake feature | P0 | 4 | Not Started | clawft-plugin | Feature |
| VS2.1.2 | Train "Hey Weft" wake word model (record 5-8 samples, train via rustpotter CLI) | P0 | 4 | Not Started | models/ | Feature |
| VS2.1.3 | Implement WakeWordDetector -- rustpotter integration | P0 | 4 | Not Started | clawft-plugin | Feature |
| VS2.1.4 | Wake word -> VAD pipeline activation (trigger listening on wake word) | P0 | 4 | Not Started | clawft-plugin | Feature |
| VS2.1.5 | CLI: weft voice wake -- start always-on wake word listener | P1 | 4 | Not Started | clawft-cli | Feature |
| VS2.1.6 | CLI: weft voice train-wake "hey weft" -- guided wake word training | P1 | 4 | Not Started | clawft-cli | Feature |
| VS2.1.7 | Custom wake word support (user-trained models stored in ~/.clawft/models/wake/) | P1 | 4 | Not Started | clawft-plugin | Feature |
| VS2.1.8 | CPU budget monitoring -- ensure wake word uses < 2% CPU | P1 | 4 | Not Started | clawft-plugin | Performance |

**VS2.1 Summary**: 8 items

---

## VS2.2 Echo Cancellation + Quality (Week 5)

**Sprint**: VS2 -- Wake Word + Platform Integration
**Deliverable**: Echo cancellation working, noise suppression active, multi-language support

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS2.2.1 | Software AEC implementation (loopback subtraction or WebRTC AEC crate) | P0 | 5 | Not Started | clawft-plugin | Feature |
| VS2.2.2 | Noise suppression pre-filter (sherpa-rs built-in or separate) | P1 | 5 | Not Started | clawft-plugin | Feature |
| VS2.2.3 | Adaptive silence timeout (learn user speech patterns over time) | P2 | 5 | Not Started | clawft-plugin | Feature |
| VS2.2.4 | Multi-language STT support (language auto-detection or config) | P1 | 5 | Not Started | clawft-plugin | Feature |
| VS2.2.5 | Voice selection (multiple TTS voices, per-agent voice config) | P2 | 5 | Not Started | clawft-types | Feature |
| VS2.2.6 | Audio quality metrics (SNR, latency percentiles, WER estimation) | P2 | 5 | Not Started | clawft-plugin | Performance |

**VS2.2 Summary**: 6 items

---

## VS2.3 Platform Integration (Week 6)

**Sprint**: VS2 -- Wake Word + Platform Integration
**Deliverable**: Voice Wake daemon with platform service files, Discord voice bridge, privacy indicator

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS2.3.1 | Linux systemd user service for Voice Wake daemon | P1 | 6 | Not Started | scripts/ | DevOps |
| VS2.3.2 | macOS launchd agent for Voice Wake daemon | P1 | 6 | Not Started | scripts/ | DevOps |
| VS2.3.3 | Windows startup task for Voice Wake daemon | P1 | 6 | Not Started | scripts/ | DevOps |
| VS2.3.4 | Microphone permission request handling (macOS/Windows) | P1 | 6 | Not Started | clawft-plugin | Feature |
| VS2.3.5 | Privacy indicator: visual notification when mic is active | P1 | 6 | Not Started | clawft-plugin | Feature |
| VS2.3.6 | PipeWire audio integration (Linux native) | P2 | 6 | Not Started | clawft-plugin | Feature |
| VS2.3.7 | Discord voice channel bridge: receive voice in VC -> STT -> respond via TTS | P1 | 6 | Not Started | clawft-channels | Feature |
| VS2.3.8 | Platform audio test suite (automated tests on CI) | P1 | 6 | Not Started | tests/ | Testing |

**VS2.3 Summary**: 8 items

---

## VS3.1 UI Voice Integration (Week 7)

**Sprint**: VS3 -- Advanced Features + UI Integration
**Deliverable**: Dashboard shows voice status, Talk Mode visual overlay, settings panel for voice configuration

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS3.1.1 | Voice status bar component (UI): idle/listening/processing/speaking indicator | P1 | 7 | Not Started | ui/ | Feature |
| VS3.1.2 | Talk Mode overlay (UI): floating transcript + stop/mute buttons | P1 | 7 | Not Started | ui/ | Feature |
| VS3.1.3 | Audio waveform visualizer (UI): real-time mic input display | P2 | 7 | Not Started | ui/ | Feature |
| VS3.1.4 | Voice settings panel (UI): mic select, voice select, language, wake word toggle | P1 | 7 | Not Started | ui/ | Feature |
| VS3.1.5 | Push-to-talk button (UI): hold to speak, release to process | P1 | 7 | Not Started | ui/ | Feature |
| VS3.1.6 | WebSocket voice events: partial transcription streaming to UI | P1 | 7 | Not Started | clawft-services | Feature |
| VS3.1.7 | WebSocket voice events: TTS progress (word highlighting) | P2 | 7 | Not Started | clawft-services | Feature |
| VS3.1.8 | Tauri voice integration: native mic access from desktop shell | P1 | 7 | Not Started | ui/ | Feature |

**VS3.1 Summary**: 8 items

---

## VS3.2 Cloud Fallback + Quality (Week 8)

**Sprint**: VS3 -- Advanced Features + UI Integration
**Deliverable**: Cloud fallback for STT/TTS, speaker identification, voice sessions persisted

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS3.2.1 | Cloud STT fallback: OpenAI Whisper API integration | P1 | 8 | Not Started | clawft-plugin | Feature |
| VS3.2.2 | Cloud TTS fallback: ElevenLabs / OpenAI TTS API integration | P1 | 8 | Not Started | clawft-plugin | Feature |
| VS3.2.3 | Fallback chain: local first -> cloud on failure/low confidence | P1 | 8 | Not Started | clawft-plugin | Feature |
| VS3.2.4 | Speaker diarization: multi-speaker identification (sherpa-rs) | P2 | 8 | Not Started | clawft-plugin | Feature |
| VS3.2.5 | Conversation mode: distinguish speakers in multi-party voice | P2 | 8 | Not Started | clawft-plugin | Feature |
| VS3.2.6 | Voice transcription logging (persist voice conversations to session) | P1 | 8 | Not Started | clawft-core | Feature |

**VS3.2 Summary**: 6 items

---

## VS3.3 Advanced Voice Features (Week 9)

**Sprint**: VS3 -- Advanced Features + UI Integration
**Deliverable**: Production voice system with per-agent voices, comprehensive benchmarks, E2E tests

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| VS3.3.1 | Per-agent voice personality (different voice per agent in multi-agent setup) | P2 | 9 | Not Started | clawft-types | Feature |
| VS3.3.2 | Voice command shortcuts ("Hey Weft, check my email" -> direct tool invocation) | P2 | 9 | Not Started | clawft-plugin | Feature |
| VS3.3.3 | Audio file input (process .wav/.mp3 files through STT) | P2 | 9 | Not Started | clawft-tools | Feature |
| VS3.3.4 | Audio file output (save TTS to .wav file) | P2 | 9 | Not Started | clawft-tools | Feature |
| VS3.3.5 | Latency benchmarking suite: speech-end to first-response-byte | P1 | 9 | Not Started | tests/ | Testing |
| VS3.3.6 | WER (Word Error Rate) benchmarking against test corpus | P1 | 9 | Not Started | tests/ | Testing |
| VS3.3.7 | Battery/CPU profiling: ensure wake word < 2%, full pipeline < 10% CPU | P1 | 9 | Not Started | tests/ | Performance |
| VS3.3.8 | Voice permission integration: restrict voice-triggered tool execution by level | P1 | 9 | Not Started | clawft-plugin | Security |
| VS3.3.9 | End-to-end voice tests (Playwright + audio simulation) | P1 | 9 | Not Started | tests/ | Testing |

**VS3.3 Summary**: 9 items

---

## Cross-Sprint Integration Tests

| Test | Sprints | Week | Priority | Status |
|------|---------|------|----------|--------|
| VoiceChannel -> MessageBus integration | VS1, Main D8 | 3 | P0 | Not Started |
| Wake Word -> Talk Mode activation | VS2.1, VS1.3 | 4 | P0 | Not Started |
| Voice events -> UI WebSocket | VS1.3, UI S1 | 7 | P1 | Not Started |
| Cloud fallback -> local STT handoff | VS3.2, VS1.2 | 8 | P1 | Not Started |
| Platform daemon -> Voice pipeline | VS2.3, VS1.1 | 6 | P1 | Not Started |
| Discord voice -> STT -> agent response | VS2.3, VS1.2, Main E1 | 6 | P1 | Not Started |

Test infrastructure: `tests/integration/voice/`

---

## Dependencies on Main Sprint

| Voice Task | Depends On (Main Sprint) | Status | Critical? |
|-----------|--------------------------|--------|-----------|
| VS1.1.1 (voice feature flag) | C1 (Plugin trait crate) | Not Started | Yes -- VoiceHandler trait must exist |
| VS1.3.1 (VoiceChannel) | C1, C7 (ChannelAdapter trait) | Not Started | Yes -- need plugin ChannelAdapter |
| VS1.3.2 (MessageBus integration) | D8 (Bounded bus channels) | Not Started | No -- works with unbounded (current) |
| VS1.3.8 (WS events) | UI S1.1.7 (WebSocket handler) | Not Started | No -- voice works without UI |
| VS2.3.7 (Discord voice) | E1 (Discord Resume) | Not Started | No -- basic Discord works |
| VS3.1.* (UI integration) | UI S1, S2 | Not Started | No -- voice works headless |

---

## Sprint Summary

| Element | Phases | Items | Weeks | Key Deliverables |
|---------|--------|-------|-------|-----------------|
| VP Pre-Implementation | VP1-VP5 | 5 | 0 | Audio prototype, model hosting, feature flags, platform testing, AEC research |
| VS1.1 Audio Foundation | VS1.1.1-VS1.1.8 | 8 | 1 | cpal capture/playback, Silero VAD, model download, voice config types |
| VS1.2 STT + TTS | VS1.2.1-VS1.2.8 | 8 | 2 | Streaming STT/TTS, voice_listen + voice_speak tools, CLI test commands |
| VS1.3 VoiceChannel + Talk Mode | VS1.3.1-VS1.3.9 | 9 | 3 | VoiceChannel adapter, Talk Mode, interruption detection, WS events |
| VS2.1 Voice Wake | VS2.1.1-VS2.1.8 | 8 | 4 | "Hey Weft" wake word, rustpotter integration, custom wake word training |
| VS2.2 Echo Cancellation | VS2.2.1-VS2.2.6 | 6 | 5 | Software AEC, noise suppression, multi-language, audio quality metrics |
| VS2.3 Platform Integration | VS2.3.1-VS2.3.8 | 8 | 6 | systemd/launchd/Windows daemons, Discord voice bridge, privacy indicator |
| VS3.1 UI Voice Integration | VS3.1.1-VS3.1.8 | 8 | 7 | Status bar, Talk Mode overlay, waveform visualizer, settings panel |
| VS3.2 Cloud Fallback | VS3.2.1-VS3.2.6 | 6 | 8 | OpenAI Whisper/ElevenLabs fallback, speaker diarization, session logging |
| VS3.3 Advanced Features | VS3.3.1-VS3.3.9 | 9 | 9 | Per-agent voices, benchmarks, CPU profiling, voice permissions, E2E tests |
| **Total** | **VP + VS1-VS3** | **75** | **0-9** | |

### Priority Distribution

| Priority | Count | Description |
|----------|-------|-------------|
| P0 | 21 | Must-have for MVP or critical pre-implementation |
| P1 | 33 | Important for complete voice system |
| P2 | 21 | Nice-to-have, stretch goals, advanced features |

### Exit Criteria

- [ ] All P0 items complete and verified
- [ ] All P1 items complete or explicitly deferred with justification
- [ ] `cargo test --workspace --features voice` passes with zero failures
- [ ] `cargo clippy --workspace --features voice -- -D warnings` clean
- [ ] STT accuracy > 90% on English test corpus (WER benchmark)
- [ ] TTS first-byte latency < 200ms (latency benchmark)
- [ ] End-to-end voice pipeline latency < 3s
- [ ] Wake word CPU usage < 2% (profiling benchmark)
- [ ] Full pipeline CPU usage < 10% (profiling benchmark)
- [ ] All 6 cross-sprint integration tests pass
- [ ] Platform audio works on Linux, macOS, Windows
- [ ] Voice pipeline works headless (no UI required)
- [ ] Cloud fallback chain functional (local -> cloud)
- [ ] All documentation updated to match implementation
