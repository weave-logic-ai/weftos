# Upstream issue: `plane-mcp-server` `list_*` endpoints return HTTP 404

- **Status**: Open (2026-04-28)
- **Tracks**: WEFT-478
- **Repo**: `uvx plane-mcp-server` (upstream Plane MCP shim)
- **Workspace impact**: every `list_*` Plane tool exposed via MCP is
  unusable from inside Claude. We work around it with
  `.claude/skills/plane-workflow/scripts/plane.sh`, which speaks the
  Plane HTTP API directly. The wrapper is the load-bearing path; the
  MCP server is best-effort for one-shot create/update verbs only.

## Minimal repro

1. Install / launch the upstream MCP server:

   ```bash
   uvx plane-mcp-server
   ```

2. From any MCP client (Claude Desktop, `clawft mcp-server`, etc.),
   invoke any of:

   - `mcp__plane__list_states`
   - `mcp__plane__list_labels`
   - `mcp__plane__list_cycles`
   - `mcp__plane__list_work_items`
   - `mcp__plane__get_me`

3. Expected: the underlying Plane API call hits
   `https://api.plane.so/api/v1/workspaces/<ws>/projects/<proj>/states/`
   and returns the resource list.

4. Actual: the server returns HTTP 404 with body
   `{"error": "Page not found."}`. Curl against the same path with
   `X-API-Key` succeeds, so the bug is in URL composition on the MCP
   side (likely `projects/None/...`).

## Diagnosis hint

`plane.py` shows what the URL must look like:

```
https://api.plane.so/api/v1/workspaces/<ws-slug>/projects/<proj-uuid>/issues/?per_page=100
```

The MCP server is either (a) not threading the project id through the
list endpoints (the path interpolation lands `None` literally — see
the 404 body the wrapper script emits when project id is missing) or
(b) using a stale endpoint shape that v1 retired.

The two verbs that DO work (`mcp__plane__create_work_item`,
`mcp__plane__update_work_item`) take the project id as an explicit
argument, which is consistent with the diagnosis.

## Workaround in this tree

`.claude/skills/plane-workflow/scripts/plane.sh` (and `plane.py` it
shells out to) is the canonical Plane CLI for WeftOS work. All Plane
verbs we need — `list-issues`, `transition`, `defer`, `close`,
`comment`, `create-issue`, `add-to-cycle` — are wrapped there. Agents
should drive Plane through that wrapper, not the MCP server.

`docs/api-cheatsheet.md` (under
`.claude/skills/plane-workflow/references/`) records the raw curl
recipes for cases the wrapper doesn't cover.

## Disposition

- **Upstream**: file the bug against `plane-mcp-server` with this
  repro. Track resolution; do not block 0.7.0.
- **In-tree**: keep the wrapper. Even if the upstream is fixed, the
  HTTP-API wrapper is faster (no MCP startup) and gives us a stable
  surface that doesn't churn with the upstream's tool schema.
