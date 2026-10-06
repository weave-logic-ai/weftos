# ADR-108: Project workspaces across machines (discover, install, configure, migrate over the mesh)

- **Status**: Proposed (2026-10-05)
- **Deciders**: owner
- **Builds on**: ADR-103 (projects as identities, project kernels, user daemon, machine mesh),
  ADR-099 (signed `workload.ctl` node-admin channel, amended 2026-10-05 for dashboard
  rotation), the dashboard installations model (`project_installations`,
  `installation_parameters`, `hosts`, `nodes`), the daemon `[dashboard]` reporter.

## Context

A project can now live on more than one machine: its canonical home on a server (for example
photo-gallery, one Linux account per project) and working copies on members' laptops. Today the
dashboard only knows installations that Terraform declares on servers. It does not know where a
member has a project checked out, it cannot put a project onto a member's machine, and there is
no supported way to move a project's primary home between machines.

Members' machines are mesh nodes (the user daemon with its own node key, pinned peers) and can
run the dashboard reporter. The signed `workload.ctl` node-admin channel already carries
operator requests between nodes with expiry, replay guard and chain records.

## Decision (proposed)

1. **One project identity, many installations.** A project keeps one ULID everywhere. Each
   machine holding it is an *installation* (project × host), with a role:
   - `primary` — the canonical home that runs the project's services and holds its chain;
     exactly one per project.
   - `workspace` — a member's working copy (source + local state), any number.
   A copy is never a fork: `weft project init --adopt <ULID>` registers an existing identity on
   another machine. Forks keep using `--fork`.
2. **Discover from the daemon, not from configuration.** Every member's user daemon reports, in
   its heartbeat `observed` block, each registered project: ULID, root path, git remote,
   branch, head, dirty count, ahead/behind of the primary, last activity. The dashboard upserts a
   `workspace` installation per (project, host) from that. Nothing is reported for projects the
   daemon does not have registered.
3. **Commands go dashboard → node through the heartbeat; data goes node ↔ node over the mesh.**
   The dashboard cannot reach the tailnet, so a member's action ("install here", "update",
   "migrate primary") is queued as a `node_action` addressed to that member's node and delivered
   in the heartbeat response. The node acknowledges and reports results on later beats. Source
   and state never pass through the dashboard.
4. **Transfer over the mesh, authorised per project.** The node that owns the primary serves
   `project.fetch` on its `workload-host` (node-admin method):
   - git repositories through a git remote helper (`git-remote-weftos`, URLs
     `weftos://<node>/<ULID>/<repo>`), so clone and later pulls are incremental;
   - non-git folders as a streamed, checksummed tar.
   The serving node answers only nodes on the project's access list (owner-set; members of the
   workspace may request access, which the owner approves). Each fetch is chained on both nodes.
   All bytes travel inside the mesh's Noise XX session between the two node keys (mutually
   authenticated, encrypted end to end, forward-secret): no SSH, no separate tunnel, no shared
   secret.
7. **Key exchange over the mesh, approved in the dashboard.** Pairing two nodes for project
   work must not mean hand-editing `workload-peers.json` and `workload-host.json` on each side.
   A node asks to pair (`mesh.pair.request`, carrying its node key and the member's identity);
   the dashboard shows the request with the key fingerprint and project scope; on approval the
   decision is delivered to both nodes (as a `node_action` over their heartbeats) and each
   writes the other's key into its trust files at the agreed tier (`pinned` for a member's own
   machines, controller entries scoped to the approved projects). Revocation is the same path
   in reverse and takes effect on the next beat. The fingerprint is shown on both machines so a
   member can compare it out of band.
5. **Configure from the project, with consent.** A project may declare `.weftos/setup.toml`
   (toolchain checks, env templates, post-install commands). The installing node shows the plan
   in the dashboard and runs it only after the member confirms. Secrets are never copied; the
   setup declares which secrets it needs and where the member supplies them.
6. **Migrate the primary as a handover.** Moving `primary` from host A to host B: B fetches the
   latest state, A stops and seals its chain head, B adopts the project kernel with a chain
   record linking to A's head (reusing ADR-103's migration and anchor rules), roles swap, A
   becomes a `workspace` or is retired. Never two primaries; a failed handover leaves A primary.

## Phases

| Phase | Delivers | Check |
|---|---|---|
| P1 Discover | Reporter `observed` gains per-project workspace facts; dashboard upserts `workspace` installations; project panel lists "where it's installed" per member/host. Members' Macs run the reporter. | A member's checkout of a project appears on that project's panel within one heartbeat, with path, branch, head, dirty. |
| P2 Actions | `node_actions` table + RLS (members), queue/ack/result over the heartbeat; dashboard buttons. | An action queued in the dashboard reaches the node on its next beat and its result shows in the panel. |
| P2b Pairing | `mesh.pair.request`, dashboard approval with fingerprints, trust files written on both nodes, revocation. | A new member Mac pairs with the primary's node from the dashboard with no file edits; revoking it stops `project.fetch` on the next beat. |
| P3 Install | `project.fetch` node-admin method, `git-remote-weftos`, tar stream, per-project access list, `weft project init --adopt`. | "Install on my machine" clones every repo of a project from its primary over the mesh into a chosen path and registers the same ULID; the panel shows the new workspace. |
| P4 Configure | `.weftos/setup.toml`, plan preview and confirm in the dashboard, secret placeholders. | A project with a setup file installs and configures end to end after one confirmation; no secret leaves its machine. |
| P5 Migrate | Primary handover with chain linkage; role swap in the dashboard. | Moving primary between two nodes keeps one primary at every point, links the chains, and is reversible. |

## Consequences

- The dashboard becomes the place to see every copy of every project, not only server installs.
- Client data moves only between WeftOS nodes over the mesh, under per-project access lists, with
  chain records on both ends; the dashboard sees metadata, never content.
- Members' machines must run the user daemon with the reporter and be admitted to the mesh.
- New surface: `project.fetch` and the remote helper are data-exfiltration paths if access lists
  are wrong; they default to deny and need negative tests (unlisted node, revoked node, other
  project's ULID).

## Open questions

- Access model: owner approves each member per project, or "all WeaveLogic members may fetch
  any project" (mirrors dashboard access)?
- Should large non-git trees (datasets, model files) transfer at all, or stay server-only with a
  manifest?
- Local root convention for installs (`~/Clients/<company>/<project>`, `~/Projects/<slug>`, or
  member's choice per install)?
