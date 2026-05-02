# Phase BW1: Foundation

**Phase ID**: BW1
**Workstream**: W-BROWSER
**Duration**: Week 1-2
**Goal**: Make `clawft-types`, `clawft-platform` (traits only), `clawft-plugin`, and `clawft-security` compile for `wasm32-unknown-unknown`

---

## S -- Specification

### What Changes

This phase introduces `native`/`browser` feature flags to the foundation crates and splits platform trait definitions from native implementations. After this phase, the trait-only subset of the platform layer compiles for WASM.

### Files to Change

| File | Change | Task |
|---|---|---|
| `Cargo.toml` (workspace root) | Add `getrandom` workspace dep with `js` feature | P1.0 |
| `crates/clawft-types/Cargo.toml` | Make `dirs` optional behind `native` feature | P1.1 |
| `crates/clawft-types/src/config/mod.rs` | Gate `dirs::home_dir()` call at line 74 | P1.1 |
| `crates/clawft-platform/Cargo.toml` | Add `native`/`browser` features; make `tokio`, `reqwest`, `dirs` optional | P1.2 |
| `crates/clawft-platform/src/lib.rs` | Gate `NativePlatform` + impl behind `#[cfg(feature = "native")]` | P1.2 |
| `crates/clawft-platform/src/http.rs` | Gate `NativeHttpClient` behind `#[cfg(feature = "native")]`; add `?Send` to trait | P1.2, P1.3 |
| `crates/clawft-platform/src/fs.rs` | Gate `NativeFileSystem` behind `#[cfg(feature = "native")]`; add `?Send` to trait | P1.2, P1.3 |
| `crates/clawft-platform/src/env.rs` | Gate `NativeEnvironment` behind `#[cfg(feature = "native")]` | P1.2 |
| `crates/clawft-platform/src/process.rs` | Gate `NativeProcessSpawner` behind `#[cfg(feature = "native")]` | P1.2 |
| `crates/clawft-platform/src/config_loader.rs` | Fix `path.exists()` leak at line 38-39; use `fs.exists()` or accept boolean param | P1.4 |
| `crates/clawft-plugin/Cargo.toml` | Make `tokio-util` optional behind `native` feature | P1.2b |
| `.github/workflows/pr-gates.yml` | Add `wasm-browser-check` job | P1.5 |
| ~~`scripts/check-features.sh`~~ → `scripts/build.sh gate` | Feature-flag validation script. SUPERSEDED (WEFT-409, 2026-04-30): `scripts/build.sh gate` covers the same checks (native + WASI + browser + clippy + bundle-size) and is the canonical entrypoint. The standalone script was never created. | P1.6 |
| `docs/architecture/adr-027-browser-wasm-support.md` | New file: ADR for browser WASM decision | P1.8 |
| `docs/development/feature-flags.md` | New file: feature flag guide | P1.9 |

### Exact Cargo.toml Changes

#### Workspace Root (`Cargo.toml`)

Add `getrandom` to workspace dependencies:

```toml
# After line 98 (rand = "0.8")
getrandom = { version = "0.2", features = ["js"] }
```

#### `crates/clawft-types/Cargo.toml`

Current:
```toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }
uuid = { workspace = true }
dirs = { workspace = true }
```

New:
```toml
[features]
default = ["native"]
native = ["dep:dirs"]

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }
uuid = { workspace = true }
dirs = { workspace = true, optional = true }
```

#### `crates/clawft-platform/Cargo.toml`

Current:
```toml
[dependencies]
clawft-types = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }
reqwest = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
dirs = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
```

New:
```toml
[features]
default = ["native"]
native = ["dep:tokio", "dep:reqwest", "dep:dirs", "clawft-types/native"]
browser = ["dep:wasm-bindgen", "dep:wasm-bindgen-futures", "dep:web-sys", "dep:js-sys"]

[dependencies]
# Always available (trait definitions)
clawft-types = { workspace = true, default-features = false }
async-trait = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }

# Native only
tokio = { workspace = true, optional = true }
reqwest = { workspace = true, optional = true }
dirs = { workspace = true, optional = true }

# Browser only
wasm-bindgen = { version = "0.2", optional = true }
wasm-bindgen-futures = { version = "0.4", optional = true }
web-sys = { version = "0.3", optional = true, features = [
    "Request", "RequestInit", "RequestMode", "Response", "Headers",
    "Window", "WorkerGlobalScope",
    "FileSystemDirectoryHandle", "FileSystemFileHandle",
    "FileSystemWritableFileStream",
    "StorageManager",
] }
js-sys = { version = "0.3", optional = true }
```

#### `crates/clawft-plugin/Cargo.toml`

Current:
```toml
[features]
default = []
voice = []

[dependencies]
async-trait = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio-util = { workspace = true }
chrono = { workspace = true }
tracing = { workspace = true }
semver = "1"
```

New:
```toml
[features]
default = ["native"]
native = ["dep:tokio-util"]
voice = []

[dependencies]
async-trait = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio-util = { workspace = true, optional = true }
chrono = { workspace = true }
tracing = { workspace = true }
semver = "1"
```

### Behavior Changes

- **`dirs` in clawft-types**: The `Config::workspace_path()` method (line 71-79 of `crates/clawft-types/src/config/mod.rs`) calls `dirs::home_dir()`. This must be gated behind `#[cfg(feature = "native")]`. A browser-compatible alternative accepts home dir as a parameter or returns a relative path.
- **Platform traits**: The `HttpClient`, `FileSystem`, `Platform` traits remain universally available. Only their native implementations (`NativeHttpClient`, `NativeFileSystem`, `NativePlatform`) are gated.
- **`config_loader::discover_config_path`**: Currently uses `path.exists()` (std sync filesystem check) at lines 38-39 and 42-43. This must be changed to accept a list of pre-checked paths or use the async `fs.exists()` in `load_config_raw` instead.
- **`async_trait` bounds**: The `Platform`, `HttpClient`, and `FileSystem` traits use `#[async_trait]` which implies `Send`. Browser WASM is single-threaded and some browser types are `!Send`. Must add conditional `?Send`.

---

## P -- Pseudocode

### Feature Flag Pattern for Trait Definitions

```rust
// crates/clawft-platform/src/lib.rs

pub mod config_loader;
pub mod env;
pub mod fs;
pub mod http;
pub mod process;

use async_trait::async_trait;

/// Platform trait -- always available.
/// Uses conditional Send bound: Send on native, !Send on browser.
#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
pub trait Platform: Send + Sync {
    fn http(&self) -> &dyn http::HttpClient;
    fn fs(&self) -> &dyn fs::FileSystem;
    fn env(&self) -> &dyn env::Environment;
    fn process(&self) -> Option<&dyn process::ProcessSpawner>;
}

// Native impl -- only when native feature is on
#[cfg(feature = "native")]
mod native_platform;
#[cfg(feature = "native")]
pub use native_platform::NativePlatform;

// In Phase BW4, browser impl will be added here:
// #[cfg(feature = "browser")]
// mod browser;
// #[cfg(feature = "browser")]
// pub use browser::BrowserPlatform;
```

### Conditional async_trait on Sub-Traits

```rust
// crates/clawft-platform/src/http.rs

use async_trait::async_trait;
use std::collections::HashMap;

// Trait definition -- always available
#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
pub trait HttpClient: Send + Sync {
    async fn request(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>>;

    // ... default methods unchanged
}

// Native impl -- gated
#[cfg(feature = "native")]
pub struct NativeHttpClient { /* ... */ }

#[cfg(feature = "native")]
#[async_trait]
impl HttpClient for NativeHttpClient { /* ... */ }
```

### Gating dirs in clawft-types

```rust
// crates/clawft-types/src/config/mod.rs

impl Config {
    /// Get the expanded workspace path.
    ///
    /// On native, expands `~/` to the user's home directory.
    /// On browser/WASM, returns the raw path (caller provides home dir).
    pub fn workspace_path(&self) -> PathBuf {
        let raw = &self.agents.defaults.workspace;
        if let Some(rest) = raw.strip_prefix("~/") {
            #[cfg(feature = "native")]
            if let Some(home) = dirs::home_dir() {
                return home.join(rest);
            }
        }
        PathBuf::from(raw)
    }

    /// Get workspace path with an explicit home directory.
    /// Used by browser/WASM where dirs::home_dir() is unavailable.
    pub fn workspace_path_with_home(&self, home: Option<&std::path::Path>) -> PathBuf {
        let raw = &self.agents.defaults.workspace;
        if let Some(rest) = raw.strip_prefix("~/") {
            if let Some(home) = home {
                return home.join(rest);
            }
        }
        PathBuf::from(raw)
    }
}
```

### Fixing config_loader path.exists() Leak

```rust
// crates/clawft-platform/src/config_loader.rs

/// Discover the config file path using the fallback chain.
///
/// The synchronous path.exists() calls are replaced with a
/// two-step approach:
/// 1. discover_candidates() returns candidate paths
/// 2. load_config_raw() checks existence via Platform fs trait
pub fn discover_config_candidates(
    env: &dyn super::env::Environment,
    home_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // Step 1: Check CLAWFT_CONFIG env var
    if let Some(env_path) = env.get_var("CLAWFT_CONFIG") {
        candidates.push(PathBuf::from(env_path));
        return candidates; // Env var takes absolute precedence
    }

    // Step 2 & 3: Home directory paths
    if let Some(home) = home_dir {
        candidates.push(home.join(".clawft").join("config.json"));
        candidates.push(home.join(".nanobot").join("config.json"));
    }

    candidates
}

/// Load config by checking candidates against the filesystem.
pub async fn load_config_raw(
    fs: &dyn super::fs::FileSystem,
    env: &dyn super::env::Environment,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let home = fs.home_dir();
    let candidates = discover_config_candidates(env, home);

    // Check each candidate via the platform fs trait (async, WASM-safe)
    let mut config_path = None;
    for candidate in &candidates {
        if fs.exists(candidate).await {
            config_path = Some(candidate.clone());
            break;
        }
    }

    let Some(path) = config_path else {
        tracing::info!("no config file found, using defaults");
        return Ok(Value::Object(serde_json::Map::new()));
    };

    // ... rest unchanged (read and parse)
}
```

### CancellationToken Abstraction in clawft-plugin

```rust
// crates/clawft-plugin/src/lib.rs (or a cancellation.rs module)

#[cfg(feature = "native")]
pub use tokio_util::sync::CancellationToken;

#[cfg(not(feature = "native"))]
mod cancellation {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Lightweight CancellationToken for non-tokio environments.
    #[derive(Clone)]
    pub struct CancellationToken {
        cancelled: Arc<AtomicBool>,
    }

    impl CancellationToken {
        pub fn new() -> Self {
            Self { cancelled: Arc::new(AtomicBool::new(false)) }
        }

        pub fn cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
        }

        pub fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }

        pub async fn cancelled(&self) {
            // In browser, poll in a loop with a yield
            loop {
                if self.is_cancelled() { return; }
                // Yield to the event loop
                #[cfg(target_arch = "wasm32")]
                {
                    // Use a short sleep/yield
                    gloo_timers::future::sleep(std::time::Duration::from_millis(10)).await;
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    std::future::pending::<()>().await;
                }
            }
        }
    }
}
#[cfg(not(feature = "native"))]
pub use cancellation::CancellationToken;
```

---

## A -- Architecture

### How Trait-Only Compilation Works

```
clawft-platform (with --no-default-features)
    |
    +-- src/lib.rs        -> Platform trait definition (no impls)
    +-- src/http.rs        -> HttpClient trait + HttpResponse struct
    +-- src/fs.rs          -> FileSystem trait
    +-- src/env.rs         -> Environment trait
    +-- src/process.rs     -> ProcessSpawner trait
    +-- src/config_loader.rs -> Pure Rust config loading logic
    |
    Dependencies: clawft-types, async-trait, thiserror, tracing, serde, serde_json
    (all WASM-safe)

clawft-platform (with --features native)  [default]
    |
    +-- All of the above, PLUS:
    +-- NativePlatform     -> Bundles NativeHttpClient + NativeFileSystem + etc.
    +-- NativeHttpClient   -> reqwest-based HTTP
    +-- NativeFileSystem   -> tokio::fs-based filesystem
    +-- NativeEnvironment  -> std::env-based env vars
    +-- NativeProcessSpawner -> tokio::process-based spawning
    |
    Additional deps: tokio, reqwest, dirs
```

### Crate Dependency with Features

After BW1, the dependency graph for `--no-default-features` looks like:

```
clawft-types (no features)
    |  only deps: serde, serde_json, chrono, thiserror, uuid
    v
clawft-platform (no features)
    |  only deps: clawft-types, async-trait, thiserror, tracing, serde, serde_json
    v
clawft-plugin (no features)
    |  only deps: async-trait, serde, serde_json, thiserror, chrono, tracing, semver
    v
clawft-security
    |  only deps: serde, serde_json, thiserror, tracing, regex, chrono, sha2
```

All of these are WASM-safe. No `tokio`, `reqwest`, `dirs`, `notify`, `tokio-util` in the dependency tree.

### Module Organization After BW1

```
crates/clawft-platform/src/
    lib.rs           -- Platform trait (always), NativePlatform (feature = "native")
    http.rs          -- HttpClient trait (always), NativeHttpClient (feature = "native")
    fs.rs            -- FileSystem trait (always), NativeFileSystem (feature = "native")
    env.rs           -- Environment trait (always), NativeEnvironment (feature = "native")
    process.rs       -- ProcessSpawner trait (always), NativeProcessSpawner (feature = "native")
    config_loader.rs -- Config discovery and loading (always, no std::path::Path::exists)
```

The native implementations stay in the same files but behind `#[cfg(feature = "native")]`. This minimizes the diff and keeps the code co-located with its trait definition.

---

## R -- Refinement

### Regression Risks

| ID | Risk | Specific Location | Prevention |
|---|---|---|---|
| N1 | `cargo build` breaks because `dirs` is now optional | `clawft-types/Cargo.toml` | `default = ["native"]` includes `dep:dirs`. Verify with bare `cargo build` |
| N2 | Test code references `NativePlatform` which is now gated | `crates/clawft-platform/src/lib.rs:118-143` (tests) | Tests use `#[cfg(test)]` which runs under default features; `native` is in default |
| N3 | Downstream crate `clawft-core` uses `use clawft_platform::NativePlatform` | `crates/clawft-core/src/agent/loop_core.rs:700` (test) | `NativePlatform` is re-exported when `native` is on; `clawft-core` depends on `clawft-platform` with default features |
| N4 | `Send` bound removed from traits on native | All trait definitions | `#[cfg_attr(not(feature = "browser"), async_trait)]` preserves `Send` on native. Only `browser` feature uses `?Send` |
| N5 | `clawft-plugin` tests use `CancellationToken` from `tokio-util` | Plugin test code | `default = ["native"]` ensures `tokio-util` is available in default build |
| N7 | Feature unification pulls `native` when `browser` is intended | Transitive deps | `clawft-types` does not have a `browser` feature in BW1; it just has `native` (default) and bare (no features). Crates enabling `browser` must use `default-features = false` for upstream crates |

### Edge Cases

1. **Config without home_dir**: Browser calls `Config::workspace_path()` without `dirs::home_dir()`. Result: returns `~/.clawft/workspace` as a literal path string. The browser caller should use `workspace_path_with_home(Some(Path::new("/")))` instead.

2. **config_loader with no filesystem**: In browser, `discover_config_path` may find no candidates (no home dir, no env var). This is correct -- browser config comes from JS `init()`, not filesystem discovery.

3. **Plugin crate consumers**: All `clawft-plugin-*` crates depend on `clawft-plugin`. They are not in the browser build path, so they always get `native` features via default. No changes needed.

### Testing Strategy

1. **Compilation checks** (automated):
   - `cargo check --target wasm32-unknown-unknown -p clawft-types --no-default-features`
   - `cargo check --target wasm32-unknown-unknown -p clawft-platform --no-default-features`
   - `cargo check --target wasm32-unknown-unknown -p clawft-plugin --no-default-features`

2. **Regression tests** (automated):
   - `cargo test --workspace` -- all 823+ tests pass
   - `cargo clippy --workspace` -- no new warnings

3. **Manual verification**:
   - Inspect `cargo tree -p clawft-platform --no-default-features` to confirm no tokio/reqwest/dirs in the tree
   - Inspect `cargo tree -p clawft-platform` (default) to confirm tokio/reqwest/dirs ARE present

---

## C -- Completion

### Exit Criteria

- [ ] `cargo check --target wasm32-unknown-unknown -p clawft-types --no-default-features` passes
- [ ] `cargo check --target wasm32-unknown-unknown -p clawft-platform --no-default-features` passes
- [ ] `cargo check --target wasm32-unknown-unknown -p clawft-plugin --no-default-features` passes
- [ ] `cargo test --workspace` -- all tests pass (zero regressions)
- [ ] `cargo clippy --workspace` -- no new warnings
- [ ] `cargo build --release --bin weft` -- native CLI builds
- [ ] `cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm` -- WASI build works
- [ ] `cargo tree -p clawft-platform --no-default-features` shows no tokio/reqwest/dirs

### Test Commands

```bash
# WASM compilation checks
rustup target add wasm32-unknown-unknown
cargo check --target wasm32-unknown-unknown -p clawft-types --no-default-features
cargo check --target wasm32-unknown-unknown -p clawft-platform --no-default-features
cargo check --target wasm32-unknown-unknown -p clawft-plugin --no-default-features

# Native regression
cargo test --workspace
cargo clippy --workspace
cargo build --release --bin weft

# WASI regression
cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm

# Dependency tree verification
cargo tree -p clawft-platform --no-default-features
cargo tree -p clawft-types --no-default-features
```

### CI Additions

Add to `.github/workflows/pr-gates.yml`:

```yaml
wasm-browser-check:
  name: Browser WASM compilation check
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
      with:
        targets: wasm32-unknown-unknown
    - run: cargo check --target wasm32-unknown-unknown -p clawft-types --no-default-features
    - run: cargo check --target wasm32-unknown-unknown -p clawft-platform --no-default-features
    - run: cargo check --target wasm32-unknown-unknown -p clawft-plugin --no-default-features
    - run: cargo check --workspace  # Sanity: native still works
```

### Documentation Deliverables

1. **ADR-027**: `docs/architecture/adr-027-browser-wasm-support.md`
   - Decision: Hybrid approach (real engine in WASM, exclude server-side)
   - Alternatives rejected: full port, thin client
   - Trade-offs: feature flag complexity vs. single codebase
   - Feature flag strategy rationale

2. **Feature flag guide**: `docs/development/feature-flags.md`
   - How `native`/`browser` features work
   - Rules: every optional dep must appear in `default = ["native"]`
   - How to check both targets
   - Mutual exclusivity of `native`/`browser`
   - Adding new dependencies: decision tree

### Feature Validation: `scripts/build.sh gate` (SUPERSEDES `scripts/check-features.sh`)

WEFT-409 (2026-04-30): The original plan called for a standalone
`scripts/check-features.sh`. It was never created. The canonical
phase-gate entrypoint is now `scripts/build.sh gate`, which runs the
12-check suite (native + WASI + browser + clippy + bundle-size + audit
+ docs regen check). Use it whenever this doc says "feature flag
validation script".

```bash
# Canonical phase gate (replaces scripts/check-features.sh)
scripts/build.sh gate

# Or, for a fast iteration loop, just the cargo check across targets:
scripts/build.sh check        # native cargo check --workspace
scripts/build.sh wasi         # wasm32-wasip2 build
scripts/build.sh browser      # wasm32-unknown-unknown build
```

The exact targets covered by `scripts/build.sh gate` are documented
inline in `scripts/build.sh` (search for `phase_gate()`), and any new
target should be added there rather than to a parallel script.
