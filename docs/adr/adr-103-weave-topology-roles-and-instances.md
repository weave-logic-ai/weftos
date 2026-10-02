# ADR-103: The Weave topology: roles, project kernels, instances and environments

- **Status**: Accepted (2026-09-30; decisions D1–D14 set by the owner 2026-09-30; implementation tracked on cards weave-topology-P0..P4, leaf track, cog-boundary audit)
- **Updated**: 2026-09-30. Phase 0 implemented (integrate/p0, Fable phase review `docs/research/daemon-topology/phase-0-review.md`). D11 implemented. Amendments A1–A5 below record what Phase 0 added beyond the original text.
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
  - *D8 limits (Phase 2):* the kernel gate, the `agent.chat` tool gate and the spawned-agent gate follow live parent updates and `governance.reload`; the placement `WorkloadGate` is built from the effective rules when the control plane is built, so a push reaches it on the next rebuild. `max_processes` and `spawn_budget` (the concurrent sub-agent spawn cap) apply at boot; a push that changes them reports `restart_required`. The child trusts the user key in `<run>/<id>/user.pub` (written by the supervisor, outside the project tree) and falls back to its own certificate with a warning only while that pin is absent; this assumes nothing inside the project can write the run dir, which Phase 4 sandboxes must keep true.
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

- **A7 (Phase 2 contracts, package A).** The shared formats every Phase 2 package codes against; the file-level plan is `docs/plans/weave-topology-p2-plan.md` section 1.
  - *One project key.* `<root>/.weftos/project.key` is the node key, the chain signing key, the anchor key and the proof-of-possession key, so a child's `node_id` equals its `project_key_id` (first 16 bytes of SHA-256 of the public key, 32 hex). Because one key signs several message types, every signature carries a domain tag: `weftos-project-cert-v1`, `weftos-project-anchor-v1`, `weftos-mesh-local-pop-v2`.
  - *Certificate.* The user key signs `"weftos-project-cert-v1\n"` plus canonical JSON (sorted keys, no whitespace, no `sig`) of `{v, type, project_id, project_pubkey, project_key_id, user_key_id, user_pubkey, serial, issued_at, expires_at}`. No root path is inside, because projects move. `expires_at` is null in Phase 2; the verifier honours it. The first key seen for a project id is certified (TOFU against the manifest); a different key for the same id needs an explicit `project.rekey`.
  - *Anchor statement.* The project key signs `"weftos-project-anchor-v1\n"` plus canonical JSON of `{project_id, project_key_id, cert_serial, seq, chain_id, head_hash, head_seq, rule_hash, at, prev_anchor}`; `prev_anchor` is the SHA-256 of the previous accepted statement including its `sig`. The user daemon attests "this key claimed head X at time T"; it cannot verify X without subscribing to the project chain, and says so.
  - *Child root.* `RootSource::Child { id, project_root }`: ephemeral files under `~/.weftos/run/<id>/` (socket, pid, lock, log, `spawn.json`, `parent-policy.json`, `state.json`), durable files under `<root>/.weftos/` (`project.key`, `project.cert.json`, `chain/`, `state/`, committed `overlay.toml`). The project walk-up never selects a child.
  - *Overlay.* Tighten-only: `deny` and `require_approval` union, numeric limits `min(parent, overlay)`, booleans `parent OR overlay`; `permit`, `deactivate`, a parent rule id (compared trimmed and case-insensitively) or a looser limit is a hard error naming the key, and an invalid overlay refuses boot. The effective-rules hash is `SHA-256("weftos-effective-rules-v1\n" + canonical JSON of {parent_hash, overlay_hash, merged rules sorted by id, merged limits})` and rides on every child chain event as `rule_hash`.
  - *mesh-local/1 (Phase 2 child registry).* `mesh.challenge`, `mesh.register` (project key proof over `weftos-mesh-local-pop-v2\n<op>\n<user_key_id>\n<nonce>\n<project_id>` (op `register` or `rekey`, nonce 32 lowercase hex issued by the user daemon, id a canonical ULID, all validated) plus the spawn nonce), `mesh.heartbeat`, `mesh.unregister`; a session expires after three missed heartbeats and a second live session for one project is refused. Until Phase 3 peer credentials, the guard is the 0600 socket, the spawn nonce and the proof of possession. Delivery to a child is register-only in Phase 2.
  - *Verified project.* `GatePrincipal` gains `project_id` and `instance_id` (absent on old payloads). `project_id` is stamped only from a `ProjectAttestation` that the daemon's `VerifiedProject` produces (cross-crate sealing is not possible, so this is by construction path plus a grep test): the kernel's own bound project, a validated token scoped to a project, or a user-signed forward header. A bare `Request.project` stays a claim. This is an isolation guard between one user's projects, not a boundary against a hostile same-uid process.
  - *Verified project, as built (package I).* The forward header is `{project_id, issued_at_ms, sig}`, signed by the user key over `"weftos-project-forward-v2\n<project_id>\n<issued_at_ms>\n<method>\n<sha256 of canonical params>\n<target child key id>"`, so it authorises one request to one child; the child accepts it once and within 5 s, and the TCP relay strips it from remote lines. Refusal kinds: `forward_unavailable`, `forward_bad_signature`, `forward_expired`, `forward_replayed`, `project_scope_mismatch`, and the reserved `forward_wrong_target` (a header signed for a sibling child currently reports as `forward_bad_signature`). A method containing a newline is refused on both sides; the proxy stamps the header as its last step, after any param rewriting. A token scoped to one project cannot act as another (`project_scope_mismatch`). Honest limit: same-uid callers can still lie via `Request.project` on the claim path (no token, no forward header, unbound daemon), which keeps its Phase 1 meaning and is logged as `claimed`; `VerifiedProject` is an isolation guard between one user's projects, not a boundary against a hostile local process (Phase 3 peer credentials, Phase 4 sandboxes).

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
