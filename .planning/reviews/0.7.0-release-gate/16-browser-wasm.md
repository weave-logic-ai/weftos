---
title: "Browser WASM Runtime (W-BROWSER)"
slug: browser-wasm
workstream_id: "16"
status: "shipped — pipeline NOT wired (compile-only + LLM bypass)"
last_updated: 2026-04-28
auditor: Opus 4.7 (1M)
release_target: "0.7.0 (audit-only, not gate-blocking)"
related_workstreams:
  - "15 — UI / WASM browser mode (S3.6 wasm-adapter)"
  - "12 — WASI runtime / wasip1 / wasip2 split"
sources:
  - .planning/wasm-browser/{00..06}-*.md
  - .planning/sparc/browser/{00..06}-*.md
  - .planning/development_notes/step{1..6}-bw{1..6}*.md
  - .planning/development_notes/step{2..6}-phase-gate.md
  - .planning/development_notes/sprint-16/browser-wasm-features.md
  - .planning/development_notes/orchestrator-log.md
  - crates/clawft-wasm/, crates/clawft-platform/src/browser/
  - .github/workflows/wasm-browser.yml, scripts/build.sh
  - docs/adr/adr-044-wasm-wasip1-target.md
  - CHANGELOG.md (0.6.19)
---

# Browser WASM Runtime (W-BROWSER)

## General Description

W-BROWSER is the workstream that makes clawft's agent core compile and run inside a
browser tab as `wasm32-unknown-unknown` with `--features browser`, mutually exclusive
with the `native` feature stack (tokio + reqwest[rustls-tls] + dirs + notify + tokio-util).
The intended end-state per the consensus plan (`.planning/wasm-browser/00-consensus-plan.md`)
is `AgentLoop<BrowserPlatform>` running in the page, with:

- `BrowserHttpClient` — `web-sys` fetch API (Window/WorkerGlobalScope fallback).
- `BrowserFileSystem` — OPFS-backed (Origin Private File System) for `.clawft/` workspace.
- `BrowserEnvironment` — in-memory `Mutex<HashMap>` for env vars (no OS env in WASM).
- `process()` returning `None` (no subprocess in WASM).
- LLM transport via `BrowserLlmClient` (reqwest auto-detects wasm32 + Fetch) with CORS
  proxy support and Anthropic `anthropic-dangerous-direct-browser-access` direct mode.

The crate stack split as `clawft-{types, platform, plugin, security, core, llm, tools, wasm}`
all compile for `wasm32-unknown-unknown` under `--no-default-features --features browser`,
while `clawft-{channels, services, cli, plugin-*}` are explicitly excluded.

The audit's bottom line: **the crate stack DOES compile for wasm32-unknown-unknown
through BW1–BW5 and a test harness exists in `crates/clawft-wasm/www/` (BW6)**, but the
"real `AgentLoop<BrowserPlatform>`" goal stated in the consensus plan is **NOT met**.
What ships is:

1. A compile-only WASM build (CI gate `wasm-browser.yml` + per-crate `cargo check` matrix
   in `scripts/build.sh gate`).
2. A `wasm-bindgen` shim (`browser_entry::init/send_message/set_env`) that bypasses the
   pipeline entirely and calls `BrowserLlmClient::complete` directly with conversation
   history maintained in a `OnceLock<BrowserRuntime>`.
3. Two side features added in sprint 16 (`analyze_files`, `boot_info`) for the
   weftos.weavelogic.ai playground — not actually agent-loop work.

## Status & Timeline

| Phase | Title                              | Phase Gate Doc                    | Code Landed | AgentLoop Wired |
|-------|------------------------------------|-----------------------------------|-------------|-----------------|
| BW1   | Foundation (types + platform + plugin) | `step1-bw1-foundation.md` (no `step1-phase-gate.md` — note 1) | YES | n/a |
| BW2   | Core Engine (`clawft-core`)        | `step2-phase-gate.md` (7/7 PASS)  | YES | n/a |
| BW3   | LLM Transport (`browser_transport`) | `step3-phase-gate.md` (8/8 PASS) | YES | n/a |
| BW4   | BrowserPlatform                    | `step4-phase-gate.md` (9/9 PASS)  | YES (in-memory FS, not OPFS) | n/a |
| BW5   | WASM Entry + Tools                 | `step5-phase-gate.md` (11/11 PASS) | YES (shim only) | NO |
| BW6   | Integration Testing                | `step6-phase-gate.md` (11/11 PASS) | YES (HTML harness + 5 docs) | NO |

Note 1: A `step1-phase-gate.md` is referenced indirectly in `orchestrator-log.md` but the
file itself is missing from `.planning/development_notes/`. Phase-gate verification
starts at step 2.

CHANGELOG 0.6.19 (2026-04-22) records: *"Browser WASM: feature-gated crate stack compiles
for wasm32-unknown-unknown"* — exactly the compile-only state, with no claim of a wired
agent loop. The current branch is `development-0.7.0`; integration into 0.7.0 has been
the per-crate `gate` matrix and the `wasm-browser.yml` artifact build.

The pre-built artifacts at `crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm` are
~840 KB raw (Apr 20 2026), well over the planning target of <300 KB raw / <120 KB gzip
(see "What's Left" below). No gzip number is recorded in CI summaries.

## Released Features

The following are present in tree on `development-0.7.0` and exercised by CI:

- **Feature-flag split**: `native` ⊻ `browser` enforced across `clawft-types`,
  `clawft-platform`, `clawft-core`, `clawft-llm`, `clawft-tools`, `clawft-plugin`,
  `clawft-wasm`. Workspace deps default to `default-features = false` so consumers
  pick a flavor explicitly.
- **`clawft-platform/src/browser/`** — `BrowserPlatform`, `BrowserHttpClient`
  (web-sys fetch with Worker/Window fallback + JS-iter header decoding),
  `BrowserFileSystem` (in-memory `Mutex<HashMap<PathBuf, String>>`),
  `BrowserEnvironment` (in-memory `Mutex<HashMap>`). `Platform` impl uses
  `#[async_trait(?Send)]`.
- **`clawft-llm::browser_transport`** — `BrowserLlmClient` with `complete()` and
  `complete_stream_callback(FnMut(StreamChunk) -> bool)`, CORS proxy URL resolution
  (`{proxy}/{target}` pattern), Anthropic direct-browser header injection,
  HTTP-status → `ProviderError` classifier, 28 unit tests.
- **`clawft-core/src/runtime.rs`** — platform abstraction module: `now_millis()`
  (native `SystemTime` / browser `js_sys::Date::now()`), `Mutex` (tokio /
  futures-util), `RwLock` (tokio / `std::sync::RwLock` thin async wrapper), used to
  scrub `tokio::time::Instant` and `SystemTime::now()` from non-gated code paths.
- **Dual-impl `MessageBus`** — `bus.rs` rewritten with native (`tokio::sync::mpsc`
  bounded) and browser (`futures_channel::mpsc` unbounded) implementations sharing
  one public API.
- **`CancellationToken` polyfill** — in `clawft-plugin` for browser builds
  (`Arc<AtomicBool>` shim) when `tokio-util` is gated out.
- **wasm-bindgen entry shim** (`crates/clawft-wasm/src/lib.rs::browser_entry`) —
  exports `init(config_json) -> Promise<()>`, `send_message(text) -> Promise<String>`,
  `set_env(key, value)`, `boot_info() -> String`, `analyze_files(files_json) -> String`.
  Provider routing: prefix-match against builtins, OpenRouter fallback for vendor-prefixed
  models like `arcee-ai/...`.
- **CORS proxy + browser-direct config** — `ProviderConfig` carries `cors_proxy:
  Option<String>` and `browser_direct: bool` with `#[serde(default)]` so existing
  `.clawft/config.json` files parse unchanged.
- **HTML test harness** — `crates/clawft-wasm/www/{index.html, main.js}` plus a
  pre-built `www/pkg/clawft_wasm_bg.wasm` (859,494 bytes) + `clawft_wasm.js`
  (35,364 bytes) checked into the tree.
- **CI**:
  - `.github/workflows/wasm-browser.yml` — builds `wasm32-unknown-unknown` with
    `wasm-bindgen-cli 0.2.108`, uploads `browser-pkg/` artifact, attaches tarball
    to GitHub releases on tag.
  - `.github/workflows/pr-gates.yml` — has a `wasm-browser-check` step at line 197.
  - `scripts/build.sh gate` — gates 6 browser crates (`clawft-{types, platform, core,
    llm, tools, wasm}`) under `--target wasm32-unknown-unknown --features browser`.
  - `scripts/build.sh browser` and `scripts/build.sh serve` (port 8080) for local
    iteration on the harness.
- **Browser docs** — `docs/browser/{building, quickstart, api-reference, architecture,
  deployment}.md` (BW6, 5 files).
- **Sprint-16 side features**:
  - `boot_info()` — JSON array of mock kernel boot phases (INIT/CONFIG/SERVICES/
    NETWORK/READY) consumed by the docs-site ExoChain log component.
  - `analyze_files(files_json)` — in-WASM static analyzer that mirrors native
    ComplexityAnalyzer / SecurityAnalyzer / DependencyAnalyzer / TopologyAnalyzer
    on in-memory file blobs. Used by the GitHub repo-assessment UI in
    `docs/src/app/clawft/WasmSandbox.tsx`.
- **UI integration** (cross-ref to W-UI/15) — `BackendAdapter` / `WasmAdapter` /
  `wasm-loader.ts` / `feature-detect.ts` / `mode-context.tsx` consume the wasm-bindgen
  exports above. AES-256-GCM encrypted API keys live in IndexedDB; this is UI-side and
  outside the W-BROWSER crate scope.

## What's Left — Total Depth

This section captures **every** unfinished, deferred, or orphaned item across the
workstream regardless of release-scope. The leading verdict is that the workstream is
roughly two-thirds done: compile + transport + harness shipped; the actual goal
(`AgentLoop<BrowserPlatform>` running through the 6-stage pipeline in the page) is
not.

### TODOs / FIXMEs in code

- `crates/clawft-wasm/src/lib.rs:78-99` — both stub functions still carry their phase
  comments:
  - `pub fn init() -> i32 { /* Phase 3A Week 11: Will load config from WASI filesystem */ 0 }`
  - `pub fn process_message(input: &str) -> String { /* Phase 3A Week 12: Will run the
    full 6-stage pipeline. */ format!("…(pipeline not yet wired)") }`
  - These are the **non-browser** entry points (used by the wasip2 build) and remain
    advisory placeholders, not the production browser entry.
- `crates/clawft-wasm/src/lib.rs:353-355` — `pub fn set_env(_key, _value)` is a no-op
  with the comment *"Browser env vars are managed by `BrowserPlatform.env()`"* but the
  `BrowserRuntime` in `OnceLock` only stores `config + client + model_name + messages`
  and exposes no path to mutate the live `BrowserEnvironment`. Tracked in
  `step6-bw6-integration-testing.md` "What Remains: set_env wiring".
- `crates/clawft-platform/src/browser/fs.rs:3-9` — file-level doc explicitly calls
  out *"acceptable for the current stub/MVP phase"* for OPFS deferral.
- `crates/clawft-platform/src/browser/mod.rs:7` — header still says
  *"In-memory filesystem (OPFS planned for future)"*.
- `crates/clawft-tools/src/file_tools.rs:19,28,58` — comment chain documenting that
  `resolve_sandbox_path` / `path_exists` rely on OPFS having no symlinks; if/when an
  OPFS backend lands, these heuristics need a re-audit.
- The two `#[cfg(feature = "wasm-plugins")]` modules (`sandbox`, `engine`,
  `permission_store`, `audit` totalling ~28 KLOC) are wasmtime-host code that runs on
  *native* / wasip2 — not the browser. They sit in `clawft-wasm` because of historical
  crate naming and are completely orthogonal to W-BROWSER. Worth splitting in a future
  housekeeping pass to avoid confusion when reading the crate.

### Deferred items (called out as "What Remains" in step docs)

From `step5-bw5-wasm-entry.md` "What Remains for BW6":

1. **Wire AgentLoop** — connect `init()` to a real `AgentLoop<BrowserPlatform>` once
   all internal types are plumbed. **Not done.** The current `send_message` calls
   `BrowserLlmClient::complete` directly; classifier, tiered router, context assembler,
   quality scorer, verification, tool registry, skills are all unreached in the
   browser path.
2. **ListDirectoryTool metadata** — browser stub returns `(false, 0u64)` for
   `is_dir`/`size`; revisit when OPFS metadata API lands.
3. **Binary size audit** — measure actual `wasm32` binary with `wasm-opt -Oz` and
   verify <300 KB raw / <120 KB gzip planning budget. The committed
   `www/pkg/clawft_wasm_bg.wasm` is **859 KB raw**, ~2.9× over budget. No `wasm-opt`
   pipeline is wired into `wasm-browser.yml`.
4. **wasm-bindgen-test** — run the wasm-bindgen-test suite in headless browser. **No
   `wasm-bindgen-test`** dependency or harness exists anywhere in the workspace
   (`grep wasm_bindgen_test crates/` returns zero matches). The pre-built `pkg/` is
   the only browser-target verification artifact.

From `step6-bw6-integration-testing.md` "What Remains":

1. **Wire AgentLoop** — duplicate of the BW5 item, still open.
2. **OPFS persistence** — replace the in-memory `BrowserFileSystem` with a real OPFS
   implementation. Blockers documented in `step4-bw4-browser-platform.md §3.2`:
   `web-sys` `FileSystemDirectoryHandle` / `FileSystemFileHandle` bindings are behind
   unstable feature flags. **No plan currently lists which web-sys version unblocks
   this.**
3. **wasm-bindgen-test suite** — duplicate.
4. **Binary size audit** — duplicate.
5. **`set_env` wiring** — make the BrowserPlatform reachable from the wasm-bindgen
   bridge so JS-driven env mutation actually mutates state.

From `.planning/wasm-browser/05-task-breakdown.md` Phase 6 (BW6) — items NOT done:

| ID | Task | Status |
|----|------|--------|
| P6.2 | E2E pipeline test (classify → route → LLM → tool → response) | **Open** — pipeline not wired so this is impossible to write |
| P6.3 | OPFS file ops test (write/read/list/delete + persistence across reload) | **Open** — depends on OPFS impl |
| P6.4 | Config persistence test (init → reload → verify config survives) | **Open** — config is held only in `OnceLock`, lost on reload |
| P6.5 | Performance profiling (load/init/first-msg latency, memory) | **Open** — no metrics captured anywhere |
| P6.6 | Web Worker variant (stretch) | **Open** — `BrowserHttpClient` already supports `WorkerGlobalScope::fetch`, but no worker.js / harness exists |
| P6.7 | Final regression suite + docker smoke + ≤10 % test-duration regression | **Partial** — the per-crate `gate` matrix runs every PR but no timing comparison |
| P6.10 | Update `README.md` and `CLAUDE.md` to point at `docs/browser/` | **Open** — neither root file mentions the browser stack |

From `.planning/wasm-browser/05-task-breakdown.md` Phase 1 (BW1) — items NOT done:

- **P1.5** — there is a `wasm-browser-check` job in `pr-gates.yml`, but the
  **`wasm-browser-size` job with the 500 KB gzip budget never landed**. Combined with
  the 859 KB raw artifact this is a real gap.
- **P1.6** — `scripts/check-features.sh` does **not exist** under `scripts/`; the
  current entrypoint is `scripts/build.sh gate`, which covers the same checks but
  the contract documented in step1-bw1 references the missing script name.
- **P1.8** — **ADR-027 "Browser WASM Support" was never written.** The slot at
  `docs/adr/adr-027-*.md` is occupied by `adr-027-selective-libp2p.md`, an unrelated
  topic. There is no ADR for the entire W-BROWSER decision tree (hybrid vs full port
  vs thin client, feature-flag mutex, OPFS deferral). `docs/architecture/wasm-browser-portability-analysis.md`
  exists but is an analysis, not an ADR.
- **P1.9** — `docs/development/feature-flags.md` (rules for adding new deps so they
  don't break the WASM target) was never written.

From `.planning/wasm-browser/05-task-breakdown.md` Phase 3 (BW3) — items NOT done:

- **P3.6** — `docs/browser/cors-provider-setup.md` (per-provider CORS recipes) was
  never created. `docs/browser/deployment.md` has a small CORS section but not the
  per-provider matrix the spec called for.
- **P3.7** — `docs/browser/config-schema.md` (full annotated `config.json` schema for
  browser mode) was never created.
- **Streaming via `ReadableStream`** — `BrowserLlmClient::complete_stream_callback`
  exists but uses reqwest's stream surface; an SSE parser using `web-sys`
  `ReadableStream` was the planned fallback. The `browser_delay()` utility is a
  no-op `yield` (note in the source: *"upgrade to `gloo-timers`"*), so any
  retry/backoff path inside the browser transport currently does **not** actually
  yield to the event loop.

### Open questions

1. **OPFS unlock condition** — which `web-sys` minor version (or `gloo-fs` /
   `wasm-fs` shim) graduates `FileSystemFileHandle` out of unstable, and what's the
   migration path for the in-memory data already held by deployed pages?
2. **Where does the wasm-plugin host (`wasmtime`-based `engine.rs`/`sandbox.rs`/
   `audit.rs`) live long-term?** It currently sits in `clawft-wasm` but is native-only
   code. Splitting to a dedicated `clawft-wasm-host` crate would keep `clawft-wasm`
   purely the browser-target entry and trim the public-API surface.
3. **Streaming path for browser** — keep callback-based `complete_stream_callback`
   or migrate to `wasm-streams` / `ReadableStream`? Affects SSE backpressure and
   CORS-proxy compatibility.
4. **Tool sandbox boundary** — when AgentLoop is wired, should `Spawn`, `ShellExec`,
   `Message`, `Delegate` be silently absent, return `Err("not available in browser")`,
   or surface as JSON-RPC stubs that route to a back-end? Currently they're cfg'd out
   so any reachability would be a compile error, not a runtime decision.
5. **Web Worker dispatch model** — main-thread WASM blocks UI on long LLM calls;
   `BrowserHttpClient` is already worker-ready but the harness never instantiates it
   in a worker. Decision and a `worker.js` template owe.
6. **Persistent conversation history** — `OnceLock<BrowserRuntime>` puts the
   `Mutex<Vec<ChatMessage>>` in module-scope memory; on reload it vanishes. Plan
   spoke of `CLAUDE.md`-per-group in OPFS (mirroring openbrowserclaw); not designed.
7. **Provider-routing fallback ordering** — the hard-coded fallback chain
   `["openrouter", "openai", "anthropic", "groq", "deepseek", "gemini", "xai"]` in
   `resolve_provider` is suitable for a demo but probably should be data-driven from
   the config (or at minimum follow a documented preference order).
8. **Versioning / signing of the browser bundle** — the release artifact attached to
   tags via `wasm-browser.yml` is unsigned. Signing parity with the WASI release flow
   (`release-wasi.yml`) is not in scope of any current step doc.

### Orphaned work / loose ends

- **Pre-built `www/pkg/`** is checked in (Apr 20 commit). Either it should be
  `.gitignore`d and rebuilt by the harness boot script, or there should be a stale-
  artifact check in CI. Currently it can drift silently from the `wasm-browser.yml`
  output.
- **Sprint-16 side features (`analyze_files`, `boot_info`)** are wired into the
  weftos.weavelogic.ai docs-site (`WasmSandbox.tsx`) but they have no inverse on the
  primary `crates/clawft-wasm/www/` harness, and they're invisible to anyone who
  expects this crate to be only "browser AgentLoop entry". The crate's module
  layering (`browser_entry::*` re-exported from `lib.rs` via `pub use`) should be
  documented or split.
- **Pre-existing warning** at `crates/clawft-core/src/workspace/agent.rs:257`
  (unreachable expression on non-Unix targets) appears in every browser phase-gate
  doc from BW2 onward — never fixed, just noted as "non-blocking".
- **`wasm32-wasi` → `wasm32-wasip1` rename** — `step2-phase-gate.md` flagged that
  CI/scripts/docs need to follow the Rust 1.93 rename. ADR-044 documents the wasip1
  default with intent to migrate to wasip2 in "Sprint 12" — that migration was never
  executed; both `scripts/build.sh wasi` and `release-wasi.yml` already say
  `wasm32-wasip2`, so the doc is stale rather than the code, but the inconsistency
  remains.
- **Browser `dirs::home_dir()` fallback** — `clawft-core/src/agent/skill_autogen.rs`
  uses `.clawft/skills` as a relative path on browser. With an in-memory FS this is
  fine; with real OPFS it implicitly anchors at the OPFS root (`/clawft`) and
  collides with the virtual home returned by `BrowserFileSystem::home_dir()`. Worth
  formalizing once OPFS lands.
- **`BrowserPlatform::with_env`** is the only constructor that takes pre-populated
  env vars; `init()` doesn't expose a way for JS callers to seed it. So
  `BrowserEnvironment` is, in practice, dead code for the production entry — only
  `BrowserHttpClient` and `BrowserFileSystem` are reachable from JS.

### Cross-cutting risks

- **Mutual-exclusion enforcement is by convention**: there's no `compile_error!` if
  someone enables both `native` and `browser` features. The crate stack relies on
  consumers to set `default-features = false` and pick one — easy to break.
- **`async_trait` Send-bound tax**: every trait crossed by browser code carries the
  `cfg_attr(?Send)` pattern. Adding a new platform trait method without that pattern
  will silently break the browser build at the next `cargo check`.
- **The `BrowserLlmClient` keeps `api_key: SecretString`** in JS-readable memory once
  injected via `init(config_json)`. Any XSS in the host page lifts the key. The UI
  side at least encrypts in IndexedDB; the WASM side does not. Worth an explicit
  threat-model note.

## Task List

### P0 — required to make "AgentLoop runs in browser" a real claim

1. [Open] **Wire `AgentLoop<BrowserPlatform>` through `browser_entry::send_message`**
   replacing the direct `BrowserLlmClient::complete` call. Requires the full pipeline
   trait surface (classifier + router + context + transport + scorer + verification +
   tool registry) compiled under `--features browser`. Add a regression test that
   exercises a no-op classifier path end-to-end.
2. [Open] **Implement OPFS-backed `BrowserFileSystem`** with the same `FileSystem`
   trait surface, gated behind a new sub-feature (e.g. `browser-opfs`) so the
   in-memory variant remains as a fallback. Add the `web-sys` features for
   `FileSystemDirectoryHandle` and `FileSystemFileHandle` and verify they're stable
   enough on current `web-sys = "0.3"`.
3. [Open] **Wire `set_env` to `BrowserEnvironment`**. Store the `BrowserPlatform`
   (or a handle to its env) in `OnceLock<BrowserRuntime>` and have `set_env` mutate
   through it.
4. [Open] **Add `wasm-bindgen-test` harness** — `crates/clawft-wasm/tests/browser_*.rs`
   covering: `BrowserHttpClient` round-trip, `BrowserFileSystem` write/read/list/
   delete, `init()` + `send_message()` happy-path against a local fake LLM endpoint.
   Run via `wasm-pack test --headless` in CI.
5. [Open] **Binary size CI gate** — wire `wasm-opt -Oz` into `wasm-browser.yml`,
   capture raw + gzip sizes in the step summary, fail PRs that bust the
   `<300 KB raw / <120 KB gzip` budget (or relax the budget in writing).

### P1 — process / docs hygiene

6. [Open] **Write ADR-027 "Browser WASM Support"** (the planned-but-missing ADR).
   Capture: hybrid vs full-port vs thin-client decision, `native ⊻ browser`
   feature-mutex, OPFS deferral, async_trait `?Send` tax, CORS-proxy convention.
7. [Open] **Write `docs/development/feature-flags.md`** (BW1 P1.9) covering the
   workspace-level `default-features = false` discipline + how to add new deps
   without breaking the WASM target.
8. [Open] **Write `docs/browser/cors-provider-setup.md` and `docs/browser/config-schema.md`**
   (BW3 P3.6/P3.7).
9. [Open] **Update root `README.md` and `CLAUDE.md`** with browser-build instructions
   and links into `docs/browser/`.
10. [Open] **Decide and document handling of mutually-exclusive features** —
    add a `compile_error!` block when both `native` and `browser` are enabled to
    fail loud.

### P2 — cleanups + nice-to-haves

11. [Open] **Split `clawft-wasm`** so `wasm-plugins` host code (wasmtime,
    `sandbox.rs`, `engine.rs`, `audit.rs`, `permission_store.rs`) moves to a
    dedicated `clawft-wasm-host` (or merges into `clawft-services`), leaving
    `clawft-wasm` as the pure browser/wasip2 entry.
12. [Open] **Replace `OnceLock<BrowserRuntime>` with persistence** — define a CLAUDE.md-per-group
    layout in OPFS; serialize conversation history on `send_message` exit.
13. [Open] **Web Worker harness** — ship a `worker.js` variant of the harness,
    document the WorkerGlobalScope fetch path that already works in
    `BrowserHttpClient`, address main-thread blocking.
14. [Open] **Migrate `browser_delay()`** from no-op `yield` to `gloo-timers`
    (see source comment) so retry/backoff in the browser transport actually waits.
15. [Open] **`.gitignore` `crates/clawft-wasm/www/pkg/`** OR replace it with a
    `.cache-meta` checked-in stub that points at the CI artifact, to eliminate
    drift.
16. [Open] **Remove the long-standing `unreachable_code` warning** in
    `crates/clawft-core/src/workspace/agent.rs:257` — flagged in BW2 through BW6
    phase-gate docs and never fixed.
17. [Open] **Audit ADR-044 vs reality** — script + release workflow already use
    `wasm32-wasip2`; ADR-044 still presents wasip1 as the shipping target. Either
    update the ADR to ratify the migration or revert the scripts.
18. [Open] **Provider-routing data-driven fallback** — move the hard-coded
    `fallback_order` slice in `resolve_provider` into the `Config` so users can
    influence it without recompiling.

## Sources

Primary planning material:

- `/home/aepod/dev/clawft/.planning/wasm-browser/00-consensus-plan.md` — three-agent
  consensus, hybrid approach.
- `/home/aepod/dev/clawft/.planning/wasm-browser/02-dependency-audit.md` — five
  root-cause deps + three abstraction leaks.
- `/home/aepod/dev/clawft/.planning/wasm-browser/03-feature-flag-spec.md`,
  `04-browser-platform-spec.md`, `05-task-breakdown.md`, `06-architecture-analysis.md`.
- `/home/aepod/dev/clawft/.planning/sparc/browser/{00-orchestrator, 01..06}-phase-BW{1..6}-*.md`.

Step / phase-gate notes:

- `/home/aepod/dev/clawft/.planning/development_notes/step{1..6}-bw{1..6}*.md`.
- `/home/aepod/dev/clawft/.planning/development_notes/step{2..6}-phase-gate.md`
  (note: `step1-phase-gate.md` is missing from the tree).
- `/home/aepod/dev/clawft/.planning/development_notes/sprint-16/browser-wasm-features.md`.
- `/home/aepod/dev/clawft/.planning/development_notes/orchestrator-log.md`.

Code:

- `/home/aepod/dev/clawft/crates/clawft-wasm/Cargo.toml`
- `/home/aepod/dev/clawft/crates/clawft-wasm/src/lib.rs` (browser_entry module)
- `/home/aepod/dev/clawft/crates/clawft-wasm/src/platform.rs` (WASI bundle, separate from BrowserPlatform)
- `/home/aepod/dev/clawft/crates/clawft-wasm/www/{index.html, main.js, pkg/*}`
- `/home/aepod/dev/clawft/crates/clawft-platform/src/browser/{mod, http, fs, env}.rs`
- `/home/aepod/dev/clawft/crates/clawft-llm/src/browser_transport.rs`
- `/home/aepod/dev/clawft/crates/clawft-core/src/runtime.rs`
- `/home/aepod/dev/clawft/crates/clawft-core/src/bus.rs`
- `/home/aepod/dev/clawft/crates/clawft-tools/src/file_tools.rs`
  (`resolve_sandbox_path`, `path_exists`)

Build + CI:

- `/home/aepod/dev/clawft/scripts/build.sh` (`cmd_browser`, `cmd_wasi`, `cmd_gate`)
- `/home/aepod/dev/clawft/.github/workflows/wasm-browser.yml`
- `/home/aepod/dev/clawft/.github/workflows/pr-gates.yml` (line 197 wasm-browser-check)
- `/home/aepod/dev/clawft/.github/workflows/wasm-build.yml`,
  `.github/workflows/release-wasi.yml`,
  `.github/workflows/docs-assets.yml`

ADRs / architecture:

- `/home/aepod/dev/clawft/docs/adr/adr-044-wasm-wasip1-target.md`
- `/home/aepod/dev/clawft/docs/architecture/wasm-browser-portability-analysis.md`
- `/home/aepod/dev/clawft/docs/browser/{building, quickstart, api-reference,
  architecture, deployment}.md`
- `/home/aepod/dev/clawft/CHANGELOG.md` (entry 0.6.19, 2026-04-22)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws16-browser-wasm` label.

- **Range**: WEFT-388 … WEFT-409 (22 items)
- **Per cycle**: 0.7.x: 2, 0.8.x: 20
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
