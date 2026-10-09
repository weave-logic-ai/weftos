# ADR-115: An `rvm` workload kind: rvm's verify-before-load and isolation-claim contract over WeftOS's own engine

- **Status**: Proposed (2026-10-09)
- **Updated**: 2026-10-09. Owner decisions in the last section: `.rvf` sources are our own signed
  builds or approved packages whose publisher key we hold; WASM-only isolation is an option, not
  a requirement.
- **Deciders**: owner
- **Evidence**: [`docs/research/rvm-spike-2026-10.md`](../research/rvm-spike-2026-10.md), a hands-on
  spike of ruvnet/rvm at 510a82f (MIT OR Apache-2.0). Everything below about rvm comes from that
  spike or from rvm's own source, not from its README.
- **Builds on**: the governed placement layer (one layer for cogs, NPU/TPU jobs and local
  inference), the Wasmtime project kernel (ADR-103 topology; the wasmtime cards on the board),
  the Seatbelt and Linux-container drivers, ADR-114 (`weftos://` names; pinned `rev`), the chain.

## Context

The owner asked whether WeftOS can run rvm. rvm is a bare-metal Rust microhypervisor with host
crates for running verified RVF agents. The spike found:

- **The bare-metal kernel** does not boot under QEMU as shipped. The Makefile names the wrong
  ELF, the 64 KB stack is about 380 KB too small, and the boot stub never enables FP/NEON. With
  local fixes it reaches EL2 and emits witness records. It is research-grade and has no device
  story for our hardware.
- **`rvm-rvf`, `rvm-host`, `rvm-launch`** (crates.io 0.1.1, `no_std` plus `std`,
  `forbid(unsafe_code)`) are real and well designed:
  - verify-before-load as a type: `VerifiedPackage` can only be built from a passing report;
  - capability classes, with refusal rather than degradation;
  - an **honest isolation claim**: `wasm-only`, `os-sandbox+wasm` or `partition`, derived from
    what was actually engaged, never asserted;
  - a witnessed lifecycle (create, start, suspend, checkpoint, resume, terminate).

  But **nothing executes**. `start` validates the module and registers an agent record, with no
  WASM engine behind it. `HostedAdapter` engages no macOS or Linux mechanism; the embedder must
  apply the sandbox and then declare it. The only end-to-end test uses synthetic fixtures, not
  a signed `.rvf`.
- **`rvm-context-service`** works over TLS with uniform refusals. It has one scope fixed at
  start and a development key provider only. ADR-114 covers WeftOS's own naming, so this is not
  adopted.

WeftOS already has the parts rvm leaves to the embedder: an execution engine (the Wasmtime
project kernel), real OS confinement (Seatbelt on macOS, the Linux-container driver), placement,
and the chain.

## Decision

### 1. A new workload kind, `rvm`, for verified RVF agents

Placement gains a workload kind `rvm`. Launching one does the following, in order:

1. **Admit.** Resolve a pinned `weftos://<mesh>/agents/<id>?rev=sha256:…` (ADR-114) to the RVF
   bytes. The package must be signed either by our own release key or by a publisher whose
   public key is on the mesh's approved-publisher list. An unknown signer is refused. Verify them with `rvm_rvf::verify`, which writes its witness record. Build a
   `VerifiedPackage`. An unverified package, or one declaring a capability class WeftOS cannot
   enforce on that node, is refused before anything is allocated, and the refusal is chained.
2. **Confine.** Apply WeftOS's own confinement for the node: Seatbelt profile on macOS,
   namespaces, seccomp and landlock through the container driver on Linux. Only after it has
   taken hold, declare it to rvm (`HostedAdapter::engaging(...)`). The required isolation is a
   per-project (or per-agent) policy: `wasm-only` is an allowed choice, not a failure. A node
   that cannot confine runs the agent as `wasm-only` when the policy allows it, and refuses it
   when the policy or the package requires more.
3. **Execute.** WeftOS runs the module in the Wasmtime engine and maps rvm's granted capability
   classes onto host functions. rvm's `Instance` stays the lifecycle record of truth (start,
   suspend, checkpoint, resume, terminate), and WeftOS calls it at each transition.
4. **Witness.** rvm's 64-byte witness records for the instance are appended to the node's
   chain (hash-linked in rvm, and anchored by the chain entry). The dashboard shows the
   **isolation claim rvm derived**, never one WeftOS asserts.

### 2. Depend on the contract crates only, pinned and feature-gated

- `rvm-rvf`, `rvm-host`, `rvm-launch` and `rvm-witness` at exact versions, behind a non-default
  `rvm` feature, until the integration is proven.
- WeftOS does not depend on `rvm-kernel`, `rvm-wasm` as an engine, `rvm-context-service`, or the
  `ruvector` submodule. Their licences and transitive dependencies are audited before any
  feature is turned on by default.

### 3. Bare metal is watch-only

rvm partitions (`IsolationClaim::Partition`) are tracked but not targeted. When rvm boots on
hardware WeftOS runs, a node could host rvm under the same contract with the `partition`
claim. Until then nothing in WeftOS depends on it.

### 4. Findings stay local

The spike's findings (boot defects, the missing engine, stale README counts) are recorded in
the research note. Under the current rule we push only to our own remotes and do not open
upstream PRs or issues; the owner decides if and when to report them.

## Consequences

- WeftOS gains rvm's strongest ideas, verify-before-load as a type and honest isolation claims,
  without waiting for rvm to grow an engine or OS confinement.
- The Wasmtime engine work becomes a prerequisite for useful `rvm` workloads. The existing
  wasmtime cards (capability parity, precompile cache, acceptance under a real daemon) are on
  this path.
- A second witness format (rvm's 64-byte records) sits inside the chain. The chain entry is the
  anchor; rvm's own chain is checked with `rvm_witness::verify_chain` on read.
- If rvm's contract version changes (`HOST_CONTRACT_VERSION`, `LAUNCH_CONTRACT_VERSION`,
  `RVF_CONTRACT_VERSION`), the exact pins catch it at upgrade time.

## Phases

| Phase | Delivers | Done when |
|---|---|---|
| V1 | Admission only behind `--features rvm`: verify, `VerifiedPackage`, refusal of unenforceable classes, witness to chain | a test refuses an unverified package and a package needing an unenforceable class, and both refusals are chained |
| V2 | Confinement mapping: Seatbelt and the Linux driver engaged, then declared; the claim surfaced to the dashboard | the claim shows `os-sandbox+wasm` only when confinement took hold, and `wasm-only` otherwise |
| V3 | Execution through the Wasmtime engine with capability-class host functions; full lifecycle including checkpoint and resume | an RVF agent runs, suspends, checkpoints, resumes and terminates, with every step on the chain |
| V4 | Our own `.rvf` packer and signer (from WeftOS agent packages, signed with the release key) plus the approved-publisher key list; `weftos://` pinned resolution | an agent we built and signed, and one from an approved publisher, both launch from the dashboard; an unknown signer is refused |

## Owner decisions (2026-10-09)

1. **Sources of `.rvf` agents:** we build and sign them ourselves, or we use approved packages
   whose publisher public key we hold. The mesh keeps an approved-publisher key list (managed
   like the cog repository's pinned key, COG-008); nothing else is admitted.
2. **Isolation:** WASM-only isolation is an option, not a requirement. The required level is
   policy, set per project or per agent, and the dashboard always shows the claim rvm derived.
