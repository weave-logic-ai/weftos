# Tailnet router: one HTTPS front door per dev machine

Every project on a dev machine runs its own process-compose on its own ports
(ADR-098). The tailnet router (ADR-116) sits inside the user daemon and makes
each of them reachable at

```text
https://<machine>.<tailnet>.ts.net/<project>/
```

Tailscale Serve terminates TLS on `:443` and proxies everything to the router
on loopback; the router proxies by path prefix to the project's port. It is
tailnet-only, it never enables Funnel, and it adds no authentication of its
own in R1 (whoever reaches the machine on the tailnet reaches the routed
apps, as with Tailscale Serve today).

## Enable it

In `~/.weftos/weave.toml` (user daemon, `weaver kernel start --profile user`):

```toml
[router]
enabled = true                  # off by default
# listen = "127.0.0.1:18000"    # loopback only
# poll_secs = 5                 # how often each project's compose/ports.yaml mtime is checked
# health_timeout_ms = 1500      # per health / process-compose probe
```

Restart the daemon. `weaver route list` shows `tailnet router on 127.0.0.1:18000`
once it is up. An invalid section is logged and the daemon keeps running
without the router.

## Declare routes

Routes live in each project's own `compose/ports.yaml`, next to its port
claims. The router reads every project registered with the user daemon
(`~/.weftos/projects`), including the repositories an ADR-108 workspace lists.

```yaml
project: shastaos
claims:
  - { port: 18110, use: process-compose-http }
  - { port: 18120, use: shasta-field }
routes:
  - { prefix: /shastaos, port: 18120, health: /api/health, default: true }
```

| Field | Meaning |
|---|---|
| `prefix` | `/segment[/segment]` of `[a-z0-9-]`; defaults to `/<project>`. `/`, `/api/`, `/console/` and `/_weftos/` are reserved. |
| `port` | Loopback upstream, 1024 to 65535. |
| `health` | A path the index GETs (2xx/3xx is healthy). Optional. |
| `default` | At most one route on the machine; it also answers paths no prefix matches (`/`). Transitional, see the Shasta example. |

Rules:

- Longest prefix wins, on segment boundaries (`/a` covers `/a/x`, never `/ab`).
- The prefix is **kept** on the upstream request. The router sets
  `X-Forwarded-Host`, `X-Forwarded-Proto: https`, `X-Forwarded-Prefix` and
  `X-Forwarded-For`; `Host` passes through as Tailscale Serve sends it.
- WebSocket upgrades are tunnelled (Next.js and Vite hot reload work).
  Bodies stream in both directions. There is no HTML rewriting.
- A prefix or port already taken by an earlier project, or a second
  `default: true`, is **refused and reported** (`weaver route list`,
  `/_weftos/`), never resolved silently. A bad declaration refuses only
  itself; a file that does not parse refuses that project's routes.
- The table is re-read when a `ports.yaml` changes (mtime poll) and on
  `weaver route reload`.

### The base-path requirement

An app under a prefix must know it. Set it once in the app rather than
relying on link rewriting:

- Next.js: `basePath: '/shastaos'` in `next.config.js` (assets and the HMR
  socket follow).
- Vite: `base: '/myapp/'` in `vite.config.ts`.
- Plain servers: read `X-Forwarded-Prefix` when building absolute links.

Until the app has its base path, the project can hold the root with
`default: true`, which keeps `https://<machine>.<tailnet>.ts.net/` serving it
while the prefixed URL also works.

## The index: `/_weftos/`

`https://<machine>.<tailnet>.ts.net/_weftos/` lists every route, its project,
the upstream's health and the project's process-compose state (read from the
project's own process-compose HTTP API, the port its `ports.yaml` claims as
`process-compose-http`; `GET /processes`, read only). The same document is
JSON at `/_weftos/routes.json` and is what `weaver route list` prints.

A route whose upstream is down gets a 502 page naming the project and the
port it should be on.

## Commands

```text
weaver route list [--json]        # routes, refused declarations, health, process-compose state
weaver route reload               # re-read every project's compose/ports.yaml now
weaver route serve --plan         # the Tailscale Serve change needed (:443 → the router)
weaver route serve --apply        # run it: tailscale serve --bg --https=443 http://127.0.0.1:18000
```

`serve` reads `tailscale serve status --json` and refuses, changing nothing,
when:

- Funnel is on for `:443` (turn it off first; the router never enables it);
- an existing mapping has no route equivalent, for example `/ → http://127.0.0.1:18120`
  with no route for `:18120`. Declare the route (`default: true` to keep it at
  `/`), `weaver route reload`, then apply.

A covered mapping under a sub-path is cleared first (`--set-path=<p> off`),
then `/` is pointed at the router. `tailscale` runs with an argument vector,
no shell.

## Example: Shasta on the owner's Mac

Today Tailscale Serve proxies `/` straight to Shasta's field app on `:18120`.
To move it behind the router without a gap:

1. In Shasta's `compose/ports.yaml` add
   `routes: [{ prefix: /shastaos, port: 18120, health: /login, default: true }]`
   (keep `project: shastaos` or whatever slug the prefix should default to).
2. `weaver route reload`, then `weaver route list` shows `/shastaos/ *` healthy.
3. `weaver route serve --plan` prints the one `tailscale serve` command and
   notes that `/ → http://127.0.0.1:18120` is covered by the default route.
4. `weaver route serve --apply`. `https://<machine>.<tailnet>.ts.net/` still
   serves Shasta (default route); `/shastaos/` serves it too; `/_weftos/`
   lists it.
5. Set `basePath: '/shastaos'` in Shasta's Next.js config, rebuild, drop
   `default: true`. The root then shows the index.

WeftOS itself declares its docs site at `/weftos-docs` (`compose/ports.yaml`
in this repo, port 4000).
