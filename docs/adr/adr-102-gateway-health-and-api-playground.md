# ADR-102: Gateway health detail, daemon-issued tokens, and the API playground

- **Status**: Partially implemented (decisions D1–D5 set by the user 2026-09-29). Done: D1 tiered `/api/health` and `/status` removal, D3 gateway validation through the daemon, D5 mint route and `TokenStore` removed, the non-loopback TLS guard (cards 01-05, 09 in part). Also done: `/mcp` mounted in the gateway (card 06, POST only, no SSE). Not done: the `/playground` page (card 08), upstream MCP servers and bind-address/client-count fields in `/api/health`, `weft mcp-server --issue-token` change.
- **Date**: 2026-09-29
- **Deciders**: Platform / ops
- **Depends-On**: ADR-022 (mandatory ExoChain audit), ADR-075 (Grok ↔ WeftOS MCP bridge, session capability tokens), ADR-076 (MCP tool surface and profiles)
- **Relates-To**: WEFT-122 (gateway kernel facade stub), WEFT-570 (server-side revoke), WEFT-102 (token sweep), WEFT-697 (MCP session capabilities)
- **Amends**: none. Replaces the gateway's own token store (`crates/clawft-services/src/api/auth.rs`) as the authority for gateway bearer tokens.

## Context

We want a web page, served by WeftOS itself, that shows health and API status and lets an operator try the REST and MCP APIs in the browser, the way the Cognitum Seed playground (`https://seed.cognitum.one/`) does. Use of the API from that page must need a short-lived token that WeftOS issues.

### What exists today (verified in source, 2026-09-29)

| Piece | Where | State |
|---|---|---|
| REST + WS gateway | `weft gateway` / `weft ui`, axum, default port 18789 (`crates/clawft-cli/src/commands/gateway.rs:287-310`) | Works. Serves static files. |
| Gateway health | `GET /api/health` (`crates/clawft-services/src/api/handlers.rs:161`) | Public; returns `status`, `version`, `uptime_secs` only. |
| Gateway kernel routes | `/status`, `/processes`, `/services`, `/chain/*`, `/ecc/*`, `/vectors/*` via `rpc_get`/`rpc_post` (`http_facade_api.rs:172-183`) | Backed by `InMemoryKernelFacade`, a **stub** (`gateway.rs:831-833`). WEFT-122 (done) wired the handlers to the facade types but never gave them a daemon backend, so these routes do not reach the daemon. |
| Gateway tokens | `TokenStore`, 24 h TTL (`api/auth.rs:16-61`, `handlers.rs:112-120`) | `POST /api/auth/token` is on `PUBLIC_PATHS` (`auth.rs:139-145`): any caller that can reach the port can mint a token. Safe only while bound to loopback. |
| MCP HTTP server | `weft mcp-server --listen` (`crates/clawft-services/src/mcp/http_serve.rs:100-155`), separate process, port 8742 | Works: `/health` public, `/mcp` needs `Bearer` (checked 2026-09-29: no token → 401, token → `initialize` and `tools/call` succeed). |
| MCP tokens | `SessionTokenStore` with scopes, TTL, client label, enterprise ceiling (`mcp/session_cap.rs:279-407`) | Good model, but `weft mcp-server --issue-token` mints into a throwaway store (`commands/mcp_server.rs:402-430`); a running server cannot be issued a token. |
| Chain append | daemon RPC `chain.append` → `ChainManager::append` (`crates/clawft-weave/src/daemon.rs:6188-6220`, `crates/clawft-kernel/src/chain.rs:1197`); idempotent variant exists (WEFT-103) | Works. |

So there are two token stores, neither on the chain, and the gateway's view of the kernel is a stub.

## Decisions

### D1. Status lives on `/health`, tiered by token

One route, `GET /api/health` (and `/health` nest-relative), stays public.

- **No token, or invalid token**: health only. `{"status":"ok"}` with HTTP 200, or `{"status":"degraded"}` / `"down"` with HTTP 503. No version, no component names. This keeps the endpoint usable by load balancers and uptime checks without disclosing anything.
- **Valid bearer token**: the same route returns the full status document:
  - build: version, git commit, dirty flag, build time, binary path
  - gateway: uptime, bind address, API/WS/SSE client counts
  - daemon: reachable, pid, version, CLI↔daemon version skew (`clawft-rpc/src/version_check.rs`)
  - kernel: services and processes with state
  - chain: sequence, head hash, last checkpoint, `chain.verify` result (cached, not recomputed per request)
  - MCP: this gateway's `/mcp` surface (profile, tool count) and each attached upstream MCP server with status
  - channels: each configured channel and its connection state
  - providers: configured provider names and reachability; never keys, URLs with credentials, or env values
  - the caller's own token: id, issued_at, expires_at

The separate `/status` stub route is removed once `/health` serves this.

### D2. The playground is a gateway page

The gateway serves `/playground`, a static page. The MCP HTTP router (`mcp_http_router`) is mounted into the gateway at `/mcp` so the page talks to one origin with one token. `weft mcp-server --listen` stays for headless MCP-only use.

The page:
- reads the token from the URL fragment (below), keeps it in memory only, shows its expiry countdown, and offers "revoke".
- renders MCP tools from `tools/list` (each tool already carries a JSON schema) as generated forms.
- renders REST endpoints from an OpenAPI 3.1 document at `/api/openapi.json`, generated from the axum handlers (`utoipa`).
- shows request, response, status, latency, and a copyable `curl` for every call.
- links to `/health` and shows the full status document when a token is present.

### D3. Tokens are issued by the daemon and recorded on the chain

The daemon is the single token authority. The daemon must be running to issue or validate a token.

- `weft token issue [--ttl 15m] [--label playground]` calls daemon RPC `auth.token.issue` over the local socket. Being able to reach that socket is the proof of being the local owner.
- The daemon generates a 256-bit secret, stores only its SHA-256, and appends a chain event `auth.token.issued` with: token id (first 16 hex of the hash), label, issued_at, expires_at, issuer (node id and local uid). The secret is never written to the chain, logs, or disk.
- The CLI prints the token once and a link `http://<gateway>/playground#t=<token>`. The fragment is not sent to the server, so it stays out of access logs and `Referer`.
- `weft token revoke <id>` and the playground's revoke button call `auth.token.revoke`, which appends `auth.token.revoked`. Expiry needs no event; `weft token list` shows active tokens by id.
- The gateway validates bearers through daemon RPC `auth.token.validate`, with a positive-result cache no longer than 30 s and a revocation check that bypasses the cache on revoke.
- **Daemon down**: `weft token issue` fails with a clear message ("start the daemon: weft kernel start"); authenticated gateway requests return 503 with that message; `/health` without a token reports `down`/`degraded`.
- **Daemon restart**: the token table is rebuilt from the chain (`issued` minus `revoked` minus expired), so tokens survive a restart. The hashes are what's rebuilt; secrets were never stored.

### D4. Scope is full

The operator issuing the token is already authenticated by local socket access, so a token grants the full surface: every REST route and the MCP `full` profile, including shell, process, file-write and spawn tools. There are no scoped playground tokens in this ADR. `SessionScopes::owner()` is the capability attached to every issued token; the enterprise ceiling (`session_cap.rs:355`) still applies if a deployment sets one.

A token is owner-equivalent, including lifecycle verbs; it cannot issue/revoke/list tokens (the daemon refuses `auth.token.issue`, `revoke` and `list` for a token bearer).

Because a token is full-power, TTL is the main limit: default 15 minutes, maximum 24 hours.

### D5. One token path

- `POST /api/auth/token` and its `PUBLIC_PATHS` entry are removed. `/api/auth/revoke` stays and forwards to `auth.token.revoke`.
- The gateway's `TokenStore` is removed; the MCP router mounted in the gateway uses the same daemon validation.
- `weft mcp-server --issue-token` is changed to call the daemon, or removed.

### D6. Wire the gateway to the daemon

The `InMemoryKernelFacade` stub is replaced by a `DaemonKernelFacade` over `clawft_rpc::DaemonClient` (the backend WEFT-122 left as a stub). This is a prerequisite for D1's kernel/chain sections and for D3.

## Security notes

- The gateway binds to loopback by default. Binding a non-loopback address with auth enabled must also require TLS (or an explicit `--dangerously-plain-http` flag), because a full-scope bearer over plain HTTP on a LAN is equivalent to handing out shell access. Implemented: the gateway has no TLS of its own, so a non-loopback API bind is refused at startup unless `--dangerously-plain-http` / `gateway.dangerously_plain_http` states that TLS is terminated in front.
- CORS on `/mcp` and `/api` is same-origin only unless `gateway.cors_origins` is set.
- Every authenticated call is audit-logged with token id and label `playground`; shell/process/file-write calls are additionally appended to the chain under their existing tool-audit kinds.

## Consequences

- One token system instead of two, and every token grant is on the chain.
- Health checks stay anonymous; detail needs a token.
- The gateway gains a hard dependency on the daemon for anything authenticated. Standalone `weft gateway` without a daemon becomes health-only.
- OpenAPI generation adds `utoipa` annotations to every REST handler.

## Implementation cards

| Card | Work | Done when |
|---|---|---|
| api-playground-01 | Daemon RPC `auth.token.issue/revoke/validate/list`; hash-only store; chain events; rebuild from chain on start | Unit + daemon RPC tests; restart test shows a token survives and a revoked one does not |
| api-playground-02 | `weft token issue/revoke/list` CLI; prints token and fragment link | CLI test against a test daemon |
| api-playground-03 | `DaemonKernelFacade` replaces the stub left by WEFT-122 | `/api/processes`, `/api/chain/status` return live daemon data in an integration test |
| api-playground-04 | Gateway auth via daemon validation; remove public `/auth/token` and `TokenStore` | Old mint route 404; issued token works; revoked token 401 within 30 s |
| api-playground-05 | Tiered `/api/health` (D1); remove `/status` stub | Anonymous response has only `status`; tokened response has every D1 section; no secrets (leak test) |
| api-playground-06 | Mount MCP router at `/mcp` in the gateway, full profile, daemon auth | `initialize`, `tools/list`, `tools/call` pass through the gateway with an issued token |
| api-playground-07 | `utoipa` annotations; `/api/openapi.json` | Spec validates; every route in `handlers.rs` is present |
| api-playground-08 | `/playground` page (D2) | Manual run plus a headless browser test: open link, call a REST route and an MCP tool, revoke, next call 401 |
| api-playground-09 | Docs: `docs/guides/api-playground.md`, update `mcp.md` and `configuration.md`; TLS-on-non-loopback guard | Gate green; docs match behaviour |
