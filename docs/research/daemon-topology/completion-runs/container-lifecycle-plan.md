# Linux container lifecycle acceptance harness

Status: source written; not executed, compiled, or built. No engine or production
operations were performed for this task. Harness:
[`scripts/dev/p4-container-lifecycle.py`](../../../../scripts/dev/p4-container-lifecycle.py).

## Executable scope

The harness starts a **real `weaver` user daemon and a real supervised project
kernel inside one disposable Linux container**. The parent is a fixture only in
the sense that it has an empty, isolated home/config/chain: it is not a Python RPC
mock, the fake kernel from the Rust supervisor tests, or a version-command stub.
It exercises production RPC dispatch, certificate issuance, project bootstrap,
supervision, adoption, and revocation. The child uses the **logical** supervisor
driver inside that outer container. This choice is explicit and never a fallback
after a failed `linux-container` launch.

The host runner uses Docker/Podman CLI to create one labeled volume and one
container. Neither engine executable nor engine API/socket is passed to the
guest. A single Linux-engine-owned volume is mounted at `/case`. Both processes
use native Linux UDS beneath `/case/r`, including the parent's `child-ipc`
directory. No macOS UDS is exported into a VM, no host project/home is mounted,
and no port is published. The Python source is streamed over container stdin.

This is a real containerized lifecycle gate, **not acceptance of the imported
Linux container launcher**. Its receipt permanently says
`container_driver_accepted: false` and `d10_accepted: false`. It does not establish
a sandbox boundary between the parent and logical child: they share uid, mount,
PID and network namespaces and the child can access fixture parent state. It
does establish isolation of the entire fixture from host state and networking.
It does not test a distinct nested user daemon (D10).

## Imported-source findings and the remaining container-driver gate

Inspected in main:

- `crates/clawft-weave/src/project_supervisor/container.rs`:
  `OperatorConfig::load`, `create_args`, `parse_inspect`, and `EngineClient`.
- `project_supervisor/child.rs`: child environment/arguments, Linux container
  registration, launch and persisted engine identity handling.
- `tests/project_supervisor_support/{fixture,rpc_test}.rs`: fake-process fixture,
  real certificate/RPC wiring and lifecycle expectations.
- `tests/project_kernel_e2e_support/{world,scenario}.rs`: real kernel bootstrap,
  registration, identity-preserving restart, revoke and cleanup expectations.
- `handshake_rpc.rs`, `mesh_local_rpc.rs`, `clawft-rpc/src/mesh_local.rs`, and
  `clawft-types/src/project/cert.rs`: actual wire formats/signature domains.

The accepted mount-layout correction is `/usr/local/bin/weaver` for the
executable and `/weftos/{project,run,trust,parent}` for directories. Previously
`--entrypoint /weftos` conflicted with mounts below that file. The source delta
updates the launcher, inspect entrypoint contract, regression tests, image/CLI
smoke, operator guide and this logical harness together. Existing pinned images
must be rebuilt/re-pinned with the corrected layout; no old-layout fallback.

The actual driver acceptance gate is specified below. The existing logical-pair
harness remains supplemental and cannot turn `container_driver_accepted` true.

## Acceptance assertions

Every step fails closed; missing verbs, certificates, proofs or prerequisites are
failures, not skips. Each RPC and lifecycle poll is bounded; the outer container
execution has a 900-second timeout.

1. Boot `/usr/local/bin/weaver kernel start --foreground --profile user` with explicit isolated
   JSON config. Require a user-role handshake, the expected PID/runtime, and mesh
   mode `off`. The worker verifies the executable SHA-256 before starting it.
2. Register one fixture project through real `project.register`; atomically set
   its newly generated manifest's serve table to logical child supervision.
   Start it through real `project.start`.
3. Independently verify the Ed25519 project certificate under the parent's user
   identity. Verify a fresh `kernel.handshake` challenge under the certified
   project key; a changed challenge must fail verification. Require consumed
   `spawn.json`, the user-chain `project.register` receipt and a live registry
   session. An invalid registration binding signature must return `pop_failed`.
4. SIGKILL only the verified child PID/executable/start-time identity. Require
   automatic supervisor restart with a different PID, the same certificate/key,
   and a new valid handshake proof.
5. Shut down only the fixture parent using its RPC, require the child to survive,
   then boot the parent on the same private state. Require unchanged parent key,
   a `project.kernel.adopted` receipt naming the survivor PID,
   `ensure_running.started=false`, the same child PID, and a live re-registration.
   The Python PID-1 worker reaps only tracked orphan project children, matching
   their process start times; it does not steal the parent subprocess exit status.
6. Require distinct parent/child signing seeds and child chain storage, mesh off,
   no engine/outer-mesh sockets, only the loopback network interface and an EROFS
   failure writing outside the mounted writable areas. The host independently
   inspects read-only root, capabilities, network mode and the mount allowlist.
7. Invoke real `project.revoke`; require child exit, revoked marker and socket
   cleanup. `project.start`, `project.restart` and `project.ensure_running` must
   all return `project_revoked`.
8. Shut down the parent and require owner/child socket removal. The host removes
   only the immutable container ID after a fresh ownership-label check and only
   the labeled volume created by this invocation. Require their disappearance.
   Cleanup failure makes the run fail even if the lifecycle receipt passed.

The live-session probe is deliberately nonmutating: it supplies a correctly
signed binding but a fresh, invented spawn nonce. A live registry must refuse
with `second_session`; an expired adoption tombstone cannot authorize that nonce.
It never supplies the original spawn credential and never opens a replacement
session. This avoids mistaking an initially empty `unregistered_secs` status for
proof of successful re-registration. Fixture-only project private key access is
used to sign this probe; keys and spawn/token files are never exported.

The existing child-endpoint permission/binder tests remain owned by the other
lane. This harness does not duplicate that permission matrix.

## Exact artifact and build prerequisites

No build, pull, install or engine invocation is performed by plan-only mode.

- Host: Python **3.11+** (stdlib `tomllib`) and Docker or Podman CLI configured for
  an explicitly chosen disposable Linux test engine. Do not run against a
  production engine. Only a future invocation with `--run` contacts the engine.
- Guest image: **already present** at a `repository@sha256:<64-lowercase-hex>`
  reference. Every create uses `--pull=never`. No mutable-tag fallback.
- `/usr/local/bin/weaver` must be the regular Linux **`clawft-weave` / `weaver` executable**, not
  `clawft-cli`'s `weft`, `weftosCLI`, a shell wrapper, or a symlink to a differently
  named executable. Supply its independently recorded file SHA-256. A digest
  alone does not prove that the image contains the correct binary.
- The image must contain Python **3.11+**, importable `cryptography` with Ed25519
  support, and the runtime libraries needed by that particular Linux artifact.
  Dependencies must be baked into the pinned image; the harness installs nothing.
- No image-declared `VOLUME`, fixture state, host secrets or external-service
  configuration. The runner disables healthchecks, overrides entrypoint/workdir,
  and gives daemon processes a cleared environment. Root inside the fixture
  container is intentional for ownership of a fresh volume; all capabilities
  are dropped and no-new-privileges is set. Rootless Podman is compatible in
  principle; it is not claimed tested.
- The engine must support read-only root, network none, capability drop,
  no-new-privileges, tmpfs, memory/CPU/PID limits and named volumes. Unsupported
  flags fail the run; there is no weaker fallback. Allow 4 GiB RAM and 256 PIDs.
- Build the **main checkout revision being evaluated**, including imported P4
  handshake-proof and supervisor code. Record its commit and dirty-source
  provenance alongside the image digest and binary hash. This harness cannot
  infer that provenance from a digest supplied by an operator.

For an independently authorized future native Linux build, the precise Cargo
selection is:

```sh
cargo +1.95 build --locked --release -p clawft-weave --bin weaver
```

`rust-toolchain.toml` selects 1.95. Keep the crate's default features:
`cluster,ecc,exochain,mesh,placement`; do not substitute a no-default-features CLI
build or the fake supervisor test executable. The build environment needs the
full workspace and locked dependency sources plus a working native Linux Rust
compiler/linker toolchain. Native dependency/library availability and a successful
build are prerequisites, **not verified results of this task**. Use the target
architecture of the chosen Linux engine; do not copy a macOS Mach-O artifact into
the image. Package `target/release/weaver` as `/usr/local/bin/weaver` and record its SHA-256.

The repository's existing root `Dockerfile` is unsuitable: it builds
`clawft-cli --bin weft`, installs `/usr/local/bin/weft`, declares a volume and uses
a gateway entrypoint. It does not provide the artifact required here. No
Dockerfile or Rust changes were made by this task. An operator must supply the
separately built/pinned fixture image before executing this source.

## Invocation and evidence

Plan only; replace both placeholders with actual recorded values:

```sh
python3 scripts/dev/p4-container-lifecycle.py \
  --image 'REGISTRY/FIXTURE@sha256:IMAGE_DIGEST' \
  --weaver-sha256 'WEAVER_FILE_SHA256'
```

An independently authorized future run adds `--run`, `--engine docker` (or
`podman`) and a **new** report directory. Long-running execution must be detached
per the repository instructions; for example, after filling real artifact values:

```sh
nohup python3 scripts/dev/p4-container-lifecycle.py \
  --image 'REGISTRY/FIXTURE@sha256:IMAGE_DIGEST' \
  --weaver-sha256 'WEAVER_FILE_SHA256' \
  --engine docker --run \
  --report-dir docs/research/daemon-topology/completion-runs/container-lifecycle-RUN \
  > docs/research/daemon-topology/completion-runs/container-lifecycle-RUN.log 2>&1 &
```

Capture the returned PID and poll the log/process. Do not treat these placeholder
commands as already executed. Plan-only mode creates no output directories.

Evidence consists of `container-inspect.json`, `lifecycle.log`, `receipt.json`
and `cleanup.json` in the new report directory. The receipt identifies its limited
scope, immutable container ID, image digest, binary hash and child PIDs. Accept
only process exit zero **and** lifecycle/cleanup passing. A lifecycle receipt
alone is insufficient. Failure emits bounded fixture daemon log tails; cleanup
removes private key/token state with the volume. Abrupt termination of the host
runner can leave labeled resources; investigate those recorded IDs/labels,
never use a global engine prune.

Source validation for this task is limited to static parsing/review and whitespace
checks. Real engine behavior, binary feature completeness, shared-service startup
and the end-to-end assertions remain unverified until an authorized run uses the
correct pinned image. There is no claim that P4 or D10 is complete.


## Actual Linux container driver acceptance: fixed gate

This is the acceptance target for the imported driver, not a new definition of
success for the logical harness. Run after this patch is applied and both parent
binary and child image are built from the evaluated source. This section is a
runbook specification, not an implemented driver runner or execution receipt.
Failure remains failure; do not substitute `--version`, CLI help, a mocked
engine, direct engine-started children, or the logical driver.

### Dedicated Docker-in-Docker topology

Use one disposable Linux VM/test host, containing one dedicated, digest-pinned
Docker-in-Docker fixture container. Provision the actual parent `weaver`, Docker
CLI, lifecycle controller and `dockerd` in that SAME fixture container; execute
the parent/controller there using exec, not as sibling containers connected to a
remote socket. They share its Linux PID and mount namespaces and the same
`/case` path. `dockerd` creates the project child as a nested container with its
own PID/mount/network namespaces. Its inspected State.Pid is therefore meaningful
in the parent's /proc, and engine bind-source paths are the parent's actual paths.
The child PID from its handshake is guest-local; never equate it to State.Pid.

Use the fixture's OWN `/var/run/docker.sock` created by its own dockerd. Do not
mount any host Docker/Podman socket, host /proc, device tree, home or project
checkout; do not use host PID/network namespaces. Put `/case`, dockerd's data
root and exec root on disposable fixture-owned storage. Use unique ownership
labels for the outer fixture and storage, retain their immutable IDs, and remove
only resources created by this run. No API TCP port is published. Clear inherited
DOCKER_HOST, DOCKER_CONTEXT and CONTAINER_HOST in the parent; resolve `docker`
from a controlled PATH against this local dedicated socket.

Rootful DinD commonly requires a privileged OUTER fixture for namespaces,
mounts and cgroups. Authorize that only in a disposable Linux VM, not on a
production/shared host. That privilege belongs to the test engine fixture, not
the project child: the driver must still create the child with cap-drop ALL,
read-only root, no-new-privileges, pids-limit 64 and network none. Do not weaken
those child flags to make DinD pass. Provision artifacts first; disable fixture
networking for the acceptance run. Use local Unix RPC throughout. If the chosen
VM cannot run DinD's storage/cgroup setup, use the VM-host alternative below and
report the topology actually tested; do not silently change the driver.

Before starting a user daemon, establish these preconditions in evidence:

- Pinned DinD fixture image, dedicated engine ID/version, parent executable hash
  and source revision/dirty-delta fingerprint; sufficient PID/memory capacity.
- Correct child image already present in the INNER engine at the exact digest
  in `project-container.json`, with executable `/usr/local/bin/weaver` and no
  conflicting image VOLUMEs. Both parent and child hashes identify `weaver`.
  Preload during isolated provisioning. If archive import loses RepoDigests,
  restore/verify a resolvable digest using a disposable provisioning registry;
  do not substitute a tag or assume `docker load` preserved the digest. The
  acceptance phase must need no registry and still use `--pull=never`.
- A filesystem sentinel at `/case` observed through a disposable inner-engine
  bind mount proves shared source-path semantics. Compare parent and dockerd
  PID/mount namespace IDs; inspect the probe container's State.Pid and show it
  in the parent's /proc. Verify native UDS exchange through the mounted parent
  DIRECTORY, not a mount of a stale socket inode. Remove the probe by its ID.
- Isolated HOME `/case/h`, runtime `/case/r`, project `/case/p`; parent mesh and
  TCP IPC disabled. Parent owns private `.weftos/project-container.json` with
  `{"engine":"docker","image":"repository@sha256:<recorded digest>"}`.
  Parent and child UID/GID must match the inspected driver contract. A fixture
  root uid is acceptable; no real user home or production chain is present.

Start the real parent inside that fixture namespace:
`/usr/local/bin/weaver kernel start --foreground --profile user --config /case/config.json`.
Register `/case/p` over its owner UDS. Set that fixture manifest's `[serve]` to
`via="child-kernel"`, `sandbox="linux-container"`, with a bounded restart budget.
Start through `project.start`, NEVER a direct `docker run` of the test child.
The parent must invoke the real `EngineClient` and `create_args` paths.

### Required observations and failure criteria

1. **Launch/identity:** capture project.start/status, persisted state.json and
   inner-engine inspect. Require a 64-hex container ID A, pinned image, exact
   `/usr/local/bin/weaver` entrypoint, project/supervisor labels, UID/GID, all four
   expected bind sources/targets/permissions and all isolation flags. Record
   guest PID and inspected host PID separately. Require a signed certificate,
   fresh project-key handshake proof and a live container-bound registration
   using the v2 binding (container ID plus parent-selected host socket). Missing
   proof, stale ID or a refused readiness check is failure, not a timeout waiver.
2. **Boundary:** the child must reach only the mounted
   `/weftos/parent/child.sock` parent endpoint. Inspect `/weftos/trust` and parent
   directory read-only status; attempt writes to user.pub/parent-policy/revoked
   and require failure. Verify the owner socket, parent private keys and engine
   socket are not mounted or reachable, project/runtime writes work, root writes
   fail and no external interface/outer mesh exists. Use a prepared probe helper
   in the pinned image if needed; never inject host socket mounts for probing.
3. **Automatic crash restart:** record A's fresh inspected identity, kill only A
   through the dedicated engine, and let the REAL supervisor restart it. Require
   old A removed by verified restart, new immutable ID B != A, same project key/
   certificate, fresh signed registration/proof and bounded restart count. Do
   not manually create B or accept native-PID-only restart evidence.
4. **Parent restart/adoption:** stop only the user daemon while dockerd and B
   stay alive. Restart on the SAME parent state, verify unchanged user key and
   an adoption receipt, and require the SAME container ID B. Require no duplicate
   project-labeled container, `ensure_running.started=false`, fresh signed
   re-registration and working parent UDS after rebind. Signed container proof,
   not guest PID equality or a cached socket, must establish readiness.
5. **Revoke:** invoke the real `project.revoke` on the parent; require B stopped,
   terminal revoked marker, registration gone and all three start/restart/ensure
   paths refused. Accept the current driver's retained EXITED engine record for
   re-inspection; do not invent a requirement that revoke itself force-removes
   that record. A live B or successful re-registration is a failure.
6. **Cleanup:** shut down the fixture parent; inspect B and its full ownership
   contract, remove its exited record by immutable ID, and prove no live or
   exited project-labeled children remain. Export sanitized receipts/logs first,
   then remove the outer fixture and its labeled data volumes or discard the
   disposable VM. Check every removal; no global prune. A pass requires both
   lifecycle success and cleanup success. Abrupt interruption leaves a failed
   run with resource IDs for explicit recovery, never a success receipt.

The receipt must name `scope=linux-container-driver`, parent and child source/
artifact hashes, image digest, engine identity, IDs A/B, adoption identity,
certificate/proof checks, isolation checks, revoke results and cleanup results.
The pass bit may become true only after ALL six observations succeed. Keep the
logical harness's existing false driver-pass bit unchanged. This driver test
establishes D9 container lifecycle, not the separate D10 nested-user-daemon gate.

### Accessible local Linux VM alternative

A newly provisioned dedicated Linux VM can run its own dockerd and real parent
as ordinary VM-host processes, using the same `/case` paths and native UDS.
Drive it over SSH/console; no macOS UDS or host engine socket is forwarded. The
same digest, child flags, six observations and receipt apply unchanged. First
verify that such a VM is available and disposable; none was discovered or
accessed in this source-only task. No existing machine is assumed safe to use.

### Deferred validation commands

After source application, the path regression filter is
`cargo test -p clawft-weave project_supervisor::container::tests --lib` (default
features). The targeted regression asserts every actual Docker and Podman mount
target neither descends from nor hides the actual entrypoint and retains all
four `/weftos` targets. Replacing that entrypoint with the old `/weftos` makes the
assertion fail; invalid runtime overlaps are rejected before engine calls.
Inspection tests also reject old/wrong/augmented entrypoints. Run the repo's
required gates only in the authorized build lane; none were executed to prepare
this patch. Detach future builds and engine acceptance runs and capture PID/logs.
