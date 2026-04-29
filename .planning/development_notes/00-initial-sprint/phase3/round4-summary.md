# Phase 3 Round 4 Summary

> **HISTORICAL — 2026-02-17 snapshot (WEFT-25, archived 2026-04-28).**
> Phase-3 round-4 summary from the initial Python → Rust port sprint.
> Retained for context only; current state lives in
> `.planning/reviews/0.7.0-release-gate/`.

**Date**: 2026-02-17
**Agents**: 8 concurrent
**Focus**: WASM compilation fix + CI/CD hardening
**Test Count (pre-round)**: 1,048 (from Round 3)

---

## Round 4 Agents

| # | Agent | Task | Status |
|---|-------|------|--------|
| 1 | WASM restructure | Decouple clawft-wasm deps, get wasip1 check passing | In Progress |
| 2 | WASM allocator | Add dlmalloc, size profiling script | In Progress |
| 3 | CI workflow fixes | Fix wasm-build.yml for wasip1, YAML validation | In Progress |
| 4 | Benchmark regression | Regression detection scripts + CI integration | In Progress |
| 5 | Release validation | End-to-end packaging script test | In Progress |
| 6 | CLI integration tests | Review + enhance test coverage | In Progress |
| 7 | Exit criteria review | Comprehensive gap analysis | In Progress |
| 8 | Dev notes (this) | Documentation updates | In Progress |

---

## Key Decision: WASM Target Change

Changed from `wasm32-wasip2` to `wasm32-wasip1` because:
- `wasm32-wasip2` std lib requires Rust 1.87+ (we're on 1.85)
- `wasm32-wasip1` works with Rust 1.85
- The Platform trait implementations don't use WASI preview2 features yet
- Will upgrade to wasip2 when Rust toolchain is updated

## Key Decision: WASM Dependency Decoupling

Removed clawft-core and clawft-platform from clawft-wasm deps because:
- clawft-platform pulls tokio["full"] which doesn't compile for WASM
- clawft-core pulls clawft-llm -> reqwest which has WASM issues
- clawft-wasm now only depends on clawft-types (compiles clean for wasip1)
- WASM platform implementations are self-contained stubs
- Will bridge back to Platform trait when real WASI impls are ready

---

## Objectives

Round 4 addresses the two critical blockers discovered during Round 3:

1. **WASM compilation fails** -- `wasm32-wasip2` requires Rust 1.87+ which we don't have. Switching to `wasm32-wasip1` and decoupling heavy deps from `clawft-wasm`.
2. **CI/CD hardening** -- wasm-build.yml needs updating for the new target, benchmark regression detection scripts need creation, and release packaging needs end-to-end validation.

### Specific Goals

1. Get `cargo check --target wasm32-wasip1 -p clawft-wasm` passing cleanly
2. Fix `.github/workflows/wasm-build.yml` to use `wasm32-wasip1`
3. Add dlmalloc as the WASM allocator (replacing talc, which was the previous plan)
4. Create benchmark regression detection scripts with threshold-based alerts
5. Validate release packaging end-to-end (build, package, checksum, verify)
6. Review and enhance CLI integration test coverage
7. Perform comprehensive exit criteria gap analysis
8. Update all development notes (this document)

---

## Changes from Round 3

### What Was Completed in Round 3

| Deliverable | Notes |
|-------------|-------|
| Benchmark report with real data | `report_benchmarks.md` populated -- 3.5ms startup, 4.6MB binary, 418 inv/s |
| CLI integration tests | `clawft/tests/cli_integration.rs` with assert_cmd |
| Release packaging scripts | `clawft/scripts/release-package.sh` |
| Deployment documentation | `clawft/docs/deployment/docker.md`, `wasm.md`, `release.md` |
| Crate metadata polish | All 9 Cargo.toml files updated with keywords, categories, descriptions |
| Security reference documentation | `clawft/docs/security.md` |
| CHANGELOG | `clawft/CHANGELOG.md` in Keep a Changelog format |
| Development notes | `round3-summary.md`, `phase3-status.md`, `cicd-progress.md` |

### What Round 3 Revealed

- `wasm32-wasip2` target cannot build std on Rust 1.85 (needs 1.87+)
- `clawft-wasm` dependency chain pulls in tokio and reqwest which don't compile for WASM
- Benchmark regression detection was deferred from Round 3 scope
- Release packaging script exists but needs end-to-end validation

---

## Previous Rounds

| Round | Agents | Focus | Tests After |
|-------|--------|-------|-------------|
| 1 | 8 | Security modules (SEC-1/2/3), CI/CD scaffolding, WASM crate skeleton, docs | 960 |
| 2 | 8 | WASM platform stubs, feature flags, build scripts, integration wiring, CI workflows | 1,029 |
| 3 | 8 | Benchmarks, CLI integration tests, release packaging, documentation, metadata polish | 1,048 |
| 4 | 8 | WASM compilation fix, CI hardening, benchmark regression, exit criteria review | TBD |
