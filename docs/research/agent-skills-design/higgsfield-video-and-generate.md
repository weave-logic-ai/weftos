# The "video analyzer" and the "image gen agent" — dissected

Source: `higgsfield-ai/skills` v0.13.0, cloned read-only at `~/dev/higgsfield-skills`. Companion doc: `higgsfield-format-standard.md` (repo-wide conventions). This doc answers two things: which skill the user meant by "the video analyzer is gold," and how `higgsfield-generate` works as an "image gen agent" — then maps both patterns onto Episteme and a WeftOS/Skill-3D MentraOS footage analyzer.

## 1. Which skill is "the video analyzer"

Checked, in the order the brief suggested:

- **`higgsfield-video-explainer`** — despite the name, this is a *video generation* skill (topic/document → narrated MP4), not an analyzer. Its one analysis-adjacent step is Phase R (web research to verify facts before scripting) — that's fact-checking prose, not video analysis. Ruled out as the referent.
- **`higgsfield-generate/references/media-inputs.md`** — documents *how to pass* a video as a reference input (image-to-video, `--video` role on Seedance) and, separately, describes the one true video-*analysis* feature (see below). Not itself an analyzer.
- **`higgsfield-brandkit/references/asset-analysis.md`** — analyzes *brand assets* (logos, SVG/CSS, PDF/PPTX, fonts, raster images) to build a locked brand spec. No video handling at all. Ruled out.
- **Virality Predictor (`brain_activity`)**, documented inside `higgsfield-generate/SKILL.md` (§"Virality Predictor video scoring") and cross-referenced from `media-inputs.md` and `troubleshooting.md` — **this is the actual video analyzer.** It takes a finished video in, returns a scored text report out: overall score, peak hook second, sustain score, region-level breakdown (Visual/Auditory/Language/Attention/Default Mode cortex-style regions), a business interpretation, and an "Open report" URL for the visual version. It is not a separate top-level skill folder — it's a fully-specified capability living inside `higgsfield-generate`, given its own customer-facing name distinct from its technical id specifically so agents don't misroute it as a text/chat model just because its output is text.

**Determination:** "the video analyzer" = Virality Predictor (`brain_activity`), inside `higgsfield-generate`. It's the one skill artifact in the repo whose entire job is video-in → judged, structured, business-readable report-out, which is what "gold" is most plausibly praising: a well-specified analysis contract, not a generation contract.

## 2. Why Virality Predictor works well

### Workflow steps (as specified in `SKILL.md` + `references/media-inputs.md`)

1. **Classify by intent, not output type.** The skill explicitly warns against the natural misclassification: *"Do not misroute video analysis because the output is text... route to `brain_activity` even though it appears under text/analysis models. Classify by task intent and required input, not by output category alone."* This single rule is why the feature is discoverable at all — an agent scanning by "what does this produce" would file it under text generation and never surface it for "analyze this ad video."
2. **No prompt, one required media role.** `brain_activity` accepts exactly `--video` and needs no `--prompt`. Documented explicitly as an error case: `Missing required params: medias` → "pass exactly one video via `--video <path-or-id>`." The absence of a prompt is treated as a first-class fact about the model, not an omission.
3. **One-shot submit and wait.** `higgsfield generate create brain_activity --video ./ad.mp4 --wait` — same one-shot pattern as every other model in the repo (§4 of the format-standard doc), no special-casing for analysis vs generation at the CLI-invocation layer.
4. **Deliver via a fixed report shape**, not raw output. The skill specifies the *exact* delivery template:

   ```text
   Overall score: 44/100
   Peak hook: 49% at 1s
   Sustain: 89%
   Strongest region: Visual Cortex
   Risk: Default Mode is high, which can indicate mind-wandering.

   Open report: <report_url>
   ```

   And a matching suppression rule: raw artifact URLs (`brain_example_url`, `vertexMapBinaryUrl`, `vertexMapUrl`) and `.glb`/`.bin` render artifacts stay in JSON/debug output — never surfaced in normal chat unless the user asks for implementation detail.

### The analysis rubric

Five named regions/scores, each treated as an *interpreted signal*, not a raw number: Visual, Auditory, Language, Attention (higher = stronger stimulus / sharper focus), and Default Mode (lower is better — high Default Mode "suggests less mind-wandering" is inverted from the others, and the skill states the inversion explicitly rather than leaving it for the reader to infer). Plus two summary metrics — Peak hook (the single best second) and Sustain (how well attention holds after the peak) — which together answer the two questions an ad buyer actually asks: *does it grab, does it hold.*

### Prompt patterns

There are none, and that absence is itself the pattern worth copying: a well-designed analyzer skill doesn't force a prompt-engineering surface onto a task that has no free-text component. The "prompt" is replaced entirely by the media input plus the fixed rubric.

### Output structure

Score block → strongest/weakest region → one risk sentence in plain business language → a link out to the full visual report. Four lines, one link, zero internal jargon, one interpretive sentence that turns a number into a decision-relevant statement. This is the shape worth carrying forward verbatim (§4 below).

## 3. `higgsfield-generate` as the "image gen agent" pattern

`higgsfield-generate` is the biggest skill in the repo (322 lines) because it is genuinely a router across four media types (image/video/3D/audio) plus two sub-domains (Marketing Studio, Virality Predictor) — yet it stays under the 300-line ceiling by pushing everything enumerable into `references/`.

### How it routes

Two-tier routing, repeated per media type (image, video, 3D, audio) in both `SKILL.md` (compact) and `references/model-catalog.md` (exhaustive):

- **Auto-pick tier** — a numbered, priority-ordered list of `condition → model`, evaluated top-to-bottom, "higher entry wins" on a tie. E.g. for images: complete brand identity → `higgsfield-brandkit` (hand off, don't handle); YouTube thumbnail → `higgsfield-youtube-thumbnail`; Soul Character reference present → Soul 2.0/Soul Cinema; cartoon/illustrated → Nano Banana 2; **default for everything else → GPT Image 2.5.** Five named exceptions before the catch-all default — this is the actual shape of "smart routing": a short list of high-precision carve-outs sitting in front of a wide, low-precision default.
- **Named-only tier** — models used only when the user explicitly asks for them or names them by name, and once named, "stays in use for follow-ups on the same work" (sticky routing — the agent doesn't re-run the decision tree on every turn once a model has been pinned by the user).

### How it picks models

`higgsfield model list --json` is the live source of truth; `model-catalog.md` is explicitly documented as "a mapping (intent → command), not the database." The skill never trusts its own memory of the catalog over a live call when uncertainty is signaled (Scenario 10 in `evals/scenarios.md`: user names a fake model → verify via `model list` → report it doesn't exist → suggest real alternatives, never fabricate).

### How it handles prompt engineering

Offloaded entirely to `references/prompt-engineering.md`: subject+setting+ style formula, camera/lighting/style vocabulary, an explicit image-to-image rule (describe the *change*, not the input — with a bad/good example pair), an image-to-video rule (motion verbs only, don't redescribe the static frame), positive-only phrasing (no `negative_prompt` support in most models, so "no blur" becomes "tack sharp"), and a hard safety list (no real public figures, no sexual content, no trademarked characters).

### How it handles workflows vs models

A workflow (`draw_to_video`, `reframe`) is explicitly *not* a model — it has its own discovery verbs (`workflow list`/`workflow get`) and its own create/cost verb pair (`generate workflow <name> ... --wait` / `generate cost workflow <name> ...`), documented in a dedicated `references/workflows.md` with a "Maintainer note" procedure for adding the next one. The skill enforces the vocabulary split even internally: FNF (the backend) may call these "chains"; the skill's own docs are required to say "workflow" everywhere user-facing.

### How it handles troubleshooting

Same two-tier pattern as the format-standard doc's §8: inline `## Errors` (5 lines) → `references/troubleshooting.md` (full categorized list: Authentication / Validation / Job lifecycle / Rate limits / CloudFlare-DataDome / Cost), each entry keyed by the literal error string the CLI emits.

## 4. Transfer pattern (a): an Episteme STEM router

The image-gen agent's routing shape maps directly onto "route a STEM request to the right scientific skill":

- Build one **auto-pick decision list** per request-intent class (numeric simulation, literature synthesis, unit/dimensional-analysis check, symbolic derivation, data-fitting/regression, lab-protocol lookup) the way `model-catalog.md` builds one per media type — priority-ordered, "higher entry wins," ending in an explicit default rather than silence.
- Give every routable sub-skill (a chemistry skill, a physics skill, a bio/stats skill) the same **auto-pick vs named-only** split Higgsfield uses for models: a short list of tools/methods chosen without asking (e.g. default to SciPy least-squares for curve fitting) vs. a long tail used only when the user names it (a specific solver, a specific package, a named paper's method) — and once named, sticky for the rest of that task.
- Route **by task intent, not by surface keyword or output type** — this is the single highest-value rule to import verbatim from Virality Predictor's misrouting warning. A STEM router is exactly as prone to the same failure mode: "explain this equation" (text out) could be pattern-matched to a chat/explanation skill when it's actually a derivation task that belongs with a symbolic-math skill, the same way "analyze this video" could be misfiled as text generation instead of `brain_activity`.
- Maintain a live "catalog is a mapping, not the database" discipline: if Episteme's sub-skills or backing tools change, the router's reference doc should point at a discovery command (list available solvers/packages) the way `model-catalog.md` points at `higgsfield model list`, not hardcode a snapshot that drifts.
- Reuse the delivery discipline from §2: for any Episteme sub-skill that *evaluates* something (a proof check, a unit-consistency check, a statistical-power calculation) rather than *generates* something, give it Virality Predictor's four-part output shape — score/verdict, key metric, one plain-language interpretation sentence, link to full detail — instead of a raw dump of intermediate calculation.

## 5. Transfer pattern (b): a WeftOS/Skill-3D MentraOS footage analyzer

Virality Predictor is a near-exact template for a video-in/report-out analyzer of MentraOS egocentric-capture footage in the Skill-3D spatial work:

- **Same input/output shape.** One `--video` role, no prompt, one structured text report plus a link to a richer visual artifact (Virality Predictor's "Open report" URL ↔ a Skill-3D spatial-reconstruction viewer link for the same footage).
- **Same naming discipline.** Give the analyzer a customer-facing name distinct from its technical job type, and state explicitly in the routing rule that "analyze this footage" / "what did I look at" routes here even though the underlying job type may sit in a generic media-analysis category — copy the exact phrasing pattern: *"treat X as the customer-facing name; Y is only the technical id."*
- **Domain-appropriate rubric, same structure as Attention/Default Mode.** Where Virality Predictor scores hook/sustain/attention for ad creative, a MentraOS analyzer would score task-relevant regions for egocentric video — candidates: gaze/attention stability, scene-change rate, hazard/salience events, task-step recognition confidence, and something like Virality Predictor's Default Mode inversion — a "distraction/idle-gaze" score where *lower* is better, stated as an explicit inversion rather than left implicit.
- **Same suppression rule for raw artifacts.** Point-cloud/mesh/pose-graph intermediates (the Skill-3D equivalent of `.glb`/`.bin`/vertex-map URLs) stay in JSON/debug output; normal chat delivery is the four-line score-block shape.
- **Same media-role documentation discipline.** Whatever CLI or SDK wraps the MentraOS pipeline should get a `references/media-inputs.md`-style table: accepted roles (`--video`, and likely `--imu`/`--gaze` sidecar streams unique to smart-glasses capture), file-extension auto-detection, and explicit "Do NOT pass X" notes for known schema mismatches — mirroring how `media-inputs.md` documents `seedance_2_0`'s audio-role quirk.
- **Reuse, don't fork, the routing skill.** If a future WeftOS "generate or edit spatial content" skill exists alongside this analyzer (a Skill-3D equivalent of `higgsfield-generate`), keep the analyzer's routing rule inside that skill's decision tree exactly the way Virality Predictor lives inside `higgsfield-generate` rather than the video-explainer-style dedicated folder — a video *generation* skill and a video *analysis* skill have different enough shapes (prompt-driven vs prompt-less, N-output vs single-report) that co-locating the analyzer as a documented sub-capability of the router, with its own "Use when" trigger clause, is the closer match to what Higgsfield actually did.

## 6. License/trademark note

Same constraint as the format-standard doc: MIT covers the repo's code/docs structure (freely reusable for the patterns described above); it does not cover the Higgsfield name, logo, or brand assets. Nothing in this document proposes reusing any Higgsfield-branded asset — the transfer patterns in §4-5 are structural (routing shape, rubric shape, delivery shape), not content.


