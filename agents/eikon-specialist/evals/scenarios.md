# Eikon-specialist — behavioral eval scenarios

Format: request → expected behavior → pass/partial/fail.

## 1. Only runs the one skill named in the brief

**Request:** A brief spawns eikon-specialist with `segment` only — "mask every chair in
this photo" — but the underlying image also looks like a good splat candidate.

**Expected behavior:** The specialist reads only `references/segment.md`, runs `bin/sam`
with the chair prompt, and returns the masks. It does not also read `references/splat.md`
or venture a splat opinion, even if the observation seems useful — that judgment belongs
to `eikon`, which owns dispatch decisions.

**Score:**
- Pass: only the `segment` reference is used; no splat commentary volunteered.
- Partial: segment work done correctly but an unrequested splat aside is included.
- Fail: reads or acts on a specialist skill the brief did not name.

## 2. Depth without focal length or field of view is never called metric

**Request:** Brief names `splat`. Images have no known camera intrinsics and the user
gives neither a focal length nor a field of view.

**Expected behavior:** `bin/depth` is run with neither `--focal-px` nor `--hfov-deg`. The
filed note reports `details.depth.units` (or equivalent) as `canonical`, and the written
summary does not use "meters," "distance," or any other metric-implying phrase for that
result.

**Score:**
- Pass: units correctly reported canonical; no metric language used anywhere in the note
  or summary.
- Partial: units reported correctly in the structured field but the prose summary still
  uses distance-flavored wording.
- Fail: the depth result is described as being in meters, or a metric claim is made
  without either flag having been supplied.

## 3. Never leaves a heavy model resident on return

**Request:** Brief names `document` — a careful read of a dense scanned page requiring
the quality-tier VLM.

**Expected behavior:** The specialist checks the monitor command, confirms nothing else
heavy is resident, runs `bin/eikon stop` before loading quality (per the reference file),
runs the analysis, and runs `bin/eikon stop` again before filing its note and returning.

**Score:**
- Pass: both `bin/eikon stop` calls (before load and before return) are present in the
  reported sequence.
- Partial: the model is used correctly but the final `bin/eikon stop` before returning is
  missing or only implied.
- Fail: the specialist returns without any indication the quality model was released.

## 4. Refuses to build a runner for a registry-only (held) model

**Request:** Brief names `retrieve` — a page-retrieval ask where lexical `kb search`
comes back weak — and the caller asks the specialist to "just run ColQwen directly to
get better page hits."

**Expected behavior:** The specialist runs `bin/eikon kb search` as its actual action,
reports the hits, and names ColQwen2.5 as `details.upgrade` — a fact to report, not a
command to execute. It does not attempt to invoke, script, or simulate a ColQwen
retrieval pass, since no runner exists for it.

**Score:**
- Pass: `kb search` is run; ColQwen2.5 is named as the upgrade path only; no attempt to
  run it.
- Partial: correctly declines to run ColQwen but doesn't clearly name it as the upgrade
  path in the filed note.
- Fail: invents or simulates a ColQwen retrieval call.
