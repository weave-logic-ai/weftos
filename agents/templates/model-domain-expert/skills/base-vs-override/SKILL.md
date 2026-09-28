---
version: 0.1.0
name: base-vs-override
description: |
  Traces a claimed model figure to its dependency chain and states explicitly whether it
  reflects the base model or a live override, never pairing the two without independent
  sourcing for each.
  Use when: a deliverable quotes a model figure and it's unclear whether it's base or
  overridden; "what's driving this number" needs answering with an actual trace, not a
  guess.
  Chain with: grounded-domain when the question is about org/process reality rather than
  the model's own computation.
  NOT for: inventing a threshold the model doesn't document; asserting a figure with no
  traceable source.
argument-hint: "[figure-or-cell-reference]"
allowed-tools: Read, Grep, Glob, Bash
---

# Base vs. Override

Enforces one honesty rule: never let a base-model value and a live-override value wear
each other's clothes.

## Bootstrap

1. Read `.agents/project-context.md` for the `model_tracing` block: the project's own
   IO-map / dependency-graph / formula-explain tooling, or (minimally) its documented
   model structure.
2. Confirm which mode you're reading in — pristine base model, or a specific override
   scenario — before tracing anything.

## UX Rules

1. Every quoted figure states base-vs-override explicitly.
2. Never pair a "default" and a "current" value unless both are independently sourced.
3. An undocumented threshold is reported as "threshold undocumented," never filled with
   an invented benchmark.

## Workflow

1. **Identify the figure's origin mode** — is the project asking about the base model or
   a specific scenario/override?
2. **Trace the dependency chain** from the figure to its inputs, using the project's own
   tracing tooling; report the chain, with values, and the revision/state you read it at.
3. **State base-vs-override explicitly** in the answer, sourced independently for each if
   both are mentioned.
4. **For a threshold/policy value**, cite where it's documented, or say "threshold
   undocumented" and refuse to invent one.

## Errors

- `figure has no traceable dependency chain` → answer `unknown`, don't guess a driver.
- `base and override values requested together but only one is sourced` → report only
  the sourced one, explicitly note the other is not established.
- `threshold not documented anywhere` → report "threshold undocumented"; never assert a
  pass/fail against an invented number.

## Reference docs

None in the base template — a concrete project instance should add its own tracing-tool
reference once the project's model-tooling is known.
