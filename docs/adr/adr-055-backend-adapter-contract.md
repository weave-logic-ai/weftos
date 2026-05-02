# ADR-055: BackendAdapter Contract for the Agent Dashboard

**Date**: 2026-04-30
**Status**: Accepted
**Deciders**: clawft-ui maintainers, 0.7.0 release-gate audit (workstream 09)
**Source**: `.planning/reviews/0.7.0-release-gate/09-clawft-agent-dashboard.md` (task #28, ADR Coverage gap), WEFT-319

## Context

The clawft agent dashboard at `clawft-ui/` runs in three different
backend modes — server-attached (`axum`), browser-only
(`wasm`), and developer (`mock`) — and the auth, persistence,
and capability story is materially different in each. Existing ADRs
(003 / 005 / 006 / 007 / 013 / 015 / 016 / 038) describe the WeftOS
GUI / Explorer (workstream 08), not this dashboard. The 0.7.0
release-gate audit explicitly flagged this as a documentation gap.

The questions that have been answered de-facto in code, but never
written down, are:

1. **What is the contract** between React components and the
   underlying transport (HTTP / WebSocket / wasm-bindgen)?
2. **How is the mode selected** at runtime?
3. **How does the dashboard authenticate** against an axum
   gateway, and what is the path forward for multi-user (Tailscale)?
4. **Where do API keys live** in browser-only mode, and what are the
   threat-model assumptions?
5. **How does mock-first development** work without a live backend?

## Decision

### 1. `BackendAdapter` is the single integration seam

All React hooks, stores, and components talk to the backend through
the `BackendAdapter` interface defined in
`clawft-ui/src/lib/backend-adapter.ts`. Direct use of `api-client.ts`
or `ws-client.ts` from feature code is discouraged; new modules
should accept an adapter via the `useMode()` hook.

`BackendAdapter` exposes:

- `mode: "axum" | "wasm" | "mock"` — current backend.
- `capabilities: BackendCapabilities` — fine-grained feature flags
  (`channels`, `cron`, `delegation`, `multiUser`, `skillInstall`,
  `realtime`, `monitoring`, `ready`). UI affordances are gated on
  capabilities, not on mode.
- Domain methods: `listAgents`, `listSessions`, `sendMessage`,
  `searchMemory`, etc. Each adapter implements these against its
  own transport.
- Optional event hooks: `subscribe`, `unsubscribe`, `onEvent` (no-op
  in `wasm` mode without WebSockets).

Two implementations ship today:

- **`AxumAdapter`** (`adapters/axum-adapter.ts`) wraps the
  `api-client.ts` fetch helpers and `ws-client.ts` reconnecting
  WebSocket against `crates/clawft-services/src/api/`. Full
  capability set.
- **`WasmAdapter`** (`adapters/wasm-adapter.ts`) loads
  `clawft_wasm.js` (the `clawft-wasm` crate compiled for the
  browser) and routes domain calls through wasm-bindgen.
  `channels` / `cron` / `delegation` / `multiUser` /
  `skillInstall` / `monitoring` are intentionally `false` —
  these features require a server.

A third (`MockAdapter`) is implied by the `mock` mode value but is
implemented today via MSW handlers (see point 5).

### 2. Mode selection: URL → env → default → runtime probe

`ModeProvider` (`lib/mode-context.tsx`) resolves the active mode in
this order:

1. **URL search param** `?mode=wasm|axum|mock|auto` — beats
   everything; lets the same deployed bundle switch shape per
   visit (`/?mode=wasm` for browser-only demos).
2. **Build-time env** `VITE_BACKEND_MODE` — baked in by Vite for
   dedicated deployments.
3. **Default** `axum` — matches the most common
   `weft ui` workflow.
4. **Runtime probe** when the resolved mode is `auto`: hit
   `${apiUrl}/api/health`. On success, construct an `AxumAdapter`;
   on failure, fall back to `WasmAdapter`. This is the only mode
   that performs a network round-trip before adapter construction.

### 3. Auth model: Bearer today, Tailscale tomorrow

The clawft gateway (axum) hands out a 24h UUID-v4 bearer token via
`POST /api/auth/token`. `weft ui` opens the browser at
`https://<host>/?token=<uuid>` so the user does not have to copy
anything. The dashboard handles the token via
`clawft-ui/src/lib/use-auth.ts` (WEFT-309):

- `consumeUrlToken()` reads `?token=` once on first paint, persists
  it to `localStorage["clawft-token"]`, and immediately strips the
  param via `history.replaceState`. **Single-use**; reload, share,
  and screenshot of the URL all leak nothing.
- `useAuth()` exposes `{ token, ready, setToken, logout }` to React.
- `logout()` clears `localStorage` *and* sets a per-tab
  `sessionStorage["clawft-logged-out"]` latch so a stale `?token=`
  left in the address bar (e.g. browser back button) cannot
  silently re-auth.
- `api-client.ts` reads / writes the token via the shared helpers
  so direct callers and the React hook stay in sync.

The Tailscale provider tracked in WEFT-316 (deferred to 1.0.x) is
expected to plug in as a server-side `AuthProvider` abstraction
without changing the client surface — `useAuth()` will continue to
expose `{ token, ready, setToken, logout }`. A scope-token field may
be added to support per-user data isolation; that is a non-breaking
extension.

### 4. Browser-only mode: encrypted IndexedDB key storage

When `mode === "wasm"`, the user types a provider API key into
`components/wasm/browser-config.tsx`. We **never** persist the raw
key. Instead:

- A non-extractable `AES-GCM` `CryptoKey` is generated via
  `crypto.subtle.generateKey()` and stored in IndexedDB
  (`clawft-config / crypto-keys`). The key cannot be exported, so a
  malicious extension that gets `IDBObjectStore` access still cannot
  exfiltrate it.
- The provider key is encrypted with the `CryptoKey` (12-byte IV +
  ciphertext, base64) and stored under
  `clawft-config / config / current`.
- The `cors_proxy` URL the user supplies for providers without
  browser CORS support is validated by `lib/url-validator.ts`
  (WEFT-310): HTTPS is always allowed, HTTP is allowed only for
  loopback hosts (`localhost`, `127.0.0.1`, `::1`). The validator
  is a pure function and is reused by future Tauri / Explorer
  config screens.

### 5. Mock-first development: MSW for all 9 endpoint groups

`src/mocks/handlers.ts` provides MSW (Mock Service Worker) handlers
for every endpoint group the dashboard talks to. Boot is opt-in:
`main.tsx` only starts MSW when `import.meta.env.VITE_MOCK_API ===
"true"`, so a developer can iterate without a running gateway and a
production build never includes the worker.

The Playwright E2E suite (WEFT-314) runs against MSW for fast,
deterministic CI — no `weft gateway` process required. A second CI
job that runs against a real gateway is a follow-up.

## Consequences

### Positive

- **One seam** for backend swaps. Adding a desktop adapter (Tauri
  IPC) or an SSE-only adapter does not require feature-code changes.
- **Capability flags decouple UI affordances from mode**. A future
  Axum build that disables `channels` (e.g. headless deploy) will
  hide the channels nav without per-mode `if` branches in
  components.
- **Auth is testable in isolation**. `useAuth()` is a pure React
  hook with deterministic side-effects; the Playwright suite can
  exercise the URL-token flow without mocking the HTTP layer.
- **Browser-only mode never sees the server's API key**. The threat
  model assumes a hostile host page cannot export the encryption
  key; that property holds today.
- **MSW-first dev** lets the UI and gateway evolve in parallel
  without lockstep deployments.

### Negative

- **WASM adapter capabilities lag the Axum adapter** by definition
  (no server-side scheduler, no multi-user). This is acceptable for
  0.7.x but will need a story (likely an OPFS-backed cron worker)
  before browser-only mode is positioned as production-grade.
- **Three modes mean three test matrices**. Today only Axum is
  exercised end-to-end; WASM-mode E2E is on the WEFT-314 follow-up
  list.
- **`?mode=` in the URL** can be confusing if a user shares a link
  — they may not realize they're flipping their friend into
  browser-only mode. Documented in
  [browser-mode.md](../ui/browser-mode.md); a hardened deployment
  can drop the URL override by setting `VITE_BACKEND_MODE`.

### Neutral

- Adding a new endpoint requires touching `BackendAdapter`, both
  adapter implementations, and the MSW handler set. This is
  intentional — the contract is the documentation.

## References

- `clawft-ui/src/lib/backend-adapter.ts` — the interface itself.
- `clawft-ui/src/lib/mode-context.tsx` — runtime selection.
- `clawft-ui/src/lib/use-auth.ts` — auth lifecycle (WEFT-309).
- `clawft-ui/src/lib/url-validator.ts` — `cors_proxy` HTTPS rule
  (WEFT-310).
- `clawft-ui/src/components/wasm/browser-config.tsx` — encrypted
  IndexedDB key storage.
- `crates/clawft-services/src/api/auth.rs` — server-side
  `TokenStore`.
- [docs/ui/api-reference.md](../ui/api-reference.md) — endpoint
  catalogue and the client-side token lifecycle.
- [docs/ui/browser-mode.md](../ui/browser-mode.md) — provider setup
  and CORS.
- [docs/ui/developer-guide.md](../ui/developer-guide.md) — UI
  developer onboarding.
- CHANGELOG 0.6.19 — the `ui/` → `clawft-ui/` rename that produced
  the current layout.
- ADR-038 — Tauri shell history (superseded by egui for the
  Explorer; a separate Tauri shell for *this* dashboard is tracked
  in WEFT-313).
- WEFT-316 — Tailscale auth provider (deferred to 1.0.x).
