# Real Pi 5 test lane (`scripts/build.sh test-pi`)

Card mesh-placement-fu-pi-test-lane. Decision (2026-09-29): anything that has to
prove it runs on ARM is tested on the real Raspberry Pi 5, not on the Mac or in a
container standing in for it. Governing design:
[ADR-099](../adr/adr-099-governed-workload-placement.md),
[ADR-100](../adr/adr-100-cog-workload-kind.md); the harness it drives is
described in [conformance-harness.md](conformance-harness.md).

```bash
export WEFTOS_PI_HOST=pi5                              # [user@]host or ~/.ssh/config alias
scripts/build.sh test-pi                               # full lane (see below)
scripts/build.sh test-pi clawft-kernel                 # one crate's tests on the Pi
scripts/build.sh test-pi clawft-kernel --filter chain  # libtest name filter
scripts/build.sh test-pi --live-native                 # native adapter live test only
scripts/build.sh test-pi --cogs                        # cog conformance (ssh mode) only
scripts/build.sh test-pi --placement                   # two-node placement, Mac -> Pi (card 12)
scripts/build.sh test-pi --placement --mac-container debian@sha256:<digest>  # plus a Docker adapter on the Mac
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
7. **Two-node placement (`--placement`, card mesh-placement-12).** Both
   nodes are real, isolated `weaver` daemons, and the Mac side is driven only
   through the operator CLI. The lane cross-builds `weaver` for the Pi and
   builds it natively for the Mac. It generates a throwaway node key with
   `weaver workload keygen` (the Mac daemon's `node.key` and the package
   signer) and, with `workload_node daemon-files`, the Pi daemon's policy:
   `workload-host.json` (serve `workload-host` on **9471** with Noise XX to
   that controller only), `workload-trust.json` and `workload-permits.json`.
   These go into `~/weftos-test-pi/runtime`, the isolated daemon's
   `WEFTOS_RUNTIME_DIR`; it runs under `env -i` with its own HOME, chain and
   socket, so the system weaver keeps `:9470` and `~/.clawft`. A synthetic
   ADR-069 feed (`scripts/pi/csi_feed.py`) runs on `127.0.0.1:15006`. The
   Mac daemon runs under `env -i` in a temp dir (own HOME, runtime dir, key
   and chain; mesh transport off) with the Pi in `workload-peers.json` as
   `paired`. Once it has learned the Pi, the lane binds that tier to the
   Pi's node key (`"key"` in the peers entry) and checks it stays paired.
   Then, through `weaver workload`:
   - `explain` must choose the Pi (`aarch64-native`, tier `native`) over
     the Mac;
   - `place` runs the released anomaly-detect cog on the Pi (fetched from
     the Mac over the placement connection); the lane reads its status,
     stops it, counts its anomaly reports and unloads it;
   - a package whose `aarch64` binary is really x86-64 must be refused by
     the Pi adapter's own admission self-check and retried on the next
     candidate;
   - `place --pin <Mac>`.

   With `--mac-container IMAGE@sha256:DIGEST` (a local, pinned base image)
   the Mac daemon also serves a Docker adapter (`workload-container.json`;
   the daemon gets the current Docker context's endpoint): the mislabeled
   package is then refused by the Mac's container adapter too, and the pin
   runs the cog in a container on the Mac. Without it, the Mac serves only
   its native adapter, so the controller never offers it a container
   variant: nothing is dispatched to it and the pin is refused by the
   engine. The Pi daemon's chain is exported (`weaver chain export`) before
   the daemon and the feed are stopped. The stage passes only if both
   sides' evidence agrees (`pi_ctl_plan.judge`).
   `--placement-evidence <file>` writes the decision, explain output,
   attempts and the placement-related event kinds of both chains, and
   refuses to write anything that looks like an address or a home path.
   Committed runs: `docs/cogs/evidence/placement-mac-pi5-weaver-container-2026-09-29.json`
   (with the Mac's Docker adapter) and
   `placement-mac-pi5-weaver-native-only-2026-09-29.json` (without). Earlier
   runs: `placement-mac-pi5-weaver-2026-09-30.json` and
   `placement-mac-pi5-2026-09-29.json`.
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
