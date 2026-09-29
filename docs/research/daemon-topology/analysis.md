# Daemon topology: machine, user, project, and the places with none of them

**Date:** 2026-09-29 (rev 2: adds machine mesh service, ESP32/WASM profiles,
OpenShell, project boundary contract, nested WeftOS)
**Branch reviewed:** 0.8-metaharness
**Scope:** architecture review and recommendation only. No code, daemon, or
dotfile was changed. Every claim about current code cites `file:line`.
Prior-art claims are tagged [fetched], [snippet] or [recall].

## 1. Answer first

WeftOS should be described as **four roles**, not a process count:

| Role | Owns | Runs as |
|---|---|---|
| **Machine mesh service** | the box's node key, the one mesh listener, Noise sessions, discovery, peer admission (crypto + revocation), routing to local tenants, machine hardware facts | OS service, unprivileged service user |
| **User daemon** | user key, user chain, gateway and token authority, secrets, model/accelerator access path, placement client, tenant registry `~/.weftos/` | one per user per machine |
| **Project tenant** | project key, project chain, governance overlay (tighten-only), workspace config, workload catalog, the project's own boundary | inside the user daemon by default; optionally an isolated child kernel inside a sandbox |
| **Leaf / client** | one provisioned identity, one role, no chain (journal anchored upstream) | ESP32 firmware, browser tab, WASI component |

The number of processes is a **profile**, not a rule. A Mac shared by
several people runs three tiers; a Pi, a container or a server collapses
all three roles into one process; an ESP32 runs the leaf role only and a
browser tab runs a client role behind a gateway. What stays fixed across
profiles is the layering of identity (machine certifies user certifies
project certifies actor), the direction of chain anchoring (leaf → project
→ user → machine journal), and the rule that governance decisions are made
by a user daemon or project kernel, never by the mesh service and never by
a leaf.

The owner's instinct that governance, chains and workloads are per project
is right, and so is the container-like boundary. The step from "granular
per project" to "an independent daemon that is also a mesh node per
project" is the one to avoid: a project is a **tenant and a workload**
(ADR-099 vocabulary), the machine is the **node**. Section 8 has the phased
migration; section 10 what could not be verified.

## 2. What exists today (verified)

Three partially overlapping answers to "where does this daemon's state
live", and they do not agree:

| State | Resolution today | Scope in practice |
|---|---|---|
| Socket, PID, log | `WEFTOS_RUNTIME_DIR` -> nearest ancestor `.weftos/runtime/` -> `~/.clawft/` (`crates/clawft-rpc/src/protocol.rs:47-72`) | per project when CWD is inside one |
| Node key `node.key` | parent dir of the socket (`crates/clawft-weave/src/daemon.rs:965-970`, `crates/clawft-weave/src/node_identity.rs:31,76`; also `crates/clawft-weave/src/commands/kernel_cmd.rs:401-403`) | per project |
| Workload catalog `workloads.json` | same runtime dir (`daemon.rs:971-973`) | per project |
| Chain checkpoint, `chain.rvf`, `chain.key`, anchors | explicit config -> `WEFTOS_RUNTIME_DIR` -> `~/.clawft/` (`crates/clawft-types/src/config/chain_paths.rs:32-61`; `crates/clawft-types/src/config/kernel.rs:1172-1184`; key derived by extension at `crates/clawft-kernel/src/boot.rs:886-893`) | **per user**; the `.weftos/runtime` walk-up is not consulted |
| Mesh node id | `Uuid::new_v4()` at every boot (`boot.rs:304-306`) | per process lifetime |
| Cluster peer file | literal relative path `.weftos/runtime/cluster_peers.json` (`boot.rs:756`) | whatever the daemon's CWD was |
| Noise static key | `noise_key_path` or ephemeral per boot (`kernel.rs:852-855`) | per process |
| Config | defaults < `~/.clawft/config.json` < `.clawft/config.json` (`docs/guides/workspaces.md:129`); plus cwd `clawft.toml`/`weave.toml` and `CLAWFT_CONFIG` (`docs/adr/adr-070-mcp-registry-ownership.md:80`; `crates/clawft-weave/src/commands/mod.rs:43-45`) | per user + workspace overlay |
| Workspace registry | `~/.clawft/workspaces.json` (`crates/clawft-types/src/workspace.rs:3`) | per user |
| Governance rules | 22+ genesis rules compiled in (`crates/clawft-kernel/src/governance.rs:2335-2501`), anchored at boot (`docs/adr/adr-033-three-branch-governance.md:24-31`); `RuleDistribution` is in-memory LWW with no file I/O (`crates/clawft-kernel/src/rule_distribution.rs:1-19`); gossip wiring is a follow-up (`docs/adr/adr-092-governance-rule-distribution.md:34-36`) | per process, seeded from the binary |

Node id derivation is specified three different ways: SHAKE-256 in ADR-025
(`docs/adr/adr-025-ed25519-node-identity.md:14`), SHA-256 in code and in
ADR-099 (`crates/clawft-kernel/src/cluster.rs:323`; `docs/adr/adr-099-governed-workload-placement.md:24`),
and truncated BLAKE3 `n-<6hex>` for ESP32 nodes
(`.planning/sensors/JOURNALED-NODE-ESP32.md:86,108`). ADR-025's key file
`$WEFTOS_DATA/identity.key` (`adr-025:28`) exists nowhere; `WEFTOS_DATA` is
undefined in the tree.

The docs already disagree with each other about the topology:
`docs/deployment/release.md:403-410` defines three scopes (`~/.clawft/` =
user, `<workspace>/.clawft/` = project, `.weftos/runtime/` = "daemon, next to
the project that owns the daemon"); `docs/guides/weftos-deployment-sops.md:207`
says "single kernel instance per project"; `:667` says "no multi-tenant
isolation within mesh, all connected projects see all events"; ADR-021 says
"single daemon process eliminates race conditions"
(`docs/adr/adr-021-cli-kernel-compliance.md:68`); ADR-071 says a shared
multi-tenant daemon behind one UDS is unacceptable without per-caller scoping
(`docs/adr/adr-071-wasm-panel-auth.md:22-28`) and proposes scoped panel
tokens (`:35-60`). Per-project process-compose is already the unit for a
project's own processes (`docs/adr/adr-098-environment-process-compose.md:34-36`).

Ports and defaults: mesh off by default, `0.0.0.0:9470` TCP when on
(`kernel.rs:823-825,858-864`; `docs/guides/kernel.md:389-391` still says
`9421`); RPC-over-TCP off, `127.0.0.1:9471`, refuses non-loopback without a
bearer (`kernel.rs:689-697,736-738`); gateway `18789`
(`crates/clawft-types/src/config/mod.rs:837-844`); MCP HTTP `8742`, separate
process (`docs/adr/adr-102-gateway-health-and-api-playground.md:22`); local
models `127.0.0.1:8090` and `:11434` (`crates/clawft-types/src/config/local_llm.rs:24-29`).
A mesh bind failure is logged and boot continues (`crates/clawft-kernel/src/mesh_tcp.rs:208-213`;
`boot.rs:621-627`). Capability advertisement is a closed, Ed25519-signed
allow-list (`crates/clawft-kernel/src/capability_claim.rs:1-30`). No peer
credential check (`SO_PEERCRED`/`getpeereid`) exists anywhere in `crates/`.

Liveness and discovery: PID-file check then stale-socket probe, per runtime
dir (`daemon.rs:646-658,877-891,914`); `DaemonClient::connect()` dials
`socket_path()` only (`crates/clawft-rpc/src/client.rs:54,193`); on failure
commands fall back to in-process mode (`crates/clawft-cli/src/commands/agent.rs:297`).
No lock on the chain file; sequence in memory (`crates/clawft-kernel/src/chain_storage.rs:980`);
JSON checkpoint is a whole-file rewrite (`crates/clawft-kernel/src/chain.rs:1632`).

Leaves and clients today:

- **ESP32 (no_std, `crates/clawft-edge-pad`)**: connects by plaintext TCP to a **compiled-in daemon IP** and port 9470 (`crates/clawft-edge-pad/src/mesh.rs:62-63,283-285`), identifies itself by a **MAC-derived id**, no keypair (`mesh.rs:64-66`), speaks length-prefixed JSON `MeshIpcEnvelope` and subscribes to `mesh.leaf.<id>.push` (`mesh.rs:22-33`; `docs/leaf-push-protocol.md:38-55`). The protocol calls a leaf "a peer mesh node" (`leaf-push-protocol.md:16-20`) and accepts unsigned publishes "for bring-up" (`:66-68`). The std variant (`crates/clawft-edge-pad-idf`) adds NVS and Wi-Fi provisioning (`crates/clawft-edge-pad-idf/src/main.rs:45,68`), no key yet. The journals specify the target: Ed25519 node key in NVS/eFuse provisioned at first flash, node and actor keys distinct, actor paths `substrate/<actor-id>/**` private by default under ADR-057 (`JOURNALED-NODE-ESP32.md:57-81`; `.planning/actors/JOURNALED-ACTOR-INKPAD.md:34-41,112-150`). T0 devices cannot run `clawft-kernel` at all (`.planning/symposiums/cognitum-seed-gaps/tiered-kernel-profiles.md:210-220`).
- **Browser (`crates/clawft-wasm`, `--features browser`)**: a real `AgentLoop` in the tab, channels/cron/CLI/native plugins excluded (`docs/adr/adr-083-browser-wasm-support.md:46-48`); no node identity, no mesh connection, no chain in the crate (grep of `crates/clawft-wasm/src/lib.rs`, `platform.rs`, `http.rs`); talks to LLM providers directly or through a first-party proxy (`docs/browser/security.md:246-269`). ADR-025 already allows a session-scoped ephemeral keypair for browser nodes (`adr-025:72`). The browser leaf renderer plans a WebSocket mesh transport (`crates/weftos-leaf-canvas/src/lib.rs:50`) and the kernel has one for "browser-based nodes" (`crates/clawft-kernel/src/mesh_ws.rs:1-5`).
- **WASI (`wasm32-wasip2`, ADR-044)**: the same `clawft-wasm` without browser FFI (`adr-083:70-76`), for wasmtime/edge hosts. `crates/clawft-wasm-host` is the *native* side: a plugin sandbox with permission, SSRF, path and rate-limit enforcement plus a per-plugin host-call audit (`crates/clawft-wasm-host/src/lib.rs:1-12`).
- **Cognitum Seeds / RuOS**: governed at the adapter, tenant derived from the token, adapter-attested facts (`docs/research/mesh-placement/fleet-compat.md:37,91-93`; `docs/adr/cog-001-cog-workload-kind.md:59-68`).

On this machine (read-only, 2026-09-29): PID 70730 runs from CWD
`weave-coordinator`, socket in that project's `.weftos/runtime/`, listening
on `*:9470` (its `weave.toml` enables mesh with Noise); `~/weftos/.weftos/runtime/`
has a stale socket and dead PID 50246; `~/.clawft/` has the 46 MB chain and
chain key and no socket; `~/.weftos/` exists for other reasons
(`crates/clawft-app/src/registry.rs:147`, `crates/clawft-gui-egui/src/shell/window_manager.rs:192`,
voice models), so the `.weftos/` walk-up matches `$HOME` for any command run
outside a project and a third runtime dir appeared (`~/.weftos/runtime/node.key`,
created today). Four node keys, one chain key, one live daemon.

## 3. The four roles and the profiles that collapse them

| Profile | Processes | Machine role | User role | Project role | Leaf/client role |
|---|---|---|---|---|---|
| **Full host** (shared Mac/Linux box, several humans) | mesh service + N user daemons (+ optional isolated project kernels) | OS service | one per user | tenant in user daemon, or child kernel in a sandbox | n/a |
| **Single-tenant host** (personal Mac default, Pi 5, container, server) | one `weaver` process | in-process `MeshService` (`boot.rs:706`) | same process | tenants | n/a |
| **Constrained leaf** (ESP32 S3/C3) | one firmware image | attaches to a parent node | owner user by provisioning | optional project binding by provisioning | node or actor identity, no chain |
| **Browser tab** | page + WASM | none; reaches a gateway | the user who holds the token | project from the token scope | origin-scoped ephemeral key, no chain |
| **WASI / edge component** | one component under wasmtime | attaches to a parent node (or runs collapsed if it has FS + sockets) | provisioned | provisioned | node identity, journal if no FS |

The collapse is mechanical: the mesh service is already a `SystemService`
registered inside the kernel (`boot.rs:702-717`); making it also runnable
as a separate binary that speaks the same local registration protocol
(section 4) is the only new code path. Profiles are selected in
`weave.toml` (`profile = "node" | "edge"` already exists,
`tiered-kernel-profiles.md:29-44`) plus `roles = ["mesh","user","project"]`.

## 4. The machine mesh service

### 4.1 What it owns

- **Machine node key** (`/var/lib/weftos/mesh/node.key`, 0600, service user): the ADR-025 identity remote peers see. One per box.
- **The listener(s)**: one mesh port (pick one number; the code says 9470, the guide and the owner say 9421), transports TCP/WS/QUIC (`kernel.rs:827-832`), Noise static key (`kernel.rs:852-855`), session table.
- **Discovery**: `_weftos._tcp` advertisement and browsing (`crates/clawft-kernel/src/mesh_mdns.rs:22`), Kademlia when enabled, seed peers, SWIM heartbeats for the box (`mesh_heartbeat.rs`).
- **Cryptographic admission**: genesis-hash match, JoinRequest signature, timestamp skew, revocation list (`adr-025:45-53`). It enforces admission results; it does not decide policy (4.2).
- **Routing table**: which local daemon registered which addresses (user ids, project ids, topic prefixes) and the peer-credential uid that registration came from.
- **Machine facts** for ADR-099: arch, OS, memory, accelerators, probed locally and signed by the machine key (`adr-099:56-92`). Capacity is advertised once per box, which is the whole point.
- **Machine journal**: a small signed append-only log of admissions, registrations and revocations. It is not a chain a user or project depends on; each user daemon anchors the journal head into its own user chain so ADR-022 is satisfied without the service owning anyone's chain.

### 4.2 What it must not own

User or project chains and keys; LLM/API secrets and gateway tokens
(ADR-102 stays in the user daemon); governance evaluation (`GateBackend`
lives in user daemons and project kernels; the service consumes decisions:
"admit peer X", "user U may publish on prefix P"); agent execution; model
access; project files; any HTTP surface beyond a loopback health check.
The `cluster.join` gate check (`adr-025:50`) is answered by the user daemon
that owns the cluster membership the peer is joining, over the local
registration channel, and the service caches the verdict with a TTL.

### 4.3 Local registration and user isolation

- Socket `/var/run/weftos/mesh.sock`, created by launchd/systemd with mode 0660 and a `weftos` group, or 0666 with all authorization done by peer credentials.
- On accept, the service reads the peer uid: `SO_PEERCRED` (Linux), `LOCAL_PEERCRED`/`getpeereid` (macOS/BSD); on Windows the named pipe's client process token (unverified detail, section 10). Nothing in `crates/` does this today; it is the one new primitive the design needs.
- `mesh.register { protocol: "mesh-local/1", user_pubkey, user_cert?, addresses: [user_id, project_id...], topic_prefixes, version, build_sha }`. First registration for a uid binds `uid <-> user_pubkey` (TOFU, or admin approval on shared boxes; owner decision). A later registration for the same uid with a different key is refused until revoked. Only one daemon per uid may hold the user address; isolated project kernels register their project address with a certificate signed by the user key.
- The service returns a **machine-signed certificate** for the user key (`machine_key.sign(user_pubkey, uid, node_id, expiry)`), which the user daemon includes in anything it signs for remote peers.
- Isolation: routing keyed by (uid, address); a daemon can subscribe only to prefixes it registered or that another tenant's ADR-057 ACL grants; per-uid inbound rate limits; no shared runtime dir; users' daemons never talk to each other except through the service with ACL checks.

### 4.4 Addressing

`weft://<node-id>/<user-id>/<project-id>/<topic>` with `node-id` =
hash(machine pubkey), `user-id` = hash(user pubkey), `project-id` = the
manifest ULID (or hash of the project pubkey; owner decision). Wire: add
`dest_scope { user, project }` to `MeshIpcEnvelope` (`crates/clawft-kernel/src/mesh_ipc.rs:40-100`
per ADR-099's table) and route unscoped messages to the machine's default
daemon only when the box runs one user daemon; otherwise reject. Substrate
paths keep ADR-057's rule "prefix = publisher key id, enforced by
signature" (`adr-057:35`) and extend the set of publishers from node keys to
any certified key: machine, user, project, actor. The Inkpad journal already
does exactly this for actors (`JOURNALED-ACTOR-INKPAD.md:141-150`).

### 4.5 Privilege, install, update, versioning

- **No root.** The port is above 1024; the socket and state directory are created by launchd (`LaunchDaemon` with `UserName`/`GroupName`) or systemd (`User=weftos`, `RuntimeDirectory=weftos`, `StateDirectory=weftos`). mDNS on 5353: use the OS responder where present (Bonjour on macOS, Avahi/systemd-resolved on Linux) and only bind our own with `SO_REUSEPORT` when none is. No capabilities beyond bind/listen.
- **Install**: one admin step (`weaver mesh install-service`) on shared boxes; optional everywhere else because the single-tenant profile collapses the role into the user daemon. `install-update-review.md`'s receipt records three tiers: service binary + version, per-user daemon binary + version, per-project isolated kernels + versions.
- **Versioning**: `mesh-local/N` is negotiated at register; the service is the slow-moving component and must accept older and newer daemons within a stated window; feature bits, not a lockstep. `daemon_guard.rs` already compares build stamps per connection (`crates/clawft-cli/src/commands/daemon_guard.rs:1-16`); reuse its wire key.
- **Restart order**: service restart drops sessions, daemons reconnect with backoff, peers see a heartbeat flap and no chain effect. Daemon restart does not touch the service. The installer restarts the service only when its own package changed.

### 4.6 What remote peers see

One node per box with one signed fact set (ADR-099 §2). Tenants show up as
**addresses under the node** and as **certified keys**, not as nodes.
Capability claims (`capability_claim.rs`) gain a `scope` field
(`node|user|project|actor`) and a certificate chain so a peer can verify
"this `llm` claim is from user U on node N". ADR-092 rule gossip stays
node-to-node and carries genesis-level and user-level rules; project overlays
are not gossiped (they travel with the project). ADR-099 placement treats
the node as the capacity unit and `(user, project)` as the tenant that owns
a workload instance; `instance_id = (manifest hash, config hash, node id)`
(`adr-099:52`) is unchanged.

## 5. Identity layering

| Key | Where | Certified by | Signs |
|---|---|---|---|
| Machine node key | `/var/lib/weftos/mesh/node.key` (service) or `~/.weftos/run/node.key` (collapsed) | genesis / cluster join (ADR-025) | node facts, admissions, machine journal, user certs |
| User key | `~/.weftos/user.key` | machine key (cert issued at register) | user chain, tokens (ADR-102), project certs, capability claims scoped `user` |
| Project key | `<root>/.weftos/project.key` (gitignored) | user key (`project.register` event on user chain) | project chain, project-scoped claims, anchors |
| Actor / leaf key | device NVS or eFuse; browser session key | user key (provisioning) or project key | substrate publishes under `substrate/<actor-id>/` |

Certification, not derivation: an HKDF child of the machine key would tie a
project to one box and force re-keying on rotation; a project moves between
a Mac and a Pi and must keep its key. Chain of custody is a certificate chain
recorded on the chains: machine journal holds user certs, user chain holds
project certs, project chain holds actor certs it issued. Node id
derivation must be one function used by the kernel, the ESP32 journal and
the ADRs (today three); fix ADR-025 to say SHA-256 or fix `cluster.rs:323`,
and drop the BLAKE3 variant from the ESP32 plan.

## 6. The project boundary contract

The owner's requirement: a project may be split into a container/sandbox
structure like OpenShell; WeftOS owns lifecycle and coordination and stays
out of the project's own development work. Defined as a contract that
holds in every profile, with enforcement strength varying by profile.

**Crosses the boundary (WeftOS side):**
- Control API: create, start, stop, restart, update, inspect, logs, snapshot; carried over the user daemon socket (or the mesh service for remote nodes) and chained as `workload.*` events (`adr-099:117`).
- Capability grants: signed claims (ADR-066/094) delivered at start and revocable; the project cannot mint them.
- Chain and audit: the project chain lives with the project; anchors and governance decisions go up; OCSF-style audit of boundary crossings recorded by the supervisor side.
- Mounts: project root read-write, caches read-write, model artifacts and shared indexes read-only, secrets injected at start and never persisted inside.
- Network egress policy: allow-list per project (OpenShell-style), including the model endpoint.
- Model and accelerator access path: a fixed loopback endpoint inside the boundary (ADR-101's `127.0.0.1:8090` proxy, `adr-101:87-89`) that the user daemon terminates and places; accelerator device access is a placement decision, not something the project configures.
- Identity: the project key and certificate are inside; the boundary supervisor verifies signatures on the way out.
- Mesh routing: messages addressed to the project are delivered by the parent; the project never listens on the network.

**Stays inside:** toolchain, build system, code, tests, the agents' internals and prompts, the project's own process-compose (ADR-098), its MCP servers, its memory and vector stores, its ports on loopback.

**How the project daemon relates to the sandbox:** the sandbox **is the
project's workload** (a new ADR-099 kind `project`: spec = root, driver,
policy, resources); a project kernel runs **inside** it as the project's
supervisor (the ADR-098 process-compose grows into it); the user daemon is
the supervisor **outside** (create, policy, restart, audit), which is the
OpenShell gateway+supervisor shape. In `inproc` mode the same contract is
enforced logically (ACL, budgets, `project_id` on every call); the API is
identical, only the enforcement primitive changes. Placing a `project`
workload on another node (Pi instead of Mac) is then the ordinary placement
path.

**Drivers by platform:** Linux: OpenShell (Docker/Podman/Kubernetes/libkrun
microVM), plain containers, landlock + seccomp for a lightweight mode.
macOS: Seatbelt profiles for the light mode, Apple `container`/Docker
Desktop/Lima for the heavy mode. Windows: AppContainer/job objects, WSL2
containers. WASM: a wasmtime component with WASI capabilities; the
`clawft-wasm-host` sandbox (`crates/clawft-wasm-host/src/lib.rs:6-12`) is
already this driver for plugins. ESP32 and browser: no container exists;
the boundary is identity plus the parent-side write gate and ACL, which is
the profile's honest enforcement level.

## 7. Nested WeftOS

A project may be WeftOS development (this repo) or a product with its own
kernel. Treat the inner instance as an **isolated project kernel** (section
6) that happens to be a full WeftOS:

- **Runtime**: inner runs with `WEFTOS_RUNTIME_DIR=<root>/.weftos/runtime/inner` (the isolation `04516c09` added for probe daemons, with tests in `crates/clawft-kernel/tests/chain_runtime_isolation.rs`), its own socket, own `node.key`, own chain, own genesis. It binds no TCP by default.
- **Identity and mesh**: the inner node key is certified by the outer *user* key, never the outer machine key reused. Default is **isolated** (no mesh). Opt-in level 1: register with the outer user daemon as a tenant address (the vcluster shape: inner control plane, outer transport). Opt-in level 2: appear as a distinct virtual node behind the same machine service (the Docker-in-Docker shape), for testing multi-node behaviour on one box.
- **Chains and governance**: inner has its own constitution, so by ADR-033 (`adr-033:37`) it is a different cluster; the outer never syncs rules with it. The outer caps it at the boundary (mounts, egress, grants, budgets), the inner enforces its own within. Inner chain heads anchor into the outer project chain via `project.anchor`.
- **CLI and MCP resolution**: order flag `--runtime`/`--project` > `WEFTOS_RUNTIME_DIR` > project manifest entry > user default. Then a **handshake**: `kernel.status` returns `{node_id, user_id, project_id, depth, parent}` and `weft` prints which instance it reached and refuses scope mismatches. Editor MCP servers are launched with the manifest entry pinned, never by CWD guessing.
- **Ports and versions**: inner gets no ports unless the outer allocates them in the manifest; inner and outer versions differ by design (dev vs release), which is why the local protocols (`mesh-local/N`, daemon RPC) need version negotiation and why the inner must never touch the outer's chain files directly.
- **Testing**: today's `WEFTOS_RUNTIME_DIR` isolation is depth-1 nesting with mesh off; extend the tests to start an inner kernel under an outer and register it.

Prior art: mounting the host's `docker.sock` into a container is
root-equivalent on the host, and true DinD needs `--privileged`; sysbox and
kind/KinK/vcluster nest by giving the inner instance its own control plane
over a scoped channel ([Baeldung DinD](https://www.baeldung.com/ops/docker-in-docker) [fetched],
[vcluster nesting](https://www.vcluster.com/docs/architecture/nodes) [snippet]).
Lesson: the inner WeftOS gets a scoped registration channel, never the
outer's raw control socket.

## 8. Profiles in detail

| Profile | Identity source | Chain events go to | Governance | Mesh address | Update |
|---|---|---|---|---|---|
| Full host | machine key by service; user key at first login; project key by `weft init` | machine journal; user chain; project chain; anchors upward | user daemon and project kernel evaluate; service enforces admission | `weft://node/user/project` | service by admin installer; daemons by per-user updater; isolated kernels by parent |
| Single-tenant host | all three generated by one `weaver` on first boot, same files | same three files, one process | in-process | same | one binary, one restart (closes `install-update-review.md:191-197`) |
| ESP32 leaf | Ed25519 in NVS/eFuse at provisioning (`JOURNALED-NODE-ESP32.md:57-81`); owner user cert and optional project binding provisioned with it | no local chain; signed publishes chained by the parent user daemon that admitted them; a tiny ring journal for offline periods, replayed and anchored on reconnect | parent-side write gate and ACL; on-device only reflexes | its own node id, routed via the parent node (`weft://<leaf-node>/...` with parent as next hop); discovers the parent by mDNS or provisioned address, replacing the compiled-in IP (`mesh.rs:62`) | OTA as an ADR-099 workload later; flash today |
| Browser tab | origin-scoped ephemeral key (`adr-025:72`) certified by the user daemon through the ADR-102 token flow | none locally; the gateway chains on the user's behalf with `project_id` from the token | daemon-side | not a node; a client of `<gateway>/mcp` or of `mesh_ws` when it acts as a leaf renderer | page reload |
| WASI / edge | provisioned by host env or secret; can collapse to single-tenant if it has FS and sockets | local chain if FS, else journal upstream | local if kernel present, else parent | node behind a parent or standalone | host redeploy |

The `~/.weftos/` manifest, per-project chains with subscription, and
project-level governance still hold when a project spans a Mac, a Pi and
several ESP32 leaves: the manifest entry lists the project's **nodes and
leaves** (by node id and certificate), the project chain is the one the
project kernel writes wherever it is placed, other nodes subscribe to
`chain/<project-id>/` through the ACL, and the overlay travels with the
project because it is a file under `<root>/.weftos/`. Leaves never hold the
overlay; the parent applies it to what they publish.

## 9. Prior art

- **NVIDIA OpenShell** [fetched: blog; snippet: repo/docs]. Three parts: a Gateway that "manages the lifecycles and policies of many sandboxes", a Supervisor "paired with each sandbox, it runs outside the agent workload and checks outbound requests against policy", and a Sandbox with "kernel-level controls over its filesystem and processes, and no network path except through the supervisor". Policy is YAML compiled to OPA/Rego evaluated per outbound request; filesystem and process policy are locked at sandbox creation, network and inference policy hot-reloadable; real credentials stay outside the workload and are bound to authorized requests; decisions are logged as OCSF; drivers are Docker, Podman, Kubernetes and an experimental libkrun microVM; wraps Codex, Claude Code, Pi, Hermes; version 0.0.x; macOS and Windows are not mentioned. ([blog](https://developer.nvidia.com/blog/add-runtime-controls-to-ai-agents-with-nvidia-openshell/), [repo](https://github.com/NVIDIA/OpenShell), [docs](https://docs.nvidia.com/openshell/)). **For WeftOS:** pattern-copy the boundary (supervisor outside the workload, credential binding, inference rerouting, which is ADR-101's proxy), integrate it as the Linux driver for `project` workloads through its CLI/SDK, and keep the in-process gates: OpenShell governs a workload's syscalls, files and egress; ADR-033/094/057 govern agent actions, spawns and topics and chain them. Both layers are needed; neither replaces the other. It has nothing to offer the ESP32 or browser profiles and nothing on macOS today, so the driver must be pluggable.
- **Tailscale** [fetched]. One `tailscaled` per machine holding the machine and node keys; a non-root operator is granted with `tailscale set --operator`; privileged LocalAPI actions stay root-only (ts-2026-005). Exactly the machine-service plus per-user-operator split, including the lesson that the service must restrict which local callers may do what. ([operator permission](https://tailscale.com/docs/reference/troubleshooting/linux/linux-operator-permission), [identity](https://tailscale.com/docs/concepts/tailscale-identity))
- **Docker** [fetched]. Rootful: one daemon per host, socket access is root-equivalent; rootless: one per user at `$XDG_RUNTIME_DIR/docker.sock`. containerd sits under Docker as the shared runtime [recall]. ([rootless](https://docker-docs.uclv.cu/engine/security/rootless/))
- **sshd vs ssh-agent, systemd-resolved/Avahi, CUPS** [recall, unverified]. The host key lives in the system daemon (sshd), user secrets in a per-user agent discovered by env var; the resolver, the mDNS responder and the print spooler are single shared system services with per-user clients over a socket or D-Bus. This is the split proposed here: machine key in the service, user secrets in the user daemon.
- **systemd `--user`** [fetched]. One user manager per user; per-project services as template instances; linger for boot. ([systemd/user](https://wiki.archlinux.org/title/Systemd/user))
- **Nix** [fetched]. One daemon because the store is shared mutable state needing one trusted writer. ([manual](https://nix.dev/manual/nix/2.20/installation/multi-user))
- **Kubernetes / vcluster / kind** [fetched]. Namespace, virtual control plane, cluster per tenant; nesting is routine and vcluster nests inside vcluster. ([vcluster](https://www.vcluster.com/guides/understanding-kubernetes-multi-tenancy), [kind](https://spot.rackspace.com/blog/kind-kubernetes))
- **rust-analyzer** [snippet]. One server per workspace is expensive; `ra-multiplex` shares one. ([ra-multiplex](https://www.lib.rs/crates/ra-multiplex))
- **tmux** [fetched]. One server per (user, socket name). ([tmux(1)](https://man7.org/linux/man-pages/man1/tmux.1.html))
- **VS Code server** [snippet]. Versions coexist keyed by commit; relevant to inner/outer version skew.
- **git config**, **Erlang/OTP** [recall + `supervisor.rs:27-40`]. Layered precedence with tighten-only for governance; restart strategies are an in-node choice.

## 10. Recommendation, decisions, failure modes, migration

### 10.1 Recommendation

Adopt the four-role model (section 1) with these defaults: the machine
mesh service is **collapsed into the user daemon on single-user machines**
and **installed as an OS service on shared boxes and servers**; projects are
**tenants by default and sandboxed workloads on request**; a nested WeftOS
is an **isolated project kernel, mesh-off by default**; leaves and browsers
are **clients of a parent node**, never nodes with their own chain. Record
it as an ADR ("daemon topology, roles and profiles") that supersedes the
per-project runtime-dir fix `0955e5b0` and amends ADR-021/025/057/099/101/102
where they say "the daemon" or "the node".

Reasons, in weight order: (1) five accepted ADRs model one node per
machine; (2) the shared mutable state (node key, capacity facts, chain
files, token table, model port) needs one writer per box; (3) the owner's
real requirements (granular chains, governance, workloads, a clean
container-like boundary, a machine-wide mesh) are all satisfied by roles
and tenancy without N nodes per box; (4) ESP32 and browser profiles fit
only if the layers are roles a process can lack.

### 10.2 Decisions the owner must make

1. Mesh port number: `9421` (guide, owner) or `9470` (code). One number, everywhere.
2. Is the OS mesh service mandatory or profile-dependent (recommended: profile-dependent, collapsed by default on personal machines)?
3. `uid <-> user key` binding on shared boxes: TOFU or admin approval.
4. Runtime roots: `~/.weftos/run/` and `/var/lib/weftos`, `/var/run/weftos` (recommended) vs keeping `~/.clawft/`.
5. Project id: manifest ULID (recommended) vs hash of project pubkey.
6. Project chain location: in-tree `.weftos/chain/` (recommended, gitignored) vs `~/.weftos/projects/<id>/chain/`.
7. Key model: certification chain (recommended) vs HKDF derivation.
8. Governance overlays tighten-only (recommended) vs relaxation with a user-level permit.
9. `project` as an ADR-099 workload kind with pluggable sandbox drivers (recommended) and which driver is default per OS (Linux: OpenShell or containers; macOS: Seatbelt light / Apple `container` heavy).
10. Nested WeftOS default: isolated (recommended) vs tenant-registered.
11. Node id hash: settle SHA-256 vs SHAKE-256 vs BLAKE3 and change the losing documents.
12. Commands outside any project: "home" tenant vs error.

### 10.3 Failure modes of the recommended design

- **Mesh service down**: every user on the box loses mesh; local work continues; daemons reconnect. Mitigate with `KeepAlive`/`Restart=always` and a health check the daemons expose in `weaver health`.
- **Mesh service compromise**: attacker holds the machine key and can impersonate the box on the mesh, but not user chains, secrets or tokens; rotate via `governance.root.supersede` and user certs.
- **One user daemon, all tenants**: a panic takes the user's projects down until restart; per-tenant task boundaries, `SpawnBudget` and rate limits are prerequisites, and sandboxed mode exists for the risky project.
- **Tenant confusion**: mandatory `project_id` on tenant-scoped verbs and events; test that unscoped calls are refused.
- **Version skew across three tiers**: negotiated local protocols with a stated support window; `daemon_guard` per connection; the installer receipt lists all three.
- **Peer-credential gaps**: platforms without `SO_PEERCRED`/`getpeereid` semantics fall back to socket-directory ownership plus a registration secret in the user's runtime dir.
- **Leaf offline**: journal overflows on long outages; size the ring and mark gaps on the chain when replay is partial.
- **Manifest drift** and **user-chain migration** as in rev 1: probe roots, mark `missing`; old un-scoped events stay on the user chain.

### 10.4 Phased migration

**Phase 0, stop the bleeding.** One resolver for `node.key`, chain,
`workloads.json`, `cluster_peers.json` (`protocol.rs:47`, `chain_paths.rs:32`,
`boot.rs:756`); the `.weftos/` walk-up stops at `project.toml`/`weave.toml`,
never `$HOME`; `weft` prints the socket it tried and stops silent in-process
fallback; mesh node id from the Ed25519 key (`boot.rs:304-306`); one node-id
hash; `kernel.md:389-391` port fix; operator housekeeping of the stale socket
and the four node keys.

**Phase 1, one user daemon with the mesh role inside it.** Runtime root
`~/.weftos/run/`; refuse a second user daemon per machine; `weft` sends
`project_root`/`project_id`; workspace overlay resolved per request; the
existing `weave-coordinator` daemon becomes the first user daemon; launchd
user agent; `weaver update` restarts it.

**Phase 2, tenancy and the boundary contract v1.** Manifest, project keys and
certs, per-project `ChainManager` and anchors, governance overlays,
`project_id` in `GatePrincipal`, per-tenant budgets, ADR-102 token scope,
control API for projects (inproc enforcement), `project` workload kind in
ADR-099.

**Phase 3, machine mesh service as an OS service.** Extract the mesh role
into `weaver mesh serve`, `mesh-local/1` registration with peer
credentials, user certificates, machine journal and its anchoring, LaunchDaemon
and systemd units, installer tier. Personal machines stay collapsed.

**Phase 4, sandboxed projects and nested WeftOS.** Sandbox drivers
(OpenShell on Linux first, Seatbelt/Apple `container` on macOS), isolated
project kernels with the parent/child protocol, nested-WeftOS registration
levels, the CLI handshake.

**Leaf track, in parallel from Phase 1.** Provisioned Ed25519 keys on ESP32
(`JOURNALED-NODE-ESP32.md:57-81`), unified node id, signed publishes
required (retire the bring-up exception at `leaf-push-protocol.md:66-68`),
parent discovery replacing the compiled-in IP, offline journal and replay;
browser via ADR-102 tokens and `mesh_ws` with an ephemeral certified key.

### 10.5 Consequences for ADR-102 and install-update-review.md

- **ADR-102**: D3's single token authority is the *user daemon*; state it. Add `project_id` scope to `auth.token.issue` and the `auth.token.issued` event; the gateway forwards it on proxied MCP calls; the playground gets a project selector; `/health` (with token) lists tenants, chain heads and the machine service's status. The machine service exposes no HTTP beyond loopback health.
- **install-update-review.md**: the receipt and `weaver doctor install` cover three tiers (service, user daemon, isolated project kernels); restart the user daemon found via `~/.weftos/run/kernel.pid`, restart the service only when its package changed (admin step), never restart child kernels from the installer; record binary path and build sha per tier so skew is reportable.

## 11. What I could not verify

- Whether the RVF append path is `O_APPEND` or a rewrite; only the JSON checkpoint was confirmed as a whole-file rewrite (`chain.rs:1632`).
- Which of the four `node.key` files remote peers (the Pi 5 in ADR-099) have pinned.
- Whether any launchd/systemd unit starts a daemon on this Mac (none found under `~/Library/LaunchAgents`; PID 70730 has PPID 1).
- Windows peer-credential mechanics for the registration socket; described from general knowledge.
- OpenShell's supervisor-to-gateway protocol, macOS support and SDK shape: the blog and repo snippet do not say; the docs site was not fetched.
- Whether `mesh_ws.rs` is wired to any browser client today; the leaf-canvas crate says the WebSocket transport is a later phase (`weftos-leaf-canvas/src/lib.rs:50`).
- sshd/ssh-agent, systemd-resolved/Avahi, CUPS and containerd comparisons are from general knowledge, not fetched this session.
- The helpers' fact sheets were spot-checked where quoted above (ADR-021:68, ADR-071:22-28, ADR-098:34-36, release.md:403-410, SOPs:207,667, cluster.rs SHA-256, fleet-compat:37,91-93); items I did not re-read are not cited here.
