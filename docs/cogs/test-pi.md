# Real Pi 5 test lane (`scripts/build.sh test-pi`)

Card mesh-placement-fu-pi-test-lane. Decision (2026-09-29): anything that has to
prove it runs on ARM is tested on the real Raspberry Pi 5, not on the Mac or in a
container standing in for it. Governing design:
[ADR-099](../adr/adr-099-governed-workload-placement.md),
[COG-001](../adr/cog-001-cog-workload-kind.md); the harness it drives is
described in [conformance-harness.md](conformance-harness.md).

```bash
export WEFTOS_PI_HOST=pi5                              # [user@]host or ~/.ssh/config alias
scripts/build.sh test-pi                               # full lane (see below)
scripts/build.sh test-pi clawft-kernel                 # one crate's tests on the Pi
scripts/build.sh test-pi clawft-kernel --filter chain  # libtest name filter
scripts/build.sh test-pi --live-native                 # native adapter live test only
scripts/build.sh test-pi --cogs                        # cog conformance (ssh mode) only
scripts/build.sh test-pi --placement                   # two-node placement, Mac -> Pi (card 12)
scripts/build.sh test-pi --dry-run                     # print the plan, touch nothing
scripts/build.sh test-pi --help
```

`WEFTOS_PI_HOST` is never committed. When it is unset the lane prints a `SKIP`
line and exits 0, so it is safe in scripts that also run off the tailnet.

## What a run does

The orchestrator is `scripts/pi/pi_lane.py`, and its pure helpers are in
`scripts/pi/pi_plan.py`. Their unit tests are `scripts/pi/test_pi_plan.py`; run
them with `python3 -m unittest discover -s scripts/pi`.

1. **Preflight.** It checks over SSH that the Pi is `aarch64`, reads its glibc
   and `$HOME`, and records the size and mtime of the Pi's operator files
   (`~/.clawft/chain.rvf`, `chain.key`, `chain.tree.json`, `config.json`,
   `node.key`) and the state of `weaver.service` with its `MainPID` and
   `ActiveEnterTimestampMonotonic`, so a restart counts as a change. On the Mac it records the
   mtime of `~/.clawft/chain.rvf`. If the Pi probe fails or comes back
   incomplete, the lane stops before building anything.
2. **Cross-build.** It runs `cargo test --no-run` inside
   `rust:<rust-toolchain.toml channel>-bookworm` with `--platform linux/arm64`
   (OrbStack). Bookworm has glibc 2.36, and the lane refuses to run if the
   builder's glibc is newer than the Pi's (2.41 today). The workspace is mounted
   read-only at `<Pi $HOME>/weftos-test-pi/src`, the same absolute path it will
   have on the Pi, so compile-time `env!("CARGO_MANIFEST_DIR")` paths such as
   `../../config` resolve there. Build output goes to `target/pi-aarch64/`, the
   cargo registry is cached next to it, and the rustup toolchain is cached in the
   `weftos-pi-rustup` docker volume. The test executables come from cargo's JSON
   artifacts. With `--cogs` it also builds the `cog_adapter_run` launcher.
3. **Stage.** It removes any `~/weftos-test-pi` a killed earlier run left behind
   (with `sudo -n` for root-owned harness files), recreates
   `~/weftos-test-pi/{bin,src,home,runtime,tmp}` on the Pi, rsyncs only the
   git-tracked files under `crates/`, `config/` and `assets/` (plus the workspace
   manifests) into `src/`, and puts the test binaries in `bin/`. Untracked local
   files never leave the Mac. A tracked file deleted locally but not committed
   is left out of the list (with a NOTE), so it does not fail rsync.
4. **Run.** Each binary runs from its crate directory, as `cargo test` would, as
   `env -i PATH=… HOME=~/weftos-test-pi/home WEFTOS_RUNTIME_DIR=~/weftos-test-pi/runtime
   TMPDIR=… XDG_*=… CARGO_MANIFEST_DIR=… <bin> [filter]`. Nothing from the login
   environment leaks in, and nothing can reach `~/.clawft` or the system weaver on
   `:9470`. Output streams back live. A binary passes when it exits 0, libtest
   printed a `test result:` line, and no test failed. With `--filter`, the stage
   also fails if the filter selected zero tests (passed, failed or ignored)
   across all binaries, so a typo cannot report green. Each binary runs under the Pi's `timeout -k 10 <--timeout>`, so
   a hung test is killed on the Pi, not only the local ssh client.
5. **Native adapter live test.** The clawft-kernel lib test binary runs
   `workload_runtime::tests_live::live_native_anomaly_detect` with
   `WEFTOS_NATIVE_LIVE=1`, using the released `cog-anomaly-detect-aarch64` (fetched
   and ELF-checked through the `scripts/cogs` cache). This stage passes only if
   the test actually ran (1 passed, not an early return) and printed its
   interval-run evidence line. It runs `NativeRuntime` under `WorkloadHost` with
   an in-memory chain on UDP 5006, so that port must be free on the Pi.
6. **Cog conformance, remote mode.** `scripts/cogs/conformance.py sweep --runtime ssh --sudo`
   runs twice in `expected` mode for anomaly-detect, fall-detect, sleep-apnea
   and health-monitor (`--cogs-ids` to change): once with the plain harness, and
   once through the Pi-built `cog_adapter_run` launcher with the native adapter,
   dropping the cog to `65534:65534`. The work dirs are `~/weftos-test-pi/cogs-*`,
   set with the ssh adapter's `--remote-dir`. Results land in
   `scripts/cogs/results/pi5-ssh-aarch64-expected/` and
   `scripts/cogs/results/pi5-adapter-native-aarch64-expected/`. `--sudo` is
   needed only because the harness ingest stub binds `127.0.0.1:80`.
7. **Two-node placement (`--placement`, card mesh-placement-12).** The lane
   cross-builds the `workload_node` example for the Pi and builds it natively
   for the Mac. On the Pi it starts `workload_node serve` detached under the
   same `env -i` isolation, from `~/weftos-test-pi/runtime`, listening on mesh
   port **9471** with Noise XX (the system weaver keeps `:9470`), trusting only
   a throwaway controller key generated for the run, with a synthetic ADR-069
   feature feed on `127.0.0.1:15006`. On the Mac, `workload_node place` signs
   the released anomaly-detect binary as an operator package, learns the Pi
   over a signed `workload.describe` (its probed, signed facts) and runs the
   placement control plane: the engine must choose the Pi
   (`aarch64-native`, tier `native`) over the Mac (`aarch64-container`, tier
   `dev_fallback`); the Pi fetches the package from the Mac over the same
   connection before loading, admits it and runs it; after a few seconds the
   Mac reads its status, stops it, reads its output (anomaly reports) and
   unloads it. Then it pins the Mac, whose host has no container adapter, and
   the admission refusal must be chained. The node is stopped with SIGTERM
   (it writes its in-memory chain to the scratch dir) and the stage passes
   only if both sides' evidence agrees (`pi_plan.judge_placement`).
   `--placement-evidence <file>` writes the decision, explain output,
   attempts and both chains' event kinds, refusing to write anything that
   looks like an address or a home path. The committed run is
   `docs/cogs/evidence/placement-mac-pi5-2026-09-29.json`.
8. **Clean up.** It removes `~/weftos-test-pi`, including the root-owned
   harness output (with `sudo -n` when needed), unless you pass `--keep`. The
   removal runs even when a stage fails or the lane aborts (a failed build,
   rsync or fetch, Ctrl-C, SIGTERM or SIGHUP); the abort becomes a FAIL row in
   the summary. From cleanup on, SIGTERM and SIGHUP are recorded instead of
   raised, so cleanup, the guard and the report always finish; such a signal
   still fails the run. SIGKILL cannot be caught, so step 3 of the next run removes
   whatever such a run left.
9. **Guard.** It runs after every run that got past the before-probe, aborted
   or not, and re-reads the Pi operator files, `weaver.service` and the Mac
   chain mtime. If any of them changed, or the Pi can no longer be probed (an
   unknown state is never counted as unchanged), the lane prints `CRITICAL` and
   exits 3. `sessions/` and `kernel.log` are not compared, because the Pi's own
   weaver writes them.

`--report <file>` writes a JSON summary with the glibc versions, builder
image, per-stage counts and the guard result. It holds no host names. The
committed evidence for the card is `docs/cogs/evidence/test-pi-2026-09-29.json`.

## Limits

- Doctests are not run, because the Pi has no `rustc`. Run them on the Mac
  with `scripts/build.sh test`.
- Tests that need services the Pi does not have (docker, Apple `container`, a
  Seed) skip themselves the way they do everywhere else. The container live tests
  stay Mac-side.
- Tests have to leave UDP 5006 and TCP 80 free for the live and conformance
  stages.
