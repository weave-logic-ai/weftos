---
title: "Browser pipeline wire-through (W-BROWSER P0.1)"
slug: browser-pipeline-wire-plan
workstream_id: "16b"
status: partial
completion_pct: 80
last_updated: 2026-04-28
related: 16-browser-wasm.md
release_target: "0.7.0 (audit-only, follow-on hardening)"
---

# Browser pipeline wire-through

## Scope

Replace the LLM-only bypass in `crates/clawft-wasm/src/lib.rs::browser_entry::send_message`
with a full `AgentLoop<BrowserPlatform>::handle_turn` call routed through the
6-stage pipeline (classifier → router → assembler → transport → scorer → learner).
This addresses task **P0.1** of `.planning/reviews/0.7.0-release-gate/16-browser-wasm.md`.

## What landed

| Component | Status | File |
|-----------|--------|------|
| `BrowserPlatform` instantiated in `init()` | DONE | `crates/clawft-wasm/src/lib.rs` |
| `AppContext<BrowserPlatform>` built (bus, sessions, memory, skills, context, tools) | DONE | bootstrap.rs |
| Tool registry populated via `clawft_tools::register_all` | DONE | wasm entry |
| Stage 1 — `KeywordClassifier` | WIRED | pipeline default |
| Stage 2 — `StaticRouter` (or `TieredRouter` per config) | WIRED | pipeline default |
| Stage 3 — `TokenBudgetAssembler` | WIRED | pipeline default |
| Stage 4 — Transport via `BrowserLlmAdapter` (new) → `BrowserLlmClient` | WIRED | `crates/clawft-core/src/pipeline/browser_llm_adapter.rs` |
| Stage 5 — `NoopScorer` / `FitnessScorer` | WIRED | pipeline default |
| Stage 6 — `NoopLearner` / `TrajectoryLearner` | WIRED | pipeline default |
| `ContextBuilder` (memory + skills + history) | WIRED | inherited via `AppContext` |
| `SessionManager` (in-memory FS) | WIRED | inherited via `AppContext` |
| `ConversationSink` (`InMemorySink`) | WIRED | `AgentLoop` default |
| `EffectGate` (`NoopGate`) | WIRED | `AgentLoop` default |
| `ContextRouter` (`NullRouter`) | WIRED | `AgentLoop` default |
| `AgentLoop::handle_turn` dispatch from `send_message` | WIRED | wasm entry |

All six pipeline stages now run on every browser turn. `send_message` builds an
`InboundMessage { channel: "web", chat_id: "browser" }` and dispatches it through
`AgentLoop::handle_turn`, returning `OutboundMessage.content` to JS.

## Supporting changes

The native↔browser feature mutex left `clawft-core` failing to compile under
`--features browser` because three callers of `tokio` and `crate::agent_bus`
were not feature-gated:

* `crates/clawft-core/src/agent/identity.rs` — `tokio::sync::RwLock` import →
  switched to `crate::runtime::RwLock` polyfill that fans out to `tokio` on
  native and `std::sync::RwLock`-wrapper on browser (already provided in
  `runtime.rs` since BW2; never adopted by `identity.rs`).
* `crates/clawft-core/src/bootstrap.rs` — three `set_agent_bus` /
  `agent_bus()` / `agent_bus: Option<...>` references gated behind
  `#[cfg(feature = "native")]` to match the existing `pub mod agent_bus`
  gating in `lib.rs:27`.

The pipeline traits also required surgery to admit a `!Send` browser
transport:

* `pipeline::traits::LlmTransport` — wrapped `#[async_trait]` in
  `#[cfg_attr(not(feature = "browser"), async_trait)]` /
  `#[cfg_attr(feature = "browser", async_trait(?Send))]`. Same change to
  `pipeline::transport::LlmProvider` and the
  `OpenAiCompatTransport` impl block.
* `PipelineRegistry::complete_stream` and the `LlmTransport::complete_stream`
  default impl gated `#[cfg(not(feature = "browser"))]` because
  `StreamCallback = Box<dyn FnMut(&str) -> bool + Send>` is incompatible
  with the browser's single-threaded model. A browser streaming entry will
  land alongside an SSE-via-`ReadableStream` parser (W-BROWSER §"What's
  Left" — *Streaming via `ReadableStream`*).

## Send/Sync soundness

`BrowserLlmAdapter` carries `unsafe impl Send + Sync`, justified because
`wasm32-unknown-unknown` is single-threaded by construction and no value
ever traverses a thread boundary. The `?Send` `async_trait` relaxation
above means the future returned by `complete()` is no longer required to
be `Send`, so the underlying `reqwest::wasm::Response` (`!Send`) is
admitted. Native builds keep the strict `Send` bound.

## What did NOT land (deferred follow-up)

| Item | Why deferred | Owner / next step |
|------|--------------|-------------------|
| Streaming chat (`complete_stream`) | `StreamCallback` requires `Send`; needs a browser-flavoured callback type or `wasm-streams` rewrite | W-BROWSER §"What's Left" — *Streaming via `ReadableStream`* |
| `set_env` wiring | `BrowserRuntime` doesn't currently retain a handle to `BrowserPlatform`'s `BrowserEnvironment` (the platform is moved into `AgentLoop`); needs an `Arc<BrowserEnvironment>` clone stored alongside the agent | step6 BW6 *"set_env wiring"* |
| OPFS-backed `BrowserFileSystem` | `web-sys` `FileSystemFileHandle` bindings remain unstable | step4 BW4 §3.2 |
| `wasm-bindgen-test` regression for `init() + send_message()` end-to-end | No `wasm-bindgen-test` harness exists in the workspace yet | P0.4 in 16-browser-wasm.md |
| Binary size budget (<300 KB raw / <120 KB gzip) | Wiring the full pipeline pushed the bindgen artefact from ~840 KB to ~1.32 MB. `wasm-opt -Oz` not yet wired into `wasm-browser.yml` | P0.5 in 16-browser-wasm.md |

## Verification

* `scripts/build.sh check` — PASS (cargo check --workspace, native).
* `cargo check --target wasm32-unknown-unknown -p clawft-wasm --no-default-features --features browser` — PASS.
* `scripts/build.sh browser` — PASS (release-wasm + wasm-bindgen, artefact regenerated under `crates/clawft-wasm/www/pkg/`).
* `scripts/build.sh native-debug` — PASS (full debug build of `weft` + `weaver`).
* `cargo build --release --bin weft --bin weaver` — PASS (full release build).

No tests exercise the browser path end-to-end yet; that is P0.4 above.

## Files touched

* `crates/clawft-core/src/agent/identity.rs` — RwLock import polyfill.
* `crates/clawft-core/src/bootstrap.rs` — feature-gate `agent_bus` references; add `build_browser_pipeline()`.
* `crates/clawft-core/src/pipeline/mod.rs` — register `browser_llm_adapter`.
* `crates/clawft-core/src/pipeline/browser_llm_adapter.rs` — NEW.
* `crates/clawft-core/src/pipeline/traits.rs` — `?Send` relaxation on `LlmTransport`; gate `complete_stream` to native.
* `crates/clawft-core/src/pipeline/transport.rs` — `?Send` relaxation on `LlmProvider` and the `OpenAiCompatTransport` impl.
* `crates/clawft-wasm/Cargo.toml` — add `chrono` to the `browser` feature.
* `crates/clawft-wasm/src/lib.rs::browser_entry` — replace direct
  `BrowserLlmClient::complete` call with `AgentLoop::handle_turn` dispatch.
