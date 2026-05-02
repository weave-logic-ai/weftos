# Plane HTTP API — raw curl recipes

For when `scripts/plane.sh` doesn't cover your case, or when debugging
why a wrapper call failed.

## Auth

```bash
export PLANE_API_KEY=$(python3 -c "import json; \
  print(json.load(open('$HOME/.claude.json'))['mcpServers']['plane']['env']['PLANE_API_KEY'])")
WS=weftos
PROJ=e5d6dd76-c47e-43f0-b228-efbea039c6e7
BASE=https://api.plane.so/api/v1/workspaces/$WS/projects/$PROJ
H_AUTH="X-API-Key: $PLANE_API_KEY"
H_JSON="Content-Type: application/json"
```

The `X-API-Key` header is the only auth. Bearer tokens / cookies are
ignored by Plane's API.

## List

```bash
# states
curl -sf -H "$H_AUTH" "$BASE/states/" | jq

# cycles
curl -sf -H "$H_AUTH" "$BASE/cycles/" | jq

# labels
curl -sf -H "$H_AUTH" "$BASE/labels/" | jq

# work items (paginated; per_page max 100)
curl -sf -H "$H_AUTH" "$BASE/issues/?per_page=100" | jq '.results[] | {id, sequence_id, name}'
```

> **MCP gotcha (2026-04-28)**: `mcp__plane__list_states`,
> `mcp__plane__list_labels`, `mcp__plane__list_cycles`,
> `mcp__plane__list_work_items`, `mcp__plane__get_me` all return HTTP
> 404. Use the curl forms above. `mcp__plane__create_work_item` and
> `mcp__plane__update_work_item` work fine if you only need a one-shot.
> Upstream issue and minimal repro live at
> `.planning/sparc/voice/plane-mcp-upstream-issue.md` (tracks
> WEFT-478). The wrapper script `scripts/plane.sh` is the load-bearing
> path; do not wait for the upstream fix.

## Create work item

```bash
curl -sf -H "$H_AUTH" -H "$H_JSON" -X POST "$BASE/issues/" -d '{
  "name": "ws05: Email channel — implement IMAP poll loop",
  "description_html": "<p>Body in HTML. The wrapper does MD→HTML for you.</p>",
  "priority": "high",
  "state": "76d8ee2a-0afd-4359-bf45-7ddd64a59d6f",
  "labels": ["<ws05-uuid>", "<audit-finding-uuid>"]
}' | jq
```

Response includes `id`, `sequence_id` (the human-readable `WEFT-N`
suffix), and the full inflated record. Capture `id` for follow-up
calls; capture `sequence_id` for cross-references.

## Add work item to a cycle

Cycle membership is its own endpoint:

```bash
CYCLE=e3df6167-3b59-46e4-bee8-7f37146b9a9f   # 0.7.x
ISSUE=<work-item-uuid>

curl -sf -H "$H_AUTH" -H "$H_JSON" -X POST \
  "$BASE/cycles/$CYCLE/cycle-issues/" \
  -d "{\"issues\": [\"$ISSUE\"]}" | jq
```

`POST` is idempotent on the membership pair — safe to retry.

## Move state / claim / assign

```bash
# claim and move to In Progress in one call
curl -sf -H "$H_AUTH" -H "$H_JSON" -X PATCH "$BASE/issues/$ISSUE/" -d '{
  "state": "09fead4c-e5d2-43a3-a8b5-25339bec3901",
  "assignees": ["<my-user-uuid>"]
}' | jq

# move to Done
curl -sf -H "$H_AUTH" -H "$H_JSON" -X PATCH "$BASE/issues/$ISSUE/" -d '{
  "state": "7d0ebbba-5ad6-4b05-9c93-f2e871eaf6b3"
}' | jq
```

To get your own user UUID, `curl -sf -H "$H_AUTH"
"https://api.plane.so/api/v1/workspaces/$WS/members/" | jq` — your
membership row contains it.

## Comment on a work item

```bash
curl -sf -H "$H_AUTH" -H "$H_JSON" -X POST \
  "$BASE/issues/$ISSUE/comments/" \
  -d '{"comment_html": "<p><b>Closed</b>: shipped in <code>abc1234</code>.</p>"}' | jq
```

## Defer (move from one cycle to another)

```bash
OLD=<old-cycle-uuid>
NEW=<new-cycle-uuid>
ISSUE=<work-item-uuid>

# remove from old
curl -sf -H "$H_AUTH" -X DELETE \
  "$BASE/cycles/$OLD/cycle-issues/$ISSUE/"

# add to new
curl -sf -H "$H_AUTH" -H "$H_JSON" -X POST \
  "$BASE/cycles/$NEW/cycle-issues/" \
  -d "{\"issues\": [\"$ISSUE\"]}"

# document it
curl -sf -H "$H_AUTH" -H "$H_JSON" -X POST \
  "$BASE/issues/$ISSUE/comments/" \
  -d '{"comment_html": "<p><b>Deferred to 0.8.x</b>: blocked by ruvllm-wasm 11-pattern HNSW cap.</p>"}'
```

## Search

There's no first-class search-by-text endpoint on the issues
collection in v1. Workaround: list with a high `per_page` and grep
locally. The wrapper's `search` subcommand caches the listing for
60 seconds to amortise.

```bash
curl -sf -H "$H_AUTH" "$BASE/issues/?per_page=100" | \
  jq '.results[] | select(.name | test("email"; "i")) | {sequence_id, name}'
```

## Rate limits

Empirically ~10 writes/sec before HTTP 429. Sleep 100ms between
batched POSTs. The wrapper handles this; raw curl loops should add
`sleep 0.1`.

## Common 4xx pitfalls

- **400** on create: missing `state` (Plane requires explicit state
  on create; default is not auto-applied).
- **400** on create: `state_id` instead of `state`. The field is
  `state` (the UUID; `state_id` is silently ignored).
- **400** on create: `label_ids` instead of `labels`. Same shape.
- **404** on cycle-issues POST: cycle UUID belongs to a different
  project. Re-check `references/ids.json`.
- **404** on `/states/` or `/labels/` via MCP server but 200 via
  curl: known MCP server bug as of 2026-04-28.
