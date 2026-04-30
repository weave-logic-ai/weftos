# Phase BW2: Core Engine

**Phase ID**: BW2
**Workstream**: W-BROWSER
**Duration**: Week 2-3
**Depends On**: BW1 (Foundation)
**Goal**: Make `clawft-core` compile for WASM with `--no-default-features --features browser`

---

## S -- Specification

### What Changes

This phase feature-gates all native-only dependencies in `clawft-core` and introduces a `runtime.rs` abstraction module that provides async primitives (`spawn`, `sleep`, `select`, `channel`) backed by either tokio (native) or futures + wasm-bindgen-futures (browser).

### Five Blockers in clawft-core

| # | Blocker | File | Line(s) | Fix |
|---|---|---|---|---|
| 1 | `notify` hard dep | `Cargo.toml:43` | `notify = { workspace = true }` | Make optional behind `native` |
| 2 | `skill_watcher` module uses `notify` | `src/agent/mod.rs:9` | `pub mod skill_watcher;` | Gate behind `#[cfg(feature = "native")]` |
| 3 | `tokio-util` CancellationToken | `Cargo.toml:36`, `src/agent/loop_core.rs:31` | `use tokio_util::sync::CancellationToken;` | Use feature-gated abstraction from clawft-plugin |
| 4 | `dirs` crate | `Cargo.toml:33` | `dirs = { workspace = true }` | Make optional behind `native` |
| 5 | `SystemTime` in TieredRouter | `src/pipeline/tiered_router.rs:405-408` | `std::time::SystemTime::now()` | Feature-gate: use `js_sys::Date::now()` on browser |

### Files to Change

| File | Change | Task |
|---|---|---|
| `crates/clawft-core/Cargo.toml` | Add `native`/`browser` features; make `notify`, `dirs`, `tokio-util`, `tokio` optional/conditional | P2.1 |
| `crates/clawft-core/src/agent/mod.rs` | Gate `skill_watcher` behind `#[cfg(feature = "native")]` | P2.2 |
| `crates/clawft-core/src/runtime.rs` | New file: async runtime abstraction | P2.3 |
| `crates/clawft-core/src/lib.rs` | Add `pub mod runtime;` | P2.3 |
| `crates/clawft-core/src/agent/loop_core.rs` | Replace `tokio_util::sync::CancellationToken` with feature-gated version; replace `tokio::select!` with runtime abstraction | P2.4 |
| `crates/clawft-core/src/pipeline/tiered_router.rs` | Feature-gate `SystemTime` usage at lines 405-408 | P2.5 |
| Any file using `dirs::home_dir()` | Replace with `Platform::fs()::home_dir()` or feature-gate | P2.6 |

### Exact Cargo.toml Changes

#### `crates/clawft-core/Cargo.toml`

Current:
```toml
[features]
default = ["full"]
full = []
vector-memory = ["dep:rand", "dep:instant-distance"]
rvf = ["vector-memory", "dep:rvf-runtime", "dep:rvf-types", "dep:sha2", "dep:reqwest"]
signing = ["dep:ed25519-dalek", "dep:sha2", "dep:rand"]

[dependencies]
clawft-types = { workspace = true }
clawft-platform = { workspace = true }
clawft-llm = { workspace = true }
clawft-plugin = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
dirs = { workspace = true }
rand = { workspace = true, optional = true }
serde_yaml = { workspace = true }
tokio-util = { workspace = true }
futures-util = { workspace = true }
# ... optional deps
notify = { workspace = true }
```

New:
```toml
[features]
default = ["full", "native"]
full = []
native = [
    "dep:notify",
    "dep:dirs",
    "dep:tokio",
    "dep:tokio-util",
    "clawft-platform/native",
    "clawft-llm/native",
    "clawft-plugin/native",
    "clawft-types/native",
]
browser = [
    "clawft-platform/browser",
    "clawft-llm/browser",
    "dep:wasm-bindgen-futures",
    "dep:js-sys",
]
vector-memory = ["dep:rand", "dep:instant-distance"]
rvf = ["vector-memory", "dep:rvf-runtime", "dep:rvf-types", "dep:sha2", "dep:reqwest"]
signing = ["dep:ed25519-dalek", "dep:sha2", "dep:rand"]

[dependencies]
# Always available
clawft-types = { workspace = true, default-features = false }
clawft-platform = { workspace = true, default-features = false }
clawft-llm = { workspace = true, default-features = false }
clawft-plugin = { workspace = true, default-features = false }
async-trait = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
serde_yaml = { workspace = true }
futures-util = { workspace = true }
percent-encoding = "2"
fnv = "1"

# Native only
tokio = { workspace = true, optional = true }
tokio-util = { workspace = true, optional = true }
dirs = { workspace = true, optional = true }
notify = { workspace = true, optional = true }

# Browser only
wasm-bindgen-futures = { version = "0.4", optional = true }
js-sys = { version = "0.3", optional = true }

# Optional across both
rand = { workspace = true, optional = true }
instant-distance = { workspace = true, optional = true }
rvf-runtime = { workspace = true, optional = true }
rvf-types = { workspace = true, optional = true }
sha2 = { workspace = true, optional = true }
reqwest = { workspace = true, optional = true }
ed25519-dalek = { workspace = true, optional = true }
```

---

## P -- Pseudocode

### runtime.rs Module

```rust
//! Async runtime abstraction.
//!
//! Provides platform-agnostic wrappers for async primitives. On native,
//! delegates to tokio. On browser WASM, uses futures crate + wasm-bindgen-futures.

// ── spawn ─────────────────────────────────────────────────────────

/// Spawn a future on the current runtime.
///
/// On native: tokio::spawn (requires Send + 'static)
/// On browser: wasm_bindgen_futures::spawn_local ('static, !Send ok)
#[cfg(feature = "native")]
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future)
}

#[cfg(feature = "browser")]
pub fn spawn<F>(future: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
}

// ── sleep ─────────────────────────────────────────────────────────

/// Sleep for a duration.
#[cfg(feature = "native")]
pub async fn sleep(duration: std::time::Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(feature = "browser")]
pub async fn sleep(duration: std::time::Duration) {
    let millis = duration.as_millis() as i32;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        // Use web_sys::window().set_timeout_with_callback_and_timeout_and_arguments_0
        // For simplicity, use gloo_timers if available
        let _ = web_sys::window()
            .expect("no window")
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                millis,
            );
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.ok();
}

// ── channels ──────────────────────────────────────────────────────

/// MPSC channel (multi-producer, single-consumer).
#[cfg(feature = "native")]
pub use tokio::sync::mpsc;

#[cfg(feature = "browser")]
pub mod mpsc {
    pub use futures_channel::mpsc::{
        channel, unbounded, Receiver, Sender,
        UnboundedReceiver, UnboundedSender,
    };
}

// ── oneshot ───────────────────────────────────────────────────────

#[cfg(feature = "native")]
pub use tokio::sync::oneshot;

#[cfg(feature = "browser")]
pub mod oneshot {
    pub use futures_channel::oneshot::{channel, Receiver, Sender};
}

// ── now ───────────────────────────────────────────────────────────

/// Get current time as milliseconds since epoch.
/// Safe on both native and browser WASM.
#[cfg(feature = "native")]
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(feature = "browser")]
pub fn now_millis() -> u64 {
    js_sys::Date::now() as u64
}
```

### Gating skill_watcher

```rust
// crates/clawft-core/src/agent/mod.rs

pub mod agents;
pub mod context;
pub mod helpers;
pub mod loop_core;
pub mod memory;
pub mod sandbox;
pub mod skill_autogen;
pub mod skills;
pub mod skills_v2;
pub mod verification;

#[cfg(feature = "native")]
pub mod skill_watcher;
```

### Replacing CancellationToken in loop_core.rs

```rust
// crates/clawft-core/src/agent/loop_core.rs

// Replace:
//   use tokio_util::sync::CancellationToken;
// With:
use clawft_plugin::CancellationToken;

// The AgentLoop struct field stays the same:
//   cancel: Option<CancellationToken>,

// The run() method's select! must be feature-gated:
pub async fn run(&self) -> clawft_types::Result<()> {
    info!("agent loop started, waiting for messages");

    loop {
        let msg = if let Some(ref token) = self.cancel {
            #[cfg(feature = "native")]
            {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        info!("agent loop cancelled via token, exiting");
                        break;
                    }
                    msg = self.bus.consume_inbound() => msg,
                }
            }
            #[cfg(feature = "browser")]
            {
                // In browser, use futures::select! or a simpler approach
                use futures_util::future::{select, Either};
                use std::pin::pin;

                let cancel_fut = pin!(token.cancelled());
                let msg_fut = pin!(self.bus.consume_inbound());

                match select(cancel_fut, msg_fut).await {
                    Either::Left(_) => {
                        info!("agent loop cancelled via token, exiting");
                        break;
                    }
                    Either::Right((msg, _)) => msg,
                }
            }
        } else {
            self.bus.consume_inbound().await
        };

        // ... rest unchanged
    }
    Ok(())
}
```

### Fixing SystemTime in TieredRouter

```rust
// crates/clawft-core/src/pipeline/tiered_router.rs
// Lines 405-408, inside select_model() Random strategy:

TierSelectionStrategy::Random => {
    // Use platform-safe time source for pseudo-random selection
    let seed = crate::runtime::now_millis();
    let idx = (seed as usize) % available.len();
    available[idx].clone()
}
```

---

## A -- Architecture

### How AgentLoop<P> Works with Browser Executor

```
Browser Tab
    |
    v
wasm-bindgen-futures executor (single-threaded)
    |
    v
AgentLoop<BrowserPlatform>::process_message()
    |
    +-- ContextBuilder::build_messages()    -- uses Platform::fs()
    |
    +-- PipelineRegistry::complete()
    |       |
    |       +-- KeywordClassifier::classify()    -- pure computation
    |       +-- TieredRouter::route()            -- pure computation
    |       +-- ContextAssembler::assemble()     -- pure computation
    |       +-- LlmTransport::complete()         -- uses BrowserHttpClient (fetch API)
    |       +-- QualityScorer::score()           -- pure computation
    |       +-- LearningBackend::record()        -- pure computation
    |
    +-- ToolRegistry::execute()             -- uses Platform::fs(), Platform::http()
    |
    +-- SessionManager::save_session()      -- uses Platform::fs()
    |
    v
OutboundMessage -> dispatched via MessageBus -> back to JS
```

Key architectural point: The `AgentLoop` is generic over `P: Platform`. The browser variant `AgentLoop<BrowserPlatform>` uses the exact same code paths as `AgentLoop<NativePlatform>` -- only the platform implementations differ.

### Pipeline Modules That Need Zero Changes

These modules compile unchanged for WASM because they are pure computation:

| Module | File | Why It Works |
|---|---|---|
| `KeywordClassifier` | `pipeline/classifier.rs` | String matching, no I/O |
| `StaticRouter` | `pipeline/router.rs` | Config lookup, `async_trait` |
| `TieredRouter` | `pipeline/tiered_router.rs` | AtomicUsize, tier matching (with SystemTime fix) |
| `PipelineRegistry` | `pipeline/traits.rs` | Trait orchestration, `std::time::Instant` |
| `QualityScorer` | Trait only | Synchronous scoring |
| `LearningBackend` | Trait only | Synchronous learning |
| `ContextAssembler` | Trait only | Trait definition |
| `PermissionResolver` | `pipeline/permissions.rs` | Config-driven, no I/O |
| `security` module | `security.rs` | Truncation, validation |

### MessageBus in Browser

The `MessageBus` uses `tokio::sync::mpsc` channels. For browser, this needs to use `futures_channel::mpsc` instead. The bus is internal to `clawft-core`, so the abstraction is straightforward:

```rust
// In bus.rs, use the runtime module's channel abstractions:
use crate::runtime::mpsc;
```

The `MessageBus` module will need its channel type swapped. This is a contained change within `clawft-core/src/bus.rs`.

---

## R -- Refinement

### Regression Risks

| ID | Risk | Prevention |
|---|---|---|
| N1 | `cargo build` fails because `tokio` is now optional in clawft-core | `default = ["full", "native"]` includes `dep:tokio`. Bare `cargo build` uses default features |
| N2 | Test code in `loop_core.rs` uses `tokio::test`, `NativePlatform` | Tests compile under default features which include `native`. `#[cfg(test)]` blocks use `#[tokio::test]` which is available via default |
| N3 | `clawft-tools` depends on `clawft-core` and uses its types | `clawft-tools` will use `clawft-core` with default features until BW5. No breakage |
| N4 | `futures_util::future::join_all` used in loop_core.rs:631 | `futures-util` is not gated (always available, WASM-safe). No change needed |
| N7 | Feature unification: something enables `clawft-core/native` transitively | Only explicit feature activation enables native. `default-features = false` on upstream deps prevents accidental activation |

### What About the MessageBus?

The `MessageBus` in `clawft-core/src/bus.rs` currently uses `tokio::sync::mpsc`. Two options:

1. **Feature-gate the channel type** in bus.rs to use `futures_channel::mpsc` on browser
2. **Use the `runtime::mpsc` abstraction** from the new runtime module

Option 2 is preferred for consistency. The bus is internal to clawft-core and not exposed publicly.

### What About `serde_yaml`?

`serde_yaml` is a hard dependency in clawft-core (line 35 of Cargo.toml). It compiles for WASM but adds ~200KB to binary size. For BW2, leave it as-is. In BW5 (binary size audit), consider:
- Making it optional behind `yaml` feature (default on)
- Browser builds can exclude it if JSON-only config is acceptable

### What About `std::time::Instant`?

`std::time::Instant` is used in `pipeline/traits.rs` for latency measurement. On `wasm32-unknown-unknown`, `Instant::now()` panics in some runtimes. Options:
- Use the `instant` crate (drop-in replacement, uses `performance.now()` on WASM)
- Feature-gate with `js_sys::Date::now()`
- The `web-time` crate provides `Instant` for WASM

Recommended: Add `web-time` or `instant` as a dependency for the browser feature. This is a small, well-maintained crate.

### Testing Strategy

1. **Compilation**: `cargo check --target wasm32-unknown-unknown -p clawft-core --no-default-features --features browser`
2. **Native regression**: `cargo test --workspace` -- all 823+ tests pass
3. **Pipeline verification**: The pipeline modules should need zero changes. If compilation fails in any pipeline module, it indicates an unexpected native dependency leak that must be fixed.

---

## C -- Completion

### Exit Criteria

- [ ] `cargo check --target wasm32-unknown-unknown -p clawft-core --no-default-features --features browser` passes
- [ ] `crates/clawft-core/src/runtime.rs` exists with `spawn`, `sleep`, `now_millis`, channel abstractions
- [ ] `skill_watcher` module gated behind `#[cfg(feature = "native")]`
- [ ] `notify` dependency is optional
- [ ] `tokio-util` dependency is optional (via clawft-plugin feature propagation)
- [ ] `dirs` dependency is optional
- [ ] `SystemTime` usage in tiered_router.rs replaced with `runtime::now_millis()`
- [ ] Pipeline modules (classifier, router, tiered_router, traits) compile unchanged
- [ ] `cargo test --workspace` -- zero regressions
- [ ] `cargo build --release --bin weft` -- native CLI builds
- [ ] `cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm` -- WASI build works
- [ ] `scripts/build.sh gate` passes (WEFT-409: supersedes the never-created `scripts/check-features.sh`)

### Test Commands

```bash
# Browser WASM check
cargo check --target wasm32-unknown-unknown -p clawft-core --no-default-features --features browser

# Native regression
cargo test --workspace
cargo clippy --workspace
cargo build --release --bin weft

# WASI regression
cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm

# Dependency tree verification
cargo tree -p clawft-core --no-default-features --features browser
# Should show NO tokio, notify, dirs, tokio-util

cargo tree -p clawft-core
# Should show tokio, notify, dirs, tokio-util (default features)
```

### Phase Gate

```bash
#!/bin/bash
set -euo pipefail

echo "=== Gate 1: Native tests ==="
cargo test --workspace

echo "=== Gate 2: Native CLI build ==="
cargo build --release --bin weft

echo "=== Gate 3: WASI WASM build ==="
cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm

echo "=== Gate 4: Browser WASM check ==="
cargo check --target wasm32-unknown-unknown -p clawft-core --no-default-features --features browser

echo "BW2 phase gate PASSED"
```
