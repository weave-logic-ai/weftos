# Phase 3 -- Status Dashboard

> **HISTORICAL — 2026-02-17 snapshot (WEFT-25, archived 2026-04-28).**
> Phase-3 status from the initial Python → Rust port sprint. Tracker
> is no longer live; see
> `.planning/reviews/0.7.0-release-gate/README.md` for current state.

**Last Updated**: 2026-02-17 (Phase 3C Rust upgrade in progress)
**Phase Start**: 2026-02-17
**Rounds Completed**: 4 of 4 (+ Phase 3C stream active)

---

## Stream Status

| Stream | Description | Status | Progress |
|--------|-------------|--------|----------|
| 2I | Security fixes (SEC-1, SEC-2, SEC-3) | COMPLETE | 100% |
| 3A | WASM core (clawft-wasm crate) | CONDITIONAL PASS | ~75% (infrastructure done, real impls deferred) |
| 3B | CI/CD + Polish | COMPLETE | ~95% |
| 3C | Rust toolchain upgrade (1.85 -> 1.93.1) | IN PROGRESS | ~90% (verification agents running) |

---

## Stream 2I: Security Fixes -- COMPLETE

Completed in Round 1 with manual wiring in Round 2.

| Deliverable | Status |
|-------------|--------|
| CommandPolicy (allowlist mode) | DONE |
| UrlPolicy (SSRF protection) | DONE |
| shell_tool.rs wiring | DONE |
| spawn_tool.rs wiring | DONE |
| web_fetch.rs wiring | DONE |
| CLI entry points (agent.rs, gateway.rs) pass policies | DONE |
| Config types (CommandPolicyConfig, UrlPolicyConfig) | DONE |

---

## Stream 3A: WASM Core -- ~75% (CONDITIONAL PASS)

| Deliverable | Status | Round |
|-------------|--------|-------|
| clawft-wasm crate scaffold | DONE | R1 |
| Cargo.toml with WASM dependencies | DONE | R1 |
| WASM entrypoint (lib.rs exports) | DONE | R1 |
| WasiHttpClient stub | DONE | R2 |
| WasiFileSystem stub | DONE | R2 |
| WasiEnvironment (in-memory HashMap) | DONE | R2 |
| WasmPlatform bundle struct | DONE | R2 |
| `native-exec` feature flag (clawft-tools) | DONE | R2 |
| Feature flags in clawft-core | DONE | R2 |
| Feature flags in clawft-cli | DONE | R2 |
| WASM build CI workflow | DONE | R2 |
| Target changed from wasip2 to wasip1 | DONE | R4 |
| Decouple clawft-wasm from clawft-core/platform | DONE | R4 |
| dlmalloc allocator for WASM | DONE | R4 |
| WASM size profiling script | DONE | R4 |
| `cargo check --target wasm32-wasip1 -p clawft-wasm` passes | **DONE** | R4 |
| `cargo build --target wasm32-wasip1 -p clawft-wasm --release` passes | **DONE** | R4 |
| 41 clawft-wasm unit tests passing | DONE | R4 |
| Real WasiHttpClient (WASI HTTP preview2) | DEFERRED | Phase 4 |
| Real WasiFileSystem (WASI FS preview2) | DEFERRED | Phase 4 |
| micro-hnsw-wasm integration | DEFERRED | Phase 4 |
| `cargo check --target wasm32-wasip2` | PENDING | Unblocked by Rust 1.93.1 upgrade -- verification running |
| Size profiling with twiggy | DEFERRED | Phase 4 |
| WASM runtime validation (wasmtime, WAMR) | DEFERRED | Phase 4 |

---

## Stream 3B: CI/CD + Polish -- ~95% (COMPLETE)

| Deliverable | Status | Round |
|-------------|--------|-------|
| `.github/workflows/ci.yml` (build matrix, 5 targets) | DONE | R1 |
| `.github/workflows/release.yml` (tag-triggered release) | DONE | R1 |
| `Dockerfile` (FROM scratch, static musl) | DONE | R1 |
| Benchmark scripts (startup, memory, throughput, size) | DONE | R1 |
| Benchmark runner (`run-all.sh`) | DONE | R1 |
| `.github/workflows/benchmarks.yml` | DONE | R2 |
| `.github/workflows/wasm-build.yml` | DONE | R2 |
| `scripts/cross-compile.sh` | DONE | R2 |
| `scripts/docker-build.sh` | DONE | R2 |
| Benchmark report populated with real data | DONE | R3 |
| CLI integration tests (assert_cmd, 29 tests) | DONE | R3 |
| Release packaging scripts (zip + checksums) | DONE | R3 |
| Deployment documentation (Docker, WASM, release) | DONE | R3 |
| Crate metadata polish (all 9 crates) | DONE | R3 |
| Security reference documentation | DONE | R3 |
| CHANGELOG (Keep a Changelog format) | DONE | R3 |
| WASM CI workflow fix (wasip1 target) | DONE | R4 |
| Benchmark regression detection scripts | DONE | R4 |
| Release packaging scripts (generate-changelog, package, package-all) | DONE | R3-R4 |
| Security integration tests (33 tests) | DONE | R3 |
| All 4 CI workflows valid YAML | DONE | R4 |
| All 13 shell scripts pass syntax check | DONE | R4 |
| `docs/benchmarks/results.md` | MISSING | - |
| Multi-arch Docker images (buildx) | DEFERRED | - |
| macOS code signing | DEFERRED | - |
| Caching optimization (rust-cache tuning) | DEFERRED | - |

---

## Stream 3C: Rust Toolchain Upgrade -- IN PROGRESS

Rust 1.85 -> 1.93.1 upgrade. All code changes applied; verification pending.

| Deliverable | Status | Notes |
|-------------|--------|-------|
| `rust-toolchain.toml` channel 1.85 -> 1.93 | DONE | |
| `Cargo.toml` workspace rust-version 1.85 -> 1.93 | DONE | |
| Fix `let_and_return` in slack.rs, telegram.rs | DONE | 2 fixes (preexisting lint debt) |
| Fix `derivable_impls` in security_policy.rs | DONE | 1 fix (new lint at 1.91+) |
| Fix `collapsible_if` across workspace | DONE | 11 additional warnings found via clean build |
| `.github/workflows/wasm-build.yml` wasip2 primary | DONE | wasip1 retained as fallback |
| `cargo check --workspace` on 1.93.1 | DONE | |
| `cargo clippy --workspace -- -D warnings` on 1.93.1 | PENDING | Verification agents running |
| `cargo test --workspace` on 1.93.1 | PENDING | Verification agents running |
| `cargo check -p clawft-wasm --target wasm32-wasip2` | PENDING | Now available after upgrade |
| `cargo check -p clawft-wasm --target wasm32-wasip1` | PENDING | Backward compat check |

### Files Changed (15 total)

**Config files (2)**:
- `clawft/rust-toolchain.toml`, `clawft/Cargo.toml`

**Clippy fixes (12 files, 3 crates)**:
- `clawft-cli`: `markdown/slack.rs`, `markdown/telegram.rs` (let_and_return)
- `clawft-tools`: `security_policy.rs` (derivable_impls)
- `clawft-types`: `config.rs`, `provider.rs`, `session.rs` (collapsible_if)
- `clawft-core`: `agent/skills.rs`, `pipeline/transport.rs`, `pipeline/llm_adapter.rs` (collapsible_if)
- `clawft-channels`: `discord/channel.rs`, `host.rs`, `slack/channel.rs` (collapsible_if)

**CI workflow (1)**:
- `.github/workflows/wasm-build.yml`

### Key Insight

The pre-upgrade research (`review_rust_update.md`) identified only 3 clippy warnings
because `rustup run 1.93.1 cargo clippy` reused cached 1.85 build artifacts. A clean
build revealed 11 additional `collapsible_if` warnings. Lesson: always `cargo clean`
before testing clippy on a new toolchain.

---

## Test Count Progression

| Milestone | Tests | Delta | Notes |
|-----------|-------|-------|-------|
| Phase 2 complete | 892 | - | All 9 crates, 0 failures, 0 clippy warnings |
| Phase 3 Round 1 | 960 | +68 | Security policy tests, WASM skeleton tests |
| Phase 3 Round 2 | 1,029 | +69 | WASM platform stubs, feature flag tests, CI tests |
| Phase 3 Round 3 | 1,048 | +19 | CLI integration tests, benchmark validation, metadata |
| Phase 3 Round 4 | **1,058** | **+10** | WASM crate tests (41 total), CI hardening, regression detection |

---

## Files Created/Modified Across All Rounds

| Round | Files Created | Files Modified | Total Touched |
|-------|---------------|----------------|---------------|
| R1 | ~18 | ~12 | ~30 |
| R2 | ~12 | ~15 | ~27 |
| R3 | ~10 | ~12 | ~22 |
| R4 | ~5 (est.) | ~10 (est.) | ~15 (est.) |
| **Total** | **~45** | **~49** | **~94** |

### Key Files by Round

**Round 1 (created)**:
- `clawft-tools/src/security_policy.rs`, `clawft-tools/src/url_safety.rs`
- `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- `clawft/Dockerfile`
- `clawft/scripts/bench/startup.sh`, `memory.sh`, `throughput.sh`, `size-check.sh`, `run-all.sh`
- `clawft-wasm/src/lib.rs`, `Cargo.toml`

**Round 2 (created)**:
- `.github/workflows/benchmarks.yml`, `.github/workflows/wasm-build.yml`
- `clawft/scripts/cross-compile.sh`, `clawft/scripts/docker-build.sh`
- `clawft-wasm/src/http.rs`, `fs.rs`, `env.rs`, `platform.rs`
- Feature flag additions across clawft-tools, clawft-core, clawft-cli

**Round 3 (complete)**:
- `clawft/CHANGELOG.md`
- `clawft/docs/deployment/docker.md`, `wasm.md`, `release.md`
- `clawft/docs/security.md`
- `clawft/scripts/release-package.sh`
- `clawft/tests/cli_integration.rs`
- Cargo.toml metadata updates (9 crates)

**Round 4 (complete)**:
- `clawft-wasm/Cargo.toml` dependency restructure (decoupled from clawft-core/clawft-platform)
- `clawft-wasm/src/allocator.rs` (dlmalloc for wasm32 target)
- `.github/workflows/wasm-build.yml` (wasip1 target fix)
- `scripts/bench/regression-check.sh`, `save-results.sh` (benchmark regression detection)
- `scripts/release/generate-changelog.sh`, `package.sh`, `package-all.sh`
- Exit criteria review: `.planning/development_notes/phase3/exit-criteria-review.md`

---

## Codebase Statistics (Current -- Round 4)

| Metric | Value |
|--------|-------|
| Rust source files | ~121 |
| Crates in workspace | 9 |
| CI workflow files | 4 (all valid YAML) |
| Build/bench/release scripts | 13 (all pass syntax check) |
| Unit + integration tests | **1,058** (0 failures, 8 ignored) |
| Clippy warnings | 0 |
| `cargo check --target wasm32-wasip1 -p clawft-wasm` | PASS |
| `cargo check -p clawft-tools --no-default-features` | PASS |
| Documentation files (clawft/docs) | 16 markdown files |

---

## Exit Criteria Checklist

> Full exit criteria review: `phase3/exit-criteria-review.md`

### Phase 3A (WASM Core) -- CONDITIONAL PASS

**MUST HAVE**:
| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 1 | WASM binary builds for wasm target | **PASS** (wasip1; wasip2 pending verification) | `cargo check/build --target wasm32-wasip1 -p clawft-wasm` passes. wasip2 now available with Rust 1.93.1 -- verification pending. |
| 2 | Binary size <= 300 KB | **PARTIAL** | rlib is 142 KB. cdylib not yet configured. |
| 3 | Tests pass in Wasmtime and WAMR | **DEFERRED** | 41 tests pass on native. Runtime validation deferred to Phase 4. |
| 4 | HTTP client works with APIs | **DEFERRED** | Stub exists. Real WASI HTTP deferred to Phase 4. |
| 5 | Config/session via WASI filesystem | **DEFERRED** | Stub exists. Real WASI FS deferred to Phase 4. |

**SHOULD HAVE**:
| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 6 | Vector search (micro-hnsw-wasm) | DEFERRED | Phase 4 |
| 7 | Startup < 50ms in Wasmtime | DEFERRED | Phase 4 |
| 8 | Memory < 10 MB idle | DEFERRED | Phase 4 |

**NICE TO HAVE**: All deferred to Phase 4.

### Phase 3B (CI/CD + Polish) -- PASS

**MUST HAVE**:
| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 1 | CI builds all targets | **PASS** | 5 native targets (ci.yml) + WASM (wasm-build.yml). All 4 workflows valid YAML. |
| 2 | Native binary < 15 MB | **PASS** | size-check.sh enforces limit. |
| 3 | WASM binary < 300 KB enforced | **PASS** | wasm-build.yml asserts 300 KB / 120 KB limits. |
| 4 | Release pipeline | **PASS** | release.yml: tag-triggered, changelog, assets, Docker. |
| 5 | Docker image < 20 MB | **PASS** | FROM scratch + static binary. docker-build.sh validates. |

**SHOULD HAVE**:
| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 6 | Startup < 50ms (10x) | **PASS** | 3.5ms measured (229x faster). |
| 7 | Memory < 10 MB (5x) | **PASS** (est.) | 4.6 MB binary; RSS well under 10 MB. |
| 8 | Throughput > 1000 msg/s | **PARTIAL** | 418 inv/s measured (tool invocation metric differs from raw msg/s). |
| 9 | Benchmark results documented | **PARTIAL** | Data in round3-summary. Standalone results.md missing. |

**NICE TO HAVE**:
| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 10 | Automated changelog | **PASS** | generate-changelog.sh + CHANGELOG.md |
| 11 | Benchmark workflow on release | **PASS** | benchmarks.yml |
| 12 | Size profiling in CI | DEFERRED | twiggy integration not in CI |

---

## Remaining Work (Deferred to Phase 4+)

| Item | Stream | Priority | Notes |
|------|--------|----------|-------|
| Real WasiHttpClient (WASI preview2) | 3A | High | Unblocked by Rust 1.93.1 upgrade |
| Real WasiFileSystem (WASI preview2) | 3A | High | Unblocked by Rust 1.93.1 upgrade |
| cdylib crate type for standalone WASM | 3A | Medium | Currently rlib only |
| micro-hnsw-wasm vector search | 3A | Medium | Optional feature, crate structure supports it |
| wasmtime/WAMR runtime validation | 3A | Medium | After real impls exist |
| Upgrade wasm32-wasip1 to wasip2 | 3A | Medium | Unblocked by Rust 1.93.1; CI updated, code verification pending |
| `docs/benchmarks/results.md` | 3B | Low | Data exists in round3-summary |
| Size profiling (twiggy) in CI | 3B | Low | Nice-to-have |
| Multi-arch Docker images (buildx) | 3B | Low | Deferred |
| macOS code signing | 3B | Low | Deferred |
| rust-cache tuning | 3B | Low | Deferred |
