# Dashboard reporter and token rotation

The user daemon can report to the WeftOS dashboard itself. It replaces the
`heartbeat.sh` script on a systemd timer: same endpoint, same report, no shell,
no `curl`, and the token never appears on a command line.

## Configure

In `~/.weftos/weave.toml` (user daemon, `weaver kernel start --profile user`):

```toml
[dashboard]
enabled = true                       # off by default
url = "https://weftos-dashboard.vercel.app"
node_id = "a5e70b99-a919-4ff1-9f44-5fcba69d439c"   # this node's UUID in the dashboard
installation_id = "photo-gallery"
token_file = "/home/weftos/.weftos/dashboard/node.token"
interval_secs = 60                   # 5..=3600
# gateway_url = "http://127.0.0.1:8080"             # else derived from [gateway] when its API is on
# units = ["weftos.service", "weftos-gateway.service"]   # systemctl --user units to report (Linux)
# allow_remote_rotate = true         # accept a rotate sent over the mesh (see below)
# report_workspaces = true           # report project checkouts and git state (default on)
```

Rules the daemon enforces at start (it logs why and keeps running if one fails):

- `url` is `https://`; plain `http://` is accepted only for a loopback host. No credentials, query or fragment.
- `token_file` is absolute, a regular file (symlinks refused), mode exactly `0600`, owned by the daemon's uid, in a directory that is not group- or world-writable. Otherwise nothing is sent.
- `node_id` is a UUID; `interval_secs` is 5 to 3600; an unknown key is an error.

## What is reported

`POST <url>/api/nodes/heartbeat` with `Authorization: Bearer <token>` and

```json
{"node_id": "...", "installation_id": "...",
 "report": {"host": "<installation_id>", "weaver_version": "0.8.3",
            "observed": {"<project ULID>": {"release": "v0.8.3", "unit_weftos": "active",
                         "unit_gateway": "active", "project_child_state": "running",
                         "mesh_listen": "0.0.0.0:9470", "gateway_url": "http://127.0.0.1:8080"}}}}
```

One `observed` entry per registered project, with its child state from the
project supervisor (`not running` when the supervisor has no slot for it).
Failures back off (doubling, capped at 15 minutes). A 401 or 403 logs
`dashboard token rejected` and the reporter keeps running; write a fresh token
to the file and the next beat picks it up. The token is read on every beat and is
never logged.

## Workspaces (ADR-108 P1)

Unless `report_workspaces = false`, every beat also carries `report.workspaces`:
one entry per project in this daemon's manifest index (`~/.weftos/projects`),
so the dashboard can show where a project is checked out on each machine.

```json
"workspaces": [{"ulid": "<project ULID>", "root": "/home/me/code/proj",
  "git": [{"path": ".", "remote": "https://github.com/org/proj.git", "branch": "main",
           "head": "1a2b3c4d", "dirty": 2, "ahead": 1, "behind": 0}],
  "last_activity": "2026-10-05T11:58:02+00:00"}],
"workspaces_truncated": false
```

- `git` lists each repository at the project root (`path` `.`) or one directory
  below it (`path` relative to the root). Symlinked and hidden directories are not
  followed.
- An ADR-108 workspace (`weft project init --adopt <ULID> [--repo DIR ...]`) may
  list extra repository directories outside its root in its manifest (`repos`):
  each is reported after the root's own, as `../<name>` for a sibling of the root
  or its absolute path otherwise. Entries that are not absolute, are symlinks, are
  missing or are not repositories are skipped. The 8-repository cap covers both.
- `remote` is the `origin` URL with any userinfo, query and fragment removed
  (`https://user:token@host/x` is sent as `https://host/x`; `git@host:org/x` as
  `host:org/x`). `branch` is null when detached; `head` is the first 8 characters
  of the commit id, null on an unborn branch. `dirty` is the number of entries in
  `git status --porcelain` (untracked included). `ahead`/`behind` are null without
  an upstream. `last_activity` is the newest mtime of any repository's `HEAD` or
  `index` (null when there are none).
- Only counts and refnames leave the machine: never file contents, diffs or
  untracked file names.
- Caps: 50 projects, 8 repositories per project, 16 KiB for the whole `report`,
  and a 15 second budget per beat for git. Past a cap the list is cut from the end
  and `workspaces_truncated` is `true`.
- Facts come from the `git` CLI (no shell, 10 s timeout and 1 MiB output limit per
  call, no terminal prompt, no optional index lock, `core.fsmonitor` forced off).
  A repository git cannot read (not installed, `safe.directory`, timeout) is left
  out of `git`.

## Routes (ADR-116 R3)

While the tailnet router is on (`[router] enabled = true`), every beat also
carries `report.routes`; the key is left out when it is off.

```json
"routes": {"base_url": "https://machine.example.ts.net", "served": true,
  "items": [{"prefix": "/shastaos", "project": "shastaos", "project_ulid": "<ULID or null>",
             "port": 18120, "default": true, "healthy": true, "restricted": false, "source": "repo"}],
  "refused": [{"project": "other", "prefix": "/shastaos", "port": 5000,
               "reason": "prefix /shastaos is already routed by project shastaos", "source": "dashboard"}]}
```

- `base_url` is `https://` + `Self.DNSName` from `tailscale status --json`
  (trailing dot stripped); it is absent when Tailscale is unavailable. `served`
  is true when `tailscale serve status --json` shows `:443` proxying `/` to the
  router. Both run without a shell, with a 20 s limit, and are cached for two
  minutes.
- `healthy` is the route's health probe: `true` (2xx/3xx), `false`, or `null`
  when the route declares no `health` path. `restricted` says whether an
  `allow` list is set; the list itself never leaves the node. `source` is
  `repo` (the project's `compose/ports.yaml`) or `dashboard` (an overlay).
  `project_ulid` is null for a workspace repository directory that is not a
  manifest root.
- At most 32 items and 16 refusals, longest prefix first.

## Actions (ADR-108 P2)

The dashboard's answer to a heartbeat may carry work for this node:

```json
{"ok": true, "actions": [{"id": "<id>", "kind": "install", "payload": {}, "created_at": "..."}]}
```

Kinds are `install`, `update`, `remove`, `pair` and `route`. An answer without `actions`
(or with a malformed one) is fine. The reporter keeps at most 64 queued actions
and 128 recorded ones in memory (a restart forgets them; the dashboard repeats an
action until it has a final result). Ids are limited to `[A-Za-z0-9_-]{1,64}`; any
other action is dropped. After a successful beat each queued action is
acknowledged with `POST <url>/api/nodes/actions/{id}/result` (node token as the
bearer):

1. `{"status": "running"}`
2. the handler's outcome: `{"status": "failed" | "succeeded", "result": {...}}`.

A final result the dashboard did not accept is re-sent on the next beat without
running the handler again.

`install`, `update` and `remove` run real handlers (ADR-108 P3a), and `pair` is
handled too (see [Pairing](#pairing-adr-108-p2b)). An unknown kind answers `failed` with
`{"error": "unknown action kind \"<kind>\"", "kind": "<kind>"}`. The seam is the
`ActionHandler` trait in `crates/clawft-weave/src/dashboard_actions.rs`
(`Dashboard::set_action_handler`). Handlers must not trust `payload`: it is
whatever the dashboard sent, and it is never logged, kept in the action log or
echoed in an error.

- **`install`** `{project_ulid, target_path, slug?, sources: [{url, branch?, dir}], primary?}`.
  The payload is validated first (ULID, path inside your home, `https://`,
  `ssh://` or `user@host:path` URLs without credentials, at most 8 repositories).
  The `.` source is cloned into `target_path`, other sources beside it
  (`<parent>/<dir>`), or under it when there is no `.` source. `target_path` and
  every destination must be absent or empty; otherwise the action fails and
  nothing is touched. Clones use your own git credentials (credential helper or
  ssh agent; git never prompts), with a 15 minute limit per repository. The
  checkout is then registered with `weft project init --adopt <ULID>
  [--repo <sibling>]` (the `weft` binary beside `weaver`, else on `PATH`). If
  any step fails, what the install created is removed. Result: `{"fetcher":
  "git-remote", "root": "<path>", "repos": [{"dir", "head", "remote"}], "bytes":
  0, "archived": []}`. The mesh fetcher (P3b) joins the same handler and is
  preferred when the project's primary is a paired peer.
- **`update`** `{project_ulid}`: `git pull --ff-only` in each repository of the
  registered workspace (root, registered extras, and repositories one level
  below the root). A repository that cannot fast-forward keeps its local work
  and reports an `error`; the action then fails. Result: `{"project_ulid",
  "root", "repos": [{"path", "head", "updated", "error"?}]}`.
- **`remove`** `{project_ulid}`: unregisters the workspace from
  `~/.weftos/projects`. It never deletes files; the result names `root` and
  `repos` so you can delete them by hand. A project's own home (not a workspace)
  is refused.
- **`route`** (ADR-116 R3) `{op: "set" | "remove", project_ulid, prefix, port?,
  health?, default?, allow?}`: adds, replaces or removes a tailnet-router route
  for a project registered on this node. The route is written to the project's
  overlay, `~/.weftos/routes/<ULID>.yaml` (0600, atomic; other entries kept),
  never to a repository file; the router is reloaded and the result says what
  it now serves: `{"op", "prefix", "applied": true|false, "reason"?}`.
  Validation is the router's own (prefix `/seg[/seg]` of `[a-z0-9-]`, port
  1024 to 65535, `health` a path, `allow` login-shaped and at most 64, no
  unknown fields) and an unregistered ULID is refused; those fail without
  writing anything. `applied: false` with a `reason` means the overlay was
  written but the router does not serve it: the prefix is declared in the
  project's own `compose/ports.yaml` (the repository wins), it is taken by
  another project, or the router is off on this node. `remove` of a prefix that
  is not an overlay route fails. See
  [Dashboard-managed routes](tailnet-router.md#dashboard-managed-routes).

```bash
weaver dashboard actions          # recent actions and their outcomes (local, Read)
weaver dashboard actions --json
```

RPC: `dashboard.actions` (Read, local only). `dashboard.status` adds
`actions_recorded` and `actions_queued`.

## Pairing (ADR-108 P2b)

Pairing lets a member's machine fetch a project from its primary over the mesh
without anyone editing `workload-peers.json` by hand (ADR-108 decision 6). The
wire shapes are fixed in `docs/plans/adr-108-p2b-p3-contract.md`.

**Identity.** When the mesh is on, every beat carries `report.mesh_identity`:

```json
"mesh_identity": {"node": "<32 hex>", "fingerprint": "<first 16 hex of node>",
                  "ed25519": "<64 hex>", "advertise": "100.64.0.9:9471"}
```

`node` is `node_id_from_pubkey` of the key this node's `workload-host` signs
with: the node key, or the control key in service mode (ADR-106 phase 3). That
is the id peers address on the signed `workload.ctl` wire and the key they pin.
`fingerprint` is the first 16 hex of the node id (the first 16 hex of the
SHA-256 of the key, ADR-025); it is what a member compares between the two
machines before approving. `advertise` is `workload-host.json`'s `advertise`
when set, else the bind address (or the kernel mesh listen address) with a
wildcard host replaced by this machine's tailnet-facing address; it is left out
when there is only loopback. The identity is omitted while the placement
control plane is not up.

**Asking.** `weaver mesh pair request --with <mesh node id> [--project <ULID>]...`
records a pending request (`weaver mesh pair list`, `weaver mesh pair cancel
<request_id>`) in `pair-requests.json` under the daemon's runtime dir (mode
0600, at most 32). Each beat carries them as `report.pair_requests` (at most 8,
oldest first):

```json
"pair_requests": [{"request_id": "<uuid v4>", "with_node": "<32 hex>",
                   "projects": ["<ULID>"], "requested_at": "<rfc3339>"}]
```

The RPCs are `mesh.pair.request` and `mesh.pair.cancel` (Admin) and
`mesh.pair.list` (Read); `--with` takes the mesh node id (the other node's
`mesh_identity.node` in the dashboard), not a dashboard name. The install
handler records a request the same way when its primary is not yet paired
(`clawft_weave::mesh_pair_requests::record`).

**Approval.** A member approves in the dashboard, which queues one `pair`
action to each node:

```json
{"op": "add" | "remove", "request_id": "<uuid>", "peer_node": "<32 hex>",
 "fingerprint": "<16 hex>", "peer_ed25519": "<64 hex>",
 "advertise": "<host:port>", "role": "primary" | "member", "projects": ["<ULID>"]}
```

`role` is the other side's role. Before writing anything the node checks that
`peer_ed25519` is an Ed25519 key, that it derives `peer_node`, and that
`fingerprint` is its first 16 hex; a mismatch, an unknown field, a bad address
or a non-ULID project fails the action with no file touched. A node also
refuses to pair with itself. On `add`:

- the **member** writes the primary into `workload-peers.json` at tier
  `pinned`, `addr` = `advertise`, `key` = `peer_ed25519`;
- the **primary** writes the member at tier `paired` and a grant in
  `project-fetch.json` (`{"peer_node", "projects", "granted_at", "source":
  "dashboard-pair:<action id>", "peer_ed25519"}`), which the P3b `project.fetch`
  gate enforces (default deny: peer listed **and** project granted).

Neither side ever touches `workload-host.json`: a paired peer is never a
controller. `remove` drops the peer entry and the grant. Both files are written
atomically (temp file and rename, mode 0600); entries the pairing did not
write, and keys this build does not know, are kept as they are. A re-add
updates the one entry and the one grant.

The placement control plane reads `workload-peers.json` on every call
(`sync_peers`), so an add or remove takes effect on the next placement or
node-admin call; a `project.fetch` gate reads `project-fetch.json` per
request. No restart is needed. Each add and remove is chained on the daemon's
chain as `mesh.pair.add` / `mesh.pair.remove` (source `mesh.pair`) with the
request id, the action id, the peer's node id, fingerprint, tier, role and
projects; no key material. The action's result is `{"op", "peer_node",
"fingerprint", "tier", "projects"}`, and the pending requests for that peer
are dropped.

Both trust files are refused when group- or world-writable or owned by another
user, and `weaver mesh pair` needs the placement control plane (a unix build
with the `placement` feature).

## Status and rotation

```bash
weaver dashboard status                 # enabled, last heartbeat, last rotation; no token material
weaver dashboard rotate-token           # rotate this node's token
weaver dashboard status --node <id>     # ask a peer over the mesh
weaver dashboard rotate-token --node <id>
```

RPC: `dashboard.status` (Read) and `dashboard.token.rotate` (Admin).

Rotation presents the current token to `POST /api/nodes/token/rotate`
(`{"node_id"}`). The dashboard revokes it and issues a new one. The daemon
writes the new token to a temp file (0600, same directory) and renames it over
the old one, then reports success. If the write fails after the dashboard
rotated, the old token is already revoked: the daemon logs the failure loudly,
keeps the new token in memory (heartbeats keep working), saves it on a later beat
once the file is writable, and reports an error. If it restarts first, issue a new
token in the dashboard and write it to the token file.

## Rotating a remote node over the mesh

`--node <id>` sends the request to that peer on the signed `workload.ctl` mesh
wire (the transport placement uses; see the ADR-099 amendment of 2026-10-05). It
is the same request, run by the peer against its own token file. All of this must
hold:

1. You hold Admin on your daemon (the local CLI does).
2. The peer is in your `workload-peers.json` with tier `pinned` (rotation) or
   `pinned`/`paired` (status).
3. The peer serves `workload-host` (`workload-host.json` with `listen`) and lists
   **your node's key** under `controllers`.
4. The peer's `[dashboard]` is enabled and `allow_remote_rotate` is not `false`.

A node that is not on the mesh yet has to be added first (steps 2 and 3, one-time,
operator-approved); until then the CLI says which of them is missing. The request
is signed with your node key, expires, is replay-guarded, and is chained on both
nodes (`node_admin.*`). The answer is `{rotated, rotated_at, token_file}`, never a
token.

## Mesh install (ADR-108 P3b)

"Install on my machine" for a project whose primary node is a paired peer fetches
the project over the same signed mesh wire (`project.fetch`, a `git bundle` per
repository and a checksummed tar of the non-git content) and leaves each clone with
`origin` on a `weftos://` URL that `git-remote-weftos` serves incrementally. Access
is per project: the primary's `workload-peers.json` plus `project-fetch.json`,
default deny. The full description, the exclusion list and the operator steps are in
[`project-install.md`](project-install.md).

## Retiring the shell heartbeat

Once a node runs the reporter, remove its `weftos-dashboard-heartbeat.timer` and
`.service` and `heartbeat.sh` (in the Terraform environment:
`environments/dashboard_heartbeat.tf`) and keep the `node.token` file the reporter
now owns. Running both only doubles the heartbeats.
