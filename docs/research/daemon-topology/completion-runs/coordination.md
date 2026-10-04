# ADR-103 completion run — 2026-10-04

Base inspected: `0.8-metaharness` at `e6662b818`. Existing unrelated working-tree changes were preserved. No commits or pushes in this run.

## Scope verified

The dashboard lists topology phases 0–3, ADR-102 API playground work, and install/update as done. Source includes the gateway playground and daemon-token authentication. Phase 4 and leaf cards remained todo and unassigned; both were claimed through an approved dashboard CLI call in this session.

- Phase 4 ticket: `0dc06c23-56a0-4718-b7d5-aefc2f05d98c`.
- Leaf ticket: `444c7964-0a35-40f2-9462-11c020419cfe`.

Phase 4 acceptance: pluggable sandbox drivers, Linux container isolation and macOS Seatbelt, nested `weave.master` controls, real isolated child-kernel verification. Leaf acceptance: certified provisioned Ed25519 keys, canonical SHA256 identity, required signed publishing with explicit bring-up exception, discovery, bounded offline replay.

## Active implementation lanes

- Phase 4: managed worktree `topology-finish/weftos`; Codex session `01a10839-caf1-77e2-aa6f-27f7c9dfc8b6`.
- Leaf: managed worktree `leaf-finish/weftos`; Codex session `01a10839-caf4-7500-814e-761eaa0d92f0`.

Both are instructed to implement locally, use isolated tests, report exact remaining gaps, and make no commits, pushes, live-daemon changes or hardware flashes. Independent review and integration remain required. Use Codex wait_threads for current state; do not infer activity from the original shell PIDs.

The initial shell-detached launches exited. Independent process-session launches succeeded; subsequent continuation messages are managed through the Codex app.

## Verification

A baseline `scripts/build.sh gate` started as detached PID 31147. Its summary log is `baseline-gate.log`; per-check details are in the main checkout's `target/gate-logs`. At this receipt's creation the workspace test build was still compiling: no final gate result is claimed.

## Permissions and coordination

An implementation agent's browser board request was denied by automatic approval review, reported as permission declined. No browser retry is authorized or attempted. This session's separately reviewed CLI claims succeeded before the lead read that denial. Both agents were told the claim prerequisite was fulfilled and to perform no further board operations.

## Still required before completion

Review both diffs adversarially; fix findings; verify integration tests, typecheck/lint/build and relevant container builds; run the final gate; obtain phase review; validate the nested development-kernel flow in isolated state; distinguish host tests from real ESP32 and Linux runtime evidence. Do not mark either card complete based on an agent's implementation report alone.

## Progress update

The baseline discovered 12,300 tests and has begun execution; no final result yet. A read-only specialist reviewed the Linux lifecycle mismatch and produced `p4-architecture.md` with a concrete contract for immutable container identity, host/guest paths, protected bootstrap state, verified registration/adoption and master-controlled nested registration. This has been sent to the Phase 4 lane.

Early code review found overly broad Seatbelt grants, unsafe passing skips for sandbox failures, and leaf acknowledgment/replay/filesystem concerns; the lanes have received these findings. They remain open until tested fixes are reviewed.

An attempted recurring continuation heartbeat was rejected by automatic review because the user had not explicitly requested a schedule. No automation was created; do not retry or work around that rejection. Current implementation sessions and baseline test job are the only active work dispatched here.

## Lane split and strict runtime result

Linux containers now have a separate implementation lane: managed worktree `project-container/weftos`, session `01a10852-1ceb-7742-8ad1-78d7e43549e6`, initial PID 96662. It owns the reviewed container lifecycle contract; the original Phase 4 lane retains Seatbelt and full nested registration. Shared-file conflicts require lead integration. All remain under the already-claimed Phase 4 umbrella ticket.

The lead ran `scripts/build.sh test clawft-weave --filter seatbelt` through a separately approved host call with isolated test HOME/runtime. Log: `p4-seatbelt-host.log`. Result: 4 run, 1 passed, 3 failed (exit 100). Failures: quoted-path child non-success, allowed project read denied, supervised child aborted with signal 6 before readiness. This is a confirmed runtime failure and has been sent to the author. Passing skips for PermissionDenied must not mask it. No successful Seatbelt runtime verification is claimed.

Leaf early-review additions: strip forged inner scopes and prove certified-parent-only delivery on the machine service; a duplicate subscribe after reconnect must restore its connection subscription. Replay effect atomicity and bounded trusted-state file handling still require verification.

## Latest verification

Baseline nextest completed: **12,300 passed, 27 skipped**, no failures. Doctests and the remaining gate stages are still running; this is not a final gate result.

Strict Seatbelt host runs r2 and r3 still failed with SIGABRT before readiness. Targeted crash reports put the abort in dyld's cache bootstrap. Independent short host probes using `/usr/bin/sandbox-exec` and `/usr/bin/true` isolated the missing permission: adding only `(allow file-read-data (literal "/"))` makes startup succeed; root metadata, global file-map-executable, or system-fsctl alone do not. The finding was sent to the Seatbelt lane. Full supervised enforcement still needs a passing rerun.

The official `espressif/idf-rust` ARM64 image was downloaded at manifest digest `sha256:4eadb6ba185332e0605b0c89287e8038288c4c7160d43c996da4a69d15cffca3`. An isolated no-network/no-mount compiler probe confirmed Rust 1.93.0-nightly (ESP 1.93.0.0), Cargo, and ldproxy. Firmware compilation is pending the source snapshot; no hardware was accessed. Image source: <https://hub.docker.com/r/espressif/idf-rust>.

Container work-in-progress review probes are in `container-review-notes.md`. In particular, adoption must validate isolation settings and reject additional mounts, not merely find expected mounts.

## Confirmed platform progress

Seatbelt host r4 passed **4/4**, exit 0: both actual supervised deny probes (ordinary and quoted root) and profile tests. Log `p4-seatbelt-host-r4.log`. Root-directory literal data read fixed dyld startup. A later auth-boundary change will require its own test.

Independent disposable host probes showed `setpgid(0,0)` and `setsid()` both succeed under the proposed sandbox process grants. Process-group-only parent authentication therefore is not a sandbox boundary. The specialist contract in `child-endpoint-contract.md` specifies a dedicated child-only UDS, endpoint-level allowed methods, and no access to the owner socket. The Seatbelt/nested lane is implementing it; the container lane still needs this integration contract after its active CLI writer yields.

Firmware r1 reached Xtensa application compilation and failed on five anyhow conversions from no-std codec errors. The author fixed them. Firmware **r2 release build passed** in 3m55s, log `esp-firmware-build-r2.log`. Task-owned Docker container `weftos-leaf-build-r2` is stopped and retained solely for incremental final compilation. It holds no real credentials; only the example Wi-Fi strings and unprovisioned source were built. Snapshot is `target/leaf-firmware-snapshot-r2` in the main checkout.

The independent leaf review (`leaf-review-r1.md`) still blocks completion on observation production, reconnect subscription, ACK/push demultiplexing, and unsupported advertised capabilities. Lead also requires compatibility with the default Noise-required machine service without disabling its existing security. The author is fixing these; the passing r2 firmware build predates them and is not final sign-off.

Baseline gate passed stages 1–15 and was running stage 16 at this update. Disk free space is about 25 GiB; avoid duplicate full builds or new Cargo targets. Preserve unrelated artifacts and live services.

Baseline final result: **22 passed, 0 failed, 0 skipped; GATE_EXIT=0**. This includes all 12,300 nextest tests (27 individual tests skipped), doctests, native release builds, WASM matrices, UI tsc/vite, audits and warnings-as-errors Clippy. This proves the original main checkout baseline, not the pending worktree changes.

Disk then fell to about 20 GiB. Both app-managed implementation lanes were asked to pause compiler invocations once current jobs finish so only their newly generated incremental caches can be reclaimed (leaf 12 GiB, Seatbelt/nested 9 GiB). Sources, test binaries, logs and unrelated targets must be preserved. No cache deletion has occurred at this point.

Both lanes subsequently confirmed compiler idle. Independently approved cleanup removed **only** their task-generated `target/debug/incremental` directories, with exact resolved-path guards. Sources, binaries, logs and firmware evidence remain. Future runs use `CARGO_INCREMENTAL=0`; about 32 GiB was free afterward.

Seatbelt host **r5 passed 4/4**, exit 0 (`p4-seatbelt-host-r5.log`). The expanded supervised probe now forks and calls `setsid`, then tries literal admin and a test owner token for mint/revoke/shutdown against the real child RPC handler. All are refused; direct/symlink owner-socket connections, private file access, sibling signal permission, protected-pin replacement and run-directory rename are also denied. Actual `daemon::run` endpoint wiring, allowed child calls and full nested-instance behavior still need integration verification.

Firmware final r3 snapshot lacked the newly introduced local weftos-leaf-scene dependency. Added that source-only crate and reran the retained task compiler as r4: release build passed in 56.30s, FIRMWARE_BUILD_PASS. No hardware or real credentials. Final leaf source rereview is active.

Independent current container review found four blockers (container-review-r1.md): writable project/trust path overlap, incomplete inspect isolation/mount set validation, inspect errors treated as Running, and create-success/followup-failure orphan name collision. Container CLI writer remains active so app followup delivery is blocked until it yields; fixes are not claimed delivered. Nested implementation agent resumed for genuine D10/runtime wiring completion.

Container writer yielded and four-blocker review plus dedicated endpoint contract were successfully delivered through app followup. Current source fix round active. Final leaf review closed producer/reconnect/demux/outbound revalidation, but requires certification ownership for parent_scope; host enrollment fix active, firmware frozen at passing r4. Disk fell to14GiB; all lanes paused new builds. Idle container lane generated5.9GiB incremental cache is being reclaimed with explicit approval; main165GiB incremental cache is pre-existing/unrelated and preserved.

Host r6 ran already-built lifecycle_rpc_end_to_end with isolated HOME/runtime; PASS1/1 in12.5s, no compilation (p4-child-endpoint-host-r6.log). Container cache cleanup complete; free18GiB. Only leaf lane authorized next sequential focused tests/check/clippy with CARGO_INCREMENTAL=0. Container and nested remain compiler-paused. Native worker Zeno (01a10881-59e9-7842-9e8d-2c8f514d0358) implements genuine nested user-instance in topology-finish, preserving endpoint lane edits. Native explorer Godel (01a10882-3d99-7161-958a-35ef288ce203) assesses exact WASM project driver gap.

New real-binary smoke script scripts/dev/p4-child-endpoint-smoke.py is syntax-checked. Negative baseline first found test-harness config format bug (--config reads JSON); corrected. Negative baseline r2 booted original daemon successfully and failed missing child listener as intended after45s, then cleaned up its own process/state. The final smoke additionally pins LLM probe to loopback port0 so it cannot probe the operator's default8090 backend. Positive integrated binary run remains pending.

Wasmtime assessment confirms existing WASI component/library and percall plugin runtimes do NOT implement a supervised project kernel. Managed worktree wasm-project created at e6662b818, worker Noether01a10886-5c65-79d3-b36e-8778d24fd6cb implements bounded real guest/runner contract source-first with no builds yet.

Container source freeze final imported28 modified+3newfiles to main at samebase e6662b818; container-integration.json lists files/hash. No staging/commit. Independent review closes stop escalation too; focused12/12. Full target cleanup approved after copying14logs to container-logs; sourceworktree preserved, disk24GiB. Integration with child endpoint and actual engine lifecycle tests still pending. All other lanes retain source ownership; nested/Wasmtime workers will hand coherent source for shared-cache integration rather than newfulltargets.

Leaf routing host r1 exposed a real test failure after the sandbox EPERM was removed: BrokenPipe at mesh_p3_e2e.rs:74 after intentional forged AddressAdd. Author notified to preserve fatal BadSig policy and use a fresh valid session if appropriate, with assertion forged claim never installs. No routing pass claimed.

Lead split imported container.rs unchanged test module into container_tests.rs, preserving container::tests namespace and formatting both. Production file442lines/test330lines. This structural integration change awaits final integrated tests.

## Combined source integration

Resolved schema, supervisor and lifecycle conflicts while preserving container protections and nested-parent checks. Sandbox selection is Logical (default, native alias), Seatbelt or LinuxContainer; refresh remains async. Source diff check passes; integrated compilation/runtime validation still pending. Compiler lane paused at 3 GiB free; idle topology cache reclamation preserves named test binaries and logs.

Leaf frozen source imported: 32 modified + 3 new files, automatic merges clean in kernel boot and mesh boot. Barrier check was green in author lane; final binary stale and main tests still required. Combined check r1 failed only seven unresolved libc references; fixed via nix::libc imports. Combined check r2 detached PID96729, CARGO_INCREMENTAL=0, verbose. D10 reviewer found 3 blockers; source fixes delegated to Zeno. WASM reviewer found 4 blockers; Noether owns fixes and launcher wiring. Idle worktree compiler caches reclaimed with source/log/test-binary preservation; unrelated integrate-w1 compiler observed and left untouched.

Combined native check r2 PASSED (1m39s), including imported leaf barrier and nested/container/Seatbelt source. Combined tests r1 now detached PID86029: weave/kernel/types/mesh-service/leaf-types, private HOME/runtime under target/it1, no-fail-fast, CARGO_INCREMENTAL=0. Host local socket/child-process test execution approved. Review fixes remain pending; do not claim final acceptance.

Combined tests r1 finished: 5356passed,10failed,7skipped in528s after6m11s compilation. Confirmed leafNoisecompatibility regression fixed in source; newbarrier startup/disconnect corrections imported. Container executable/mountcollision fixed to /usr/local/bin/weaver via reviewed sourcepatch. Admin kernel.shutdown now userlevel under read_only, childendpoint remainsdenied; newtest added. Runtimepath fixture homeboundary corrected; nestedprefix classification added. D10delta awaiting pipeclose and portablefixture finalization. Project e2e fixture missingchildlistener delegated. Three supervisor readiness timeouts under concurrent machine load require focusedserial rerun, not automatic timeoutrelaxation. Failednestedtest orphan PID6094 identityverified at target/n/nrxRoMn/... and SIGTERM onlythatprocess; unrelateddaemons untouched. Childsmoke r2 provedboot+ceiling; r3 exposedownershutdown scopebug.
