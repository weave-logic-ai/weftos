# SPARC UI Element: Web Dashboard + Live Canvas

> **Status (2026-04-28): Implemented with deferred tracks.** Sprint S1
> (foundation + core views) and S2 (Live Canvas + advanced views) shipped
> in 0.6.x; S3.1 / S3.2 / S3.6 / S3.7 shipped; S3.3 (PWA / mobile),
> S3.4 (Tauri shell), and parts of S3.5 (CSP, rate-limit, E2E, a11y,
> Tailscale, multi-user) are deferred. The dashboard codebase moved
> from `ui/` to `clawft-ui/` (CHANGELOG 0.6.19, 2026-04-22) and the
> toolchain rename was completed in WEFT-292/293/294/295/296/297/318
> (0.7.0 release wave M1-E).
>
> Per-row `Status: Not Started` markers in this file and in
> `01-phase-S1-foundation-core-views.md` / `05-ui-tracker.md` are
> stale — they were never refreshed as work landed. The de-facto
> trackers are the audit doc
> (`.planning/reviews/0.7.0-release-gate/09-clawft-agent-dashboard.md`)
> plus Plane work items WEFT-292..321 under label
> `ws09-clawft-dashboard`. Future updates happen in Plane, not here.

**Workstream**: W-UI (Web Dashboard + Live Canvas)
**Timeline**: Weeks 1-10 (parallel with voice sprint)
**Status**: Implemented (S1 + S2 + most of S3); see banner above for deferred items.
**Dependencies**: B5 (shared tool registry builder), C3/C4/C6 (skill system for skill browser), H1/H2 (memory for explorer), L1 (agent routing), M1/M2 (delegation for monitor), D5/D6 (latency/cost data), W-BROWSER Phase 5 (WASM entry points for browser-only mode)
**Blocks**: K6 (native shells via Tauri), VS3 (voice UI integration -- S2 blocks VS3 canvas voice hooks)

---

## 1. Summary

Build a standalone web dashboard and Live Canvas for ClawFT. The UI is an independent application (Vite + React + TypeScript + shadcn/ui + Tailwind CSS) that operates in two modes:

1. **Axum Backend Mode** (default): Connects to any clawft gateway instance via configurable REST API URL and WebSocket transport. Full feature set including channels, cron, delegation monitoring, and multi-user auth.

2. **WASM Browser-Only Mode**: Connects directly to the `clawft-wasm` module loaded in the same browser tab. No server required. The WASM module runs `AgentLoop<BrowserPlatform>` with browser-native file system (OPFS), HTTP client (fetch API), and environment. Feature set is reduced: no channels, no cron, no delegation, no multi-user. LLM calls go directly from browser to provider APIs (Anthropic direct access, or via CORS proxy for other providers).

The UI can be developed, tested, and deployed without the Rust backend running (mock API via MSW). The backend API layer extends the existing Axum gateway with REST endpoints and WebSocket real-time transport. The dashboard covers agent management, WebChat with streaming, session explorer, tool registry, Live Canvas (agent-driven interactive UI), skill browser, memory explorer, configuration editor, delegation monitoring, Tauri desktop shell, and browser WASM integration. Optionally embeddable into the `weft` binary for single-binary distribution.

### 1.1 Dual-Mode Architecture

```
                   +---------------------+
                   |  React UI (ui/)     |
                   |  Vite + shadcn/ui   |
                   +----------+----------+
                              |
              +---------------+---------------+
              |                               |
    +---------v---------+         +-----------v-----------+
    | BackendAdapter    |         | WasmAdapter           |
    | (api-client.ts)   |         | (wasm-client.ts)      |
    |                   |         |                       |
    | REST: /api/*      |         | wasm-bindgen bridge   |
    | WS: /ws, /ws/*    |         | init(), send_message()|
    | Bearer token auth |         | on_response(callback) |
    +---------+---------+         +-----------+-----------+
              |                               |
    +---------v---------+         +-----------v-----------+
    | Axum Gateway      |         | clawft-wasm module    |
    | (clawft-services) |         | (wasm32-unknown-      |
    | Port 18789        |         |  unknown)             |
    +-------------------+         | AgentLoop<Browser-    |
                                  |  Platform>            |
                                  +-----------+-----------+
                                              |
                                  +-----------v-----------+
                                  | LLM Provider API      |
                                  | (direct or via proxy)  |
                                  +-----------------------+
```

The adapter layer is selected at startup based on:
- `VITE_BACKEND_MODE=axum` (default): Use REST/WS API client
- `VITE_BACKEND_MODE=wasm`: Load and initialize clawft-wasm module
- Auto-detect: If `VITE_API_URL` is set, use Axum; if `clawft_wasm.js` is present, use WASM

---

## 2. Phases

### Phase S1.1: Backend API Foundation (Week 1) -- P0

| Deliverable | Description | Crate |
|-------------|-------------|-------|
| Axum REST router | `clawft-services/src/api/mod.rs` -- Axum router factory with `axum-extra` + `tower-http` | clawft-services |
| Auth middleware | `/api/auth/token` + Bearer middleware; `weft ui` generates one-time token, opens browser | clawft-services |
| Agent CRUD endpoints | `GET/POST/PATCH /api/agents/*`, start/stop actions | clawft-services |
| Session endpoints | `GET/DELETE /api/sessions/*` -- list, detail, delete | clawft-services |
| Tool listing | `GET /api/tools`, `GET /api/tools/:name/schema` -- registered tools with JSON Schema | clawft-services |
| WebSocket upgrade handler | `/ws` with topic subscription (`subscribe`/`unsubscribe` commands) | clawft-services |
| `weft ui` CLI command | Starts gateway + opens browser with token param | clawft-cli |
| Static file serving | `ui/dist/` from disk or `rust-embed` behind `--features ui` | clawft-services |
| CORS middleware | `tower-http::cors` with configurable `cors_origins` (default: `localhost:5173`) | clawft-services |

### Phase S1.2: Frontend Scaffolding (Week 1, parallel with S1.1) -- P0

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Vite + React + TS project | Initialize standalone project in `ui/` with own `package.json` | ui/ |
| Tailwind CSS v4 + shadcn/ui | Full component library init with core components | ui/ |
| MSW mock layer | Mock handlers for agents, sessions, tools; fixture JSON data | ui/src/mocks/ |
| `api-client.ts` | Fetch wrapper with Bearer auth token, configurable API URL | ui/src/lib/ |
| `ws-client.ts` | Reconnecting WebSocket client with exponential backoff + topic subscription | ui/src/lib/ |
| `use-auth.ts` hook | Token extraction from URL param, localStorage persistence | ui/src/hooks/ |
| TanStack Router | File-based type-safe routing setup | ui/src/routes/ |
| `MainLayout` | Collapsible sidebar navigation shell | ui/src/components/layout/ |
| Dockerfile | Multi-stage build: Vite build -> nginx:alpine serving `dist/` | ui/ |

### Phase S1.3: Core Views (Weeks 2-3) -- P0

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Dashboard Home | Agent count, channel status, recent sessions, system health cards | ui/src/routes/index.tsx |
| Agent Management | List view with status badges, start/stop actions, config form | ui/src/routes/agents.tsx |
| WebChat + Streaming | Session list sidebar + message thread + real-time WS display | ui/src/routes/chat.tsx |
| Tool Call Cards | Expandable cards showing tool name, args, result inline in chat | ui/src/components/chat/ |
| Session Explorer | DataTable with session keys, message counts, timestamps, export | ui/src/routes/sessions.tsx |
| Tool Registry | List all tools with JSON Schema tree viewer | ui/src/routes/tools.tsx |
| Theme toggle | Dark/light mode via shadcn theme system | ui/src/components/layout/ |
| Command palette | Cmd+K global navigation (shadcn CommandPalette) | ui/src/components/common/ |

### Phase S2.1: Live Canvas (Weeks 4-5) -- P0

| Deliverable | Description | Location |
|-------------|-------------|----------|
| `CanvasCommand` protocol | render, update, remove, reset, snapshot command types | clawft-types + ui/src/lib/types.ts |
| Canvas WS handler | `/ws/canvas` backend handler with command routing | clawft-services |
| `render_ui` tool | Agent-callable tool that pushes UI elements to Canvas | clawft-tools |
| `CanvasRenderer` component | Renders CanvasCommand stream into interactive elements | ui/src/components/canvas/ |
| Element types | text, button, input, image, code, chart, table, form | ui/src/components/canvas/ |
| Interaction routing | Click/submit events routed back to agent as tool results | ui/src/components/canvas/ |
| State persistence | Canvas snapshot/restore via API | clawft-services + ui/ |
| Split view | Canvas + Chat side-by-side (ResizablePanel) | ui/src/routes/canvas.tsx |

### Phase S2.2: Skill Browser (Week 5) -- P1

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Skill listing endpoint | `GET /api/skills`, `DELETE /api/skills/:name` | clawft-services |
| ClawHub proxy | `GET /api/skills/hub/search` -- proxies to ClawHub registry | clawft-services |
| Installed skills view | Card grid with name, version, description, tools provided | ui/src/routes/skills.tsx |
| ClawHub search | Search bar + results grid with install button | ui/src/routes/skills.tsx |
| Install/uninstall actions | Install from ClawHub or local path, progress toast | ui/src/components/skills/ |

### Phase S2.3: Memory Explorer (Week 5) -- P1

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Memory CRUD endpoints | `GET/POST/DELETE /api/memory/*`, `POST /api/memory/search` | clawft-services |
| Memory list view | DataTable with key, namespace, tags, timestamp | ui/src/routes/memory.tsx |
| Semantic search | Search bar with threshold slider, namespace filter | ui/src/routes/memory.tsx |
| Memory write dialog | Key, value, namespace, tags, TTL input form | ui/src/components/ |

### Phase S2.4: Configuration Editor (Week 6) -- P1

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Config read/write endpoints | `GET/PATCH /api/config`, `GET /api/config/validate` | clawft-services |
| Tabbed config sections | Agents, providers, channels, tools, routing, delegation tabs | ui/src/routes/config.tsx |
| Inline editing | JSON schema validation with react-hook-form + zod | ui/src/routes/config.tsx |
| Diff view | Show pending changes before save confirmation | ui/src/components/ |

### Phase S2.5: Cron + Channels (Week 6) -- P1

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Cron CRUD endpoints | `GET/POST/DELETE /api/cron/*` | clawft-services |
| Cron dashboard | Job list, expression editor, next-fire preview, execution log | ui/src/routes/cron.tsx |
| Channel status view | Connection state per channel, message counts, restart button | ui/src/routes/channels.tsx |
| Channel routing visualization | Visual routing table (channel -> agent mapping) | ui/src/routes/channels.tsx |

### Phase S3.1: Delegation & Monitoring (Week 7) -- P1

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Delegation monitor | Active delegations list: target (local/Claude/Flow), status, latency | ui/src/routes/delegation.tsx |
| Rule editor | Regex pattern, target selector, complexity threshold editor | ui/src/routes/delegation.tsx |
| Pipeline inspector | Real-time 6-stage pipeline visualization per session | ui/src/components/ |
| Token usage dashboard | Per-session, per-provider, per-model usage charts (recharts) | ui/src/routes/delegation.tsx |
| Cost tracker | Daily/weekly/monthly cost breakdown by provider | ui/src/routes/delegation.tsx |

### Phase S3.2: Advanced Canvas (Weeks 7-8) -- P2

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Chart rendering | recharts/visx integration for Canvas chart elements | ui/src/components/canvas/ |
| Code editor element | Monaco or CodeMirror embedded in Canvas | ui/src/components/canvas/ |
| Form builder | Agent constructs multi-field forms with validation | ui/src/components/canvas/ |
| Canvas history | Undo/redo stack, replay timeline | ui/src/stores/canvas-store.ts |
| Canvas export | PNG screenshot, HTML export, JSON state export | ui/src/components/canvas/ |

### Phase S3.3: Mobile Responsive + PWA (Week 8) -- P2

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Responsive sidebar | Drawer on mobile, collapsible on tablet | ui/src/components/layout/ |
| Mobile WebChat | Bottom-anchored input, swipe navigation | ui/src/routes/chat.tsx |
| PWA manifest | Service worker for offline dashboard shell | ui/public/ |
| Push notifications | Service worker + WS event bridge for browser notifications | ui/src/lib/ |

### Phase S3.4: Tauri Desktop Shell (Weeks 8-9) -- P2

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Tauri project init | Tauri wraps `ui/dist/` in native window | ui/src-tauri/ |
| System tray icon | Agent status indicator in system tray | ui/src-tauri/ |
| Global hotkey | Cmd+Shift+W to toggle window visibility | ui/src-tauri/ |
| Auto-start gateway | Launch `weft gateway` on Tauri app start | ui/src-tauri/ |
| Spotlight integration | macOS Spotlight quick agent query | ui/src-tauri/ |
| Native notifications | Linux/Windows notification bridge | ui/src-tauri/ |

### Phase S3.5: Production Hardening (Week 9) -- P1

| Deliverable | Description | Location |
|-------------|-------------|----------|
| CSP headers | Content Security Policy + XSS protection on static file serving | clawft-services |
| Rate limiting | API endpoint rate limiting middleware | clawft-services |
| WS heartbeat | WebSocket heartbeat + dead connection cleanup | clawft-services |
| Error boundaries | Graceful degradation components with fallback UI | ui/src/components/ |
| E2E tests | Playwright tests for dashboard, WebChat, Canvas flows | ui/tests/ |
| Accessibility | axe-core audit + WCAG AA compliance fixes | ui/ |
| Tailscale auth | X-Tailscale-User header auth provider for remote access | clawft-services |
| Multi-user isolation | Per-user session isolation + permission scoping | clawft-services |

### Phase S3.6: Browser WASM Integration (Weeks 9-10) -- P1

| Deliverable | Description | Location |
|-------------|-------------|----------|
| Backend adapter interface | TypeScript interface abstracting Axum REST/WS vs WASM bridge | ui/src/lib/backend-adapter.ts |
| Axum adapter | Concrete adapter using api-client.ts + ws-client.ts (existing code) | ui/src/lib/adapters/axum-adapter.ts |
| WASM adapter | Concrete adapter wrapping clawft-wasm `init()`, `send_message()`, `on_response()` | ui/src/lib/adapters/wasm-adapter.ts |
| WASM loader | Async loader for clawft_wasm.js + .wasm binary with progress indicator | ui/src/lib/wasm-loader.ts |
| Config UI for browser mode | IndexedDB config storage, API key input with Web Crypto encryption, provider setup | ui/src/components/wasm/ |
| Feature detection | Runtime capability detection (OPFS, Web Crypto, wasm-bindgen) with fallback warnings | ui/src/lib/feature-detect.ts |
| Vite WASM build config | wasm-pack integration or manual WASM loading config for Vite | ui/vite.config.ts |
| Browser-mode route gating | Disable routes unavailable in WASM mode (channels, cron, delegation, multi-user) | ui/src/lib/mode-context.ts |

### Phase S3.7: Documentation + Developer Guide (Week 10) -- P2

| Deliverable | Description | Location |
|-------------|-------------|----------|
| UI developer guide | Setup, architecture, adding new routes/components, MSW patterns | docs/ui/developer-guide.md |
| API reference | REST + WS endpoint catalog with request/response examples | docs/ui/api-reference.md |
| Browser mode guide | How to build and deploy browser-only clawft UI with WASM module | docs/ui/browser-mode.md |
| Deployment guide | Docker, CDN, reverse proxy, Tauri packaging, single-binary embedding | docs/ui/deployment.md |

---

## 2.5 Internal Dependency Graph

### UI Phase Dependencies

```
S1.1 (Backend API)
  |
  +---> S1.3 (Core Views) -- needs real API endpoints (or MSW mocks)
  |       |
  |       +---> S2.1 (Live Canvas) -- extends WS transport from S1.1, builds on chat from S1.3
  |       |       |
  |       |       +---> S3.2 (Advanced Canvas) -- extends base Canvas elements
  |       |
  |       +---> S2.2 (Skill Browser) -- needs base DataTable/card patterns from S1.3
  |       +---> S2.3 (Memory Explorer) -- needs base DataTable patterns from S1.3
  |       +---> S2.4 (Config Editor) -- needs base form patterns from S1.3
  |       +---> S2.5 (Cron + Channels) -- needs base DataTable/status patterns from S1.3
  |       +---> S3.1 (Delegation Monitor) -- needs WS events + chart patterns
  |
  +---> S3.5 (Production Hardening) -- hardens backend layer from S1.1
  |
  +---> S3.4 (Tauri Desktop) -- wraps complete UI

S1.2 (Frontend Scaffolding)
  |
  +---> S1.3 (Core Views) -- needs layout shell, router, api-client, ws-client
  |
  +---> S3.3 (Mobile + PWA) -- adapts layout from S1.2
  |
  +---> S3.6 (Browser WASM Integration) -- needs adapter layer over api-client/ws-client
              |
              +---> S3.7 (Documentation) -- documents both modes
```

### External Workstream Dependencies

```
Main Sprint Workstream              UI Phase That Needs It
-------------------------------     ----------------------
B5  (shared tool registry)     ---> S1.1 (tool listing API needs ToolRegistry access)
C3  (skill loader)             ---> S2.2 (skill browser needs skill registry)
C4  (hot-reload)               ---> S2.2 (skill:reload WS events)
C6  (MCP skill exposure)       ---> S2.2 (skill tools visible in tool inspector)
H1  (per-agent workspace)      ---> S2.3 (memory API needs workspace directories)
H2  (HNSW VectorStore)         ---> S2.3 (semantic search API backend)
L1  (agent routing table)      ---> S2.5 (channel routing visualization data)
M1  (FlowDelegator)            ---> S3.1 (delegation status events)
M2  (flow_available detection) ---> S3.1 (runtime delegation target info)
D5  (record actual latency)    ---> S3.1 (pipeline inspector latency data)
D6  (thread sender_id)         ---> S3.1 (per-user cost attribution)
K4  (ClawHub registry)         ---> S2.2 (ClawHub search proxy)
```

### Cross-Workstream Dependencies

```
W-BROWSER Workstream                W-UI Phase That Needs It
-------------------------------     ----------------------
W-BROWSER Phase 5 (WASM entry  ---> S3.6 (Browser WASM Integration: wasm-adapter.ts calls
  points: init(), send_message,       init(), send_message(), on_response() from clawft-wasm)
  on_response)
W-BROWSER Phase 4 (Browser     ---> S3.6 (Config UI: browser mode config must match
  Platform: config from JS)           BrowserPlatform init() JSON schema)
W-BROWSER Phase 3 (LLM         ---> S3.6 (Provider setup UI: CORS proxy config,
  Transport in browser)               browser_direct toggle, API key encryption)

W-UI Phase That Blocks              Downstream
-------------------------------     ----------------------
S2 (Canvas + advanced views)   ---> VS3 (Voice UI integration: voice commands target
                                      Canvas elements, need stable Canvas protocol)
S3.6 (Browser WASM)            ---> W-BROWSER can test E2E with real UI (not just
                                      minimal HTML test harness from Phase 6)
S3.3 (PWA)                     ---> S3.6 (Service worker must handle WASM binary
                                      caching alongside static assets)
```

---

## 3. Exit Criteria

### S1.1 Backend API Foundation

- [ ] Axum REST router compiles and serves `/api/agents`, `/api/sessions`, `/api/tools`
- [ ] Bearer token auth middleware rejects unauthenticated requests with 401
- [ ] `/api/auth/token` generates valid JWT/opaque token with configurable TTL
- [ ] WebSocket `/ws` accepts connections, supports `subscribe`/`unsubscribe` commands
- [ ] `weft ui` command starts gateway and opens browser with token parameter
- [ ] CORS middleware allows configurable origins (default: `localhost:5173`)
- [ ] Static file serving works from `ui/dist/` on disk or via `rust-embed`

### S1.2 Frontend Scaffolding

- [ ] `pnpm dev` starts Vite dev server on `:5173` with HMR
- [ ] `VITE_MOCK_API=true pnpm dev` runs fully without any backend
- [ ] MSW mock handlers return realistic fixture data for agents, sessions, tools
- [ ] `api-client.ts` attaches Bearer token to all requests
- [ ] `ws-client.ts` reconnects with exponential backoff on disconnect
- [ ] Sidebar navigation renders all route links
- [ ] `pnpm build` produces optimized `ui/dist/` under 200 KB gzipped
- [ ] Dockerfile builds and serves via nginx:alpine

### S1.3 Core Views

- [ ] Dashboard home displays agent count, channel status, recent sessions
- [ ] Agent list shows status badges; start/stop actions update via WS
- [ ] Agent detail form allows editing config and selecting model
- [ ] WebChat displays real-time streaming messages via WebSocket
- [ ] Tool call cards expand to show tool name, arguments, and result
- [ ] Session explorer lists sessions with search, sort, and export
- [ ] Tool registry lists all tools with expandable JSON Schema viewer
- [ ] Dark/light theme toggle persists across sessions
- [ ] Cmd+K command palette navigates to any view

### S2.1 Live Canvas

- [ ] `CanvasCommand` types defined in both Rust and TypeScript
- [ ] `/ws/canvas` handler routes commands between agents and frontend
- [ ] `render_ui` tool callable by agents to push UI elements
- [ ] `CanvasRenderer` renders text, button, input, image, code, table elements
- [ ] Button clicks and form submits route back to agent as tool results
- [ ] Canvas state persists across page refreshes via snapshot API
- [ ] Split view (Canvas + Chat) works with resizable panels
- [ ] Canvas renders 100 elements within 16ms (60fps target)

### S2.2 Skill Browser

- [ ] Installed skills display in card grid with name, version, tools provided
- [ ] ClawHub search returns results via proxy endpoint
- [ ] Install action triggers skill download + hot-reload notification
- [ ] Uninstall action removes skill and updates tool registry
- [ ] SKILL.md content renders as formatted markdown

### S2.3 Memory Explorer

- [ ] Memory list displays entries with key, namespace, tags, timestamp
- [ ] Semantic search returns results ranked by similarity score
- [ ] Namespace filter restricts search to selected namespace
- [ ] Memory write dialog creates entries with key, value, namespace, tags, TTL
- [ ] Delete action removes entry with confirmation dialog

### S2.4 Configuration Editor

- [ ] Config loads current resolved configuration into tabbed view
- [ ] Inline editing validates against JSON schema
- [ ] Diff view shows pending changes before save
- [ ] Save action sends PATCH and refreshes config display
- [ ] Validation endpoint catches invalid config before save

### S2.5 Cron + Channels

- [ ] Cron job list displays expression, next fire time, last run status
- [ ] Create/delete cron jobs via form with cron expression validation
- [ ] Channel status shows connection state with real-time WS updates
- [ ] Channel routing visualization maps channels to assigned agents

### S3.1 Delegation & Monitoring

- [ ] Delegation monitor shows active delegations with target, status, latency
- [ ] Rule editor creates/edits delegation rules with regex pattern matching
- [ ] Pipeline inspector visualizes 6-stage pipeline in real-time per session
- [ ] Token usage chart displays per-session, per-provider, per-model breakdown
- [ ] Cost tracker shows daily/weekly/monthly cost by provider

### S3.2 Advanced Canvas

- [ ] Charts render via recharts/visx within Canvas elements
- [ ] Code editor element (Monaco/CodeMirror) supports syntax highlighting
- [ ] Form builder allows agents to construct multi-field validated forms
- [ ] Undo/redo stack tracks Canvas history with replay capability
- [ ] Export produces PNG screenshot, HTML, and JSON state

### S3.3 Mobile Responsive + PWA

- [ ] Sidebar collapses to drawer on screens < 768px
- [ ] WebChat input anchors to bottom on mobile with swipe navigation
- [ ] PWA manifest enables Add to Home Screen
- [ ] Push notifications fire on key WS events (agent error, task complete)

### S3.4 Tauri Desktop Shell

- [ ] Tauri app launches and loads `ui/dist/` in native window
- [ ] System tray icon shows agent status (green/yellow/red)
- [ ] Cmd+Shift+W toggles window visibility globally
- [ ] `weft gateway` auto-starts on app launch if not already running
- [ ] Native notifications bridge WS events to OS notification center

### S3.5 Production Hardening

- [ ] CSP headers block inline scripts and restrict resource origins
- [ ] API rate limiting enforces per-IP request limits
- [ ] WebSocket heartbeat detects and cleans up dead connections
- [ ] Error boundaries catch React errors with graceful fallback UI
- [ ] Playwright E2E tests cover dashboard, WebChat, and Canvas flows
- [ ] axe-core audit passes with zero critical WCAG AA violations
- [ ] Tailscale auth provider validates X-Tailscale-User headers
- [ ] Multi-user sessions are isolated (no cross-user data leakage)

### S3.6 Browser WASM Integration

- [ ] `BackendAdapter` interface abstracts Axum and WASM backends with identical method signatures
- [ ] `AxumAdapter` wraps existing api-client.ts + ws-client.ts behind the adapter interface
- [ ] `WasmAdapter` loads clawft-wasm module, calls `init(config_json)`, bridges `send_message()` / `on_response()`
- [ ] WASM module loads with progress indicator (download + compile + init phases)
- [ ] Browser-mode config UI stores config in IndexedDB, encrypts API keys with Web Crypto AES-256-GCM
- [ ] Provider setup UI supports `browser_direct` toggle (Anthropic) and `cors_proxy` URL input
- [ ] Feature detection warns users if OPFS or Web Crypto are unavailable
- [ ] `VITE_BACKEND_MODE=wasm` env var selects WASM adapter at build time
- [ ] Auto-detection works: if API URL is reachable use Axum, otherwise fall back to WASM
- [ ] Routes unavailable in WASM mode (channels, cron, delegation, multi-user) are hidden/disabled
- [ ] WebChat works end-to-end in WASM mode: user message -> WASM pipeline -> LLM API -> response
- [ ] Tool results from WASM mode display identically to Axum mode in the UI
- [ ] Service worker caches .wasm binary alongside static assets for offline PWA shell
- [ ] CSP headers in browser-only mode allow `'wasm-unsafe-eval'` for WASM execution

### S3.7 Documentation + Developer Guide

- [ ] UI developer guide covers project setup, architecture, and contribution workflow
- [ ] API reference documents all REST and WS endpoints with request/response examples
- [ ] Browser mode guide explains build, config, provider CORS setup, and deployment
- [ ] Deployment guide covers Docker, CDN, reverse proxy, Tauri, and single-binary modes

---

## 4. Security Requirements

### 4.1 Content Security Policy (CSP)

The static file serving layer MUST set CSP headers that:
- Block inline script execution (`script-src 'self'`)
- Restrict resource loading to same origin + configured CDN domains
- Disable `eval()` and dynamic code generation
- Allow WebSocket connections only to the configured backend origin

### 4.2 Cross-Site Scripting (XSS) Prevention

- All user-generated content (chat messages, memory values, tool results) MUST be rendered through React's default escaping (no `dangerouslySetInnerHTML` on untrusted input)
- Markdown rendering (SKILL.md, memory content) MUST use `rehype-sanitize` to strip dangerous HTML
- Canvas elements received from agents MUST be validated against a strict schema before rendering; arbitrary HTML/JS injection via Canvas commands is prohibited

### 4.3 CORS Configuration

- Backend CORS middleware MUST reject requests from origins not in `GatewayConfig.cors_origins`
- Default allowed origin: `http://localhost:5173` (Vite dev server only)
- Production deployments MUST configure explicit allowed origins
- Credentials (`Access-Control-Allow-Credentials`) MUST be restricted to allowed origins only

### 4.4 Authentication & Token Handling

- Auth tokens MUST NOT be stored in cookies (use `localStorage` or memory only to avoid CSRF)
- Tokens are sent exclusively via `Authorization: Bearer <token>` header
- Token TTL defaults to 24 hours; configurable in `GatewayConfig`
- `weft ui` one-time token in URL parameter MUST be consumed on first use (single-use, not replayable)
- Token stored in `~/.clawft/ui-token` MUST have `0600` file permissions
- API MUST return 401 on expired/invalid tokens without leaking token details

### 4.5 WebSocket Security

- WebSocket upgrade MUST validate Bearer token before completing handshake
- WebSocket connections MUST enforce same-origin or configured allowed origins
- Heartbeat interval (30s default) detects and terminates dead connections
- Per-connection message rate limiting prevents flooding
- Canvas commands received from server MUST be schema-validated before render
- Client-to-server commands MUST be validated against expected types (reject unknown command types)

### 4.6 API Security

- All `/api/*` routes require Bearer token authentication (except `/api/auth/token`)
- Rate limiting on all endpoints (configurable, default: 100 req/min per token)
- Request body size limits (default: 1 MB for API, 10 MB for file upload endpoints)
- Path parameters MUST be validated (UUID format for IDs, alphanumeric for names)
- Config PATCH endpoint MUST NOT allow overwriting security-sensitive fields (auth tokens, secret keys) via the UI

### 4.7 Multi-User Security (S3.5)

- Tailscale auth relies on trusted `X-Tailscale-User-*` headers; backend MUST verify these headers originate from the Tailscale proxy (not forgeable by clients)
- Per-user permission scoping prevents users from accessing other users' sessions or memory
- Audit logging records all config changes with user identity

### 4.8 Browser WASM Security (S3.6)

- API keys in browser mode MUST be encrypted with Web Crypto API (AES-256-GCM) before storage in IndexedDB; encryption key is non-extractable CryptoKey derived from user passphrase or device fingerprint
- API keys are decrypted only at runtime and passed to the WASM module via `init(config_json)`; they are never stored in plaintext in IndexedDB, localStorage, or OPFS
- CSP for browser-only mode adds `'wasm-unsafe-eval'` to `script-src` to allow WASM execution; no other unsafe policies are added
- CORS proxy URL in config MUST be validated to use HTTPS in production (HTTP allowed only for localhost development)
- The WASM module runs in the main thread or a Web Worker; no `SharedArrayBuffer` is required (avoids COOP/COEP header complexity)
- Users MUST be warned that API keys transit their browser in WASM mode: "Your API key is sent directly from your browser to the LLM provider. Use a separate API key with restricted permissions for browser usage."

---

## 5. Risks

| Risk | Likelihood | Impact | Score | Mitigation |
|------|-----------|--------|-------|------------|
| Backend API not ready for S1 frontend work | Medium | Low | **3** | UI is standalone-first. Full MSW mock layer enables frontend development and testing without any backend. Mock handlers are maintained alongside real API type contracts. |
| WebSocket protocol changes mid-sprint | Medium | Medium | **6** | Define protocol types in shared `types.ts` (frontend) and `clawft-types` (Rust). Version WS message schema from day one. Breaking changes require mock handler updates. |
| Canvas complexity explosion beyond MVP scope | High | Medium | **8** | MVP Canvas supports text, button, input, code, image, table only. Charts, Monaco editor, and form builder deferred to S3.2 (P2). Strict element type allowlist enforced in renderer. |
| External workstream dependencies (C3/H1/M1) not ready on time | Medium | Medium | **6** | Each UI phase that depends on external workstreams has MSW mock fallbacks. Skill browser can show mock data until C3 lands. Memory explorer can use HashEmbedder until H2 lands. Delegation monitor can show mock events until M1 lands. |
| Bundle size exceeds 200 KB budget | Medium | Low | **4** | Dynamic imports for Canvas, charts (recharts), code editor (Monaco). Measure with `vite-bundle-analyzer` in CI. Tree-shaking enforced via ES module imports. |
| XSS via agent-generated Canvas commands | Low | Critical | **6** | Canvas commands validated against strict schema. No raw HTML rendering. React default escaping for all text content. `rehype-sanitize` for markdown. CSP blocks inline scripts. |
| Tauri cross-platform audio/notification issues | Medium | Low | **4** | Tauri handles window management only. Voice is a separate native daemon (G5 workstream). Notifications use OS-native APIs with graceful fallback to in-app toast. |
| WebSocket connection instability on unreliable networks | Medium | Medium | **6** | Reconnecting client with exponential backoff (1s, 2s, 4s, ... 30s max). Missed events recovered via REST API polling on reconnect. Connection status indicator in UI header. |
| shadcn/ui or Tailwind breaking changes during sprint | Low | Medium | **3** | Pin all dependency versions in `pnpm-lock.yaml`. Use `components.json` lock for shadcn. Only upgrade dependencies between sprints, not during. |
| Multi-user auth bypass via header forgery (Tailscale) | Low | Critical | **5** | Backend MUST verify `X-Tailscale-User-*` headers originate from Tailscale proxy (check source IP or use Tailscale HTTPS cert verification). Disable header auth when not behind Tailscale proxy. |
| WASM binary size exceeds budget (>500KB gzipped) | Medium | Medium | **6** | Tree-shake via `wasm-opt`, audit dependencies with `twiggy`. Target <500KB gzipped. Service worker caches WASM binary to avoid re-download. |
| WASM module blocks main thread during init | Medium | Medium | **6** | Show loading spinner during WASM compile+init. Move to Web Worker if init exceeds 2 seconds. Use `WebAssembly.compileStreaming()` for parallel download+compile. |
| CORS blocks direct LLM API calls in browser mode | High | High | **9** | Anthropic supports `anthropic-dangerous-direct-browser-access` header. Other providers require CORS proxy. Config UI prominently warns about CORS and offers proxy setup instructions. |
| API key exposure in browser IndexedDB | Medium | High | **8** | Web Crypto AES-256-GCM encryption with non-extractable key. UI warns users to use restricted API keys. Browser storage is inherently less secure than server-side -- document this trade-off. |
| W-BROWSER Phase 5 not ready when S3.6 starts | Medium | Medium | **6** | S3.6 can be developed against a mock WASM adapter that returns canned responses. Real WASM integration tested once W-BROWSER delivers the entry points. |
