---
version: 0.1.0
name: board-steward
description: |
  Stages, validates, deduplicates and applies work-tracker manifests through a project's
  board adapter. Wraps whatever board-write CLI/API the project declares in
  `.agents/project-context.md` (WeftOS default: `scripts/dashboard-board.mjs`).
  Use when: "file this as tickets", "stage a manifest for the board", "dedupe this
  against the board", "audit the board for doubled cards", "apply this plan".
  Chain with: the doc-garden skill when a finding belongs in a card file rather than a
  board comment; the domain-expert template when a card's business meaning is unclear.
  NOT for: writing card-file discussion/comments (use doc-garden), deciding board status
  or assignee (human call), any board write without a shown, confirmed plan.
argument-hint: "[manifest.json] [--apply]"
allowed-tools: Bash, Read, Grep
---

# Board Steward

Stages and applies a work-tracker manifest through the project's board adapter, with
mandatory dedupe, validation and human confirmation before any write.

## Bootstrap

1. Read `.agents/project-context.md` for the `board_adapter` block: list command, dry-run
   command, apply command, intake-contract path, schema/roster paths.
2. Run the adapter's **list** command to pull the live board. Never dedupe from a cached
   or remembered view.
3. Confirm the intake contract's validator is runnable (`node <validator> --help` or
   equivalent).

## UX Rules

1. Be concise in the plan you present — counts, then a one-line-per-card list, never a
   raw JSON dump.
2. Never apply without a shown plan and explicit confirmation captured in your brief.
3. State every dedupe verdict, with the id matched against.
4. Never invent an owner, date, or requirement — write "unknown" instead.

## Workflow

Auto-pick vs named-only, in priority order:

1. **Manifest supplied, brief carries authorization** → validate → dry-run → apply →
   verify by read-back → report ticket ids. Auto-pick.
2. **Manifest supplied, no authorization in brief** → validate → dry-run → return the
   plan as the final report, state nothing was written. Auto-pick; do not wait.
3. **"Audit the board" / "why do we have three cards about X"** (named-only, no
   manifest) → read live state, judge against the contract, propose the smallest set of
   changes as a manifest, then follow path 1 or 2.
4. **Producer hands you prose, not a manifest** → convert it into a manifest yourself,
   say that you did, then follow path 1 or 2.

### Manifest shape (generic; see the project's intake contract for the authoritative one)

```json
{
  "batch": { "producer": "...", "slug": "...", "title": "...", "date": "YYYY-MM-DD", "source": "path/or/url" },
  "tickets": [
    {
      "action": "create | update | comment | drop",
      "match": "existing-ticket-id | null",
      "outcome": "...",
      "what_we_heard": { "quote": "...", "locator": "...", "asked_by": "..." },
      "requirements": ["..."],
      "acceptance": ["..."],
      "open_questions": ["..."],
      "refs": ["..."],
      "kind": "...", "priority": "...", "target": "...", "turn_owner": "name | unclear"
    }
  ]
}
```

### Apply sequence

1. `validate(manifest)` via the intake contract's own validator — fix errors, re-run
   until clean.
2. `dry_run(manifest)` via the adapter — read the printed plan verbatim into your report.
3. Confirm authorization is present in your brief (see Workflow step 1/2 above).
4. `apply(manifest)` with the adapter's explicit write flag — **do not rely on a
   transport default**, most board transports default writes to dry-run.
5. Read at least one changed card back through the adapter's get-single-item call and
   report what you observed.
6. Report `source_key → ticket_id` for every item, plus dedupe matches, drops, and open
   questions.

### Bulk close

Treat "close everything in lane X" as N individual reads-and-decisions, never a sweep.
Report which items did not close and what each is missing.

## Errors

- `dry-run only, no write occurred` → this is not an error; it means your brief carried
  no authorization. Return the plan as your report.
- `validator rejects manifest` → fix the manifest, re-run the validator; never hand a
  human an unvalidated plan.
- `apply reported success but a read-back shows no change` → treat as a transport dry-run
  default silently swallowing the write; re-apply with the explicit write flag and verify
  again before reporting success.
- `duplicate ticket found after apply` → run the project's doubled-card repair path if one
  exists; otherwise report the duplicate as a finding rather than silently merging it.

## Reference docs

- `references/manifest-contract-notes.md` — full field-by-field notes on what a complete
  card needs and why (evidence, next-mover naming, goal-membership honesty).
