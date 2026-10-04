# weft-cog-host: `/hw/*` API, token, origins, agent command

`weft-cog-host serve` (default `:9480`) exposes the hardware routes used by the console's
**Identify hardware** modal and **Hardware Dex**. The host binds all interfaces, so the API is
guarded against drive-by browser requests.

## Routes

| Route | Auth | What |
|---|---|---|
| `GET /hw/usb` | token, read-only | USB inventory vs the baseline, labelled from the bundled id table plus your registered rows. Serials are never returned in full (`…1234`); macOS port names have their serial suffix redacted (`/dev/cu.usbmodem…3501`). Includes `unseen_catches`. |
| `POST /hw/usb/scan` | token | Same report, but records sightings in the dex (catches, `times_seen`). |
| `POST /hw/usb/baseline` | token | Accept the current scan (or body `{keys:[..]}`) as known. |
| `POST /hw/usb/identify` | token | Body `{key}`. Asks the configured agent about the device (see below). |
| `GET /hw/dex` | token, read-only | `caught`, `unseen_catches`, `wild`, `totals`, `badges`, `types`, `numbers`. |
| `POST /hw/dex/catch` | token | Body `{key, catalog_id \| "wild", name?, answer?, force?}` ("Register species"). A device that already matches a bundled id row is refused unless `force`, which adds only a row scoped to that exact product string. |
| `POST /hw/dex/ack` | token | Body `{refs?}`; clears NEW CATCH banners (all when omitted). |

Device keys are `vid:pid:s-<16 hex>` where the hash is salted per host (`<root>/hw/salt`). State
lives in `<root>/hw/` (`dex.json`, `usb-baseline.json`, `salt`). Dex numbers are per host: each host
numbers the catalog independently (append-only), so the same part can have different numbers on
different hosts. A match made on a product string only (for example the Cognitum Seed on the shared
Linux Foundation VID) is marked `claimed`; a device can spoof it.

## Browser and network safety

- **Host header allowlist (DNS rebinding):** requests whose `Host` is not a loopback name, an IP
  literal, this machine's hostname (plus `<short>.local` and its tailnet MagicDNS name when
  `tailscale` is installed, looked up with a 2 s timeout) or one of `WEFT_COG_HOST_NAMES` (comma
  separated) get `421`. `WEFT_COG_HOST_NAMES` is needed for router-domain names such as `pi5.lan`, and
  when the tailscale CLI is not available (macOS also tries
  `/Applications/Tailscale.app/Contents/MacOS/Tailscale`). This applies
  to every route, including the read views. A missing `Host` header is refused too, except on
  `/fleet/heartbeat`, which minimal firmware may send without one.
- **Token on every POST** (except edge `/fleet/heartbeat`, which firmware cannot sign) and on every
  `GET /hw/*`: `Authorization: Bearer <token>`. `GET /status`, `/network` and `/healthz` stay open
  for the console's read views.
- Every `POST` (except heartbeats) also needs `Content-Type: application/json` and
  `X-Weft-Host: 1`. The custom header forces a CORS preflight.
- CORS is answered only for allowed origins on `/hw/*` and on every POST (an exact `Origin` is echoed,
  never `*`). Allowed: `http://127.0.0.1:*`, `http://localhost:*`, `http://[::1]:*`, the host's own
  origin, plus `WEFT_COG_HOST_ORIGINS` (comma separated, entries may end in `:*`). Plain GETs of
  `/status`, `/network`, `/healthz` still send `*` for the web console.
- Limits: 32 concurrent connections (503 beyond), 20 s per request, 64 KB bodies on `/hw/*`, 4 KB on
  `/fleet/heartbeat`.
- `/fleet/heartbeat` stays unauthenticated and is bounded: at most 256 nodes (least recently seen
  evicted), strings capped (id 64, others 128), rssi and battery range-checked. The roster is
  display-only (`/network`); nothing may use it for placement or policy.

**Out-of-repo scripts** that call the legacy routes (`/install`, `/cogs/<id>/start|stop`, `/reload`)
now need `Authorization: Bearer <token>` plus `Content-Type: application/json` and `X-Weft-Host: 1`,
and must send an allowed `Host` (an IP or the machine name works).

## Host token

The token is `$WEFT_COG_HOST_TOKEN`, else the 0600 file `<root>/host.token`, generated (32 random
bytes, hex) on first start. The console takes it from the **token** field in the top bar,
`WEFTOS_HOST_TOKEN` (native) or `?token=` (browser), and shows "host token required" when it is
missing or wrong.

## Agent command (Ask agent)

Device-reported strings are untrusted (any USB device can claim any name), so they are flattened to
one line, capped at 64 chars and fenced as untrusted data in the prompt. The agent is **off by
default**: set `WEFT_COG_HOST_AGENT_CMD` to a command that runs with tools disabled; the prompt is
appended as the last argument. Example (check your CLI's flag):

```
WEFT_COG_HOST_AGENT_CMD='claude -p --tools ""' weft-cog-host serve
```

`weft agent` has no flag to disable tools, so it is not used as a default. The command runs in its
own process group with a 90 s timeout (the whole group is killed), 16 KB output cap, one at a time.

## Mesh view: `GET /mesh/cogs` (ADR-107)

Read-only, open like `/status`. The host returns its own cogs plus the cogs of every online tailnet peer that answers `GET :9480/status` as a cog host (override the peer port with `WEFT_COG_HOST_PEER_PORT`):

```json
{"ok":true,"scope":"mesh|this_host_only","reason":"...","self":"cog0",
 "nodes":[{"node":"cog0","ip":"","self":true,"reachable":true,"cogs":[<status rows>]},
          {"node":"pi5","ip":"100.x","self":false,"reachable":false,"error":"connection refused","cogs":[]}]}
```

`/status` cog rows also carry `log_bytes` and `log_age_s` (size and age of the cog's `host.log`). Cogs have no ingest queue, so these are the only backlog numbers a host reports.
