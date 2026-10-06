# ADR-103 D9 Wasmtime project kernel

Status: integrated into the 0.8.3 line behind the off-by-default
`clawft-weave/wasmtime-project` feature. Compiled, unit-tested and exercised
against a signing-parent protocol fixture; **not** run against a real user
daemon (see "Verification" and "Not done").

Originally written against a 2026-10-04 baseline and ported onto the current
`project_supervisor` (nested instances, Linux-container and Seatbelt drivers,
ADR-108 reporter). Wasmtime and `wasmtime-wasi` are 48.0.5, as pinned in
`Cargo.lock`.

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

## Launcher, schema and adoption integration

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
`.weftos` directory must have mode 0700. Config/state files are 0600. Nothing
here creates or modifies a live operator configuration.

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

The Python protocol fixture refuses nonce-less registration until it has
verified adoption and authorized that PID. It tests the refusal/retry window
before authorization; it does not grant a session merely because a cert exists.

### Interaction with the container, Seatbelt and nested lanes

`serve.sandbox` is `logical`, `seatbelt`, `linux-container` or `wasmtime`; only
`wasmtime` selects the `wasmtime-project-v1` adapter. The other three keep the
`logical` adapter (the native kernel, started by their own drivers) and must not
carry `serve.adapter = "wasmtime-project-v1"`. `Launcher::spawn` selects the
driver from the manifest, refuses a `ChildSpec` whose adapter differs from the
manifest, runs `wasmtime::preflight` before issuing a project token, and shares
the owned-process tail (waiter, process group, `state.json`) with the native
launch through `spawn_owned`. `sandbox.rs` refuses `Wasmtime` if it is ever
reached, so the native helper can never exec a Wasmtime project. Boot scan,
start-time scan and handshake retry all go through `Supervisor::scan_project`,
which routes a run directory with a Wasmtime receipt to the signed-adoption
verifier and everything else to the existing native/container scan.

## Verification

Run on 2026-10-06, macOS aarch64, Rust 1.95, Wasmtime 48.0.5, using the
`scripts/build.sh` lanes named below. Python `cryptography` is required by the
fixture lane.

| Check | Command | Result |
|---|---|---|
| Guest builds for `wasm32-wasip1` | `scripts/build.sh wasm-project guest` | pass |
| Runner builds (Wasmtime 48.0.5) | `scripts/build.sh wasm-project runner` | pass |
| Guest unit tests (storage restore binding, forward-v2 refusals, chain, governance) | `scripts/build.sh test clawft-wasm --features project-kernel` | 123 passed |
| Runner tests (absolute I/O deadlines, adoption proof, parent-relay and WASI escape negatives) | `scripts/build.sh test clawft-wasm-host --features project-kernel` | 150 passed |
| Adapter identity through load/start/stop; kind capability per adapter | `scripts/build.sh test clawft-kernel --filter workload` | 403 passed |
| Project schema | `scripts/build.sh test clawft-types --filter project` | 78 passed |
| Driver, supervisor and daemon regressions | `scripts/build.sh test clawft-weave --features wasmtime-project` | 1083 passed, 2 skipped |
| Lifecycle against the real guest and runner | `scripts/build.sh test-wasm-project` (release) | pass |
| Workspace compile and lint | `scripts/build.sh check`, `scripts/build.sh clippy` | pass |

The nested-instance tests of `clawft-weave` time out occasionally when the whole
crate runs in parallel on a loaded machine; they pass alone and on re-run.

Negative tests that run in the suites above (not only the happy path):

- Forbidden parent methods: only `mesh.challenge`, `mesh.register`,
  `mesh.heartbeat`, `mesh.unregister` and `project.anchor.submit` are relayed;
  `chain.append`, `kernel.stop`, `project.start`, `governance.parent.update`, an
  empty method and non-JSON are refused, as is any request or payload naming
  another project (`sandbox_tests` in the runner).
- Storage and escape attempts: with the real Wasmtime WASI context a guest
  module cannot open `../x`, an absolute path, a nested `..`, or a symlink out of
  its `.weftos` preopen; it sees exactly one preopen and no environment
  (`sandbox_tests`). Writing inside the preopen works and lands in the project.
- Forwarded requests: tampered params or method, a stranger's signature, a
  missing signature, a stale or future timestamp, another project's id and a
  replay are refused (`forward/tests.rs`).
- Operator pins: group/world-readable configuration, zero fuel, tiny memory, a
  logical adapter, malformed or uppercase hash pins, unknown fields, an artifact
  inside the project, a project root overlapping supervisor authority, a
  non-0700 operator directory and unsafe runtime or state directories are
  refused; a substituted or symlinked artifact fails its pin; a malformed
  receipt blocks any fallback to the native adapter
  (`project_supervisor/wasmtime/tests.rs`).
- The lifecycle fixture covers a wrong artifact hash, a logical adapter and a
  one-unit fuel budget, a forged parent policy and registration acknowledgement,
  chain replay, tampering and cross-project forwards, a duplicate runner, session
  loss with signed re-adoption, a byte trickle, pending-anchor replay after a
  restart, a signed policy rollback (live and at restart), an overlay denial,
  graceful unregister, lifetime cancellation and revocation.


## Not done

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
4. The Wasmtime driver has not run under a real user daemon. The signing-parent
   fixture is a protocol oracle, not a substitute: an isolated user-daemon test
   with operator pins, the dedicated child socket and a daemon restart (adoption
   across `weaver` restart) is still required before the driver is enabled
   anywhere. Operator artifacts must be installed and pinned explicitly; pin
   changes deliberately refuse adoption of old runners and there is no automatic
   upgrade or pin migration. This is a depth-1 project below the user daemon;
   nested Wasmtime supervision is not implemented. `.weftos` mode 0700 is a
   prerequisite and the driver never silently chmods existing project state.
5. Storage uses full-chain signed snapshots, not an incremental journal. Guest
   memory/fuel are bounded, but there is no disk quota or compaction policy.
   Atomic rename and file fsync cover process failure; power-loss durability of
   directory metadata has not been established. An arbitrary replacement guest
   must never be approved merely because it satisfies the ABI: the approved
   artifact has signing authority and read/write access to its project state.
6. The guest crate reuses native sources by `#[path]` (`chain.rs`,
   `chain_subscribe.rs`, `parent_policy.rs`, `governance_overlay.rs`,
   `mesh_local.rs`); that composition must be addressed in publication
   packaging before `clawft-wasm` is published with this feature.
