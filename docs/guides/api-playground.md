# API playground

The gateway serves an interactive page at `/playground` for trying its REST
routes and MCP tools with a bearer token. It is part of the `clawft-ui` build
(ADR-102 D2: `docs/adr/adr-102-gateway-health-and-api-playground.md`).

## Open it

1. Start the kernel daemon and the gateway with the UI built
   (`scripts/build.sh ui`, then `weft ui`, or `weft gateway` with a static UI
   directory).
2. Issue a token: `weft token issue --label playground` (default lifetime 15
   minutes, maximum 24 hours). `weft ui` prints a link of the form
   `http://localhost:18789/#token=<token>`.
3. Open `http://<host>:<api_port>/playground#token=<token>`.

The page reads the token from the URL fragment and removes it from the address
bar. The fragment is never sent to the server, so the token stays out of access
logs and `Referer` headers. Without a token the page shows how to get one and
can reach only the public `/api/health`; it also accepts a pasted token in a
password field.

If the gateway was started without a static UI directory, or the UI was not
built, `/playground` answers 404 with a hint.

## What the page does

| Section | Source | Notes |
|---|---|---|
| Token bar | `token` section of the tokened `/api/health` | Shows token id, label and an expiry countdown. **Revoke** calls `POST /api/auth/revoke`. After a revoke, **Call again** sends one more request so you can see the gateway answer 401. |
| Health | `GET /api/health` with the token | Daemon, kernel, chain, MCP, channels, providers and token sections, plus a "no token" comparison that shows the public view. |
| MCP tools | `POST /mcp` (`initialize`, `tools/list`, `tools/call`) | One form per tool, generated from the tool's `inputSchema`. Scalars, enums and booleans get typed inputs; nested objects and arrays are JSON fields. A raw-JSON mode sends arguments as written. |
| REST | `GET /api/openapi.json` | Every documented operation, grouped by tag. Path and query parameters get inputs; operations with a body get a JSON editor. Streaming routes (`/stream`, `/ws`, `/events`) are listed but not sendable. |

Every call shows the request line, status, latency, request and response
bodies, and a copyable `curl`.

## The token and curl

- The token is held in page memory only. It is never written to
  `localStorage`, `sessionStorage` or a cookie, and never put in a URL or query
  string (the request helper refuses to build such a URL). Reloading the page
  needs the link again.
- The token travels only in an `Authorization: Bearer` header.
- **Mask token in curl** is on by default. A masked curl reads the token from
  `$WEFT_TOKEN` (`export WEFT_TOKEN=...` first), so it is safe to paste into a
  ticket or chat. Turning the mask off inlines the real token in the header; it
  still never appears in the URL.
- Response bodies and error text are scrubbed of the exact token string before
  they are shown. Redaction is exact-match only: a server that echoes the token
  re-encoded (base64, URL-encoded, split or truncated) is not caught, so do not
  treat the display as safe to share.
- A token is owner-equivalent (ADR-102 D4): every REST route and the full MCP
  profile, including shell, process and file-write tools. Treat the page like a
  root shell and revoke the token when done.

## What the gateway enforces

- `/playground` is plain HTML with no data and no token in it. It is served with
  `Cache-Control: no-store`, `Referrer-Policy: no-referrer` and its own strict
  CSP: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
  img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none';
  frame-ancestors 'none'`. Unlike the gateway-wide policy it does not allow
  `ws:`/`wss:`, so the page can only talk to its own origin.
- The dashboard's service worker (`clawft-ui/public/sw.js`) bypasses
  `/playground`, `/playground/` and `/playground.html`, so the cached dashboard
  shell is never served there.
- Everything the server sends (tool names and descriptions, OpenAPI text, tool
  output, response bodies) is rendered as text by React; nothing uses
  `dangerouslySetInnerHTML`.
- Everything the page calls (`/api/*`, `/mcp`) goes through the bearer
  middleware. Without a valid token those calls return 401, and
  `/api/health` returns only `{"status":"ok"}`.
- Tests: `crates/clawft-services/tests/gateway_auth.rs` (`playground_*`) covers
  the page being served and the data calls requiring the token;
  `clawft-ui/tests/e2e/playground.spec.ts` drives the page in a browser (token
  link, REST call, MCP call, curl masking, revoke, 401, and hostile server
  content rendering as text), and
  `clawft-ui/src/playground/core.test.ts` covers the pure logic
  (`npm run test:unit`).

## Development

The page is a second entry of the `clawft-ui` Vite build
(`clawft-ui/playground.html`, sources in `clawft-ui/src/playground/`). It shares
Tailwind and the UI primitives with the dashboard but not its token storage or
service worker. With `npm run dev`, open `/playground.html#token=...`; the Vite
proxy forwards `/api` to the gateway on `:18789`, and `/mcp` is not proxied
by default, so add it to `server.proxy` in `vite.config.ts` for local MCP work.

To run the browser tests against Chrome you already have installed, set
`CLAWFT_UI_E2E_CHROME` to its executable path.
