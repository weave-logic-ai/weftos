# cargo audit cold run — 2026-04-28

Source: WEFT-104 (ws02: tooling — add cargo audit to scripts/build.sh gate
and CI). First end-to-end run of `cargo audit` against the workspace
`Cargo.lock` at the head of `development-0.7.0`.

`cargo audit` version: 0.22.1 (cargo-audit-audit binary, advisory DB synced
from RustSec on the run date).

## Headline

- **21 vulnerability finding rows** across **18 unique advisory IDs**.
- **8 warning rows** across **6 unique advisory IDs** (5 unmaintained, 1
  unsound — `rand` 0.8.5 + 0.9.2 share the same `RUSTSEC-2026-0097`).
- **24 unique advisory IDs** total in the gate ignore-list.
- All findings are transitive — none originate from a `clawft-*` direct
  dependency that we control with a single bump.
- The two large clusters: **wasmtime 33.0.2** (15 advisories, all reachable
  from `clawft-kernel`'s wasm host — 14 from RUSTSEC-2026-* plus
  RUSTSEC-2025-0118) and **rustls-webpki 0.101.7 + 0.103.10**
  (3 distinct advisories, x2 versions = 6 finding rows, reachable through
  reqwest / hyper-rustls / quinn).

## Why this is not a 0.7.0-blocking fix-up

The task instruction for WEFT-104 was explicitly _XS effort, pure tooling_:
wire the gate so the next round of advisories is caught. Wiping out 24
distinct advisories — most of them gated behind `wasmtime 36/42/43` or
`rustls-webpki 0.103.13` — is a multi-day dependency-untangling exercise:

- `wasmtime 33.x → 36.x/42.x/43.x` is a major-version jump for the kernel's
  wasm host. The component-model API surface has churn between 33 and 42
  that touches `clawft-wasm` and `clawft-kernel/runtime/wasm.rs`.
- `rustls-webpki 0.101 → 0.103` requires `rustls 0.23+` consistency across
  reqwest, hyper-rustls, tonic, quinn, and tokio-rustls — a coordinated
  cargo-update + downstream API touchups.

Both are tracked as their own 0.8.x deps cycle followups (see WEFT-N below).
Filing 24 separate advisory-row items would be noise — the audit lists
them, the followup item links to this report.

## Gate scaffold (this commit)

- `scripts/build.sh gate` gains a 12th check: `cargo audit`. Today it runs
  with `--ignore` flags covering the 18 vulnerability IDs + 6 warning IDs
  enumerated below, so the gate stays green. Each `--ignore` carries a
  trailing comment naming the advisory family and the followup WEFT-N.
- `.github/workflows/pr-gates.yml` gains a `cargo-audit` job using the same
  ignore-list. New advisories that are NOT in the ignore-list will fail
  the gate.
- The TODO comment in `cmd_gate` points at this file.

## Followup items (0.8.x)

| WEFT-N | Title | Advisory cluster |
|--------|-------|------------------|
| WEFT-551 | ws02: deps — bump wasmtime 33 → 43 to clear 15 RUSTSEC advisories | `wasmtime`, `cap-rand` |
| WEFT-552 | ws02: deps — bump rustls-webpki via rustls/reqwest/quinn alignment | `rustls-webpki` |
| WEFT-553 | ws02: deps — replace unmaintained crates and unsound rand for cargo-audit cleanup | `bincode`, `instant`, `paste`, `rustls-pemfile`, `serial`, `rand` |

Once those land, the corresponding `--ignore` lines come out of
`scripts/build.sh` and `.github/workflows/pr-gates.yml`.

## Full advisory list

### Vulnerabilities (18 unique IDs, 21 finding rows)

Cluster: **wasmtime 33.0.2 → upgrade ≥43.0.1**

- RUSTSEC-2026-0020 — Guest-controlled resource exhaustion in WASI
- RUSTSEC-2026-0021 — Panic adding excessive fields to `wasi:http/types.fields`
- RUSTSEC-2026-0085 — Panic when lifting `flags` component value
- RUSTSEC-2026-0086 — Host data leakage with 64-bit tables and Winch
- RUSTSEC-2026-0087 — Wasmtime segfault with `f64x2.splat` on Cranelift x86-64
- RUSTSEC-2026-0088 — Data leakage between pooling allocator instances
- RUSTSEC-2026-0089 — Host panic when Winch executes `table.fill`
- RUSTSEC-2026-0091 — Out-of-bounds write or crash transcoding component strings
- RUSTSEC-2026-0092 — Panic transcoding misaligned UTF-16
- RUSTSEC-2026-0093 — Heap OOB read in component model UTF-16/latin1 transcoding
- RUSTSEC-2026-0094 — Improperly masked return from `table.grow` (Winch)
- RUSTSEC-2026-0095 — Sandbox-escape with Winch backend
- RUSTSEC-2026-0096 — Sandbox escape on aarch64 Cranelift
- RUSTSEC-2025-0118 — Unsound API access to shared linear memory
- RUSTSEC-2026-0006 — Wasmtime segfault with `f64.copysign` on x86-64

Cluster: **rustls-webpki → upgrade ≥0.103.13**

- RUSTSEC-2026-0098 — Name constraints for URI names incorrectly accepted
- RUSTSEC-2026-0099 — Name constraints accepted for wildcard names
- RUSTSEC-2026-0104 — Reachable panic in CRL parsing

### Warnings (6 unique, 8 finding rows)

- RUSTSEC-2025-0141 — `bincode` is unmaintained (1.3.3 + 2.0.1)
- RUSTSEC-2024-0384 — `instant` is unmaintained
- RUSTSEC-2024-0436 — `paste` is unmaintained
- RUSTSEC-2025-0134 — `rustls-pemfile` 1.x is unmaintained
- RUSTSEC-2017-0008 — `serial` is unmaintained
- RUSTSEC-2026-0097 — `rand` 0.8.5 + 0.9.2 unsound with custom logger

## How to reproduce

```bash
cargo install --locked cargo-audit  # if not present
cargo audit                          # cold (no ignores)
scripts/build.sh gate                # gated (with ignore-list)
```
