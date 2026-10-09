# ADR-114: `weftos://` names: internal mesh addressing for business concepts, mesh resources and artifacts

- **Status**: Proposed (2026-10-09; the owner chose the `weftos://` scheme and asked for this ADR)
- **Updated**: 2026-10-09. Owner: names are internal mesh addressing and cover companies, projects
  and other business concepts as well as mesh resources; the authority is the mesh, not a node.
  N1 built on `target-0.8.4`: `crates/clawft-weave/src/weftos_uri.rs` (strict parser, round-trip
  and reject tables), `mesh_names.rs` (accepts this node's MeshId and local aliases from
  `<runtime>/mesh-aliases.json`), `mesh_pairings.rs` (the member records which primary serves
  which projects). The root repository is named by the project itself, because `.` cannot be a
  segment.
- **Deciders**: owner
- **Prior art**: rvm ADR-157, "Capability-Governed `ruv://` Context Namespace" (ruvnet/rvm,
  MIT OR Apache-2.0, Proposed 2026-08-22), and ADR-158, its hosted service. This ADR adopts
  ADR-157's rules for WeftOS's own names. It does not depend on rvm, and it copies no code.
- **Builds on**: ADR-108 (pairing, per-project fetch grants, `git-remote-weftos`), ADR-112
  (agent teams; TM6 signed bundles), ADR-100 / ADR-105 (cogs and cog sources), ADR-025 (node
  identity is an Ed25519 key), the chain.

## Context

WeftOS names things in many ways: project ULIDs, cog ids and catalog entries, team ids,
memory stores, mesh node ids, and as of ADR-108 P3b the git remote URL
`weftos://<node>/<ULID>/<dir>`. Each feature also builds its own access check (pairing plus
per-project grant for fetch, the node-action payload rule that bans key, token and secret
field names, cog licence checkouts). The rules agree in spirit and differ in detail.

rvm's `ruv://` work (ADR-157) states the rule we keep re-deriving: **a name grants nothing.**
The capability travels separately, the check and its audit record come before any lookup,
and a pinned revision is a hash of the complete bytes. Agents make this urgent. An agent
acts on text it has just retrieved, so if naming a thing were enough to reach it, any string
that reaches the model could become an instruction.

`git-remote-weftos` claimed the `weftos://` scheme while being built. The scheme should
have a grammar before 0.8.4 ships, or the git URL becomes a released format that nothing
else fits.

## Decision

### 1. Scope: internal mesh addressing

`weftos://` names exist only inside a mesh. They are never published in the public cog
catalog, never resolved from outside the mesh, and never routed over the internet. A name
whose authority is not the resolving node's own mesh is refused with the same uniform error as
any other refusal (§3).

Names identify **things**, not locations. Which node holds a project, or serves a cog, is the
resolver's job (installations, pairing records, placement), so a name stays valid when the
thing moves.

**One thing, one name.** Relationships are data, never path. A project belongs to a company,
but the project's name is `projects/<ULID>`, never `companies/<id>/projects/<ULID>`, so no
resource ever has two names.

### 2. Grammar (version 1)

```text
weftos-uri = "weftos://" authority "/" kind "/" id [ "/" path ] [ "?" query ]
authority  = mesh-id | mesh-alias
mesh-id    = 64-lowercase-hex                  ; the licence MeshId (weft-licence-wire)
mesh-alias = lowercase DNS form, labels 1-63    ; a local alias the node maps to its MeshId
kind       = business-kind | mesh-kind | artifact-kind
business-kind = "companies" | "projects" | "goals" | "tickets" | "installations" | "members"
mesh-kind     = "nodes" | "hosts" | "services"
artifact-kind = "cogs" | "teams" | "agents" | "memory" | "sensors"
id         = kind-specific identifier (ULID, UUID, node id, slug)
path       = segment *( "/" segment )          ; kind-specific sub-resources only
query      = revision | view | revision "&" view
revision   = "rev=sha256:" 64-lowercase-hex
view       = "view=abstract" | "view=overview" | "view=content"
```

| Kind | Id | Sub-resources (v1) | Notes |
|---|---|---|---|
| `companies` | dashboard company UUID | none | clients, agencies, prospects |
| `projects` | WeftOS project ULID | `repos/<dir>` | git remote URLs: `projects/<ULID>` names the root repository, `projects/<ULID>/repos/<dir>` a sibling; the resolver finds the primary from pairing records |
| `goals`, `tickets` | dashboard UUID (imported `WEFT-N` stays a label, not an id) | none | board items |
| `installations` | installation UUID | none | project x host |
| `members` | member UUID | none | people; never an email address in a name |
| `nodes` | mesh node id (32 hex) | `services/<name>` | the mesh's own addressing |
| `hosts` | host UUID | none | the machine behind one or more nodes |
| `services` | service name | none | mesh-wide services (placement, licence proxy) |
| `cogs` | cog id | none; pin with `rev` | signed cog packages |
| `teams` | team id | none; pin with `rev` | ADR-112 bundles (TM6) |
| `agents` | agent id | none | registered or running agents |
| `memory` | store id | record path | per-scope indexes (§3) |
| `sensors` | sensor id | none | sensors reported by cogs and nodes |

Examples:

```text
weftos://<mesh>/projects/01K6ZQ8N3T4V5W6X7Y8Z9A0B1C               # git clone URL, root repository
weftos://<mesh>/projects/01K6ZQ8N3T4V5W6X7Y8Z9A0B1C/repos/app     # git clone URL, sibling repository
weftos://<mesh>/companies/7f3c…                                    # a company
weftos://<mesh>/nodes/<node id>/services/workload-host             # a service on a node
weftos://<mesh>/cogs/ld2450-spatial?rev=sha256:…                   # a pinned cog
weftos://<mesh>/teams/weftos-core?rev=sha256:…                     # a pinned team bundle
```

**Canonical means strict.** The rules follow ADR-157 §1. The parser never repairs input.
- **Characters:**
  - Only ASCII is accepted.
  - The scheme is exactly lowercase `weftos://`.
  - Every `%` is refused, so there is no percent-encoding at all.
- **Rejected outright:** fragments, userinfo, ports, and empty, trailing, `.` or `..` segments.
- **Authority:** the 64-hex MeshId, or a mesh alias in lowercase DNS form (labels of 1–63 characters from `[a-z0-9-]` that do not start or end with `-`).
- **Ids and segments:** each is 1–128 bytes of `[A-Za-z0-9._~-]`, at most 32 segments, and at most 2,048 bytes in total.
- **Query:**
  - `rev` comes before `view`.
  - Unknown or duplicate keys are refused.
  - Uppercase hex in `rev` is refused.
- **Round-trip:** parsing a valid string and serializing it reproduces exactly the same string.

### 3. A name is never authority

- A `weftos://` name carries no key, token, grant or credential. Authority comes from WeftOS's
  existing mechanisms, passed separately:
  - the signed node-admin request plus pairing and per-project grants for mesh access (ADR-108);
  - the caller's RPC capability locally;
  - cog licence checkouts (ADR-106) for cogs.
- **Authorization comes first.** Every resolution authorizes first and records the decision
  on the chain (allowed or refused) before any resolver, index or file is touched.
- **Refusals all look the same.** A refusal returns one error that is identical whether the
  name does not exist, is not granted, is revoked, or names another project. Existence must
  not leak through the error.
- **Search respects scope.** Search never runs over another scope's data and filters the
  results afterwards. Each scope has its own index.
- **Business records stay minimal in names.** Ids only: never an email address, a client name
  or other personal data in a name.

### 4. Pinned bytes, moving aliases

- A `rev` is the SHA-256 of the **complete** artifact bytes, such as the whole cog package or
  the whole team bundle. It is never a hash of one segment or a mutable path.
- A name without `rev` is an alias. An alias moves to a new revision only by compare-and-swap
  against the generation it last saw.
- Installing or executing anything (a cog, a team, an agent) requires a pinned `rev`. Reading
  a name never executes it. Execution goes through the existing verified launch paths: cog
  signature checks (COG-008) and, later, an rvm workload kind.

### 5. Relationship to `ruv://`

The two schemes are separate. WeftOS does not resolve `ruv://`. If WeftOS later hosts an rvm
context service or runs RVF agents through rvm, it will map between the two at that boundary.
The mapping will be explicit and recorded on the chain, and it will never be implicit.

### 6. Code

- One parser and serializer, `weftos_uri`, is shared by the git helper, the install handler,
  the dashboard (as a TypeScript twin with the same test vectors) and later resolvers. Kinds
  are an enum, so adding one is a reviewed change.
- The shared test vectors live with the parser. They include a round-trip set and a reject
  table, and every implementation must pass both.

## Consequences

- The git remote URL changes from `weftos://<node>/<ULID>/<dir>` to
  `weftos://<mesh>/projects/<ULID>/repos/<dir>` before 0.8.4 ships; the member's daemon
  resolves the primary from its pairing records. Nothing has been released
  with the old form.
- Team bundles (TM6), cog packages and memory records gain one citation form, and the
  dashboard can show and link names without them ever carrying access.
- Each new resolver has to meet §2: authorize and record before lookup, and give uniform
  refusals. This adds work, and it is the reason for the ADR.
- Business records (companies, members, tickets) become addressable inside the mesh. Names
  carry only ids, never emails, client names or other personal data.
- `ruv://` interop is a boundary mapping, so rvm's grammar can change while it is still
  Proposed without breaking WeftOS names.

## Phases

| Phase | Delivers | Done when |
|---|---|---|
| N1 | `weftos_uri` parser and serializer with vectors (all kinds parse); git remote URL migrated with mesh authority and primary resolved from pairing | `git-remote-weftos` accepts only the new form; round-trip and reject tables pass |
| N2 | Dashboard twin in TypeScript, using the same vectors; names shown on project, cog and team pages | the dashboard renders and copies `weftos://` names; vectors pass in both languages |
| N3 | Cog packages and team bundles cited by `rev` (with COG-008 and TM6) | install refuses an unpinned name; the chain records the pinned rev |
| N4 | A memory resolver with per-scope indexes and uniform refusals | negative tests: unscoped, revoked, other project, nonexistent all return the same error |

## Open questions for the owner

1. **Mesh aliases:** who sets a mesh's alias (the operator at mesh creation, or the dashboard
   workspace), and is it unique per licence?
2. **Cross-mesh references:** v1 refuses any name from another mesh. Do we ever need a project
   shared between two meshes (for example, a client's own mesh)?
