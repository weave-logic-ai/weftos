# MatImageAgent: A Large Language Multimodal Agent for Materials Characterization Images — Deep Read

Source: Jiang, AitTamerd, He, Bi, Zhao, Li, Yang. *Materials Genome Engineering Advances*
4(3), e70058 (first published 14 March 2026, print issue August 2026), DOI
[10.1002/mgea.70058](https://doi.org/10.1002/mgea.70058). Open Access (Wiley). Full text
fetched directly from the publisher page (`onlinelibrary.wiley.com/doi/full/10.1002/mgea.70058`)
via a real browser session — the DOI resolver and `curl` both hit a Cloudflare bot
challenge that a normal fetch cannot pass, so cite the browser-rendered page, not a cache.
Code/data repo: `https://github.com/menghaoyoung/MatImageAgent`. This is a full read, not
an abstract-only summary.

Shared this session via `https://synapsesocial.com/papers/69b79e488166e15b153ab5a8`, which
carries only the title/authors/DOI and three headline numbers, not the paper itself.

## 1. What it does

MatImageAgent automates the **post-processing** stage of materials characterization
imaging: given a human-written task description (TD) and a folder of images, it writes
and runs its own Python, rather than calling a fixed library of pre-built vision tools.
The stated motivation is that materials scientists "manually executing their created code
and prompting large language models" is the bottleneck standing between raw greyscale
images and a finished analysis.

**Modalities and tasks**, all sourced from previously published solid-state-battery and
polymer-film studies (§4):

- **SEM (Task 1)** — void detection and height measurement at the Li/solid-electrolyte
  interface. A pixel is a void if greyscale is 5–30 *and* it sits in a contiguous
  ≥20-pixel adjacent block. Height = `(row_max − row_min + 1) × re`, `re` = image
  resolution in µm/pixel — a TD constant, not measured from the image (§3).
- **XCT (Task 2)** — greyscale-vs-distance profiling along a hand-specified line segment,
  with an 8-bit→16-bit remap (`G8bit = 255·(G16bit − Gmax)/(Gmax − Gmin)`, `Gmax=65535`,
  `Gmin=0`, both fixed TD constants) to reveal lithium deposition in electrolyte cracks.
- **AFM (Task 3)** — CLAHE enhancement then rule-based phase marking (micro phase white,
  macro phase black) of polymer thin films, same greyscale+adjacency logic as Task 1 with
  different thresholds (1–150, ≥25-pixel block).

Each task splits into 4–5 sub-steps (e.g., T1-A "CSV of per-pixel greyscale" … T1-D "full
research report"), graded independently for stability accounting (§4).

**Users**: materials scientists who "find it challenging to apply artificial intelligence
techniques to their own research" and lack "advanced programming skills" — the paper's own
framing, echoed in the GUI they built for non-programmers to upload images and TDs.

## 2. Architecture

No planner/router chooses among a fixed tool set per image type. The architecture is a
bounded, domain-narrowed coding-agent loop, not a tool-calling agent:

1. **Human authors the TD** — a `.txt` "Mission/Task Description" with objective, image
   spec/path, analysis parameters (thresholds, resolution constants), and output
   requirements (`Task_Guide.txt` template walks non-experts through it). All domain logic
   — thresholds, the void-height formula, the 8/16-bit remap — lives in the TD's *text*,
   authored by a person, never learned or retrieved by the agent.
2. **LLM decomposes the TD into subtasks** and generates Python via multi-turn,
   multimodal (image + text) API calls.
3. **Programs execute locally** (`python Agent.py -s MyTD.txt`).
4. **Completion check** — did the TD's stated outputs (CSV, marked image, report) appear.
5. **DebugAgent (verifier)**, added after the base pipeline showed poor stability (§4):
   locates every generated program, executes it, captures error logs, feeds
   `(TD, program, log)` back to the LLM for a repair pass, re-saves, re-executes. This is
   the paper's one architectural addition beyond "LLM writes and runs code," evaluated on
   its own below.

**LLMs used** (three, swappable, no fine-tuning): GPT-4.1, Claude-3.7, DeepSeek-R1.

**Tool set**: effectively none, in the classical-CV-library sense — the "tools" are
whatever Python the LLM writes per run (NumPy/PIL/OpenCV-style ops, CLAHE, matplotlib,
python-docx). SAM3 appears only as an *external comparison baseline* for Tasks 1/3
(text-prompted "black area"), never as a callable tool inside the agent. There is no
fixed perception-tool API, no tool-selection gate, no per-modality routing — one generic
decompose→generate→execute→check loop is reused for SEM, XCT, and AFM, with all
modality-specific behavior pushed into the human-authored TD instead of into architecture.

**Memory/knowledge base**: none beyond the live conversation (TD + generated code +
debug logs in the multi-turn context). No vector store, no retrieval, no cross-run skill
library, no persistent lessons — each of the 10 stability reruns starts cold. Sharp
contrast with SciVisAgentSkills' versioned `SKILL.md` packages and Skill-3D's Scene
Memory + promote/merge skill library; see §6.

**Human-in-the-loop**: load-bearing at design time (every threshold/constant hand-written
into the TD before the run; the agent never infers these or asks a clarifying question)
and at evaluation time (`Height_act` hand-annotated per SEM image; AFM phase masks
hand-drawn by three materials scientists P1–P3, averaged for "accuracy"). No
approve/reject step on individual outputs mid-run — the GUI lets a non-programmer submit
and collect results, not steer them.

## 3. Scale and units handling

The section most relevant to WeftOS's honest-geometry doctrine, and the paper's practice
falls short of it in a specific, checkable way.

- **No scale-bar reading, no metadata parsing.** Pixel-to-physical-length conversion is a
  single constant the *human* types into the TD (`re` µm/pixel for SEM; "each pixel on
  the line segment represents 0.9 μm" for XCT). The agent never reads an embedded scale
  bar, OCRs a caption, or pulls calibration from image metadata (EXIF/TIFF, instrument
  logs). Nothing verifies the TD's stated resolution matches the image actually supplied
  — a mismatch would silently propagate into every downstream measurement.
- **Grey-value remap constants are likewise hand-supplied, not derived**: `Gmax = 65535`,
  `Gmin = 0` for the 16-bit XCT source are typed in as fixed values for that one dataset,
  not read from the file's actual bit depth.
- **No uncertainty propagation.** The only quantitative error measures are a single
  aggregate MAE (SEM heights) and a single aggregate accuracy (AFM masks), both against
  manual ground truth — no per-image confidence interval. All three LLMs get exactly the
  same 0.11 µm MAE on Task 1 because the deterministic pixel *code*, not the vision, does
  the measuring — determinism is not the same claim as accuracy. Box plots of void height
  by current density (Figure 2e) are the closest thing to a distributional report, and
  even those compare medians, not calibrated error bars.
- **Net read for WeftOS**: MatImageAgent has *no verified scale source* in the sense
  Skill-3D's `weftos-adaptation-plan.md` §3.2 requires (`metric_basis ∈ {survey, stereo,
  lidar_tof, yardstick, mono_predicted, scale_free, none}`). Every measurement it reports
  would classify as unverified/`none`-provenance under that scheme — the resolution
  constant is trusted input, not evidence. Cite this as a concrete negative example for
  the provenance envelope (§3.2) and the "non-coercible units" rule (§3.5): a
  `MetricDistance` built on a hand-typed constant with no calibration record is exactly
  the failure those typed boundaries exist to catch before it reaches a report.

## 4. Evaluation

**Datasets** (all reused from prior published studies, not newly collected): SEM — 11
in-situ images of Li-stripping voids at current densities 0–1.25 mA·h/cm² (Yao/Zhao et
al.); XCT — solid-electrolyte cross-sections from Bruce/Ning et al.'s operando study
(original unmarked images unavailable, so the paper re-selected a line segment offset
"5 µm below and to the right" of the source); AFM — 5 images from Jayaraman et al.'s
public POEGMA-*sb*-PS polymer-film dataset, plus one severely degraded image used only
for a qualitative failure-mode illustration.

**Baselines**: Meta's web-based SAM3 (Segment Anything Model 3), prompted with the text
"black area," for Task 1 (void segmentation) and Task 3 (AFM phase segmentation). Also an
implicit effort/accuracy comparison against Jayaraman et al.'s own bespoke unsupervised
pipeline (~2000 lines of custom code, 0.85 accuracy) vs. MatImageAgent's 41-line TD (0.82
accuracy).

**Metrics**: MAE in µm (Task 1 void height, Eq. 2); pixel-level accuracy against
multi-annotator manual labels (Task 3, Eq. 4, averaged over annotators P1–P3); completion
ratio over 10 independent reruns per task/model (stability, §3.4).

**Results — verbatim numbers**:

| Result | Value |
|---|---|
| SEM void-height MAE, all 3 LLMs (Task 1, T1-D) | 0.11 µm — *identical* across GPT-4.1, Claude-3.7, DeepSeek-R1 |
| AFM phase-marking accuracy, MatImageAgent (Task 3) | 0.82 (average vs. 3 human annotators; each >0.8) |
| AFM phase-marking accuracy, SAM3 baseline | 0.886 (better than the agent) |
| AFM phase-marking accuracy, Jayaraman et al.'s custom ~2000-line pipeline | 0.85 (better than the agent, at ~50× the code) |

**Table 1 — task completion over 10 reruns (all 3 LLMs pooled, best-of-10 used for the
headline accuracy numbers above; this table is the stability accounting)**:

| Task | Complete/all | Ratio |
|---|---|---|
| Task 1 (SEM) | 62/120 | 0.52 |
| Task 2 (XCT) | 116/200 | 0.58 |
| Task 3 (AFM) | 61/120 | 0.51 |
| **Overall** | **239/440** | **0.54** |

Per-model qualitative stability notes (Figure 5, §3.4, text-only — no separate per-model
numeric table beyond Table 1's pooled figures): DeepSeek-R1 was most stable on Task 1;
Claude-3.7 had zero partial completions on Task 2 and was best on Task 3, but on Task 1 it
fully completed the *entire* task chain only once in 10 runs because it frequently
"generates code and does not run it" (an execution-omission failure specific to that
model); GPT-4.1 estimated void height and generated marked images accurately but produced
only one fully correct *report* across its runs.

**DebugAgent ablation**: re-running Task 3 with Claude-3.7 plus the DebugAgent verifier
loop for 10 reruns raised completion from the baseline ~0.51 pooled ratio to **10/10
(100%)** — the paper's clearest evidence that a self-repair loop, not a better base model,
is what closes most of the stability gap.

**Reported failure modes** (§4 Conclusions, plus scattered through §3):
1. **Domain-knowledge ceiling** — understanding is bounded entirely by what the TD
   states; no reasoning about materials-science context beyond it.
2. **TD-requirement omission** — missing/incomplete reports, or a written-but-never-
   executed Python file (the Claude-3.7 Task 1 pattern above); even with DebugAgent, some
   generated programs still fail to execute.
3. **Multimodal-heavy tasks are weaker** — Tasks 1/3 (new marked *images*) scored lower
   completion (0.51–0.52) than Task 2's pure numeric/plotting pipeline (0.58); authors
   attribute this to higher multimodal-generation demands.
4. **No specialized denoising** — on one severely degraded AFM image, generic
   CLAHE+threshold code left noise artifacts a dedicated AFM denoiser would avoid.
5. **Report scope drift** — with nothing in the TD constraining narrative framing, the LLM
   invented an Abstract/Introduction ("microstructural analysis of lithium batteries")
   diverging from the actual study — unprompted fabrication risk in auto-generated prose.
6. **Segmentation bias vs. SAM3** — at high current density, SAM3 over-segments non-void
   edges (height biased high) while MatImageAgent under-marks them (biased low); opposite
   failure directions, neither ground-truth-accurate.

## 5. Code, data, and license availability

- **Data**: "openly available … at `https://github.com/menghaoyoung/MatImageAgent`" (the
  paper's Data Availability Statement) — raw SEM/XCT/AFM images are re-hosted there, not
  just cited to the three source papers.
- **Code**: same repo, mainly `MatImageAgent_V2/` (DebugAgent verifier + its 10-cycle
  rerun logs) plus the base pipeline in `MatImageAgent_Project/Core_code/MatImageAgent.py`.
  A `Developer's Guide.docx` and `Task_Guide.txt` support TD authoring; a GUI is described
  for non-programmer use. The live demo is a bare IP address
  (`http://121.4.77.184:4001/`) — treat as unstable, not a durable reference.
- **License**: **none set**. `gh api repos/menghaoyoung/MatImageAgent` → `"license": null`,
  no `LICENSE` file in the tree, and the README's own "Paper Information" section reads
  "Waiting for updating." Public/forkable under GitHub defaults, no explicit reuse grant —
  the same gap flagged for `KuangshiAi/SciVisAgentSkills` in
  `paper-scivis-agent-skills.md`. Do not vendor into WeftOS without resolving this first.
- **Paper itself**: Wiley Open Access; the specific CC variant was not stated in the
  rendered article text captured here — confirm before quoting more than short excerpts.

## 6. WeftOS relevance

**(a) Episteme STEM skill pack and its router agent**
(`docs/research/episteme/adoption-notes.md`). MatImageAgent is a useful negative example
for the router-agent design modeled on Higgsfield's `higgsfield-generate`: it has *no*
router — one generic loop is reused for three modalities purely because a human wrote
different thresholds into three different TDs. That works at N=3 hand-tuned tasks and
visibly degrades: completion is lowest on the two tasks (SEM, AFM) needing new-image
generation, and Claude-3.7 silently skips execution on Task 1 under load. If Episteme's
router dispatches across many more STEM skills than this, it should not copy
"push all modality logic into the prompt" — it needs to select and compose fixed, tested
tools (as `database-lookup` and the pure-analysis skills in the Episteme core tier already
do) rather than re-deriving CV logic per run. The DebugAgent result (0.51→1.00 completion
on one task) is the strongest transferable idea: a cheap self-repair pass on generated
code beats a stronger base model, and Episteme's skill-execution layer should adopt it
regardless of domain.

**(b) Skill-3D's image-analysis tool layer** (`weftos-adaptation-plan.md` §3.5).
MatImageAgent resembles the "code escape hatch" §3.5 describes for a narrowly scoped,
capability-gated `code_executor` — except it uses that hatch as its *only* execution
path, with no typed tool layer or non-coercible-unit types beneath it. §3.5's rule that
"any downstream output inherits `restored_input`/`scale_free` unless it cites a metric
input" is exactly the discipline missing in §3 above: MatImageAgent's void-height and
grey-value outputs are `MetricDistance`-shaped numbers built on a hand-typed constant with
no calibration record — under Skill-3D's honesty contract those would be tagged
unverified, not reported as µm. If WeftOS wires SEM/XCT/AFM-style pixel-rule measurement
into `clawft-perception`, it should require `ObservationProvenance.scale_source`
(survey/calibration-file/none) rather than accept a bare resolution float from a prompt,
keeping the TD-as-code-gen pattern (useful for one-off analyses) strictly behind that
gate, never as the default measurement path.

**(c) Agent Directory's shared image-analysis skills**. MatImageAgent's TD format (a
hand-written mission file with objective / image spec / analysis parameters / output
requirements, plus a `Task_Guide.txt` for authoring one) is a plausible seed for a
`materials-imaging` skill's parameter-elicitation prompt, but the Directory should not
copy the architecture: no memory, no skill reuse across runs, no fixed tool catalog, and a
54% raw completion rate (Table 1) even with three frontier LLMs. A shared image-analysis
skill should sit on a typed, tested tool layer — as SciVisAgentSkills' four `SKILL.md`
packages do, and Skill-3D's `PerceptionBackend` trait is meant to — rather than asking the
model to write fresh pixel-processing code per invocation; that gap is the direct cause of
MatImageAgent's 0.5–0.6 stability ceiling without the DebugAgent add-on.

**Comparisons**. SciVisAgentSkills (`paper-scivis-agent-skills.md`) is the opposite bet —
version-pinned `SKILL.md` packages give a coding agent *fixed* domain API knowledge
(ParaView, napari, VMD, TTK) rather than asking it to reinvent the tool, and reports
higher mean scores over the same base harnesses. MatImageAgent is effectively the
no-skill-library control case: same LLM+generated-code+local-execution idea, no
persistent procedural layer, and lower, more model-dependent completion (0.51–0.58 per
task). Both papers converge on the same lever for WeftOS: a debug/verify loop plus a
reusable, version-pinned knowledge layer move completion/accuracy far more than the choice
of base LLM. Against Skill-3D's evolved skills (`skill-3d/papers/paper-skill-3d.md`):
Skill-3D retrieves skills via dense embedding + rerank + MMR, cited through
`<skill_choice>`, with success/failure lessons persisted across rollouts — MatImageAgent
has none of that; every one of its 10 stability reruns is architecturally identical to the
first, consistent with why its completion ratio never trends upward across reruns the way
a lesson-accumulating system would.
