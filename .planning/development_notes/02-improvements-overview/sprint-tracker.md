# Sprint Tracker: Improvements Sprint (Phase 5)

**Project**: clawft
**Sprint**: Phase 5 -- Improvements Sprint
**Source**: `.planning/improvements.md`
**Orchestrator**: `.planning/sparc/02-improvements-overview/00-orchestrator.md`
**Test Baseline**: 2,075+ tests at sprint start
**Created**: 2026-02-19
**Closed**: 2026-04-28 (WEFT-24)
**Status**: CLOSED — historical reference only

---

## Closure Summary (2026-04-28)

This sprint tracker is **closed for live tracking**. It is retained
verbatim as a historical record of the Phase-5 improvements sprint
that ran from late February 2026 through the lead-up to the 0.7.0
release gate.

**Where to look now:**
- The 0.7.0 release-gate audit
  (`.planning/reviews/0.7.0-release-gate/`) is the current
  source-of-truth for outstanding work. The audits enumerate what
  shipped, what lingered, and what was deferred per workstream.
- Live tracking happens in **Plane** (`weftos` workspace, project
  WeftOS). Every audit-finding TODO from this sprint that was not
  resolved by the time of the audit was lifted into a Plane work
  item in cycle `0.7.x`, `0.8.x`, `0.9.x`, or `1.0.x`. Search the
  cycle by audit-finding label to find descendants of any item
  below.
- The MVP / Full-Vision checkboxes preserved below reflect the
  state at the time work paused on this tracker; they are not
  authoritative — see the per-workstream audits in
  `.planning/reviews/0.7.0-release-gate/` for shipped state.

The unchecked MVP / Full-Vision boxes (e.g. live hot-reload smoke,
FlowDelegator end-to-end against a real provider, F9a MCP client
endpoint smoke) were either superseded by the 0.7.0 release-gate
audit work or covered by Plane items in later cycles. Do not treat
the unchecked state as "still in flight" — it is a snapshot.

Workstreams A, B, I, J (rows below) are reflected in the
ws01-core/ws05-channels/ws06-memory/ws15-mcp audits at
`.planning/reviews/0.7.0-release-gate/0{1,5,6}*.md` and
`.../15-mcp-coordination.md`. Subsequent workstreams (C, D, E, F,
H, K, L, M) are similarly captured by their respective
release-gate audits.

---

## Milestone Status

- [ ] **MVP (Week 8)**: Plugin system (C1-C4) with skill precedence + hot-reload, email channel (E2), multi-agent routing (L1), 3 ported OpenClaw skills, F9a minimal MCP client, Claude Flow integration (M1-M3), all critical/high fixes (A1-A9) resolved, architecture cleanup (B1-B9) complete
- [ ] **Full Vision (Week 12)**: Browser automation, dev tool suite, F9b full MCP client, ClawHub with vector search, per-agent sandboxing, security plugin, Docker images, CI/CD pipeline, all forward-compat hooks for voice and UI in place

### MVP Verification Checklist
- [x] security-review passes without any p0.
- [x] `cargo test --workspace` passes with zero failures
- [x] `cargo clippy --workspace -- -D warnings` produces no warnings
- [x] Binary size < 10 MB (release build, default features)
- [x] Gateway starts, accepts email message, routes to agent, agent responds
- [x] `weft skill install` loads skill, tools appear in MCP `tools/list` -- verified: skill__prompt-log in tools/list (`78b3101`)
- [ ] Hot-reload: modify `SKILL.md`, verify tool list updates -- code complete, needs live integration test
- [ ] FlowDelegator: delegate task to Claude Code -- code complete, needs `ANTHROPIC_API_KEY` exported or in providers config (`78b3101` adds config fallback)
- [ ] MCP client (F9a): connect to external MCP server -- transport code works, needs running MCP server endpoint

---

## Element 03: Critical Fixes & Cleanup (Weeks 1-5)

**SPARC Dir**: `03-critical-fixes-cleanup`
**Workstreams**: A, B, I, J
**Dev Assignment**: `dev-assignment-03-critical-fixes.md`

### Workstream A: Critical Fixes (Week 1-2)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| A1 | Session key round-trip corruption | P0 | 1-2 | Not Started | clawft-core | Bug |
| A2 | Unstable hash function in embeddings | P0 | 1-2 | Not Started | clawft-core | Bug |
| A3 | Invalid JSON from error formatting | P0 | 1-2 | Not Started | clawft-core | Bug |
| A4 | Plaintext credentials in config structs | P0 | 1-2 | Not Started | clawft-types | Security |
| A5 | API key echoed during onboarding | P0 | 1-2 | Not Started | clawft-cli | Security |
| A6 | Incomplete private IP range in SSRF protection | P0 | 1-2 | Not Started | clawft-services | Security |
| A7 | No HTTP request timeout on LLM provider client | P0 | 1-2 | Not Started | clawft-llm | Reliability |
| A8 | `unsafe std::env::set_var` in parallel tests | P1 | 1-2 | Not Started | clawft-core | Correctness |
| A9 | `--no-default-features` does not compile | P1 | 1-2 | Not Started | clawft-cli | Bug |

### Workstream B: Architecture Cleanup (Week 2-4)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| B1 | Unify `Usage` type across crates | P1 | 2-3 | Not Started | clawft-types, clawft-llm | Refactor |
| B2 | Unify duplicate `LlmMessage` types | P1 | 2-3 | Not Started | clawft-core | Refactor |
| B3 | Split oversized files (9 files, 950-1668 lines) | P1 | 2-4 | Not Started | Multiple | Refactor |
| B4 | Unify cron storage formats | P1 | 2-3 | Not Started | clawft-cli, clawft-services | Bug/Refactor |
| B5 | Extract shared tool registry builder | P1 | 2-3 | Not Started | clawft-cli | Refactor |
| B6 | Extract shared policy types | P1 | 2-3 | Not Started | clawft-services, clawft-tools | Refactor |
| B7 | Deduplicate `ProviderConfig` naming collision | P1 | 2-3 | Not Started | clawft-llm, clawft-types | Refactor |
| B8 | Consolidate `build_messages` duplication | P2 | 3-4 | Not Started | clawft-core | Refactor |
| B9 | MCP protocol version constant | P2 | 2-3 | Not Started | clawft-services | Cleanup |

### Workstream I: Type Safety & Cleanup (Week 2-6)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| I1 | `DelegationTarget` serde consistency | P2 | 2-3 | Not Started | clawft-types | Fix |
| I2 | String-typed policy modes to enums | P2 | 2-3 | Not Started | clawft-types | Fix |
| I3 | `ChatMessage::content` serialization | P1 | 2-3 | Not Started | clawft-llm | Fix |
| I4 | Job ID collision fix | P1 | 2-3 | Not Started | clawft-cli | Fix |
| I5 | `camelCase` normalizer acronym handling | P2 | 3-4 | Not Started | clawft-platform | Fix |
| I6 | Dead code removal | P2 | 2-6 | Not Started | Multiple | Cleanup |
| I7 | Fix always-true test assertion | P2 | 2-3 | Not Started | clawft-core | Fix |
| I8 | Share `MockTransport` across crates | P2 | 3-4 | Not Started | clawft-services | Fix |

### Workstream J: Documentation & Docs Sync (Week 3-5)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| J1 | Fix provider counts in docs | P1 | 3-4 | Not Started | docs/ | Doc fix |
| J2 | Fix assembler truncation description | P1 | 3-4 | Not Started | docs/ | Doc fix |
| J3 | Fix token budget source reference | P1 | 3-4 | Not Started | docs/ | Doc fix |
| J4 | Document identity bootstrap behavior | P1 | 3-4 | Not Started | docs/ | Doc |
| J5 | Document rate-limit retry behavior | P2 | 4-5 | Not Started | docs/ | Doc |
| J6 | Document CLI log level change | P2 | 3-4 | Not Started | docs/ | Doc |
| J7 | Plugin system documentation | P1 | 5+ | Not Started | docs/ | Doc |

**Element 03 Summary**: 33 items (A: 9, B: 9, I: 8, J: 7)

---

## Element 04: Plugin & Skill System (Weeks 3-8)

**SPARC Dir**: `04-plugin-skill-system`
**Workstream**: C
**Dev Assignment**: `dev-assignment-04-plugin-skill-system.md`

### Workstream C: Plugin & Skill System (Week 3-8)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| C1 | Define `clawft-plugin` trait crate | P0 | 3-4 | Not Started | clawft-plugin (new) | Feature |
| C2 | WASM plugin host | P0 | 4-6 | Not Started | clawft-wasm, clawft-plugin | Feature |
| C3 | Skill Loader (OpenClaw-compatible) | P0 | 4-6 | Not Started | clawft-core | Feature |
| C4 | Dynamic skill loading & hot-reload | P0 | 5-7 | Not Started | clawft-core | Feature |
| C4a | Autonomous skill creation | P2 | 8+ | Not Started | clawft-core | Feature |
| C5 | Wire interactive slash-command framework | P1 | 6-7 | Not Started | clawft-cli | Feature |
| C6 | Extend MCP server for loaded skills | P1 | 6-7 | Not Started | clawft-services | Feature |
| C7 | Update PluginHost to unify channels + tools | P1 | 5-6 | Not Started | clawft-channels | Refactor |

**Element 04 Summary**: 8 items (C: 8)

---

## Element 05: Pipeline & LLM Reliability (Weeks 2-5)

**SPARC Dir**: `05-pipeline-reliability`
**Workstream**: D
**Dev Assignment**: `dev-assignment-05-pipeline-reliability.md`

### Workstream D: Pipeline & LLM Reliability (Week 2-5)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| D1 | Parallel tool execution | P1 | 2-3 | Not Started | clawft-core | Performance |
| D2 | Streaming failover correctness | P1 | 3-4 | Not Started | clawft-llm | Bug |
| D3 | Structured error variants for retry | P1 | 2-3 | Not Started | clawft-llm | Refactor |
| D4 | Configurable retry policy | P2 | 3-4 | Not Started | clawft-core | Feature |
| D5 | Record actual latency | P1 | 2-3 | Not Started | clawft-core | Feature |
| D6 | Thread `sender_id` for cost recording | P1 | 3-4 | Not Started | clawft-core | Feature |
| D7 | Change `StreamCallback` to `FnMut` | P2 | 2-3 | Not Started | clawft-core | Fix |
| D8 | Bounded message bus channels | P1 | 3-4 | Not Started | clawft-core | Reliability |
| D9 | MCP transport concurrency | P1 | 3-5 | Not Started | clawft-services | Performance |
| D10 | Cache skill/agent bootstrap files | P2 | 4-5 | Not Started | clawft-core | Performance |
| D11 | Async file I/O in skills loader | P2 | 4-5 | Not Started | clawft-core | Performance |

**Element 05 Summary**: 11 items (D: 11)

---

## Element 06: Channel Enhancements (Weeks 4-8)

**SPARC Dir**: `06-channel-enhancements`
**Workstream**: E
**Dev Assignment**: `dev-assignment-06-channel-enhancements.md`

### Workstream E: Channel Enhancements (Week 4-8)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| E1 | Discord Resume (OP 6) | P1 | 4-5 | Not Started | clawft-channels | Feature |
| E2 | Email channel plugin | P0 | 4-6 | Not Started | New plugin | Feature |
| E3 | WhatsApp channel | P1 | 5-7 | Not Started | New plugin | Feature |
| E4 | Signal / iMessage bridge | P2 | 6-8 | Not Started | New plugin | Feature |
| E5 | Matrix / IRC channels | P2 | 6-8 | Not Started | New plugin | Feature |
| E5a | Google Chat channel | P2 | 6-8 | Not Started | New plugin | Feature |
| E5b | Microsoft Teams channel | P2 | 6-8 | Not Started | New plugin | Feature |
| E6 | Enhanced heartbeat / proactive check-in | P2 | 5-7 | Not Started | clawft-services | Feature |

**Element 06 Summary**: 8 items (E: 8)

---

## Element 07: Dev Tools & Apps (Weeks 5-10)

**SPARC Dir**: `07-dev-tools-apps`
**Workstream**: F
**Dev Assignment**: `dev-assignment-07-dev-tools-apps.md`

### Workstream F: Software Dev & App Tooling (Week 5-10)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| F1 | Git tool plugin | P1 | 5-7 | Not Started | New plugin | Feature |
| F2 | Cargo/build integration | P2 | 6-8 | Not Started | New plugin | Feature |
| F3 | Code analysis via tree-sitter | P2 | 6-8 | Not Started | New plugin | Feature |
| F4 | Browser CDP automation | P1 | 7-9 | Not Started | New plugin | Feature |
| F5 | Calendar integration | P2 | 7-9 | Not Started | New plugin | Feature |
| F6 | Generic REST + OAuth2 helper | P1 | 5-6 | Not Started | New plugin | Feature |
| F7 | Docker/Podman orchestration tool | P2 | 7-9 | Not Started | New plugin | Feature |
| F8 | MCP deep IDE integration | P1 | 8-10 | Not Started | clawft-services | Feature |
| F9a | MCP client -- minimal (single server) | P0 | 5-6 | Not Started | clawft-services | Feature |
| F9b | MCP client -- full (auto-discovery, pooling) | P1 | 9-10 | Not Started | clawft-services | Feature |

**Element 07 Summary**: 10 items (F: 10, with F9 split into F9a/F9b)

---

## Element 08: Memory & Workspace (Weeks 4-8)

**SPARC Dir**: `08-memory-workspace`
**Workstream**: H
**Dev Assignment**: `dev-assignment-08-memory-workspace.md`

### Workstream H: Memory & Workspace (Week 4-8)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| H1 | Markdown workspace with per-agent isolation | P0 | 4-6 | Not Started | clawft-core | Feature |
| H2 | Complete RVF Phase 3 (vector memory) | P0 | 4-8 | Not Started | clawft-core | Feature |
| H2.1 | HNSW-backed VectorStore | P0 | 4-6 | Not Started | clawft-core | Feature |
| H2.2 | Production embedder (LLM embedding API) | P0 | 5-6 | Not Started | clawft-core | Feature |
| H2.3 | RVF file I/O (segment read/write) | P1 | 5-7 | Not Started | clawft-core | Feature |
| H2.4 | `weft memory export` / `weft memory import` CLI | P1 | 6-7 | Not Started | clawft-cli | Feature |
| H2.5 | POLICY_KERNEL storage | P1 | 6-7 | Not Started | clawft-core | Feature |
| H2.6 | WITNESS segments (tamper-evident audit) | P2 | 7-8 | Not Started | clawft-core | Feature |
| H2.7 | Temperature-based quantization | P2 | 8+ | Not Started | clawft-core | Feature |
| H2.8 | WASM micro-HNSW compatibility | P2 | 8+ | Not Started | clawft-wasm | Feature |
| H3 | Standardize timestamp representations | P1 | 4-5 | Not Started | clawft-types | Refactor |

**Element 08 Summary**: 11 items (H: 11, with H2 expanded into sub-items)

---

## Element 09: Multi-Agent Routing & Orchestration (Weeks 3-9)

**SPARC Dir**: `09-multi-agent-routing`
**Workstreams**: L, M
**Dev Assignment**: `dev-assignment-09-multi-agent-routing.md`

### Workstream L: Multi-Agent Routing & Orchestration (Week 5-9)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| L1 | Agent routing table | P0 | 5-6 | Not Started | clawft-core | Feature |
| L2 | Per-agent workspace and session isolation | P1 | 6-8 | Not Started | clawft-core | Feature |
| L3 | Multi-agent swarming | P2 | 7-9 | Not Started | clawft-core | Feature |
| L4 | Planning strategies in Router (ReAct) | P2 | 8-9 | Not Started | clawft-core | Feature |

### Workstream M: Claude Flow / Claude Code Integration (Week 3-7)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| M1 | Implement `FlowDelegator` | P0 | 3-5 | Not Started | clawft-services | Feature |
| M2 | Wire `flow_available` to runtime detection | P0 | 3-4 | Not Started | clawft-tools | Bug fix |
| M3 | Enable `delegate` feature by default | P0 | 3-4 | Not Started | clawft-cli, clawft-services | Config fix |
| M4 | Dynamic MCP server discovery | P1 | 5-6 | Not Started | clawft-cli, clawft-services | Feature |
| M5 | Claude Code as MCP client transport | P1 | 6-7 | Not Started | clawft-services | Feature |
| M6 | Delegation config documentation | P2 | 7 | Not Started | docs/ | Doc |

**Element 09 Summary**: 10 items (L: 4, M: 6)

---

## Element 10: Deployment & Community (Weeks 8-12)

**SPARC Dir**: `10-deployment-community`
**Workstream**: K (in-scope: K2-K5, K3a)
**Dev Assignment**: `dev-assignment-10-deployment-community.md`

### Workstream K: Deployment & Community (Week 8-12)

| # | Item | Priority | Week | Status | Crate | Type |
|---|------|----------|------|--------|-------|------|
| K2 | Multi-arch Docker images | P1 | 8-9 | Not Started | Dockerfile, scripts/ | DevOps |
| K2-CI | CI/CD pipeline (PR gates + release) | P1 | 8-9 | Not Started | .github/workflows/ | DevOps |
| K3 | Enhanced sandbox with per-agent isolation | P1 | 9-11 | Not Started | clawft-plugin, clawft-wasm, clawft-core | Security |
| K3a | Security plugin system (50+ audit checks) | P1 | 9-11 | Not Started | clawft-security (new) | Security |
| K4 | ClawHub skill registry with vector search | P1 | 10-12 | Not Started | clawft-services, clawft-cli | Feature |
| K5 | Benchmarks vs OpenClaw | P1 | 10-12 | Not Started | benches/, scripts/bench/ | Testing |

**Element 10 Summary**: 6 items (K: 6)

---

## Out of Scope Items (Tracked for Reference)

| Item | Workstream | Reason | Future Track |
|------|-----------|--------|-------------|
| G (Voice) | G | Deferred to post-sprint | `voice_development.md` |
| K1 (Web Dashboard + Live Canvas) | K | Deferred to post-sprint | `ui_development.md` |
| K6 (Native Shells) | K | Deferred to post-sprint | `ui_development.md` |

**Forward-compatibility hooks** (in scope, tracked under C1/C7):
- [ ] C1 defines `VoiceHandler` trait placeholder
- [ ] Plugin manifest reserves `voice` capability type
- [ ] `ChannelAdapter` trait supports binary/audio payloads
- [ ] Feature flag `voice` wired as no-op in Cargo.toml
- [ ] Agent loop and bus support structured/binary payloads (for future UI)
- [ ] MCP server tool schemas stable for future dashboard introspection
- [ ] Config and session APIs read-accessible without agent loop

---

## Cross-Element Integration Tests

| Test | Elements | Week | Priority | Status |
|------|----------|------|----------|--------|
| Email Channel -> OAuth2 Helper | 06, 07 | 7 | P0 | Not Started |
| Plugin -> Hot-reload -> MCP | 04, 07 | 8 | P0 | Not Started |
| FlowDelegator -> Per-Agent Isolation | 09, 08 | 9 | P0 | Not Started |
| Multi-Agent -> Bus Isolation | 09, 05 | 9 | P0 | Not Started |
| Agent Routing -> Sandbox | 09, 10 | 10 | P1 | Not Started |
| Vector Search -> ClawHub Discovery | 08, 10 | 11 | P1 | Not Started |
| ClawHub Install -> Security Scan | 10, 04 | 11 | P1 | Not Started |

Test infrastructure: `tests/integration/cross_element/`

---

## Sprint Summary

| Element | Workstreams | Items | Weeks | Key Deliverables |
|---------|-------------|-------|-------|-----------------|
| 03 | A, B, I, J | 33 | 1-5 | All critical fixes, architecture cleanup, type safety, doc sync |
| 04 | C | 8 | 3-8 | Plugin trait crate, WASM host, skill loader, hot-reload |
| 05 | D | 11 | 2-5 | Parallel tools, streaming failover, bounded bus, MCP transport |
| 06 | E | 8 | 4-8 | Discord resume, email channel, WhatsApp, messaging platforms |
| 07 | F | 10 | 5-10 | Git, tree-sitter, browser, OAuth2, MCP client (F9a/F9b split) |
| 08 | H | 11 | 4-8 | Per-agent workspace, vector memory (RVF Phase 3), timestamps |
| 09 | L, M | 10 | 3-9 | Agent routing, Claude Flow integration, MCP bridge |
| 10 | K | 6 | 8-12 | Docker, CI/CD, sandbox, security plugin, ClawHub, benchmarks |
| **Total** | **A-M** | **97** | **1-12** | |

### Priority Distribution

| Priority | Count | Description |
|----------|-------|-------------|
| P0 | ~25 | Must-have for MVP or critical fix |
| P1 | ~45 | Important for full vision |
| P2 | ~27 | Nice-to-have, stretch goals, post-MVP |

### Weekly Execution Plan

| Weeks | Active Elements | Focus |
|-------|----------------|-------|
| 1-2 | 03, 05 | Critical fixes (A), type safety quick wins (I), pipeline early fixes (D) |
| 2-4 | 03, 05 | Architecture cleanup (B), pipeline reliability (D) |
| 3-5 | 03, 04, 09 | Doc sync (J), plugin trait crate (C1), Claude Flow (M1-M3) |
| 4-8 | 04, 06, 07, 08, 09 | Plugin system (C2-C7), channels (E), dev tools (F), memory (H), multi-agent (L, M4-M6) |
| 5-9 | 06, 07, 08, 09 | Channels complete, dev tools, MCP client, multi-agent routing |
| 8-12 | 10 | Docker, CI/CD, sandbox, security, ClawHub, benchmarks |

### Exit Criteria (Full Sprint)

- [ ] All P0 items complete and verified
- [ ] All P1 items complete or explicitly deferred with justification
- [ ] `cargo test --workspace` passes (2,075+ tests at baseline, expect significant growth)
- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] Binary size < 10 MB (release, default features)
- [ ] Docker image < 50MB compressed
- [ ] All 7 cross-element integration tests pass
- [ ] All 5 forward-compatibility verification tests pass
- [ ] Benchmark suite produces comparison report against OpenClaw
- [ ] Security scan runs 50+ checks across 8+ categories
- [ ] All documentation updated to match implementation
