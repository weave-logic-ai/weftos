# ADR-044: wasm32-wasip1 as WASI Build Target

**Date**: 2026-04-03
**Status**: Accepted
**Deciders**: Sprint 11 Symposium Track 3 (TD-13, HP-9, W49)

## Context

WeftOS builds a WASM artifact (`clawft-wasm` crate) for server-side WASM execution via wasmtime. The CI/release pipeline must target a specific WASM+WASI platform. Two options exist:

**wasm32-wasip1**: The stable, widely-supported WASI preview 1 target. Available in all Rust toolchains since 2023. Compatible with wasmtime, wasmer, and other WASI runtimes. Uses a POSIX-like capability model with file descriptors.

**wasm32-wasip2**: The newer WASI preview 2 target based on the Component Model. Provides typed interfaces, composable components, and a richer capability model. Stabilized more recently and not yet supported by all runtimes and toolchains.

The `scripts/build.sh` WASI command (line 145-155) currently builds for `wasm32-wasip1`:
```bash
cargo build --target wasm32-wasip1 --profile "$profile" -p clawft-wasm
```

Sprint 11 Symposium resolved three related items:
- **TD-13**: "Standardize on wasip2 for CI/release, retain wasip1 in build.sh as secondary"
- **HP-9**: "Standardize on wasip1 or wasip2?" -- resolved as "wasip2 for CI/release; wasip1 as secondary target"
- **W49**: "Fix WASM target mismatch: standardize on wasip2" -- listed as P2/Sprint 12 work

The decision to standardize on wasip2 was made, but the build script still targets wasip1 pending the Sprint 12 migration.

## Decision

The current CI/release pipeline builds for `wasm32-wasip1`. The `scripts/build.sh` `cmd_wasi()` function targets `wasm32-wasip1` with the `release-wasm` profile, producing `target/wasm32-wasip1/${profile}/clawft_wasm.wasm`. A separate `cmd_browser()` function targets `wasm32-unknown-unknown` with `--no-default-features --features browser` for browser WASM builds.

The transition to `wasm32-wasip2` is planned for Sprint 12 (W49) and will:
1. Change the primary WASI target in `scripts/build.sh` from `wasip1` to `wasip2`
2. Update the `wasmtime-wasi` dependency configuration for Component Model support
3. Retain `wasip1` as a secondary target for backward compatibility with older runtimes
4. Verify that existing WASM tool modules compile and pass tests under wasip2

Until the migration completes, `wasm32-wasip1` remains the shipping target.

## Consequences

### Positive
- wasip1 is stable and universally supported by WASI runtimes, minimizing deployment friction
- The build script infrastructure (`cmd_wasi`, `cmd_browser`, profile selection) is already in place
- Retaining wasip1 as a secondary target after migration preserves compatibility with constrained environments
- The migration is bounded to Sprint 12 with clear work items (W49)

### Negative
- wasip1 lacks the Component Model, preventing composable WASM components and typed interfaces
- WASM tool authors targeting weftos today must write to the wasip1 capability model, which will change when wasip2 arrives
- The gap between the decision (TD-13: standardize on wasip2) and the implementation (still wasip1) creates confusion about which target to develop against
- Existing WASM modules may require rework during the wasip2 migration if they rely on wasip1-specific APIs

### Neutral
- The browser WASM target (`wasm32-unknown-unknown`) is unaffected by the wasip1/wasip2 decision
- The `clawft-wasm` crate's feature-gated architecture (`#[cfg(feature = "browser")]`) isolates platform-specific code
- Both targets produce standard WASM binaries; the difference is in the system interface, not the instruction set
