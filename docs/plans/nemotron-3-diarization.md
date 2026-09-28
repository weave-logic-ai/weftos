# Nemotron 3 Diarization — WeftOS engine

**Plane:** [WEFT-735](https://app.plane.so/) (Todo, 0.8.x, ws10-voice). Follow-on to WEFT-227 (Done).

## Source
- lab: `~/llm/docs/models/registry/speech.yaml` (`Nemotron-3-Diarization`)
- weights: `mlx-community/Nemotron-3-Diarization` (~199 MB bf16)
- upstream: https://huggingface.co/nvidia/Nemotron-3-Diarization (2026-09-23, OpenMDW 1.1)
- mlx-audio: Blaizzy/mlx-audio#970 merged 2026-09-23 (`03a4d99`); lab pin is git, not PyPI 0.5.5
- code: `crates/clawft-channels/src/voice/diarization.rs` (`DiarizationBackend`, `SherpaDiarizer` placeholder)
- talk: `crates/clawft-voice-talk/src/session.rs` (ECAPA enroll only)
- tools: `crates/clawft-tools/src/voice_backend.rs` (`Transcript.speakers` / `with_diarization` unused)
- prior: WEFT-227 (Done) — types + energy-gap + embedding-cluster + sherpa TODO

## Problem / gap
WEFT-227 shipped the **seam**, not a neural engine. Production Talk Mode attributes a whole utterance with ECAPA (`SpeakerEmbedder` → enrolled identity). That answers "is this Mathew?" It does **not** answer "who spoke when" inside a buffer with overlap, barge-in, or a second person in the room.

`default_diarizer()` is still `EnergyGapDiarizer`. `SherpaDiarizer` always returns `VoiceError::Config`. `Transcript.with_diarization` is never called.

NVIDIA Nemotron 3 Diarization is the right engine: 100M Sortformer, 8 speakers, 10 ms frames, streaming 0.32–1.04 s or offline 30.4 s, arrival-order labels, no clustering. Lab smoke 2026-09-23 on this M5 Max: `mlx_audio.vad.load(..., strict=True)` + `set_streaming_config("low")` on a 10 s wav → 2 `speaker_0` segments. Kokoro TTS still works on the new mlx-audio.

Do **not** pull `nemo-toolkit` / CUDA `.nemo` into WeftOS. Do **not** replace ECAPA. Diarization labels (`spk-0`) stay generic until a registry match upgrades them to an enrolled `SpeakerId`.

## Integration (how it fits)

```
mic → AEC → Silero VAD → [NEW Nemotron diarize] → Parakeet STT
                         ↘ ECAPA embed (enroll / gate)
Talk session already has SpeakerRegistry. DiarizationBackend.diarize(pcm)
produces DiarizationSegment[]; remap_labels() + SpeakerRegistry match
upgrades spk-N → enrolled id when cosine is above threshold.
```

Layering vs existing backends:
| Backend | Role | Keep? |
|---|---|---|
| `FixtureDiarizer` / `EnergyGapDiarizer` | CI / no weights | yes, default without onnx |
| `EmbeddingDiarizer` | windowed ECAPA cluster | keep as fallback |
| `SherpaDiarizer` | unstaged pyannote TODO | replace with Nemotron impl |
| **Nemotron3Diarizer** | neural "who spoke when" | **new canonical native engine** |
| ECAPA `SpeakerEmbedder` | enrolled identity | unchanged |

Runtime: prefer `ort` ONNX in `clawft-voice-onnx` if we can export; else Apple-only `mlx-rs` on the mlx-community safetensors (lab-proven). FluidAudio/ANE is a third option — do not take a Swift sidecar unless ONNX and mlx-rs both fail. Python mlx-audio stays in the **llm lab**, not the WeftOS kernel.

## Acceptance criteria
- [ ] `DiarizationBackend` impl named `nemotron` (or `nemotron3`) in `clawft-voice-onnx` (or channels if no extra deps), gated like other voice models: degrades with a clear error when weights are absent.
- [ ] Weights live under `~/.weftos/models/diarization/` (or catalog path), ~200 MB, OpenMDW 1.1 noted. `weft voice setup` can fetch `mlx-community/Nemotron-3-Diarization`.
- [ ] Streaming config presets: `ultra-low` 0.32 s, `low` 1.04 s, `offline` 30.4 s. Default for Talk: `low`.
- [ ] Overlap allowed (two active speakers). Max 8. Labels arrival-order `spk-0`….
- [ ] Talk/STT path calls `diarize` and `Transcript.with_diarization` so `voice_listen` / transcript log carry segments.
- [ ] Optional remap: `spk-N` → enrolled `SpeakerId` via ECAPA cosine (existing `remap_labels`).
- [ ] `EnergyGapDiarizer` remains `default_diarizer()` when the neural engine is off (CI hermetic).
- [ ] `SherpaDiarizer` either deleted or re-documented as superseded; `diarization-sherpa` feature renamed or aliased.
- [ ] Tests: unit on a fixture wav (single speaker → one label); two-talker fixture if we have one; graceful missing-weights; `scripts/build.sh test` for channels + voice-onnx + voice-talk; `scripts/build.sh check`.
- [ ] Docs: `docs/development/feature-flags.md`, `diarization.rs` module docs, ADR-053 note that STT stays Parakeet/substrate — only the diarization engine changes.

## Dependencies
- blocked-by: none (WEFT-227 already Done)
- blocks: multi-party Talk transcripts, meeting capture, ECC speaker-turn impulses
- lab already: mlx-audio git pin + catalog entry in `~/llm`

## Notes
- Do not install Pi. Do not run NeMo on CUDA.
- mlx-audio PyPI 0.5.5 (2026-09-21) does **not** include PR #970; wait for the next wheel before dropping the git pin in the lab.
- Vendor DER (NVIDIA, RTX PRO 5000): DIHARD III full 12.73 offline / 13.18 @ 1.04 s. FluidAudio M5 Pro: DER ~9.4, 31–904× RTFx by preset — useful Apple numbers, different harness.
