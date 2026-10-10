# ADR-098: Per-project process-compose; environment pane is planned only

- **Status**: Draft (Proposed)
- **Updated**: 2026-10-10. ADR-116 builds the query-and-route half of the deferred overlay
  (many process-compose instances plus one pane, no YAML merge); §2 is amended by it.
- **Date**: 2026-08-16
- **Deciders**: Pending (platform / ops)
- **Depends-On**: ADR-096 (MetaHarness), ADR-070 (MCP registry ownership)
- **Related**: ADR-040 (process table), ADR-021 (CLI through kernel),
  `docs/guides/agent-harness-triple-loop.md`

## Context

Forge and WeftOS both need durable YAML for local servers and agent jobs.
process-compose v1.120.0 does that well **inside one repo**: health,
namespaces, disabled MCP job tools, `pc_*` control tools, SSE.

The next ask is a **single pane** for the whole machine: discover every
`process-compose.yaml`, merge them, manage LLM testers, kernels, docs
sites, and port maps from one API.

Official merge (`-f` / `extends`) is **docker-compose shaped**, not
terraform-module shaped:

- Same process name → last file wins (namespaces do not uniquify).
- Paths are relative to the **first** `-f` file, not each repo.
- `.env` loads from CWD only.
- One HTTP API and at most one `mcp_server` per running instance.
- `disabled: false` in an override is unreliable; use `is_disabled: "false"`.

A dry-run on 2026-08-16 merging two project files that both defined
`web` validated **3** processes, not 4.

## Decision

### 1. Per-project process-compose is the unit

Each repo runs **its own** process-compose (or none). WeftOS uses
`:18090` / `:18091`. Forge uses `:18080` / `:18081`. Product kernel
stays on `:8080`.

### 2. Do not build the environment overlay yet

No scanner, no home-level `up` that `-f`s every yaml, no shared TUI
that owns every port. Building that now would collide names and ports
and start the wrong processes.

### 3. Required contract (land now, overlay later)

A project may set `compose/manifest.yaml` `overlay_ready: true` only
when:

| field | rule |
|---|---|
| `id` | Unique slug (`weftos`, `forge-krause`) |
| `prefix` | Every process name starts with it (`weft-`, `forge-`) |
| `pc_http` / `pc_mcp` | Unique on this machine; listed in `compose/ports.yaml` |
| `working_dir` | Absolute in every process |
| `mcp_server` | Absent in the leaf, **or** the reserved unique port |
| `overlay_ready` | Default `false` |

The future overlay is a **registry + rewriter**, not a raw `-f` glob:

1. Read `~/.weftos/compose/registry.yaml` (or WeftOS config) of project roots.
2. Load each `compose/manifest.yaml`.
3. Refuse any project that is not `overlay_ready`.
4. Rewrite names/ports if needed, then merge **or** attach many PC
   instances behind one query API.

Prefer **many PC instances + one query pane** if merge still loses
identity. Prefer **one rewritten merge** only after a fixture suite
proves name/port/`working_dir` isolation.

### 4. Jobs stay in YAML

Agent workflow jobs (`job_score`, `job_gate`, `job_plane_ready`, …)
live in the project's yaml with `disabled: true` and `mcp.type: tool`.
No ad-hoc Python for those jobs.

## Consequences

### Positive

- WeftOS can run the triple loop and its own servers without fighting
  Forge's PC on `:18081`.
- Overlay, when built, has a fail-closed admission gate.

### Negative

- Operators attach to more than one TUI/API until the pane exists.
- Leaf yamls must be edited to become overlay-ready (prefixes, abs paths).

### Neutral

- process-compose remains the per-repo engine. WeftOS kernel process
  table (ADR-040) is a different plane (in-kernel agents, not host
  subprocesses).

## Follow-ups

- [x] WeftOS `process-compose.yaml` + `compose/{manifest,desired,ports}.yaml`
- [x] Guide: `docs/guides/agent-harness-triple-loop.md`
- [ ] Registry schema under `config/` when a second overlay-ready project exists
- [ ] Fixture: merge two prefixed yamls without name loss
- [ ] Optional: `weaver compose` as the query pane (does not spawn a second scheduler)
