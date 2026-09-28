# Cluster synthesis: spatial-vlm-3d-embodied (Skill-3D refs 1,2,3,6,9,17,22,24,28,35,38,47,57,60,61,62,63,68,72,84,88,89,93,95,96,97,98)

**Source paper:** Skill-3D, arXiv:2606.07436, "Evolving Scene-Aware Skills for Agentic 3D Spatial Reasoning."
**Scope:** spatial-reasoning VLMs, 3D-scene LLMs, video spatial reasoning, world-model test-time reasoning, and embodied/robotics/driving/navigation models cited by Skill-3D.
**Companions (do not duplicate):** [`docs/research/spatial-intelligence-2026/README.md`](../../spatial-intelligence-2026/README.md), [`scene-graphs-open-vocab.md`](../../spatial-intelligence-2026/scene-graphs-open-vocab.md), [`feedforward-reconstruction.md`](../../spatial-intelligence-2026/feedforward-reconstruction.md). Per-ref detail lives in `analysis/NNN-*.md`; this file is the roll-up.

**Honesty rule (same as every WeftOS survey):** appearance/monocular output never mints metric geometry (ADR-078/079). This cluster's central finding: almost none of these 27 papers can be verified as metric from an abstract-level pass, and several are provable anti-patterns.

---

## 1. Reference table

| # | Title (short) | arXiv | Family | 3D repr. | Metric scale | Egocentric fit | Verdict |
|---|---|---|---|---|---|---|---|
| 1 | Gemini Robotics 1.5 | [2510.03342](https://arxiv.org/abs/2510.03342) | Embodied/robotics | none (RGB+lang→action) | n/a (action policy) | low | WATCH |
| 2 | Synthetic Vision (Physics Context Builders) | [2412.08619](https://arxiv.org/abs/2412.08619) | Native VLM | 2D only | n/a | low | SKIP |
| 3 | SpatialBot | [2406.13642](https://arxiv.org/abs/2406.13642) | Native VLM | RGB+depth map | unverified (sensor vs. estimator) | medium | WATCH |
| 6 | SpatialVLM | [2401.12168](https://arxiv.org/abs/2401.12168) | Native VLM | monocular→templated metric VQA | **anti-pattern**: claims metric from monocular RGB | low | SKIP (keep as anti-pattern exemplar) |
| 9 | SpatialRGPT | [2406.01584](https://arxiv.org/abs/2406.01584) | Native VLM | 3D scene graph + depth plugin | unverified depth provenance | medium | PATTERN |
| 17 | VLM-3R | [2505.20279](https://arxiv.org/abs/2505.20279) | Native VLM | implicit 3D tokens (opaque) | not metric by construction, monocular | medium (video) | SKIP |
| 22 | Chat-Scene | [2312.08168](https://arxiv.org/abs/2312.08168) | Native VLM | object-instance identifiers over ScanNet point clouds | inherited metric (pre-scanned) | low (offline scans) | PATTERN |
| 24 | RoboBrain | [2502.21257](https://arxiv.org/abs/2502.21257) | Embodied/robotics | none (2D affordance/trajectory) | not found | low | WATCH |
| 28 | Perspective-aware reasoning (APC) | [2504.17207](https://arxiv.org/abs/2504.17207) | Native VLM | abstracted object pose layout | relative/proportional only | medium | PATTERN |
| 35 | Coarse correspondences | not found (search) | Native VLM | 2D tracking only | none | **high** (frame-to-frame) | WATCH |
| 38 | OpenSpatial | [2604.07296](https://arxiv.org/abs/2604.07296) | Native VLM | 3D bbox data engine | not found | low (synthetic data) | WATCH |
| 47 | GPT4Scene | [2501.01428](https://arxiv.org/abs/2501.01428) | Native VLM | BEV + marker-annotated keyframes from video | unverified | **high** (video) | PATTERN |
| 57 | SpatialPrompting | [2505.04911](https://arxiv.org/abs/2505.04911) | Agentic/3D-augmented | keyframes + camera poses (no point clouds) | honest by omission (consumes upstream pose) | **high** (video capture) | PATTERN |
| 60 | RoboBrain 2.0 | [2507.02029](https://arxiv.org/abs/2507.02029) | Embodied/robotics | not found | not found | low | WATCH |
| 61 | Gemini Robotics (ER) | [2503.20020](https://arxiv.org/abs/2503.20020) | Embodied/robotics | 2D multi-view → predicted 3D boxes | **unconfirmed, flagged as possible violation** | low | SKIP |
| 62 | Cambrian-1 / CV-Bench | not found (search) | Native VLM (backbone+bench) | 2D multi-encoder features | n/a | low | WATCH (bench only) |
| 63 | GPT-4V for robotics | not found (search) | Embodied/robotics | 2D detection, no 3D | none (defers to robot sensors) | medium (human demo video) | WATCH |
| 68 | Chat-3D | [2308.08769](https://arxiv.org/abs/2308.08769) | Native VLM | object-centric 3D features (source unconfirmed) | not found | low | SKIP |
| 72 | Spatial-MLLM | [2505.23747](https://arxiv.org/abs/2505.23747) | Native VLM | feed-forward geometry features (VGGT-class) | not obtained, up-to-scale | **high** (video) | WATCH |
| 84 | MindJourney | [2507.12508](https://arxiv.org/abs/2507.12508) | Agentic/3D-augmented | none — generative video-diffusion rollout | not claimed (most honest of the cluster) | medium | WATCH |
| 88 | GR3D (geometrically-referenced) | [2603.08592](https://arxiv.org/abs/2603.08592) | Agentic/3D-augmented | object 3D attrs → text | not found | medium | PATTERN |
| 89 | SpatialMind / ScanForgeQA | [2506.03642](https://arxiv.org/abs/2506.03642) | Native VLM | simulation-generated training scenes only | metric in sim, none at inference | low | WATCH |
| 93 | Think3D | [2601.13029](https://arxiv.org/abs/2601.13029) | Agentic/3D-augmented | point clouds via Pi3X tool, cached | unverified — **key open question** | **high** (Skill-3D's own baseline) | PATTERN |
| 95 | CoV (chain-of-view) | [2601.05172](https://arxiv.org/abs/2601.05172) | Agentic/3D-augmented | inherits underlying scanned scene | inherited metric (ScanNet-class) | medium | PATTERN |
| 96 | DriveAgent-R1 | [2507.20879](https://arxiv.org/abs/2507.20879) (search) | Embodied/robotics (driving) | BEV/camera, tool-invoked | not found | low | PATTERN (RL pattern only) |
| 97 | RoboRefer | [2506.04308](https://arxiv.org/abs/2506.04308) | Native VLM / embodied | RGB + depth encoder, SFT+RFT | depth-source unconfirmed | medium | WATCH |
| 98 | NavGPT | [2305.16986](https://arxiv.org/abs/2305.16986) (search) | Embodied/robotics (navigation) | textualized Matterport3D nav graph | inherited metric (simulator), none exposed to LLM | low (simulator, not live) | PATTERN |

**Verdict counts:** ADOPT 0 · PATTERN 8 (9, 22, 28, 47, 57, 88, 93, 95, 96, 98) · WATCH 14 · SKIP 5 (2, 6, 17, 61, 68).

---

## 2. Taxonomy

**A. Native spatial VLMs (15 refs: 2,3,6,9,17,22,28,35,38,47,62,68,72,89,97)** — spatial competence is baked into the backbone or its fine-tuning data (depth-plugin fusion, spatial-VQA datasets, implicit geometry tokens, object-identifier tokens). No per-question external tool call. This is the largest family and the one most prone to the monocular→metric anti-pattern (6, and arguably 17/72's implicit/feed-forward geometry).

**B. 3D-augmented / agentic tool-use (6 refs: 57,84,88,93,95,96)** — an MLLM agent invokes external geometry tools (point-cloud reconstruction, keyframe selection, viewpoint/camera-action policies, generative world-model rollouts) inside a reasoning loop, closest to Skill-3D's own design. Think3D (93) is Skill-3D's direct baseline and prior-art loop; MindJourney (84) is the one paper in the whole cluster that is explicitly honest about not claiming metric output.

**C. Embodied / robotics / driving / navigation (6 refs: 1,24,60,61,63,98)** — spatial reasoning applied to acting in the world (VLA action policies, manipulation planning/affordance/trajectory, autonomous driving, VLN). Mostly out of current WeftOS scope (no actuator surface), useful only as reasoning-loop/RL patterns (96) or as the honest-geometry cautionary case (61).

---

## 3. Metric scale and honesty

Only three refs (3 SpatialBot, 9 SpatialRGPT, 97 RoboRefer) use an **explicit depth channel** at all, and even for those the depth *source* (calibrated sensor vs. monocular estimator) could not be confirmed from abstract-level fetches — every file flags this rather than assuming either way. Two refs are **honest by construction**: 57 (SpatialPrompting, consumes upstream camera poses, claims nothing new) and 84 (MindJourney, explicitly treats generative rollouts as reasoning evidence, not measured geometry). One ref is the cluster's **textbook anti-pattern**: 6 (SpatialVLM) builds an "internet-scale... 3D spatial reasoning dataset in metric space" from monocular RGB with no external scale anchor — this is exactly the failure ADR-078/079 legislate against and should be cited by name in any WeftOS honest-geometry writeup. 93 (Think3D) — Skill-3D's own baseline, and the closest analog to a future Rust reimplementation — has an **unverified** point-cloud tool scale; this is the single most important open question to resolve before porting its reasoning-loop shape, since Skill-3D inherits the same tool. The remainder (roughly two-thirds of the cluster) simply do not expose enough method detail at abstract level to confirm scale either way; treat all of them as **not metric until proven otherwise**.

---

## 4. Egocentric / glasses (MentraOS) fit

**High fit** (video-native, frame-sequential, suits live capture): 35 (coarse correspondences — frame-to-frame tracking is exactly what an egocentric stream needs), 47 (GPT4Scene, BEV from video), 57 (SpatialPrompting, keyframe-driven, explicitly designed for off-the-shelf MLLM + video), 72 (Spatial-MLLM, feed-forward geometry features from video), 93 (Think3D, the reasoning loop Skill-3D and any Rust port would run per-capture-session).
**Medium fit:** 3, 9, 28, 63, 84, 95, 97 — depth/pose/object-pose methods that could consume MentraOS frames but were designed for static/offline scans or robot sensors.
**Low fit:** pre-scanned point-cloud dialogue systems (22, 68), simulator/navigation-graph methods (98), synthetic-data engines (38, 89), and the robot-body-centric VLA/manipulation family (1, 24, 60, 61) — these assume a different capture modality entirely.

---

## 5. Recommendations for WeftOS Rust design and Urth scene memory

1. **Do not port any native-spatial-VLM's metric claims as truth.** SpatialVLM (6), VLM-3R (17), Gemini Robotics-ER (61), and (until verified) Think3D's point-cloud tool (93) must have their outputs — if ever consumed by a WeftOS agent — labeled `confidence`-bearing estimates, never written as measured `AabbWire` into BVH. This is a direct extension of the same doctrine already applied to LangSplat/HOV-SG in `scene-graphs-open-vocab.md` §2.2.
2. **Steal four prompting/addressing patterns, not models:** (a) Chat-Scene's (22) durable per-object identifier token for dialogue → maps directly onto referring to `LeafId`-backed Objects across an agent conversation turn; (b) SpatialRGPT's (9) "region query → grounded relative-geometry answer" shape → template for a future BVH-backed spatial-QA tool call; (c) SpatialPrompting's (57) keyframe-selection heuristic (VL-similarity + pose spread + sharpness) → a capture-triage step ahead of Urth Event ingestion from MentraOS streams; (d) GPT4Scene's (47) BEV + consistent-object-ID visual prompting → an agent-context packing style once Urth leaf IDs exist.
3. **Think3D (93) is the load-bearing prior art.** Since it is Skill-3D's own comparison baseline and the paper this whole cluster is scaffolding around, a faithful Rust reimplementation should reuse its reasoning-loop shape and (if Apache-2.0, confirm license) its eval-split scripts for VSI-Bench/CV-3D/MMSI-Bench/BLINK — but gate any geometry it produces behind the same `vector: None` / `bvh_published: false` discipline W1 already uses for RANSAC/SpatialLM proposals (`feedforward-reconstruction.md` §2-4).
4. **MindJourney (84) and CoV (95) are the cleanest LeWM-adjacent and F10-adjacent patterns respectively.** MindJourney's test-time generative rollout is a candidate prior for interactive agent planning (ADR-090, compose-later, never SoT). CoV's coarse-to-fine active-viewpoint selection policy is a direct analog for a future "what should I look at next" tool over Urth Graph Views (F10 subgraph-pack territory, `scene-graphs-open-vocab.md` §3.2) — and, combined with SpatialPrompting's keyframe heuristic, is the most actionable idea for MentraOS-driven capture-guidance ("look here next" prompts to the wearer).
5. **Embodied/robotics refs (1, 24, 60, 61, 63) stay WATCH-only.** WeftOS has no actuator surface today; revisit only if a robot/actuator scope opens. RoboBrain 2.0 (60) supersedes RoboBrain (24) and should be the sole watch target if that day comes. DriveAgent-R1's (96) cost-aware active-perception RL pattern is the one exception worth studying now, purely for WeftOS's own tool-cost routing in agent skill selection — independent of the driving domain.
6. **No ADOPT verdicts in this cluster.** Nothing here is a drop-in geometry source or crate; every usable idea is a *prompting, addressing, or tool-orchestration* pattern layered on top of WeftOS's existing BVH/HNSW/Graph-View doctrine, not a replacement for it.

---

## 6. Sources

27 per-reference analysis files at `docs/research/skill-3d/papers/analysis/{001,002,003,006,009,017,022,024,028,035,038,047,057,060,061,062,063,068,072,084,088,089,093,095,096,097,098}-*.md`, each citing arXiv/venue pages fetched 2026-09-28. Numeric results are marked "not found" throughout rather than fabricated — this cluster's abstract-level research pass could not verify most quantitative claims; a follow-up pass reading full PDFs is recommended before citing any benchmark score from this cluster in a design doc.
