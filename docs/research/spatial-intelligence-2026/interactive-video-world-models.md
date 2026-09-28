# Interactive video world models (2025–2026) — WeftOS mapping

**Date:** 2026-09-21  
**Status:** Session research capture (not an ADR)  
**Priority:** **B** (Genie 3 / Cosmos / WBench as priors) · **C** (open-weight 2026 video WMs — watch, do not champion)  
**Parent:** [README.md](./README.md) (S2 + S8)

**WeftOS stance (do not re-litigate):**

> Pixel-video “worlds” are **not** Urth geometry. They **may** feed LeWM priors, synthetic rollouts, and Graph View **soft** edges. Metric source of truth remains **BVH + chain** (ADR-056 / ADR-078 / ADR-079). Latent / predictive world models are **optional** (ADR-090 R1–R5). Generative fill is cosmetic, always labeled non-metric (ADR-079).

This is the competing definition of “world model” in 2026: an autoregressive video generator that rolls the next frame under action. Marble-style persistent 3D (S1) is closer to our appearance/structure split. This family is a **simulator of pixels**, not a twin of objects.

---

## 1. Landscape

Dates are first public announcement / arXiv unless noted. “Realtime?” means interactive closed-loop generation at usable fps, not offline clip render. “Open?” means weights (not just a paper). WBench Navi scores are from the **2026-09-17** leaderboard ([WBench](https://meituan-longcat.github.io/WBench/)), 158 navigation cases, 0–100.

| Model | Output | Realtime? | Open? | Date | WBench Navi (avg) | URL |
|---|---|---|---|---|---|---|
| **Genie 3** (DeepMind) | 720p interactive video, promptable events | Yes ~24 fps | Closed (research preview / Project Genie web) | 2025-08-05 | 73.9 | [blog](https://deepmind.google/blog/genie-3-a-new-frontier-for-world-models/) · [product](https://deepmind.google/models/genie/) |
| **SIMA 2** (DeepMind) | Gemini-powered *agent* in 3D games / Genie worlds | Agent, not a generator | Closed | 2025-11-13 | — | [blog](https://deepmind.google/blog/sima-2-an-agent-that-plays-reasons-and-learns-with-you-in-virtual-3d-worlds/) |
| **Cosmos-Predict2.5** | Flow video WM (T2W / I2W / V2W), 2B+14B | Offline / post-train; not a 24 fps game loop | Yes (NVIDIA Open Model License) | 2025-10-28 paper; models 2025-10-06 | 74.6 (listed as Cosmos 2.5) | [arXiv:2511.00062](https://arxiv.org/abs/2511.00062) · [lab](https://research.nvidia.com/labs/cosmos-lab/cosmos-predict2.5/) · [gh](https://github.com/nvidia-cosmos/cosmos-predict2.5) |
| **Cosmos-Transfer2.5** | ControlNet-style RGB/depth/seg → photoreal video | Offline SDG | Yes | 2025-10 | — | [docs](https://docs.nvidia.com/cosmos/latest/transfer2.5/index.html) · [gh](https://github.com/nvidia-cosmos/cosmos-transfer2.5) |
| **Cosmos-Reason2** | Physical-AI VLM (2B / 8B / 32B) | N/A (reasoner) | Yes | 2025-12-19 | — | [gh](https://github.com/nvidia-cosmos/cosmos-reason2) |
| **Cosmos 3** (Nano 16B / Super 64B / later Edge 4B, H) | Omnimodal MoT: reason + generate text/image/video/audio/action | Mixed (Edge targeted at on-device; Super is datacenter) | Yes (OpenMDW-1.1) | **GTC Taipei 2026-06-01** (verified) | Super 76.0 · Nano 74.2 | [press](https://investor.nvidia.com/news/press-release-details/2026/NVIDIA-Launches-Cosmos-3-the-Open-Frontier-Foundation-Model-for-Physical-AI/default.aspx) · [arXiv:2606.02800](https://arxiv.org/abs/2606.02800) · [blog](https://developer.nvidia.com/blog/develop-physical-ai-reasoning-world-and-action-models-with-nvidia-cosmos-3/) · [gh](https://github.com/nvidia/cosmos) |
| **HY-World 1.5** (WorldPlay) | Streaming 720p video, 24 fps, geometric consistency | Yes 24 fps | Yes | 2025-12-17 | 78.1 | [arXiv:2512.14614](https://arxiv.org/abs/2512.14614) · [gh](https://github.com/Tencent-Hunyuan/HY-WorldPlay) |
| **Yume 1.5** | Keyboard + text-event interactive video (TSCM) | ~12 fps @ 540p on 1× A100 | Yes | 2025-12-26 paper; CVPR 2026 | 73.3 | [arXiv:2512.22096](https://arxiv.org/abs/2512.22096) · [project](https://stdstu12.github.io/YUME-Project/) · [gh](https://github.com/stdstu12/YUME) |
| **Matrix-Game 3.0** | 720p streaming, minute memory | Up to 40 fps (5B distilled) | Yes (MIT code; 5B weights) | 2026-03-27 / arXiv 2026-04-10 | 71.3 | [site](https://matrix-game-v3.github.io/) · [arXiv:2604.08995](https://arxiv.org/abs/2604.08995) · [gh](https://github.com/SkyworkAI/Matrix-Game) |
| **Lyra 2.0** | Camera-controlled video **lifted** to 3DGS + mesh | Distilled 4-step AR; not a 24 fps pixel loop | Code Apache-2.0; **weights research-only** (NVIDIA Internal Scientific Research license) | 2026-04-14/15 | 76.4 | [arXiv:2604.13036](https://arxiv.org/abs/2604.13036) · [page](https://research.nvidia.com/labs/sil/lyra2) · [gh](https://github.com/nv-tlabs/lyra) |
| **SANA-WM** | 2.6B, 720p **minute-scale** camera-controlled video | Distilled 60s clip in ~34s on RTX 5090 (faster than realtime *offline*); chunk-causal AR for sequential rollout | Yes | 2026-05-14 | 76.0 | [arXiv:2605.15178](https://arxiv.org/abs/2605.15178) · [page](https://nvlabs.github.io/Sana/WM/) · [gh](https://github.com/NVlabs/Sana) |
| **Astronex-World 1.0** | 5B Wan2.2-TI2V prior; 832×480 @ 24 fps causal; PRoPE camera + 64-D action | Yes 24 fps on 1× L20 48 GB | Yes | 2026-09-17 | 73.5 (Full 70.0) | [arXiv:2609.20034](https://arxiv.org/abs/2609.20034) · [gh](https://github.com/Astronex-Robotics/Astronex-World) |
| **WBench** | Benchmark (not a model): 289 cases, 1,058 turns, 5 dims, 22 metrics, 39 models | — | Yes (code + data) | paper 2026-05; leaderboard 2026-09-17 | — | [home](https://meituan-longcat.github.io/WBench/) · [arXiv:2605.25874](https://arxiv.org/abs/2605.25874) · [gh](https://github.com/meituan-longcat/WBench) |

**Not in the table, but adjacent:** HY-World **2.0** ([arXiv:2604.14268](https://arxiv.org/abs/2604.14268)) is a **3DGS / mesh** generator, not a pixel WM — that belongs with Marble (S1), not this file. Waymo World Model (2026-02) is Genie 3 post-trained for driving (camera + lidar) — closed, domain-narrow.

### 1.1 Airtree taxonomy (why this file exists)

Airtree’s 2026 primer ([world-models-primer](https://www.airtree.vc/open-source-vc/world-models-primer), Dan Coughlan, originally June 2026 / last reviewed 2026-06-24) splits the overloaded phrase “world model” using Jeff Hawke’s four buckets:

| Bucket | Learns | 2026 example | WeftOS analog |
|---|---|---|---|
| **Dynamics / general-purpose WM** | How the world *evolves* under action | Genie 3, Cosmos 3 generator, HY-World 1.5 | **LeWM** (optional predictive substrate) |
| **Spatial intelligence** | How the world *appears* (geometry, layout) | World Labs Marble, HY-World 2.0, Lyra lift | **BVH + splat dual output** (ADR-078) |
| **Behaviour / policy** | How to *act* | SIMA 2, Cosmos 3 action/policy, Physical Intelligence | Agents over ECC; not a geometry store |
| **Proxy** | Abstraction as a byproduct (LLMs) | GPT-class | ECC text/HNSW — not spatial SoT |

Video WMs in this file are **dynamics models that happen to render pixels**. Urth is **spatial intelligence with a chain**. Mixing the two is the category error this note exists to prevent.

Airtree also records the 2025 architectural shift: CausVid / Self-Forcing distilled bidirectional DiT quality into **AR-DiT** students so interaction is possible. That is why every 2026 open model in the table talks about DMD, context forcing, and “minute-long” memory — they are fighting **error accumulation**, not building a BVH.

---

## 2. What they can / cannot give Urth

### Can (useful, if labeled)

- **Synthetic visual rollouts** of “what if I walk left / it rains / a box falls,” for LeWM surprise training and Graph View *candidate* edges (F4), never as `WM_OBJECT` truth.
- **Action-conditioned priors:** 6-DoF camera (PRoPE / Plücker), 64-D continuous action (Astronex), keyboard WASD (Yume / Matrix-Game). These are the right *shape* of conditioning for a latent planner, not a replacement for pose from capture.
- **Control-mapped photorealism:** Cosmos-Transfer2.5 takes depth / seg / lidar-style maps → RGB. That is a **texture/appearance** stage on top of structure we already own.
- **Physical-common-sense language:** Cosmos-Reason2 / Cosmos 3 Reasoner as a *probe* (“is this collision plausible?”) on View edges. Output is text/score, not a leaf.
- **Agent curricula:** SIMA 2 (Gemini 2.5 Flash-Lite; ~65% task completion vs SIMA 1 ~31%, human ~71%) shows why Genie-class worlds exist — unlimited instruction-following practice. WeftOS agents should train against **Views + BVH queries**, using video WMs only as optional imagination.
- **Lift-to-3D as appearance seed:** Lyra 2.0 (and HY-World 2.0, out of scope here) generate walkthrough video then reconstruct 3DGS/mesh. That can seed L4 appearance, still subject to ADR-078 structure stage and ADR-079 “unknown vs fake terrain.”

### Cannot (hard no)

- **Metric geometry.** No AABB, no ECEF, no occupancy volume, no object ID that survives a rescan. Cosmos 3’s own technical report has **no entity/slot/scene-graph mechanism** — world state is implicit in the diffusion latent (already cross-walked in `.planning/research/cosmos3-vs-weftos-worldmodel.md`).
- **Hours of identity.** DeepMind states Genie 3 visual memory is on the order of **one minute**, continuous interaction **a few minutes, not hours**. Open 2026 models advertise “minute-long” consistency. Urth is a **sparse-first planet** that densifies over calendar time.
- **Geographic accuracy.** Genie 3’s own limitations list: cannot simulate real-world locations with perfect geographic accuracy; limited action space; weak multi-agent; text rendering. That is the opposite of OSM/SRTM L0–L3 ingest (ADR-079).
- **Authoritative “what is where.”** ADR-078: SoT is BVH + chain, not a SOG and not a generated MP4. ADR-090 R1: WM must not override ECC reasoning.
- **Cheap inference on WeftOS nodes.** Super is 64B datacenter; even “realtime 5B @ 720p” wants an H100-class GPU. Kernel / leaf devices stay ECC-only (ADR-090 R2/R5).

DeepMind is explicit that Genie worlds are **not** NeRF/splat: consistency is emergent in the pixel stream, with no explicit 3D representation. That sentence is the whole WeftOS objection.

---

## 3. Map to LeWM vs BVH vs Graph Views

```
  video WM  ──synthetic pixels / action tokens──►  LeWM (optional)
       │                                              │ Impulse / Observation only
       │                                              ▼
       │                                         ADR-090 facade
       │                                              │
       │         F4 soft edges (candidates)           ▼
       └──────────────► Graph View (F1–F10) ──F9──► BVH + chain  = Urth SoT
                            ▲
              sensors, splat structure, OSM, ToF
```

| WeftOS seam | Video WM role | Must not |
|---|---|---|
| **LeWM** (`weftos-worldmodel*`, SIGReg, `pred_φ`) | Teacher / prior: encode rollouts, surprise vs live sensors, CEM imagination | Become a required runtime (R2). Write causal edges (R3/R4). Replace ECC (R1). |
| **BVH** (`clawft-bvh`, ADR-056/078) | **None** as geometry. Optional: Lyra/Transfer output as *appearance* attached to an existing leaf | Publish generated frames as `WM_OBJECT` / `WM_SURFACE` without a structure stage |
| **Graph Views** F1–F10 | F4: ANN / DreamSim-style soft edges from video embeddings. F8: optional dual-branch score features. Never F9 without human/sensor confirm | Treat a Genie session as a fusion View’s hard spatial source (that is F2, BVH only) |
| **Urth LOD** (ADR-079) | L4 cosmetic fill, labeled `non-metric` | Fill L0–L3 unknown volumes with generated terrain |
| **Cosmos 3 action encoding** | Small, copyable idea: action tokens as first-class modality next to vision (already the recommended borrow in the 2026-07 Cosmos 3 note) | Import Cosmos tokenizer / 64B Super into the kernel |

**SIMA 2 vs WeftOS agents.** SIMA is a *behaviour* model sitting *on top of* a dynamics WM. Our equivalent is an agent that queries a Graph View / BVH, not an agent that lives inside generated pixels. Pairing SIMA-style instruction following with Genie-style imagination is interesting for **offline** LeWM training, not for the live twin.

---

## 4. WBench — steal the questions, do not copy the exam

[WBench](https://meituan-longcat.github.io/WBench/) (Fudan + Meituan LongCat, [arXiv:2605.25874](https://arxiv.org/abs/2605.25874); homepage updated **2026-09-17**) is the first serious multi-turn exam for this family:

- 289 cases, 1,058 turns; 4 interaction types (nav 601, subject action 213, event edit 183, perspective switch 61).
- 5 dimensions / 22 metrics: video quality, setting adherence, interaction, consistency, physics.
- Unified nav protocol (text / 6-DoF / discrete WASD) so families can be compared.
- Headline result: **no model dominates all dimensions.** Navigation is largely independent of event-edit / subject-action. Physics correlates with *rendering quality* (ρ ≈ 0.82), **not** with control. Multi-turn nav drops ~21 points from turn 1 to turn 4.

**Why we should not copy it wholesale as an Urth / LeWM gate:**

1. **It scores pixels, not leaves.** Aesthetic / HPSv3 / flicker / MegaSAM pose-from-video are the right tests for a *generator*. They say nothing about AABB stability, `chain_seq`, or object re-identification after a rescan.
2. **Physics is a VLM vibe check** (visual plausibility regressor + causal-fidelity VLM). That is F8-adjacent at best. WeftOS physics for agents is occupancy / no-go volumes and collider structure (ADR-078), optionally a real engine — not a captioner.
3. **Closed and API models sit on the same board as 5B distillations.** Useful journalism; a terrible release criterion for `clawft-bvh`.
4. **Navigation independence** is the useful finding: camera control ≠ subject control ≠ event editing. If we ever eval LeWM rollouts, **split those axes** instead of one “world-model score.”
5. **Compounding error is the actual bug.** Dedicated geometric-control WMs degrade slower than text-only video models. That argues for *explicit state* (our BVH) rather than for adopting WBench’s auto-eval stack.

**What to steal:** the *taxonomy of interactions* (nav / subject / event / viewpoint) as labels on Graph View edges and on LeWM action heads; the honesty that multi-turn is where everyone dies; the refusal to declare a champion.

---

## 5. Apply / compose / watch

Priority matches the parent README.

| Pri | Item | Action |
|---|---|---|
| **B compose** | Cosmos 3 **action tokens** + Reasoner-as-probe | Keep the 2026-07 borrow: action as a first-class conditioning stream for LeWM. Do **not** vendor Super. Optional: Reasoner NIM later as an F8 scorer behind a feature flag. |
| **B compose** | Cosmos-Transfer2.5 control maps | Appearance-only path: if we already have depth/seg from the splat/structure stage, Transfer is a cosmetic renderer. Output tagged non-metric. |
| **B watch** | Genie 3 + SIMA 2 | Closed. Use as the capability ceiling (“minutes, 720p24, promptable events”) in Urth docs so we do not over-claim. No integration path. |
| **B watch** | WBench | Read leaderboard when we write a LeWM eval; do not add WBench as a `scripts/build.sh` gate. |
| **C watch** | Astronex-World 1.0, Matrix-Game 3.0, HY-World 1.5, Yume 1.5, SANA-WM | Open AR-DiT zoo. Interesting for **offline** synthetic rollouts if someone has an H100. **Do not pick a champion this cycle.** PRoPE / 64-D action / TSCM are the portable ideas. |
| **C compose-later** | Lyra 2.0 lift | Closest video→3DGS path to ADR-078 appearance. Blocked by NVIDIA research-only **weight** license; code is Apache-2.0. Revisit if a redistributable checkpoint appears. Treat as S1-adjacent, not as Urth SoT. |
| **A (already decided)** | Dual output, unknown volumes, LeWM optional | Marble / HY-World 2.0 language, not Genie. See S1. |

No Plane ticket from this note unless asked. Residuals stay in the parent README queue.

---

## 6. Risks

1. **Consistency is minutes, not hours.** Genie 3: few minutes of interaction, ~1 minute visual memory. Open 2026 models: “minute-long” as a *brag*. Urth identity is multi-session and multi-node. Pixel memory ≠ object permanence.
2. **Closed weights / hostile licenses.** Genie 3 and SIMA 2 are research-preview. Lyra 2.0 weights are NVIDIA Internal Scientific Research (no production). Cosmos is OpenMDW-1.1 (usable, but not “drop into `clawft-kernel`”). Do not build a runtime dependency.
3. **GPU.** Realtime 720p AR-DiT is an H100 / L20 / 5090 story. Super is 64B datacenter. WeftOS local-only nodes must remain correct with WM absent (ADR-090 R2/R5).
4. **Generative-as-truth.** The product failure mode is filling `unknown` Urth volumes with a pretty Genie street and then routing agents through it. ADR-079 already forbids this; docs and F9 must keep the label.
5. **Eval theatre.** WBench / VBench / HUE will keep moving. Shipping “we beat Matrix-Game 3 on Navi” would be a category error.
6. **No entities.** Implicit latents do not survive a View promote. If a video WM cannot emit a stable object id, it cannot F9.
7. **Multi-agent and action poverty.** Genie 3 lists both as open problems. Graph Views exist specifically for multi-sensor, multi-agent association — do not wait for pixel WMs to grow a scene graph.

---

## 7. Sources

Live-fetched 2026-09-21.

**Closed frontier**

- [Genie 3 — DeepMind blog (2025-08-05)](https://deepmind.google/blog/genie-3-a-new-frontier-for-world-models/)
- [Genie 3 product page](https://deepmind.google/models/genie/)
- [SIMA 2 — DeepMind blog (2025-11-13)](https://deepmind.google/blog/sima-2-an-agent-that-plays-reasons-and-learns-with-you-in-virtual-3d-worlds/)
- [TechCrunch SIMA 2](https://techcrunch.com/2025/11/13/googles-sima-2-agent-uses-gemini-to-reason-and-act-in-virtual-worlds/)
- [Waymo World Model (Genie 3 post-train, 2026-02)](https://waymo.com/blog/2026/02/the-waymo-world-model-a-new-frontier-for-autonomous-driving-simulation)

**NVIDIA Cosmos**

- [World Simulation with Video Foundation Models — arXiv:2511.00062](https://arxiv.org/abs/2511.00062) (Predict2.5 / Transfer2.5)
- [Cosmos-Predict2.5 lab page](https://research.nvidia.com/labs/cosmos-lab/cosmos-predict2.5/)
- [Scale synthetic data — NVIDIA tech blog (2026-03-13)](https://developer.nvidia.com/blog/scale-synthetic-data-and-physical-ai-reasoning-with-nvidia-cosmos-world-foundation-models/)
- [NVIDIA launches Cosmos 3 — GTC Taipei 2026-06-01](https://investor.nvidia.com/news/press-release-details/2026/NVIDIA-Launches-Cosmos-3-the-Open-Frontier-Foundation-Model-for-Physical-AI/default.aspx)
- [Cosmos 3: Omnimodal World Models — arXiv:2606.02800](https://arxiv.org/abs/2606.02800)
- [Develop Physical AI with Cosmos 3 — NVIDIA tech blog (2026-05-31)](https://developer.nvidia.com/blog/develop-physical-ai-reasoning-world-and-action-models-with-nvidia-cosmos-3/)
- [github.com/nvidia/cosmos](https://github.com/nvidia/cosmos) (post-GTC home)
- In-tree: `.planning/research/cosmos3-vs-weftos-worldmodel.md`

**Benchmark**

- [WBench homepage](https://meituan-longcat.github.io/WBench/) (leaderboard 2026-09-17)
- [WBench — arXiv:2605.25874](https://arxiv.org/abs/2605.25874)
- [github.com/meituan-longcat/WBench](https://github.com/meituan-longcat/WBench)

**Open 2026 models**

- [Astronex-World 1.0 — arXiv:2609.20034](https://arxiv.org/abs/2609.20034)
- [Matrix-Game 3.0 — arXiv:2604.08995](https://arxiv.org/abs/2604.08995) · [site](https://matrix-game-v3.github.io/)
- [HY-World 1.5 / WorldPlay — arXiv:2512.14614](https://arxiv.org/abs/2512.14614)
- [Yume 1.5 — arXiv:2512.22096](https://arxiv.org/abs/2512.22096) · [CVPR 2026 poster](https://cvpr.thecvf.com/virtual/2026/poster/36968)
- [SANA-WM — arXiv:2605.15178](https://arxiv.org/abs/2605.15178) · [project](https://nvlabs.github.io/Sana/WM/)
- [Lyra 2.0 — arXiv:2604.13036](https://arxiv.org/abs/2604.13036) · [project](https://research.nvidia.com/labs/sil/lyra2)

**Taxonomy**

- [Airtree — To simulate is to understand (world-models primer)](https://www.airtree.vc/open-source-vc/world-models-primer)

**WeftOS**

- ADR-056 BVH · ADR-078 splat → structure · ADR-079 Urth · ADR-090 LeWM decoupling · ADR-095 / [graph-views.md](../graph-views.md) F1–F10 · [ruv-worldgraph-vs-weftos.md](../ruv-worldgraph-vs-weftos.md)
