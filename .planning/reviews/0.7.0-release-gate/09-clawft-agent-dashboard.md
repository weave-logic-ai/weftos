---
title: "Clawft Agent Dashboard (React 19)"
slug: clawft-agent-dashboard
workstream_id: "09"
audit_scope: comprehensive
release_target: "0.7.0"
status: implemented-with-deferred-tracks
last_updated: 2026-04-28
sources:
  - clawft-ui/
  - .planning/sparc/ui/
  - .planning/development_notes/step{1..7}-s*.md
  - .planning/development_notes/step{0..7}-phase-gate.md
  - crates/clawft-services/src/api/
  - crates/clawft-cli/src/commands/ui_cmd.rs
  - docs/ui/
  - docs/adr/adr-038-tauri-desktop-shell.md
---

# Clawft Agent Dashboard (React 19)

## General Description

This workstream is the **standalone clawft AGENT dashboard** — a Vite + React 19 + TypeScript SPA shipped from `/home/aepod/dev/clawft/clawft-ui/`. It is distinct from the WeftOS GUI / Explorer (workstream 08, `gui/` + `clawft-gui-egui`) which is an egui/Tauri shell for the kernel/process explorer. The clawft AGENT dashboard talks to the agent gateway over Axum REST + WebSocket OR (in browser-only mode) drives the `clawft-wasm` module in-tab via a `BackendAdapter` indirection.

Plan-of-record is `.planning/sparc/ui/00-orchestrator.md` (W-UI workstream, phases S1.1 -> S3.7, weeks 1-10, parallel with W-VOICE and W-BROWSER). The UI was scaffolded standalone-first with MSW mocks so the frontend can be developed without the Rust backend running.

Architecture in one paragraph: TanStack Router + TanStack Query + Zustand + Tailwind CSS v4 + lucide-react, no shadcn/ui dependency (custom UI primitives in `src/components/ui/`). A `BackendAdapter` interface (`AxumAdapter` / `WasmAdapter` / mock) is selected at startup by the `ModeProvider` based on `?mode=` URL param, `VITE_BACKEND_MODE` env var, or runtime probe of the Axum health endpoint. Browser-mode persists encrypted (Web Crypto AES-256-GCM) provider keys in IndexedDB. Backend exposes `/api/{agents,sessions,tools,skills,memory,config,cron,channels,delegation,monitoring,voice,health,auth/token}` plus `/ws` with topic subscribe/unsubscribe/ping under feature gate `api` in `clawft-services`.

## Status & Timeline

| Phase | Title | Step doc | Verified | Functional state |
|-------|-------|----------|----------|------------------|
| S1.1 | Backend API foundation (Axum REST + WS + Bearer auth) | step1-s1.1-api-scaffold.md (2026-02-24) | Step-7 phase gate | Done. 9 routers merged. |
| S1.2 | Frontend scaffolding (Vite + React 19 + TS + TanStack + MSW) | step2-s1.2-frontend-scaffold.md (2026-02-24) | Step-7 phase gate | Done. |
| S1.3 | Core views (dashboard, agents, chat, sessions, tools) | step3-s1.3-core-views.md (2026-02-24) | Step-7 phase gate | Done. |
| S2.1 | Live Canvas (CanvasCommand protocol, render_ui tool, renderer) | step4-s2.1-live-canvas.md | Step-7 phase gate | Done. render_ui still a logging stub re message bus. |
| S2.2-S2.5 | Skill browser, memory explorer, config editor, cron, channels | step5-s2.2-s2.5-advanced-views.md | Step-7 phase gate | Done. All five routes ship. |
| S3.1 | Delegation monitor + token/cost dashboards | step6-s3.1-s3.3-delegation-production.md | Step-7 phase gate | Done. Backend handlers return mock data. |
| S3.2 | Advanced Canvas (chart/code-editor/form-advanced + undo/redo) | step7-s3.2-advanced-canvas.md | Step-7 phase gate | Done. |
| S3.3 | Mobile responsive + PWA (manifest, SW, push) | (not implemented) | Not started | DEFERRED — no manifest, no SW, no responsive drawer. |
| S3.4 | Tauri desktop shell | (not implemented) | Not started | DEFERRED — no `clawft-ui/src-tauri/`. |
| S3.5 | Production hardening (CSP, rate limiting, E2E, axe, Tailscale, multi-user) | step6-s3.1-s3.3-delegation-production.md | Partial | ErrorBoundary + skeletons + health endpoint shipped. CSP / rate-limit / E2E / a11y audit / Tailscale auth / multi-user isolation NOT shipped. |
| S3.6 | Browser WASM integration (BackendAdapter, WasmAdapter, IndexedDB AES-GCM) | step7-s3.6-s3.7-wasm-integration-docs.md | Step-7 phase gate | Done in code. Real end-to-end run depends on W-BROWSER WASM bindgen entry points. |
| S3.7 | Documentation (developer-guide, api-reference, browser-mode, deployment) | step7-s3.6-s3.7-wasm-integration-docs.md | Files exist in `docs/ui/` | Done. |

Final phase-gate metrics (`.planning/development_notes/step7-phase-gate.md`, 2026-02-24): 11/11 checks pass. UI build: 1,920 modules transformed, 452.39 kB JS / 127.54 kB gzip, 40.52 kB CSS / 7.61 kB gzip, 3.05 s. Workspace tests: 2,547 passed, 0 failed.

The CHANGELOG entry that matters for this workstream is in `[0.6.19] - 2026-04-22` under "Changed": `Renamed ui/ to clawft-ui/ for workspace clarity`. The rename is incomplete in the toolchain — see orphaned work below.

## Released Features

Routes shipped (`clawft-ui/src/routes/`, 14 pages wired into `App.tsx`):

- `/` Dashboard home — agent count, channel status, recent sessions, system-health cards.
- `/agents` — card grid, status badges, start/stop mutations, 10s poll.
- `/chat` — session sidebar + thread, role-coloured messages, WS streaming (`stream_chunk` / `stream_done`), tool-call cards, auto-scroll, optimistic send.
- `/sessions` — expandable rows, JSON export, delete, 15s poll.
- `/tools` — grid/list toggle, search, JSON-Schema viewer.
- `/canvas` — live agent-driven UI: text/button/input/code/table/form + chart/code-editor/form-advanced, undo/redo (50-deep), toolbar with Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y, WS topic `canvas`.
- `/skills` — installed-skill grid + ClawHub registry search dialog (debounced 300ms), install/uninstall.
- `/memory` — DataTable, namespace + tag filters, semantic search with similarity slider, write dialog.
- `/config` — tabbed editor (general/agents/providers/channels/gateway), diff summary, save/reset. API keys exposed as `api_key_set` boolean only.
- `/cron` — DataTable, create dialog, enable/disable, run-now, delete, next-fire preview (simplified parser handling `*`, `*/N`, comma lists).
- `/channels` — card grid + WS topic `channels` for live status, routing indicator.
- `/delegation` — three-tab Active / Rules / History.
- `/monitoring` — token-usage summary, by-provider, by-session, ADR-026 tier costs, pipeline runs.
- `/voice` — push-to-talk overlay, status bar, settings panel, waveform animation.

Library / infrastructure:

- `lib/api-client.ts` — Bearer-token fetch wrapper, namespaces for all 12 endpoint groups.
- `lib/ws-client.ts` — reconnecting WebSocket with exponential backoff (1s -> 30s), pub/sub, topic subscribe/unsubscribe.
- `lib/backend-adapter.ts` + `lib/adapters/{axum,wasm}-adapter.ts` — abstracted transport.
- `lib/wasm-loader.ts` — phased loader (download/compile/init/ready/error) with progress callbacks.
- `lib/feature-detect.ts` — WebAssembly / OPFS / Web Crypto / SW / IndexedDB / fetch-streaming probes; `canRunWasmMode()`, `preferredMode()`.
- `lib/mode-{context,store}.tsx/.ts` + `lib/use-backend.ts` — `ModeProvider`, `useBackend()`, `useCapability()` (split for `react-refresh/only-export-components`).
- `components/wasm/browser-config.tsx` — provider picker (Anthropic direct, OpenAI proxy, Ollama local, LM Studio, custom), AES-256-GCM-encrypted IndexedDB key storage.
- 12 Zustand stores (`stores/{agent,canvas,channels,config,cron,delegation,memory,monitoring,skills,theme,voice}-store.ts`) + `theme-store` with localStorage persistence.
- 9 MSW handlers covering all endpoint groups (`mocks/handlers.ts` + `mocks/browser.ts` + `public/mockServiceWorker.js`).
- `ErrorBoundary` at app root, enhanced `Skeleton` / `SkeletonText` / `SkeletonCard` / `SkeletonTable`.

Backend support (`crates/clawft-services/src/api/`):

- 15 module files: `mod.rs`, `auth.rs`, `bridge.rs`, `broadcaster.rs`, `chat.rs`, `handlers.rs`, `ws.rs`, plus per-domain `agents`-style files (`channels_api.rs`, `chat.rs`, `config_api.rs`, `cron_api.rs`, `delegation.rs`, `memory_api.rs`, `monitoring.rs`, `skills.rs`, `voice_api.rs`).
- `weft ui` CLI command (`clawft-cli/src/commands/ui_cmd.rs`) — forces `gateway.api_enabled`, opens browser at `http://127.0.0.1:<port>?token=...` with `--no-open`, `--port`, `--ui-dir` flags.
- `GET /api/health` (uptime via `OnceLock<Instant>`), `POST /api/auth/token` (24 h TTL).

Documentation (`docs/ui/`): `developer-guide.md`, `api-reference.md`, `browser-mode.md`, `deployment.md`. Plus the four step docs under `.planning/development_notes/step{1..7}-s*.md` and the phase-gate doc.

## What's Left — Total Depth

### TODOs / FIXMEs (in code)

Source code is unusually clean — `grep -rn 'TODO\|FIXME\|XXX\|HACK' clawft-ui/src` returns **zero hits**. The deferred markers all live in the Rust API bridge:

1. `crates/clawft-services/src/api/handlers.rs:130` — `// TODO: Add Content-Security-Policy (CSP) middleware via tower layer.`
2. `crates/clawft-services/src/api/handlers.rs:133` — `// TODO: Add rate limiting middleware via tower::limit::RateLimitLayer or tower-governor.` Suggested per-endpoint defaults already documented in the comment (`/api/auth/token`: 5/min, `/api/delegation/*`: 60/min, `/api/monitoring/*`: 30/min).
3. `crates/clawft-services/src/api/bridge.rs:282` — `// TODO: implement skill installation via ClawHub registry` — currently returns `Err("not implemented")`.
4. `crates/clawft-services/src/api/bridge.rs:287` — `// TODO: implement skill uninstallation` — same shape.
5. `crates/clawft-services/src/api/bridge.rs:395` — `// TODO: implement memory entry deletion` — append-only files, deletion requires rewriting.
6. `crates/clawft-services/src/api/bridge.rs:467` — `// TODO: implement config persistence (deserialize, validate, write to file)` — `save_config` returns `Err("config saving not yet implemented")`.
7. `clawft-ui/src/lib/adapters/wasm-adapter.ts:205` — `// Schema introspection deferred to future WASM entry point` — `getToolSchema()` returns `null` in WASM mode.

### Mock-Data Backends (live UI, fake data)

The frontend pages render correctly, but several backend handlers return hardcoded mock fixtures rather than reading from a live subsystem. Replacing these is a follow-on task that the planning docs explicitly call out:

- `api/delegation.rs::list_active_delegations` (line 92: `// Mock data for now; will be wired to live delegation manager later.`) — three hardcoded delegations.
- `api/delegation.rs::list_delegation_rules` (line 131: `// Mock rules; will be loaded from config in production.`).
- `api/delegation.rs::*` — `upsert`, `delete`, `history` handlers all mock-shape.
- `api/monitoring.rs::token_usage` (line 99: `// Mock data; will be wired to actual metrics collector later.`) — hardcoded provider/session totals.
- `api/monitoring.rs::cost_breakdown` and `pipeline_runs` — same.
- `crates/clawft-tools/src/render_ui.rs` — per `step4-s2.1-live-canvas.md` design note 2: "The tool validates and logs commands but does not yet publish to the message bus." A real Canvas push from agent to UI requires wiring `render_ui` -> message bus -> WS broadcaster.
- `api/voice_api.rs` — voice components are documented as "pure UI stubs — real WebAudio integration deferred" (`orchestrator-log.md`).

### Deferred Phases (planned, not started)

The plan-of-record has 17 sub-phases; three are entirely unstarted:

**S3.3 Mobile Responsive + PWA** (P2):
- Responsive sidebar drawer for screens < 768 px.
- Mobile WebChat with bottom-anchored input + swipe nav.
- PWA manifest (Add to Home Screen).
- Service worker for offline shell + WASM binary caching.
- Push notifications via SW + WS event bridge.
- Verification: no `manifest.webmanifest`, no `service-worker.*`, no responsive drawer in `MainLayout.tsx`.

**S3.4 Tauri Desktop Shell** (P2):
- Tauri 2.0 init wrapping `clawft-ui/dist/` in native window.
- System tray icon with agent-status colour.
- Cmd+Shift+W global hotkey.
- Auto-start `weft gateway` on app launch.
- macOS Spotlight integration.
- Native notification bridge (Linux/Windows/macOS).
- Verification: no `clawft-ui/src-tauri/` directory. (Note: the `gui/` workstream — WeftOS Explorer, separate from this workstream — does have `gui/src-tauri/`. ADR-038 is about that.)

**S3.5 Production Hardening** — partially shipped (ErrorBoundary, enhanced Skeletons, health endpoint) but the security-critical items remain:
- CSP middleware (TODO marker at `handlers.rs:130`).
- Rate limiting middleware (TODO at `handlers.rs:133`).
- WebSocket heartbeat + dead-connection cleanup (sparc S3.5.4 not implemented).
- Playwright E2E tests (no `clawft-ui/tests/` directory; `package.json` has no `playwright`/`vitest`/`cypress` dep).
- axe-core / WCAG AA audit not run.
- Tailscale auth provider (`X-Tailscale-User-*` header validation + source-IP / cert verification).
- Multi-user session isolation (per-user data scoping).
- Bundle analysis (vite-bundle-analyzer in CI).
- Dockerfile (`clawft-ui/Dockerfile` missing — multi-stage Vite build -> nginx:alpine — listed as S1.2.12 / S3.5 deployment artefact).
- `.env` / `.env.mock` files missing (S1.2.13).

### Open Questions

1. **Toolchain says `ui/` but code lives in `clawft-ui/`** — `scripts/build.sh::cmd_ui()` (line 195-213) checks `[ -d "$ROOT/ui" ] && [ -f "$ROOT/ui/package.json" ]` and runs `cd "$ROOT/ui" && npm run build`. That path no longer has `package.json` (only stale `dist/` from 2026-02-27 and `node_modules/`). The cmd silently `skip`s. The `weft ui` CLI default also references `--ui-dir ./ui/dist` (`crates/clawft-cli/src/commands/ui_cmd.rs:122`, `help_text.rs:121`). This is the highest-priority orphaned work item.
2. **Stale `ui/dist/` on disk** is the build output from before the rename (timestamp 2026-02-27, 459 KB JS). It can mislead `weft ui` into serving an older bundle than `clawft-ui` would produce today.
3. **`clawft-ui/package.json` still has `"name": "ui"`** — cosmetic but inconsistent with the rename.
4. **Step 7 phase-gate doc says `cd ui && npm run build` PASSED** with 1,920 modules — this was before the 2026-04-22 rename. Re-running the gate today against `clawft-ui/` would still pass, but the doc is now stale on the path.
5. **shadcn/ui never adopted** — the plan and tracker repeatedly say "shadcn/ui + Tailwind" (P0 items S1.2.3, S1.2.4 list 19 shadcn components to install). Implementation chose to roll custom primitives in `components/ui/` (button, card, badge, skeleton, separator, tooltip, dialog) using only Tailwind + clsx. This is a fine outcome but the planning artefacts still describe the unrealised shadcn path.
6. **No `use-auth.ts` hook** (S1.2.8). Token handling is inlined in `api-client.ts` against `localStorage["clawft-token"]`. The "one-time URL token" semantics from the SPARC plan (single-use, consumed on first read) are not enforced in the visible code paths.
7. **Cmd+K command palette is a placeholder** — `step3-s1.3-core-views.md` notes it as a placeholder ("Ctrl+K opens, Escape closes"); a real palette / fuzzy-finder is not implemented.
8. **WebSocket `/ws/canvas` and `/ws/chat/:session` routes** described in the SPARC docs are not present as separate routes; the canvas + chat pages share the global `/ws` socket and topic-subscribe to `canvas` / chat events.
9. **CORS proxy validation** (S3.6 security req: HTTPS in production, HTTP only for localhost) is not visibly enforced in `browser-config.tsx`.
10. **Where does the workstream live for the 0.7.0 ship?** The orchestrator log is the most recent ground truth (Session 2026-02-25): "Step 7 phase gate: 11/11 PASS". Subsequent 0.6.x point releases (0.6.16 -> 0.6.19) were rolled forward from `development-0.7.0` but did not touch this workstream's deferred items.

### Orphaned / Stale Work

- `ui/` (2026-02-27 dist + node_modules with no package.json) — stale legacy artefact from before the rename. Build script still references it.
- `scripts/build.sh::cmd_ui` — references `$ROOT/ui` not `$ROOT/clawft-ui`. Will silently `skip` until updated.
- `crates/clawft-cli/src/commands/ui_cmd.rs:122` and `help_text.rs:121` — `--ui-dir ./ui/dist` default path needs to be `./clawft-ui/dist`.
- `ui_cmd.rs:127` test expects `Some("./ui/dist")` — will need updating in the same PR.
- `clawft-ui/README.md` — still the literal Vite template README ("React + TypeScript + Vite", links to `@vitejs/plugin-react`, talks about React Compiler not being enabled). No project-specific content.
- `clawft-ui/package.json` `"name": "ui"`, `"version": "0.0.0"` — should align with workspace versioning conventions.
- `clawft-ui/index.html` `<title>ui</title>` — should be "Clawft Agent Dashboard" or similar.
- `.planning/sparc/ui/05-ui-tracker.md` — every row is `Status: Not Started`. Tracker was never updated as work landed; the step docs and orchestrator log are the de-facto trackers.
- `.planning/sparc/ui/01-phase-S1-foundation-core-views.md` lines 3053-3087 — every S1.x task row carries `Status: TODO`; same problem.
- Pre-implementation tasks P1-P6 in the tracker (API design, WS protocol design, auth design, performance budget) — all `Not Started`; in practice the design landed inline with the step docs.

### Cross-Workstream Dependencies (open)

| Dep | Source workstream | UI consumer | State |
|-----|-------------------|-------------|-------|
| WASM `init() / send_message() / on_response()` entry points | W-BROWSER Phase 5 / 16 | `WasmAdapter` real end-to-end run | Code path exists, real validation depends on W-BROWSER ship. |
| FlowDelegator real events | M1/M2 | `delegation.rs` handlers | UI ready, backend mocked. |
| Per-session latency (D5) | Main sprint D5 | Pipeline inspector / monitoring.rs | UI ready, backend mocked. |
| Cost attribution by sender_id (D6) | Main sprint D6 | Cost tracker / monitoring.rs | UI ready, backend mocked. |
| Skill loader / hot-reload (C3/C4) | Main sprint C-stream | `skills.rs` + UI | UI ready, install/uninstall return "not implemented". |
| ClawHub registry (K4) | Main sprint K4 | `skills.rs::hub_search` | Mock results only. |
| Memory store HNSW (H2) | Main sprint H2 | `memory_api.rs` semantic search | Wired through MemoryStore but `delete()` is a no-op. |
| Channels routing table (L1) | Main sprint L1 | `channels.rs` + UI | UI ready, static data. |
| Voice WebAudio (VS1.x) | W-VOICE | `/voice` route | UI is stub-only by design ("real WebAudio integration deferred"). |

### ADR Coverage

The ADRs the audit prompt cites are mostly **about the WeftOS GUI / Explorer (workstream 08)**, not this dashboard:

- ADR-003 codemirror, ADR-005 xterm-js, ADR-006 custom-block-renderer, ADR-013 json-block-descriptor, ADR-015 three-property-web — block-renderer architecture for the WeftOS explorer in `gui/`. None are referenced from `clawft-ui/`. The clawft AGENT dashboard's nearest analogue is the Live Canvas + `CanvasCommand` protocol in `crates/clawft-types/src/canvas.rs`, which evolved independently.
- ADR-007 Zustand + Tauri events — applies to `gui/` (WeftOS GUI). The agent dashboard does use Zustand (12 stores) but talks to the Axum gateway via fetch + WS, not Tauri events.
- ADR-016 multi-target theming — partially applicable: the dashboard ships a dark/light theme toggle persisted to localStorage; multi-target theming has not been formally adopted.
- ADR-038 Tauri 2.0 desktop shell — applies to `gui/src-tauri/` (WeftOS GUI). The agent dashboard's S3.4 Tauri shell would be a separate `clawft-ui/src-tauri/` and is deferred.

There is no ADR specifically scoped to this agent dashboard (auth model, BackendAdapter contract, MSW-first development pattern, browser/axum dual mode). That is a documentation gap.

## Task List

Ordered by impact for any subsequent ship. Items are observed work, not prescribed scope.

1. **[Tooling, blocker for `weft ui`]** Update `scripts/build.sh::cmd_ui` to reference `$ROOT/clawft-ui` instead of `$ROOT/ui`. Update DRY-RUN print line at 202 / 407.
2. **[Tooling]** Update `crates/clawft-cli/src/commands/ui_cmd.rs:122` and `help_text.rs:119-122` to `--ui-dir ./clawft-ui/dist`. Update the matching test at line 127.
3. **[Hygiene]** Delete or symlink the stale `ui/` directory (2026-02-27 dist + node_modules without package.json). Decide whether to keep `ui/` as a forwarding alias or remove entirely.
4. **[Hygiene]** `clawft-ui/package.json` rename `"name": "ui"` -> `"name": "clawft-ui"`; bump version off `0.0.0`.
5. **[Hygiene]** `clawft-ui/index.html` set `<title>` to a real product name.
6. **[Hygiene]** Replace `clawft-ui/README.md` (currently the literal Vite template) with project-specific content covering local dev (MSW), Axum-mode dev, browser-mode dev, build, and deployment.
7. **[Backend, S3.5 security]** Implement CSP middleware (`handlers.rs:130` TODO). Plan-of-record specifies `script-src 'self'`, no inline scripts, restricted resource origins, WebSocket origin allowlist, `'wasm-unsafe-eval'` only in browser-only mode.
8. **[Backend, S3.5 security]** Implement rate-limiting middleware (`handlers.rs:133` TODO). Per-endpoint defaults already drafted in the comment.
9. **[Backend, S3.5]** WebSocket heartbeat + dead-connection cleanup (S3.5.4 — sparc 03 phase doc).
10. **[Backend bridge]** Wire skill install/uninstall (`bridge.rs:282/287`) into the real Skill loader once C3/K4 land.
11. **[Backend bridge]** Implement memory `delete()` (`bridge.rs:395`) — note files are append-only, deletion needs rewriting.
12. **[Backend bridge]** Implement `save_config` persistence (`bridge.rs:467`) with validation + atomic write.
13. **[Backend]** Replace mock data in `delegation.rs` (4 handlers) with live FlowDelegator subscription once M1/M2 land.
14. **[Backend]** Replace mock data in `monitoring.rs` (`token_usage`, `cost_breakdown`, `pipeline_runs`) with live metrics collector + sender_id thread (D5/D6).
15. **[Backend]** Wire `render_ui` tool (`crates/clawft-tools/src/render_ui.rs`) to the message bus -> WS broadcaster so agents can actually push to the canvas.
16. **[Frontend, WASM]** Schema introspection in `wasm-adapter.ts:205` once W-BROWSER exposes it.
17. **[Frontend]** Replace the placeholder Cmd+K with a real command palette (S1.3.12).
18. **[Frontend]** Add real `use-auth.ts` enforcing single-use URL-token consumption (S1.2.8).
19. **[Frontend, S3.6 security]** Validate `cors_proxy` URL is HTTPS in production (`browser-config.tsx`).
20. **[Frontend, S3.3]** PWA manifest + service worker (offline shell + WASM caching) + push notifications.
21. **[Frontend, S3.3]** Mobile responsive sidebar drawer + bottom-anchored chat input + swipe nav.
22. **[Frontend, S3.4]** Scaffold `clawft-ui/src-tauri/` (Tauri 2.0 wrapping `clawft-ui/dist/`); system tray, global hotkey Cmd+Shift+W, auto-start gateway, native notifications. Reuse the `gui/src-tauri/` learnings, but keep a separate package.
23. **[Frontend, S3.5]** Playwright E2E suite (`clawft-ui/tests/`) covering dashboard, WebChat streaming, Canvas command flow, browser-mode bootstrap.
24. **[Frontend, S3.5]** axe-core / WCAG AA audit + fixes; vite-bundle-analyzer in CI; bundle currently 452 KB JS / 128 KB gzip — target was <200 KB gzip per S1.2 exit criteria.
25. **[Backend, S3.5]** Tailscale auth provider (`X-Tailscale-User-*` header validation + source-IP or cert check) + per-user session isolation.
26. **[Deployment]** Multi-stage Dockerfile (Vite -> nginx:alpine) — S1.2.12 / S3.5 deployment artefact, never written.
27. **[Config]** `.env` / `.env.mock` files documenting `VITE_API_URL`, `VITE_WS_URL`, `VITE_MOCK_API`, `VITE_BACKEND_MODE` (S1.2.13).
28. **[Docs]** Author an ADR for the agent dashboard's BackendAdapter contract (axum/wasm/mock indirection) — currently undocumented at the ADR level.
29. **[Docs]** Update `.planning/sparc/ui/05-ui-tracker.md` and `.planning/sparc/ui/01-phase-S1-foundation-core-views.md` so each row reflects observed status (most are still `Not Started` / `TODO` despite shipping).
30. **[Docs]** Note the `ui/` -> `clawft-ui/` rename in the step-7 phase-gate doc, and re-run the gate from `clawft-ui/` to confirm continued 11/11.

## Sources

- `clawft-ui/package.json`, `clawft-ui/vite.config.ts`, `clawft-ui/eslint.config.js`, `clawft-ui/index.html`, `clawft-ui/README.md`
- `clawft-ui/src/App.tsx`, 14 routes under `clawft-ui/src/routes/`, 12 stores under `clawft-ui/src/stores/`, lib + adapters + components (~10,623 source LOC across 69 files; 5,450 LOC in `lib/` + `routes/` + top-level `tsx`)
- `crates/clawft-services/src/api/{mod,handlers,bridge,broadcaster,auth,ws,chat,delegation,monitoring,memory_api,config_api,cron_api,channels_api,skills,voice_api}.rs`
- `crates/clawft-cli/src/commands/ui_cmd.rs`, `crates/clawft-cli/src/help_text.rs`
- `crates/clawft-tools/src/render_ui.rs`, `crates/clawft-types/src/canvas.rs`
- `.planning/sparc/ui/00-orchestrator.md`, `01-phase-S1-foundation-core-views.md`, `02-phase-S2-canvas-advanced-views.md`, `03-phase-S3-polish-production.md`, `04-ui-pre-implementation.md`, `05-ui-tracker.md`, `06-ui-security-review.md`, `07-axum-api-layer.md`
- `.planning/development_notes/step1-s1.1-api-scaffold.md`
- `.planning/development_notes/step2-s1.2-frontend-scaffold.md`
- `.planning/development_notes/step3-s1.3-core-views.md`
- `.planning/development_notes/step4-s2.1-live-canvas.md`
- `.planning/development_notes/step5-s2.2-s2.5-advanced-views.md`
- `.planning/development_notes/step6-s3.1-s3.3-delegation-production.md`
- `.planning/development_notes/step7-s3.2-advanced-canvas.md`
- `.planning/development_notes/step7-s3.6-s3.7-wasm-integration-docs.md`
- `.planning/development_notes/step7-phase-gate.md` (11/11 pass, 1,920 modules, 452.39 kB / 127.54 kB gzip)
- `.planning/development_notes/orchestrator-log.md` (Session 2026-02-25 summary)
- `scripts/build.sh` (lines 195-213, 264-266 — references `$ROOT/ui`)
- `CHANGELOG.md` line 780 (`Renamed ui/ to clawft-ui/ for workspace clarity`, 0.6.19, 2026-04-22)
- `docs/ui/{developer-guide,api-reference,browser-mode,deployment}.md`
- `docs/adr/adr-007-zustand-tauri-events.md`, `docs/adr/adr-016-multi-target-theming.md`, `docs/adr/adr-038-tauri-desktop-shell.md` (these scope WeftOS GUI / `gui/`, not the agent dashboard, and are noted for cross-reference only)
- `ui/dist/index.html` (stale 2026-02-27 build artefact)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws09-clawft-dashboard` label.

- **Range**: WEFT-292 … WEFT-321 (30 items)
- **Per cycle**: 0.7.x: 12, 0.8.x: 15, 0.9.x: 3
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
