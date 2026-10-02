# ADR-103: The Weave topology: roles, project kernels, instances and environments

- **Status**: Accepted (2026-09-30; decisions D1–D14 set by the owner 2026-09-30; implementation tracked on cards weave-topology-P0..P4, leaf track, cog-boundary audit)
- **Updated**: 2026-09-30. Phase 0 implemented (integrate/p0, Fable phase review `docs/research/daemon-topology/phase-0-review.md`). D11 implemented. Amendments A1–A6 below record what Phase 0 added beyond the original text.
- **Date**: 2026-09-30
- **Deciders**: Owner / platform
- **Depends-On**: ADR-022 (mandatory chain audit), ADR-025 (Ed25519 node identity), ADR-033 (three-branch governance), ADR-057 (substrate read ACLs), ADR-092 (rule distribution), ADR-094 (spawn permission), ADR-098 (process-compose), ADR-099 (governed workload placement), ADR-101 (inference kind), ADR-100 (cog kind)
- **Amends**: ADR-025 (node id hash, key location), ADR-099 (adds the `project` workload kind; node = machine), ADR-102 (token authority lives in the user role), the kernel guide (mesh port), `docs/plans/install-update-review.md` (restart targets)
- **Supersedes**: the undocumented per-project runtime rule from commit `0955e5b0` (CWD walk-up to `.weftos/runtime/`)
- **Analysis**: [`docs/research/daemon-topology/analysis.md`](../research/daemon-topology/analysis.md) rev 3 (sections cited as §N)

## Context

The ADRs assume one daemon ("the daemon", "the chain", "the node"). The code and the ops docs went per-project in April 2026 without an ADR: socket, PID file, `node.key` and `workloads.json` follow a CWD walk-up to `.weftos/`, while `chain.rvf` and `chain.key` resolve from `WEFTOS_RUNTIME_DIR` or `~/.clawft/` (§2). Two project daemons therefore share one chain file and signing key with no lock. The mesh node id is a fresh UUID each boot, three documents disagree on the node-id hash, a lost mesh bind is non-fatal, and because `~/.weftos/` exists for other reasons the walk-up treats `$HOME` as a project. On the owner's Mac there are four `node.key` files and one `chain.key`.

The owner wants projects to run granularly, with their own chains and governance, behind a machine-level mesh service; projects inside a container-like boundary where WeftOS does lifecycle and coordination but not the project's development work; WeftOS runnable inside WeftOS; ESP32 and WASM supported; and a way for an organization to stand up a primary instance that distributes work to subordinates, with development, staging and production instances that are shaped differently.

## Decision

### Roles, not a process count (§3, §4)

WeftOS is four roles. A profile decides which roles one process holds.

| Role | Owns | Must not own |
|---|---|---|
| **Machine mesh service** | the box node key, the one mesh listener, Noise sessions, discovery, cryptographic admission and revocation, routing to local tenants, signed machine facts, a small machine journal | user or project chains, secrets, tokens, governance evaluation (it consumes verdicts from the owning user daemon) |
| **User daemon** | the user key and user chain, the ADR-102 token authority, secrets, the model/accelerator access path (ADR-101 loopback proxy), shared heavy services (embeddings, indexes, voice), the project manifest, supervision of project kernels | the mesh listener when a machine service runs |
| **Project kernel** | the project key and chain, the tighten-only governance overlay, workspace config, the project's workload catalog, the boundary around the project | a mesh listener; duplicated heavy services |
| **Leaf / client** | one provisioned identity and one role | a chain; governance (enforced by its parent) |

Profiles: full shared host (service + a user daemon per user + a kernel per active project); personal host (machine and user roles in one `weaver`, project kernels as children); single-tenant host such as a Pi, container or server (one process, all roles); ESP32 leaf; browser client (behind the gateway, ADR-102 token, ephemeral certified key); WASI (leaf or collapsed host).

### Owner decisions

- **D1 Mesh port.** Default **9489**, "the weave". Configurable (`kernel.mesh.listen`). Replaces 9470 in code and 9421 in docs.
- **D2 Machine mesh service.** Optional, **on by default**. Standalone and single-tenant installs may run it collapsed inside the user daemon; `kernel.mesh.service = off` disables it.
- **D3 User binding.** A user key is **bound** to the OS uid on first registration over a peer-credential-checked local socket (`getpeereid` / `SO_PEERCRED`). The binding is recorded in the machine journal; an admin can revoke and rebind.
- **D4 Runtime roots are machine-wide.** Machine role: `/var/lib/weftos` (state) and `/var/run/weftos` (sockets). Per-user state under `~/.weftos/` (`run/`, `projects/`, `chain/`). `~/.clawft/` becomes legacy, read for migration only. `WEFTOS_RUNTIME_DIR` remains the full-isolation override for tests, probes and nested instances.
- **D5 Project id.** A stable ULID in `<root>/.weftos/project.toml`, mirrored in `~/.weftos/projects/<id>.toml`.
- **D6 Project chain.** In-tree at `<root>/.weftos/chain/` (gitignored), anchored into the user chain with `project.anchor` events and subscribable by others at `chain/<project-id>/` under ADR-057 ACLs.
- **D7 Keys.** A **certification chain**: machine key certifies user key; user key certifies project key; project or user certifies actor and leaf keys. No derivation, so projects can move between machines.
- **D8 Governance overlays** are **tighten-only**: a project (and a nested instance) may add denies and lower limits, never relax the parent's rules. Every chain event records the effective-rule hash.
- **D9 `project` is an ADR-099 workload kind** with pluggable sandbox drivers: `logical` (no sandbox, Phase 2), OpenShell or containers on Linux, Seatbelt or Apple `container` on macOS, wasmtime for WASM projects. The sandbox is the workload; the project kernel runs inside it as its supervisor; the user daemon supervises from outside (§6). *(Owner did not answer D9 explicitly; recorded as the analysis recommendation, reversible until Phase 4.)*
- **D10 Nested WeftOS** is isolated by default. A toggle, `weave.master = true`, marks an instance as the master of the WeftOS instances nested under it; the master manages their config (ports, registration level, governance cap). An inner instance never joins the outer mesh unless its master registers it.
- **D11 Node-id hash: SHA-256**, `node_id = hex(SHA-256(pubkey)[..16])`, as the code (`cluster.rs:323`) and ADR-099 already use. ADR-025 (SHAKE-256) and the ESP32 journal (BLAKE3) are corrected to match. *(Owner was unsure; SHA-256 chosen because it changes the fewest live ids.)* **Implemented in Phase 0**: one `node_id_from_pubkey` (`crates/clawft-kernel/src/node_id.rs`) used by mesh, cluster, registry and substrate; the substrate ACL still parses stored legacy `n-` ids.
- **D12 Commands outside any project** are decided by **governance**. The shipped default rule allows read-only commands (`status`, `doctor`, `health`, `version`, `project list`) against the user daemon and denies state-changing commands with "not in a project; run `weft project init` or pass `--project`". Operators may change the rule.
- **D13 Project kernels are processes by default.** In-process tenancy is the opt-in for small projects and the collapse for single-tenant hosts.
- **D14 Resolution.** `weft` finds a kernel by flag → env → manifest → user default, then confirms with a `kernel.status` handshake returning `{node_id, user_id, project_id, depth, parent}`. It never maps CWD straight to a socket, never falls back silently to an in-process kernel, and prints what it tried.

### Instances, federation and environments

An entity or organization stands up a **primary instance**: the WeftOS instance that holds its governance root (genesis) and decides where work goes. A primary may have **subordinate instances**, which are separate WeftOS instances (their own genesis and chain) that the primary delegates work to. Primaries and subordinates talk through the existing inter-instance surface: mesh admission with capability claims, `coordination.link` (ADR-022), chain subscription and anchoring (D6), and ADR-099 placement for delegated workloads.

For software projects the owner's model has three instances with different shapes:

| Instance | Shape | Why |
|---|---|---|
| **Development** | distributed: many developers' user daemons and project kernels on one shared mesh | collaboration; placement fans work out |
| **Staging** | a gate: a small, deliberately undistributed instance that receives a candidate, runs checks, holds the promote/reject decision and the intelligence around it | promotion must be governed, reproducible and auditable, not spread across whoever is online |
| **Production** | the deployed target, fed only by staging promotions | stability |

Promotion is a governed, chain-recorded hand-off between instances, not a mode of one instance. The same shape fits sensor fusion: leaves and edge nodes form a distributed instance; a fusion instance consumes their chains; downstream consumers subscribe.

### Core versus cog

Staging-as-a-gate, CI/CD, sensor fusion and similar purpose-specific behavior are **cogs** built on the OS primitives (ADR-100), not kernel features. The kernel keeps what every instance needs: identity and certification, chains, governance, placement, IPC and mesh, supervision, the token authority. A separate audit (card cog-boundary-01) lists everything in the WeftOS tree today that should have been a cog on those primitives. That audit is **advisory**: nothing is removed because it is listed; each item gets a keep, move later, or move now verdict with its cost, and core pieces that are correctly core are fixed in place rather than extracted.

## Amendments

- **A1 (Phase 0, legacy chain).** A kernel with no chain of its own keeps using the legacy `~/.clawft` chain and key rather than forking history. `chain.lock` guards every chain in use. While no `chain.lock` exists beside a legacy chain, the first adoption needs `weaver kernel start --adopt-legacy-chain` (also when the kernel is rooted at `~/.clawft` itself); a write within 120 s is refused as a probable lock-unaware writer. `--new-chain` starts fresh, and is refused where it would overwrite the legacy chain. Phase 1 `weaver migrate user-chain` reuses this flag family and lives beside `choose_default_chain`.
- **A2 (mic source policy).** The voice pipeline's input node is: an operator pin (`WHISPER_INPUT_NODE_ID` or `voice.mic_node_id`), else the single registered node publishing `sensor/mic`; two or more candidates, including one appearing after an automatic choice, stop speech-to-text until a pin is set. `node.register` carries no capability declaration yet; requiring a declared mic capability is on the leaf track.
- **A3 (voice principal).** Voice commands dispatch through the authorization chokepoint as an internal principal with `read, chat, write`, never `admin`. Whether cron mutations stay reachable by voice is decided in Phase 1 package G.
- **A4 (D14 scope).** Phase 0 delivered the "prints what it tried, no silent fallback" half of D14. Manifest resolution and the `kernel.status` handshake are Phase 1 packages A and D.
- **A5 (gateway bind).** The gateway's `0.0.0.0` default is changed to loopback with ADR-102 card 03; LAN exposure is an explicit choice.

- **A6 (decisions of 2026-10-01, owner accepted the recommendations).**
  - *D12 default:* `read_only` applies only to the `--profile user` daemon; every other root keeps `allow_all` until migrated; an explicit `kernel.governance.outside_project` always wins. Voice is inside the scope but is denied cron mutations. Denial `error_kind` is `project_required`.
  - *Phase 2:* one project key (node = chain = anchor key); invalid overlay at boot refuses to start; `idle_stop_secs` 1800 and never while agents, workloads or streams are active; children survive a user-daemon restart by adoption (`kernel stop` cascades unless `--keep-children`); mesh delivery to children is register-only until Phase 3; project chains start fresh with a parent-head link; existing projects opt in via `weaver project migrate-kernel`, default flips in Phase 3.
  - *Phase 3:* uid binding is TOFU on single-human-user machines and admin-approved otherwise; admission runs in observe mode for one release, then enforce after the Pi and ESP32 are redeployed; a designated `cluster_owner_uid` answers cluster verdicts, fail-closed, with a 10-minute grace for already-admitted peers; the owner's Mac adopts its existing `node.key` as the box key (new machines generate one); `user.key` keeps the migrated `chain.key` seed; the service runs as `weaver mesh serve` from a root-owned path; the local socket is 0666 with peer-credential authorization; service accounts `_weftos` (macOS) and `weftos` (Linux); user certificates last 24 h and renew at 12 h; Windows is design-only in Phase 3. On macOS a non-root LaunchDaemon writes `/var/run/weftos` only when its user is in the directory's group, so the installer creates the directory group-owned by the service group and adds the service account to it.

## Consequences

- One resolver for every runtime file; a second daemon for the same scope refuses to start; `$HOME` is never a project; the mesh node id is stable and derived from the node key.
- ADR-102's token authority moves to the user role; tokens can carry an optional `project_id` scope; the playground gets a project selector.
- `weaver update` restarts the user daemon found via its PID file and, per project, the kernels it supervises, recording binary path and sha in the manifest.
- The existing project-local daemon on the owner's Mac (other project, PID 70730) is migrated in Phase 1 by the owner (stop, register as the first manifest entry); it is not stopped automatically.
- Three tiers (service, user daemon, project kernels) can run different versions, so every local protocol (`mesh-local/N`, daemon RPC) is versioned and negotiated from Phase 1.
- The leaf path needs provisioned keys and a parent-side admission gate; until then unsigned leaf publishes stay a bring-up exception that doctor flags.

## Phases

| Phase | Scope | Card |
|---|---|---|
| 0 | One resolver (node key, chain, workloads, cluster peers, socket, PID, log); walk-up stops at a project marker (`.weftos/` plus `project.toml`, `.weftos/weave.toml`, `weave.toml` beside it, an existing `.weftos/runtime/`, or `.git`), never `$HOME`; `kernel.lock` single instance, `chain.lock` per chain, explicit first adoption of the legacy chain (`--adopt-legacy-chain`, `--new-chain`) and stale-socket recovery; `weft` prints the socket and stops silent fallback; node id from the Ed25519 key with SHA-256; mesh port 9489 and bind failure fatal; docs port fix | weave-topology-P0a..c |
| 1 | User daemon at `~/.weftos/run/` with machine and user roles collapsed; user chain migration from `~/.clawft/`; token authority; project manifest seeded from `~/.clawft/workspaces.json`; `weft project init/list`; resolution + handshake (D14); governance rule for D12; launchd/systemd user unit | weave-topology-P1 |
| 2 | Project kernels as children: project key certified on the user chain, project chain with anchors, tighten-only overlay, `project_id` in `GatePrincipal`, shared services via the user daemon, idle stop, `project` workload kind with the `logical` driver | weave-topology-P2 |
| 3 | `weaver mesh serve` as an OS service: peer-credential registration and user binding (D3), user certificates, machine journal, LaunchDaemon/systemd units, installer tier | weave-topology-P3 |
| 4 | Sandbox drivers (OpenShell/containers on Linux, Seatbelt/`container` on macOS, wasmtime); nested WeftOS with `weave.master` (D10), exercised by running this repo's dev kernel under the release user daemon | weave-topology-P4 |
| Leaf | Provisioned ESP32 keys, unified node id, signed publishes required, parent discovery, offline journal; browser via ADR-102 tokens | weave-topology-leaf |
