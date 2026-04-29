# Kernel Operator Guide

This guide is the operator-facing entry point to the WeftOS kernel
(`clawft-kernel`) as it ships in 0.7.0. It covers the boot sequence,
the kernel services that an operator interacts with, the K-phase
status as of 0.7.0, governance and ExoChain auditing at the
operator level, and the day-to-day log lines that show up under
normal operation (including the DEMOCRITUS "still stuck" warning
that has historically been mistaken for a bug).

For developer-facing material — adding modules, extending traits,
unit-testing the kernel — see the WeftOS docs site at
[`docs/src/content/docs/weftos/kernel-guide.mdx`](../src/content/docs/weftos/kernel-guide.mdx).

For the architectural rationale behind the kernel, see ADR-049
(`docs/adr/adr-049-weftos-kernel.md`) and ADR-048 (Kernel Phase
Responsibilities).

---

## What the Kernel Is

`clawft-kernel` is the runtime that supervises agents, brokers IPC,
maintains the ExoChain audit log, enforces three-branch governance,
runs the DEMOCRITUS cognitive tick, and (optionally) the K6 mesh.
It is the same binary on every node — the operating mode is selected
by configuration (see ADR-042: Three Operating Modes).

The kernel runs as a long-lived daemon. The CLI front-ends (`weft`
and `weaver`) are thin clients: they enqueue commands over the
kernel's RPC socket and the kernel does the actual work, logs the
event to the chain, and routes any outbound traffic. ADR-021 (CLI
Kernel Compliance) is the load-bearing rule: **no CLI command
should bypass the daemon for state-changing operations**. If you
see a command that does, file it as a bug.

---

## Boot Sequence

Boot is a strict state machine driven by `Kernel::boot()` in
`crates/clawft-kernel/src/boot.rs`. An operator should expect to
see the following phases in order, each emitting a `BootEvent` that
is chain-logged when ExoChain is enabled:

| Phase | What happens |
|-------|--------------|
| 0 | Configuration loaded; feature gates evaluated; logging set up. |
| 1 | Process table + supervisor initialized (K1). |
| 2 | IPC + A2A router + topic router come up (K2). |
| 3 | Service registry started; default `SystemService`s (cron, health, etc.) registered (K2). |
| 3.5 | ExoChain primed: chain manager, tree manager, gate backends. |
| 4 | WASM tool runner ready (K3, behind `wasm-sandbox`). |
| 4.5 | Container manager (K4, behind `containers`). |
| 5a | App framework (K5) loads `weftapp.toml` manifests. |
| 5b | ECC substrate (K3c, behind `ecc`): causal graph, HNSW, calibration, cognitive tick. |
| 5d | Mesh networking (K6, behind `mesh`): listeners, peer discovery, heartbeat. |
| 6 | Kernel transitions to `Running`. |

A clean boot ends with `kernel state: Running` and a chain entry
of kind `kernel.boot.complete`. Anything earlier than that without
a corresponding entry means the boot stalled — `weaver kernel logs`
will show where.

---

## K-Phase Status (0.7.0)

K0–K5 are complete; K6 is at Phase 1. The full status table —
including which subsystem each phase owns and what is deferred —
lives in [`docs/weftos/k-phases.md`](../weftos/k-phases.md).

| Phase | Status | Highlights |
|-------|--------|-----------|
| K0    | Complete | Boot state machine, config, feature gates. |
| K1    | Complete | Process table, agent supervisor, RBAC. |
| K2    | Complete | IPC, A2A, topics, cron, services, health. |
| K2b   | Complete | Hardening: GovernanceGate, signed agent.spawn/exit/restart. |
| K2.1  | Complete | Symposium decisions: SpawnBackend, dual-signing scaffolding, ServiceEntry, MessageTarget::Service. |
| K3    | Complete | Wasmtime sandbox, fuel metering, ServiceApi, dual-layer A2ARouter gate. |
| K3c   | Complete | ECC: causal DAG, HNSW, cognitive tick, EML. |
| K4    | Complete | Container lifecycle, ChainAnchor trait + MockAnchor. |
| K5    | Complete | App framework, manifest install/start/stop, namespaced agents/tools. |
| K6    | Phase 1   | Types, traits, TCP/WS transport, 136 tests. Replication / split-brain handling deferred. |

---

## Operator-Facing Subsystems

### ExoChain (audit log)

ExoChain is the tamper-evident, hash-chained audit log behind every
state-changing operation in the kernel. It is enabled by default in
the 0.7.0 native binary; the chain file lives under
`~/.clawft/chain/` (or the workspace overlay). Operator commands:

```bash
weaver chain status          # current chain head, length, last entry kind
weaver chain verify          # walk the chain and verify hashes / signatures
weaver chain export <path>   # export to JSON for offline review
```

Per ADR-022 (ExoChain Mandatory Audit), every privileged operation
should produce a chain entry. If you run an action and `weaver chain
status` does not show a new entry, that is a regression — file it.

### Governance (three-branch)

Governance is a permission-and-deferral layer in front of the chain.
ADR-033 (Three-Branch Governance) defines the model: legislative
(rules / capabilities), executive (the running agent / service), and
judicial (post-hoc review of `Defer` outcomes). Effects are scored
via the `EffectAlgebra` (ADR-034) so high-blast-radius actions can
be auto-deferred for human review.

```bash
weaver governance status            # current rule set summary
weaver governance pending           # actions Deferred and awaiting review
weaver governance review <id>       # accept or reject a deferred action
```

### Process table & supervisor (K1)

```bash
weaver agent list                  # PID / name / state for every agent
weaver agent inspect <pid>         # capabilities, parent PID, recent events
weaver agent spawn <type>          # spawn a new agent (subject to RBAC)
weaver agent stop <pid>            # graceful stop (SIGTERM-equivalent)
weaver agent restart <pid>         # supervised restart, chain-logged
```

### Health & lifecycle

```bash
weaver kernel start                # bring the kernel up
weaver kernel stop                 # graceful shutdown (drains agents)
weaver kernel status               # boot state + uptime
weaver health                      # aggregated health across services
weaver kernel logs                 # last N boot/runtime events
```

### Mesh (K6)

When the `mesh` feature is enabled, the kernel also listens on a
mesh transport for cross-node IPC, chain replication, and SWIM
heartbeats (ADR-039). The mesh runs as a phase-5d boot step and is
configured under `[kernel.mesh]` in `~/.clawft/config.json`.

```bash
weaver mesh status                 # peer count, view, listener address
weaver mesh peers                  # detailed peer table with last-heard timestamps
```

---

## DEMOCRITUS Cognitive Tick

DEMOCRITUS (named for the atomist; see ADR-047 Self-Calibrating
Tick) is the kernel's cognitive loop. It runs behind the `ecc`
feature flag and drives the ECC substrate through a SENSE → THINK
(fast EML) → DETECT DRIFT → THINK (exact RFF) → LOG → COMMIT cycle.
Operators encounter DEMOCRITUS in two places:

1. **Boot logs**: at phase 5b you will see lines like
   `DEMOCRITUS: tick interval calibrated to N ms` and
   `DEMOCRITUS: causal graph idle`.
2. **Runtime logs**: periodic INFO/WARN lines describing causal
   activity, drift detection, and cycle / stuck-state warnings.

### Reading the "still stuck" log line

`crates/clawft-kernel/src/cognitive_tick.rs` emits a `WARN` line of
the form:

```
DEMOCRITUS: still stuck after N checks: Stuck { net_change: 0.0, ... }
```

(or `Oscillating { ... }`) when the cycle detector observes that the
λ₂ coherence history has flat-lined or oscillated for a window.
**This is not a bug** in the overwhelming majority of cases.

It happens during normal operation in three scenarios:

- **Empty causal graph**: the kernel boots, ECC is enabled, but no
  agent has yet produced an event for the cognitive tick to chew on.
  The graph is empty, so coherence does not move, and the cycle
  detector reports "stuck" exactly as designed. The `idle-graph
  gate` (post-v0.6.19) suppresses this in steady state, but transient
  empty-graph windows during boot still surface one or two warnings.
- **Idle conversation**: the operator-visible agent has not received
  a new prompt for many ticks. The graph stops growing, λ₂ stops
  changing, and the detector reports "stuck" until the next event
  perturbs the graph.
- **Steady-state convergence**: the conversation has settled into a
  stable attractor. From the cycle detector's point of view this is
  indistinguishable from "stuck"; from the operator's point of view
  this is healthy behaviour.

What v0.6.19 did about it:

- **Edge-triggered logging**: entering and leaving the stuck phase
  always log; in-phase repeats log on an exponential-backoff
  schedule (`stuck_checks_since_log` doubles after each warning,
  capped at 256 checks). You should see the warning *less often*
  the longer it persists, not more.
- **Idle-graph gate**: when `causal.node_count() == 0` the loop
  skips the cycle-detector branch entirely, so a freshly-booted
  kernel does not spam the warning before any event has landed.
- **Bounded coherence history**: the rolling window cannot grow
  without bound, so memory does not leak through long stuck
  phases.

When the warning *does* mean something:

- The warning fires *immediately on entry* with `ConversationState::Stuck`
  *and* you have an active conversation — i.e. messages are being
  produced but coherence is not moving. That suggests the agent is
  looping on itself or wedged on a thinking step.
- The warning fires every tick despite the backoff (it shouldn't —
  if it does, the backoff state machine is broken).
- The warning correlates with elevated tick latency in
  `weaver health` or with a flapping process under
  `weaver agent list`.

In all three of those cases, capture the surrounding chain entries
(`weaver chain export`) and file an issue. Otherwise: ignore the
line, or filter it at your log aggregator.

The cognitive-tick code that emits these lines is gated as of commit
`5f888a1a` (v0.6.19) — see the kernel-governance audit
(`.planning/reviews/0.7.0-release-gate/02-kernel-governance.md`,
"Open questions and known limitations") for the full backstory.

---

## Configuration

Operator-facing kernel configuration lives in
`~/.clawft/config.json` under the `kernel` key (with workspace and
project-level overlays per the
[Configuration Guide](./configuration.md)). The most-touched fields:

```jsonc
{
  "kernel": {
    "features": ["exochain", "ecc", "mesh"],
    "mesh": {
      "listen": "tcp://0.0.0.0:9421",
      "seed_peers": ["tcp://10.0.0.2:9421"]
    },
    "chain": {
      "path": "~/.clawft/chain"
    },
    "governance": {
      "default_policy": "permit",
      "deferral_threshold": 0.7
    },
    "ecc": {
      "tick_interval_ms": "auto",
      "stuck_suppress_cap": 256
    },
    "ipc_tcp": {
      "enabled": false,
      "listen": "127.0.0.1:9420"
    }
  }
}
```

See [`docs/weftos/kernel-modules.md`](../weftos/kernel-modules.md)
for the full per-module reference and `kernel-modules.md` for
which features pull in which crate dependencies.

---

## Where to Go Next

- **Full architecture**: [`docs/weftos/architecture.md`](../weftos/architecture.md).
- **Per-phase status (live)**: [`docs/weftos/k-phases.md`](../weftos/k-phases.md).
- **ADRs that govern kernel design**: ADR-021 (CLI Kernel
  Compliance), ADR-022 (ExoChain Mandatory Audit), ADR-023
  (Assessment as a Kernel Service), ADR-033 (Three-Branch
  Governance), ADR-047 (Self-Calibrating Tick), ADR-048 (Kernel
  Phase Responsibilities), ADR-049 (WeftOS Kernel Architecture).
- **Audit (release-gate snapshot)**:
  [`.planning/reviews/0.7.0-release-gate/02-kernel-governance.md`](../../.planning/reviews/0.7.0-release-gate/02-kernel-governance.md).
- **Developer-facing kernel guide**:
  [`docs/src/content/docs/weftos/kernel-guide.mdx`](../src/content/docs/weftos/kernel-guide.mdx).

If you need a concept that is not covered here, check the
audit doc first — it is the most current snapshot of what
0.7.0 actually ships.
