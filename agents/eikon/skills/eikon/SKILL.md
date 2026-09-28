---
version: 0.1.0
name: eikon
description: |
  Analyzes images with bin/eikon — Apple Vision first (no weights), then one VLM JSON
  call per batch covering description, scene, categories, tags, labels, objects, text,
  and facts. SigLIP-2 and Molmo run only when the ask needs a label score, an embedding,
  or a point. Heavier work (splat, careful document read, pointing at length,
  segmentation, page retrieval) is dispatched to eikon-specialist, not run here.
  Use when: "describe/caption this image", "categorize/classify/label/tag/identify",
  "OCR this", "build a searchable base from this folder", "find images like this one".
  Chain with: eikon-specialist for a splat call, a careful read of a hard document page,
  pointing at length, segmentation masks, or visual page retrieval.
  NOT for: running a specialist skill in this agent's own context (dispatch instead);
  starting a second heavy model on top of one already resident; a model this registry
  holds out of the fan-out (YOLO11, WD tagger, ColQwen2.5, Florence-2, Grounding DINO).
argument-hint: "<path-or-dir> [--labels a,b,c] [--point \"phrase\"] [--native-only]"
allowed-tools: Bash, Read, Glob, Grep
---

# Eikon

Runs the default image fan-out (Vision, then one VLM call, then SigLIP/Molmo only on
request) and knows when to dispatch heavier work to `eikon-specialist` instead of running
it in-context.

## Bootstrap

1. Read `llm_home` from `.agents/project-context.md` (default `~/llm`). Every relative
   path below (`bin/…`, `var/…`, `docs/…`) is relative to that root.
2. Read `$LLM_HOME/USING_LOCAL_MODELS.md` section 3 far enough to honor it before pulling
   or warming anything.
3. Run the project's memory-monitor command (`bin/monitor` by convention) before a big
   load. Never start a heavy profile beside the machine's daily coder.
4. `bin/eikon roster` if unsure which VLM profile applies — same table as
   `references/models.md`.

## UX Rules

1. One VLM call covers describe, categorize, classify, label, tag, and identify. Never
   start a second model per verb.
2. Apple Vision OCR is the text you trust; the VLM's `text` field is appended only when
   it adds something new — say which stage produced which field when it matters.
3. Read `ok` on a record before claiming a field came from the VLM — a failed stage is
   recorded, not silently dropped.
4. Never invent a runner for a held model (see "Held models" below) — name the reason
   instead.

## Workflow

Auto-pick by what the ask needs:

1. **Describe / categorize / classify / label / tag / identify / OCR** → `bin/eikon
   analyze <path>` (default profile `fast`). One VLM JSON call covers all of these.
2. **Score against a label list** → add `--labels a,b,c` (SigLIP-2).
3. **Build or extend a searchable base** → `analyze` a directory, then `bin/eikon kb
   search`.
4. **Point at a described thing** → add `--point "<phrase>"` (Molmo, after the VLM).
5. **A hard page, a splat feasibility call, masks, or page retrieval** → do not run it
   here. Use `bin/eikon dispatch` and spawn `eikon-specialist` with the numbered prompt —
   see "Dispatch" below.
6. **No downloads right now** → add `--native-only` (Apple Vision only).

### Dispatch

```bash
bin/eikon dispatch --path photo.jpg "can this room be a gaussian splat"
bin/eikon dispatch --prompt 1 --path photo.jpg "can this room be a gaussian splat"
```

Each numbered plan entry is one spawn of `eikon-specialist`, given that entry's prompt as
its whole task and nothing else from this skill. `inline: catalog` means: run `bin/eikon
analyze` first, then pass the resulting record ids into a second `dispatch --id <id>`.
Quality-VLM, Molmo, and SAM are separate heavy spawns — run them in sequence and
`bin/eikon stop` between them, never together.

| Skill | Dispatch when |
|---|---|
| `splat` | gaussian splat, COLMAP, Brush, novel view, multi-view feasibility |
| `document` | careful read, handwriting, charts, small print |
| `ground` | point at, where is the, coordinates |
| `segment` | masks, segmentation, world model |
| `retrieve` | visual RAG, page retrieval |

The child files its result with `bin/eikon note <id> --json '...'`. Synthesize the
catalog record and the note yourself — never rerun the child's model.

## Commands

```bash
bin/eikon roster
bin/eikon pull --profile fast          # then: bin/modelstore status
bin/eikon warm --profile fast          # returns while a cold load finishes
bin/eikon status
bin/eikon analyze <path> --out var/eikon/kb
bin/eikon analyze page.png --labels invoice,receipt,form
bin/eikon analyze shot.png --point "the serial number"
bin/eikon analyze folder --native-only
bin/eikon analyze folder --profile quality      # only after the monitor says memory is free
bin/eikon kb search "invoice 4401"
bin/eikon kb search --image query.jpg
bin/eikon kb show <id>
bin/eikon stop
bin/eikon selftest      # routing, JSON parse, merge, lexical search, a Vision OCR round-trip; no VLM
```

## Held models

YOLO11, the WD tagger, ColQwen2.5, Florence-2, and Grounding DINO are catalogued and
deliberately off this fan-out. SAM 3.1 belongs to the `segment` specialist skill only —
never call it directly from here. Full reasons: `references/models.md`.

## Errors

- `a different model is resident on the VLM port` → `bin/eikon stop` first; this skill
  will not load a second VLM on top of one that's already up.
- `--profile quality (or moe) requested beside the daily coder` → refuse, name what's
  resident (per the monitor command), and suggest the smaller default profile or a
  background window when the coder is idle.
- `a stage failed mid-batch` → the rest of the record is kept; report `ok` per stage
  rather than treating the batch as a total failure.
- `the ask needs a held model (YOLO11, WD tagger, ColQwen2.5, Florence-2, Grounding
  DINO)` → do not build a runner; say which held model would cover it and why it's out
  of the default fan-out (`references/models.md`).

## Reference docs

- `references/models.md` — the VLM profile table, the always-on Apple Vision requests,
  the flag-gated specialists (SigLIP-2, Molmo), and the held-model registry with reasons.
