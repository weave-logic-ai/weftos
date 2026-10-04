#!/usr/bin/env python3
"""Run ARM tests on the real Raspberry Pi 5 (scripts/build.sh test-pi).

  scripts/build.sh test-pi [crate ...] [--filter <test>] [--live-native] [--cogs]
                           [--placement]

Cross-builds aarch64-unknown-linux-gnu test binaries in an arm64 Debian
container (OrbStack / Docker; the Pi has no Rust toolchain), copies them and
the git-tracked crate sources over SSH to a scratch dir on the Pi, runs them there
under `env -i` with an isolated HOME and WEFTOS_RUNTIME_DIR, streams the output
back and removes the scratch dir. With no crate and no stage flag it runs the
full lane: clawft-kernel tests, the native adapter live test (anomaly-detect),
the scripts/cogs conformance harness in remote (ssh) mode, and the two-node
placement run (isolated weaver daemons on the Mac and the Pi, pi_placement).

The Pi comes from WEFTOS_PI_HOST ([user@]host, never committed); unset means
the lane is skipped (exit 0). It never touches the Pi's ~/.clawft or its
weaver.service, and checks the chain file mtimes on both ends before and after.
"""
import argparse
import os
import signal
import subprocess
import sys
import tempfile
import threading

import pi_ctl_plan as ctl
import pi_placement as placement
import pi_plan as plan

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
COGS_DIR = os.path.join(ROOT, "scripts", "cogs")
TARGET = os.path.join(ROOT, "target", "pi-aarch64")
SSH_OPTS = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=15"]
LIVE_TEST = "workload_runtime::tests_live::live_native_anomaly_detect"
LIVE_MARK = "interval run:"
NO_MANIFEST_WHY = ("no verified sha256 for cog-anomaly-detect-aarch64; pass --sha256-manifest PATH "
                   "or set WEFTOS_PI_COG_MANIFEST (the lane never runs an unverified download)")
DEFAULT_COGS = "anomaly-detect,fall-detect,sleep-apnea,health-monitor"
CHAIN = ".clawft/chain.rvf"   # Mac side; Pi side: pi_plan.OPERATOR_FILES


class Runner:
    """Runs commands, streaming output; returns (rc, captured text)."""

    def __init__(self, dry_run=False):
        self.dry_run = dry_run

    def __call__(self, cmd, capture="all", timeout=None):
        if self.dry_run:
            print("  DRY   " + " ".join(cmd))
            return 0, ""
        if capture == "stdout":   # stdout captured quietly, stderr streamed
            try:
                p = subprocess.run(cmd, stdout=subprocess.PIPE, text=True, timeout=timeout)
            except subprocess.TimeoutExpired as e:   # clean failure, not a traceback
                out = e.stdout.decode(errors="replace") if isinstance(e.stdout, bytes) else ""
                return -9, out or ""
            return p.returncode, p.stdout
        p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                             text=True, errors="replace")
        timer = threading.Timer(timeout, p.kill) if timeout else None
        if timer:
            timer.start()
        chunks = []
        for line in p.stdout:
            chunks.append(line)
            if capture != "quiet":
                sys.stdout.write(line)
                sys.stdout.flush()
        rc = p.wait()
        if timer:
            timer.cancel()
        return rc, "".join(chunks)

    def spawn(self, cmd, log_path, cwd):
        """Start a long-running local process (its output to `log_path`);
        returns the Popen, or None on a dry run."""
        if self.dry_run:
            print("  DRY   (background) " + " ".join(cmd))
            return None
        with open(log_path, "w") as log:
            return subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT,
                                    stdin=subprocess.DEVNULL, cwd=cwd, start_new_session=True)


def local_chain_mtime():
    try:
        return int(os.stat(os.path.expanduser("~/" + CHAIN)).st_mtime)
    except OSError:
        return None


class Lane:
    def __init__(self, args, host, runner):
        self.a, self.host, self.run = args, host, runner
        self.results = []   # {stage, name, rc, passed, failed, ignored, ok}
        self.facts = {}
        self.skipped = []   # {stage, why}: stages that could not run

    # ── helpers ────────────────────────────────────────────────────────
    def ssh(self, line, capture="all", timeout=None):
        return self.run(["ssh"] + SSH_OPTS + [self.host, line], capture, timeout)

    def record(self, stage, name, rc, text, require_ran=False, marker=None):
        tot = plan.parse_results(text)
        ok = plan.stage_ok(rc, tot, require_ran) and (marker is None or marker in text)
        if self.run.dry_run:
            ok = True
        self.results.append(dict(stage=stage, name=name, rc=rc, ok=ok, **tot))
        print("  %s  %s %s (%d passed, %d failed, %d ignored)" % (
            "PASS" if ok else "FAIL", stage, name, tot["passed"], tot["failed"], tot["ignored"]))
        return ok

    def pi_state(self):
        """Operator files + weaver.service on the Pi, or None if the probe failed."""
        rc, out = self.ssh(plan.pi_state_command(), capture="quiet", timeout=60)
        if self.run.dry_run:
            out = "".join("%s absent\n" % f for f in plan.OPERATOR_FILES)
            out += "weaver dry-run\n%s\n" % plan.STATE_END
        return plan.parse_pi_state(rc, out)

    # ── stages ─────────────────────────────────────────────────────────
    def preflight(self):
        rc, out = self.ssh('uname -m; ldd --version 2>&1 | head -1; printf "%s\\n" "$HOME"',
                           capture="quiet")
        if self.run.dry_run:
            out = "aarch64\nldd (dry-run) 2.41\n/home/pi-user\n"
        lines = out.splitlines()
        if rc != 0 or len(lines) < 3:
            raise SystemExit("test-pi: cannot reach the Pi over ssh (rc %s)" % rc)
        if lines[0].strip() != "aarch64":
            raise SystemExit("test-pi: target is %s, not aarch64" % lines[0].strip())
        home = lines[2].strip()
        if not home.startswith("/") or any(c.isspace() for c in home):
            raise SystemExit("test-pi: unexpected remote $HOME")
        self.scratch = "%s/%s" % (home, self.a.scratch)
        rc, _ = self.ssh("command -v rsync >/dev/null && command -v python3 >/dev/null",
                         capture="quiet")
        if rc != 0:
            raise SystemExit("test-pi: the target needs rsync and python3 "
                             "(sudo apt-get install -y rsync python3)")
        rc, bout = self.run(["docker", "run", "--rm", "--platform", "linux/arm64",
                             self.a.image, "sh", "-c", "ldd --version 2>&1 | head -1"],
                            capture="quiet")
        pi_glibc, b_glibc = plan.glibc_version(lines[1]), plan.glibc_version(bout)
        if not self.run.dry_run and not plan.glibc_compatible(b_glibc, pi_glibc):
            raise SystemExit("test-pi: builder glibc %s is newer than the Pi's %s"
                             % (b_glibc, pi_glibc))
        self.facts.update(arch="aarch64", pi_glibc=pi_glibc, builder_glibc=b_glibc,
                          builder_image=self.a.image)
        print("  INFO  Pi aarch64, glibc %s; builder %s glibc %s" % (
            pi_glibc, self.a.image, b_glibc))

    def build(self, crates, launcher, node=False):
        os.makedirs(os.path.join(TARGET, "cargo-registry"), exist_ok=True)
        src = self.scratch + "/src"
        mk = lambda args: plan.builder_command(self.a.image, ROOT, src, TARGET,
                                                os.path.join(TARGET, "cargo-registry"), args)
        arts = []
        if crates:
            print("── Cross-building test binaries: %s" % " ".join(crates))
            rc, out = self.run(mk(plan.test_cargo_args(crates)), capture="stdout")
            if rc != 0:
                raise SystemExit("test-pi: container build failed (rc %d)" % rc)
            arts = plan.parse_test_artifacts(out.splitlines(), TARGET)
            if not arts and not self.run.dry_run:
                raise SystemExit("test-pi: cargo produced no test binaries")
        if launcher:
            print("── Cross-building cog_adapter_run launcher")
            rc, _ = self.run(mk(plan.launcher_cargo_args()))
            if rc != 0:
                raise SystemExit("test-pi: launcher build failed (rc %d)" % rc)
        if node:
            print("── Cross-building the weaver daemon (placement stage)")
            rc, _ = self.run(mk(plan.weaver_cargo_args()))
            if rc != 0:
                raise SystemExit("test-pi: weaver build failed (rc %d)" % rc)
            placement.build_mac(self.run)
        return arts

    def stage_remote(self, arts, extra_bins):
        s = self.scratch
        rd = self.a.scratch   # a killed earlier run may have left it (root files too)
        rc, _ = self.ssh("%s && mkdir -p %s/bin %s/src %s/home %s/runtime %s/tmp"
                         % ((plan.remove_scratch_command(rd),) + (rd,) * 5), capture="quiet")
        if rc != 0:
            raise SystemExit("test-pi: cannot create the Pi scratch dir")
        files, missing = plan.present_files(ROOT, subprocess.run(
            plan.sync_files_command(ROOT), stdout=subprocess.PIPE, check=True).stdout)
        if missing:
            print("  NOTE  %d tracked file(s) deleted locally are not synced, e.g. %s"
                  % (len(missing), missing[0]))
        rsh = "ssh " + " ".join(SSH_OPTS)
        with tempfile.NamedTemporaryFile(suffix=".lst") as lst:
            lst.write(files)
            lst.flush()
            print("── Syncing tracked sources and %d binaries to the Pi" % (len(arts) + len(extra_bins)))
            rc, _ = self.run(["rsync", "-a", "--from0", "--files-from=" + lst.name, "-e", rsh,
                              ROOT + "/", "%s:%s/src/" % (self.host, s)], capture="quiet")
        bins = [a["local_path"] for a in arts] + list(extra_bins)
        if rc == 0 and bins:
            rc, _ = self.run(["rsync", "-a", "-e", rsh] + bins
                             + ["%s:%s/bin/" % (self.host, s)], capture="quiet")
        if rc != 0:
            raise SystemExit("test-pi: rsync to the Pi failed (rc %d)" % rc)

    def rsync_to(self, paths, remote_dir):
        """Copy local files into a directory under the Pi scratch dir."""
        if not remote_dir.startswith(self.scratch + "/"):
            raise SystemExit("test-pi: refusing to copy outside the scratch dir")
        rsh = "ssh " + " ".join(SSH_OPTS)
        rc, _ = self.run(["rsync", "-a", "-e", rsh] + list(paths)
                         + ["%s:%s" % (self.host, remote_dir)], capture="quiet")
        if rc != 0:
            raise SystemExit("test-pi: rsync to the Pi failed (rc %d)" % rc)

    def remote_bin(self, art):
        return "%s/bin/%s" % (self.scratch, os.path.basename(art["container_path"]))

    def run_tests(self, arts, crates):
        test_args = [self.a.filter] if self.a.filter else []
        for art in (a for a in arts if a["crate"] in crates):
            name = "%s[%s:%s]" % (art["crate"], art["kind"], art["name"])
            print("── Running %s on the Pi" % name)
            line = plan.remote_test_command(self.scratch, self.remote_bin(art),
                                            art["manifest_dir"], test_args,
                                            timeout=self.a.timeout)
            rc, out = self.ssh(line, timeout=self.a.timeout + 30)
            self.record("test", name, rc, out)
        if self.a.filter and not self.run.dry_run and not plan.filter_matched(self.results):
            self.results.append(dict(stage="test", name="--filter %s" % self.a.filter, rc=1,
                                     ok=False, passed=0, failed=0, ignored=0, suites=0))
            print("  FAIL  test --filter %s matched no test on the Pi" % self.a.filter)

    def run_live_native(self, arts):
        lib = next((a for a in arts if a["crate"] == "clawft-kernel" and a["kind"] == "lib"), None)
        if lib is None and self.run.dry_run:
            lib = {"container_path": "/target/debug/deps/clawft_kernel-<hash>",
                   "manifest_dir": self.scratch + "/src/crates/clawft-kernel"}
        if lib is None:
            raise SystemExit("test-pi: clawft-kernel lib test binary missing")
        print("── Native adapter live test (anomaly-detect) on the Pi")
        env = {"WEFTOS_NATIVE_LIVE": "1",
               "WEFTOS_COG_AARCH64_BIN": "%s/bin/%s" % (self.scratch, self.cog_bin_name)}
        line = plan.remote_test_command(self.scratch, self.remote_bin(lib), lib["manifest_dir"],
                                        [LIVE_TEST, "--exact", "--nocapture"], env,
                                        timeout=self.a.timeout)
        rc, out = self.ssh(line, timeout=self.a.timeout + 30)
        self.record("live-native", "anomaly-detect", rc, out, require_ran=True, marker=LIVE_MARK)

    def run_cogs(self):
        launcher = os.path.join(TARGET, "debug", "examples", "cog_adapter_run")
        if not self.a.sha256_manifest:
            return self.skip("cogs", NO_MANIFEST_WHY)
        base = [sys.executable, os.path.join(COGS_DIR, "conformance.py"), "sweep",
                "--runtime", "ssh", "--ssh-host", self.host, "--sudo", "--arch", "aarch64",
                "--mode", "expected", "--cogs", self.a.cogs_ids,
                "--sha256-manifest", self.a.sha256_manifest]
        sweeps = [
            ("harness", ["--remote-dir", self.a.scratch + "/cogs-harness",
                         "--label", "pi5-ssh-aarch64-expected"]),
            ("adapter-native", ["--remote-dir", self.a.scratch + "/cogs-adapter",
                                "--launcher", launcher, "--adapter-runtime", "native",
                                "--adapter-run-as", "65534:65534",
                                "--label", "pi5-adapter-native-aarch64-expected"]),
        ]
        for name, extra in sweeps:
            print("── Cog conformance on the Pi (%s)" % name)
            rc, out = self.run(base + extra, timeout=self.a.timeout)
            ok = rc == 0 or self.run.dry_run
            self.results.append(dict(stage="cogs", name=name, rc=rc, ok=ok, passed=0,
                                     failed=0 if ok else 1, ignored=0, suites=1))
            print("  %s  cogs %s (rc %d)" % ("PASS" if ok else "FAIL", name, rc))

    def skip(self, stage, why):
        """A stage that cannot run is reported, never silently dropped or crashed."""
        self.skipped.append(dict(stage=stage, why=why))
        print("  SKIP  %s: %s" % (stage, why))

    def fetch_cog(self):
        """The verified anomaly-detect binary, or None (the cog stages then skip)."""
        sys.path.insert(0, COGS_DIR)
        import runtimes  # scripts/cogs: cached, ELF-checked released binaries
        self.cog_bin_name = runtimes.binary_name("anomaly-detect", "aarch64")
        if self.run.dry_run:
            return os.path.join(COGS_DIR, ".cache", "aarch64", self.cog_bin_name)
        expected = None
        if self.a.sha256_manifest:
            try:
                expected = runtimes.load_hash_manifest(self.a.sha256_manifest).get(self.cog_bin_name)
            except ValueError as e:
                raise SystemExit("test-pi: %s" % e)
        if expected is None:
            return None
        path, why = runtimes.fetch_binary("anomaly-detect", "aarch64",
                                          os.path.join(COGS_DIR, ".cache", "aarch64"),
                                          expected_sha256=expected)
        if path is None:
            raise SystemExit("test-pi: anomaly-detect aarch64 binary: %s" % why)
        return path

    def cleanup(self):
        placement.cleanup(self)
        if self.a.keep:
            print("  INFO  --keep: leaving %s on the Pi" % self.a.scratch)
            return
        rc, _ = self.ssh(plan.remove_scratch_command(self.a.scratch), capture="quiet",
                         timeout=120)
        if rc != 0 and not self.run.dry_run:
            print("  WARN  could not remove ~/%s on the Pi (rc %s); the next run removes it"
                  % (self.a.scratch, rc))

    def stages(self):
        a = self.a
        self.preflight()
        build = list(a.crates) + (["clawft-kernel"] if a.live_native else [])
        arts = self.build(list(dict.fromkeys(build)), launcher=a.cogs, node=a.placement)
        cog = self.fetch_cog() if (a.live_native or a.placement) else None
        if cog is None and (a.live_native or a.placement):
            for stage, wanted in (("live-native", a.live_native), ("placement", a.placement)):
                if wanted:
                    self.skip(stage, NO_MANIFEST_WHY)
        live_native = a.live_native and cog is not None
        extra = [cog] if live_native else []
        if a.placement and cog is not None:   # only the weaver daemon goes; the cog travels over the mesh
            extra.append(placement.pi_binary(TARGET))
        self.stage_remote(arts, extra)
        if a.crates:
            self.run_tests(arts, a.crates)
        if live_native:
            self.run_live_native(arts)
        if a.cogs:
            self.run_cogs()
        if a.placement and cog is not None:
            placement.run_placement(self, cog)

    def abort(self, why):
        """A stage stopped the lane: it is a FAIL row, and the guard still runs."""
        self.results.append(dict(stage="abort", name=why, rc=1, ok=False, passed=0,
                                 failed=1, ignored=0, suites=0))
        print("  FAIL  lane aborted: %s" % why)


LANE_SIGNALS = (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)


def _raise_on_signal(signum, _frame):
    raise SystemExit("test-pi: interrupted by signal %d" % signum)


def _defer_signals(deferred):
    """From cleanup on, a signal is recorded, not raised, so cleanup, the
    after-probe, the guard and the report always finish."""
    for sig in LANE_SIGNALS:
        signal.signal(sig, lambda signum, _f: deferred.append(signum))


def parse_args(argv):
    ap = argparse.ArgumentParser(prog="scripts/build.sh test-pi", description=__doc__.split("\n")[0])
    ap.add_argument("crates", nargs="*", help="crates whose tests run on the Pi")
    ap.add_argument("--filter", help="libtest name filter passed to every test binary")
    ap.add_argument("--live-native", action="store_true",
                    help="native adapter live test (anomaly-detect) on the Pi")
    ap.add_argument("--cogs", action="store_true",
                    help="scripts/cogs conformance in remote mode (harness + native adapter)")
    ap.add_argument("--placement", action="store_true",
                    help="two-node placement: Mac weaver daemon + CLI -> isolated weaver daemon on the Pi")
    ap.add_argument("--placement-evidence", help="write the placement evidence JSON here")
    ap.add_argument("--mac-container", metavar="IMAGE@sha256:DIGEST",
                    help="placement: the Mac daemon also serves a Docker adapter with this "
                         "local, operator-pinned base image")
    ap.add_argument("--full", action="store_true",
                    help="clawft-kernel + --live-native + --cogs + --placement (default, no args)")
    ap.add_argument("--sha256-manifest", metavar="PATH",
                    default=os.environ.get("WEFTOS_PI_COG_MANIFEST") or None,
                    help="JSON map of cog-<id>-aarch64 to the expected sha256 (env "
                         "WEFTOS_PI_COG_MANIFEST). Without it the live-native, cogs and "
                         "placement stages skip; downloads are never unverified")
    ap.add_argument("--cogs-ids", default=DEFAULT_COGS, help="cogs for --cogs (comma-separated)")
    ap.add_argument("--image", help="arm64 builder image (default rust:<toolchain>-bookworm)")
    ap.add_argument("--scratch", default="weftos-test-pi",
                    help="scratch dir relative to the Pi login dir (removed at the end)")
    ap.add_argument("--timeout", type=int, default=3600, help="per-stage cap in seconds")
    ap.add_argument("--keep", action="store_true", help="leave the Pi scratch dir in place")
    ap.add_argument("--report", help="write a JSON summary here (no host names)")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args(argv)
    if not (a.crates or a.live_native or a.cogs or a.placement) or a.full:
        a.crates = list(dict.fromkeys(a.crates + ["clawft-kernel"]))
        a.live_native = a.cogs = a.placement = True
    for c in a.crates:
        if not plan.valid_crate(c):
            ap.error("invalid crate name %r" % c)
    if a.filter is not None and not plan.valid_filter(a.filter):
        ap.error("--filter must match [A-Za-z0-9_:.-]+")
    if a.mac_container and not ctl.IMAGE_RE.match(a.mac_container):
        ap.error("--mac-container must be name@sha256:<64 hex>")
    if not plan.valid_scratch(a.scratch):
        ap.error("--scratch must be a relative path of [A-Za-z0-9._-] parts")
    if not all(plan.valid_crate(c) for c in a.cogs_ids.split(",")):
        ap.error("--cogs-ids must be comma-separated cog ids")
    if not a.image:
        with open(os.path.join(ROOT, "rust-toolchain.toml")) as f:
            a.image = "rust:%s-bookworm" % (plan.toolchain_channel(f.read()) or "1")
    return a


def main(argv=None):
    a = parse_args(sys.argv[1:] if argv is None else argv)
    host = os.environ.get("WEFTOS_PI_HOST", "").strip()
    if not host:
        print("  SKIP  test-pi: WEFTOS_PI_HOST is not set ([user@]host of the Pi 5); "
              "nothing ran")
        return 0
    if not plan.valid_host(host):
        raise SystemExit("test-pi: WEFTOS_PI_HOST must be a plain [user@]host")
    lane = Lane(a, host, Runner(a.dry_run))
    lane.rev = plan.source_rev(ROOT)
    mac_before = local_chain_mtime()
    pi_before = lane.pi_state()
    if pi_before is None:
        raise SystemExit("test-pi: cannot read the Pi's operator state; not running "
                         "without a before-snapshot")
    print("── test-pi: crates=%s live-native=%s cogs=%s placement=%s filter=%s" % (
        ",".join(a.crates) or "-", a.live_native, a.cogs, a.placement, a.filter or "-"))
    # SIGTERM/SIGHUP/SIGINT become SystemExit so cleanup and the guard still
    # run, and from cleanup on they are deferred (SIGKILL cannot be caught; the
    # next run removes the stale scratch dir).
    deferred = []
    for sig in LANE_SIGNALS:
        signal.signal(sig, _raise_on_signal)
    try:
        lane.stages()
    except (SystemExit, KeyboardInterrupt) as e:
        code = getattr(e, "code", None)
        lane.abort(code if isinstance(code, str) else "interrupted (%s)" % type(e).__name__)
    finally:
        _defer_signals(deferred)
        lane.cleanup()
    pi_after, mac_after = lane.pi_state(), local_chain_mtime()
    if deferred:
        lane.abort("signal %d during cleanup; cleanup and guard completed" % deferred[0])
    guard = plan.operator_guard(pi_before, pi_after, mac_before, mac_after)
    rc = 0 if lane.results and all(r["ok"] for r in lane.results) else 1
    print("\n── test-pi summary")
    for r in lane.results:
        print("  %s  %-12s %s" % ("PASS" if r["ok"] else "FAIL", r["stage"], r["name"]))
    print("  INFO  operator data: %s" % guard)
    if not guard["ok"] and not a.dry_run:
        print("  CRITICAL  %s" % ("operator files or weaver.service changed during the run"
                                  if guard["pi_probe_ok"] else
                                  "could not re-read the Pi's operator state; unverified"))
        rc = 3
    if a.report:
        import json
        with open(a.report, "w") as f:
            json.dump({"source": lane.rev, "facts": lane.facts, "guard": guard, "results": lane.results,
                       "skipped": lane.skipped, "crates": a.crates,
                       "filter": a.filter, "ok": rc == 0}, f, indent=2)
            f.write("\n")
    return rc


if __name__ == "__main__":
    sys.exit(main())
