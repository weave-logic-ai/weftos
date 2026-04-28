---
name: plane-workflow
description: >-
  Authoritative Plane work-tracker discipline for WeftOS / clawft. Use whenever
  you create, claim, finish, or defer a meaningful unit of work, or when
  triaging audits / TODOs / FIXMEs / orphans into Plane work items. Codifies
  the lifecycle (Backlog → Todo → In Progress → Done | Cancelled), the cycle
  taxonomy (0.7.x must-ship, 0.8.x / 0.9.x / 1.0.x deferred), and the HTTP
  API workaround for the partially-broken MCP server.
---

<!-- TOC: Why | The Rule | IDs | Lifecycle | HTTP API | Triage | Templates | References -->

# plane-workflow — Plane is the authoritative tracker

> **Project rule**: every meaningful unit of work for WeftOS / clawft goes
> through a Plane work item in the `weftos` workspace. State must reflect
> reality. No silent closures. Deferred work moves cycles with a reason.

This skill is the on-ramp for that rule. It gives you the cached IDs, the
HTTP API patterns (the MCP server's `list_*` endpoints currently return
HTTP 404 — see `references/api-cheatsheet.md`), and the conventions for
turning an audit row, a code TODO, or an in-flight discovery into a Plane
work item that a future agent / human can pick up cold.

---

## 1. Why this exists

Pre-2026-04-28 we tracked work in markdown sprint trackers, ad-hoc TODO
files, and `.planning/` notes. The 0.7.0 release-gate audit at
`.planning/reviews/0.7.0-release-gate/` exposed ~430 open items spread
across 17 workstreams with no single source of truth, several
"complete" trackers that were lying (channels 9/9 ≠ reality, browser
WASM 6/6 ≠ wired), and stale audit rows for things already fixed in
git. Plane is now the single tracker.

`docs/handoff.md` (top section, "New project rule") is the authoritative
written rule. This skill is the operational manual.

---

## 2. The Rule (verbatim from handoff)

- **New items**: when a TODO is identified (audit, code review, user
  request, in-flight discovery), create a Plane work item in the
  appropriate cycle (`0.7.x` for must-ship-before-0.7, `0.8.x`+ for
  later). Include: file path / source citation, acceptance criteria,
  any dependencies, link back to source-of-truth doc.
- **Items being worked on**: transition to **In Progress** on claim,
  **before** starting code. The state must reflect reality.
- **Items finished**: close with details — what shipped, the commit
  SHA, any follow-up items spawned during the work, tests / build
  status. No silent closures.
- **Items deferred**: move to a later cycle with an explicit reason
  in the comment (blocked by upstream, scope-cut, superseded by
  another item).

---

## 3. Cached IDs (workspace / project / cycles / states)

Single source of truth: `references/ids.json`. The values below are
mirrored for quick reading; if they ever drift, `references/ids.json`
wins and `scripts/plane.sh refresh-ids` will rewrite both.

```
workspace_slug : weftos
project_id     : e5d6dd76-c47e-43f0-b228-efbea039c6e7   # "WeftOS"
api_base       : https://api.plane.so/api
auth_header    : X-API-Key: $PLANE_API_KEY               # NEVER commit the key

cycles:
  0.7.x : e3df6167-3b59-46e4-bee8-7f37146b9a9f   # must-ship-before-0.7
  0.8.x : 76a2e899-a3fd-4fdd-ab88-5310d458bb22   # H1 2027 horizon
  0.9.x : e5abd13f-9634-485a-a0c5-0d075ff3dc19   # H2 2027 horizon
  1.0.x : 852ebfd6-ba10-4d82-b63c-676201d7e985   # H1 2028 horizon

states:
  Backlog     : 129bc069-e372-41f7-a563-becf429154f8   # group=backlog
  Todo        : 76d8ee2a-0afd-4359-bf45-7ddd64a59d6f   # group=unstarted
  In Progress : 09fead4c-e5d2-43a3-a8b5-25339bec3901   # group=started
  Done        : 7d0ebbba-5ad6-4b05-9c93-f2e871eaf6b3   # group=completed
  Cancelled   : 0e18be3e-9bbc-46d1-b1fc-875718dce5e3   # group=cancelled
```

**Cycles are gates, not time-boxed sprints.** Targeting a cycle is a
scope decision, not a commitment to ship by a date.

---

## 4. Lifecycle

```
                       create
                         │
                         ▼
                     [Backlog]                — newly captured, not triaged
                         │  triage
                         ▼
                       [Todo]                 — accepted, in a cycle, ready
                         │  claim
                         ▼
                  [In Progress]               — owner working, code in flight
                         │
              ┌──────────┼──────────┐
              ▼          ▼          ▼
            [Done]   [Cancelled]  defer→cycle bump + comment
```

**Transitions:**

- `Backlog → Todo`: triage step. Adds the work item to a cycle and a
  workstream label. Acceptance criteria written if not already.
- `Todo → In Progress`: claim step. Set `assignees` and the `In
  Progress` state in the **same** call. Do this before the first
  code change, not after.
- `In Progress → Done`: close step. Required comment fields:
  - **Shipped**: one-liner of what landed.
  - **Commit(s)**: short SHA(s).
  - **Tests**: which test command was run, what passed.
  - **Build**: `scripts/build.sh check` / `clippy` / `gate` outcome.
  - **Follow-ups**: any new Plane work items spawned (link by `WEFT-N`).
- `In Progress → Cancelled`: only when superseded or scope-cut.
  Comment must say which item supersedes, or which decision cut it.
- `* → defer`: move to a later cycle (HTTP API: see §5.4) and add a
  comment with the reason. Don't close — that loses provenance.

---

## 5. HTTP API patterns (MCP is partially broken)

The `mcp__plane__*` tools are surfaced but `list_states`, `list_labels`,
`list_cycles`, `list_work_items`, and `get_me` all returned **HTTP 404**
on 2026-04-28. The HTTP API works fine. Until the MCP server is fixed,
prefer `curl` via `scripts/plane.sh`. `mcp__plane__create_work_item`
and `mcp__plane__update_work_item` *do* work — use them when you only
need a single create/update and want type-checked args.

`scripts/plane.sh` is a thin wrapper that handles the auth header and
URL prefix. It expects `PLANE_API_KEY` in the env (the user's MCP
config has it; agents should re-export it from there per §7).

### 5.1 Create a work item

```bash
scripts/plane.sh create-issue \
  --name "ws05: Email channel — implement IMAP poll loop" \
  --priority high \
  --state-name Todo \
  --cycle 0.7.x \
  --labels ws05-channels,audit-finding \
  --description-md path/to/spec.md
```

Equivalent raw call (POST `…/projects/$PROJ/issues/`, JSON body):

```json
{
  "name": "ws05: Email channel — implement IMAP poll loop",
  "priority": "high",
  "state": "76d8ee2a-0afd-4359-bf45-7ddd64a59d6f",
  "labels": ["<label-uuid>", "..."],
  "description_html": "<p>...</p>"
}
```

> **Gotcha** (per handoff): the body field is `project_id` if you POST
> against the workspace endpoint, but `state` (singular) is the state
> UUID, not `state_id`. Labels go in `labels`, not `label_ids`. The
> wrapper handles all of this.

### 5.2 Add an item to a cycle

Cycle membership is a separate POST against the cycle's
`cycle-issues/` endpoint with `{ "issues": ["<issue-uuid>", ...] }`.
Use `scripts/plane.sh add-to-cycle <cycle-name> <issue-id>...`.

### 5.3 Move state / claim / close

`scripts/plane.sh transition <issue-id> <state-name> [--assignee me]`
PATCHes the issue's `state` field (and `assignees` when `--assignee`
is given). Always pair `Todo → In Progress` with `--assignee` so the
who-is-working-on-this is visible.

### 5.4 Defer (cycle bump)

`scripts/plane.sh defer <issue-id> <new-cycle> --reason "..."` does
three things atomically: removes the issue from its current cycle,
adds it to the new cycle, and posts a comment with the reason.

### 5.5 Comment on close

```bash
scripts/plane.sh close <issue-id> \
  --shipped "Email IMAP poll loop landed; channel emits real inbound" \
  --commits abc1234,def5678 \
  --tests "scripts/build.sh test (1549/1549 lib pass)" \
  --build "scripts/build.sh check + clippy clean" \
  --followups WEFT-42,WEFT-43
```

Builds the structured close comment, transitions to `Done`, and
posts the comment in one call.

See `references/api-cheatsheet.md` for raw curl recipes if the
wrapper doesn't cover your case.

---

## 6. Triage protocol — audit rows → work items

Use this when walking a `.planning/reviews/0.7.0-release-gate/NN-*.md`
audit doc, a `git grep -n 'TODO\|FIXME'`, or any other unstructured
backlog into Plane.

### 6.1 Per-row checklist

For each row:

1. **Skip if already fixed in git.** Run `git log -S '<distinctive
   token>' --oneline` first when the row is older than ~14 days. The
   handoff explicitly flags `02-kernel-governance.md:591-593` as
   stale (fixed in `a0c54a47`); the same trap exists elsewhere.
2. **Skip if a Plane item already exists.** `scripts/plane.sh search
   "<key phrase>"` is a name+description substring search. Don't
   create duplicates; comment on the existing item instead.
3. **Decide cycle.** Default rules:
   - **0.7.x** — bug, regression, governance/security gap, anything
     the audit flags as critical or P0, anything that breaks a
     "complete" claim users will hit (channel stubs, browser
     pipeline wire, version drift before publish).
   - **0.8.x** — substantive features that don't block the cut but
     are the next obvious deltas (full per-conv cost circuit-breaker,
     EscalateToHuman, MicroLoraRouter v3 once `ruvllm-wasm` lifts
     the HNSW cap, JEPA/LeWM crate tree if the research bet
     promotes).
   - **0.9.x / 1.0.x** — long-horizon, research-heavy, or
     speculative. When in doubt, defer; better to bump up than to
     leave 0.7.x bloated.
4. **Write the description**. Required sections (markdown body):
   ```
   ## Source
   - audit: .planning/reviews/0.7.0-release-gate/NN-*.md#anchor
   - code: crates/foo/src/bar.rs:123  ← if applicable
   - prior planning: .planning/.../foo.md  ← if applicable

   ## Problem / gap
   <one paragraph: what is missing or wrong, in plain English>

   ## Acceptance criteria
   - [ ] <observable behavior or test>
   - [ ] <build / lint gate>
   - [ ] <doc / tracker update>

   ## Dependencies
   - blocks: <other WEFT-N or "none">
   - blocked-by: <other WEFT-N or "none">

   ## Notes
   <freeform: known traps, related ADRs, prior session context>
   ```
5. **Labels**. Always at least two:
   - workstream label (one of `ws01-core` … `ws17-research`); see §6.3.
   - finding-type label (`audit-finding` for audit-derived; plus one
     of `bug`, `gap`, `stub`, `orphan`, `governance`, `tech-debt`,
     `docs`, `tests`, `tooling` as appropriate).
6. **Priority**. `urgent` for live behavioural bugs (Democritus
   stuck-loop class), `high` for 0.7.x blockers, `medium` for 0.7.x
   non-blockers, `low` for deferred-cycle items, `none` for pure
   bookkeeping.
7. **Create + add to cycle** in one wrapper call. Capture the
   returned `WEFT-N` identifier in your triage notes so cross-links
   resolve.

### 6.2 Batch protocol (one workstream at a time)

The audit is 18 docs. Triage by workstream, not by file order:

1. Read the workstream doc end-to-end.
2. Build a flat list of rows on a scratchpad (markdown is fine).
3. Decide cycle for each row (mark `0.7|0.8|0.9|1.0` in the margin).
4. Hand the list to `scripts/plane.sh batch-create
   <workstream-slug>.json` which will create + cycle + label in one
   pass and return a `WEFT-N → row` map.
5. Update the audit doc footer with a "Triaged into Plane on
   <date> — first item WEFT-N, last item WEFT-M" stamp so re-runs
   don't duplicate.
6. Move on to the next workstream.

Use parallel subagents (`Agent` tool, `subagent_type: general-purpose`)
when more than one workstream can run in true isolation — but never
more than 4 in flight at once or the Plane API will rate-limit
(observed: HTTP 429 above ~10 writes/sec).

### 6.3 Workstream label slugs

Mirror the audit doc numbering:

```
ws01-core            ws07-multi-agent       ws13-app-substrate
ws02-kernel          ws08-weftos-gui        ws14-deployment
ws03-pipeline        ws09-clawft-dashboard  ws15-mcp
ws04-plugin-skills   ws10-voice             ws16-browser-wasm
ws05-channels        ws11-agent-core-v1     ws17-research
ws06-memory          ws12-knowledge-graph
```

Plus a cross-cutting `audit-0.7.0` so all audit-derived items are
filterable as a set, and `release-gate-blocker` for the must-fix-
before-0.7 subset.

`scripts/plane.sh ensure-labels` is idempotent — it creates any
labels in `references/labels.json` that don't yet exist and updates
the local cache with their UUIDs.

---

## 7. Environment

The `PLANE_API_KEY` is in the user's MCP config (`~/.claude.json`,
`mcpServers.plane.env.PLANE_API_KEY`). For shell-based use:

```bash
export PLANE_API_KEY=$(python3 -c "import json; \
  print(json.load(open('${HOME}/.claude.json'))['mcpServers']['plane']['env']['PLANE_API_KEY'])")
```

`scripts/plane.sh` does this automatically when `PLANE_API_KEY` is
unset. **Never echo the key, never commit it, never include it in a
work-item description.**

---

## 8. Templates

- `references/triage-template.md` — copy/paste body for a new audit-
  derived work item.
- `references/close-template.md` — copy/paste body for the close
  comment.
- `references/labels.json` — canonical label set (workstream + type).
- `references/ids.json` — workspace / project / cycle / state UUIDs.
- `references/api-cheatsheet.md` — raw curl recipes for the cases
  the wrapper doesn't cover.

---

## 9. Quality bar (do not skip)

Before you call a triage pass "done":

- [ ] Every row in the audit doc is either a Plane work item, a
      "skipped — already fixed in <SHA>" annotation, or a
      "skipped — duplicate of WEFT-N" annotation. Zero silent skips.
- [ ] No work item has only a workstream label (every item also has
      a finding-type label).
- [ ] No work item is in a cycle without acceptance criteria.
- [ ] `scripts/plane.sh check` (validates label coverage + cycle
      assignment + AC presence) returns clean.

---

## 10. Failure modes seen so far

- **MCP `list_*` endpoints return 404.** Use the HTTP API. Fixed
  upstream is tracked under… (file a Plane item under `ws15-mcp` if
  it ever rises above background noise).
- **Plane rate-limits at ~10 writes/sec.** Batches above ~50 items
  should sleep 100ms between calls. The wrapper already does this.
- **`description` vs `description_html` vs `description_stripped`.**
  Plane stores them all and renders the HTML. The wrapper takes
  markdown, runs it through a minimal MD→HTML pass, and writes
  both `description_html` and `description_stripped`.
- **State UUIDs vary per project.** Don't hard-code in agent code;
  always read from `references/ids.json`.
- **Cycle membership is not a field on the issue.** It's a separate
  endpoint. Forgetting this is the #1 way an item ends up "in" the
  0.7.x cycle in your head but unfiled in Plane.
