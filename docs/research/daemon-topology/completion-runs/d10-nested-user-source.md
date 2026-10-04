# D10 nested user instances: source handoff

Lane: nested user implementation. Tree: `/Users/mathewbeane/.codex/worktrees/topology-finish/weftos`. Base: detached `e6662b818`; existing Phase 4 edits preserved. No commits, pushes, board actions, live-HOME writes, compiler jobs, or new target directories.

## Verification status

**Not compiled or runtime-tested.** The lead reserved the compiler lane for containers. This is ready for source merge/review and checking against main's existing cache, not an acceptance claim.

Completed checks: rustfmt syntax parsing of all 18 D10-touched Rust files, formatting of new Rust files, `git diff --check`, and a conservative source scan for exhaustive top-level `Config` literals missing the new field (none found). Syntax parsing is not macro expansion, typechecking, lint, or testing.

Reviewed references: ADR-103 D8/D10/D14; pending project.nested implementation; main completion-runs coordination, child-endpoint contract, and nested-instance-contract. `.agents/project-context.md` was absent in both checkouts. The reported host r5/r6 passes concern the earlier Seatbelt/child-endpoint source, not this combined implementation.

## Implemented behavior

- Top-level `weave.master` enables a user-instance supervisor. `instance.nested.register/start/stop/grant/revoke` are Admin routes, user-level scope-gated, and reject project-scoped callers. The child endpoint ceiling remains unchanged; it does not admit these routes.
- A registered instance launches the existing binary with `kernel start --foreground --profile user --config <private JSON>`, without `--project`. `env_clear` supplies private HOME, runtime, XDG directories and TMPDIR. It does not inherit credentials, mesh socket overrides or dynamic-loader variables. CLI `--config` is JSON; it is not the user weave.toml layer.
- The signed, domain-separated boot contract binds identity, parent/depth, private paths, config hash, generation, expiry, master cap/policy and registration. Boot validates the master pin, policy and inner key before daemon/mesh boot. A persistent generation floor and exclusive generation marker reject replay and concurrent consumption.
- Each inner has its own persistent user seed, chain, manifests and project authority. Its collapsed node identity uses that granted inner seed. The master's user key signs the boot contract and effective policy. Registry recovery verifies the signed record/config and preserves identity; it never adopts an unverified PID. Runtime locking checks stale sockets before a new launch.
- Default isolated registration disables mesh, service discovery and seed peers. No machine-service metadata probe occurs when service probing is disabled. Explicit collapsed grants bind the inner key, a nonzero loopback listening port, genesis and literal-address peers pinned by node ID. Discovery/open membership are disabled; existing Noise, genesis, revocation and governance admission remain active, with an additional peer ceiling.
- Readiness requires profile=user, no bound project, exact user/node key, parent, depth and owned PID. Short deterministic state directory names leave room for project sockets. Excessively long runtime roots are refused explicitly.
- Grants stop the existing process before changing config; revoke marks the registration and waits for shutdown/kill before returning. A parent-liveness pipe requests normal shutdown when the master dies, then bounds a wedged exit. Normal stop cascades to the inner's project supervisor. No same-UID machine-service rebind is used.
- Nested user boot feeds the existing overlay machinery through a signed-user trust path, leaving project certificate verification intact. Limits, rule hashes, version/history pins, overlay merging and gate installation are shared. Master reload/update quiesces nested users; their next explicit start takes a fresh master policy and reapplies the retained cap. A direct different parent-policy update/reload on the inner requires a new signed boot contract and restart, so it cannot replace the contract cap in place.
- Successful nested lifecycle RPCs append chain receipts. Per-instance `daemon.log` retains boot errors in private state.

## D10 files changed

New:

- `crates/clawft-types/src/config/nested.rs`
- `crates/clawft-weave/src/nested_boot.rs`
- `crates/clawft-weave/src/nested_supervisor.rs`
- `crates/clawft-weave/src/nested_rpc.rs`
- `crates/clawft-weave/tests/nested_instances.rs`

Integration:

- `crates/clawft-types/src/config/mod.rs`
- `crates/clawft-kernel/src/boot.rs`
- `crates/clawft-kernel/src/overlay_runtime.rs`
- `crates/clawft-kernel/src/mesh_admit.rs`
- `crates/clawft-kernel/src/mesh_admit_gate.rs`
- `crates/clawft-weave/src/lib.rs`
- `crates/clawft-weave/src/commands/kernel_cmd.rs`
- `crates/clawft-weave/src/daemon.rs`
- `crates/clawft-weave/src/rpc_ext.rs`
- `crates/clawft-weave/src/handshake_rpc.rs`
- `crates/clawft-weave/src/governance_push.rs`
- `crates/clawft-weave/src/scope_gate.rs`
- `crates/clawft-weave/src/mesh_boot.rs`

This handoff is the only D10 documentation addition. Other dirty files belong to the preexisting Seatbelt/project-nesting lane. Shared daemon/RPC/handshake edits were made only after the user declared that lane frozen.

## Exact remaining merge and verification work

1. Merge this combined worktree source into main without replacing main's reviewed container changes. Main is authoritative for `ChildProbe::Unverifiable`, async `refresh_token`, `ProjectSandbox::LinuxContainer`, and protected runtime trust paths. Specifically reconcile nested overlay path construction with main's trust-path APIs; preserve both endpoint accept loops and their existing authorization ceiling.
2. When the lead releases the compiler lane, typecheck the affected crates using main's existing cache with `CARGO_INCREMENTAL=0`. Resolve type/macro/feature errors before claiming build readiness. No separate full target or all-feature build is needed for this first check.
3. Run the new `nested_instances` integration target and the nested config/mesh-ceiling unit tests. Supply `D10_TEST_ROOT` under a short path such as main's `docs/n`; the long isolated-worktree path cannot hold a nested project's macOS UDS. Tests create their own private subdirectories and do not use `/tmp` or live HOME. The real-daemon tests require the newly integrated `weaver` binary.
4. New tests cover disabled master, private keys/config, launch arguments/environment allowlist, cap relaxation and rollback, tampered/wrong/expired signatures, replay, wrong-key grants, persisted identity/revoke, real inner project supervision, restart identity, mandatory-port collision, and listener/session closure on revoke. These are authored but **none has run** in this lane.
5. Run main's `scripts/dev/p4-child-endpoint-smoke.py` against that binary, rerun strict Seatbelt and lifecycle probes, and run the reviewed container tests on the merged source. Earlier binaries/pass logs do not validate the combination.
6. Add/run an authenticated two-peer mesh probe to verify positive admitted traffic and rejection of an otherwise-valid but ungranted peer at the real wire boundary. Run a poisoned-parent-environment/no-outer-socket spy probe: source constrains the environment and skips probing, but the existing tests do not demonstrate those negatives with an instrumented outer service. Exercise master-death cleanup and nested registry recovery after abrupt termination. These runtime proofs remain acceptance work.
7. Run the repository's appropriate lint/build/typecheck/container gates before any later commit. No commit is authorized by this handoff.

Operational limits: this follows the existing same-UID logical-supervision trust model, not a newly added OS sandbox for nested users. Running processes are owned by `Child` handles; restart recovery preserves registrations/keys and refuses locked live instances instead of guessing PID ownership. Master cap updates intentionally stop nested instances and require explicit starts. Loopback collapsed listeners and explicit pinned seeds are the supported minimal registration form. No automatic port allocation or transparent live adoption is claimed.
