## Source

- audit: `cargo audit` cold run 2026-04-28 (WEFT-104)
- report: `.planning/reviews/0.7.0-release-gate/audit-findings/cargo-audit-cold-run-2026-04-28.md`

## Problem / gap

`wasmtime 33.0.2` (transitive via `clawft-kernel/runtime/wasm.rs`) ships
with 14 active RustSec advisories spanning sandbox escapes (Cranelift
aarch64, Winch x86-64), heap OOB writes in component-model string
transcoding, host data leakage between pooling allocator instances, and
guest-controlled resource exhaustion in WASI.

These are currently `--ignore`d in `scripts/build.sh gate` and
`.github/workflows/pr-gates.yml` so the cargo-audit gate stays green.

## Acceptance criteria

- [ ] `wasmtime` upgraded to ≥ `43.0.1` (or the closest LTS in the
      `36.0.7+ / 42.0.2+ / 43.0.1+` set chosen for our wasi-component
      surface).
- [ ] `cap-rand` (transitive via wasmtime-wasi) re-resolves cleanly.
- [ ] All 14 RUSTSEC IDs removed from the gate `--ignore` list:
      RUSTSEC-2026-{0020,0021,0085,0086,0087,0088,0089,0091,0092,0093,
      0094,0095,0096} + RUSTSEC-2025-0118 + RUSTSEC-2026-0006.
- [ ] `scripts/build.sh gate` passes with the ignores removed.
- [ ] `clawft-wasm` + `clawft-kernel/runtime/wasm.rs` compile against the
      new component-model API (breaking changes between wasmtime 33 and
      42 affect `Linker`, `Component`, and `Store::limiter`).

## Dependencies

- blocked-by: nothing
- blocks: closing the cargo-audit followup cluster
- pairs-with: ws02 deps — rustls-webpki bump

## Notes

- Effort L: not just a Cargo.toml bump; component-model API churn is
  non-trivial.
- This is the largest of the three audit-followup items; the wasmtime
  upgrade alone clears 14 of the 24 advisories.
