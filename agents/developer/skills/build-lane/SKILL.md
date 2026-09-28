---
version: 0.1.0
name: build-lane
description: |
  Runs a scoped code change end to end in an isolated worktree: query the code index,
  write the change and its tests, gate, and report — never touching the board, goals, or
  production.
  Use when: a card needs implementation; a scoped bug fix is ready to write.
  Chain with: tester's prove-it-bites skill for defect-planting proof; reviewer's
  adversarial-review skill before merge; board-steward when the change needs a card
  filed or moved.
  NOT for: board/goal writes; production writes; running an e2e suite against a live
  system; merging to main.
argument-hint: "[card-id] [worktree-path]"
allowed-tools: Read, Write, Edit, Bash, Grep, Glob
---

# Build Lane

Implements a scoped change in its own worktree, gates it, and reports what it verified.

## Bootstrap

1. Confirm you're in a dedicated worktree, not the shared checkout.
2. Query the project's code index (if one exists) for the relevant symbols before
   grepping or reading files.
3. Enumerate the project's gates from its CI workflow definition, freshly, at the
   revision you're gating — never from a remembered list.

## UX Rules

1. Report lane/tree/branch/revision/dirty-state at the top of every report.
2. State what you could not establish — that section outranks the happy-path summary.
3. Never claim a gate passed without having actually run it at the current revision.

## Workflow

1. **Implement** the change, updating any assertion the change makes wrong rather than
   deleting it.
2. **Test with the change**, in the same commit — see the tester lane's skill for the
   plant-and-verify discipline if you're also writing the covering test.
3. **Gate** with the project's own runner; check each step's exit code individually.
4. **Never commit to main** — open a change request titled with its card id.
5. **Report** what you verified, what you could not, and the card/goal it serves.

## Errors

- `gate count in a doc disagrees with the CI workflow file` → trust the workflow file,
  fetched fresh; flag the doc as stale to the documentation agent.
- `fresh worktree has no dependencies installed` → install before gating; "command not
  found" from every gate step is this, not a broken gate.
- `revert with a hard reset destroyed uncommitted work` → this is why reverts are by
  copy, not reset; recover from any available backup and never repeat the reset-based
  revert.

## Reference docs

None — this skill stays inline; nothing here is safe to cut without breaking
decision-making mid-task.
