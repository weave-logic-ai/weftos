# ADR-044: wasm32-wasip2 as WASI Build Target

**Date**: 2026-04-03 (decided), 2026-04-04 (migration shipped in Sprint 12)
**Status**: Accepted -- migration complete
**Deciders**: Sprint 11 Symposium Track 3 (TD-13, HP-9, W49)

## Context

WeftOS builds a WASM artifact (`clawft-wasm` crate) for server-side WASM execution via wasmtime. The CI/release pipeline must target a specific WASM+WASI platform. Two options exist:

**wasm32-wasip1**: The stable, widely-supported WASI preview 1 target. Available in all Rust toolchains since 2023. Compatible with wasmtime, wasmer, and other WASI runtimes. Uses a POSIX-like capability model with file descriptors.

**wasm32-wasip2**: The newer WASI preview 2 target based on the Component Model. Provides typed interfaces, composable components, and a richer capability model. Stabilized more recently and not yet supported by all runtimes and toolchains.

Sprint 11 Symposium resolved three related items:
- **TD-13**: "Standardize on wasip2 for CI/release, retain wasip1 in build.sh as secondary"
- **HP-9**: "Standardize on wasip1 or wasip2?" -- resolved as "wasip2 for CI/release; wasip1 as secondary target"
- **W49**: "Fix WASM target mismatch: standardize on wasip2" -- scheduled into Sprint 12.

This ADR captures the decision and the migration that followed.

## Decision

`wasm32-wasip2` is the canonical WASI target for WeftOS.

- `scripts/build.sh` `cmd_wasi()` builds `wasm32-wasip2` with the `release-wasm` profile, producing `target/wasm32-wasip2/${profile}/clawft_wasm.wasm`. The phase gate exercises the same target.
- `.github/workflows/release-wasi.yml` builds `wasm32-wasip2` on every tag push and attaches `clawft_wasm.wasm` to the GitHub Release. cargo-dist v0.31 doesn't yet support wasip2 in its target matrix (HP-16); this workflow runs alongside `release.yml` to fill the gap.
- `cmd_browser()` continues to target `wasm32-unknown-unknown` with `--no-default-features --features browser` for browser WASM builds. The browser target is independent of the wasip1/wasip2 decision.
- `wasmtime-wasi` is configured for Component Model support in line with wasip2.
- `wasm32-wasip1` is retained as an opt-in secondary target only; the build does not produce or test it on every cycle, but the toolchain still recognises it for users on constrained runtimes.

## Consequences

### Positive
- WASM tool authors get the typed interfaces, composable components, and richer capability model that the Component Model provides.
- The CI gate, the release pipeline, and the local `scripts/build.sh wasi` flow all target the same triple, so "works locally" matches "works in the release artifact" on this axis.
- Sprint 12 closure (W49) eliminated the gap between the resolution (wasip2) and the running pipeline (wasip1) that this ADR originally documented.

### Negative
- wasip2 is younger and less universally supported than wasip1. Older or constrained runtimes (e.g. embedded wasmtime forks, third-party WASI hosts that haven't shipped Component Model support) require a wasip1 build, which is no longer produced by default.
- cargo-dist v0.31 does not include wasip2 in its target matrix (HP-16), which forced the parallel `release-wasi.yml` workflow. When cargo-dist gains wasip2 support, the parallel workflow can collapse back into `release.yml`.

### Neutral
- The browser WASM target (`wasm32-unknown-unknown`) is unaffected by the wasip1/wasip2 decision.
- The `clawft-wasm` crate's feature-gated architecture (`#[cfg(feature = "browser")]`) isolates platform-specific code.
- Both targets produce standard WASM binaries; the difference is in the system interface, not the instruction set.
