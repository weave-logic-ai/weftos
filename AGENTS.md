# AGENTS — WeftOS (master controller)

You are the **lead** for this repo. Incoming work is the WeftOS dashboard board. Jobs and
local servers are `process-compose.yaml`. Full loop:
`docs/guides/agent-harness-triple-loop.md`. Decision: ADR-098.

| lane | owns | board |
|---|---|---|
| **lead** (you) | routing, score, research receipt, overlay contract | dashboard board |
| implementers | crates / docs / gui as claimed | same board, `claim` first |
| MetaHarness | score / genome / flywheel receipts | not a second board |

## Triple loop

| loop | every turn | command |
|---|---|---|
| **research** | never skip | `node scripts/metaharness/crosscut.mjs` + `node scripts/dashboard-board.mjs ready` |
| **development** | when ready exists | `claim` → worktree if needed → `done` with tests |
| **ops** | when servers/jobs move | this repo's process-compose on **:18090** / **:18091** |

Score is its own subtask: `scripts/metaharness/score.sh`.

```
          research / ready / score
   lead ──────────────────────────► dashboard board
     ▲                               │
     │ receipts                      │ claim / done
     │                               ▼
     └──────── implementers ◄────────┘
```

Forever flywheel (`node scripts/metaharness/loop-runner.mjs --forever`)
only if the user asked. It spends tokens on a timer.

## Process-compose

This project: HTTP `127.0.0.1:18090`, MCP `127.0.0.1:18091/sse`.
**Not** Forge's `:18080` / `:18081`. Kernel is a **shared UDS** at
`~/.clawft/kernel.sock` (`WEFTOS_RUNTIME_DIR`) so 1v1sion and Forge can
talk to one WeftOS. One Ruflo **daemon per repo** (not per crate, not MCP stdio).

```bash
node scripts/pc-enforce.mjs enforce --agent lead --reason "…"
process-compose process start job_score -p 18090 --address 127.0.0.1
```

Do not launch a second process-compose via stdio MCP. Do not glob other
repos' yaml into this instance (ADR-098). `overlay_ready` is **false**.

## Hard rules

- Never commit to `master` / `main`. Branch is `0.8-metaharness` (or a
  feature branch).
- MetaHarness is not a `weft` runtime dependency (ADR-096).
- No silent flywheel promote.
- Claim on the dashboard board before you edit for a ticket. Use
  `node scripts/dashboard-board.mjs claim <ticket UUID|WEFT-N>`.
- Imported Plane IDs are historical references. Do not create or change Plane
  tickets for WeftOS work after this board cutover.
- Do not take down Forge `:3333` or Forge `:3000` (v2 frontend).
