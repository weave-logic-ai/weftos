# Dashboard → project console (cog manager per project)

- **Status:** Proposed, 2026-10-05. Owner request: the WeftOS dashboard opens the WeftOS
  cog manager in the context of a project (first `weftos`, then every project), and shows
  every parameter we create for a project (account, UID/GID, home, paths, ULID, gateway,
  mesh, nodes, units).
- **Related:** `docs/plans/dashboard-fleet-terraform-integration.md` (entities, binding
  rule, "no host token in the dashboard DB or browser storage"),
  `docs/plans/coordinator-move-to-pg.md` (PG is the primary node; gateway on the tailnet),
  ADR-102 (gateway), ADR-103 (projects as child kernels).

## Today (research 2026-10-05, file:line evidence in the session report)

- **Cog manager** (`crates/weftos-cog-manager/src/client/settings.rs:35-106`): settings
  come from env (native) or `?host= ?token= ?gw= ?gwtoken= ?seeds= ?wl= ?cog=` (web). No
  project parameter, no fragment parsing, no project filter; the only "project" is the
  `placement.project_id` display field in the fleet snapshot (`views/fleet.rs:114-139`).
- **Gateway**: refuses tokens that carry a project (`clawft-services/src/api/auth.rs:337-358`);
  read tokens reach only a fixed GET list (`auth.rs:46-69`); no `project.*` HTTP routes
  (`clawft-kernel/src/http_facade.rs:353-420`); `weft gateway` serves no static UI
  (`clawft-cli/src/commands/gateway.rs:141-148`); CSP `connect-src 'self'` and
  `frame-ancestors 'none'` (`api/middleware.rs:57-68`).
- **Dashboard** (`~/dev/weftos-dashboard` @ `fdc40c4`): `projects` has id, workspace,
  name, slug, description, kind, company; `nodes.last_report` jsonb is the only free-form
  field. Single page; a project only filters the board. No link to any WeftOS surface.
  Credential tables hold hashes of dashboard-minted tokens only.
- **Parameters** exist only in `~/weavelogic.terraform` (`inventory/hosts.yaml`, locals
  in `environments/main.tf`) and the plan docs. Terraform outputs only account
  name/uid/gid/home.

## Design

One record per project, two kinds of facts, one launch action.

- **Desired facts** (what we created): host, tailnet address, Linux account, UID/GID,
  home, project root, data/log/runtime dirs, WeftOS project ULID, gateway URL, mesh
  address, Cog Host URL/port, unit names, release. Source of truth: Terraform. Exported
  as `terraform output -json` and synced into the dashboard. Never secrets.
- **Observed facts** (what is running): node ids, mesh listen address, project child
  state, versions, unit active state, last seen. Source: the WeftOS daemon (heartbeat
  report and gateway read routes).
- **Launch**: an "Open cog manager" button on the project's dashboard page opens a new
  tab to the cog manager served by that project's host over the tailnet, with the
  project's ULID and endpoints in the URL and the token passed in the fragment (never a
  query string, never stored by the dashboard).

## Slices

1. **Terraform projects block and outputs** (`~/weavelogic.terraform`). Move the `weftos`
   project's parameters from locals into an inventory `projects:` block (per host:
   account, ULID, root, data/log/runtime dirs, gateway, mesh, Cog Host port, units) and
   emit them as outputs. Check: `terraform output -json projects` lists `weftos` with every
   field above and no secret; plan shows no resource change.
2. **Dashboard schema and sync** (`weftos-dashboard`). Tables `project_installations`
   (dashboard project UUID ↔ host + WeftOS ULID, explicit binding, never inferred from a
   slug) and `installation_parameters` (desired, with source and time); observed values
   from the node heartbeat. RLS via `owns_workspace`. A sync script upserts the Terraform
   output through an authenticated server route. Check: migration plus RLS negative tests;
   the `weftos` project shows its PG installation.
3. **Dashboard project page** (`weftos-dashboard`). A per-project detail view: parameters
   panel (desired vs observed side by side, with source and age), nodes, units, and the
   "Open cog manager" button. Every new project gets the same page from its records.
   Check: the `weftos` page shows the PG facts listed in the research table.
4. **Cog manager project context** (`weftos-cog-manager`). Accept `?project=<ULID>`;
   read tokens from the fragment (`#gwtoken=`, `#token=`) and strip them from the address
   bar; filter Network and Cogs by the project and select that project's Cog Host; show
   the project name and ULID in the top bar. Check: unit tests for parsing; opening with a
   project shows only that project's instances.
5. **Gateway: serve the console and project reads** (`weft gateway`). Serve the cog
   manager web build (`--static-dir`), with CSP `connect-src` extended by config to the
   project's Cog Host and Seeds; add read routes `/api/projects`, `/api/projects/{ulid}`
   (project.show/status) and a project filter on `/api/fleet/snapshot`; allow a read
   token scoped to one project. Check: tests for the new routes and the scope; a read
   token for project A cannot read project B.
6. **Token handoff** (decision below). Until it lands, the console asks for the read
   token once per session; the dashboard never sees it.
7. **Cog Host per project on PG** (`deploy/photo-gallery/`, Terraform). Deploy
   `weftos-cog-host@weftos.service` on its own port; fix the template's
   `WorkingDirectory` (still the old `/srv/weftos/projects/%i`). Check: unit active, the
   console's Cogs tab for `weftos` lists its cogs.

## Decision needed before slice 6

How the console gets a token when launched from the dashboard:

- **A. Tailnet identity (recommended).** The PG gateway trusts the caller's Tailscale
  identity (`tailscale whois` on the peer address, allowlisted logins) and mints a
  short-lived read token for the requested project. Nothing secret leaves PG; no token
  in the dashboard.
- **B. Dashboard broker.** A Vercel server route asks PG for a short-lived token after
  Supabase auth. Needs Vercel → tailnet reachability and a PG credential stored in Vercel.
- **C. Manual.** The console prompts for a token minted with `weft token issue`
  (max 24 h). No new code, more friction.
