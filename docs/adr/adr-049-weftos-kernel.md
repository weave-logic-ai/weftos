# ADR-049: WeftOS Kernel Architecture

**Status**: Accepted (architectural overview; supersedes the earlier
"Proposed" status now that K0–K5 have shipped)
**Date**: 2026-02-28 (originally proposed); 2026-04-28 (status flip
on consolidation)
**Deciders**: Core team
**SPARC Workstream**: W-KERNEL (`.planning/sparc/weftos/`)
**Renumbered / Relocated**: 2026-04-28 (WEFT-140) — this document
was originally `docs/architecture/adr-028-weftos-kernel.md`, which
collided in number with ADR-028 (Mandatory Dual Signing) under
`docs/adr/`. Both ADRs are now under `docs/adr/`; this one is
ADR-049. The file path was the prior cross-reference target in
`docs/weftos/architecture.md` and the K0–K3 gap-analysis notes;
those references have been updated.

---

## Context

OpenFang and similar "Agent Operating System" frameworks demonstrate that treating
AI agents as first-class OS processes (with PIDs, supervisors, RBAC, IPC, and sandboxing)
enables capabilities beyond what a simple agent loop provides:

- **Process isolation**: Agents crash without bringing down the host
- **Capability-based security**: Per-agent tool and resource restrictions
- **Inter-agent communication**: Structured message passing replaces ad-hoc delegation
- **Application packaging**: Third-party agent systems run as managed applications
- **Service orchestration**: Sidecar services managed alongside agents

clawft already has nearly every kernel primitive needed:

| Primitive | Existing Location |
|---|---|
| `MessageBus` | `clawft-core/src/bus.rs` |
| `SandboxEnforcer` | `clawft-core/src/agent/sandbox.rs` |
| `SandboxPolicy` | `clawft-plugin/src/sandbox.rs` |
| `PermissionResolver` | `clawft-core/src/pipeline/permissions.rs` |
| `UserPermissions` | `clawft-types/src/routing.rs` |
| `CronService` | `clawft-services/src/cron_service/` |
| `AgentLoop` + `CancellationToken` | `clawft-core/src/agent/loop_core.rs` |
| `Platform` trait | `clawft-platform/src/lib.rs` |
| `PluginHost` | `clawft-channels/src/host.rs` |
| `ToolRegistry` | `clawft-core/src/tools/registry.rs` |
| `PluginSandbox` | `clawft-wasm/src/sandbox.rs` |
| `DelegationEngine` | `clawft-services/src/delegation/mod.rs` |
| `MCP server/client` | `clawft-services/src/mcp/` |
| Container tools | `clawft-plugin-containers/src/lib.rs` |

These primitives exist in separate crates with no unified composition layer. There is
no concept of a "process table", no per-agent capability enforcement, no structured
inter-agent messaging, and no application packaging model.

## Decision

Create a new `clawft-kernel` crate that composes existing primitives into an OS
kernel abstraction. The kernel:

1. **Wraps `AppContext<P>`** in a boot sequence with state machine (Booting -> Running -> ShuttingDown -> Halted)
2. **Adds a process table** tracking agent PIDs, states, capabilities, and resource usage
3. **Introduces `SystemService` trait** for service registry and lifecycle management
4. **Extends IPC** with agent-to-agent messaging using typed message envelopes and pub/sub topics
5. **Enforces per-agent capabilities** (RBAC) on tool calls, IPC access, and resource consumption
6. **Provides WASM tool sandboxing** via Wasmtime with fuel metering and memory limits
7. **Manages container sidecars** for external service orchestration
8. **Implements application framework** with manifest-based packaging and lifecycle management

### What This Is NOT

- Not a rewrite of existing code. Every kernel subsystem wraps or composes existing types.
- Not a fork. `weft` remains the single CLI entry point.
- Not required. Kernel is opt-in; existing `weft` commands work without kernel activation.

### Crate Structure

```
crates/clawft-kernel/
  Cargo.toml
  src/
    lib.rs           -- crate root
    boot.rs          -- Kernel<P> struct, boot sequence
    process.rs       -- Process table, PID allocation
    service.rs       -- SystemService trait, ServiceRegistry
    ipc.rs           -- KernelIpc, KernelMessage types
    capability.rs    -- AgentCapabilities, IpcScope, ResourceLimits
    health.rs        -- Health checks
    config.rs        -- KernelConfig
    supervisor.rs    -- Agent supervisor (spawn/stop/restart)
    a2a.rs           -- Agent-to-agent protocol
    topic.rs         -- Pub/sub topic routing
    wasm_runner.rs   -- Wasmtime tool execution (feature: wasm-sandbox)
    container.rs     -- Docker sidecar management (feature: containers)
    app.rs           -- Application manifests and lifecycle
```

### Feature Gates

| Feature | Dependency | Purpose |
|---|---|---|
| `wasm-sandbox` | `wasmtime` | WASM tool execution with fuel metering |
| `containers` | `bollard` | Docker container management |

Both are optional and not in default features, preserving current binary size.

## Consequences

### Positive

- **Unified abstraction**: All kernel primitives accessible through one `Kernel<P>` type
- **Per-agent security**: RBAC prevents tools from accessing resources beyond their scope
- **Inter-agent communication**: Structured IPC replaces ad-hoc message passing
- **External framework interop**: Application manifests enable third-party agent systems
- **Container services**: Sidecar orchestration enables database, cache, and API dependencies
- **No breaking changes**: Existing code paths preserved; kernel is additive

### Negative

- **New crate**: Adds to workspace compilation time
- **Wasmtime size**: `wasm-sandbox` feature adds significant binary size (~5MB); mitigated by feature gate
- **Docker dependency**: `containers` feature requires Docker daemon; mitigated by feature gate and graceful error handling
- **Complexity**: Kernel abstraction adds indirection; justified by capabilities gained

### Neutral

- **Migration path**: Existing `weft` users see no changes until they opt into kernel features
- **Testing surface**: Each kernel subsystem has focused unit tests; integration tests verify cross-subsystem behavior
- **Documentation**: Kernel guide, per-phase decision records, and ADR provide comprehensive coverage

## Alternatives Considered

### 1. Extend AppContext Directly

Add process table, capabilities, and IPC to `AppContext`. Rejected because `AppContext` is consumed by `into_agent_loop()`, making it unsuitable for long-lived kernel state. Also increases coupling in `clawft-core`.

### 2. Fork into Separate Binary

Create a `weftos` binary separate from `weft`. Rejected because it fragments the user experience and duplicates CLI infrastructure.

### 3. External Process Manager

Use an external process manager (systemd, supervisord) for agent lifecycle. Rejected because it loses the in-process capability enforcement and IPC typing that make the OS abstraction valuable.

## References

- OpenFang comparison: `.planning/development_notes/openfang-comparison.md`
- SPARC workstream: `.planning/sparc/weftos/00-orchestrator.md`
- Existing kernel primitives audit: See Context section above
