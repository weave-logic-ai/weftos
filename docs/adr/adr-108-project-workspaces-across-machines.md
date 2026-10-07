# ADR-108: Install projects locally (discover, install and configure working copies over the mesh)

- **Status**: Accepted (2026-10-05; scope and open questions decided by the owner the same day; implementation tracked on the board)
- **Updated**: 2026-10-06. P1 and P2 shipped in v0.8.3. Decision 1's `weft project init --adopt <ULID>`
  is built (ahead of the rest of P3): it writes `project.toml` and a manifest with
  `role = "workspace"` and no key, chain, certificate or `[serve]`, hides `.weftos/` in the
  repository's `.git/info/exclude` (never a tracked `.gitignore`), refuses a nested root or a
  second root for the same ULID, and the project supervisor refuses to serve a workspace
  (`project_is_workspace`). `--repo DIR` (repeatable) records sibling repositories of a
  workspace, which the reporter includes. `project.fetch`, the remote helper and pairing are
  still open.
- **Deciders**: owner
- **Builds on**: ADR-103 (projects as identities, project kernels, user daemon, machine mesh),
  ADR-099 (signed `workload.ctl` node-admin channel, amended 2026-10-05 for dashboard
  rotation), the dashboard installations model (`project_installations`,
  `installation_parameters`, `hosts`, `nodes`), the daemon `[dashboard]` reporter.

## Context

A project can now live on more than one machine: its canonical home on a server (for example
photo-gallery, one Linux account per project) and working copies on members' laptops. Today the
dashboard only knows installations that Terraform declares on servers. It does not know where a
member has a project checked out, and it cannot put a project onto a member's machine and set it
up there. This ADR is about **installing a project locally** next to its canonical home; it does
not move the project (owner, 2026-10-05: "install locally, not move").

Members' machines are mesh nodes (the user daemon with its own node key, pinned peers) and can
run the dashboard reporter. The signed `workload.ctl` node-admin channel already carries
operator requests between nodes with expiry, replay guard and chain records.

## Decision (proposed)

1. **One project identity, many installations.** A project keeps one ULID everywhere. Each
   machine holding it is an *installation* (project × host), with a role:
   - `primary` — the canonical home that runs the project's services and holds its chain;
     exactly one per project, and it stays where it is (normally the server).
   - `workspace` — a member's working copy (source + local state), any number.
   A copy is never a fork: `weft project init --adopt <ULID>` registers an existing identity on
   another machine. Forks keep using `--fork`.
2. **Discover from the daemon, not from configuration.** Every member's user daemon reports, in
   its heartbeat `observed` block, each registered project: ULID, root path, git remote,
   branch, head, dirty count, ahead/behind of the primary, last activity. The dashboard upserts a
   `workspace` installation per (project, host) from that. Nothing is reported for projects the
   daemon does not have registered.
3. **Commands go dashboard → node through the heartbeat; data goes node ↔ node over the mesh.**
   The dashboard cannot reach the tailnet, so a member's action ("install locally", "update",
   "remove") is queued as a `node_action` addressed to that member's node and delivered
   in the heartbeat response. The node acknowledges and reports results on later beats. Source
   and state never pass through the dashboard.
4. **Transfer over the mesh, authorised per project.** The node that owns the primary serves
   `project.fetch` on its `workload-host` (node-admin method):
   - git repositories through a git remote helper (`git-remote-weftos`, URLs
     `weftos://<node>/<ULID>/<repo>`), so clone and later pulls are incremental;
   - non-git folders as a streamed, checksummed tar.
   The serving node answers paired nodes of WeaveLogic members (D-A below); the owner can revoke
   a machine. Each fetch is chained on both nodes.
   All bytes travel inside the mesh's Noise XX session between the two node keys (mutually
   authenticated, encrypted end to end, forward-secret): no SSH, no separate tunnel, no shared
   secret.
5. **Configure from the project, with consent.** A project may declare `.weftos/setup.toml`
   (toolchain checks, env templates, post-install commands). The installing node shows the plan
   in the dashboard and runs it only after the member confirms. Secrets are never copied; the
   setup declares which secrets it needs and where the member supplies them.
6. **Key exchange over the mesh, approved in the dashboard.** Pairing two nodes for project
   work must not mean hand-editing `workload-peers.json` and `workload-host.json` on each side.
   A node asks to pair (`mesh.pair.request`, carrying its node key and the member's identity);
   the dashboard shows the request with the key fingerprint and project scope; on approval the
   decision is delivered to both nodes (as a `node_action` over their heartbeats) and each
   writes the other's key into its trust files at the agreed tier (`pinned` for a member's own
   machines, controller entries scoped to the approved projects). Revocation is the same path
   in reverse and takes effect on the next beat. The fingerprint is shown on both machines so a
   member can compare it out of band.

## Phases

| Phase | Delivers | Check |
|---|---|---|
| P1 Discover | Reporter `observed` gains per-project workspace facts; dashboard upserts `workspace` installations; project panel lists "where it's installed" per member/host. Members' Macs run the reporter. | A member's checkout of a project appears on that project's panel within one heartbeat, with path, branch, head, dirty. |
| P2 Actions | `node_actions` table + RLS (members), queue/ack/result over the heartbeat; dashboard buttons. | An action queued in the dashboard reaches the node on its next beat and its result shows in the panel. |
| P2b Pairing | `mesh.pair.request`, dashboard approval with fingerprints, trust files written on both nodes, revocation. | A new member Mac pairs with the primary's node from the dashboard with no file edits; revoking it stops `project.fetch` on the next beat. |
| P3 Install | `project.fetch` node-admin method, `git-remote-weftos`, tar stream, per-project access list, `weft project init --adopt`. | "Install on my machine" clones every repo of a project from its primary over the mesh into a chosen path and registers the same ULID; the panel shows the new workspace. |
| P4 Configure | `.weftos/setup.toml`, plan preview and confirm in the dashboard, secret placeholders. | A project with a setup file installs and configures end to end after one confirmation; no secret leaves its machine. |

## Consequences

- The dashboard becomes the place to see every copy of every project, not only server installs.
- Client data moves only between WeftOS nodes over the mesh, under per-project access lists, with
  chain records on both ends; the dashboard sees metadata, never content.
- Members' machines must run the user daemon with the reporter and be admitted to the mesh.
- New surface: `project.fetch` and the remote helper are data-exfiltration paths if access lists
  are wrong; they default to deny and need negative tests (unlisted node, revoked node, other
  project's ULID).

## Out of scope

- Moving a project's primary home between machines. The primary stays put; local installs are
  working copies that pull from it (and push back through the project's normal git remotes).

## Owner decisions (2026-10-05)

- **D-A Access:** any confirmed WeaveLogic member's paired machine may install any project
  locally (mirrors dashboard access). Revoking a machine removes it on the next beat.
- **D-B Default path:** `~/Projects/<slug>`; the member may choose another path per install.
- **D-C What installs:** a local install fetches everything in the project. Content that should
  not be downloaded must be **marked for archiving** in the project manifest
  (`.weftos/archive.toml`: paths with a reason); marked paths move to the project's archive
  area on the primary (`<data_dir>/archive/`), are listed (name, size, reason) in the dashboard,
  and are never fetched by local installs. Nothing is silently skipped: an install of a project
  with unmarked very large content warns before fetching.
