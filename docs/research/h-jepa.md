# H-JEPA: hierarchical world models for visual planning (arXiv 2610.06805)

**Date:** 2026-10-07. **Board card:** 75bd501b-3824-4b48-ba49-5af65aa2dec0. **Status:** research capture, not an ADR.

Tags: READ = read in the paper/repo via fetch; INFERRED = my reasoning; CITED(path) = returned by `search_ruvnet`.
Caveat: the paper text reached me through a fetch-and-summarise tool, not a raw PDF read. Numbers tagged READ should be spot-checked against the PDF before anyone quotes them externally.

## Summary

H-JEPA (Zhang, Terver, Rabbat, LeCun, Balestriero; 2026-10-05) stacks action-conditioned JEPA world models. Each level predicts further ahead, in its own learned latent space. Higher levels drop fast, unpredictable detail and keep slow task-critical state. Planning runs top-down: the top level picks macro-actions toward the goal and passes subgoals down. On Visual AntMaze a three-level stack reaches 73.3% success against 18.0% for the flat LeWM baseline, with less planner compute (READ). It is validated on four simulated environments and offline on real DROID robot video.

The baseline it improves on is **LeWM**, which is also the name of the WeftOS latent world-model track (ADR-090, `weftos-worldmodel*`). This paper is the hierarchical extension of the thing we already have contract stubs for. That makes it more relevant than a generic JEPA paper (INFERRED, from READ "LeWM: ViT-Tiny + causal transformer, SIGReg" and the crate docs below).

## Verdict: PATTERN-ONLY now, with one small adoption path flagged

**Pattern-only.** Reasons:
1. The idea (stacked predictors at coarser strides, top-down subgoals, cost in the upper latent) is cheap to express and fits the 192-d SIGReg latent contract we already froze. We should steal the structure.
2. We cannot run or train it for WeftOS data today. Our LeWM crates are weights-free stubs (`LinearPredPhi`, `CemPlanner`, hash encoders); no trained encoder exists (READ in crate docs).
3. The results are all on simulated navigation plus an open-loop real-video fidelity metric. No closed-loop robot result (READ, paper limitation).
4. The gain depends on a timescale gap in the data (>2x between fastest and slowest state components). Where the gap is small the hierarchy helps little (READ).

Promote to **adopt** only if the smallest experiment below shows a hierarchy gain on a real WeftOS stream. Until then: watch the repo, borrow the design.

## Method in plain words

- **Level 1** is an ordinary LeWM-style JEPA: ViT-Tiny encoder (CLS token) plus a causal transformer predictor, trained to predict the next latent given the action (READ).
- **Level l>1** encodes a window of level l-1 latents with a two-layer MLP and has its own causal transformer predictor. Stride s=2 and window w=1 in all experiments, so each level sees a clock half as fast as the one below (READ).
- **Actions** are aggregated upward: a level-l action embeds the s lower-level actions it spans (READ).
- **Losses per level:** one-step teacher-forced latent prediction; SIGReg (push embeddings toward an isotropic Gaussian, which stops collapse); and an inverse-dynamics loss that predicts the action from consecutive latents. The inverse-dynamics term is what stops "slow-feature collapse", where an encoder throws away the moving agent and keeps the static background. Without it DROID fidelity falls to 0% (READ).
- **Stop-gradient:** upper-level action targets are detached, so gradient reaches the upper encoders only through state representations (READ).
- **Planning:** top level minimises distance from its imagined end latent to the goal, by gradient descent on macro-actions. Each lower level then minimises distance to the subgoal latents from above (end point plus a beta-weighted sum over intermediate subgoals). Level 1 outputs primitive actions (READ).
- Levels are trained end to end, not one after another (title of repo README, READ).

## Results

| Item | Result | Tag |
|---|---|---|
| Visual AntMaze, 3 levels vs flat LeWM | 73.3 +/-3.5% vs 18.0 +/-3.5% (3 seeds) | READ |
| AntMaze, 2 / 3 / 4 levels, full hierarchy | 39.3 / 73.3 / 63.3% | READ |
| Ablation: higher-level latent as cost only, no subgoals | 20.7-26.7% (vs 10-23% native); subgoal decomposition supplies most of the gain | READ |
| vs HWM (hierarchy in one shared latent) | nearly 2x success on AntMaze; marginal on FourRoom and Push-T | READ |
| Push-T | 2 levels match or beat LeWM; 3 levels worse (episodes too short: 58% of clips usable at 3 levels, 14% at 4) | READ |
| OGBench Cube | moderate gain, little selective abstraction (timescale gap under 2x) | READ |
| DROID real video, open-loop Frechet fidelity | LeWM 0% without IDM, ~20-30% with; H-JEPA ~40-50% at lower planner FLOPs | READ (approximate figures) |
| Planner compute | roughly 2-3x less at fixed success target | READ (approximate) |
| Subgoal-cost monotonicity (Spearman) | 0.89-1.00 vs 0.48-0.75 for full-horizon cost | READ |

Key mechanism finding: higher levels lose body state and distractor position but keep agent position, and the separation grows with the timescale gap (AntMaze gap >10x) (READ).

## Code, weights, licence, compute

- Repo `github.com/kevinghst/H-JEPA`, **MIT**, created recently (pushed 2026-10-06, 36 stars, 6 commits) (READ via `gh api`). Site `h-jepa.com`. Paper CC BY 4.0.
- Weights: 84 trained models (4 envs x 7 models x 3 seeds, 9.7 GB) plus 9 DROID models on Hugging Face (READ, repo README).
- Compute to train: single GPU for the sim environments; 2 GPUs, batch 128 for DROID; ~100 epochs; Python 3.11, PyTorch, CUDA 12.8 (READ). Parameter counts and wall-clock hours were not stated in what I could read.
- Data: 1.0-10M transitions per sim environment; DROID is public teleoperation video (READ).
- Mac note: CUDA 12.8 stack. Our existing Mac research notes already flag the same constraint for V-JEPA (`docs/research/spatial-intelligence-2026/vjepa-and-lewm.md` section 5). Training lives on the Linux/CUDA box, not the MacBook (INFERRED).

## Contrast with JEPA-Anything (arXiv 2609.20800, abstract only)

JEPA-Anything (Cui et al.) is orthogonal. It splits the latent target into complementary factors with dedicated pathways, recombined in one shared predictor, and tests the recipe across seven domains (vision, biology, clinical, control, molecular dynamics, physical fields, weather) (READ, abstract). H-JEPA splits along time (levels at different strides); JEPA-Anything splits along factors of the target and along domains. H-JEPA is about planning with action conditioning; JEPA-Anything reports dynamics and intervention prediction. They could compose (factorised latents inside each level) but neither paper tests that (INFERRED). A separate researcher covers JEPA-Anything in depth.

## Mapping to WeftOS

What exists (all READ in this worktree): LeWM contract with a 192-d SIGReg latent, `Encoder`, `Predictor` (`pred_phi: z_t, a_t -> z_{t+1}`), `LatentPlanner` (CEM at 10 Hz) in `crates/weftos-worldmodel-core/src/lib.rs`; weights-free stub impls in `crates/weftos-worldmodel-impls`; facade in `crates/weftos-worldmodel`; host in `crates/clawft-worldmodel-service`; invariants R1-R5 in `docs/adr/adr-090-lewm-ecc-decoupling-invariant.md`. The `Predictor` trait is single-step and single-level today.

1. **Latent contract (ADR-090 / WEFT-543).** H-JEPA needs one latent space per level. Our contract fixes 192-d for `mesh.sensor.v1` and says width changes need a wire major bump. Upper levels would be new latent kinds (for example `mesh.sensor.v1.L2`, `.L3`) with their own width, not a width change to v1. Smallest seam: a `level: u8` field on `Latent` version helpers and a `HierarchicalPredictor` that wraps N `Predictor`s (INFERRED).
2. **Multi-timescale ticks (ADR-047, 050 ms CognitiveTick; ADR-062; `.planning/voice-ecc-synthesis.md`).** The natural mapping is level k predicting at 2^k ticks. At the 50 ms tick, level 2 is 100 ms, level 3 is 200 ms. That covers sub-second horizons only. Planner reality today is CEM at 10 Hz (100 ms), so levels 1-2 already match the planner clock. Heterogeneous tick rates across nodes (10 ms server, 50 ms glasses, ADR-047) mean a strided hierarchy should be defined in wall-clock horizon, not tick count, so every node agrees on what "level 3" means (INFERRED).
3. **Voice loop (ADR-061/062).** The 5-state node lifecycle (Speculative / Frontier / Committed) is already a two-horizon scheme: cheap speculative read now, considered commit later. H-JEPA's reading is that the slow level should hold turn-level state (who has the floor, topic) and the fast level the backchannel and prosody dynamics. We have no latent for either and the voice-ecc synthesis models backchannel as a CrossRef, not a predicted latent. I do not recommend training a voice latent hierarchy: the conversation stream is not an action-conditioned control problem (INFERRED).
4. **BVH world model (ADR-056/078/079, spatial-intelligence-2026).** Our doctrine is that latent prediction is an optional sub-layer under ECC and never replaces BVH leaves (ADR-078 section 6; `vjepa-and-lewm.md`). H-JEPA's useful claim for us is "upper levels keep the agent and discard distractors". Applied here: slow-level latents attached to a region leaf via `VectorRef` (`VISUAL_FEATURES` index) would be a coarse, stable region signature; fast-level latents would stay transient observations. Planner output stays a prior, never a `WM_*` mint (ADR-090 R1, R3).
5. **Planning / agent loop.** Two separate things share the word "planning". (a) Robot/capture-head control in latent space: this is where H-JEPA applies. (b) LLM agent planning over tasks: it does not apply; there is no learned latent dynamics there. Do not conflate them in tickets (INFERRED).
6. **What data we actually have.** Honest answer: very little that fits. H-JEPA training needs (observation, action, next-observation) triples at volume, with action labels. We have: sensor streams (RF/CSI, thermal, IMU-class) with no controlled actions; splat capture sessions (phone video, hundreds of frames, no action channel); voice transcripts (not control). We have no robot, no action-labelled teleoperation corpus, and no trained LeWM checkpoint. DROID is the only public real-robot set the authors used, and it is not our domain. A hierarchy needs 3 levels' worth of long clips: the paper shows Push-T failing at 3 levels purely because episodes were too short (READ). Short capture sessions would hit the same wall (INFERRED).

## Mapping to rUv-native primitives (grounded with search_ruvnet, 2026-10-07)

Coverage for "hierarchical world model / JEPA" in the rUv corpus is thin: the first query returned only an HNSW simulation file, flagged INSUFFICIENT_EVIDENCE. I make no claim that rUv has a JEPA-style hierarchy; I found none.

- **SONA three-loop learning.** The ruvector SONA spec describes tiered temporal learning: Loop A instant (per request, micro-LoRA rank 1-2), Loop B hourly (router training, EWC++), Loop C weekly (consolidation and abstraction). CITED(`ruvector/crates/sona/src/loops/mod.rs`, `ruvector/examples/ruvLLM/docs/SONA/00-OVERVIEW.md`). This is a hierarchy of learning timescales, not of predictive latents over observations. It is a structural analogue only; it does not predict future states or accept actions as input. It could host the *training cadence* of a hierarchy (fast online adapter, slow consolidation) but is not a substitute for the predictors (INFERRED).
- **OccWorld bridge.** RuView/worldgraph already has a thin Rust client to an external occupancy world model that predicts 15 future frames and emits trajectory priors, with a model-swappable request/response contract. CITED(`ruview/docs/adr/ADR-147-nvidia-cosmos-world-foundation-model-integration.md`, `worldgraph/wifi-densepose-worldmodel/README.md`). It is single-horizon and occupancy-space, so it is the precedent for the sidecar pattern (Python model process, Rust bridge, privacy gate), not for hierarchy. The same ADR records the domain-gap lesson: pretrained weights on the wrong domain are "semantically meaningless without retraining" (CITED, ADR-147 section 5.2). That applies equally to DROID-trained H-JEPA weights on our sensors.
- **agenticow copy-on-write branching.** Branch a vector memory in ~0.5 ms and 162 B, exact read-through, instant rollback, MIT. CITED(`agenticow/docs/index.html`, `agenticow/package.json`). This fits the speculative-rollout side: a planner could branch a latent-memory base per candidate subgoal sequence and discard losers. It is memory versioning, not prediction, and its own docs say it is "infra layer, not a brain" (CITED, same page). Useful only if we persist rollouts, which the paper does not require (INFERRED).
- **RVF / HNSW** would store slow-level latents as region signatures (see BVH mapping). I did not query for RVF-specific capability beyond the above, so I assert nothing about it here.

## Smallest experiment

Goal: test the one claim that matters for us, "a stride-2 upper level improves long-horizon latent prediction on a stream with a real timescale gap", without training any robot model.

1. Pick one existing WeftOS stream with a known fast/slow split. Candidate: an RF presence/occupancy stream (fast: motion micro-variation; slow: room occupancy state). Alternative: IMU or thermal from the Seed. Needs at least ~1M frames of continuous log; check what we have before committing (open question 1).
2. Encode with a frozen small encoder into 192-d. Do not train the encoder first; use the existing hash/stub encoder only as a negative control.
3. Run the authors' repo single-GPU in its smallest config on their FourRoomDistractors data first, to reproduce the 1-level vs 2-level gap and confirm the pipeline works on our CUDA box.
4. Then fit two predictors on our stream: flat one-step vs a 2-level stack (s=2). Metric: latent prediction error at horizons 4 / 16 / 64 steps, plus whether the upper latent keeps the slow variable (linear probe for occupancy state) and drops the fast one.
5. Pass: upper-level probe recovers the slow variable with fast variable unrecoverable, and long-horizon error beats flat. Fail or no gap: record it and stop; no planner work.

No action conditioning is needed for this test, which sidesteps our missing action labels. It also does not test planning, so a pass justifies only the next experiment, not adoption.

## Open questions

1. Do we hold any continuous, action-labelled log with a timescale gap above 2x and enough length for three levels? (Not checked in the data stores; I only read the docs.)
2. Parameter counts and training hours are not in what I read; need the PDF appendix before sizing GPU time.
3. Who owns `Predictor` trait changes: multi-level needs `level` in the wire format, which ADR-090/WEFT-543 treats as a major-version event.
4. Does a fixed 2x stride suit heterogeneous-tick nodes, or do we need duration-based segments (the authors list variable-duration segments as future work)?
5. Closed-loop validity: authors have only open-loop real-robot fidelity. Do not cite the DROID numbers as control performance.
6. Licence of the HF DROID checkpoints was not separately confirmed; repo is MIT, paper CC BY 4.0.
7. Repo is one day old with 6 commits; reproducibility claims are untested by anyone else.

## Sources

- arXiv abstract: https://arxiv.org/abs/2610.06805 ; HTML: https://arxiv.org/html/2610.06805 ; alphaXiv: https://www.alphaxiv.org/abs/2610.06805 (not separately fetched; arXiv used)
- Code: https://github.com/kevinghst/H-JEPA (MIT, via `gh api`) ; site https://h-jepa.com/
- JEPA-Anything abstract: https://arxiv.org/abs/2609.20800
- In-repo: `docs/adr/adr-047-self-calibrating-tick.md`, `docs/adr/adr-062-ecc-graph-walk-conversation.md`, `docs/adr/adr-090-lewm-ecc-decoupling-invariant.md`, `docs/adr/adr-078-splat-feeds-world-model.md`, `.planning/voice-ecc-synthesis.md`, `docs/research/spatial-intelligence-2026/vjepa-and-lewm.md`, `docs/research/ruv-worldgraph-vs-weftos.md`, `crates/weftos-worldmodel-core/src/lib.rs`, `crates/weftos-worldmodel-impls/src/lib.rs`, `crates/clawft-worldmodel-service/src/lib.rs`
- search_ruvnet paths: `ruvector/crates/sona/src/loops/mod.rs`, `ruvector/examples/ruvLLM/docs/SONA/00-OVERVIEW.md`, `ruview/docs/adr/ADR-147-nvidia-cosmos-world-foundation-model-integration.md`, `worldgraph/wifi-densepose-worldmodel/README.md`, `agenticow/docs/index.html`, `agenticow/package.json`
