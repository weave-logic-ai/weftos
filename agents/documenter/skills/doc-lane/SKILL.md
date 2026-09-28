---
version: 0.1.0
name: doc-lane
description: |
  Writes and reconciles docs and decision records for a specific change or request,
  recomputing every derived value rather than transcribing it, and retracting by
  rewriting rather than annotating in place.
  Use when: a build lane needs docs written alongside its diff; an accepted decision
  needs a record drafted; a doc has drifted from the code it describes.
  Chain with: doc-garden (the doc-gardener agent's skill) for estate-wide, queued
  documentation ownership — this skill is the per-change companion, not a replacement.
  NOT for: editing production code/tests/migrations; board writes; rewriting a decision
  record's Decision section (amend/supersede instead).
argument-hint: "[change-ref] [doc-path]"
allowed-tools: Read, Write, Edit, Bash, Grep, Glob
---

# Doc Lane

Writes documentation and decision records tied to a specific change, with the same
recompute-don't-transcribe and rewrite-don't-annotate discipline as the estate-wide
doc-gardener, scoped to one change at a time.

## Bootstrap

1. Read the project's citation gate/checker if one exists.
2. Read the decision-record numbering and supersede convention.

## UX Rules

1. Never transcribe a count/ratio/list that has a live source — point at the source
   instead.
2. Dates are absolute, never relative.
3. When removing a stale section, say "do not restore this" and why.

## Workflow

1. **A build lane hands you a diff** → write the doc that ships with it, in its branch,
   promptly.
2. **A decision was already made** → draft the decision record (status: Accepted, named
   decider and date); never write "Accepted" on a decision nobody took.
3. **A doc has drifted from code** → reconcile: read the current code, correct the doc,
   cite what you read (file + line, at the revision you read it).
4. **Retracting a claim** → rewrite the sentence; record the change via commit message or
   a foot-of-document changelog; never leave struck-out text beside the fix.

## Errors

- `quote is not a literal substring of its source` → shorten the quote or fix the
  locator, never reflow the source.
- `derived value has no live source to point at` → recompute it yourself with a stated
  method, and say the method in the doc.
- `decision record's Decision section needs to change` → do not edit it; draft an
  amendment or a new superseding record instead.

## Reference docs

None — this skill mirrors the doc-gardener's evidence/retraction discipline at a smaller
scope and stays inline for that reason.
