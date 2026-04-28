# Plane work-item body template (audit-derived)

Copy this into the work item's `description` field (markdown). The
wrapper handles the HTML conversion. Keep the headings — the close-step
quality check validates that they're present.

```markdown
## Source

- audit: .planning/reviews/0.7.0-release-gate/NN-WORKSTREAM.md#anchor
- code: crates/foo/src/bar.rs:LINE   <!-- omit if not code-rooted -->
- prior planning: .planning/sparc/.../foo.md   <!-- omit if none -->
- related ADR: ADR-NNN   <!-- omit if none -->

## Problem / gap

One paragraph in plain English. Reader should understand the issue
without opening the audit doc. Cite measurements / log lines / commit
SHAs where relevant.

## Acceptance criteria

- [ ] Observable behaviour: <e.g. "agent.chat returns within 30s on cold
      first turn against the test fixture">
- [ ] Tests: <which `scripts/build.sh test -p <crate>` run, expected
      pass count>
- [ ] Build gate: `scripts/build.sh check` and `scripts/build.sh clippy`
      clean
- [ ] Doc / tracker update: <which file gets which line removed/added>

## Dependencies

- blocks: WEFT-NN  <!-- or "none" -->
- blocked-by: WEFT-NN  <!-- or "none" -->
- upstream / external: <e.g. "ruvllm-wasm 11-pattern HNSW cap"; "none"
      if internal-only>

## Notes

- Known traps, related context, who looked at this last and when.
- Anything that would have saved time if the previous person had
  written it down.
```

## Naming convention

`wsNN: <area> — <action verb> <object>`

Examples:

- `ws05: Email channel — implement IMAP poll loop and outbound SMTP send`
- `ws14: workspace deps — migrate clawft-* path-deps to [workspace.dependencies]`
- `ws02: kernel auth — add governance gate to rotate_credential` (would
  be skipped today; already fixed in `a0c54a47`)

## Priority guide

| Priority | When |
|----------|------|
| `urgent` | Live behavioural bug observable in `kernel.log` (Democritus class) |
| `high`   | Must-ship-before-0.7 with no obvious workaround |
| `medium` | In a 0.7.x cycle but has a workaround / can slip a few days |
| `low`    | Deferred to 0.8.x+ |
| `none`   | Pure bookkeeping (label cleanup, doc reflow) |
