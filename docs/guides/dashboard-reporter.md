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
