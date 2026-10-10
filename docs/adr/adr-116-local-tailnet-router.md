# ADR-116: A tailnet router on every dev machine: one HTTPS front door, projects by path

- **Status**: Proposed (2026-10-10; the owner called it critical for running projects side by side)
- **Implementation**: R1 built on branch `wt/router` (2026-10-09): router in the user daemon (`[router]` in `weave.toml`, off by default), routes from `compose/ports.yaml`, conflict refusal, WebSocket proxying, `/_weftos/` index with process-compose state, `weaver route list|reload|serve --plan|--apply`. Guide: `docs/guides/tailnet-router.md`. Cutover on the owner's Mac is pending (the lead does it with the owner).
- **Deciders**: owner
- **Builds on**: ADR-098 (per-project process-compose; `compose/{manifest,ports}.yaml`; the
  machine overlay it deferred), ADR-108 (workspaces registered with the user daemon; the
  dashboard reporter and node actions), ADR-103 (user daemon), ADR-114 (`weftos://` names).
- **Amends**: ADR-098 §2 ("do not build the environment overlay yet"). This ADR builds the
  query-and-route half of that overlay, using ADR-098's own preferred shape ("many PC instances
  plus one query pane"), and still never merges YAML or starts a second scheduler.

## Context

Every project on a dev machine runs its own process-compose (ADR-098), on its own ports:
WeftOS `:18090`, Forge `:18080`, Shasta `:18110`, with apps such as Shasta's field app on
`:18120`. Ports are claimed per repo in `compose/ports.yaml`. Reaching one from a phone or
another machine means knowing its port, and only one app can sit behind the machine's
Tailscale HTTPS name. Today that is Shasta:
`https://bigmac-the-max.tail23f8f7.ts.net/` is Tailscale Serve proxying `/` to `:18120`.

The owner wants one front door per machine that serves anything on it over the tailnet, by
project:

```text
https://bigmac-the-max.tail23f8f7.ts.net/shastaos/    → Shasta on this Mac
https://<machine>.<tailnet>.ts.net/<project>/         → that project on that machine
```

The same must work on every local dev environment (the owner's Mac, aepod-xpc, members' Macs),
because a project usually runs next to several others.

Tailscale issues one certificate per machine name and no wildcards, so per-project subdomains
are not available. Path prefixes (or extra HTTPS ports) are the options.

## Decision

### 1. Tailscale Serve owns :443; the WeftOS router owns the paths

- Tailscale Serve on each machine terminates TLS on `:443` with the machine's `*.ts.net`
  certificate and proxies **everything** to the local WeftOS router on loopback
  (`http://127.0.0.1:18000`). It is tailnet-only. The router refuses to configure Funnel, and
  refuses to start if Funnel is on for that port.
- The router runs inside the **user daemon**, so there is one per machine and it shares the
  daemon's project index (ADR-108). It binds loopback only.

### 2. Routes come from the projects, not from hand edits

- Each project declares routes in its own `compose/ports.yaml` (the ADR-098 port registry),
  next to the port claims:

  ```yaml
  project: shastaos
  claims:
    - { port: 18110, use: process-compose-http }
    - { port: 18120, use: shasta-field }
  routes:
    - { prefix: /shastaos, port: 18120, health: /api/health }
  ```

  The prefix defaults to `/<project slug>`.
- The router reads routes from every project registered with the user daemon (project homes and
  ADR-108 workspaces). It re-reads when a manifest changes and on `weaver route reload`.
- **Conflicts are refused, never resolved silently.** If two projects claim the same prefix or
  port, the later one is not routed and the conflict is reported (CLI, index page, dashboard).
  Reserved prefixes: `/`, `/api/`, `/console/`, `/_weftos/`.

### 3. What the router does

- **Reverse proxy by path:** prefix → `127.0.0.1:<port>`. It keeps the prefix (apps set their
  base path), sets `X-Forwarded-Host`, `X-Forwarded-Proto: https` and `X-Forwarded-Prefix`,
  and proxies WebSocket upgrades (dev servers' hot reload).
- **A transitional root route:** exactly one project may set `default: true` to keep serving
  at `/` while it moves to a prefix. Shasta does this until its `basePath` is set.
- **An index at `/_weftos/`:** each route, its project, its health, and the project's
  process-compose state, read from that project's own process-compose HTTP API (`pc_http`).
  This is ADR-098's "one query pane". It reads only and never starts or stops anything in
  phase 1.
- **No HTML rewriting.** An app served under a prefix must know its base path (`basePath` in
  Next.js, `base` in Vite). The router documents this rather than guessing at links.

### 4. Each machine reports what it serves; the dashboard shows it

The dashboard reporter adds `report.routes`: `[{prefix, project_ulid, port, healthy}]` plus the
machine's HTTPS base URL. The dashboard lists every machine's URLs on each project, so "where
is Shasta running" has a clickable answer. Editing routes from the dashboard (desired `route.*`
parameters applied through a node action) is phase 3.

### 5. Commands

```text
weaver route list                 # routes, conflicts, health
weaver route reload               # re-read project manifests
weaver route serve --plan|--apply # show or apply the Tailscale Serve config (:443 → router)
```

`serve --apply` prints the exact `tailscale serve` change and refuses to drop an existing
mapping that has no route equivalent (for example today's `/ → :18120`) unless that project
has a route.

## Consequences

- Every project on every dev machine gets a stable tailnet URL with no port numbers, and a phone
  or another machine can open any of them.
- Apps under a prefix need their base path set once. Until then a project can hold the root
  route, and only one at a time.
- The router sits in the request path for local dev traffic. It is loopback-bound behind
  Tailscale's TLS and identity, and it adds no authentication of its own in phase 1. Anyone on
  the tailnet who can reach the machine reaches the routed apps, as with Tailscale Serve today.
  Per-route tailnet-login allowlists are phase 2.
- ADR-098's registry is the user daemon's project index rather than a new
  `~/.weftos/compose/registry.yaml`.

## Phases

| Phase | Delivers | Done when |
|---|---|---|
| R1 | Router in the user daemon on `127.0.0.1:18000`; routes from `compose/ports.yaml`; conflict refusal; WebSocket proxying; `/_weftos/` index with process-compose state; `weaver route list|reload|serve --plan|--apply` | on this Mac, `https://bigmac-the-max.tail23f8f7.ts.net/shastaos/` serves Shasta through the router, and `/_weftos/` lists it as healthy |
| R2 | Per-route tailnet-login allowlist (Tailscale identity headers from Serve, checked against the project's members) | a login not on the list gets 403 on that prefix only |
| R3 | `report.routes` on the dashboard; desired `route.*` parameters applied through a node action | the dashboard shows each machine's URLs for a project and can add a route that the node applies |
| R4 | Optional start-on-demand: the router asks the project's process-compose to start a stopped process before proxying | a request to a stopped app starts it and is served once its readiness probe passes |

## Owner decisions (2026-10-10)

1. The project slug is the default prefix (`/<slug>/`), overridable per route.
2. Shasta keeps the root (`default: true`) during testing; the root moves to the router index
   once Shasta's `basePath` is `/shastaos`.
