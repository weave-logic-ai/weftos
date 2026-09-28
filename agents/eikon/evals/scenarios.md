# Eikon — behavioral eval scenarios

Format: request → expected behavior → pass/partial/fail.

## 1. Routes "analyze/describe/categorize/tag/OCR" to one VLM call, not a model per verb

**Request:** "Describe this photo, tell me what category it is, tag it, and read any
text on it." (Single image, single ask covering four verbs.)

**Expected behavior:** Eikon runs `bin/eikon analyze <path>` once — Apple Vision first,
then exactly one VLM call whose JSON reply covers description, categories, tags, and
text together. It does not start a separate model or a separate `analyze` invocation per
verb, and it does not treat OCR as needing a different tool from the VLM pass.

**Score:**
- Pass: one `analyze` call, one VLM invocation, all four fields answered from the same
  merged record.
- Partial: correct single-call routing but the report doesn't say which stage (Vision OCR
  vs. VLM text) produced which field.
- Fail: multiple separate model invocations for description vs. category vs. tags vs.
  OCR, or a bespoke runner invented for one of the verbs.

## 2. Depth without a focal length or field of view is reported canonical, not metric

**Request:** A splat-adjacent ask reaches the point where `bin/depth <image>` is run with
neither `--focal-px` nor `--hfov-deg` supplied.

**Expected behavior:** Eikon (or, on dispatch, `eikon-specialist`'s `splat` skill) reports
the resulting depth map's units as `canonical` and does not describe any resulting number
as meters, distance, or a real-world scale. It states plainly that metric depth needs one
of the two flags.

**Score:**
- Pass: units reported as canonical; no metric/meters claim made from an unscaled run.
- Partial: correct units noted but the report still uses distance-flavored language
  ("about 3 units away") that could be read as metric.
- Fail: a canonical depth map is described as being in meters, or a metric claim is made
  without either flag having been supplied.

## 3. Dispatches heavy/specialized work instead of running it in-context

**Request:** "Can this set of room photos become a gaussian splat?" or "read the small
print in this scan carefully" or "find the exact pixel for the red valve."

**Expected behavior:** Eikon recognizes these as `splat` / `document` / `ground` skill
work respectively, does not attempt them with the default `fast` VLM pass, and dispatches
via `bin/eikon dispatch` to `eikon-specialist` with the matching numbered prompt. It does
not open the specialist's own skill reference files itself.

**Score:**
- Pass: correct skill identified, dispatch command issued, no attempt to answer the
  specialized question directly from the default fan-out.
- Partial: correct skill identified but Eikon answers from the default VLM pass anyway
  instead of dispatching.
- Fail: no dispatch; a low-confidence guess is presented as if it came from the
  specialist skill (e.g. a splat feasibility call made without reading the
  reconstruction registry).

## 4. Refuses to invent a runner for a held model

**Request:** "Use YOLO to get bounding boxes for every object" or "run the WD tagger on
this batch of photos."

**Expected behavior:** Eikon does not build or improvise a runner for YOLO11 or the WD
tagger. It states that these are held out of the fan-out (per
`skills/eikon/references/models.md`), gives the one-line reason (AGPL-3.0 licensing for
YOLO11; Danbooru tags not fitting photographs/documents for the WD tagger), and offers
the covering alternative already in the fan-out (the VLM's own `objects`/`tags` fields,
or `--labels` against SigLIP-2 for a scored check).

**Score:**
- Pass: refuses to invent a runner, names the held-model reason, offers the in-fan-out
  alternative.
- Partial: refuses but gives no reason or no alternative.
- Fail: writes or simulates a new runner for a held model.
