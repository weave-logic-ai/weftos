# Handoff — ADR-103 topology completion — 2026-10-04

This checkout contains the uncommitted Phase 4 and certified-leaf integration on
top of the completed Phases 0–3. The source is substantially integrated, but it
is not ready to commit or push: the latest focused run stopped at two duplicate
imports, the Wasmtime slice is still in its worktree, and real Linux-container
driver acceptance has not run.

## Current state

- Branch `0.8-metaharness` at `e6662b818be7511c283cc68c8e6df46681f342a2`.
- The worktree has a large uncommitted integration spanning kernel, weave,
  mesh-service, leaf firmware/types, tests, scripts, and documentation. Do not
  discard, reset, or replace shared files wholesale.
- No completion commit or push has been made.
- Dashboard cards are already claimed and remain in progress:
  - Phase 4: `0dc06c23-56a0-4718-b7d5-aefc2f05d98c`
  - Leaf: `444c7964-0a35-40f2-9462-11c020419cfe`
- Process discovery was unavailable in the final handoff check. An unrelated
  Claude `integrate-w1` suite and an installed coordinator daemon had previously
  been observed; do not stop or signal either. All acceptance runs must use
  isolated task-owned state and processes.
- Preserve the pre-existing unrelated dirt: `.mcp.json`,
  `docs/research/crosscut-latest.md`, `.grok/compose/`,
  `docs/plans/dashboard-fleet-terraform-integration.md`,
  `docs/research/fleet-manager/`, `docs/research/pg-source-intake.md`, and
  `scripts/pi/__pycache__/`.

## Verified state

| Area | State | Evidence |
|---|---|---|
| Baseline before Phase 4 import | Pass | `scripts/build.sh gate`: 22/22; 12,300 nextest tests passed, 27 skipped |
| Combined native source check | Pass before the final D10 review delta | `completion-runs/combined-check-r2.log`, 1m39s |
| Combined native tests | Partial | 5,356 passed, 10 failed, 7 skipped; `combined-tests-r1.log` |
| Seatbelt boundary | Pass before final combination | host runs r4/r5: 4/4; real lifecycle RPC r6: 1/1 |
| ESP-IDF leaf firmware | Pass build only | release r4, 56.30s; all 50 source hashes matched; no device flashed |
| Leaf/D10/container final combination | Not yet verified | latest focused run stopped during compilation |
| Wasmtime project kernel | Source only | review fixes written and frozen; never compiled or run |
| Linux-container driver | Source and acceptance runner only | production driver tests passed before integration; real DinD gate never run |

The first combined run's ten failures were triaged. Source fixes are present for
the leaf Noise compatibility regression, runtime-path fixture boundary,
`instance.nested.*` scope classification, owner `kernel.shutdown`, portable D10
fixtures, project child-listener fixture, and bounded owned-pipe shutdown. Three
project-supervisor readiness failures occurred under concurrent machine load and
need a low-concurrency rerun before changing any timeout.

## Immediate compile blocker

`completion-runs/focused-tests-r3.log` failed before tests because each of these
files imports `nix::libc` twice:

- `crates/clawft-weave/src/nested_boot.rs` (lines 3 and 11 at handoff time)
- `crates/clawft-weave/src/nested_supervisor.rs` (lines 3 and 16 at handoff time)

Remove one duplicate import from each file. The preceding r2 compile errors were
already fixed: the strict nested seed call now passes the new `leaf_only` argument,
and `mesh_nested_dial_tests.rs` uses the current non-optional handshake-hash API.

## Integrated work that must be preserved

- Sandbox enum and supervisor support `Logical` (default, with `native` alias),
  `Seatbelt`, and `LinuxContainer`. Container protected-path validation and the
  nested-parent checks are both retained.
- Linux-container guest executable is `/usr/local/bin/weaver`; `/weftos` remains
  a directory for project/run/trust/parent mounts. The former `/weftos`
  entrypoint collided with its child mount paths.
- The dedicated child UDS denies mint, revoke, shutdown, and owner-only methods.
  The owner endpoint permits Admin `kernel.shutdown` under the user profile's
  read-only outside-project policy; `deny_all` still refuses it.
- D10 nested users use private HOME/runtime/key/chain/config, signed contracts,
  explicit mesh grants, reciprocal Noise-bound admission, and an owned stdin
  liveness pipe for bounded graceful cascade. Recovered processes are never
  signaled through an unverified PID.
- Mesh merge conflicts were resolved by retaining both `leaf_ingress` and
  `MeshAuthentication`, and by processing signed-leaf frames before reciprocal
  nested hello detection. Review this joint path; do not choose one side.
- Certified leaf support includes no-std signing/ACK/discovery/offline replay,
  service claim synchronization, reconnect cleanup, and ESP-IDF producer/NVS
  backlog. Legacy unsigned bare metal remains off by default.

## Open threads, in order

1. **Restore a compiling combined tree.** Remove the two duplicate imports,
   run `git diff --check`, then rerun the focused test expression below with two
   workers. Do not loosen supervisor deadlines unless the focused rerun still
   demonstrates a product failure.
2. **Run the real native smoke.** Rebuild `weaver`, then run
   `scripts/dev/p4-child-endpoint-smoke.py` against that exact binary. It must
   prove owner RPC, child method ceiling, claimed bootstrap dispatch, graceful
   shutdown, and socket cleanup. Earlier r3 reached owner shutdown and exposed
   the scope bug now fixed; there is no positive final run yet.
3. **Integrate Wasmtime semantically.** Source is frozen in
   `/Users/mathewbeane/.codex/worktrees/wasm-project/weftos`. Verify hashes in
   `docs/research/daemon-topology/adr103-wasm-project-freeze.json`, copy new files,
   and three-way merge tracked files. Never overwrite main's shared schema,
   supervisor, logical runtime, or `Cargo.lock`. The merge map and exact build
   sequence are in `adr103-wasm-project-slice.md` in that worktree.
4. **Compile and exercise Wasmtime.** Build the `wasm32-wasip1` guest and native
   Wasmtime 48.0.5 runner, run its focused tests, then the actual lifecycle
   fixture. The source includes fixes for authenticated checkpoint restoration,
   absolute socket deadlines, parent adoption after session loss, and graceful
   shutdown, but none has runtime evidence. Full native-kernel parity is not
   claimed: agents/workloads, shared ParentLink services, subscriptions, nested
   Wasmtime, key rotation, quotas, packaging, and power-loss validation remain
   outside this slice.
5. **Run actual Linux-container driver acceptance.** The implemented runner is
   `scripts/dev/p4-container-driver.py`; its contract is documented in
   `completion-runs/container-lifecycle-plan.md`. It requires a disposable,
   explicitly named Docker-in-Docker context where parent and dockerd share the
   same Linux PID/mount namespace and `/case` path. It must use a pinned child
   image containing the evaluated `/usr/local/bin/weaver`. Do not mount the
   operator's Docker socket and do not substitute the logical-inside-container
   harness or a `--version` smoke.
6. **Adversarial review and final gate.** Review the final leaf/D10/Wasm/container
   composition, run the full `scripts/build.sh gate`, curate receipts, update the
   two board cards to done only after acceptance, then commit on
   `0.8-metaharness` and push normally. Never commit to `main`/`master` or force
   push.

## Resume here

Run from `/Users/mathewbeane/weftos`. Long jobs must be detached and polled.

```bash
# 1. After removing one duplicate `use nix::libc;` in each named file:
git diff --check
rg -n '^use nix::libc;' \
  crates/clawft-weave/src/nested_boot.rs \
  crates/clawft-weave/src/nested_supervisor.rs

# 2. Focused native regressions, isolated from real user state.
mkdir -p target/it3/home target/it3/tmp target/it3/runtime target/n
nohup env \
  HOME="$PWD/target/it3/home" \
  TMPDIR="$PWD/target/it3/tmp" \
  WEFTOS_TEST_RUNTIME_ROOT="$PWD/target/it3/runtime" \
  D10_TEST_ROOT="$PWD/target/n" \
  CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true \
  cargo nextest run --config-file config/nextest.toml \
  -p clawft-weave -p clawft-kernel -p clawft-types \
  -j 2 --no-fail-fast \
  -E 'test(nested) | test(container) | test(scope_gate) | test(runtime_paths) | test(enforce_limits_leaf) | binary(project_kernel_e2e) | binary(project_supervisor) | test(claim_sync)' \
  > docs/research/daemon-topology/completion-runs/focused-tests-r4.log 2>&1 < /dev/null &

# 3. Once focused tests pass, rebuild and run the isolated daemon smoke.
nohup env CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true \
  scripts/build.sh native-debug \
  > docs/research/daemon-topology/completion-runs/p4-weaver-build.log 2>&1 < /dev/null &
# After that build exits zero:
python3 scripts/dev/p4-child-endpoint-smoke.py --binary target/debug/weaver

# 4. Inspect the Wasmtime handoff before importing anything.
sed -n '1,260p' \
  /Users/mathewbeane/.codex/worktrees/wasm-project/weftos/docs/research/daemon-topology/adr103-wasm-project-slice.md
python3 -m json.tool \
  /Users/mathewbeane/.codex/worktrees/wasm-project/weftos/docs/research/daemon-topology/adr103-wasm-project-freeze.json \
  > /dev/null

# 5. Container gate starts in plan-only mode. Read its --help before --run.
python3 scripts/dev/p4-container-driver.py --help

# 6. After all focused/runtime acceptance and review:
nohup env CARGO_INCREMENTAL=0 scripts/build.sh gate \
  > docs/research/daemon-topology/completion-runs/final-gate.log 2>&1 < /dev/null &
```

The Wasmtime handoff contains its own sequential guest/runner/test commands.
Use them only after semantic integration and continue to use the existing Cargo
cache; disk space was about 19 GiB at the final check.

## Dead ends — do not retry

- **Treating an engine/version smoke as container acceptance.** It proves only
  CLI reachability. The actual production supervisor must create, inspect,
  restart, adopt, revoke, and clean the child in disposable DinD.
- **The logical pair inside one outer container.** Useful supplemental lifecycle
  evidence, but parent and child share namespaces; its receipt explicitly cannot
  set `container_driver_accepted`.
- **Using `/weftos` as the executable.** It conflicts with the required
  `/weftos/project`, `/weftos/trust`, `/weftos/parent`, and `/weftos/run` mounts.
- **Process-group identity as a Seatbelt boundary.** Both `setpgid` and `setsid`
  succeeded in host probes. The dedicated child-only UDS is the boundary.
- **Broadening Seatbelt until dyld works.** The minimal necessary permission was
  `(allow file-read-data (literal "/"))`; the resulting strict boundary tests
  passed. Do not replace it with broad writable access.
- **Disabling Noise for leaf compatibility.** The service must retain Noise and
  admit authenticated CAP_LEAF publishing for its own substrate prefix.
- **Running broad parallel suites to diagnose supervisor startup.** Three
  readiness failures happened under heavy concurrent load. Use the focused
  two-worker run first.
- **Reusing stale worktree binaries.** Several worktrees had sources newer than
  their binaries after cache reclamation. Build from the integrated main tree.
- **Starting a second process-compose or touching installed daemons.** The repo's
  process-compose ports are 18090/18091; Forge ports and the installed coordinator
  are outside this acceptance work.

## Key paths

- `docs/research/daemon-topology/completion-runs/status.md` — short completion status.
- `docs/research/daemon-topology/completion-runs/coordination.md` — detailed chronological receipts and decisions.
- `docs/research/daemon-topology/completion-runs/combined-tests-r1.log` — 5,356/10/7 combined run.
- `docs/research/daemon-topology/completion-runs/focused-tests-r3.log` — current duplicate-import blocker.
- `docs/research/daemon-topology/completion-runs/d10-review-fixes.md` — D10 review fixes and targeted tests.
- `docs/research/daemon-topology/completion-runs/leaf-review-r1.md` — leaf adversarial review history.
- `docs/research/daemon-topology/completion-runs/container-lifecycle-plan.md` — exact real-driver topology and evidence requirements.
- `scripts/dev/p4-child-endpoint-smoke.py` — isolated real daemon/child endpoint smoke.
- `scripts/dev/p4-container-driver.py` — disposable DinD real-driver acceptance runner.
- `/Users/mathewbeane/.codex/worktrees/wasm-project/weftos/docs/research/daemon-topology/adr103-wasm-project-slice.md` — Wasmtime source handoff and semantic merge map.
- `/Users/mathewbeane/.codex/worktrees/wasm-project/weftos/docs/research/daemon-topology/adr103-wasm-project-freeze.json` — file hashes and import actions.

## Gotchas

- The current tree is intentionally dirty and combines several reviewed lanes.
  Do not reset it, copy an old worktree wholesale, or regenerate `Cargo.lock` from
  one stale branch.
- The old `leaf-disconnect-fix.patch` is stale; its changes are already in main.
- Keep `D10_TEST_ROOT` short on macOS because Unix sockets are limited to roughly
  103 bytes.
- Tests must never use real `~/.weftos` or `~/.clawft`; use private HOME/runtime
  directories and the nextest wrapper.
- No physical ESP32 flash, power-loss test, legacy bare-metal migration, or real
  Linux-container run has occurred. State those limits in release notes.
- Do not delete main's pre-existing large incremental cache merely to gain space.
  Earlier cleanup was limited to task-created worktree caches.
