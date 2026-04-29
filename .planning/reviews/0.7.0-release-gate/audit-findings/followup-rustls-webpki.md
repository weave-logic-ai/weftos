## Source

- audit: `cargo audit` cold run 2026-04-28 (WEFT-104)
- report: `.planning/reviews/0.7.0-release-gate/audit-findings/cargo-audit-cold-run-2026-04-28.md`

## Problem / gap

Two `rustls-webpki` versions ship in the lockfile (0.101.7 and 0.103.10),
both carrying three active advisories: a reachable panic in CRL parsing
(RUSTSEC-2026-0104), name constraints accepted for wildcard-named certs
(RUSTSEC-2026-0099), and name constraints for URI names incorrectly
accepted (RUSTSEC-2026-0098).

Both versions arrive transitively through reqwest, hyper-rustls, quinn,
and tokio-rustls — bumping in one place without aligning the rest leaves
the older copy in the lockfile.

## Acceptance criteria

- [ ] `rustls-webpki` upgraded to ≥ `0.103.13` everywhere (no duplicate
      0.101.x in the lockfile).
- [ ] `rustls 0.23+` consistency across reqwest, hyper-rustls, tonic,
      quinn, tokio-rustls.
- [ ] All 3 RUSTSEC IDs removed from the gate `--ignore` list:
      RUSTSEC-2026-0098, RUSTSEC-2026-0099, RUSTSEC-2026-0104.
- [ ] `scripts/build.sh gate` passes with the ignores removed.
- [ ] No regressions in TLS-using crates (`clawft-services`,
      `clawft-channels`, `clawft-plugin-oauth2`, etc.).

## Dependencies

- blocked-by: nothing
- pairs-with: ws02 deps — wasmtime bump

## Notes

- Effort M: coordinated cargo-update across the TLS dependency graph.
- May require touching Cargo.toml in multiple workspace crates if any
  pin a `rustls` minor version directly.
