# ADR-048: Kernel Phase (K-Level) Responsibilities

**Date**: 2026-04-03
**Status**: Accepted
**Deciders**: Architecture review, Sprint 14
**Renumbered**: 2026-04-28 (WEFT-140) — was ADR-020, collided with
ADR-020 (ChainLoggable). Earlier dependants (ADR-021, ADR-022) and
the kernel-governance audit reference this document by its old
number; references have been updated to point at ADR-048.

## Context

WeftOS is built in kernel phases (K0–K6), each adding a layer of capability. As the system grows, the boundary between what each phase owns becomes critical for security, auditability, and correct delegation. Without a formal responsibility map, new features get implemented in the wrong layer — e.g., file scanning in the CLI instead of through the kernel's governance gates.

## Decision

Each K-level has a defined set of responsibilities. All operations within a phase's scope MUST be performed by that phase's kernel services, not by external tools or CLI code.

### K0: Boot and Lifecycle

- Kernel state machine (Booting → Running → ShuttingDown → Halted)
- Configuration loading and validation
- Feature gate evaluation
- Health check infrastructure
- Graceful shutdown coordination

### K1: Process and Supervision

- PID allocation and process table management
- Agent spawning via `Supervisor::spawn()`
- Agent lifecycle (start, stop, restart, inspect)
- Resource limits and capability assignment
- Process lineage tracking (parent → child)
- RBAC enforcement on agent operations

### K2: IPC and Communication

- Message bus (publish/subscribe)
- A2A (agent-to-agent) routed messaging
- Cron scheduling and job execution
- Service registry and health monitoring
- Topic management

### ExoChain: Cryptographic Audit

- Append-only hash chain (SHAKE-256)
- Resource tree (hierarchical project model)
- Gate backends (governance enforcement points)
- Event logging for all significant state changes
- Chain verification and export

### K3: WASM Sandbox

- Plugin loading and execution
- Fuel metering and memory limits
- Host function auditing
- Permission store for plugin upgrades

### K3c: ECC Cognitive Substrate

- Causal DAG (CausalGraph)
- HNSW vector memory
- Impulse queue and cognitive tick loop
- Cross-reference store (UniversalNodeId)
- Weaver engine (HYPOTHESIZE-OBSERVE-EVALUATE-ADJUST)

### K4: Containers (planned)

- Docker/Podman integration
- Container lifecycle management
- Image registry interaction

### K5: Application Framework

- App manifest parsing
- App lifecycle (install, start, stop, remove)
- Agent fleet spawning from manifests
- Rolling upgrades

### K6: Mesh Networking

- Encrypted P2P transport (Noise protocol)
- Peer discovery (seed, mDNS, Kademlia, peer exchange)
- Cross-node IPC bridging
- State replication
- Genesis hash verification (trust model)

## Consequences

### Positive
- Clear ownership prevents capability leakage between layers
- New features slot into the correct phase naturally
- Security audits can focus on phase boundaries
- CLI remains a thin client (see ADR-021)

### Negative
- Some operations span multiple phases and need careful coordination
- Bootstrap operations (init, onboard) must work before the kernel exists

### Neutral
- Feature gates already control which phases are compiled in
- Each phase's services are independently testable
