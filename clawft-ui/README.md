# Clawft Agent Dashboard

The Clawft Agent Dashboard is a Vite + React 19 + TypeScript SPA that talks
to the `weft` agent gateway over Axum REST + WebSocket, or — in browser-only
mode — drives the `clawft-wasm` module in-tab via a `BackendAdapter`
indirection.

It is a separate workstream from the WeftOS GUI / Explorer (see `gui/`),
which is an egui/Tauri shell for the kernel and process explorer.

## Stack

- Vite 7 + React 19 + TypeScript
- TanStack Router + TanStack Query + Zustand
- Tailwind CSS v4
- MSW for offline / standalone development
- lucide-react icons, custom UI primitives in `src/components/ui/`

## Quick start

Three modes are supported. Pick one:

### 1. Standalone with mocks (no backend)

```bash
cd clawft-ui
npm install
VITE_MOCK_API=true npm run dev
```

MSW intercepts `/api/*` and `/ws` and returns realistic fixture data
sourced from `src/mocks/`. The fastest way to iterate on the UI.

### 2. Live Axum backend

In one terminal, start the agent + gateway:

```bash
cargo run -p clawft-cli --bin weft -- ui --no-open
```

In another terminal:

```bash
cd clawft-ui
npm install
npm run dev
```

The Vite dev server proxies `/api` and `/ws` to the Axum gateway.
Configure `VITE_API_URL` / `VITE_WS_URL` in `.env` to point at a
non-default gateway.

### 3. Browser-only (WASM)

Build the browser WASM and load the dashboard with `?mode=wasm`:

```bash
scripts/build.sh browser
cd clawft-ui && npm run dev
# then visit http://localhost:5173/?mode=wasm
```

In this mode the UI loads `clawft_wasm` directly and uses the
`WasmAdapter` to drive an in-tab agent. Provider keys are persisted
encrypted (Web Crypto AES-256-GCM) in IndexedDB.

## Build

```bash
# from the workspace root
scripts/build.sh ui
# or, equivalently
cd clawft-ui && npm run build
```

The bundle is written to `clawft-ui/dist/`. `weft ui --ui-dir
./clawft-ui/dist` will serve the built bundle alongside the Axum API
on a single port.

## Configuration

Two env files are read by Vite (in priority order: `.env.local` ->
`.env.mock` (when `--mode mock` is passed) -> `.env`):

- `.env` — defaults for live-backend dev (`VITE_API_URL`, `VITE_WS_URL`,
  `VITE_BACKEND_MODE=axum`).
- `.env.mock` — flips `VITE_MOCK_API=true` so MSW handlers boot. Used
  by `npm run dev -- --mode mock`.

Both are checked into the repo as documented templates. Local overrides
go in `.env.local` (gitignored).

## Project layout

```
clawft-ui/
├── public/                  # static assets, mockServiceWorker.js
├── src/
│   ├── App.tsx              # root, mounts ModeProvider + Router
│   ├── main.tsx             # Vite entry point, MSW boot
│   ├── routes/              # 14 file-based TanStack routes
│   ├── components/          # MainLayout, ui/, chat/, canvas/, wasm/
│   ├── lib/                 # api-client, ws-client, BackendAdapter
│   ├── stores/              # 12 Zustand stores
│   └── mocks/               # MSW handlers + fixtures
├── index.html
├── package.json             # name: clawft-ui
└── vite.config.ts
```

## Integration with the daemon

`weft ui` (see `crates/clawft-cli/src/commands/ui_cmd.rs`) is the
production entry point: it forces `gateway.api_enabled = true`, opens
the browser at `http://127.0.0.1:<port>?token=...`, and — when
`--ui-dir` is passed — serves `dist/` via `tower_http::services::ServeDir`
on the same port as the API. In dev, the Vite dev server proxies to
the gateway instead.

For the full architecture (BackendAdapter contract, WS topic protocol,
auth model) see `docs/ui/`.
