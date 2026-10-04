# D10 review-fix delta

Tree: topology-finish/weftos, detached e6662b818, uncommitted. These changes are relative to the frozen D10 source already imported into main, not relative to Git HEAD. Main was neither edited nor built. No compiler, new target, live daemon or board operations were run.

## Three findings addressed

1. Repeated-start policy availability: refresh compares the effective rules and limits, schema, issuer and hash. A semantic no-op leaves config, boot contract, policy signature and generation bytes unchanged. A changed effective policy must stop the instance before any rewrite. `start` also proves a recovered runtime quiescent before persisting refreshed boot files. This finding was an availability failure, not cap relaxation; the existing rule hash includes limits.
2. Recovered lifecycle: after owned-child shutdown, or when recovery has no Child handle, terminate must acquire the runtime lock and run the existing stale-socket probe under that lock. Held locks, live sockets without a lock, permission errors and unverifiable state are refusals. No recovered PID is signalled. Revoke may persist intent while returning an error, but never reports completed revocation for a live/unverified endpoint.
3. Outbound authentication: nested seeds require Noise, a pinned expected ID, signing identity and the enforcing runtime admission gate. They request a reciprocal hello. An upgraded responder sends its signed hello only after admitting the dialer; the reply binds its Ed25519 key to the actual Noise static key/session. The nested dialer verifies that reply through the normal genesis/revocation/governance/peer-ceiling gate and exact expected-ID check before installing any route, forwarding inbound data or exposing an outbound queue. Its running pump keeps the real gate for revocation instead of AllowAll. Legacy non-nested seed behavior is unchanged. A responder that cannot supply the reciprocal proof fails closed; there is no legacy fallback for nested grants.

Main's already-discovered `use nix::libc` fix is included in both nested source files. Avoid applying duplicate imports during the semantic merge.

## Delta artifacts

- `d10-review-fixes.patch`: only these eight source/test files versus the imported freeze.
- `d10-review-fixes-manifest.json`: imported and changed SHA-256 hashes.
- `d10-review-imported-baseline.json.gz`: captured imported-source text used to construct the delta.

Files:

- crates/clawft-weave/src/nested_supervisor.rs
- crates/clawft-weave/src/nested_boot.rs (libc import only)
- crates/clawft-weave/tests/nested_instances.rs
- crates/clawft-kernel/src/mesh_runtime.rs
- crates/clawft-kernel/src/mesh_serve.rs
- crates/clawft-kernel/src/mesh_admit_gate.rs
- crates/clawft-kernel/src/boot.rs
- crates/clawft-kernel/src/mesh_nested_dial_tests.rs (new)

## Regression tests, authored but not executed

- `nested_policy_noop_preserves_signed_boot_and_policy_bytes`: fails the old unconditional signed-policy rewrite.
- `nested_user_supervises_project_and_restarts_with_private_identity`: now repeats refresh/start, checks unchanged PID and policy bytes, and calls governance.reload. The old implementation's changed signature makes reload fail.
- `recovered_nested_stop_and_revoke_refuse_live_unowned_endpoint`: holds the runtime lock, then separately serves a lockless UDS; stop/revoke must refuse and preserve reachability. It also requires a changed cap to leave disk bytes untouched when shutdown cannot be established. After the listener closes, stale cleanup succeeds. The old implementation claimed success/unlinked.
- `nested_seed_requires_reciprocal_proof_before_routing`: a Noise server at the configured address sends missing/forged/wrong-key/wrong-Noise-bound proof plus a frame claiming the granted ID. Nothing may reach delivery or remain routable. The old AllowAll pump delivered the frame.
- `nested_seed_reciprocal_admission_is_verified_and_bidirectional`: real loopback Noise runtimes exchange authenticated messages in both directions; delivery must see verified membership, not merely a pinned string.

No defect-plant/run proof is claimed: compiler/test execution remains exclusively with the lead. Syntax parsing for all eight delta Rust files and git diff --check passed. `git apply --stat` parsed the generated delta successfully.

After merge, using main's existing cache and the lead's detached runner:

```sh
CARGO_INCREMENTAL=0 D10_TEST_ROOT=/Users/mathewbeane/weftos/docs/n cargo test -p clawft-weave --test nested_instances -- --test-threads=1
CARGO_INCREMENTAL=0 D10_TEST_ROOT=/Users/mathewbeane/weftos/docs/n cargo test -p clawft-kernel --features mesh,exochain nested_seed_ -- --test-threads=1
```

The short docs root is required for the real nested/project and orphan-UDS tests on macOS. Preserve main's reviewed container/trust-path integration while merging the boot hunk. Re-run combined endpoint, Seatbelt and container checks after these D10 tests; previous binaries do not contain the reciprocal path.

## Fixture portability follow-up

All nonspawning nested fixtures now use `env!("CARGO_BIN_EXE_weaver")` instead of `/bin/true`, which is absent on the lead's macOS host. Only `nested_instances.rs` changed in this follow-up; the cumulative patch and manifest were regenerated against the same imported baseline. Main's scope-gate classification and admin-shutdown fixes were not touched. The lead reported the old-freeze real granted-listener collision/revoke test passed; this lane has not rerun tests or compiled the updated fixtures. Rustfmt and diff whitespace checks only.

## Owned-pipe shutdown follow-up

`terminate` now bounds its shutdown RPC attempt to two seconds. A denial, unavailable endpoint, transport error or timeout explicitly drops/takes the owned Child's stdin pipe, invoking the inner boot's existing EOF-to-SIGTERM graceful cascade. The existing fifteen-second overall graceful wait remains; forced kill uses only the owned Child handle. Recovered child=None logic is unchanged and never signals an unverified PID. No main scope-gate/admin-shutdown changes were made.

New regression `nested_owned_pipe_cascades_when_deny_all_rejects_shutdown_rpc` starts a real inner project using a project-scoped owner request, proves unscoped shutdown is denied, then requires bounded supervisor stop to close both user and project sockets. The existing cascade assertion is retained. This test is authored, not executed here. The cumulative patch/hash manifest was regenerated; no build, new target, or live-daemon operation was run.
