# ClawFT Master Plan: Three-Workstream Orchestration

**Date**: 2026-02-24
**Status**: Ready for Implementation
**Workstreams**: W-BROWSER (6 phases), W-UI (17 phases), W-VOICE (10 phases)
**Estimated Duration**: 10 weeks (parallel execution)
**Cross-Plan Analysis**: `.planning/sparc/cross-plan-analysis.md`

---

## 1. Executive Summary

This plan orchestrates three concurrent workstreams that transform ClawFT from a CLI-only tool into a multi-platform agent system:

| Workstream | What It Delivers | Weeks | Phases |
|---|---|---|---|
| **W-BROWSER** | Real `AgentLoop<BrowserPlatform>` in browser via WASM | 1-6 | BW1-BW6 |
| **W-UI** | React dashboard + Live Canvas + WASM integration | 1-10 | S1.1-S3.7 |
| **W-VOICE** | Voice I/O pipeline: STT, TTS, Talk Mode, Wake Word | 0-9 | VP, VS1.1-VS3.3 |

All three share zero code paths at implementation time. They converge only at integration points (VS3.1 needs UI S1+S2; UI S3.6 needs Browser BW5). The native CLI build remains untouched throughout.

### How to Use This Plan

This document is the **top-level context** (~2000 tokens of essential info). Each workstream has its own orchestrator with phase-level detail (~1500 tokens each). Each phase has a full SPARC spec (~4000-6000 tokens). An implementing agent reads at most 3 docs: this plan's Section 4 + the phase orchestrator + the phase spec.

```
Level 0: This file (00-master-plan.md)           ~2000 tokens  ← you are here
Level 1: Workstream orchestrator (00-orchestrator.md)  ~1500 tokens each
Level 2: Phase spec (01-phase-*.md)                    ~4000-6000 tokens each
Level 3: Source files listed in phase spec             ~2000-4000 tokens
```

---

## 2. Dependency Graph (Cross-Workstream)

```
                         EXISTING INFRASTRUCTURE
                    Phase 4 (Tiered Router, Plugin System)
                        C1 (Plugin Traits) ── DONE
                        C7 (PluginHost)    ── DONE
                                   |
             +---------------------+---------------------+
             |                     |                     |
             v                     v                     v
     [W-BROWSER BW1]        [W-UI S1.1]          [W-VOICE VP]
     Foundation              Backend API           Pre-Validation
             |                     |                     |
             v                     v                     v
     [BW2 Core Engine]      [S1.2+S1.3]          [VS1.1 Audio]
             |               Core Views                  |
             v                     |                     v
     [BW3 LLM Transport]          |              [VS1.2 STT/TTS]
             |                     |                     |
             v                     +-----> [VS1.3 VoiceChannel]
     [BW4 BrowserPlatform]        |        (needs S1.1 WS handler)
             |                     |                     |
             v                     v                     v
     [BW5 WASM Entry]       [S2.1-S2.5]          [VS2.1-VS2.3]
             |               Canvas+Advanced      Wake+Platform
             v                     |                     |
     [BW6 Integration]            v                     v
                            [S3.1-S3.5]          [VS3.1 UI Voice]
                             Polish+Prod    (needs S1+S2 complete)
                                   |                     |
                                   v                     v
                            [S3.6 WASM Integ]     [VS3.2-VS3.3]
                            (needs BW5)           Cloud+Advanced
```

### Hard Dependencies (Blockers)

| ID | Source | Target | Nature |
|---|---|---|---|
| D1 | W-UI S1.1 (WS handler) | W-VOICE VS1.3 | Voice status events need WebSocket transport |
| D2 | W-UI S1+S2 | W-VOICE VS3.1 | Voice UI components need dashboard shell |
| D3 | W-BROWSER BW5 (WASM entry) | W-UI S3.6 | WASM adapter needs `init()`, `send_message()` exports |
| D4 | Each BW phase | Next BW phase | Linear chain: BW1→BW2→BW3→BW4→BW5→BW6 |

### Independence Guarantees

- **BW1-BW5 are fully independent** of W-UI and W-VOICE
- **W-UI S1-S2 are fully independent** of W-BROWSER and W-VOICE
- **W-VOICE VP-VS2 are independent** of W-BROWSER (only VS1.3 needs UI S1.1)
- **All three can start in Week 1** with zero conflicts

---

## 3. Pre-Implementation Actions (Step 0)

Before any workstream begins, land these preparatory changes in a single PR to eliminate the highest-risk merge conflicts.

### A1: Unified Config PR

All three workstreams add fields to `clawft-types/src/config/mod.rs`. Land them together:

| Workstream | Struct | Fields Added |
|---|---|---|
| W-VOICE | Root `Config` | `voice: VoiceConfig` (~200 lines, split to `config/voice.rs` if >150 lines) |
| W-UI | `GatewayConfig` | `api_port: u16`, `cors_origins: Vec<String>`, `api_enabled: bool` |
| W-BROWSER | `ProviderConfig` | `browser_direct: bool`, `cors_proxy: Option<String>` |

All new fields use `#[serde(default)]` -- existing configs parse without changes.

**Why**: This file is the #1 merge conflict hotspot. One PR eliminates all future conflicts.

### A2: Unified CI Pipeline Update

Add all three workstream gates to `.github/workflows/pr-gates.yml` in one PR:

```yaml
# Browser WASM compilation check
wasm-browser-check: cargo check --target wasm32-unknown-unknown -p clawft-wasm --no-default-features --features browser

# UI lint + type-check + test (when ui/ exists)
ui-check: cd ui && pnpm lint && pnpm type-check && pnpm test

# Voice feature compilation check
voice-check: cargo check --features voice -p clawft-plugin
```

### A3: Fix ProviderConfig Naming

Browser plan code samples use `base_url` but actual struct has `api_base`. Add `#[serde(alias = "base_url")]` to the field, or update plan samples to use `api_base`.

### A4: Feature Validation Script

Use `scripts/build.sh gate` (the canonical phase-gate entrypoint).
WEFT-409 (2026-04-30): supersedes the originally-planned standalone
`scripts/check-features.sh`, which was never created. `build.sh gate`
runs the 12-check suite (native + WASI + browser + clippy + bundle-
size + audit + docs regen) and is what every phase gate should call.

```bash
scripts/build.sh gate
# Or, for individual targets:
scripts/build.sh check        # native cargo check --workspace
scripts/build.sh wasi         # wasm32-wasip2
scripts/build.sh browser      # wasm32-unknown-unknown
```

---

## 4. Execution Schedule

### Week-by-Week Parallel Execution

```
         Week 0    Week 1    Week 2    Week 3    Week 4    Week 5    Week 6    Week 7    Week 8    Week 9    Week 10
           |         |         |         |         |         |         |         |         |         |         |
  STEP 0:  |--A1,A2--|
           | config  |
           | + CI PR |
           |         |
  BROWSER: |         |---BW1---|---BW2---|---BW3---|---BW4---|---BW5---|---BW6---|
           |         |Foundation|Core Eng|LLM Xport|BrwsPlatf|WASM Entry|Integr  |
           |         |         |         |         |         |         |         |
  UI:      |         |--S1.1---+--S1.3---|--S2.1---|--S2.2-S2.5-------|--S3.1-S3.5------|--S3.6--|--S3.7--|
           |         |--S1.2---+         | Canvas  | Skill/Mem/Cfg/Cron| Deleg/Canvas/  |WASM Int|  Docs  |
           |         |Backend  |CoreViews|         |                   | PWA/Tauri/Prod |        |        |
           |         |+Frontend|         |         |                   |                |        |        |
           |         |         |         |         |         |         |                |        |        |
  VOICE:   |--VP-----|--VS1.1--|--VS1.2--|--VS1.3--|--VS2.1--|--VS2.2--|--VS2.3--|--VS3.1--|VS3.2-|VS3.3--|
           |PreValid |Audio Fdn|STT+TTS  |VoiceChan|Wake Word|Echo+Qual|Platform |UI Voice |Cloud |Advancd|
```

### Phase-by-Phase Sequencing

| Step | Week | W-BROWSER | W-UI | W-VOICE | Parallel? | Conflict Risk |
|---|---|---|---|---|---|---|
| 0 | 0 | — | — | VP (pre-validation) | N/A | None |
| 1 | 1-2 | BW1: Foundation | S1.1 + S1.2: Backend + Frontend | VS1.1: Audio Foundation | YES | None (different crates) |
| 2 | 2-3 | BW2: Core Engine | S1.3: Core Views | VS1.2: STT + TTS | YES | None (different crates) |
| 3 | 3-4 | BW3: LLM Transport | S2.1: Live Canvas | VS1.3: VoiceChannel | YES* | *VS1.3 needs S1.1 WS handler |
| 4 | 4-5 | BW4: BrowserPlatform | S2.2-S2.5: Advanced | VS2.1: Wake Word | YES | Low (clawft-tools Cargo.toml) |
| 5 | 5-6 | BW5+BW6: WASM Entry+Integration | S3.1-S3.5: Polish | VS2.2-VS2.3: Quality+Platform | YES | Low |
| 6 | 7-8 | — | S3.1-S3.5 cont. | VS3.1: UI Voice Integration | YES** | **VS3.1 needs UI S1+S2 |
| 7 | 9-10 | — | S3.6+S3.7: WASM+Docs | VS3.2-VS3.3: Cloud+Advanced | YES*** | ***S3.6 needs BW5 |

### Mitigation for Cross-Stream Dependencies

| Dependency | Mitigation if Source is Late |
|---|---|
| VS1.3 needs S1.1 WS handler | Voice works CLI-only via `MessageBus` direct publish. WS transport is delivery, not function. |
| VS3.1 needs UI S1+S2 | Voice UI components can be developed against MSW mocks. Integration tested when UI lands. |
| S3.6 needs BW5 WASM entry | WASM adapter developed against mock that returns canned responses. Real integration when BW5 delivers. |

---

## 5. Phase Gate Protocol

Every phase in every workstream MUST pass before merging:

```bash
# Gate 1: Native regression (zero test failures)
cargo test --workspace

# Gate 2: Native CLI binary builds
cargo build --release --bin weft

# Gate 3: Existing WASI WASM build (unchanged)
cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm

# Gate 4: Browser WASM check (after BW1 establishes the feature)
cargo check --target wasm32-unknown-unknown -p clawft-wasm --no-default-features --features browser

# Gate 5: Feature validation (WEFT-409: scripts/build.sh gate
# supersedes the never-created scripts/check-features.sh)
scripts/build.sh gate
```

### Native Regression Risks (N1-N7)

| ID | Risk | Prevention |
|---|---|---|
| N1 | `cargo build` breaks | Every optional dep MUST appear in `default = ["native"]` |
| N2 | Tests fail with default features | Test modules use `#[cfg(test)]`, compile under default features |
| N3 | Downstream crate import breaks | Gate types behind `native` → add re-export under default feature |
| N4 | `Send` bounds lost on native | `#[cfg_attr(feature = "browser", async_trait(?Send))]` + `#[cfg_attr(not(feature = "browser"), async_trait)]` |
| N5 | WASI WASM build breaks | Browser support in new `browser` feature; existing defaults unchanged |
| N6 | Clippy warnings from dead code | Clean feature separation; no `#[allow(dead_code)]` |
| N7 | Feature unification | `native` and `browser` mutually exclusive; never both in same dep |

---

## 6. Known Gaps (from Cross-Plan Analysis)

18 gaps identified. Top items requiring attention:

| # | Gap | Severity | Owner | When |
|---|---|---|---|---|
| G1 | IndexedDB session persistence unspecified | MEDIUM | W-BROWSER | BW4 |
| G5 | No unified config migration strategy | MEDIUM | All | Step 0 (A1) |
| G8 | WebSocket auth not specified | MEDIUM | W-UI | S1.1 |
| G12 | VoiceChannel WS events need UI S1.1 | MEDIUM | W-VOICE | VS1.3 (mitigated by CLI-only mode) |
| G13 | sherpa-rs version pinning deferred | MEDIUM | W-VOICE | VP |
| G14 | Voice model download needs reqwest | LOW | W-VOICE | VS1.1 |
| G15 | No unified config reference doc | MEDIUM | All | Post-implementation |
| G16 | CI pipeline ownership fragmented | LOW | All | Step 0 (A2) |

Full gap inventory: `.planning/sparc/cross-plan-analysis.md` Section 4.

---

## 7. Synergies

| Synergy | Workstreams | Effort | When |
|---|---|---|---|
| WASM embedding in React dashboard (serverless mode) | Browser + UI | Medium | S3.6 (planned) |
| Voice controls in dashboard | Voice + UI | Low | VS3.1 (planned) |
| Shared config schema (`config.json` controls all features) | All | Zero | Automatic via Step 0 |
| Batch CI pipeline update | All | Low | Step 0 (A2) |
| Static file serving shared infra | Browser + UI | Low | BW6 + S1.1 |
| Voice in browser via Web Audio API | Browser + Voice | HIGH | Future (not in scope) |

---

## 8. Workstream Quick Reference

### W-BROWSER: Browser WASM

| Spec | Location |
|---|---|
| Orchestrator | `.planning/sparc/browser/00-orchestrator.md` |
| BW1 Foundation | `.planning/sparc/browser/01-phase-BW1-foundation.md` |
| BW2 Core Engine | `.planning/sparc/browser/02-phase-BW2-core-engine.md` |
| BW3 LLM Transport | `.planning/sparc/browser/03-phase-BW3-llm-transport.md` |
| BW4 BrowserPlatform | `.planning/sparc/browser/04-phase-BW4-browser-platform.md` |
| BW5 WASM Entry | `.planning/sparc/browser/05-phase-BW5-wasm-entry.md` |
| BW6 Integration | `.planning/sparc/browser/06-phase-BW6-integration.md` |
| Consensus Plan | `.planning/wasm-browser/00-consensus-plan.md` |
| Task Breakdown | `.planning/wasm-browser/05-task-breakdown.md` |

### W-UI: Web Dashboard

| Spec | Location |
|---|---|
| Orchestrator | `.planning/sparc/ui/00-orchestrator.md` |
| S1 Foundation | `.planning/sparc/ui/01-phase-S1-foundation-core-views.md` |
| S2 Canvas+Advanced | `.planning/sparc/ui/02-phase-S2-canvas-advanced-views.md` |
| S3 Polish+Production | `.planning/sparc/ui/03-phase-S3-polish-production.md` |
| Pre-Implementation | `.planning/sparc/ui/04-ui-pre-implementation.md` |
| Sprint Tracker | `.planning/sparc/ui/05-ui-sprint-tracker.md` |
| Security Review | `.planning/sparc/ui/06-ui-security-review.md` |

### W-VOICE: Voice Pipeline

| Spec | Location |
|---|---|
| Orchestrator | `.planning/sparc/voice/00-orchestrator.md` |
| VS1 Audio Foundation | `.planning/sparc/voice/01-phase-VS1-audio-foundation.md` |
| VS2 Wake+Platform | `.planning/sparc/voice/02-phase-VS2-wake-word-platform.md` |
| VS3 Advanced+UI | `.planning/sparc/voice/03-phase-VS3-advanced-ui-integration.md` |
| Pre-Implementation | `.planning/sparc/voice/04-voice-pre-implementation.md` |
| Sprint Tracker | `.planning/sparc/voice/05-voice-sprint-tracker.md` |
| Security Review | `.planning/sparc/voice/06-voice-security-review.md` |

### Cross-Cutting

| Doc | Location |
|---|---|
| Cross-Plan Gap Analysis | `.planning/sparc/cross-plan-analysis.md` |
| Architecture Quick Ref | Auto-memory: `architecture.md` |
| Workstream Dependencies | Auto-memory: `workstreams.md` |

---

## 9. Continuous Execution Protocol

This plan supports continuous execution by multiple agents. Each agent follows this protocol:

### Agent Startup

1. Read **this file** (Section 4 only) for current schedule position
2. Read the **workstream orchestrator** for the assigned workstream
3. Read the **phase spec** for the current phase
4. Read the **source files** listed in the phase spec
5. Total context: ~8000-10000 tokens. Fits in a single message window.

### Agent Work Cycle

```
1. Read phase spec
2. Verify blockers resolved (check dependency table in Section 4)
3. Implement deliverables listed in phase spec
4. Run phase gate checks (Section 5)
5. Mark phase complete
6. Move to next phase (or next workstream phase if current workstream is blocked)
```

### Parallel Agent Assignment

Up to 3 agents can work simultaneously (one per workstream) during Steps 1-5. During Steps 6-7, cross-stream dependencies require coordination:

| Agent | Workstream | Weeks 1-6 | Weeks 7-10 |
|---|---|---|---|
| Agent A | W-BROWSER | BW1→BW2→BW3→BW4→BW5→BW6 | Done (or help UI/Voice) |
| Agent B | W-UI | S1.1→S1.2→S1.3→S2.1→S2.2-S2.5→S3.1-S3.5 | S3.6→S3.7 |
| Agent C | W-VOICE | VP→VS1.1→VS1.2→VS1.3→VS2.1→VS2.2→VS2.3 | VS3.1→VS3.2→VS3.3 |

### Completion Criteria

The plan is complete when ALL of these are true:

- [ ] `cargo test --workspace` passes (823+ tests, zero failures)
- [ ] `cargo build --release --bin weft` produces native CLI binary
- [ ] `cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm` produces WASI WASM
- [ ] `wasm-pack build crates/clawft-wasm --target web --features browser` produces browser WASM < 500KB gzip
- [ ] Browser WASM runs full pipeline: classify → route → assemble → LLM → tool use → response
- [ ] `pnpm build` in `ui/` produces dashboard < 200KB gzip
- [ ] Dashboard connects to both Axum backend and WASM module
- [ ] `weft voice talk` runs full loop: listen → transcribe → agent → speak → listen
- [ ] All documentation written (11 browser docs, 4 UI docs, voice CLI help)
- [ ] `scripts/build.sh gate` passes all targets (WEFT-409: supersedes the never-created `scripts/check-features.sh`)

---

## 10. Risk Matrix (Cross-Workstream)

| Risk | Probability | Impact | Mitigation |
|---|---|---|---|
| `config/mod.rs` merge conflicts | HIGH | LOW | Step 0 A1: unified config PR |
| Voice VS1.3 blocked on UI S1.1 | MEDIUM | MEDIUM | CLI-only mode until WS handler lands |
| Browser BW1 breaks native builds | MEDIUM | HIGH | Phase gate: `cargo test --workspace` after every change |
| sherpa-rs version incompatibility | MEDIUM | HIGH | VP prototype validation before VS1.1 |
| WASM binary > 500KB gzip | MEDIUM | MEDIUM | `wasm-opt -Oz`, dep audit with `twiggy` |
| CORS blocks browser LLM calls | HIGH | HIGH | Anthropic direct header; proxy for others |
| CI file conflicts | LOW | LOW | Step 0 A2: batch CI updates |
| Week 7-9 overload (Voice+UI integration) | MEDIUM | MEDIUM | VS3.1 UI work done by UI team; VS3.2-3.3 are P2 (deferrable) |
