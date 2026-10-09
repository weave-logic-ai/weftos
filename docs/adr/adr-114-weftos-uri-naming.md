# ADR-114: `weftos://` names: one inert namespace for projects, cogs, teams, memory and agents

- **Status**: Proposed (2026-10-09; the owner chose the `weftos://` scheme and asked for this ADR)
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

### 1. Grammar (version 1)

```text
weftos-uri = "weftos://" authority "/" kind "/" id [ "/" path ] [ "?" query ]
authority  = mesh node id | mesh or workspace name (lowercase DNS form)
kind       = "projects" | "cogs" | "teams" | "memory" | "agents"
id         = kind-specific (project ULID, cog id, team id, store id, agent id)
path       = segment *( "/" segment )          ; kind-specific sub-resources
query      = revision | view | revision "&" view
revision   = "rev=sha256:" 64-lowercase-hex
view       = "view=abstract" | "view=overview" | "view=content"
```

The canonical forms in use or planned:

| Name | Meaning |
|---|---|
| `weftos://<node>/projects/<ULID>/repos/<dir>` | A project repository on its primary; the git remote URL (`git-remote-weftos`, ADR-108 P3b). |
| `weftos://<node>/projects/<ULID>` | The project itself (its manifest and non-git content). |
| `weftos://<mesh>/cogs/<cog-id>?rev=sha256:…` | A pinned, signed cog package. |
| `weftos://<mesh>/teams/<team-id>?rev=sha256:…` | An ADR-112 team bundle (TM6). |
| `weftos://<node>/memory/<store-id>/<path>` | A record in a memory store. |
| `weftos://<node>/agents/<agent-id>` | A running or registered agent. |

**Canonical means strict.** The rules follow ADR-157 §1. The parser never repairs input.
- **Characters:**
  - Only ASCII is accepted.
  - The scheme is exactly lowercase `weftos://`.
  - Every `%` is refused, so there is no percent-encoding at all.
- **Rejected outright:** fragments, userinfo, ports, and empty, trailing, `.` or `..` segments.
- **Authority:** lowercase DNS form, 1–253 bytes, with labels of 1–63 characters from `[a-z0-9-]` that do not start or end with `-`. A 32-hex mesh node id is a valid label.
- **Ids and segments:** each is 1–128 bytes of `[A-Za-z0-9._~-]`, at most 32 segments, and at most 2,048 bytes in total.
- **Query:**
  - `rev` comes before `view`.
  - Unknown or duplicate keys are refused.
  - Uppercase hex in `rev` is refused.
- **Round-trip:** parsing a valid string and serializing it reproduces exactly the same string.

### 2. A name is never authority

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

### 3. Pinned bytes, moving aliases

- A `rev` is the SHA-256 of the **complete** artifact bytes, such as the whole cog package or
  the whole team bundle. It is never a hash of one segment or a mutable path.
- A name without `rev` is an alias. An alias moves to a new revision only by compare-and-swap
  against the generation it last saw.
- Installing or executing anything (a cog, a team, an agent) requires a pinned `rev`. Reading
  a name never executes it. Execution goes through the existing verified launch paths: cog
  signature checks (COG-008) and, later, an rvm workload kind.

### 4. Relationship to `ruv://`

The two schemes are separate. WeftOS does not resolve `ruv://`. If WeftOS later hosts an rvm
context service or runs RVF agents through rvm, it will map between the two at that boundary.
The mapping will be explicit and recorded on the chain, and it will never be implicit.

### 5. Code

- One parser and serializer, `weftos_uri`, is shared by the git helper, the install handler,
  the dashboard (as a TypeScript twin with the same test vectors) and later resolvers. Kinds
  are an enum, so adding one is a reviewed change.
- The shared test vectors live with the parser. They include a round-trip set and a reject
  table, and every implementation must pass both.

## Consequences

- The git remote URL changes from `weftos://<node>/<ULID>/<dir>` to
  `weftos://<node>/projects/<ULID>/repos/<dir>` before 0.8.4 ships. Nothing has been released
  with the old form.
- Team bundles (TM6), cog packages and memory records gain one citation form, and the
  dashboard can show and link names without them ever carrying access.
- Each new resolver has to meet §2: authorize and record before lookup, and give uniform
  refusals. This adds work, and it is the reason for the ADR.
- `ruv://` interop is a boundary mapping, so rvm's grammar can change while it is still
  Proposed without breaking WeftOS names.

## Phases

| Phase | Delivers | Done when |
|---|---|---|
| N1 | `weftos_uri` parser and serializer with vectors; git remote URL migrated | `git-remote-weftos` accepts only the new form; round-trip and reject tables pass |
| N2 | Dashboard twin in TypeScript, using the same vectors; names shown on project, cog and team pages | the dashboard renders and copies `weftos://` names; vectors pass in both languages |
| N3 | Cog packages and team bundles cited by `rev` (with COG-008 and TM6) | install refuses an unpinned name; the chain records the pinned rev |
| N4 | A memory resolver with per-scope indexes and uniform refusals | negative tests: unscoped, revoked, other project, nonexistent all return the same error |

## Open questions for the owner

1. **Authority for shared things:** do cogs and teams use a mesh name (needs a naming
   registry) or a publisher name, for example `weftos://weavelogic/cogs/...`?
2. **Should `weftos://` names appear in the public catalog** (cogs) or only inside a mesh?
