# ObservationPack — keep the tail, stop replaying it

**From:** llm-lab Grok (daily-coder hunt, 2026-09-10)
**For:** WeftOS grokbot / weave-logic implementation
**Steal from:** NVlabs [SoL-Pi](https://github.com/NVlabs/SoL-Pi) ObservationPack (MIT)
**Lab map:** `~/llm/docs/metaharness/sol-pi-transfer.md`
**Hook:** `AgentLoop::execute_tool_with_guards` after `Ok(val)` — today this
calls `truncate_result` and **throws the tail away**.

Do not install Pi. Do not confuse this with ADR-058 `context_compress`
(lossy gist of *turns*). This is about *tool results*.

## Problem

`MAX_TOOL_RESULT_BYTES = 65_536` in `crates/clawft-core/src/agent/loop_core.rs`.
`truncate_result` in `crates/clawft-core/src/security/mod.rs` (CRIT-02) is
correct as a **prompt budget**. It is wrong as a **store**. A 200 KB test log
becomes a 64 KB prefix; later turns cannot `obs_recall` the failing assertion
that lived past byte 65536. The model re-runs the command or guesses.

SoL-Pi's surviving config (V2, EdgeBench paired A/B): 2048-byte head, 1536-byte
tail, **two full sends** then project. Original stays on disk. `obs_recall`
pages it. Fail-open: if archive or recall fails, send the original (or the
truncated prompt body you already have) — never a silent empty.

## Non-goals (this ticket)

- Action Fusion (`then_run` on edit/write) — later ticket.
- Evidence-Preserving Reducer (nested Nemotron-9B quote-check) — later; this
  archive is the substrate it needs.
- Replacing ADR-058 turn compression.
- Changing the 64 KB *prompt* cap. The LLM still sees a small projection.

## Placement

| Piece | Where |
|---|---|
| Archive + project | `clawft-core` — new `src/observation_pack.rs`, called from `execute_tool_with_guards` only (the WEFT-190 choke point). Do not also patch the stale `tool-calls.md` inline snippet. |
| `obs_recall` tool | `clawft-tools` — read-only, workspace-contained to the session archive. |
| ECC | `ImpulseType::Custom(0x70)` archived, `0x71` recalled. Do not add enum variants in this ticket unless kernel freeze allows it. |
| Tests | `clawft-core` unit tests next to `truncate_result`; one loop test that a >64 KB tool is recallable. |

## On-disk layout

Sibling of the session JSONL (`session_file_path` in `session.rs`):

```
{sessions_dir}/{percent-encoded-key}.observations/
  ledger.jsonl          # one line per pack
  {sha256}.bin          # raw UTF-8 (or original JSON bytes)
```

`ledger.jsonl` line:

```json
{
  "id": "obs_01J…",
  "sha256": "…",
  "tool": "bash",
  "tool_call_id": "…",
  "bytes": 201433,
  "created_ms": 0,
  "full_sends": 1
}
```

No secrets in the handle. Session GC (`SessionGcReport`) must delete the
`.observations/` dir with the jsonl. WASM/browser: skip archive (no disk);
keep today's truncate. `#[cfg(feature = "native")]`.

## Projection (V2 numbers, bytes not tokens)

After `Ok(val)`:

1. Serialize `val` the same way we do today.
2. If `len <= MAX_TOOL_RESULT_BYTES` **and** `len <= HEAD+TAIL` (3584): return
   as now. Do not archive tiny results.
3. Else write `{sha256}.bin` + ledger row (fail-open: if write fails, fall
   through to `truncate_result` and log).
4. `full_sends` for this tool+session: if `< 2`, return the full body this
   turn (still archive). Increment.
5. If `full_sends >= 2`, return a projection object, **not** a chopped string:

```json
{
  "observation_pack": {
    "id": "obs_01J…",
    "sha256": "…",
    "bytes": 201433,
    "tool": "bash",
    "head": "<first 2048 bytes, UTF-8 safe>",
    "tail": "<last 1536 bytes, UTF-8 safe>",
    "recall": "obs_recall",
    "hint": "Use obs_recall with this id and offset/limit to page the archive. Do not re-run the tool to re-read this output."
  }
}
```

UTF-8: never split a codepoint. Head/tail are *byte* budgets on the serialized
form, then floor to char boundaries.

Keep `truncate_result` as the fail-open / WASM path. Do not delete it.

## `obs_recall` tool

```
obs_recall:
  id: string (required)
  offset: u64 (default 0, bytes)
  limit: u32 (default 8192, max 32768)
```

- Resolve `id` only inside **this session's** `.observations/` ledger.
- Return `{id, offset, bytes, eof, chunk}`.
- Deny path traversal; `id` is `[A-Za-z0-9_]+` after the `obs_` prefix.
- EffectGate: read-only, no sandbox FS of the workspace (archive is session
  store). Same permit path as other tools.

## ECC

Emit `Custom(0x70)` on successful archive, `Custom(0x71)` on successful recall.
Payload: session key + obs id + bytes. Skip if ECC queue is not wired in this
build.

## Tests (must exist before claim-done)

- Tiny result: no archive dir created.
- Result 10 KB: archived, first two loop iterations return full body, third
  returns `observation_pack` with head/tail.
- `obs_recall` pages across the SHA file; last page sets `eof: true`.
- Archive write failure → `truncate_result` still used; loop does not error.
- Session GC removes `.observations/`.
- Existing CRIT-02 test: prompt body still `<= MAX_TOOL_RESULT_BYTES` after
  projection (`serde_json::to_string(projection).len()`). If the JSON wrapper
  exceeds 64 KB (should not with 3.5 KB head+tail), shrink head/tail, do not
  raise the cap.

## Acceptance

- [ ] `execute_tool_with_guards` is the only pack site (WEFT-190).
- [ ] Prompt-facing tool results remain `<= 65536` bytes.
- [ ] A >64 KB bash/test log is recallable later in the same session without
      re-running the command.
- [ ] WASM/browser builds still compile; pack is native-only.
- [ ] `scripts/build.sh test` for clawft-core + clawft-tools; `scripts/build.sh check`.
- [ ] `docs/guides/tool-calls.md` updated (the 64 KB line currently says the
      tail is gone).

## Later (not this ticket)

1. Evidence-Preserving Reducer on top of this archive (nested model:
   Nemotron-9B-OpenCode 8-bit, think-off, `:8092`). Every quoted line must
   match the SHA file or the frontier gets the raw projection.
2. Action Fusion: optional `then_run` on edit/write in `clawft-tools`.
