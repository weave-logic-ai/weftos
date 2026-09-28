---
version: 0.1.0
name: measure-only
description: |
  Answers "how many" / "is this still true" / "what does live state actually hold"
  through read-only project-declared measurement bindings, with mandatory before/after
  state confirmation and enumerate-don't-guess discipline. No edit tools.
  Use when: a decision depends on a live number; a build lane's own "it works" claim
  needs independent confirmation.
  Chain with: adversarial-review, which uses this skill's numbers as inputs to a
  correctness finding rather than re-measuring itself.
  NOT for: any write, edit, migration, deploy, or board application — a needed write is a
  finding to report, never an action to take here.
argument-hint: "[what-to-measure]"
allowed-tools: Read, Bash, Grep, Glob
---

# Measure Only

Produces one number or state check, with its provenance and what it does not establish.
No write path exists in this skill by design.

## Bootstrap

1. Read `.agents/project-context.md` for the `measurement_bindings` block — the
   project's own read-only endpoints/commands for board state, environment state, and
   any API surface this lane is granted.
2. Confirm any shared mutable measurement mode/environment flag, and record its value.

## UX Rules

1. Refuse rather than report zero when a credential or binding is missing — name what
   you checked.
2. Never suppress stderr — an error is a result, not noise to hide.
3. Enumerate the full set rather than sampling, unless you say explicitly that you
   sampled and what.
4. Re-measure immediately before reporting a number someone will act on; state when you
   measured it.

## Workflow

1. **Confirm the measurement environment** (if the transport has a shared mode flag) —
   read it, note it, and read it again after the measurement to confirm nothing else
   flipped it mid-run.
2. **Enumerate**, don't sample, unless the population is too large — then say so and say
   what fraction you actually read.
3. **Name the exact field/column/endpoint** measured — not a nearby one that happens to
   reconcile.
4. **Report** with full provenance: lane, command/endpoint, before/after environment
   state, timestamp, revision if applicable — then the number — then what it does not
   establish.

## Errors

- `credential or binding missing` → refuse; name the exact path/variable checked. Do not
  report a zero.
- `query returns empty` → report as "empty result," distinct from "broken command" and
  distinct from "measured as zero" — these are three different findings.
- `shared measurement mode changed mid-run` → discard the reading, re-measure, and report
  the mode flip itself as a finding.
- `instrument cannot see the thing being asked about` → say so explicitly rather than
  reporting an unrelated number as though it answered the question.

## Reference docs

None — the discipline above is short and stays inline; splitting it would risk a step
being skipped mid-measurement.
