# Cluster synthesis: benchmarks-and-rl (Skill-3D refs 7, 11, 13, 14, 15, 18, 43, 52, 70, 73, 74, 80, 81, 82, 92)

Covers the spatial/physical benchmarks and the RL-for-LLM/VLM methods Skill-3D cites. See individual files in `papers/analysis/` for full per-reference detail; this file is the cross-cutting read for WeftOS decision-making.

## Reference table

| # | Short name | Type | License | Skill-3D uses it as | Verdict |
|---|---|---|---|---|---|
| [7](analysis/007-rrvf-visual-rl-images.md) | RRVF (Learning Only with Images) | RL method | arXiv nonexclusive | related-work only (§2.2) | PATTERN |
| [11](analysis/011-physbench.md) | PhysBench | Benchmark (physical understanding) | CC BY 4.0 | related-work only (§2.1) | WATCH |
| [13](analysis/013-deepseek-r1.md) | DeepSeek-R1 | RL method (GRPO origin, applied) | model weights public | GRPO source for Skill-3D's own RL (§1, §3.3) | ADOPT (recipe) |
| [14](analysis/014-arpo.md) | ARPO | RL method (tool-use) | repo public, license unconfirmed | related-work only (§2.2) | PATTERN |
| [15](analysis/015-agent-rrm-reasoning-reward.md) | Agent-RRM / Reagent | RL reward model | repo public, license unconfirmed | related-work only (§2.3) | WATCH |
| [18](analysis/018-blink.md) | BLINK | Benchmark (perception, multi-view) | **CC BY-NC-SA 4.0** | **eval benchmark** (multi-view subset) | ADOPT (non-commercial only) |
| [43](analysis/043-openeqa.md) | OpenEQA | Benchmark (embodied QA, glasses-relevant) | code MIT, data license unconfirmed | related-work only (§2.1) | ADOPT (glasses-eval anchor) |
| [52](analysis/052-deepseekmath-grpo.md) | DeepSeekMath | RL method (GRPO origin) | unconfirmed | GRPO source for Skill-3D's own RL (§1, §3.3) | ADOPT (the algorithm) |
| [70](analysis/070-papo.md) | PAPO | RL method (perception-aware) | unconfirmed | related-work only (§2.1) | PATTERN |
| [73](analysis/073-spatialscore.md) | SpatialScore | Benchmark (unified spatial) | promised open, unconfirmed | related-work only (§2.1) | WATCH |
| [74](analysis/074-vilasr-interwoven-thinking.md) | VILASR (interwoven thinking + drawing) | RL method (visual CoT) | **CC BY-NC-ND 4.0** | related-work only (§2.1) | SKIP dependency / PATTERN idea |
| [80](analysis/080-vsi-bench-thinking-in-space.md) | VSI-Bench / Thinking in Space | Benchmark (indoor egocentric spatial) | CC BY 4.0 | **eval benchmark**, headline result (+43% Qwen3-VL-8B) | ADOPT (via ReVSI) |
| [81](analysis/081-visionthink.md) | VisionThink | RL method (resolution efficiency) | code public, **CC-BY-SA 4.0** | related-work only (§2.2) | PATTERN |
| [82](analysis/082-mmsi-bench.md) | MMSI-Bench | Benchmark (multi-image spatial) | CC BY 4.0 | **eval benchmark**, headline result (+67% Gemini-3-Flash) | ADOPT |
| [92](analysis/092-revsi.md) | ReVSI | Benchmark (corrected VSI-Bench) | CC BY 4.0 | related-work only (§2.1) — **not used in Skill-3D's own eval**, a gap | ADOPT |

Note on dates: several citations in this cluster (Agent-RRM [15], ReVSI [92]) carry 2026 preprint dates, consistent with Skill-3D's own 2606.xxxxx (2026-06) arXiv ID — this is a genuinely recent paper citing genuinely recent work, not a dating error on my part. Treat single-source abstract-page extraction for these as provisional; re-verify before hard commercial commitments.

## Which benchmarks WeftOS should use to evaluate its Rust implementation

**Use directly (CC BY 4.0, commercial-safe):**
- **MMSI-Bench [82]** — multi-image spatial intelligence, expert-authored, one of Skill-3D's two headline benchmarks. Highest priority: covers camera-camera/camera-object/object-region relations that a multi-view or glasses-capture agent needs, and ships a 4-mode error taxonomy (grounding / overlap-matching / situation-transformation / spatial-logic errors) worth reusing as WeftOS's own failure-diagnosis categories.
- **ReVSI [92] in preference to raw VSI-Bench [80]** — same task taxonomy as VSI-Bench (counting, distance, size, direction, route planning) but with point-cloud annotation artifacts fixed and frame-budget variants (16/32/64/all) that let WeftOS score under the *actual* sparse-sampling regime a deployed agent uses, instead of an idealized full-scene-access assumption. This directly serves the "honest geometry" mandate — do not silently reuse VSI-Bench's original, known-flawed annotations. Skill-3D itself doesn't use ReVSI (still reports raw VSI-Bench numbers), which is a gap in Skill-3D's own rigor WeftOS shouldn't repeat.

**Use with the glasses use case in mind:**
- **OpenEQA [43]** — the only benchmark in this set explicitly framed around egocentric/glasses-style capture (its "episodic memory" setting). Open-vocabulary/free-form rather than multiple-choice, so it tests real deployed-agent answer generation, not a 4-way guess. Repo is archived/read-only (as of 2025-11-01) — fork/vendor the harness rather than depending on upstream. MIT-licensed code; data license unconfirmed, verify before redistribution.

**Use only for non-commercial research comparison:**
- **BLINK [18]** — CC BY-NC-SA 4.0. Skill-3D uses its multi-view subset as an eval; WeftOS can replicate that for internal benchmarking/paper-comparison purposes but **cannot ship BLINK data, or any model/checkpoint substantially derived from BLINK-based training, in a commercial product** without a separate license.

**Avoid or defer:**
- **VILASR / "interwoven thinking" [74]** — CC BY-NC-ND 4.0 (no derivatives, non-commercial). Do not use its code, weights, or data at all commercially; the visual-drawing-as-reasoning *idea* is fine to reimplement independently from scratch.
- **PhysBench [11]** and **SpatialScore [73]** — both plausible future additions (PhysBench for physical-dynamics claims, SpatialScore as a 30-task aggregator with a tool-augmented SpatialAgent baseline worth studying architecturally) but neither is in Skill-3D's own eval suite, and SpatialScore's exact license/release terms are unconfirmed — revisit once actually released rather than committing now.

## Does WeftOS need an egocentric-glasses evaluation set, and what would it contain

Yes. None of the five spatial benchmarks Skill-3D evaluates on or cites (VSI-Bench/ReVSI, BLINK, MMSI-Bench, OpenEQA, PhysBench) are built primarily around consumer smart-glasses capture — OpenEQA is the closest fit (its episodic-memory setting is explicitly glasses-motivated) but is still built from pre-recorded scanned environments (HM3D-family scenes), not live MentraOS capture. A WeftOS-native glasses eval set should combine:

1. **ReVSI's methodology, not its data** — the frame-budget-variant idea (score under 16/32/64/all-frame sampling, matched to what the glasses pipeline actually captures and retains) applied to real MentraOS egocentric footage.
2. **OpenEQA's task framing** — open-vocabulary QA scored by an LLM-match protocol rather than multiple-choice, since real glasses-agent deployment answers are free-form, not 4-way guesses. Fork the harness rather than depending on the archived upstream repo.
3. **MMSI-Bench's multi-image relation taxonomy** — camera-camera and camera-object relations are exactly what a moving glasses wearer generates as they look around a scene; reuse the taxonomy (not the data) to define WeftOS-specific question templates.
4. **A metric-honesty axis absent from all five source benchmarks** — none of them explicitly score whether a spatial claim is traceable to a real metric source (depth sensor, SLAM scale, stereo baseline) vs. a plausible-looking hallucinated number. Given WeftOS's "honest geometry" requirement, this should be a first-class scored dimension in the new eval set, not an afterthought.

This is a build item, not a "use an existing set" item — flag for planning rather than treating as solved by adoption of the above benchmarks.

## The RL recipe: SFT → GRPO (Skill-3D's own pipeline) vs. alternatives

Skill-3D's own agentic post-training (paper-2606.07436.txt §3.3, lines 191–212) is:

1. **Agentic SFT** on skill-guided trajectories — teaches format (skill retrieval → tool invocation → evidence accumulation), not just answer imitation; gives RL a stable starting policy.
2. **Agentic RL via GRPO** [13, 52] — for each query, sample a group of `G` full trajectories `{τ⁽¹⁾,...,τ⁽ᴳ⁾}` (each containing the model's own skill choices, tool calls, tool outputs, reasoning, final answer), score each with:

```
R(τ) = R_ans(τ) + R_fmt(τ) + R_tool(τ)
R_tool(τ) = R_exec(τ) − |A|/B
```

where `R_ans` = answer correctness, `R_fmt` = structured-format compliance, `R_exec` = binary reward for obtaining benchmark-required evidence (determined by a frozen task parser, *not* by which skill the model picked — this is a deliberate anti-reward-hacking design choice, preventing the policy from picking easy skills to farm the tool-efficiency term), `|A|` = number of tool calls used, `B` = max tool budget. This scalar reward feeds the canonical GRPO update (group-normalized advantage, PPO-style clipped surrogate, KL penalty to reference — written out in full in [[analysis/052-deepseekmath-grpo]]).

**Alternatives surveyed in this cluster, and what each would add on top of Skill-3D's baseline recipe:**
- **ARPO [14]** — entropy-triggered adaptive rollout + step-level credit assignment, instead of uniform group sampling. Relevant if WeftOS's tool-call trajectories are long/branchy (many tool calls per query) where uniform-budget sampling wastes rollouts on already-confident steps.
- **PAPO [70]** — adds an Implicit Perception Loss (KL-divergence term) plus a Double Entropy Loss on top of the base GRPO objective, specifically to separate perception errors from reasoning errors. Directly relevant to "honest geometry": WeftOS should consider adding an analogous perception/grounding-consistency term rather than relying on `R_ans` alone to implicitly capture perception correctness.
- **Agent-RRM / Reagent [15]** — replaces the scalar reward with a learned reward *model* that emits a reasoning trace + critique + score. Heavier (needs its own training data/model) — defer until WeftOS has enough labeled failure trajectories to train one; not a day-one recipe component.
- **RRVF [7]** — reward computed from *verifying a rendered output against a source image* rather than from ground-truth labels at all. Relevant as a label-free reward pattern for any WeftOS tool whose output can be re-rendered/re-projected and compared to source imagery (e.g., a geometry estimate checked by reprojection).
- **VisionThink [81]** — RL-learned resolution/capture-fidelity decisions (correctness reward minus a calibrated over-request penalty). Directly relevant to glasses-side compute/bandwidth-constrained capture; reimplement the reward-shaping pattern rather than reuse the CC-BY-SA-licensed code (share-alike obligation is a real constraint for a commercial product).

**Recommended default for WeftOS:** start with Skill-3D's own recipe verbatim (SFT → GRPO with `R_ans + R_fmt + R_tool`), since it's already proven on the exact task family (tool-using spatial-reasoning agents) WeftOS is porting. Layer PAPO-style perception-consistency reward and VisionThink-style capture-fidelity reward shaping on top as the two most directly relevant extensions, given WeftOS's metric-honesty and glasses-capture priorities respectively.

## What the Rust side must emit for Python-side GRPO training

Given training stays in Python (per the WeftOS split: Rust produces trajectories, Python trains), the Rust agent needs to serialize, per query, a **trajectory group** with:

- `query_id`, the input (question + visual observations + retrieved skill/tool candidates)
- For each of `G` sampled trajectories: an ordered list of steps, each step tagged with — action taken (skill selection, tool call + its arguments, or a reasoning/answer token span), the tool's raw output, and (for ARPO-style step-level credit and PAPO-style perception loss) the model's token-level logprobs/entropy at that step if available
- A final answer plus the components needed to compute `R_ans`, `R_fmt`, `R_tool` independently in Python (or precomputed reward values, if Rust computes them directly against ground truth) — i.e., don't collapse to a single scalar too early; keep `R_ans`/`R_fmt`/`R_exec`/`|A|`/`B` as separate fields so reward-shaping experiments (PAPO-style additions, ARPO-style step attribution) don't require re-running inference
- Enough metadata to reconstruct which frames/tool outputs were actually visible to the model at each step (needed for ReVSI-style "answerable under actual input" scoring and for any future perception-consistency reward term)

This schema is a superset of what Skill-3D's own Eq. 1–2 reward needs, deliberately, so the same trajectory log can support the PAPO/ARPO-style extensions above without a second instrumentation pass.
