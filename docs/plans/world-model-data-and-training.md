# World model: data, labels and weights (plan)

- **Status:** Proposed 2026-10-07. Needs the owner's decisions in §8 before any phase starts.
- **Asks:** "Do we have a plan to build the weights and labels we need, and does H-JEPA add
  anything?" Short answers: **no plan existed**, and **yes, H-JEPA helps**, mainly as the
  reference trainer and evidence for a two-level predictor. It does not solve our data problem.
- **Builds on:** ADR-090 (LeWM / ECC decoupling, Accepted), the LeWM crates
  `weftos-worldmodel-core` / `-impls` / `weftos-worldmodel` / `clawft-worldmodel-service`
  (WEFT-519..532), ADR-107 (spatial evidence engine), ADR-110 (agent transcripts),
  RuView's own training ADRs (070, 071, 079), and the research notes
  [h-jepa.md](../research/h-jepa.md) and [jepa-anything.md](../research/jepa-anything.md).

## 1. Where we are (verified 2026-10-07)

| Piece | State |
|---|---|
| Architecture (192-d SIGReg latent, `pred_φ`, CEM planner at 10 Hz, four-condition rollback gate, two training surfaces, RVF model segments, hot-swap) | Built, **stubs only**. `weftos-worldmodel-impls/README.md`: "no ML weights"; the candle ViT-tiny / AdaLN path is a skeleton returning `Unavailable`. |
| Training | **On hold** since 2026-09-28 (owner, Skill-3D card 65f8a922). This plan asks to lift the hold for the sensor world model only. |
| Recorded sensor data | **None.** RuView `data/recordings` and `v2/data/recordings` are empty; the spatial workspace ships only `demo-room.synthetic.jsonl`. The first real room capture and the first hardware-backed live frame are still open cards (61020daa, f491f12e). |
| Long-running logs we do have | ExoChain (51 MB on the Mac, about 89 k events), agent sessions (ADR-110 will collect them), governance `TrajectoryRecorder`. |
| Labels | None recorded. |
| Sensor classes in code | `SensorClass` = generic, RGB-D, IMU, proprio, LiDAR, audio. **Our real sensors are missing:** mmWave radar (LD2450, LD6002), Wi-Fi CSI (RuView), ToF, ECG / biopotential, environmental. |
| Training hardware | MacBook M5 Max, 128 GB (Metal/MPS). photo-gallery: 16 cores, 188 GB, **no GPU** (storage and CPU preprocessing only). RuView ships `gcloud-train.sh` and `mac-mini-train.sh`. |
| RuView training pipeline | Designed, unused: ADR-070 self-supervised pretraining and collection protocol, ADR-071 training pipeline (Proposed), ADR-079 camera ground truth (Accepted); `scripts/collect-training-data.py`, `collect-ground-truth.py`, `align-ground-truth.js`; crates `homecore-recorder`, `wifi-densepose-train`. |

## 2. What H-JEPA adds

1. **It is the same model family.** H-JEPA's baseline is LeWM, the design our crates
   implement (192-d latent, action-conditioned predictor, CEM). A hierarchy plugs in above
   `pred_φ` without changing ADR-090: upper levels are still Observation / Impulse writers.
2. **The hierarchy suits our signals.** Upper levels filter fast, unpredictable variation and
   keep slow state. CSI and radar are noisy at 10-100 Hz, while what we care about (occupancy,
   who is where, breathing state) changes over seconds to minutes. This mirrors ECC's
   multi-timescale ticks.
3. **A working reference trainer.** MIT code with 84 trained models replaces our residual
   training stubs as a design reference, and its released models let us test our planner,
   SIGReg monitor and rollback gate on a known-good model before ours exists.
4. **What it does not give us:** data, labels or actions from our world. Its gains need
   action-labelled sequences long enough for three levels; ours do not exist yet.

## 3. Key point: labels are needed for evaluation, not for training

JEPA-style models train **self-supervised**: predict the next latent from the current one
(and an action). So the bottleneck is **recorded streams and actions**, not labels. Labels
are needed for the rollback gate's held-out probe, for checking the model learned something
real, and for downstream heads (pose, vitals). We can get most of them automatically:

| Label | Source | Notes |
|---|---|---|
| Person position / pose | Camera as a training-time teacher (RuView ADR-079), discarded at deployment | Needs a consent and handling rule (§7) |
| Room geometry | Tape-measured shell (ADR-107, already `MEASURED`) | Already captured for the test room |
| Heart rate / breathing | ECG cog (SEN0213) worn during LD6002 sessions | Cross-modal: radar learns, ECG grades |
| Presence / count | Agreement across radar, CSI, ToF, PIR, plus operator `human_confirm` events | Disagreement is flagged, not guessed |
| Protocol step | The script the operator follows ("walk A to B", "sit", "leave") | Doubles as the **action** channel |

**Actions** in a sensing mesh are scarce, so we create them: scripted protocol steps,
actuator commands (lights, the iDotMatrix panel, cog config changes), and node placement
moves. For the agent side, every tool call in ExoChain and ADR-110 transcripts is an action
with an observed outcome, the one place we already have actions at scale.

## 4. Phases

| Phase | Delivers | Done when |
|---|---|---|
| **W0 Decide** | Owner lifts the hold for the sensor world model; picks the training stack and the camera rule (§8). | Decisions recorded here and on the board. |
| **W1 Recorder** | A WeftOS capture session: cog outputs, spatial evidence (`spatial.evidence.v1`), CSI `.csi.jsonl`, actions and protocol steps written with one clock to a session manifest (room, node poses, protocol, consent, proof tag) and stored on photo-gallery under the `ruview-demo` project. Reuse RuView's collector and `homecore-recorder` rather than writing a new one. Add the missing `SensorClass` variants (radar, Wi-Fi CSI, ToF, biopotential, environmental). | A 10-minute session replays bit-for-bit from PG, with every record carrying `MEASURED` / `CODE` / `SYNTHETIC`. |
| **W2 First real dataset** | The test room (6.40 × 3.66 m): empty room, one person on scripted paths, sit/stand, two people, at least 3 hours over several days; CSI + LD2450 + LD6002 + ToF + camera teacher + tape shell + ECG during vitals runs. Closes cards 61020daa and f491f12e on the way. | Dataset card: hours per condition, held-out split by day, checksums. |
| **W3 Labels** | Automatic label tracks from §3, aligned to the session clock, stored separately from observations, each with provenance. | Held-out probe set exists; label agreement report. |
| **W4 Evaluation harness** | Matched baseline (copy-last and linear `pred_φ`), held-out probe, VoE diff, the activity-floor collapse alarm and geometry audit from jepa-anything.md, wired into the existing rollback gate and SIGReg monitor. | The gate can say no to a model on real data. |
| **W5 Flat LeWM** | Train encoder + `pred_φ` on W2 (Mac first), export weights to the RVF model segment, load through the candle path, swap at a tick. | Beats the matched baseline on held-out days, passes the gate, runs at 10 Hz on the Mac. |
| **W6 H-JEPA two-level** | 2-level vs flat on the same data, action-free first, then with protocol steps as actions. | Long-horizon prediction or planning improves over W5, or we record that it does not. |
| **W7 Agent-domain world model** (optional) | ExoChain + ADR-110 transcripts as action-labelled trajectories (tool call → outcome); same harness. | Owner decides after W5. |

## 5. Training stack (recommendation)

Train offline in **PyTorch on the Mac (MPS)**, because H-JEPA, JEPA-Anything and RuView's
`wifi-densepose-train` sources are PyTorch, and our model is small (ViT-tiny class, 192-d).
Export **safetensors** and run inference through the existing **candle** path in Rust; no
Python at runtime. Use cloud GPUs (RuView `gcloud-train.sh`) only when a run outgrows the Mac.
Keep RuView ADR-071's ruvllm route open for CSI-only heads that need SONA/LoRA adaptation
on device. photo-gallery stores sessions and does CPU preprocessing.

## 6. What stays out

- Skill-3D training (Qwen3-VL SFT/GRPO, card 65f8a922) stays on hold.
- No world-model output may override ECC reasoning (ADR-090 R1-R5); every model write goes
  through `lewm_invariant`.
- No client data: recordings come from our own test room and our own hardware.

## 7. Privacy and handling

Camera frames and ECG are sensitive. Proposed rule: camera is training-time only, frames stay
on photo-gallery under the project's 0700 home, only derived labels (keypoints, positions)
leave it, raw frames have a retention date, and only people who consent are recorded. ECG
data is labelled with the subject's consent and never published.

## 8. Owner decisions

1. Lift the training hold for the sensor world model (W1-W6)? Skill-3D stays held.
2. Training stack: PyTorch on the Mac → safetensors → candle (recommended), or another.
3. Camera ground truth: allowed in the test room under §7?
4. Recording budget: about 3 hours over several days to start; who is recorded.
5. W7 (agent-domain world model): now, later, or never.
