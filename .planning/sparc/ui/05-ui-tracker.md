# Sprint Tracker: UI Development Sprint

> **Status (2026-04-28): Superseded — implemented with deferred tracks.**
> Per-row `Not Started` markers in the tables below were never refreshed
> as work landed. The de-facto trackers are
> `.planning/development_notes/step{1..7}*.md` (per-step ship reports +
> phase gates) and the audit doc
> `.planning/reviews/0.7.0-release-gate/09-clawft-agent-dashboard.md`.
> Plane carries the open follow-up items under label
> `ws09-clawft-dashboard` (WEFT-292..321). Source location moved from
> `ui/` to `clawft-ui/` in CHANGELOG 0.6.19 (2026-04-22); the
> toolchain rename completed in the 0.7.0 release wave M1-E
> (WEFT-292/293/294/295/296/297/318/320/321). Future updates happen
> in Plane, not in this file.

**Project**: clawft
**Sprint**: UI Development -- Web Dashboard + Live Canvas
**Source**: `.planning/ui_development.md`
**Orchestrator**: `.planning/sparc/ui/00-orchestrator.md`
**Stack**: Vite + React + TypeScript + Tailwind CSS v4 (frontend, custom UI primitives instead of shadcn/ui), Axum (backend API)
**Created**: 2026-02-23
**Last shipped**: 2026-02-24 (step-7 phase gate 11/11 PASS)

---

## Milestone Status

- [x] **S1 MVP (Week 3)**: Backend API (auth, agents, sessions, tools, WS), Frontend scaffold (Vite, MSW, routing), Core views (dashboard, WebChat streaming, agent management, session explorer, tool registry), theme toggle. Cmd+K palette is a placeholder. shadcn/ui not adopted; custom primitives shipped instead.
- [x] **S2 Complete (Week 6)**: Live Canvas (CanvasCommand protocol, render_ui tool, CanvasRenderer, element types, interactions), skill browser + ClawHub, memory explorer + semantic search, config editor, cron dashboard, channel status.
- [~] **S3 Partial (Week 9)**: S3.1 delegation monitor / S3.2 advanced Canvas / S3.6 browser-WASM integration / S3.7 docs **shipped**. S3.3 (mobile responsive + PWA), S3.4 (Tauri desktop shell), and the security half of S3.5 (CSP, rate limiting, E2E, axe, Tailscale, multi-user) are **deferred** — tracked in Plane.

### MVP Verification Checklist

- [ ] Axum REST API serves /api/agents, /api/sessions, /api/tools
- [ ] Bearer token auth rejects unauthenticated requests with 401
- [ ] WebSocket /ws accepts connections with topic subscription
- [ ] `weft ui` command starts gateway and opens browser with token
- [ ] `pnpm dev` starts Vite dev server on :5173 with HMR
- [ ] `VITE_MOCK_API=true pnpm dev` runs fully without backend
- [ ] MSW mocks return realistic fixture data for agents, sessions, tools
- [ ] Dashboard displays agent count, channel status, recent sessions
- [ ] WebChat shows real-time streaming messages via WebSocket
- [ ] Agent start/stop actions update status via API
- [ ] Dark/light theme toggle persists across sessions
- [ ] Cmd+K command palette navigates to any view

---

## P Pre-Implementation (Week 0)

**SPARC Dir**: `sparc/ui`
**Purpose**: Validate architecture, define protocols, scaffold projects before sprint begins

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| P1 | Axum REST + WebSocket API layer design (endpoint list, auth model, WS protocol) | P0 | 0 | Not Started | clawft-services | Design |
| P2 | WebSocket event protocol design (server->client events, client->server commands, topic subscription) | P0 | 0 | Not Started | clawft-types | Design |
| P3 | Authentication design (token generation, Bearer middleware, one-time URL tokens, TTL config) | P0 | 0 | Not Started | clawft-services | Design |
| P4 | Frontend project structure scaffolding plan (ui/ directory layout, routes, components, stores) | P0 | 0 | Not Started | ui/ | Design |
| P5 | Component library selection + shadcn/ui component inventory (14 core components mapped to views) | P1 | 0 | Not Started | ui/ | Design |
| P6 | Performance budget definition (200 KB bundle, 1.5s FCP, 2.5s TTI, 50ms WS latency, 60fps Canvas) | P1 | 0 | Not Started | -- | Design |

**Pre-Implementation Summary**: 6 items

---

## S1.1 Backend API Foundation (Week 1)

**Sprint**: S1 -- Foundation + Core Views
**Deliverable**: Axum REST API running with auth, agent/session/tool endpoints, WebSocket handler, `weft ui` command

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S1.1.1 | Add `axum-extra` + `tower-http` to `clawft-services/Cargo.toml` | P0 | 1 | Not Started | clawft-services | Feature |
| S1.1.2 | Create `clawft-services/src/api/mod.rs` -- Axum router factory | P0 | 1 | Not Started | clawft-services | Feature |
| S1.1.3 | Implement `/api/auth/token` + Bearer middleware (token gen, validation, 401 on reject) | P0 | 1 | Not Started | clawft-services | Feature |
| S1.1.4 | Implement agent CRUD endpoints (`GET/POST/PATCH /api/agents/*`, start/stop) | P0 | 1 | Not Started | clawft-services | Feature |
| S1.1.5 | Implement session endpoints (`GET/DELETE /api/sessions/*` -- list, detail, delete) | P0 | 1 | Not Started | clawft-services | Feature |
| S1.1.6 | Implement tool listing (`GET /api/tools`, `GET /api/tools/:name/schema`) | P0 | 1 | Not Started | clawft-services | Feature |
| S1.1.7 | WebSocket upgrade handler (`/ws`) with topic subscription (subscribe/unsubscribe) | P0 | 1 | Not Started | clawft-services | Feature |
| S1.1.8 | Wire API router into existing Gateway startup (`weft gateway`) | P0 | 1 | Not Started | clawft-cli | Feature |
| S1.1.9 | CORS middleware (`tower-http::cors`) with configurable `cors_origins` | P1 | 1 | Not Started | clawft-services | Feature |
| S1.1.10 | Add `weft ui` CLI command (starts gateway + opens browser with token param) | P1 | 1 | Not Started | clawft-cli | Feature |
| S1.1.11 | Optional static file serving: `ui/dist/` from disk or `rust-embed` behind `--features ui` | P1 | 1 | Not Started | clawft-services | Feature |

**S1.1 Summary**: 11 items

---

## S1.2 Frontend Scaffolding (Week 1)

**Sprint**: S1 -- Foundation + Core Views (parallel with S1.1)
**Deliverable**: Navigable dashboard shell with sidebar, auth flow, MSW mocks. Runs standalone via `VITE_MOCK_API=true pnpm dev`

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S1.2.1 | Initialize Vite + React + TypeScript project in `ui/` with own `package.json` | P0 | 1 | Not Started | ui/ | Feature |
| S1.2.2 | Install and configure Tailwind CSS v4 | P0 | 1 | Not Started | ui/ | Feature |
| S1.2.3 | Initialize shadcn/ui (`npx shadcn@latest init`) | P0 | 1 | Not Started | ui/ | Feature |
| S1.2.4 | Install core shadcn components (Button, Card, Badge, Table, Tabs, Dialog, Sidebar, Toast) | P0 | 1 | Not Started | ui/ | Feature |
| S1.2.5 | Create `MainLayout` with collapsible sidebar navigation | P0 | 1 | Not Started | ui/src/components/layout/ | Feature |
| S1.2.6 | Create `api-client.ts` (fetch wrapper with Bearer auth token, configurable API URL) | P0 | 1 | Not Started | ui/src/lib/ | Feature |
| S1.2.7 | Create `ws-client.ts` (reconnecting WebSocket client with exponential backoff + topics) | P0 | 1 | Not Started | ui/src/lib/ | Feature |
| S1.2.8 | Create `use-auth.ts` hook (token extraction from URL param, localStorage persistence) | P1 | 1 | Not Started | ui/src/hooks/ | Feature |
| S1.2.9 | Create `types.ts` (TypeScript types mirroring Rust API types -- agents, sessions, tools, WS events) | P0 | 1 | Not Started | ui/src/lib/ | Feature |
| S1.2.10 | Set up TanStack Router for file-based type-safe routing | P0 | 1 | Not Started | ui/src/routes/ | Feature |
| S1.2.11 | MSW mock handlers for agents, sessions, tools (backend-independent dev) | P0 | 1 | Not Started | ui/src/mocks/ | Feature |
| S1.2.12 | `Dockerfile` for standalone deployment (multi-stage: Vite build -> nginx:alpine) | P2 | 1 | Not Started | ui/ | DevOps |
| S1.2.13 | `ui/.env` / `.env.mock` configuration files (VITE_API_URL, VITE_WS_URL, VITE_MOCK_API) | P1 | 1 | Not Started | ui/ | Config |

**S1.2 Summary**: 13 items

---

## S1.3 Core Views (Weeks 2-3)

**Sprint**: S1 -- Foundation + Core Views
**Deliverable**: Dashboard home, WebChat with streaming, agent management, session explorer, tool inspector, theme toggle, command palette

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S1.3.1 | Dashboard Home -- agent count, channel status, recent sessions, system health cards | P0 | 2 | Not Started | ui/src/routes/index.tsx | Feature |
| S1.3.2 | Agent Management -- list view with status badges, start/stop action buttons | P0 | 2 | Not Started | ui/src/routes/agents.tsx | Feature |
| S1.3.3 | Agent Detail -- config form, model selector, workspace path, permissions editor | P1 | 2 | Not Started | ui/src/routes/agents.$id.tsx | Feature |
| S1.3.4 | WebChat -- session list sidebar + message thread display | P0 | 2 | Not Started | ui/src/routes/chat.tsx | Feature |
| S1.3.5 | WebChat Streaming -- real-time message display via WebSocket subscription | P0 | 2 | Not Started | ui/src/routes/chat.$session.tsx | Feature |
| S1.3.6 | WebChat Input -- message composer with send button, file upload placeholder | P1 | 2 | Not Started | ui/src/components/chat/ | Feature |
| S1.3.7 | Tool Call Cards -- expandable cards showing tool name, args, result inline in chat | P1 | 3 | Not Started | ui/src/components/chat/ | Feature |
| S1.3.8 | Session Explorer -- DataTable with session keys, message counts, timestamps, search | P0 | 3 | Not Started | ui/src/routes/sessions.tsx | Feature |
| S1.3.9 | Session Detail -- full conversation history with export button | P1 | 3 | Not Started | ui/src/routes/sessions.tsx | Feature |
| S1.3.10 | Tool Registry -- list all tools with expandable JSON Schema tree viewer | P0 | 3 | Not Started | ui/src/routes/tools.tsx | Feature |
| S1.3.11 | Dark/light theme toggle (shadcn theme system, persisted to localStorage) | P1 | 3 | Not Started | ui/src/components/layout/ | Feature |
| S1.3.12 | Global command palette (Cmd+K) for navigation across all views | P1 | 3 | Not Started | ui/src/components/common/ | Feature |
| S1.3.13 | Toast notifications for WebSocket events (agent status changes, errors) | P1 | 3 | Not Started | ui/src/components/common/ | Feature |

**S1.3 Summary**: 13 items

---

## S2.1 Live Canvas (Weeks 4-5)

**Sprint**: S2 -- Advanced Views + Live Canvas
**Deliverable**: Agent renders interactive UI elements in Canvas; user interactions route back to agent as tool results

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S2.1.1 | Define `CanvasCommand` protocol (render, update, remove, reset, snapshot) in Rust + TS | P0 | 4 | Not Started | clawft-types + ui/src/lib/types.ts | Design |
| S2.1.2 | Backend: Canvas WebSocket handler (`/ws/canvas`) with command routing | P0 | 4 | Not Started | clawft-services | Feature |
| S2.1.3 | Backend: `render_ui` tool -- agent-callable tool that pushes UI elements to Canvas | P0 | 4 | Not Started | clawft-tools | Feature |
| S2.1.4 | Frontend: `CanvasRenderer` component (renders CanvasCommand stream into elements) | P0 | 4 | Not Started | ui/src/components/canvas/ | Feature |
| S2.1.5 | Canvas element types: text, button, input, image, code, chart, table, form | P0 | 5 | Not Started | ui/src/components/canvas/ | Feature |
| S2.1.6 | Canvas interaction: click/submit events routed back to agent as tool results | P0 | 5 | Not Started | ui/src/components/canvas/ | Feature |
| S2.1.7 | Canvas state persistence (snapshot/restore via REST API) | P1 | 5 | Not Started | clawft-services + ui/ | Feature |
| S2.1.8 | Split-view: Canvas + Chat side-by-side (ResizablePanel from shadcn) | P1 | 5 | Not Started | ui/src/routes/canvas.tsx | Feature |
| S2.1.9 | Canvas toolbar: zoom, pan, fullscreen, snapshot, clear buttons | P1 | 5 | Not Started | ui/src/components/canvas/ | Feature |

**S2.1 Summary**: 9 items

---

## S2.2 Skill Browser (Week 5)

**Sprint**: S2 -- Advanced Views + Live Canvas
**Deliverable**: Browse installed skills, search ClawHub, install/uninstall with progress

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S2.2.1 | Backend: Skill listing + search endpoints (`GET /api/skills`, `DELETE /api/skills/:name`) | P1 | 5 | Not Started | clawft-services | Feature |
| S2.2.2 | Backend: ClawHub proxy endpoint (`GET /api/skills/hub/search`) | P1 | 5 | Not Started | clawft-services | Feature |
| S2.2.3 | Installed skills view: card grid with name, version, description, tools provided | P1 | 5 | Not Started | ui/src/routes/skills.tsx | Feature |
| S2.2.4 | ClawHub search: search bar + results grid with install button | P1 | 5 | Not Started | ui/src/routes/skills.tsx | Feature |
| S2.2.5 | Skill detail: SKILL.md content rendered as markdown, tool list, permissions | P1 | 5 | Not Started | ui/src/components/skills/ | Feature |
| S2.2.6 | Install/uninstall actions with progress toast notification | P1 | 5 | Not Started | ui/src/components/skills/ | Feature |

**S2.2 Summary**: 6 items

---

## S2.3 Memory Explorer (Week 5)

**Sprint**: S2 -- Advanced Views + Live Canvas
**Deliverable**: Memory CRUD with semantic search, namespace filtering, write dialog

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S2.3.1 | Backend: Memory CRUD + search endpoints (`GET/POST/DELETE /api/memory/*`, `POST /api/memory/search`) | P1 | 5 | Not Started | clawft-services | Feature |
| S2.3.2 | Memory list view: DataTable with key, namespace, tags, timestamp columns | P1 | 5 | Not Started | ui/src/routes/memory.tsx | Feature |
| S2.3.3 | Memory detail view: full content with JSON/markdown rendering | P1 | 5 | Not Started | ui/src/routes/memory.tsx | Feature |
| S2.3.4 | Semantic search: search bar with threshold slider, namespace filter dropdown | P1 | 5 | Not Started | ui/src/routes/memory.tsx | Feature |
| S2.3.5 | Memory write dialog: key, value, namespace, tags, TTL input form | P1 | 5 | Not Started | ui/src/components/ | Feature |

**S2.3 Summary**: 5 items

---

## S2.4 Configuration Editor (Week 6)

**Sprint**: S2 -- Advanced Views + Live Canvas
**Deliverable**: Tabbed config viewer with inline editing, diff view, schema validation

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S2.4.1 | Backend: Config read/write endpoints with validation (`GET/PATCH /api/config`, `GET /api/config/validate`) | P1 | 6 | Not Started | clawft-services | Feature |
| S2.4.2 | Config viewer: tabbed sections (agents, providers, channels, tools, routing, delegation) | P1 | 6 | Not Started | ui/src/routes/config.tsx | Feature |
| S2.4.3 | Inline editing with JSON schema validation (react-hook-form + zod) | P1 | 6 | Not Started | ui/src/routes/config.tsx | Feature |
| S2.4.4 | Config diff view: show pending changes before save confirmation | P1 | 6 | Not Started | ui/src/components/ | Feature |
| S2.4.5 | Provider config: model selector, API key status (masked), endpoint override | P1 | 6 | Not Started | ui/src/routes/config.tsx | Feature |

**S2.4 Summary**: 5 items

---

## S2.5 Cron + Channels (Week 6)

**Sprint**: S2 -- Advanced Views + Live Canvas
**Deliverable**: Cron job management, channel status with real-time updates, routing visualization

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S2.5.1 | Backend: Cron CRUD endpoints (`GET/POST/DELETE /api/cron/*`) | P1 | 6 | Not Started | clawft-services | Feature |
| S2.5.2 | Cron dashboard: job list, expression editor, next-fire preview, execution log | P1 | 6 | Not Started | ui/src/routes/cron.tsx | Feature |
| S2.5.3 | Channel status view: connection state per channel, message counts, restart button | P1 | 6 | Not Started | ui/src/routes/channels.tsx | Feature |
| S2.5.4 | Channel routing view: visual routing table (channel -> agent mapping) | P1 | 6 | Not Started | ui/src/routes/channels.tsx | Feature |

**S2.5 Summary**: 4 items

---

## S3.1 Delegation & Monitoring (Week 7)

**Sprint**: S3 -- Polish + Advanced Features
**Deliverable**: Delegation monitor, rule editor, pipeline inspector, token usage + cost tracking

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S3.1.1 | Delegation monitor: active delegations list with target (local/Claude/Flow), status, latency | P1 | 7 | Not Started | ui/src/routes/delegation.tsx | Feature |
| S3.1.2 | Delegation rule editor: regex pattern, target selector, complexity threshold editor | P1 | 7 | Not Started | ui/src/routes/delegation.tsx | Feature |
| S3.1.3 | Pipeline stage inspector: real-time 6-stage pipeline visualization per session | P1 | 7 | Not Started | ui/src/components/ | Feature |
| S3.1.4 | Token usage dashboard: per-session, per-provider, per-model usage charts (recharts) | P1 | 7 | Not Started | ui/src/routes/delegation.tsx | Feature |
| S3.1.5 | Cost tracker: daily/weekly/monthly cost breakdown by provider | P1 | 7 | Not Started | ui/src/routes/delegation.tsx | Feature |

**S3.1 Summary**: 5 items

---

## S3.2 Advanced Canvas (Weeks 7-8)

**Sprint**: S3 -- Polish + Advanced Features
**Deliverable**: Charts, code editor, form builder, undo/redo history, export in Canvas

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S3.2.1 | Chart rendering: recharts/visx integration for Canvas chart elements | P2 | 7 | Not Started | ui/src/components/canvas/ | Feature |
| S3.2.2 | Code editor element: Monaco or CodeMirror embedded in Canvas | P2 | 7 | Not Started | ui/src/components/canvas/ | Feature |
| S3.2.3 | Form builder: agent constructs multi-field forms with validation | P2 | 8 | Not Started | ui/src/components/canvas/ | Feature |
| S3.2.4 | Canvas history: undo/redo stack, replay timeline | P2 | 8 | Not Started | ui/src/stores/canvas-store.ts | Feature |
| S3.2.5 | Canvas export: PNG screenshot, HTML export, JSON state export | P2 | 8 | Not Started | ui/src/components/canvas/ | Feature |

**S3.2 Summary**: 5 items

---

## S3.3 Mobile + PWA (Week 8)

**Sprint**: S3 -- Polish + Advanced Features
**Deliverable**: Responsive layout, mobile WebChat, PWA manifest, push notifications

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S3.3.1 | Responsive sidebar: drawer on mobile (<768px), collapsible on tablet | P2 | 8 | Not Started | ui/src/components/layout/ | Feature |
| S3.3.2 | Mobile WebChat: bottom-anchored input, swipe navigation between sessions | P2 | 8 | Not Started | ui/src/routes/chat.tsx | Feature |
| S3.3.3 | PWA manifest + service worker (offline dashboard shell, Add to Home Screen) | P2 | 8 | Not Started | ui/public/ | Feature |
| S3.3.4 | Push notification integration (service worker + WS event bridge for browser notifications) | P2 | 8 | Not Started | ui/src/lib/ | Feature |

**S3.3 Summary**: 4 items

---

## S3.4 Tauri Desktop (Weeks 8-9)

**Sprint**: S3 -- Polish + Advanced Features
**Deliverable**: Native desktop app wrapping dashboard, system tray, global hotkey, auto-start gateway

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S3.4.1 | Initialize Tauri project wrapping `ui/dist/` in native window | P2 | 8 | Not Started | ui/src-tauri/ | Feature |
| S3.4.2 | System tray icon with agent status indicator (green/yellow/red) | P2 | 8 | Not Started | ui/src-tauri/ | Feature |
| S3.4.3 | Global hotkey (Cmd+Shift+W) to toggle window visibility | P2 | 9 | Not Started | ui/src-tauri/ | Feature |
| S3.4.4 | Auto-start `weft gateway` on Tauri app launch if not already running | P2 | 9 | Not Started | ui/src-tauri/ | Feature |
| S3.4.5 | macOS Spotlight integration (quick agent query via Spotlight) | P2 | 9 | Not Started | ui/src-tauri/ | Feature |
| S3.4.6 | Linux/Windows native notification bridge (OS notification center) | P2 | 9 | Not Started | ui/src-tauri/ | Feature |

**S3.4 Summary**: 6 items

---

## S3.5 Production Hardening (Week 9)

**Sprint**: S3 -- Polish + Advanced Features
**Deliverable**: CSP, rate limiting, E2E tests, accessibility, Tailscale auth, multi-user isolation

| # | Item | Priority | Week | Status | Location | Type |
|---|------|----------|------|--------|----------|------|
| S3.5.1 | CSP headers and XSS protection on static file serving (script-src 'self', no eval) | P1 | 9 | Not Started | clawft-services | Security |
| S3.5.2 | Rate limiting on API endpoints (configurable, default 100 req/min per token) | P1 | 9 | Not Started | clawft-services | Security |
| S3.5.3 | WebSocket heartbeat (30s) + dead connection cleanup | P1 | 9 | Not Started | clawft-services | Feature |
| S3.5.4 | Error boundary components with graceful degradation fallback UI | P1 | 9 | Not Started | ui/src/components/ | Feature |
| S3.5.5 | E2E tests with Playwright (dashboard + WebChat + Canvas flows) | P1 | 9 | Not Started | ui/tests/ | Testing |
| S3.5.6 | Bundle analysis + tree-shaking optimization (vite-bundle-analyzer in CI) | P2 | 9 | Not Started | ui/ | Performance |
| S3.5.7 | Accessibility audit (axe-core) + WCAG AA compliance fixes | P1 | 9 | Not Started | ui/ | Testing |
| S3.5.8 | Tailscale auth provider (X-Tailscale-User header validation, source IP check) | P1 | 9 | Not Started | clawft-services | Security |
| S3.5.9 | Multi-user session isolation (per-user data scoping, no cross-user leakage) | P1 | 9 | Not Started | clawft-services | Security |

**S3.5 Summary**: 9 items

---

## Cross-Sprint Integration Tests

| Test | Sprints | Week | Priority | Status |
|------|---------|------|----------|--------|
| Backend API -> Frontend mock contract parity | S1.1, S1.2 | 1 | P0 | Not Started |
| WebSocket events -> UI real-time updates | S1.1, S1.3 | 3 | P0 | Not Started |
| MSW mocks -> real API endpoint compatibility | S1.2, S1.1 | 3 | P0 | Not Started |
| Canvas WS -> CanvasRenderer render pipeline | S2.1, S1.1 | 5 | P0 | Not Started |
| Canvas interaction -> agent tool result routing | S2.1, S1.3 | 5 | P0 | Not Started |
| Skill install -> tool registry update -> UI refresh | S2.2, S1.3 | 5 | P1 | Not Started |
| Memory search -> HNSW backend -> UI results display | S2.3, Main H2 | 5 | P1 | Not Started |
| Voice events -> UI voice status bar | S1.1, Voice VS1.3 | 7 | P1 | Not Started |
| Delegation events -> monitor real-time display | S3.1, Main M1 | 7 | P1 | Not Started |
| Tauri shell -> gateway lifecycle management | S3.4, S1.1 | 9 | P1 | Not Started |

Test infrastructure: `ui/tests/integration/`

---

## Dependencies on Main Sprint

| UI Task | Depends On (Main Sprint) | Status | Critical? |
|---------|--------------------------|--------|-----------|
| S1.1.6 (tool listing API) | B5 (shared tool registry builder) | Not Started | Yes -- clean API access to ToolRegistry |
| S2.1.3 (render_ui tool) | S1.1.7 (WS handler) | Not Started | Yes -- Canvas needs WS transport |
| S2.2.1 (skill listing) | C3 (Skill Loader) | Not Started | No -- MSW mocks available |
| S2.2.2 (ClawHub proxy) | K4 (ClawHub registry) | Not Started | No -- mock search results |
| S2.2.5 (skill hot-reload) | C4 (Hot-reload), C6 (MCP skill exposure) | Not Started | No -- manual refresh works |
| S2.3.1 (memory endpoints) | H1 (per-agent workspace), H2 (HNSW VectorStore) | Not Started | No -- HashEmbedder for MVP |
| S2.5.4 (channel routing viz) | L1 (agent routing table) | Not Started | No -- static route data OK |
| S3.1.1 (delegation monitor) | M1 (FlowDelegator), M2 (flow_available) | Not Started | No -- mock delegation events |
| S3.1.3 (pipeline inspector) | D5 (record actual latency) | Not Started | No -- synthetic latency data |
| S3.1.5 (cost tracker) | D6 (thread sender_id) | Not Started | No -- aggregate costs only |

---

## Sprint Summary

| Phase | Range | Items | Weeks | Key Deliverables |
|-------|-------|-------|-------|-----------------|
| P Pre-Implementation | P1-P6 | 6 | 0 | API design, WS protocol, auth design, project scaffolding plan, component inventory, perf budget |
| S1.1 Backend API | S1.1.1-S1.1.11 | 11 | 1 | Axum REST router, auth middleware, agent/session/tool endpoints, WS handler, `weft ui` CLI |
| S1.2 Frontend Scaffold | S1.2.1-S1.2.13 | 13 | 1 | Vite + React + shadcn, MSW mocks, api-client, ws-client, TanStack Router, MainLayout |
| S1.3 Core Views | S1.3.1-S1.3.13 | 13 | 2-3 | Dashboard, WebChat streaming, agent management, session explorer, tool registry, theme, Cmd+K |
| S2.1 Live Canvas | S2.1.1-S2.1.9 | 9 | 4-5 | CanvasCommand protocol, render_ui tool, CanvasRenderer, element types, interactions, split view |
| S2.2 Skill Browser | S2.2.1-S2.2.6 | 6 | 5 | Skill listing, ClawHub search, install/uninstall, SKILL.md rendering |
| S2.3 Memory Explorer | S2.3.1-S2.3.5 | 5 | 5 | Memory CRUD, semantic search, namespace filter, write dialog |
| S2.4 Config Editor | S2.4.1-S2.4.5 | 5 | 6 | Tabbed config viewer, inline editing, diff view, schema validation |
| S2.5 Cron + Channels | S2.5.1-S2.5.4 | 4 | 6 | Cron dashboard, channel status, routing visualization |
| S3.1 Delegation | S3.1.1-S3.1.5 | 5 | 7 | Delegation monitor, rule editor, pipeline inspector, token/cost charts |
| S3.2 Advanced Canvas | S3.2.1-S3.2.5 | 5 | 7-8 | Charts, code editor, form builder, undo/redo, export |
| S3.3 Mobile + PWA | S3.3.1-S3.3.4 | 4 | 8 | Responsive layout, mobile WebChat, PWA manifest, push notifications |
| S3.4 Tauri Desktop | S3.4.1-S3.4.6 | 6 | 8-9 | Native window, system tray, global hotkey, auto-start gateway, Spotlight |
| S3.5 Prod Hardening | S3.5.1-S3.5.9 | 9 | 9 | CSP, rate limiting, E2E tests, accessibility, Tailscale auth, multi-user |
| **Total** | **P + S1-S3** | **101** | **0-9** | |

### Priority Distribution

| Priority | Count | Description |
|----------|-------|-------------|
| P0 | 31 | Must-have for MVP: backend API, frontend scaffold, core views, Live Canvas protocol |
| P1 | 44 | Important for complete dashboard: skill browser, memory, config, delegation, hardening |
| P2 | 26 | Nice-to-have: advanced Canvas, mobile PWA, Tauri desktop, Dockerfile, bundle analysis |

### Cross-Sprint Integration Tests

| Scope | Tests | Priority |
|-------|-------|----------|
| S1 contract parity | 3 | P0 |
| S2 Canvas + features | 4 | P0-P1 |
| S3 cross-system | 3 | P1 |
| **Total** | **10** | |

### Exit Criteria

- [ ] All P0 items complete and verified
- [ ] All P1 items complete or explicitly deferred with justification
- [ ] `pnpm build` produces optimized `ui/dist/` under 200 KB gzipped
- [ ] `pnpm test` passes with zero failures (Vitest unit tests)
- [ ] `pnpm lint && pnpm type-check` clean (ESLint + tsc --noEmit)
- [ ] `cargo test --workspace` passes for backend API crates
- [ ] `cargo clippy --workspace -- -D warnings` clean for backend changes
- [ ] First Contentful Paint < 1.5s on production build
- [ ] Canvas renders 100 elements within 16ms (60fps target)
- [ ] WebSocket reconnects within 5s of disconnect
- [ ] All 10 cross-sprint integration tests pass
- [ ] Playwright E2E tests cover dashboard, WebChat, and Canvas flows
- [ ] axe-core audit passes with zero critical WCAG AA violations
- [ ] MSW mock layer stays in sync with real API contracts
- [ ] All documentation updated to match implementation
