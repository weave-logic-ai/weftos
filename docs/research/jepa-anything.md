# JEPA-Anything: orthogonal predictive factorization across domains

**Date:** 2026-10-07
**Status:** Research capture (not an ADR). Board card b0a7c9c3-80d1-4e83-82be-99f4bd979ef9.
**Paper:** arXiv:2609.20800, Cui, Wang, Xu, Liu, Yu, Zhang, Gao, Yang, Ouyang, Heng, Wu, Yin, Ling Yang. Submitted 2026-09-17.
**Code:** https://github.com/Gen-Verse/JEPA-Anything (cloned shallow, HEAD dated 2026-09-18, outside the repo).

Tags: **READ** = read directly in paper abstract/HTML extraction or in the cloned code. **INFERRED** = my reading, not stated by the authors. **CITED(path)** = returned by `search_ruvnet`. I read the paper through an HTML extraction (method, K/r values, Pong numbers), not as a full PDF. Model sizes and training compute are not stated in what I could extract.

## 1. Summary

JEPA-Anything takes the standard JEPA recipe (encoder, EMA target, predictor in latent space) and replaces the single latent target with K orthogonal slices of the latent, each predicted by its own pathway, then recombined into a full state. The authors apply it to seven domains (vision, biology, clinical trajectories, control, molecular dynamics, physical fields, weather). The public repo is small: a PyTorch core library for the projection, losses and audits, a design-validator "skill", and one synthetic structural recipe. It ships no trained weights, no datasets, and no training loop (READ).

## 2. Verdict: pattern-only (with one cheap watch item)

**Reason:** the idea is a regulariser plus a head layout on top of a JEPA we do not yet run. WeftOS has no trained JEPA/LeWM runtime today (ADR-090 says the `weftos-worldmodel-*` crates are not yet a runtime dep; READ in `docs/adr/adr-090-lewm-ecc-decoupling-invariant.md`). Adopting a Python/PyTorch training library with no weights and no data, ahead of a latent model to attach it to, buys nothing. What is worth taking is the pattern: split a latent state into named-later, orthogonal predictive coordinates, audit the geometry, and refuse to name a factor before an experiment justifies it. That maps cleanly onto ECC's existing "impulse, not authority" stance (ADR-090 R1-R5).

**Watch:** re-check when (a) LeWM gets a first trained predictor, or (b) independent replication of the matched-baseline gains appears. The headline numbers are single-paper, small-absolute-error results (section 5).

Adjacent paper H-JEPA (arXiv:2610.06805) is covered separately in `h-jepa.md`; not compared here.

## 3. Method in plain words

READ (paper extraction + `jepa-anything-core/src/jepa_anything_core/opf.py`, `losses.py`):

1. An encoder maps an observation to a latent state of width `d`. A frozen/EMA target encoder maps the future (or intervened) observation to the target latent, as in any JEPA.
2. A projector stack `P` of shape `(K, r, d)` with `K * r = d` splits the state into K groups of r coordinates ("factors"). `decompose_state` is an einsum with `P`. `compose_state` rebuilds the full `d`-vector with the Moore-Penrose pseudoinverse of the flattened projector, not a plain transpose (the transpose is only exact when rows are orthonormal, and the repo exposes it separately as an audit).
3. A predictor has one pathway per factor. Each pathway predicts its own r-dimensional slice of the target. Loss is prediction error on the factor coordinates.
4. Three regularisers keep the slices honest. (a) Orthogonality: cross-factor `sum ||P_i^T P_j||_F^2` plus within-factor `||P_k^T P_k - I||_F^2`. (b) Factor activity: every coordinate of the projected target must keep a minimum std (default `factor_min_std=0.1`), checked per coordinate so one live coordinate cannot hide a collapsed one. (c) Encoder variance floor on the context states (anti-collapse, default 0.1).
5. `jepa_anything_objective` sums prediction + weighted (1.0 default) orthogonality + factor-activity + encoder-variance terms. It does no optimizer or EMA update; the caller owns the training loop and stop-gradient.
6. Orthogonality mode is `soft_gram` (loss only), `qr_init`, or `qr_retraction` (QR after optimizer steps). No QR runs inside `forward`.

Typical sizes reported: K=4 everywhere shown, r = 96 (vision, DINOv3 features), 128 (single cell), 192 (clinical), 16 with d=64 (molecules), projector (4,128,32) for locomotion (READ, paper extraction).

What it is not (INFERRED): it is not a new architecture. The matched baselines hold adapter, encoder, sampler, budget, split and readout constant, so the claim is that the factorised head and orthogonality terms are what move the numbers (READ). The authors also ship an "unconstrained multi-head" ablation (`UnconstrainedMultiHeadJEPABaseline`, READ in `baselines.py`), which is the right control: it separates "multiple heads" from "orthogonal heads".

## 4. Design discipline worth copying (READ, `docs/architecture.md`)

- Factors get neutral names (`pc_000`), never `speed_latent`. Interpretation is a separate, evidence-linked step after experiments.
- A validator gates scaffold generation: instance-identity keys must match between context and target (leakage check), `K*r=d`, capacity-matched baselines, claims need an experiment and metric. Generative proposal, deterministic gate. This mirrors our own "LLM proposes, validator disposes" gates.
- Checkpoint manifest is metadata-only with SHA-256 per file; the current entry is `availability: not_published`, `training.executed: false`.

## 5. Results

READ (paper extraction; I have not seen the full tables):

- Improves on all 10 matched dynamics tasks versus baseline JEPA models.
- Interventional Pong (CITRIS), five paired seeds, four-channel MSE: single-intervention error 0.009541 to 0.006218 (**34.83%** reduction); combined-intervention 0.009441 to 0.008223 (12.90%); six-step rollout 0.009478 to 0.008665 (8.58%).
- Lowest one-step and 100-step molecular errors across four systems; forecasting over 1,000+ clinical events; biological intervention predictions reported as experimentally validated in cell cultures, organoids, tumor fragments and mice; orbital-mode analysis recovering Keplerian scaling with fitted slope -1.4991 (expected -1.5).

INFERRED caveats: the 34.8% headline is the best of three Pong metrics; the rollout gain is 8.6%. All errors are small in absolute terms. Wet-lab and clinical claims are not reproducible from the public repo (no data or weights; README says so explicitly). Models sizes and compute are "not stated" in the extraction.

## 6. Code, licence, size, dependencies, compute

| Item | Value | Tag |
|---|---|---|
| Licence | Apache-2.0 (root `LICENSE`, also `jepa-anything-core/LICENSE`) | READ |
| Repo size | 3.3 MB on disk (mostly figures) | READ |
| Python, all | 6,519 lines including tests, scripts, recipe | READ |
| Core library src | 2,069 lines: `opf.py` 444, `losses.py` 702, `baselines.py` 385, `audit.py` 429, `__init__.py` 109 | READ |
| Core version | `jepa-anything-core` 0.3.0, "Development Status :: 3 - Alpha" | READ |
| Dependencies | `torch>=2.1`, Python >=3.10; dev: pytest, ruff, mypy strict. Skill/validator tools are stdlib only | READ |
| Weights | None in git; manifest entry `not_published`; HF collection linked from README, not inspected | READ |
| Datasets | None included | READ |
| Training loop | None; "Training: outside Skill" | READ |
| Model sizes / compute | Not stated in the paper extraction | READ |

Licence fit: Apache-2.0 is compatible with WeftOS. Nothing to vendor today.

## 7. Mapping to WeftOS

### 7.1 ECC substrate and voice/ECC predictive loop (ADR-058..061, voice-ecc-synthesis)

READ context: the voice loop runs on a 50 ms `CognitiveTick` that already does "cheap-predict-every-tick, expensive-verify-on-drift" via `run_democritus_loop` (`.planning/voice-ecc-synthesis.md` section B.3). A backchannel is modelled as a `CrossRef` (`Custom(0x60 Continuer)`), not a turn. Turn-taking prediction comes from VAP-style models; Phase 4 is "Weaver-learned floor/coherence".

Concrete mapping (INFERRED):

- **Factorised turn-taking state.** If Phase 4 ever learns a latent conversation state, OPF's idea fits: split it into K slices that are predicted independently (for instance floor/turn-shift, backchannel/continuer, speaker-affect, prosody) and recombined. The existing ECC objects already separate these as impulse, cross-ref and node, so the factors have a natural readout target without any new schema.
- **Activity floor as a collapse alarm.** The per-coordinate std floor is a cheap runtime health check for any learned floor/coherence model, and it would sit next to the democritus drift check. It costs nothing and needs no training in our stack.
- **Neutral naming rule.** Do not label learned slices "turn" or "backchannel" until an intervention-style test (replay with the cue removed) shows the slice moves. This matches the repo's own rule and our evidence-first doctrine.
- Not a fit: the 50 ms tick, AEC, and TTS latency problems are signal-path problems; OPF touches none of them.

### 7.2 BVH world model (ADR-056, ADR-078, ADR-079, ADR-090)

READ: BVH leaves are geometric `(AABB, tag, payload)`; geometry plus chain is truth; learned latents live under ECC as Observation/Impulse only (ADR-090 R1-R5; `vjepa-and-lewm.md` section 3). Visual features land in HNSW `VISUAL_FEATURES` (`index_id = 1`) via `VectorRef`, never in BVH payloads (ADR-088).

Mapping (INFERRED):

- OPF output would be a **predicted latent per factor**, so it is an Observation candidate, never a leaf. `check_wm_write` already rejects `ReasoningOverride`, `CausalEdgeMutate` and `ShortCircuit`; an OPF predictor goes through the same facade, no change.
- A factored latent gives LeWM a typed intervention interface: "predict this slice if I change this action." That is the part of OPF closest to what ADR-078/079 want for rescans and digital-twin "what if" questions, still labelled non-metric.
- Per-factor `VectorRef`s are conceivable (one HNSW namespace per slice) but premature; do not add `index_id` values for slices until a producer exists.

### 7.3 Spatial intelligence 2026 notes

`vjepa-and-lewm.md` already places V-JEPA 2/2.1 as "watch / compose when a producer exists". JEPA-Anything is the same stance one layer up: a training recipe, not an artifact we can load. It does not change that note's doctrine ("never replace BVH"). If anything it adds one rule: any LeWM predictor we train must pass matched-baseline ablations (standard JEPA, unconstrained multi-head) before a MetaHarness receipt can crown it (ADR-096).

## 8. Mapping to rUv-native primitives

I loaded `search_ruvnet` and ran four queries. Coverage was thin for generic phrasing and good once I named artifacts. Only the following is claimed.

- **SONA (ruvector-dag).** CITED(`ruvector/crates/ruvector-dag/src/sona/mod.rs`): module exports `DagSonaEngine`, `EwcPlusPlus`/`EwcConfig`, `MicroLoRA`/`MicroLoRAConfig`, `DagReasoningBank`, `DagTrajectory`/`DagTrajectoryBuffer`. CITED(`.../sona/trajectory.rs`): lock-free `ArrayQueue` trajectory buffer, quality score from execution time and improvement ratio. CITED(`.../tests/integration/sona_tests.rs`): MicroLoRA adapt, EWC penalty rises when parameters drift from consolidated ones. INFERRED: this is a DAG query-optimiser learner (inputs are query DAG embeddings and timings), not a latent world-model trainer. It is a good home for the adapt-and-do-not-forget part (MicroLoRA on a small predictor, EWC++ against drift), not for training an encoder/predictor from scratch. I did not find a SONA crate outside `ruvector-dag` in this search; absence of a hit is not proof there is none.
- **agentdb-learning.** CITED(`agentdb/plugins/agentdb-learning/.claude-plugin/plugin.json`): "9 algorithms (Q-Learning, SARSA, DQN, PPO, Actor-Critic, Policy Gradient, Decision Transformer, MCTS, Model-Based RL)". CITED(`agentdb/ui/.claude/skills/agentdb-learning/SKILL.md`): experiences stored as `{state, action, reward, next_state, done}` patterns with embeddings; `adapter.train`. INFERRED: "Model-Based RL" is the nearest label to world-model learning, but the cited material shows state/action/reward replay, not a JEPA latent predictor. No hit mentions JEPA or latent-space prediction.
- **ruvector latent notes.** CITED(`ruvector/docs/research/latent-space/latent-graph-interplay.md`): discusses latent vs graph reality, GNN encode/decode, mixed-curvature embeddings. No OPF or JEPA content in what returned. The query "ruvector JEPA latent predictive embedding world model" returned only generic embedding wrappers (`npm/packages/ruvector-extensions/src/embeddings.ts`, `examples/onnx-embeddings/src/lib.rs`), evidence rated THIN.
- **Conclusion for rUv fit:** nothing found in rUv-native tools trains or serves a JEPA. Do not claim RVF/SONA/agentdb "supports JEPA". The credible rUv role is storage (RVF/HNSW for per-factor targets or embeddings), checkpointing/branching of experiments, and a SONA-style adapt-with-EWC wrapper around a small predictor, all INFERRED and unverified end to end.

## 9. What we would build first if adopted

Small, Rust-side, no PyTorch dependency, no training:

1. **Geometry audit as a Rust utility** (INFERRED effort: a few hundred lines). Given a `(K, r, d)` projection matrix, report orthonormality error, pseudoinverse round-trip error and per-coordinate std. This mirrors `audit.py` and gives us a deterministic gate we can run against any future learned basis. Home: next to `clawft-kernel::lewm_invariant` or a `weftos-worldmodel-*` crate.
2. **Activity-floor monitor** on whatever latent the voice/ECC Phase 4 model emits, wired to the democritus drift tick.
3. **Matched-baseline harness requirement** written into the LeWM plan: any claim that a factorised predictor helps must include the unconstrained multi-head control at equal parameters and FLOPs.
4. Only then, a toy: reproduce the repo's synthetic-linear-dynamics recipe in the `~/llm` lab (Python allowed there) to see the loss terms behave, before any WeftOS data is touched.

## 10. Open questions

- What does the paper report for model sizes and compute? Not found in my extraction; read the full PDF tables before costing a lab run.
- Is the benefit mostly from the orthogonality terms or from the activity floors? The ablation granularity in the paper is unverified by me.
- How do results hold with small d (embedded-scale latents)? Molecules use d=64, K=4, r=16, which is the nearest to edge sizes, but that is one domain.
- Do HF weights exist for any domain, and under what licence? README links a collection; I did not inspect it.
- Is a fixed K=4 a tuned choice or a convention? All reported setups use K=4.
- Which of our latents (voice floor state, sensor-fusion state) even have separable "intervention" structure to factorise? OPF assumes some.

## 11. Sources

- Paper: https://arxiv.org/abs/2609.20800 (abstract) and https://arxiv.org/html/2609.20800 (method, K/r, Pong numbers via extraction).
- Code: https://github.com/Gen-Verse/JEPA-Anything, files read: `README.md`, `docs/architecture.md`, `checkpoints/README.md`, `checkpoints/manifest.json`, `jepa-anything-core/pyproject.toml`, `jepa-anything-core/src/jepa_anything_core/{opf,losses,baselines}.py`.
- WeftOS: `docs/adr/adr-056-bvh-spatial-index.md`, `adr-061-conversational-voice-agent-loop.md`, `adr-078-splat-feeds-world-model.md`, `adr-090-lewm-ecc-decoupling-invariant.md`, `docs/research/spatial-intelligence-2026/vjepa-and-lewm.md`, `.planning/voice-ecc-synthesis.md`. ADR-058 to 060 and ADR-079 were listed but only skimmed or not read in full.
- rUv (search_ruvnet): paths as cited in section 8.
