---
title: "0.7.0 Release-Gate Audit — Index"
slug: "release-gate-index-0-7-0"
audit_id: "0.7.0-release-gate"
audit_run_at: 2026-04-28
last_updated: 2026-04-28
audit_scope: "Total depth — every TODO, FIXME, deferred item, orphan, and open question across all workstreams. NOT filtered by 0.7.0 scope; the 0.7 scope decision is downstream of this audit."
methodology: "17 parallel general-purpose subagents, one per workstream. Each agent independently grepped source, read planning docs / handoffs / ADRs / sprint trackers, and produced a structured per-workstream document. This index is the synthesised top-level view."
project: clawft
project_branch: development-0.7.0
project_version_at_audit: 0.6.19
post_audit_commits_unpushed: 4
workstream_count: 17
total_open_tasks_estimated: ~430
status_legend:
  landed: "Feature is in tree, tested, and considered shipped within the version."
  active: "Currently being worked on (commits in flight or sprint open)."
  partial: "Some sub-phases shipped; others deferred or stubbed."
  planning: "Spec / SPARC / plan written; little or no code."
  orphaned: "Was being worked on; current owner / momentum unclear."
  blocked: "Waiting on an upstream dependency or unresolved decision."
related_handoffs:
  - docs/handoff.md
  - .planning/development_notes/sprint-17-tasklist.md
related_adrs_total: 47
---

# 0.7.0 Release-Gate Audit — Index

This index points at 17 per-workstream audit documents under
`.planning/reviews/0.7.0-release-gate/`. Each per-workstream doc carries
the full task list, code-level TODOs, deferred items, orphan tracking,
and source citations. **This is a comprehensive audit — every workstream
is enumerated regardless of whether it's intended to ship in 0.7.0.**
The 0.7 scope decision is a *downstream* exercise once the totals are
visible.

## Audit-wide headlines

Top cross-cutting risks pulled from the 17 reports, in rough impact order:

1. **02 Kernel — two CRITICAL governance-gate FAILs in `auth_service.rs`** (`rotate_credential` L325, `request_token` L354 chain-logged but not gated). Plus **the tracing → ChainManager bridge is missing in `clawft-weave/src/main.rs`** — every `chain_event` `tracing` event currently lands in stdout and *never reaches ExoChain*.
2. **05 Channels — 7 of 11 channel adapters are stubs** (email, google_chat, teams, whatsapp, signal, matrix, irc). `send()` returns synthetic IDs and `start()` is a `debug!` log; any production deployment with those features enabled silently drops every message. The SPARC tracker reports "9/9 complete".
3. **17 Research — Democritus loop is shipped but misbehaving live.** `kernel.log` shows continuous `Stuck { net_change: 0.0 }` since 10:47 UTC on audit day. Filed as P0 in stream 17.
4. **16 Browser WASM — pipeline never wired.** BW1-BW6 are reported "complete" and the crate stack does compile under `wasm32-unknown-unknown --features browser`, but `browser_entry::send_message` shortcuts directly to `BrowserLlmClient::complete` and skips the entire 6-stage pipeline. The actual W-BROWSER goal is not met.
5. **07 Multi-agent — element-09 tracker says "14/14 done" but only the type-level scaffolding shipped.** `FlowDelegator` was never created; `McpServerManager::remove_server` and `McpBridge::initialize` carry self-documenting "in a full implementation..." stubs; `AgentRouter` is stored on `AgentContext` but the inbound dispatch path never consults it. Recursive-delegation guard is also missing.
6. **10 Voice — 5 P0 security controls (SC-1/4/7/9/10) are not implemented.** `clawft-plugin/voice/*` (planned in-process sherpa-rs) and `clawft-service-whisper`/`-classify` (real, working substrate-side whisper.cpp HTTP pipeline) coexist with no ADR reconciling them. sherpa-rs and cpal are not in `Cargo.toml`. Calling this an "MVP" today would be unsafe.
7. **14 Deployment — internal-dep version drift will break the next publish.** `Cargo.toml` workspace at `0.6.19` while every internal `clawft-*` path-dep is pinned at `0.6.6`. Plus stale `ghcr.io/clawft/clawft` paths in compose / vps-deploy / `docs/deployment/*.md`, and the docs site release notes are 6 versions behind CHANGELOG.
8. **17 Research — JEPA / LeWM world model is the largest active research bet, but none of the 7 implementation crates exist on master yet** (`weftos-worldmodel-{core,impls,facade}`, `weftos-sensor-pipeline{,-wire}`, `clawft-worldmodel-service`, `clawft-delegation`). ADRs 048-058 are drafted; the marketing page at `docs/src/app/lewm-worldmodel-rs/` is scaffolded; implementation is greenfield.
9. **12 Knowledge graph — Sprint 17 (the currently active sprint) has 18 KG-NNN tasks**; ~11 have first-cut implementations in tree (KG-001..KG-010 mostly done), KG-011/012 are stubs blocked on upstream `ruvllm-wasm`'s 11-pattern HNSW cap, KG-013/015/017/018 are not started. The DiskANN backend is still a `HashMap` linear-scan stub.
10. **11 Agent-core-v1 — all 12 acceptance criteria shipped, but the v1.1 backlog is 19 tasks long** (chain.append RPC stub, sona rerank, per-iteration cancel token, agent-side journal write, per-user agent_ids, cost circuit-breaker, EscalateToHuman, typed errors, MicroLoraRouter v3 pin, sidebar UI, MemoryConsolidator, etc.).

Smaller-but-systemic findings: **ADRs 003 / 005 / 007 / 013 / 038 describe a Tauri+React+xterm.js stack that the egui shell has effectively superseded without those ADRs being marked.** The clawft AGENT dashboard rename from `ui/` to `clawft-ui/` is incomplete in `scripts/build.sh` and `weft ui --ui-dir` defaults. Two ADR-020s and two ADR-028s share numbers. Agent-core-v1 worktrees were already cleaned up earlier in the same session as this audit.

## Workstreams (chronological by primary activity window)

Grouped by activity era; within each era ordered roughly by start. **Status** uses the legend in the frontmatter. **Completion** is the auditing subagent's best estimate; treat as directional.

### Continuous foundations (started Genesis 2026-02-17, ongoing)

| #  | Workstream                                | Status   | Period                | Completion | Doc |
|----|-------------------------------------------|----------|-----------------------|-----------:|-----|
| 01 | Core platform                             | landed   | 2026-02-17 → ongoing  |       ~90% | [01-core-platform.md](./01-core-platform.md) |
| 02 | Kernel & governance                       | partial  | 2026-02-17 → ongoing  |       ~78% | [02-kernel-governance.md](./02-kernel-governance.md) |
| 14 | Deployment & release engineering          | partial  | 2026-02-17 → ongoing  |       ~85% | [14-deployment-release.md](./14-deployment-release.md) |
| 15 | MCP integration & extension surface       | partial  | 2026-02-17 → ongoing  |       ~80% | [15-mcp-integration.md](./15-mcp-integration.md) |
| 17 | Research streams                          | active   | 2026-02-17 → ongoing  | mixed¹ | [17-research-streams.md](./17-research-streams.md) |

¹ Stream 17 is partly landed-as-feature (ECC, EML, Democritus, ExoChain) and partly active research (JEPA/LeWM); a single % understates it. See doc.

### Phase-4 sprint cluster (Mar–Apr 2026)

| #  | Workstream                       | Status   | Period               | Completion | Doc |
|----|----------------------------------|----------|----------------------|-----------:|-----|
| 03 | Pipeline & routing               | partial  | 2026-03-31 → ongoing |       ~80% | [03-pipeline-routing.md](./03-pipeline-routing.md) |
| 04 | Plugin & skills system           | partial  | 2026-04 → ongoing    |       ~75% | [04-plugin-skills.md](./04-plugin-skills.md) |
| 05 | Channels (Discord/Telegram/Slack/Gateway) | partial | 2026-04 → ongoing | ~55% | [05-channels.md](./05-channels.md) |
| 06 | Memory & workspace               | partial  | 2026-04 → ongoing    |       ~85% | [06-memory-workspace.md](./06-memory-workspace.md) |
| 07 | Multi-agent routing & delegation | partial  | 2026-04 → ongoing    |       ~40% | [07-multi-agent-routing.md](./07-multi-agent-routing.md) |

### W-tracks (planning + partial impl, Apr 2026)

| #  | Workstream                          | Status   | Period               | Completion | Doc |
|----|-------------------------------------|----------|----------------------|-----------:|-----|
| 09 | clawft AGENT dashboard (React 19)   | partial  | 2026-04 → ongoing    |       ~80% | [09-clawft-agent-dashboard.md](./09-clawft-agent-dashboard.md) |
| 10 | Voice (W-VOICE)                     | partial  | 2026-04 → ongoing    |       ~45% | [10-voice.md](./10-voice.md) |
| 16 | Browser WASM runtime (W-BROWSER)    | partial  | 2026-04 → ongoing    |       ~50% | [16-browser-wasm.md](./16-browser-wasm.md) |

### GUI / Surface (0.6.19, Apr 22 2026)

| #  | Workstream                                       | Status  | Period               | Completion | Doc |
|----|--------------------------------------------------|---------|----------------------|-----------:|-----|
| 08 | WeftOS GUI (Explorer / clawft-gui-egui / VSCode) | partial | 2026-04 → ongoing    |       ~70% | [08-weftos-gui.md](./08-weftos-gui.md) |
| 13 | App layer / substrate / surface (M1.5)           | partial | 2026-04-22 → ongoing |       ~85% | [13-app-substrate-surface.md](./13-app-substrate-surface.md) |

### Post-0.6.19 / 0.7-line (Apr 22+, 2026 — currently active)

| #  | Workstream                            | Status  | Period                  | Completion | Doc |
|----|---------------------------------------|---------|-------------------------|-----------:|-----|
| 11 | Agent-core-v1 (chat agent E2E)        | landed² | 2026-04-22 → 2026-04-28 |      ~100% | [11-agent-core-v1.md](./11-agent-core-v1.md) |
| 12 | Knowledge graph & graphify (Sprint 17)| active  | 2026-04-22 → ongoing    |       ~60% | [12-knowledge-graph-graphify.md](./12-knowledge-graph-graphify.md) |

² All 12 acceptance criteria of agent-core-v1 met; v1.1 backlog (19 items) is a follow-on stream rather than incomplete v1.

## How to use this audit

- **Per workstream**, the linked document is the source of truth: full task list with `- [ ] task — source: file:line OR plan ref`, status of every TODO/FIXME/deferred item, identified orphans.
- **For 0.7.0 scope-cut decisions**, walk top-to-bottom and pull tasks into a release-gate spreadsheet. The `risk` and `completion_pct` frontmatter fields on each per-stream doc give a first-pass triage signal.
- **For follow-on planning**, the orphans / spec-drift / ADR-supersedence findings (especially in 04, 07, 08, 13, 17) are candidates either for retiring or for adopting under a v1.1 / v0.8 plan.
- **For `Active` and `Partial` streams**, expect the per-stream task list to be fluid; the auditing subagents recorded state as of 2026-04-28.

## Frontmatter schema reference (per-workstream docs)

Each workstream document uses this YAML schema in its frontmatter:

```yaml
---
title: "<Workstream Name>"
slug: <kebab-case-id>
workstream_id: "<NN>"
status: landed | active | partial | deferred | planning | orphaned | blocked
period_start: YYYY-MM-DD
period_end: YYYY-MM-DD | null
last_updated: 2026-04-28
versions_landed: ["x.y.z", ...]
related_plans: [paths]
related_adrs: ["adr-NNN", ...]
sprint_refs: [sprint folder names]
completion_pct: 0..100
open_task_count: <integer>
risk: low | med | high
---
```

## Aggregate signal

| Signal                                              | Value     |
|-----------------------------------------------------|-----------|
| Workstreams audited                                 | 17        |
| Workstreams `landed`                                | 2 (01, 11) |
| Workstreams `partial`                               | 13        |
| Workstreams `active`                                | 2 (12, 17) |
| Workstreams `orphaned` outright                     | 0¹        |
| Open tasks (estimated, sum across stream task lists)| ~430      |
| Code-level TODO/FIXME markers (estimated, summed)   | ~50       |
| Live behavioural bugs flagged                       | 1 (Democritus loop) |
| Critical-severity governance gaps                   | 2 (auth_service.rs L325, L354) |
| ADRs flagged as superseded but not marked           | ≥5 (003, 005, 007, 013, 038) |
| ADR number collisions                               | 2 pairs (020, 028) |

¹ No *workstream* is fully orphaned, but several *sub-tracks* inside streams are: see Closure-SDK / coherence-lattice-alpha / NVIDIA-Ising / RuView in stream 17, the email/teams/whatsapp/signal/matrix/irc adapters in stream 05, the eight published-but-unconsumed plugin crates in stream 04, and the unwired `compositional-ui` / `RLM-arxiv-2512.24601` symposium outputs.

## Provenance

- Audit run on commit `cb947080` (development-0.7.0, 4 commits ahead of agent-core-v1 ship at `e2c3ecc1`).
- Each per-workstream document records its own `Sources:` section with file paths.
- This index does not introduce new claims beyond what the per-stream docs assert; aggregate numbers are sums or selections from those documents.

---

*Generated 2026-04-28 by 17 parallel audit subagents + a top-level synthesiser. Inputs: source tree, planning docs, sprint trackers, ADRs, CHANGELOG, handoff.md, kernel.log behavioural state. Treat as a snapshot for 0.7-scope decision-making — refresh before next major scope cut.*
