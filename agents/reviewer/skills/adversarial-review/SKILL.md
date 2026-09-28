---
version: 0.1.0
name: adversarial-review
description: |
  Reviews a change or claim to refute it, not confirm it — always with a failing case or
  an explicit "not established," never a bare verdict. Read-only by design.
  Use when: a change touches authorization, guards, migrations, or a public surface,
  before merge; a "zero findings"/"all green" report needs independent checking.
  Chain with: build-lane and prove-it-bites when a finding needs a fix and a covering
  test; lead-doctrine's "verify population, not that it ran" habit is this skill's core
  method.
  NOT for: making edits (escalate design problems instead); approving on a promise;
  board writes.
argument-hint: "[change-ref-or-artifact-path]"
allowed-tools: Read, Bash, Grep, Glob
---

# Adversarial Review

Refutes claims about a change rather than confirming them. Read-only.

## Bootstrap

1. Identify the exact artifact/revision under review — name it in every claim you make.
2. Fetch fresh; never review against a stale local checkout when the question is about
   the current head.

## UX Rules

1. Every finding names a concrete failing case (input/state → wrong output or missed
   refusal). No case, no finding.
2. Separate CONFIRMED from PLAUSIBLE explicitly.
3. Separate correctness findings from taste/style opinions, and label which is which.
4. Escalate design disagreements to the coordinating lead, never demand the author fix
   something you haven't justified with a case.

## Workflow

1. **Open the artifact** you're about to claim something about — not a summary of it.
2. **Re-check any guard whose stated reason has moved**, not only ones that were edited.
3. **Measure scope AND predicate separately** — a filter can be logically perfect and
   still scoped to the wrong population.
4. **Ask what an instrument would report if the thing it measures were wholly broken** —
   if the answer doesn't change, it isn't measuring what it claims to.
5. **Verify a "zero findings" claim by checking the expected population**, and where
   feasible by planting a real defect and watching the gate fire, then restoring.
6. **Compute diffs the way a merge would actually apply them** — prefer the project's
   merge-simulation tooling over a raw two-way or three-way diff when they'd disagree.
7. **Report**: CONFIRMED findings with failing cases and blast radius; PLAUSIBLE
   suspicions labeled as such; what you could not establish.

## Errors

- `no failing case for a suspected issue` → report "not established," not a finding.
- `stderr suppressed in a check you're relying on` → re-run without suppression; the
  error is frequently the actual result.
- `report claims zero findings with no stated expected population` → treat as unverified;
  ask for or independently derive the expected count before accepting the zero.

## Reference docs

None — the method above is short by design and loses value if split across files mid-task.
