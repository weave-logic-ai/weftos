# ADR-052: `clawft-core::ToolRegistry` vs `clawft-kernel::ToolRegistry` — Two Registries, Two Concerns

**Date**: 2026-04-28
**Status**: Accepted
**Deciders**: 0.7.0 release-gate review (M4-E, WEFT-62)
**Closes**: 04-plugin-skills task #4 ("Add ADR documenting `clawft-
core` vs `clawft-kernel` ToolRegistry split + migration plan");
audit Open Question 2 (lines 273-275).
**Related**: ADR-036 (Hierarchical kernel ToolRegistry), ADR-051
(orphaned plugin-crate fate).

## Context

The 0.7.0 release-gate audit
(`.planning/reviews/0.7.0-release-gate/04-plugin-skills.md`,
lines 206-218) flagged the existence of **two** distinct types both
named `ToolRegistry`:

1. `clawft_core::tools::registry::ToolRegistry`
   (`crates/clawft-core/src/tools/registry.rs:485`)
2. `clawft_kernel::wasm_runner::registry::ToolRegistry`
   (`crates/clawft-kernel/src/wasm_runner/registry.rs:37`)

Their `Tool` traits are different (`async_trait`-driven `dyn Tool` in
core, sync `dyn BuiltinTool` in kernel). Their concerns overlap on the
surface — both register tools, both gate dispatch, both filter by
permission — but in practice they serve distinct runtime tiers. ADR-036
introduced the kernel registry's hierarchical / signed model but did
not document the relationship to the pre-existing core registry. The
audit's Open Question 2 asked: *"when does the agent loop stop using
`clawft-core::tools::registry::ToolRegistry` and start using the
kernel's hierarchical one?"*

This ADR is the answer: **never (by design)**. They are not
duplicates, no migration is planned for 0.7.0, and the contract
between them is documented below so future work doesn't waste cycles
debating a merger that should not happen.

## The two registries, side by side

| Property                       | `clawft-core::ToolRegistry`                                                                       | `clawft-kernel::ToolRegistry`                                                                                |
|--------------------------------|---------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------|
| Trait registered               | `async fn execute` on `Tool: Send + Sync` (async, returns `Result<ToolOutput, ToolError>`)        | `fn execute` on `BuiltinTool: Send + Sync` (sync, returns `Result<serde_json::Value, ToolError>`)            |
| Primary caller                 | `clawft_core::agent::loop_core::run_tool_loop` — the LLM-facing agent loop                        | `clawft_kernel::wasm_runner::WasmToolRunner` — the kernel's WASM/builtin dispatch fabric                     |
| Tool flavour                   | High-level, LLM-described tools (file ops, MCP-imported tools, channel adapters, plugin tools)     | Kernel-internal builtins: `fs.read_file`, `process.spawn`, agent-table ops; signed plugin shims              |
| Description schema             | `ToolMetadata { name, description, input_schema, sandbox_hint }` — feeds the LLM prompt builder    | `BuiltinToolSpec` — internal kernel ABI, never goes to an LLM prompt                                         |
| Permission gate                | `check_tool_permission(&UserPermissions, glob)` — user-/route-level gate                          | Hierarchical `with_parent` chain + Ed25519 `Signed` overlay (ADR-036) — kernel-level capability gate         |
| Hierarchy                      | Flat `HashMap<String, Arc<dyn Tool>>` plus a per-path advisory-lock layer (WEFT-37)                | `with_parent(Arc<ToolRegistry>)` chain; child overlays parent; signed entries override unsigned at any tier  |
| Concurrency / runtime          | Async (tokio), suitable for I/O-heavy LLM tool calls and MCP RPC                                  | Sync, called from inside the kernel runner's WASM host shims (`Wasmtime` host calls are sync)                |
| Lives where on the call graph  | One layer above the LLM: agent → registry → MCP/native tool                                       | One layer below the WASM sandbox: WASM-guest → host-call shim → registry → builtin                            |
| What "register" feeds          | The LLM tool catalog (`schemas_for_tools(&allowed)`)                                              | The kernel's host-call dispatch table for WASM guests                                                        |

In short: **`clawft-core::ToolRegistry` is the LLM-facing catalog and
dispatcher**. **`clawft-kernel::ToolRegistry` is the WASM/builtin
host-call fabric**. They sit on opposite sides of the kernel
boundary. The agent loop never speaks to the kernel registry; the
WASM runtime never speaks to the core registry; LLM tool descriptions
never reach the kernel registry; kernel host-call ABI never reaches
the LLM.

## Decision

**Keep both. Treat them as canonically distinct types serving distinct
runtime concerns. Do not unify, do not migrate.**

The two registries are not duplicates. A merger would either:

- Force the kernel's host-call dispatch through an `async` trait
  (impractical: WASM host calls are sync from the runtime's
  perspective; bridging that to async would require per-call task
  spawn, which is a non-goal for sub-millisecond host calls), OR
- Force every LLM tool through `dyn BuiltinTool` (loses MCP
  integration, the `ToolMetadata` schema feeding the prompt builder,
  and the `UserPermissions` glob gate that the routing layer
  depends on).

Neither direction is desirable.

### The contract

To make the boundary unambiguous for future contributors:

1. **`clawft-core::ToolRegistry` is the agent-side LLM tool
   catalog.** It owns the schema-to-prompt path, the
   `UserPermissions` gate, MCP-imported tools, plugin-provided
   tools, and per-path advisory locking (WEFT-37). It is consumed
   exactly by `run_tool_loop`. Type signature:
   `Arc<dyn Tool>` where `Tool::execute` is `async`.

2. **`clawft-kernel::ToolRegistry` is the kernel-side host-call
   fabric.** It owns ADR-036 hierarchy (`with_parent`), Ed25519
   signing (`Signed`), and the `EffectVector` capability check for
   WASM-loaded tools. It is consumed by `WasmToolRunner` and the
   handful of kernel pieces that call into it directly (e.g.
   `assessment::*`). Type signature: `Arc<dyn BuiltinTool>` where
   `BuiltinTool::execute` is sync.

3. **Bridging** (when an LLM-facing tool is implemented as a kernel
   builtin or signed WASM plugin) goes through a per-tool adapter,
   not a registry merger. The adapter is an `impl Tool for
   KernelBuiltinAdapter { … }` that wraps the kernel registry's
   sync dispatch in an `async` shell. This pattern is already in
   use for the WASM tool path (`WasmToolAdapter`, see the audit
   reference at line 237) and is the right place to bridge.

4. **No tool may live in both registries with separate impls.**
   When a tool is bridged via adapter, the kernel-side registry
   holds the canonical `BuiltinTool` impl, and the core-side
   registry holds *only* a thin `KernelBuiltinAdapter` referencing
   it. The adapter does not duplicate logic.

## Consequences

### Positive

- Future contributors stop trying to pick "the right one" — the
  decision is the runtime tier, not a coin-flip.
- ADR-036's hierarchical/signed model is preserved at the kernel
  layer where it is meaningful, without being forced upon the
  LLM-facing catalog where it would be pointless (the LLM never
  signs anything; it asks for a tool by name).
- The async vs sync trait split is a feature, not a wart: it
  enforces that LLM-facing tools may do I/O while kernel host
  calls remain sync and fast.

### Negative / accepted

- The naming collision (`ToolRegistry` in two crates) remains. A
  rename to e.g. `KernelToolRegistry` and `AgentToolRegistry` was
  considered but rejected: each name is correct *within its
  bounded context* (kernel vs agent), and within each crate the
  short name is the right ergonomics. Cross-crate consumers
  disambiguate via fully qualified path (`clawft_core::tools::
  registry::ToolRegistry` vs `clawft_kernel::wasm_runner::
  registry::ToolRegistry`); this is acceptable given how rarely
  both are imported together (the bridge adapter is the only
  place).
- The contract above lives in this ADR rather than in code-level
  rustdoc; future drift is possible. Mitigation: the file-level
  rustdoc on each `registry.rs` will be updated to point to this
  ADR.

### Pointers to update

The following files should grow a `//!` reference to this ADR
(separate cleanup pass; not part of the WEFT-62 commit):

- `crates/clawft-core/src/tools/registry.rs` — file-level doc
- `crates/clawft-kernel/src/wasm_runner/registry.rs` — file-level doc

## Migration plan

**None.** No code moves. No types rename. No traits merge. The agent
loop continues to use `clawft-core::ToolRegistry`; the kernel
continues to use `clawft-kernel::ToolRegistry`; the bridge adapter
pattern (`WasmToolAdapter`) is the only sanctioned cross-tier path.

The only deliverable was this document.

## Sources

- `.planning/reviews/0.7.0-release-gate/04-plugin-skills.md`
  (audit lines 206-218; Open Question 2 at 273-275; task #4)
- `.planning/reviews/0.7.0-release-gate/02-kernel-governance.md`
  (audit line 237 — `WasmToolAdapter`/`ToolRegistry` security
  posture; line 307 — `register_with_metadata` skip note)
- `crates/clawft-core/src/tools/registry.rs` (LLM-facing catalog)
- `crates/clawft-kernel/src/wasm_runner/registry.rs` (kernel
  dispatch fabric)
- `crates/clawft-core/src/agent/loop_core.rs:727`
  (`run_tool_loop` — the consumer)
- `crates/clawft-kernel/src/wasm_runner/mod.rs:440-478`
  (`with_parent` overlay usage)
- ADR-036 (Hierarchical kernel ToolRegistry — defines the kernel-
  side hierarchy and signing this ADR documents)
- ADR-051 (orphaned plugin crates — some archived plugins are
  candidates to resurface as `BuiltinTool` impls in the kernel
  registry, never in the core registry)
