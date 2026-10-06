# ADR-103 D9 Wasmtime project kernel: source handoff

Base: `e6662b818be7511c283cc68c8e6df46681f342a2`.
Worktree: `/Users/mathewbeane/.codex/worktrees/wasm-project/weftos`.
Status: bounded guest/runner plus launcher/schema/adoption integration source and tests written; **not compiled or runtime-validated**.
The owner deferred compiler jobs to the integration lead's existing shared cache.
No commits, pushes, board operations, native daemon launches or project-state writes were performed.

## Delivered boundary

`clawft-wasm --features project-kernel --bin weftos-project-guest` is a persistent
project supervisor guest. `clawft-wasm-host --features project-kernel --bin
weftos-wasm-project-runner` is its native process. This is not the native `weftos`
binary, the existing stub WASI library, or a per-call `WasmToolRunner`/plugin instance.

The ABI is a core module importing `weftos_project_v1.exchange`, with four bounded
operations: bootstrap, fixed-parent RPC, inbound request, reply. Each JSON frame is
at most 1 MiB. The runner checks both memory ranges before side effects. One
`Store` and `_start` call survive all requests. The guest uses `wasm32-wasip1`;
`build-wasi.sh` and the existing `wasm32-wasip2` library/component build remain
unchanged. Both Wasmtime packages are **48.0.5 in Cargo.lock**. The runner uses
that release's `FsPerms::ReadWrite`, `WasiP1Ctx`, and synchronous P1 linker API.
Only dependency edges for the two local packages changed in the lockfile.

The native runner owns PID, 0600 UDS, 0700 external runtime, kernel and chain locks,
fixed parent address, lifetime fuel (never replenished), memory/table limits,
SIGTERM/SIGINT and deadline cancellation via epochs. It compiles only the exact
bytes whose SHA-256 matches the trusted launch record. It does not deserialize a
native-code cache. WASI exposes only the project's `.weftos` storage, random and
clocks; no inherited environment, HOME, stdio, network, process spawning, or runtime
preopen. The read/write preopen is trusted kernel state, not a capability that may
be passed to arbitrary workload guests. Cancellation of guest CPU is epoch-based;
all bridge socket connect/read/write operations are nonblocking and check cancellation
against absolute deadlines (250 ms inbound/reply, 2 seconds total parent RPC).
Byte trickles cannot renew those deadlines. The OS runner writes its PID into the
held kernel lock so adoption can verify the lock holder.

The guest owns the Ed25519 project key, certificate verification, pinned user
identity, signed parent-policy verification, tighten-only overlay merge,
rollback version pin and signed RVF chain. It directly reuses the repository's
`chain.rs`, non-native `chain_subscribe.rs`, `parent_policy.rs`,
`governance_overlay.rs`, and `mesh_local.rs` sources through `#[path]`. Its
`GovernanceEngine` is the existing no-default-features kernel implementation.
This keeps source ownership separate from the native launcher and avoids copying
or replacing governance rules. The two pure-merge constant dependencies (0.7 risk
threshold and `parent-policy.version`) are declared in the guest. This path-based
composition must be included in publication packaging before publishing the crate;
it is currently a workspace build surface.

Implemented lifecycle:

- First boot generates the key with WASI entropy; subsequent boots reuse it.
- Challenge/registration use the existing PoP, bind and acknowledgement signing
  functions. Signed registration binds host socket and runner PID; the root digest
  uses canonical host path bytes. The supervisor-provided spawn nonce is retained.
- Certificate issuer, project, public key, expiry and parent acknowledgement
  freshness are checked. Uncertified/offline boot refuses, rather than degrading.
- The signed chain uses the same project key, records genesis and effective policy,
  verifies the checkpoint signature and binds restored head, sequence, chain ID
  and count to that authenticated checkpoint on reload, and saves after every mutation. A failed
  durable write terminates the guest instead of serving uncommitted governance.
- Status/handshake are public; every other request requires a parent-signed
  forward-v2 header with request/params/target binding, 5-second freshness and
  bounded monotonic replay tracking. A supplied forward header is always checked,
  including on public methods.
- Governed verbs: `chain.status`, `chain.append`, `governance.parent.update
  {policy}`, `kernel.stop`, and the supervisor-compatible `kernel.shutdown`. Reserved chain sources/kinds remain denied.
  Denials and approvals are evaluated by the actual governance engine; the slice
  only executes Permit, so approval/warning results cannot bypass a gate.
- Heartbeat is signed and reports activity. Unknown/expired sessions re-register
  inside the same guest instance. Parent transport loss leaves a certified running
  guest alive. After session loss, a previously certified guest retries
  `spawn_not_expected` while the parent completes signed adoption and files its
  expired session. Fresh boot still requires the expiring spawn nonce; this
  retry grants no parent authorization. Other registration refusals fail closed.
- Anchors use existing `ProjectAnchorStmt::sign/hash/verify`, write a pending file
  before submitting, retry the exact statement, and persist accepted continuity in
  the signed chain before removing pending state. A parent acknowledgement is
  transport-authenticated, matching ADR-103 A7; it is not falsely claimed to carry
  a parent signature. Guest restart replays a pending statement.
- Signed `kernel.stop`/`kernel.shutdown` reply, attempt one bounded final anchor,
  unregister with the certified key and exit cleanly. Failed delivery retains
  the pending statement; successful final delivery is not guaranteed.
  Forced cancellation relies on per-mutation persistence; it does not pretend to
  unregister. The external `revoked` marker is checked at every bridge call.

## Launcher/schema/adoption integration delivered in this worktree

Build the user daemon with `clawft-weave/wasmtime-project`. The manifest must say:

```toml
[serve]
via = "child-kernel"
sandbox = "wasmtime"
adapter = "wasmtime-project-v1"
```

The operator supplies `<home>/.weftos/project-wasmtime.json` (0600), outside every
project root. Required JSON fields are `adapter`, `runner`, `runner_sha256`,
`artifact`, `artifact_sha256`, `lifetime_fuel`, `memory_bytes`, `lifetime_secs`.
Adapter is exactly `wasmtime-project-v1`; paths are absolute, canonical, owned by
the operator and outside the project. Hashes are lowercase SHA256 of the native
runner and core WASIp1 module. No manifest may choose either executable path.
The private operator directory, runtime root, per-project runtime and guest
`.weftos` directory must have mode 0700. Config/state files are 0600. The source
work did not create or modify any live operator configuration.

`project_supervisor/wasmtime.rs` validates operator configuration, path separation,
artifact hashes and resource bounds. It stages the exact hashed bytes into the
private runtime directory (`wasmtime-runner-<hash>` and `wasmtime-guest-<hash>.wasm`)
so an operator source-path replacement cannot race hash verification and exec.
The launcher consumes `SpawnFile::read_and_consume`, preserving nonce, TTL,
canonical root, project, parent socket and pinned user key, then writes a private
launch JSON and receipt. It invokes the native runner with that one absolute JSON
argument, empty environment, private runtime cwd and the existing log/waiter/
process-group machinery. PID expectation and `state.json.exe` name the runner.
The runner alone owns the socket, PID and lifetime locks; only stale dead-runner
socket/PID files are removed by the launcher, after lock/PID/receipt checks.

The project workload carries its immutable adapter into `ChildSpec`. The existing
process lifecycle adapter has a dedicated Wasmtime constructor with distinct
`wasmtime-project-v1` identity, `runtime.project.wasmtime` capability and
`wasm32-wasip1` architecture. Logical admission rejects a Wasmtime payload.
Supervisor start/restart/stop select the appropriate `WorkloadHost`; gate and
chain transitions remain intact. A manifest change cannot reinterpret a loaded
handle. Missing feature/configuration, wrong adapter, malformed receipts and pin
mismatches refuse; none selects logical as a fallback.

Readiness and every adoption entry point (start-time scan, boot scan and retry)
require a fresh random challenge, the current verified project certificate, full
runner path/hash, live PID and held lock, plus a guest signature binding the
canonical project root, host socket, PID, sandbox, adapter and guest artifact hash.
The verifier is shared with the runner crate by source inclusion, avoiding a
Wasmtime link dependency in the user daemon. Only after successful adoption does
`file_adopted_session` authorize nonce-less registration. Adopted-process signal
checks retain the runner's full executable identity. Wasmtime build metadata comes
from the verified guest handshake, not the native weaver build stamp.

The Python protocol fixture now refuses nonce-less registration until it has
verified adoption and authorized that PID. It tests the refusal/retry window
before authorization; it no longer grants a session merely because a cert exists.

### Merge map for main's imported container/Seatbelt/nested lanes

Main was read only. These edits target the isolated `e6662b818` baseline; do not
replace main's whole files with this worktree's versions:

- `clawft-types/src/project/schema.rs` and `project/mod.rs`: retain main's existing
  `ProjectSandbox::{Logical,Seatbelt,LinuxContainer}`, add `Wasmtime` and the
  optional explicit `serve.adapter` field/export. This baseline deliberately
  refuses Seatbelt/LinuxContainer because their implementations belong to main.
- `workload_runtime/logical.rs`: preserve main's `ChildIdentity` and container
  identity handling. Merge the immutable adapter field and dedicated runtime
  identity/capability constructor. The runner is an OS process, so main's native
  PID identity transport is usable; its sandbox identity still comes from the
  Wasmtime runtime, operator receipt and signed guest proof.
- `project_supervisor/child.rs`: dispatch Wasmtime before main's native/Seatbelt
  helper and container branches. Preserve main's container launcher, dedicated
  child socket, nested env and process identity representation. Merge the selected
  runner hash into spawn expectations and the existing waiter/state machinery.
- `project_supervisor/mod.rs` and `boot.rs`: merge Wasmtime host selection and
  signed scan routing alongside main's container-specific scan/proof, not over it.
  Keep main's `user_daemon::child_socket_path` parent endpoint in `post_boot`.
- `io.rs`: add `wasm_handshake` alongside main's `prove`. Its full-reply signature
  domain is different; do not substitute the native project proof.
- `sandbox.rs` in main: add a Wasmtime match arm that refuses entry through the
  native helper; it must already have dispatched to the runner. Preserve Seatbelt.
- Preserve main's `StateFile.container` and all container/private-mount rules.
  Wasmtime uses its own strict private receipt, so no container-state field is
  removed or repurposed.
- `ProjectPayload.adapter` needs `"logical"` in existing native/container test
  constructors; `prepare_project` retains all certificate/revocation checks.

## Exact remaining parity and portability work

This is not full `Kernel::boot` parity and must not be advertised as such.

1. Native project boot is not portable as currently factored:
   `clawft-kernel/src/boot.rs` rejects a project profile without `exochain`, and its
   exochain branch calls `project_identity::project_chain_key`; `lib.rs` exposes
   that module only for `exochain + native`. Enabling `native` brings native
   transports/services. `project_boot.rs`/`project_boot_run.rs` use NativePlatform,
   process-global link state, Tokio and OS process identity. This slice composes
   the real portable chain/governance pieces instead of bypassing that refusal.
2. `chain_anchor_parent.rs::submit_bounded` spawns a native thread and its pending
   writer calls the native-gated identity module. The guest implements bounded
   synchronous transport instead. It does not implement anchor-reset/history
   reconciliation, key rotation, or adoption of a divergent parent anchor ledger;
   continuity conflicts retain pending state rather than silently resetting it.
3. No agents, workload catalog/execution, shared LLM/embedding/token ParentLink,
   mesh delivery, subscription streams, user-key rotation or general RPC surface
   is installed. Unsupported verbs refuse. The guest advertises only `anchor`,
   not `subscribe`. Max-process/spawn limits are merged but there is no spawn path.
4. Compiler/dependency portability is **unverified**. In particular, the RVF source
   modules and the no-default kernel dependency must pass the actual WASIp1 build;
   the older library-only WASIp2 build is not evidence of that. Cached source APIs
   were inspected; no Cargo compiler or test job was started.
5. Launcher/schema/adoption source is implemented here, but **merging main's other
   drivers, compilation, actual parent/runner integration execution and packaging/
   install remain unvalidated/not performed**. Operator artifacts must be installed
   and pinned explicitly. Pin changes deliberately refuse adoption of old runners;
   automatic upgrade/pin migration is not provided. This slice is a depth-1 project
   below the user daemon; it does not implement nested Wasmtime supervision.
   `.weftos` mode 0700 is a prerequisite; the driver does not silently chmod a
   project's existing state. A revoked/unverifiable identity is never adopted or
   signalled solely from its unauthenticated response; the external revocation
   marker enforces cancellation of running guests.
6. Storage uses full-chain signed snapshots, not an incremental journal. Guest
   memory/fuel are bounded, but there is no disk quota/compaction policy here.
   Atomic rename and file fsync cover process failure; power-loss durability of
   directory metadata has not been established. An arbitrary replacement guest
   must never be approved merely because it satisfies the ABI: the approved
   artifact has signing authority and read/write access to its project state.

## Validation performed and deferred commands

Performed: rustfmt parsed/formatted new Rust files; `git diff --check`; Python AST
parse; both changed Cargo manifests and Cargo.lock parsed with `tomllib`; shell
syntax check. Lockfile dependency versions remain unchanged. These are source
checks, not Rust type checking or evidence that lifecycle tests pass.

Written tests (none executed):

- Guest `storage::tests::retained_signed_footer_cannot_authenticate_rewritten_events`
  uses two internally valid histories and retains the original signed checkpoint:
  old standalone signature/integrity checks pass, the new commitment check refuses.
- Runner `bounded_io_tests` exercises slow byte trickles, cancellation during a
  host read and a blocked writer under absolute deadlines.
- Kernel `wasmtime_lifecycle_tests` covers distinct admission/handle/evidence
  identities through load/start/stop and refuses logical admission.
- Weave `project_supervisor::wasmtime::tests` covers explicit selection, malformed
  receipt refusal, hash substitution and symlink refusal.

- `project_kernel::tests::adoption_binds_every_launch_fact_and_fresh_challenge`
  verifies the real signature and rejects each substituted launch field, a missing
  signature and a stale challenge.
- `scripts/test-wasm-project.py` requires the actual compiled guest and runner.
  An independently signing parent checks PoP/bind/heartbeat/unregister/anchor
  signatures, root and host socket. Cases cover adapter/hash/fuel refusal, forged
  parent policy and acknowledgement, persistent chain mutation, replay/tampering/
  cross-project refusal, signed adoption, duplicate-runner locking, session loss,
  restart with the same key, pending-anchor replay, signed policy rollback,
  overlay denial, graceful unregister, lifetime cancellation and revocation.
  The fixture never uses an echo guest. Python `cryptography` is required; do not
  install it into a live environment as part of an implicit test step.

The integration lead should set `CARGO_TARGET_DIR` to the **existing** shared cache
and run these sequentially as detached jobs (not all workspace builds). The
WASIp1 target must already be installed; the build script never installs targets.
The script is invoked via `bash` and does not require a mode change.

```bash
# With CARGO_TARGET_DIR already pointing to the lead's existing cache:
nohup bash scripts/build-wasm-project.sh --guest > docs/research/daemon-topology/wasm-project-guest-build.log 2>&1 &
# Capture $! and poll that log. Only after completion:
nohup bash scripts/build-wasm-project.sh --runner > docs/research/daemon-topology/wasm-project-runner-build.log 2>&1 &
# Then:
nohup cargo test --locked --offline -p clawft-wasm-host --features project-kernel --lib project_kernel::tests > docs/research/daemon-topology/wasm-project-adoption-tests.log 2>&1 &
# Actual lifecycle, using a NEW isolated state directory (retained as receipts):
nohup python3 scripts/test-wasm-project.py \
  --runner "$CARGO_TARGET_DIR/debug/weftos-wasm-project-runner" \
  --guest "$CARGO_TARGET_DIR/wasm32-wasip1/debug/weftos-project-guest.wasm" \
  --state-root "$PWD/.wasm-project-test" \
  > docs/research/daemon-topology/wasm-project-lifecycle.log 2>&1 &
```

The existing workspace lock may need Cargo's normal metadata validation after
integration merges the new local dependency edges. Do not update Wasmtime to a
new version to get this slice compiling. No gate above has been reported as run.

Keep the fixture state path short: macOS limits Unix socket paths to 103 bytes.
The example uses a short isolated worktree-local directory; logs remain in docs.

## Additional deferred integration commands

After the leaf lane releases resources, from the integrated checkout with an
existing `CARGO_TARGET_DIR`, run **one detached job at a time**, recording `$!` and
polling its log. These are proposed validation commands, not commands run here:

```bash
nohup cargo check --locked --offline -p clawft-weave --features wasmtime-project --lib > docs/research/daemon-topology/wasm-launcher-check.log 2>&1 &
nohup cargo test --locked --offline -p clawft-weave --features wasmtime-project --lib project_supervisor::wasmtime > docs/research/daemon-topology/wasm-launcher-tests.log 2>&1 &
nohup cargo test --locked --offline -p clawft-kernel --features workload-runtime --lib wasmtime_lifecycle_tests > docs/research/daemon-topology/wasm-runtime-tests.log 2>&1 &
nohup cargo test --locked --offline -p clawft-wasm --no-default-features --features project-kernel --bin weftos-project-guest storage::tests > docs/research/daemon-topology/wasm-storage-tests.log 2>&1 &
nohup cargo test --locked --offline -p clawft-wasm-host --features project-kernel --bin weftos-wasm-project-runner bounded_io_tests > docs/research/daemon-topology/wasm-io-tests.log 2>&1 &
nohup cargo test --locked --offline -p clawft-weave --features wasmtime-project --test project_supervisor > docs/research/daemon-topology/wasm-supervisor-regression.log 2>&1 &
```

Then build the guest/runner and run the actual lifecycle fixture using the earlier
commands. An isolated real-user-daemon test with these operator pins, dedicated
child socket and daemon restart is still required after integration; the Python
parent protocol oracle does not replace that test. Do not claim any of these
source tests pass until their deferred compiler/runtime jobs finish.

## Source freeze and import manifest

`adr103-wasm-project-freeze.json` in this directory is the file-by-file import
manifest: worktree/base, SHA256 for every listed source/test/document, baseline
SHA256 for merge files, and add-versus-merge instructions. The manifest excludes
itself from its hashes. Verify hashes immediately before importing; a mismatch
means a later edit needs a refreshed freeze. Import new files intact; merge
tracked files using the conflict map above. Do not overwrite main's imported
container/Seatbelt/nested code or its lockfile wholesale.

The four reviewer findings have source fixes and regression test source, not
executed acceptance evidence. Shutdown now attempts a final bounded anchor before
unregister; it still cannot guarantee parent delivery during failure. Remaining
limits are full native Kernel boot/services, workload/agent execution, shared
ParentLink services, streams, nested Wasmtime supervision, key rotation, anchor
ledger divergence/reset, disk quotas/compaction, crash/power-loss validation,
packaging and upgrade/pin migration. Real-parent lifecycle, compiler portability,
platform socket behavior and regression execution remain deferred checks.
The native suite totals reported by the lead are separate evidence; they do not
establish WASIp1 compilation or execution of this source slice.
