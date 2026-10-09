# ADR-108 P2b + P3: wire contract between the lanes (2026-10-08)

Four lanes build pairing (P2b), git-remote install (P3a) and mesh install (P3b)
at the same time. This file fixes the shapes they meet at. Change it only with
the lead's agreement, and record the change here.

The code half of the install contract is
[`crates/clawft-weave/src/project_install.rs`](../../crates/clawft-weave/src/project_install.rs)
(`InstallRequest`, `ProjectFetcher`, `fetch_order`).

## Rule that binds every payload

The dashboard refuses any `node_actions.payload` whose field names contain
`token`, `secret`, `key` or `password` (migration `202610050010_node_actions.sql`).
So: `peer_node`, never `node_key`; `fingerprint`, never `pubkey`.

## 1. The node's mesh identity in the heartbeat (P2b lane, Rust)

Every beat carries `report.mesh_identity` (omitted when the mesh is off):

```json
"mesh_identity": {"node": "<mesh node id, as in workload-peers.json>",
                  "fingerprint": "<first 16 hex of SHA-256 of the Ed25519 public key>",
                  "advertise": "100.90.170.87:9470"}
```

`advertise` is the address peers should dial (tailnet first; never a loopback
address). The dashboard reads it from `nodes.last_report`; no migration.

## 2. Pairing (P2b)

**Request.** `weaver mesh pair request --with <dashboard node name|mesh node id> [--project <ULID>...]`
records a pending request on this node, and an install whose `primary` is not a
paired peer records one automatically. Each beat carries the pending requests
(at most 8):

```json
"pair_requests": [{"request_id": "<uuid v4>", "with_node": "<mesh node id of the other side>",
                   "projects": ["<ULID>"], "requested_at": "<rfc3339>"}]
```

**Dashboard.** The heartbeat ingest upserts these into `pair_requests`
(workspace, requesting dashboard node, request_id, with_node, projects, status
`pending|approved|rejected|revoked`). The Installations page lists pending
requests with **both** fingerprints (from each node's `mesh_identity`) and the
project scope; a member approves or rejects. Approval queues one `pair` action
to each node.

**`pair` action payload** (to each side):

```json
{"op": "add" | "remove",
 "request_id": "<uuid>",
 "peer_node": "<the other side's mesh node id>",
 "fingerprint": "<the other side's fingerprint>",
 "advertise": "<the other side's advertise address>",
 "role": "primary" | "member",
 "projects": ["<ULID>"]}
```

`role` is the other side's role. The node checks that `fingerprint` matches
`peer_node` before writing anything. On `add`:

- the **member** side writes the primary into `workload-peers.json` at tier
  `pinned`, with `advertise` as its address;
- the **primary** side writes the member into `workload-peers.json` at tier
  `paired` and adds a fetch grant (below). It never makes the member a
  controller.

`remove` undoes both and takes effect when the action runs. Every add or remove
is chained (`mesh.pair.add`, `mesh.pair.remove`) with the request id and the
dashboard action id. The result is `{"peer_node", "fingerprint", "tier",
"projects"}`; no key material.

## 3. Fetch grants (written by P2b, enforced by P3b)

`project-fetch.json`, next to `workload-peers.json`, mode 0600:

```json
{"version": 1,
 "grants": [{"peer_node": "<mesh node id>", "projects": ["<ULID>"],
             "granted_at": "<rfc3339>", "source": "dashboard-pair:<action id>"}]}
```

Default deny: `project.fetch` serves a request only when the calling peer is in
`workload-peers.json` (tier `pinned` or `paired`) **and** a grant names that
peer and that project. Removing the peer or the grant stops the next fetch.

## 4. Install (P3a, P3b and the dashboard)

The dashboard's `install` payload becomes:

```json
{"project_ulid": "<ULID>", "target_path": "~/Projects/<slug>",
 "sources": [{"url": "https://github.com/org/repo.git", "branch": "main", "dir": "."}],
 "primary": {"node_id": "<mesh node id of the project's primary installation>"}}
```

`sources` come from the project's `source.<i>.url|branch|dir` rows (dashboard
PR #15). `primary` is present when the project has an installation with role
`primary` whose node reports a `mesh_identity`. Either may be absent, but not
both.

The install handler (P3a) validates the payload (`InstallRequest::validate`),
picks a fetcher with `fetch_order` (mesh first, then git-remote), fetches into
an absent or empty `target_path`, then runs `weft project init --adopt <ULID>`
with a `--repo` for each sibling. The result is:

```json
{"fetcher": "mesh" | "git-remote", "root": "<absolute path>",
 "repos": [{"dir": ".", "head": "1a2b3c4d", "remote": "https://..."}],
 "bytes": 0, "archived": []}
```

**Layout.** The `.` source clones into `target_path`. A sibling `dir` clones
into `<parent of target_path>/<dir>` and is registered with `--repo`. With no
`.` source every repository clones into `<target_path>/<dir>` and `target_path`
itself is a plain directory (the workspace root; its repositories are found one
level below it, so no `--repo` is passed). Every destination, and `target_path`,
must be absent or an empty directory before anything is written; two sources
whose destinations coincide are refused. On any failure (fetch or adopt) the
install removes only what it created: destinations that were absent, the
contents of empty directories it filled, and parent directories it made. The
git-remote fetcher runs `git` with an argument vector (no shell),
`GIT_TERMINAL_PROMPT=0`, `protocol.file.allow=never`, `protocol.ext.allow=never`,
`core.fsmonitor=false`, a 15 minute limit per repository and a cap on the output
it keeps. Registration execs the `weft` binary next to `weaver` (then `PATH`)
in `target_path` with `HOME` and `WEFTOS_MANIFESTS_DIR` set from the daemon's
own paths.

`update` runs `git pull --ff-only` in each registered repository of that
project. `remove` unregisters the workspace and **never deletes files**: the
result names the path so the member deletes it by hand.

## 5. Mesh fetch (P3b)

- `project.fetch` is a node-admin method on the primary's `workload-host`, gated
  by section 3.
- `git-remote-weftos` (URL `weftos://<mesh node id>/<ULID>/<dir>`) speaks git's
  remote-helper protocol over that method, so clone and later pulls are
  incremental.
- Non-git content travels as a checksummed tar stream. Paths in the project's
  `.weftos/archive.toml` are never sent and are listed in `archived`.
- Every fetch is chained on both nodes (`project.fetch`).

The negative tests ADR-108 requires: an unlisted peer, a revoked peer, another
project's ULID, a path outside the project, and an archived path.
