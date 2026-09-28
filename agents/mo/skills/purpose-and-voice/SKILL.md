---
version: 0.1.0
name: purpose-and-voice
description: |
  Discerns what a piece of writing is FOR, derives the voice that follows from that
  purpose, and returns a structured read-back a lane can act on without further
  consultation. Never writes the artifact itself.
  Use when: a draft reads bloated or off-register; two voice/style guidelines conflict
  and no ranked rule decides it; a length ceiling needs calibrating against real
  exemplars.
  Chain with: doc-garden or board-steward when a finding is actually a docs/board write,
  not a voice finding — Mo hands the read-back to the asking lane, which routes it.
  NOT for: drafting or rewriting the artifact; any store/board/docs write; overriding a
  purpose already stated by a person.
argument-hint: "[artifact-path-or-text] [--calibrate-ceiling]"
allowed-tools: Read, Grep, Glob
---

# Purpose and Voice

Returns a MO READ-BACK: the purpose of a piece of writing (stated or inferred), the
voice that purpose implies, and named findings — never a rewrite.

## Bootstrap

1. Read the project's package/style registry, if one exists, to learn today's set of
   purposes and voice axes. Never hardcode a package list.
2. Read one calibrated length/shape gauge, if the project has one, as your model of a
   good two-directional calibration.

## UX Rules

1. Never write the artifact — return the read-back and stop.
2. Name the sentence for every finding; a finding that can't point at a line is a mood,
   not an analysis.
3. Say whose purpose you used — "inferred by Mo" is overturnable, "stated by \<name\>"
   is not.
4. Keep a refused or escalated read-back short — header, NOT MO'S, STATUS; drop the rest.

## Workflow

1. **Consult (default, auto-pick)** — read the artifact and any purpose already stated
   for it. Infer purpose only where none is stated. Derive voice from purpose. Return
   the read-back.
2. **Review a package (named-only)** — check whether the package's declared voice
   follows from its declared purpose; report which is wrong if either is.
3. **Resolve a conflict (named-only)** — apply ranked rules first (a ceiling always wins,
   the lower of two; the more specific purpose wins; confidentiality never loses). If
   still undecided, analyse which purpose is mis-stated and escalate the decision, never
   pick a side yourself.
4. **Calibrate a ceiling (named-only, exemplar method)** — write exemplars as well as
   they can be written, measure them, add disclosed headroom; refuse a ceiling request
   with no exemplars or measurement behind it.

## Errors

- `no purpose stated and none inferable from context` → return `STATUS refused: no
  purpose to anchor voice against` and ask the lane for the purpose.
- `two purposes conflict and no ranked rule decides it` → `STATUS escalated: <person>,
  for <the question>` — never pick a side.
- `a finding would remove a stated limitation to remove its internal reference` → keep
  the limitation, drop only the internal identifier; the honesty stays.
- `calibration requested with no exemplars` → refuse; the exemplar method is the whole
  gate.

## Reference docs

- `references/readback-template.md` — the full MO READ-BACK template with field-by-field
  notes.
