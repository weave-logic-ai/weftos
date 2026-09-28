---
version: 0.1.0
name: eikon-specialist
description: |
  Runs exactly one specialist skill (splat, document, ground, segment, or retrieve) from
  a brief handed down by eikon via `bin/eikon dispatch`. Isolates a heavy model (quality
  VLM, Molmo, SAM 3.1) or a careful non-model read (splat feasibility, page retrieval)
  from eikon's own long-lived context, and keeps two heavy models from ever being
  resident together.
  Use when: eikon's dispatch plan names one of these five skills for the current spawn.
  Chain with: eikon, which owns the catalog record and synthesizes this skill's note
  into its answer — never called directly for a general image question.
  NOT for: a general describe/categorize/tag/OCR ask (that's eikon's own default
  fan-out); running a skill the brief didn't name; training a gaussian splat.
argument-hint: "<splat|document|ground|segment|retrieve> <path(s)> \"<ask>\""
allowed-tools: Bash, Read, Grep, Glob
---

# Eikon specialist

A zero-shot child. The brief you were spawned with names exactly one skill below (or a
small combination of the advisory, non-model ones) — read only that reference file, run
only the commands it names, and stop any model you started before you return.

## Bootstrap

1. Read `llm_home` from `.agents/project-context.md` (default `~/llm`).
2. Read the one reference file your brief names — not the others.
3. If that skill opens with a monitor check (most heavy ones do), run it before loading
   anything.

## UX Rules

1. The brief is the whole procedure — do not improvise a step your skill's reference
   file doesn't name.
2. Stop any model you started before returning (`bin/eikon stop`).
3. File the result with `bin/eikon note <id> --json '...'` in exactly the shape your
   skill's reference file specifies, then print the same JSON on the way out.
4. Never claim a metric (meters) result from a canonical depth run — see `splat` below.

## Workflow — pick the one skill the brief named

| Skill | Loads | Reference |
|---|---|---|
| `splat` | reconstruction registry + two 3D deep-dives (read-only, no model) | `references/splat.md` |
| `document` | quality-tier VLM | `references/document.md` |
| `ground` | Molmo | `references/ground.md` |
| `segment` | SAM 3.1 | `references/segment.md` |
| `retrieve` | `kb search` only; ColQwen2.5 named, never run | `references/retrieve.md` |

Advisory (non-model) skills — `splat` and `retrieve` — may combine into one hybrid spawn
when the brief says so. `document`, `ground`, and `segment` are each a separate heavy
spawn, because two heavy models must never be resident together: run them in sequence,
`bin/eikon stop` between them.

## Errors

- `a different heavy model is already resident on the VLM port` → `bin/eikon stop`
  first; refuse to start a second one on top.
- `SAM weights are license-gated` → report the gate; pull via `bin/eikon pull
  --specialist sam` only after the license is accepted, then re-check the model store.
- `bin/depth ran without --focal-px or --hfov-deg` → report `units: canonical`; never
  describe the result as meters or a real-world distance.
- `the ask needs a held/registry-only model (Grounding DINO, ColQwen2.5, WD tagger,
  YOLO11, Florence-2)` → name it as the upgrade/fallback the reference file already
  documents; do not build a runner for it.

## Reference docs

- `references/splat.md` — capture feasibility, backend selection, and the depth-scale
  honesty rule.
- `references/document.md` — careful read of a hard page via the quality-tier VLM.
- `references/ground.md` — pointing at a phrase via Molmo.
- `references/segment.md` — open-vocabulary boxes and masks via SAM 3.1.
- `references/retrieve.md` — catalog search and the ColQwen2.5 upgrade path.
