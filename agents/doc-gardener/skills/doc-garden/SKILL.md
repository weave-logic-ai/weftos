---
version: 0.1.0
name: doc-garden
description: |
  Runs the four documentation operations (graft, prune, plant, correct) against a
  project's docs and decision records, with mandatory evidence per correction and a
  durable per-item queue for work with no lane attached.
  Use when: "is X documented?", "fix the docs around X", "that page is stale", a lane
  needs docs shipped with its diff, or a documentation coverage sweep is requested.
  Chain with: board-steward when a doc finding turns out to be a work item, not a doc
  fix; a project's domain-expert agent when a doc claim needs grounding in world/model
  fact rather than code fact.
  NOT for: writing or editing production code/tests/migrations (file it, name the lane);
  board writes or board comments (hand to steward); anything without evidence in one of
  the three forms below.
argument-hint: "[assess|repair|plant|graft|prune|correct] <subject-or-path>"
allowed-tools: Read, Write, Edit, Grep, Glob, Bash
---

# Doc Garden

Keeps a documentation estate *true*, not merely tidy — every correction cites its
evidence, every retraction rewrites rather than annotates, and a durable queue holds
work that has no lane attached yet.

## Bootstrap

1. Read the project's instruction file for confidentiality tiers and commit rules.
2. Read the citation gate/checker if the project has one — your own writes get checked
   against it.
3. Read the decision-record index and numbering scheme.
4. Read the queue directory (`docs/doc-queue/` by convention) before deciding anything —
   it is state, not in your context window by default.

## UX Rules

1. Ask about a SUBJECT, never demand a path — find the documents yourself.
2. A drive-by observation costs the caller four words; record it and stop, don't act on
   it unasked.
3. Never present a correction without its evidence form attached.
4. Say what you did not cover in every repair report — silence there reads as complete
   coverage.

## Workflow

Auto-pick vs named-only:

1. **A lane hands you a diff and waits (Path A)** → auto-pick synchronous: read the diff,
   write the prose, hand it back. Never queue this without telling the lane and getting
   agreement.
2. **A drive-by mention, no lane attached (Path B)** → auto-pick: append one file to the
   queue with rank + reason, stop.
3. **"Is X documented?" (assess)** → named-only: enumerate what exists, what's stale and
   why (with evidence), what's missing. A subject with no document is a finding.
4. **"Fix the docs around X" (repair)** → named-only: locate every document touching the
   subject area, apply operations per evidence, report what you changed and what you
   refused without a ruling.
5. **Coverage sweep** → named-only, and rate-limited: enumerate units first (see
   `references/coverage-method.md`), report the denominator before any queue items, cap
   how many gaps one run files.

### The four operations — pick by what the change does

| Change | Operation | Needs | Default |
|---|---|---|---|
| doc should exist, doesn't | plant | named audience, question answered, dependency declaration | propose |
| true content scattered / attach doc to code | graft | proof it's the same claim | declare-dependency disposes; merge proposes |
| text correct but unmaintainable, or plain stale | prune | a stated reason + "do not restore" line | propose |
| claim is wrong, true claim known | correct | evidence in one of 3 forms | propose unless mechanical + re-runnable |

### Evidence forms (exactly one required per correction)

1. Verbatim quote + source locator (case-sensitive, elision in-order, long enough to mean
   something).
2. File-and-line locator, at a stated commit, naming code you actually read.
3. A stated method + its output (not a bare number) — for live data, environment
   confirmed before and after the read.

No evidence → the claim is not written. File it in the queue as `blocked`, naming what's
missing and who can supply it.

## Errors

- `no source to cite` → do not write the correction; queue it `blocked`.
- `citation gate rejects an opted-in doc` → the quote is not a literal substring; shorten
  the quote or fix the locator, never reflow the source to fit.
- `two documents disagree` → surface both, resolve toward the higher-trust source per the
  project's evidence hierarchy if one exists, otherwise queue as `blocked` for a human
  ruling.
- `decision record and code have diverged` → this is your most valuable finding; report it
  plainly rather than "fixing" either side unilaterally.

## Reference docs

- `references/coverage-method.md` — the enumerate-first, denominator-before-index method
  for a documentation coverage sweep.
- `references/retraction-forms.md` — the four retraction locations, in preference order,
  and the accretion test.
