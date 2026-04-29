## Source

- audit: `cargo audit` cold run 2026-04-28 (WEFT-104)
- report: `.planning/reviews/0.7.0-release-gate/audit-findings/cargo-audit-cold-run-2026-04-28.md`

## Problem / gap

cargo audit reports 6 unique warning advisories on the workspace lockfile:

- `bincode` 1.3.3 + 2.0.1 unmaintained (RUSTSEC-2025-0141)
- `instant` 0.1.13 unmaintained (RUSTSEC-2024-0384)
- `paste` 1.0.15 unmaintained (RUSTSEC-2024-0436)
- `rustls-pemfile` 1.0.4 unmaintained (RUSTSEC-2025-0134)
- `serial` 0.4.0 unmaintained (RUSTSEC-2017-0008)
- `rand` 0.8.5 + 0.9.2 unsound with custom logger (RUSTSEC-2026-0097)

All transitive. Currently `--ignore`d in the cargo-audit gate so the gate
stays green; need to be replaced or upgraded.

## Acceptance criteria

- [ ] `bincode` migrated to a maintained alternative (postcard?) or
      pinned to a maintained fork; both 1.x and 2.x copies removed.
- [ ] `instant` replaced by `web-time` (its successor for wasm + native).
- [ ] `paste` replaced — the obvious successor is `pastey` or the
      `proc-macro2`-based pattern; survey upstreams that pull it in.
- [ ] `rustls-pemfile` 1 dropped (the rustls-webpki bump from the
      sibling followup will pull in `rustls-pemfile` 2).
- [ ] `serial` (the 2017-vintage serial-port crate) replaced or removed —
      check who still pulls it in (likely a transitive in a tools/build
      crate).
- [ ] `rand` upgraded past the unsound `rng()` cutoff or pinned per the
      RustSec mitigation note.
- [ ] All 6 RUSTSEC IDs removed from the gate `--ignore` list.
- [ ] `scripts/build.sh gate` passes with the ignores removed.

## Dependencies

- blocked-by: nothing
- pairs-with: rustls-webpki bump (it will likely drop the rustls-pemfile
  1.x dependency on its own)

## Notes

- Effort M: 6 separate dependencies, each needing audit of who pulls
  them in and what the replacement looks like.
- This is mostly hygiene; none of these advisories is a sandbox escape
  or remote-exploitable vuln.
