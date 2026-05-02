# SPARC Orchestrator: Browser WASM Workstream

**Workstream ID**: W-BROWSER
**Date**: 2026-02-24
**Status**: Planning Complete
**Estimated Duration**: 6 weeks (Phases BW1-BW6)
**Source Analysis**: `.planning/wasm-browser/00-consensus-plan.md` + 5 supporting specs

---

## 1. Workstream Summary

Run the real `AgentLoop<BrowserPlatform>` and full pipeline (classify -> route -> assemble -> transport -> score -> learn) in the browser via `wasm32-unknown-unknown`, while maintaining zero regressions on the native build.

### Approach

- **Feature flags over new crates**: Gate native deps behind `native` feature (default on); add `browser` feature for browser-specific deps
- **Hybrid architecture**: Real engine in browser WASM; exclude channels, services, CLI, and plugin crates
- **Platform trait is the seam**: `BrowserPlatform` implements the existing `Platform` trait with browser-native backends (fetch API, OPFS, in-memory env)

### Crate Scope

| In Scope (browser build) | Out of Scope |
|---|---|
| `clawft-types` | `clawft-channels` |
| `clawft-platform` | `clawft-services` |
| `clawft-plugin` | `clawft-cli` |
| `clawft-security` | `clawft-plugin-*` (all 7 plugin crates) |
| `clawft-core` | |
| `clawft-llm` | |
| `clawft-tools` | |
| `clawft-wasm` | |

---

## 2. Phase Summary

| Phase | ID | Title | Goal | Duration |
|---|---|---|---|---|
| 1 | BW1 | Foundation | `clawft-types` + `clawft-platform` (traits) + `clawft-security` compile for `wasm32-unknown-unknown` | Week 1-2 |
| 2 | BW2 | Core Engine | `clawft-core` compiles for WASM with `--features browser` | Week 2-3 |
| 3 | BW3 | LLM Transport | `clawft-llm` works in browser with CORS support | Week 3-4 |
| 4 | BW4 | BrowserPlatform | Full `Platform` trait implementation for browser (HTTP, FS, Env) | Week 4-5 |
| 5 | BW5 | WASM Entry + Tools | Real `AgentLoop<BrowserPlatform>` wired via `wasm-bindgen` | Week 5-6 |
| 6 | BW6 | Integration | End-to-end validation, test harness, deployment guide | Week 6+ |

---

## 3. Dependencies

### Internal Dependencies (Phase-to-Phase)

```
BW1 (Foundation) -- no deps
  |
  v
BW2 (Core Engine) -- depends on BW1 (feature flags in types, platform)
  |
  v
BW3 (LLM Transport) -- depends on BW1 (feature flags in types)
  |
  v
BW4 (BrowserPlatform) -- depends on BW1 (trait definitions), BW3 (CORS config types)
  |
  v
BW5 (WASM Entry) -- depends on BW2, BW3, BW4 (all must compile for WASM)
  |
  v
BW6 (Integration) -- depends on BW5 (working WASM module)
```

### External Dependencies

- **None for BW1-BW5**: This workstream is self-contained
- **BW6 can integrate with UI sprint** (S1/S2/S3): The WASM module produced by BW5 is the input to the UI sprint. The UI sprint builds the JavaScript shell (chat interface, settings, file browser) that consumes the WASM module's `init()`, `send_message()`, `set_env()` exports

### New Cargo Dependencies

| Dependency | Version | Scope | Introduced In |
|---|---|---|---|
| `wasm-bindgen` | 0.2 | `clawft-platform`, `clawft-wasm` | BW1, BW5 |
| `wasm-bindgen-futures` | 0.4 | `clawft-platform`, `clawft-core`, `clawft-wasm` | BW1, BW2 |
| `web-sys` | 0.3 | `clawft-platform`, `clawft-wasm` | BW1, BW4 |
| `js-sys` | 0.3 | `clawft-platform`, `clawft-core` | BW1, BW2 |
| `gloo-net` | 0.6 | `clawft-platform` (optional) | BW4 |
| `gloo-timers` | 0.3 | `clawft-core` (optional) | BW2 |
| `getrandom` | 0.2 | workspace (add `js` feature) | BW1 |

---

## 4. Exit Criteria Per Phase

### BW1: Foundation
- `cargo check --target wasm32-unknown-unknown -p clawft-types --no-default-features` passes
- `cargo check --target wasm32-unknown-unknown -p clawft-platform --no-default-features` passes (traits only)
- `cargo check --target wasm32-unknown-unknown -p clawft-security` passes (already works)
- All existing tests pass: `cargo test --workspace`
- Native CLI builds: `cargo build --release --bin weft`
- Existing WASI build works: `cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm`
- ADR-027 written, feature-flags.md written

### BW2: Core Engine
- `cargo check --target wasm32-unknown-unknown -p clawft-core --no-default-features --features browser` passes
- Pipeline modules (classifier, router, tiered_router, traits) compile unchanged
- `runtime.rs` abstraction module exists and compiles for both targets
- All phase gate checks pass (native tests, CLI build, WASI build)

### BW3: LLM Transport
- `cargo check --target wasm32-unknown-unknown -p clawft-llm --no-default-features --features browser` passes
- `ProviderConfig` has `browser_direct` and `cors_proxy` fields with `#[serde(default)]`
- Existing config files parse without errors
- CORS provider docs written

### BW4: BrowserPlatform
- `BrowserPlatform` struct implements `Platform` trait
- `BrowserHttpClient` compiles and implements `HttpClient`
- `BrowserFileSystem` compiles and implements `FileSystem` (OPFS backend)
- `BrowserEnvironment` compiles and implements `Environment`
- All phase gate checks pass

### BW5: WASM Entry + Tools
- `wasm-pack build crates/clawft-wasm --target web --no-default-features --features browser` succeeds
- `init(config_json)`, `send_message(text)`, `set_env(key, value)` exported via `wasm-bindgen`
- `tokio::fs::metadata` leak fixed in `file_tools.rs`
- `canonicalize()` replaced with virtual path normalization for browser
- WASM binary < 500KB gzipped
- Browser build guide, quickstart, API reference docs written
- All phase gate checks pass (most critical gate)

### BW6: Integration
- HTML/JS test harness loads WASM and sends a message through the full pipeline
- OPFS file operations tested (write, read, list, delete)
- Config persists across page reloads
- WASM load time and message latency profiled
- Deployment guide and architecture overview written
- All existing docs updated (README, CLAUDE.md)

---

## 5. Phase Gate Rules

Every phase MUST pass all four checks before merging:

```bash
# Gate 1: Native regression -- all existing tests pass
cargo test --workspace

# Gate 2: Native CLI binary builds
cargo build --release --bin weft

# Gate 3: Existing WASI WASM build works
cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm

# Gate 4: Browser WASM check (for crates modified in this phase)
cargo check --target wasm32-unknown-unknown -p <crate> --no-default-features --features browser
```

### Regression Risk Register (N1-N7)

| ID | Risk | Prevention Rule |
|---|---|---|
| N1 | Bare `cargo build` breaks | Every `dep:X` made optional MUST appear in `default = ["native"]` |
| N2 | Tests fail with default features | All test code compiles under default features; test modules use `#[cfg(test)]` |
| N3 | Downstream crate import breaks | When gating a type behind `native`, add re-export under default feature |
| N4 | `Send` bounds lost on native | Use `#[cfg_attr(feature = "browser", async_trait(?Send))]` + `#[cfg_attr(not(feature = "browser"), async_trait)]` -- never unconditional `?Send` |
| N5 | WASI WASM build breaks | Browser support goes in new `browser` feature; existing defaults unchanged |
| N6 | Clippy warnings from dead code | Use clean feature separation; avoid `#[allow(dead_code)]` |
| N7 | Cargo feature unification | `native` and `browser` are mutually exclusive; never put both in same dep's features |

### Feature Flag Validation Script

Each phase should be verified with `scripts/build.sh gate` (WEFT-409,
2026-04-30: supersedes the never-created `scripts/check-features.sh`):

```bash
# Full phase gate — 12 checks: native + WASI + browser + clippy +
# bundle-size + audit + docs regen
scripts/build.sh gate

# Equivalent fast iteration loop
scripts/build.sh check        # native cargo check --workspace
scripts/build.sh wasi         # wasm32-wasip2 build
scripts/build.sh browser      # wasm32-unknown-unknown build
```

The full target matrix lives inline in `scripts/build.sh` (search for
`phase_gate()`); add new targets there rather than to a parallel
script.

---

## 6. CI Matrix Additions

The existing CI pipeline (`pr-gates.yml`) is extended, not replaced:

```yaml
# NEW jobs added alongside existing clippy, test, wasm-size, binary-size, smoke-test

wasm-browser-check:
  name: Browser WASM compilation check
  steps:
    - cargo check --target wasm32-unknown-unknown -p clawft-wasm --no-default-features --features browser
    - cargo check --workspace  # Sanity check native still works

wasm-browser-size:
  name: Browser WASM size gate
  steps:
    - wasm-pack build crates/clawft-wasm --target web --no-default-features --features browser
    # Gate: < 500KB gzipped
```

---

## 7. Documentation Plan

| Phase | Document | Location |
|---|---|---|
| BW1 | ADR-027: Browser WASM Support | `docs/architecture/adr-027-browser-wasm-support.md` |
| BW1 | Feature flag development guide | `docs/development/feature-flags.md` |
| BW3 | Provider CORS setup guide | `docs/browser/cors-provider-setup.md` |
| BW3 | Config schema reference (browser fields) | `docs/browser/config-schema.md` |
| BW5 | Browser build guide | `docs/browser/building.md` |
| BW5 | Browser quickstart | `docs/browser/quickstart.md` |
| BW5 | wasm-bindgen API reference | `docs/browser/api-reference.md` |
| BW6 | Browser deployment guide | `docs/browser/deployment.md` |
| BW6 | Architecture overview (browser vs native) | `docs/browser/architecture.md` |
| BW6 | README.md update | `README.md` (add Browser section) |
| BW6 | CLAUDE.md update | `CLAUDE.md` (add browser build commands) |

---

## 8. Success Criteria (Workstream-Level)

1. `cargo build --target wasm32-unknown-unknown -p clawft-wasm --features browser` succeeds
2. WASM module loads in browser, accepts config JSON, initializes `AgentLoop<BrowserPlatform>`
3. Full pipeline executes: classify -> route -> assemble -> LLM call -> tool use -> response
4. File tools (read/write/edit) work via OPFS
5. Config persists across page reloads via IndexedDB/OPFS
6. All existing native tests pass with default features (zero regressions)
7. WASM binary < 500KB gzipped
8. At least one LLM provider works from browser (Anthropic direct or proxied)

---

## 9. Risk Register

| ID | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | CORS blocks direct LLM API calls | High | Anthropic `browser_direct` header; lightweight proxy for others |
| R2 | WASM binary size exceeds 500KB gzip | Medium | `wasm-opt -Oz`, disable `serde_yaml` in browser, audit deps |
| R3 | `async_trait` Send bounds incompatible | High | Conditional `#[async_trait(?Send)]` for browser feature |
| R4 | OPFS browser support gaps | Low | Chrome 102+, Firefox 111+, Safari 15.2+; in-memory fallback |
| R5 | API key exposure in browser | Medium | Web Crypto API encryption; warn users; proxy option |
| R6 | `getrandom` needs `js` feature | Low | Add `getrandom = { version = "0.2", features = ["js"] }` |
| R7 | `chrono` needs `js-sys` feature | Low | Add feature flag or use `js_sys::Date::now()` directly |
| R8 | Web Worker overhead | Low | Structured clone is fast; batch tool results |
