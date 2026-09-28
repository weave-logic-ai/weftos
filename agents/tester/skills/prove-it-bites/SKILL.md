---
version: 0.1.0
name: prove-it-bites
description: |
  Writes a unit test and proves it actually catches the defect it claims to, by planting
  the defect, watching the test fail, restoring, and watching it pass. Never ships an
  unverified assertion.
  Use when: closing a test-coverage gap on a card; a bug fix needs a regression test
  proven to have caught the original bug.
  Chain with: build-lane for the code change the test covers; adversarial-review to
  check the plant/verify method itself under scrutiny.
  NOT for: writing production code beyond what's needed to plant/restore a defect;
  running a live-writing e2e suite; board writes.
argument-hint: "[card-id] [test-target]"
allowed-tools: Read, Write, Edit, Bash, Grep, Glob
---

# Prove It Bites

Never ships an assertion that hasn't been watched failing for the right reason.

## Bootstrap

1. Confirm you're in your own worktree.
2. Enumerate the project's gates fresh from the CI workflow file.

## UX Rules

1. Report every test's plant method, not just that it "passed."
2. Say what you could not cover — this outranks the passing-tests summary.
3. Never claim a refusal is tested without asserting both status and reason.

## Workflow

1. **Write the assertion** for the behavior the card requires.
2. **Plant the defect** the assertion claims to catch, by editing a **copy**, never by a
   destructive checkout/reset on a shared file.
3. **Verify the plant applied** — count occurrences of the change; a silently-failed
   plant and a non-biting test look identical from outside.
4. **Watch the test go red.** Strip comments before asserting on structure if a substring
   guard is involved — a comment containing the guarded string can produce a false
   green.
5. **Restore**, verify by content, watch the test go green.
6. **Run the full gate suite**, checking each exit code individually.
7. **Never execute** a live-writing e2e suite — write it, hand it over.

## Errors

- `test passes even with the defect planted` → the test is not covering what it claims;
  rewrite it before shipping, don't ship it as-is.
- `plant appears not to have applied` (test stayed green) → verify by counting
  occurrences of the planted change; a quoting/escaping issue can make a plant silently
  no-op.
- `revert via checkout/reset destroyed unrelated uncommitted work` → this is why plants
  are reverted by restoring specific content, never by a destructive git operation.

## Reference docs

None — the plant/verify/restore loop is the whole method and stays inline.
