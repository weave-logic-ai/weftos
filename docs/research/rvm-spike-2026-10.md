# rvm spike, 2026-10

Question: what of `ruvnet/rvm` works today, as input to a WeftOS ADR for an `rvm` workload kind.
Everything below was run on 2026-10-09 unless marked "not observed".

## Summary

- Host-side tests are green: 1281 passed, 0 failed across the workspace (`cargo test --workspace --lib`).
- The AArch64 kernel does not boot from a clean checkout. `make run` fails (wrong ELF name), and the binary built by hand hangs silently on QEMU `virt`. With two small local patches (bigger stack, enable FP/NEON) it boots to EL1 or EL2 and prints a witness count of 7. The patches are not upstream.
- `rvm-host` and `rvm-launch` are real, published on crates.io (0.1.1) and have an end-to-end lifecycle test. But no WASM engine is behind them: `spawn` validates the module header and sections and registers an agent record. Nothing executes.
- `HostedAdapter` engages no OS mechanism on macOS or Linux. It is a declaration layer; the embedding program must apply sandboxing itself and then tell the adapter. Without that the claim is `wasm-only`.
- `rvm-context-service` works end to end over TLS: put, CAS, read, resolve by `ruv://` URI, and an out-of-scope request returns the uniform `not_found` error. There is no scope-creation API; one scope and one capability are fixed at server start.

## Environment

| Item | Value |
|---|---|
| Host | macOS 27.0.1, arm64 (Apple silicon) |
| rustc / cargo | 1.93.1 (01f6ddf75, 2026-02-11) |
| Repo | `ruvnet/rvm` @ 510a82f, shallow clone, workspace version 0.1.1 |
| Submodule needed | `ruvector` @ 682e1c7 (`git submodule update --init --depth 1 ruvector`); required by `rvm-context-service` and, because Cargo resolves the whole workspace, by every `cargo` command in the workspace |
| Other submodules | `rudevolution`, `cuda-wasm`: not fetched, not needed |
| Installed for this spike | `qemu` 11.1.2 (brew), Rust target `aarch64-unknown-none`, `llvm-tools`, `cargo-binutils` 0.4.0 |
| OpenSSL (for the test cert) | 3.6.4 (brew, pulled in by qemu) |

Gotcha: a fresh clone without the `ruvector` submodule fails every workspace command with
`failed to read .../ruvector/crates/ruvector-context/Cargo.toml`, including `cargo test --workspace --exclude rvm-context-service`.

## Results per step

### 1. Host tests

```
git submodule update --init --depth 1 ruvector
cargo test --workspace --lib
```

Result: all test binaries `ok`, 0 failed. Sum of the per-crate lines: 1281 passed.

| Crate | Passed |
|---|---|
| rvm-anchor | 18 |
| rvm-boot | 26 |
| rvm-cap | 45 |
| rvm-coherence | 61 |
| rvm-context | 63 |
| rvm-context-service | 2 |
| rvm-gpu | 133 |
| rvm-hal | 18 |
| rvm-host | 83 |
| rvm-kernel | 67 |
| rvm-launch | 63 |
| rvm-memory | 110 |
| rvm-partition | 89 |
| rvm-proof | 138 |
| rvm-rvf | 102 |
| rvm-sched | 51 |
| rvm-security | 54 |
| rvm-tests (integration crate, lib) | 70 |
| rvm-wasm | 38 |
| rvm-witness | 50 |
| rvm-types, rvm-context-wasm, rvm-benches | 0 (no lib tests) |

Also run: `cargo test -p rvm-context-service` (integration `tests/durable.rs`: 6 passed), `cargo test -p rvm-launch -p rvm-host` (63 and 83 passed, doctests ignored).
Not run: `rvm-tests` integration tests beyond `--lib`, benches, miri, fuzz.

### 2. `make build` and `make run` (QEMU virt AArch64)

`make build`: succeeds (`Finished release profile`), produces `target/aarch64-unknown-none/release/rvm`.

`make run` fails:

```
qemu-system-aarch64: could not load kernel 'target/aarch64-unknown-none/release/rvm-kernel'
make: *** [run] Error 1
```

Cause: the Makefile expects `.../release/rvm-kernel`, but `crates/rvm-kernel/Cargo.toml` names the binary `rvm`. Running QEMU by hand on the right file:

```
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 128M -nographic \
  -kernel target/aarch64-unknown-none/release/rvm
```

prints nothing for 20 s (also nothing with `-M virt,virtualization=on`). Diagnosis with `-d int,guest_errors`:

1. First fault: Data Abort, FAR `0x3fffffa0`, ELR `0x400006d8`. The disassembly shows `rvm_main` probing a stack frame of about 0x5cea0 bytes (about 380 KB) before touching it, while `rvm.ld` reserves a 64 KB stack (`. = . + 0x10000`). The stack runs below the start of RAM.
2. With the stack raised to 1 MB: output reaches `[RVM] Exception level: EL1`, then hangs. Fault: Undefined Instruction, ESR `0x1fe00000` (EC 0x07, FP/SIMD access trapped). The boot stub never sets `CPACR_EL1.FPEN`, and the `aarch64-unknown-none` target emits NEON.
3. With a 4-instruction `CPACR_EL1` enable added at `_start`, it boots:

```
[RVM] Booting...
[RVM] Exception level: EL1
[RVM] Boot complete. First witness emitted.
[RVM] Witness records: 0x00000007
[RVM] Entering scheduler loop...
```

and with `-M virt,virtualization=on` the same output with `Exception level: EL2`.

The local patches (both reverted in the clone afterwards):

```
rvm.ld:                     . = . + 0x10000;  ->  . = . + 0x100000;
crates/rvm-kernel/src/main.rs, at _start:
    mrs x9, cpacr_el1 ; orr x9, x9, #(3 << 20) ; msr cpacr_el1, x9 ; isb
```

Observed vs the ask: "all 7 phases and the first witness appear". The first witness line appears (after patching). Seven phases are not printed individually; the only evidence of seven is the witness record count `0x7`. The `rvm-boot` crate's 26 host tests cover the phase tracker, but that is a host test, not QEMU output. Not observed: MMU/stage-2 setup, scheduler ticks, any partition running. Stock upstream at 510a82f does not boot on QEMU 11.1.2; whether it did on QEMU 8.x, or at an earlier commit, is not observed.

### 3. `rvm-host` and `rvm-launch`

ADR location: ADR-284..295 are not in this repo's `docs/adr` (which ends at ADR-159). They live in the `ruvector` submodule at `ruvector/docs/adr/` (ADR-284 execution contract, 285 hosted security boundary, 286 capability mapping, 288 base-state/delta lifecycle, 289 desktop host adapters, 291 version negotiation). This repo's `docs/RVFORGE-INTEGRATION.md` cross-references them.

Can a host program on macOS/Linux take an RVF package, verify it, and launch it? Partly.

- Verify: yes, real. `rvm_rvf::verify(bytes, &VerifyOptions)` checks container identity, segment structure and hashes, signatures against `trusted_keys`, capability declarations, and size policy. `VerifiedPackage::from_report(&report)` refuses a report with `ok == false`, so the type system blocks "launch without verify".
- Launch: the lifecycle is real, execution is not. `Instance::create/start/suspend/resume/checkpoint/restore/terminate` run a witnessed state machine. `start` calls `adapter.spawn`, which checks the module's size and runs `rvm_wasm::validate_module` (magic, version 1, section ordering and limits), then registers an agent record in `AgentManager`. `rvm-wasm` has no dependency on any WASM interpreter (its deps are `rvm-types`, `rvm-partition`, `rvm-cap`, `rvm-witness`). Its doc comment says modules "execute in a sandboxed interpreter", but there is no code that runs a module's instructions. Not observed: any WASM code running.
- Adapters: `WasmAdapter` (claim `wasm-only`), `HostedAdapter` (`os-sandbox+wasm`, or `wasm-only` when nothing engaged), `BareMetalAdapter` (`partition`, needs a live `PartitionManager`). Selection helper: `strongest(&[AdapterDescriptor])`.

Which OS mechanisms does `HostedAdapter` engage on macOS and Linux? None. The crate is `no_std` with `#![forbid(unsafe_code)]`. From `crates/rvm-host/src/hosted.rs`: it cannot call `sandbox_init`, `unshare(2)`, seccomp and so on; those are "unimplemented, needs a platform host". `HostedAdapter::new(os)` lists the per-OS stack (macOS: `MacosAppSandbox`, `MacosHardenedRuntime`, `MacosScopedEntitlements`, `MacosNotarization`; Linux: `LinuxNamespaces`, `LinuxCgroups`, `LinuxSeccomp`, `LinuxRestrictedMounts`, `LinuxNetworkNamespace`) all as `Unimplemented`. The embedding process applies a mechanism itself and then calls `.engaging(mechanism)` (or `.fully_engaged()`) to record it. The adapter trusts that call: it cannot check that the mechanism took hold. Consequences:

- Capability classes `filesystem`, `network`, `process` are only accepted when a confining mechanism is marked engaged; otherwise `prepare` returns `CapabilityUnenforceable` with a witnessed refusal.
- Device classes (gpu, sensor, display, audio) are never enforceable on a hosted adapter.
- `PORTABLE_CORE` (WASM memory isolation, default-deny capabilities, quotas) is reported as engaged, but with no engine behind it that is an unexercised claim.

End-to-end example/test? There is no example binary and no CLI. `rvm-launch` and `rvm-host` are libraries only (the ADR-289 "CLI verbs" are library methods). The end-to-end evidence is unit tests. Run:

```
cargo test -p rvm-launch the_full_lifecycle_runs_end_to_end
test instance::tests::the_full_lifecycle_runs_end_to_end ... ok
```

That test does create, start (with a minimal valid WASM header), suspend, checkpoint, resume, terminate and asserts the six witness events in order. Related passing tests: `a_hosted_instance_never_reports_bare_metal_isolation`, `a_suspended_instance_restores_from_its_own_checkpoint`, `the_same_package_runs_under_every_adapter_with_a_different_honest_claim`. All use synthetic fixtures from `rvm_host::testkit` (feature `testkit`), not a real signed `.rvf`. Not observed: verifying a real RVForge-produced signed `.rvf`.

### 4. `rvm-context-service`

Build:

```
cargo build -p rvm-context-service --features gateway-bin --bins
```

The binaries are behind the `gateway-bin` feature; without it `cargo build --bins` says "no targets matched". Built in about 13 s after dependencies.

Server configuration is entirely environment variables (`crates/rvm-context-service/src/bin/server.rs`):

| Variable | Value used |
|---|---|
| `RVM_CONTEXT_BIND` | `127.0.0.1:18443` |
| `RVM_CONTEXT_SCOPE` | `ruv://example.com/acme/user/alice/resources/docs` |
| `RVM_CONTEXT_ACTOR` | `41` (partition id) |
| `RVM_CONTEXT_ROOT` | scratch directory for the encrypted redb store |
| `RVM_CONTEXT_TOKEN_FILE` | file with a 64-hex-char bearer token (min 32 bytes) |
| `RVM_CONTEXT_DEV_KEK_HEX` | 64 hex chars (32-byte key) |
| `RVM_CONTEXT_ALLOW_LOCAL_KEK` | `1` (server refuses to start without it) |
| `RVM_CONTEXT_ALLOW_WRITES` | `1` (default is read + prove only) |
| `RVM_CONTEXT_TLS_CERT` / `_KEY` | PEM cert and PKCS#8 key |

Self-signed cert (must be `CA:FALSE` and carry a SAN for the client to accept it as its trust root):

```
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout key.pem -out cert.pem -days 1 -subj "/CN=localhost" \
  -addext "subjectAltName=DNS:localhost" -addext "basicConstraints=critical,CA:FALSE"
```

Client: `rvm-context-cli HOST PORT CA_PEM TOKEN_FILE ROUTE JSON_FILE` (POSTs the JSON file; exit 1 on HTTP status >= 400).

The put body needs a valid context RVF. There is no tool in the repo to make one, so I generated it with a throwaway integration test copying the `context_rvf` helper from `tests/durable.rs` (removed afterwards). Results:

```
# put (pinned uri ...?rev=sha256:<rvf hash>, body {"uri": ..., "rvf_base64": ...})
rvm-context-cli localhost 18443 cert.pem token.txt /v1/put put.json
{"alias":null,"revision":"sha256:4f79304b...bfd0ce","rvf_len":896,"uri":"ruv://example.com/acme/user/alice/resources/docs/item?rev=sha256:4f79304b...bfd0ce"}

# cas: bind the alias to that revision (expected: null, next_revision: <rev>)
{"alias":"ruv://example.com/acme/user/alice/resources/docs/item","generation":1,"revision":"sha256:4f79304b...","tombstone":false}

# read by alias uri
{"payload_base64":"aGVsbG8gY29udGVudA==","resolved":{...}}      # base64 decodes to "hello content"

# resolve by alias uri
{"alias":{...generation 1...},"revision":"sha256:4f79304b...","rvf_len":896,"uri":"..."}
```

Request without the capability (a URI outside the configured scope, tenant `other`):

```
rvm-context-cli ... /v1/resolve other.json
{"error":{"code":"not_found","message":"context unavailable"}}      exit=1
```

A URI inside the scope that does not exist returns the identical body, so the two cases are indistinguishable (the uniform error). A wrong bearer token returns a different response: `{"error":{"code":"unauthorized"}}` (HTTP 401). MCP: `tools/list` over `/mcp` returned `ruv_resolve, ruv_list, ruv_tree, ruv_read, ruv_search, ruv_history, ruv_verify, ruv_put, ruv_cas, ruv_forget`. Store on disk: `context.redb` and `active-index/` under the root.

"Create a scope": there is no such endpoint. The scope is the single URI prefix given at start, mapped to one root capability (`ContextAuthority::issue_root`) held by the process. A multi-tenant or multi-scope deployment would be one server process per scope, or new embedding code. Total time about 25 minutes including the build. Not observed: search (`/v1/search`), forget, history, receipt draining, a production key provider (only `LocalKeyProvider` with a dev key exists in the bin), HTTP keep-alive (server forces `keep_alive(false)`).

### 5. README "Implementation Status" cross-check

| Claim | Observed |
|---|---|
| Per-crate test counts (legacy snapshot, total 945) | Different but not contradicted: the README says the counts are a legacy snapshot. Current `--lib` counts are higher for most crates (e.g. rvm-proof 138 vs 45 listed, rvm-gpu 133 vs 65). 0 failures. |
| `rvm-boot`: 7-phase measured boot | Host tests pass; on QEMU the 7 phases are not individually visible (witness count 7 only) and the kernel does not boot unpatched. |
| `rvm-hal`: AArch64 EL2 stage-2 tables, PL011, GICv2, timer | PL011 output verified on QEMU. Stage-2, GIC, timer not exercised on QEMU in this spike. 18 host tests pass. |
| `make run` "boots at 0x4000_0000, PL011 UART output" (README lines 268, 663) | Fails as shipped: wrong ELF name in Makefile, then stack overflow, then FP trap. See step 2. |
| `rvm-host` "isolation selection and placement" | Selection/placement logic real; no OS confinement is applied (declaration only). |
| `rvm-launch` "verified instance lifecycle" | Real as a state machine plus witness chain. No execution of the module. |
| `rvm-wasm` "7-state agent lifecycle, section parser" | Parser and lifecycle present; no WASM runtime. The crate doc says "sandboxed interpreter"; none exists in the tree. |
| `Agent.rvm.img` installers etc. | README itself says roadmap. Not tested. |
| `rvm-ffi`, `rvm-node`, `rvm-policy` (named in `docs/RVFORGE-INTEGRATION.md`) | Not in the workspace (`crates/` has no such directories). `RVFORGE-INTEGRATION.md` lists `rvm-ffi` and `rvm-node` as the Tauri/Node bindings and `rvm-policy` as ADR-284 signed policy; I did not verify their status text beyond that the crates are absent. |

## API for a WeftOS integration

All of this is library code, `no_std + alloc`, `#![forbid(unsafe_code)]`; enable the `std` feature on each crate. Crates are on crates.io at 0.1.1: `rvm-rvf`, `rvm-host`, `rvm-launch` (and `rvm-types`, `rvm-wasm`, `rvm-witness`, `rvm-partition`, `rvm-context` as dependencies). Contract constants: `HOST_CONTRACT_VERSION = 1`, `LAUNCH_CONTRACT_VERSION = 1`, plus `rvm_rvf::RVF_CONTRACT_VERSION` (ADR-291 negotiation).

Verify then launch (from the crate docs; compiled in the unit tests):

```rust
use rvm_host::{HostAdapter, HostedAdapter, HostOs, IsolationMechanism, Placement, VerifiedPackage, WasmAdapter};
use rvm_launch::{inspect, verify, Instance, InstanceId};
use rvm_rvf::{VerifyOptions, WitnessContext};
use rvm_wasm::agent::AgentManager;
use rvm_witness::WitnessLog;

let log = WitnessLog::<256>::new();                          // const-generic ring
let report = verify(bytes, &opts, &log, WitnessContext::new(seq, ts))?;  // witnessed pass or fail
let package = VerifiedPackage::from_report(&report)?;        // Err(HostError::Unverified) if !report.ok

let adapter = HostedAdapter::new(HostOs::MacOs)              // or WasmAdapter::new()
    .engaging(IsolationMechanism::MacosAppSandbox)?;         // only after WeftOS actually applied it
let placement = Placement::new(PartitionId::new(1), epoch, max_memory_pages);
let mut agents = AgentManager::<4>::new();
let mut inst = Instance::create(InstanceId::new(1), adapter, package, placement, &log, now_ns)?;
inst.start(wasm_bytes, &mut agents, &log, now_ns)?;          // validates + registers; does not run code
inst.suspend(&mut agents, &log, now_ns)?;
let cp = inst.checkpoint(&log, now_ns)?;                     // bound to the base RVF identity
inst.resume(&mut agents, &log, now_ns)?;
inst.terminate(&mut agents, &log, now_ns)?;
let chain = inst.witness(&log);                              // Vec<WitnessRecord>
```

Types worth mapping onto WeftOS concepts: `VerifiedPackage` (identity, `granted_classes()`, `denied_classes()`, `is_granted(class)`), `IsolationClaim` (`WasmOnly`, `OsSandboxWasm`, `Partition`), `IsolationContext` (claim, granted classes, mechanisms), `MechanismSet::not_engaged()`, `HostError::{Unverified, CapabilityUnenforceable, MechanismNotInStack, ModuleTooLarge, ModuleRejected}`, `InstanceState`, `Checkpoint`, `WitnessRecord` (64 bytes, hash-chained, `rvm_witness::verify_chain`). Capability classes: 15 in `rvm_rvf::CapabilityClass::ALL`. Const generics (`WitnessLog<N>`, `AgentManager<M>`) mean ring and table sizes are compile-time choices.

Needed from WeftOS to make this meaningful, because the crate does not do it: (a) an actual execution engine for the WASM module (the existing `wasmtime`/WeftOS wasm path is the obvious candidate, with `spawn` used only as admission); (b) the OS confinement for macOS and Linux, applied before calling `.engaging(...)`; (c) a source of signed `.rvf` packages (RVForge, in the RuVector repo, is the producer; not tested here).

Context service as a separate integration: HTTPS/MCP gateway described in step 4; library entry points `ContextGateway::dispatch(route, body)` and `dispatch_mcp(body)` (transport-independent JSON), `PersistentContextResolver::open(...)`, `LocalKeyProvider`. Routes: `/v1/{resolve,list,tree,read,search,history,verify,put,cas,forget}` and `/mcp`.

## Gaps and risks

1. Kernel does not boot as shipped (Makefile ELF name, 64 KB stack against a ~380 KB frame, no FP enable). Local fixes are small, but they show `make run` is not part of upstream CI at this commit.
2. No WASM execution engine. "Launch" means admit and register. Any WeftOS `rvm` workload kind that promises "runs the agent" must bring its own engine.
3. Hosted isolation is self-declared. `.engaging(...)` is trusted; nothing verifies it. The honest default is `wasm-only`, which is a safe failure mode, but WeftOS would own correctness of every sandbox claim it makes through this API.
4. Whole-workspace dependency on a submodule: `ruvector` must be present for any workspace cargo command. Using the published crates (`rvm-host`, `rvm-launch`, `rvm-rvf` from crates.io) avoids that.
5. ADRs for the host/launch contract live in another repo (RuVector); ADR-291 version negotiation means the contract numbers (all at 1 today) should be pinned.
6. Context service: single scope per process, dev-only key provider in the shipped binary (`RVM_CONTEXT_ALLOW_LOCAL_KEK=1` required), no scope management API, HTTP/1.1 only without keep-alive, client expects a CA-valid server cert (CA:FALSE self-signed worked). There is no tool in the repo to build a context RVF for `put`.
7. README test counts are stale and its boot claim is false at this commit; treat README status as aspirational and rely on observed results.
8. Fixtures: all lifecycle tests use `testkit` synthetic containers. Verification of a real RVForge-signed `.rvf` is not observed.
9. rvm-ffi / rvm-node / rvm-policy named in integration docs are not in this repo.

## Licence

`MIT OR Apache-2.0` (workspace `Cargo.toml`, `license = "MIT OR Apache-2.0"`), authors "RuVector Contributors". Compatible with WeftOS use as a dependency. Submodule `ruvector` is a separate repo; its licence was not audited here. Transitive crates for the context service include `redb`, `aes-gcm`, `rustls`, `tokio`, `hyper`; not audited.
