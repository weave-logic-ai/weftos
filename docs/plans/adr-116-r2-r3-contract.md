# ADR-116 R2 + R3: contract between the node and dashboard lanes (2026-10-10)

Two lanes build R2 (per-route tailnet-login allowlists) and R3 (routes on the dashboard,
editable there) at the same time. This file fixes where they meet.

## Rule that binds every payload

`node_actions.payload` refuses field names containing `token`, `secret`, `key` or `password`.

## 1. Allowlists (R2, node)

A route may carry `allow: ["alice@example.com", "bob@example.com"]`:
- in the project's `compose/ports.yaml`, or
- in a dashboard overlay (section 3).

- **Identity:** the router identifies the caller by the `Tailscale-User-Login` header that
  Tailscale Serve sets for requests from user-owned devices.
- **Allowed:** a route with an `allow` list serves only logins on the list. Matching is exact
  and case-insensitive on the whole address.
- **Refused:** everyone else gets a uniform `403` page naming the route, not the list. That
  includes requests with no login, which come from tagged devices.
- **No list:** a route without `allow` behaves as in R1, open to the tailnet.

The router only listens on loopback, so only Serve and local processes reach it. Before
proxying, it strips every incoming `Tailscale-*` header except the ones Serve itself sets. It
then forwards `Tailscale-User-Login` and `Tailscale-User-Name` upstream unchanged, so apps can
read who is calling.

## 2. What each machine reports (R3, node to dashboard)

Every heartbeat carries `report.routes` (omitted when the router is off):

```json
"routes": {
  "base_url": "https://<machine>.<tailnet>.ts.net",
  "served": true,
  "items": [{"prefix": "/shastaos", "project": "shasta", "project_ulid": "<ULID or null>",
             "port": 18120, "default": true, "healthy": true, "restricted": true,
             "source": "repo" | "dashboard"}],
  "refused": [{"project": "...", "prefix": "...", "reason": "..."}]
}
```

- **`base_url`:** read from `tailscale status --json` (Self.DNSName). It is absent when
  Tailscale is unavailable.
- **`served`:** true when Tailscale Serve's `:443` points at the router.
- **`restricted`:** says whether an allowlist is set. The list itself is not reported.
- **Bounds:** at most 32 items and 16 refusals.
- **`report.services`** (add-on, omitted when there is nothing to report): per registered
  project with a `process-compose-http` claim, `{"project", "project_ulid": "<ULID or null>",
  "pc_port", "state": "ok" | "unreachable", "processes": [{"name", "status", "ports": [..],
  "restarts"}]}`, read from that project's process-compose `GET /processes` (name, status and
  restart count only; never commands or environment). `ports` are the project's `claims` whose
  `use` names the process, else empty. At most 16 projects and 32 processes per project.

## 3. Editing routes from the dashboard (R3, dashboard to node)

A new action kind, `route`, with this payload:

```json
{"op": "set" | "remove", "project_ulid": "<ULID>", "prefix": "/name", "port": 18120,
 "health": "/path", "default": false, "allow": ["alice@example.com"]}
```

- **Where it is written:** the node writes dashboard routes to an overlay file,
  `~/.weftos/routes/<ULID>.yaml` (mode 0600, atomic, same schema as a `routes:` list). It
  never edits a repository file.
- **Precedence:** routes declared in the repository win over overlay routes with the same
  prefix. Conflicts follow the R1 refusal rules and are reported in `refused`.
- **Validation:** the same rules as R1. The project must be registered on this node. `remove`
  deletes only an overlay route.
- **Result:** `{"op", "prefix", "applied": true|false, "reason"?}`. After applying, the next
  heartbeat's `report.routes` shows the outcome.
- **Database:** the dashboard's `node_actions.kind` check must allow `'route'`. That needs a
  migration, which the owner applies.
