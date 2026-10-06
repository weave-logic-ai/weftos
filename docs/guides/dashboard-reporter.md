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

## Actions (ADR-108 P2)

The dashboard's answer to a heartbeat may carry work for this node:

```json
{"ok": true, "actions": [{"id": "<id>", "kind": "install", "payload": {}, "created_at": "..."}]}
```

Kinds are `install`, `update`, `remove` and `pair`. An answer without `actions`
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

This build does not run anything from an action. `install`, `update`, `remove`
and `pair` answer `failed` with `{"error": "not implemented in this build (ADR-108
P3/P4)", "kind": "<kind>"}`, and an unknown kind answers `failed` with
`{"error": "unknown action kind \"<kind>\"", "kind": "<kind>"}`. The seam for the
real ones is the `ActionHandler` trait in `crates/clawft-weave/src/dashboard_actions.rs`
(`Dashboard::set_action_handler`). Handlers must not trust `payload`: it is
whatever the dashboard sent, and it is never logged or kept in the action log.

```bash
weaver dashboard actions          # recent actions and their outcomes (local, Read)
weaver dashboard actions --json
```

RPC: `dashboard.actions` (Read, local only). `dashboard.status` adds
`actions_recorded` and `actions_queued`.

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

## Retiring the shell heartbeat

Once a node runs the reporter, remove its `weftos-dashboard-heartbeat.timer` and
`.service` and `heartbeat.sh` (in the Terraform environment:
`environments/dashboard_heartbeat.tf`) and keep the `node.token` file the reporter
now owns. Running both only doubles the heartbeats.
