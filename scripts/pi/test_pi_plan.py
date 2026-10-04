"""Unit tests for the Pi test lane: python3 -m unittest discover scripts/pi"""
import io
import json
import os
import shlex
import sys
import unittest
from contextlib import redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pi_lane  # noqa: E402
import pi_plan as plan  # noqa: E402

KERNEL = "/home/u/weftos-test-pi/src/crates/clawft-kernel"


def artifact(kind, name, exe, test=True, pkg="path+file://%s#0.8.1" % KERNEL):
    return json.dumps({"reason": "compiler-artifact", "package_id": pkg,
                       "manifest_path": KERNEL + "/Cargo.toml",
                       "target": {"kind": [kind], "name": name},
                       "profile": {"test": test}, "executable": exe})


class Validation(unittest.TestCase):
    def test_host(self):
        for ok in ["pi5", "user@pi5", "user@10.0.0.2", "pi.local"]:
            self.assertTrue(plan.valid_host(ok), ok)
        for bad in ["", "-oProxyCommand=x", "a b", "u@h;rm", "h$(x)", "u@@h"]:
            self.assertFalse(plan.valid_host(bad), bad)

    def test_scratch_stays_inside_home(self):
        self.assertTrue(plan.valid_scratch("weftos-test-pi"))
        self.assertTrue(plan.valid_scratch("weftos-test-pi/cogs"))
        for bad in ["", "/tmp/x", "..", "a/../b", "a//b", ".", "~/x", "a b", "a;b"]:
            self.assertFalse(plan.valid_scratch(bad), bad)

    def test_crate_and_filter(self):
        self.assertTrue(plan.valid_crate("clawft-kernel"))
        self.assertFalse(plan.valid_crate("--all"))
        self.assertTrue(plan.valid_filter("workload_runtime::tests_live"))
        self.assertFalse(plan.valid_filter("x; rm -rf ~"))

    def test_glibc_gate(self):
        pi = plan.glibc_version("ldd (Debian GLIBC 2.41-12+rpt1) 2.41")
        self.assertEqual(pi, (2, 41))
        self.assertTrue(plan.glibc_compatible((2, 36), pi))
        self.assertFalse(plan.glibc_compatible((2, 42), pi))
        self.assertFalse(plan.glibc_compatible(None, pi))
        self.assertIsNone(plan.glibc_version(""))

    def test_toolchain_image(self):
        self.assertEqual(plan.toolchain_channel('[toolchain]\nchannel = "1.93"\n'), "1.93")


class Artifacts(unittest.TestCase):
    def test_only_test_executables_are_kept_and_mapped(self):
        lines = [
            "noise", "{bad json",
            artifact("lib", "clawft_kernel", "/target/debug/deps/clawft_kernel-1"),
            artifact("test", "e2e", "/target/debug/deps/e2e-2"),
            artifact("test", "e2e", "/target/debug/deps/e2e-2"),          # duplicate
            artifact("bin", "weaver", "/target/debug/weaver", test=False),  # not a test
            artifact("lib", "x", None),                                    # no executable
        ]
        arts = plan.parse_test_artifacts(lines, "/local/target/pi-aarch64/")
        self.assertEqual([(a["kind"], a["name"]) for a in arts],
                         [("lib", "clawft_kernel"), ("test", "e2e")])
        self.assertEqual(arts[0]["local_path"], "/local/target/pi-aarch64/debug/deps/clawft_kernel-1")
        self.assertEqual(arts[0]["crate"], "clawft-kernel")
        self.assertEqual(arts[0]["manifest_dir"], KERNEL)

    def test_package_name_forms(self):
        self.assertEqual(plan.package_name("path+file:///a/clawft-kernel#0.8.1"), "clawft-kernel")
        self.assertEqual(plan.package_name("path+file:///a/dir#weftos@0.8.1"), "weftos")

    def test_builder_mounts_source_at_pi_path(self):
        cmd = plan.builder_command("rust:1.93-bookworm", "/repo", "/home/u/s/src", "/repo/t",
                                   "/repo/t/reg", plan.test_cargo_args(["a", "b"]))
        self.assertIn("/repo:/home/u/s/src:ro", cmd)
        self.assertEqual(cmd[cmd.index("-w") + 1], "/home/u/s/src")
        self.assertEqual(cmd[cmd.index("--platform") + 1], "linux/arm64")
        self.assertEqual(cmd[-4:], ["-p", "a", "-p", "b"])


class RemoteCommand(unittest.TestCase):
    def test_env_is_isolated(self):
        line = plan.remote_test_command("/home/u/s", "/home/u/s/bin/t-1", KERNEL, ["flt"],
                                        {"WEFTOS_NATIVE_LIVE": "1"})
        self.assertTrue(line.startswith("cd %s && exec env -i " % KERNEL))
        tokens = shlex.split(line)
        for kv in ["HOME=/home/u/s/home", "WEFTOS_RUNTIME_DIR=/home/u/s/runtime",
                   "TMPDIR=/home/u/s/tmp", "CARGO_MANIFEST_DIR=" + KERNEL,
                   "WEFTOS_NATIVE_LIVE=1", "PATH=" + plan.PATH_ENV,
                   "INSTA_WORKSPACE_ROOT=/home/u/s/src", "INSTA_UPDATE=no"]:
            self.assertIn(kv, tokens)
        self.assertTrue(line.endswith("/home/u/s/bin/t-1 flt"))
        self.assertNotIn(".clawft", line)


class Results(unittest.TestCase):
    OK = "test result: ok. 12 passed; 0 failed; 3 ignored; 0 measured\n"
    BAD = "test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured\n"
    NONE = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 90 filtered out\n"

    def test_parse_and_judge(self):
        self.assertEqual(plan.parse_results(self.OK + self.BAD),
                         {"passed": 23, "failed": 1, "ignored": 3, "suites": 2})
        self.assertTrue(plan.stage_ok(0, plan.parse_results(self.OK)))
        self.assertFalse(plan.stage_ok(101, plan.parse_results(self.BAD)))
        self.assertFalse(plan.stage_ok(0, plan.parse_results("")))      # never reported
        self.assertTrue(plan.stage_ok(0, plan.parse_results(self.NONE)))
        self.assertFalse(plan.stage_ok(0, plan.parse_results(self.NONE), require_ran=True))


def pi_state_out(chain_mtime, weaver="active pid=812 since=4200"):
    rows = ["%s 32 5" % f for f in plan.OPERATOR_FILES if f != ".clawft/chain.rvf"]
    rows.append(".clawft/chain.rvf 468733 %s" % chain_mtime)
    return "\n".join(rows + ["weaver " + weaver, plan.STATE_END]) + "\n"


class Guard(unittest.TestCase):
    def test_probe_parsing_rejects_failed_or_truncated_output(self):
        full = pi_state_out("100")
        st = plan.parse_pi_state(0, full)
        self.assertEqual(st[".clawft/chain.rvf"], "468733 100")
        self.assertEqual(st["weaver"], "active pid=812 since=4200")
        self.assertIsNone(plan.parse_pi_state(255, full))
        self.assertIsNone(plan.parse_pi_state(0, ""))
        self.assertIsNone(plan.parse_pi_state(0, full.replace(plan.STATE_END, "")))
        dropped = "\n".join(l for l in full.splitlines() if "chain.key" not in l)
        self.assertIsNone(plan.parse_pi_state(0, dropped))

    def test_unknown_state_is_not_unchanged(self):
        st = plan.parse_pi_state(0, pi_state_out("100"))
        self.assertTrue(plan.operator_guard(st, dict(st), 7, 7)["ok"])
        for before, after in [(None, None), (st, None), (None, st)]:
            g = plan.operator_guard(before, after, 7, 7)
            self.assertFalse(g["ok"])
            self.assertFalse(g["pi_probe_ok"])

    def test_any_operator_file_change_is_caught(self):
        st = plan.parse_pi_state(0, pi_state_out("100"))
        for f in plan.OPERATOR_FILES:
            after = dict(st, **{f: "1 999"})
            g = plan.operator_guard(st, after, 7, 7)
            self.assertFalse(g["ok"], f)
            self.assertEqual(g["pi_changed"], [f])
        self.assertFalse(plan.operator_guard(st, dict(st, weaver="inactive"), 7, 7)["ok"])
        self.assertFalse(plan.operator_guard(st, dict(st), 7, 8)["ok"])

    def test_filter_must_match_something(self):
        sel = lambda p, f, i, st="test": {"stage": st, "passed": p, "failed": f, "ignored": i}
        r = lambda n: sel(n, 0, 0)
        self.assertFalse(plan.filter_matched([r(0), r(0)]))
        self.assertFalse(plan.filter_matched([sel(1, 0, 0, "live-native")]))
        self.assertTrue(plan.filter_matched([r(0), r(3)]))
        self.assertTrue(plan.filter_matched([sel(0, 0, 2)]))   # only #[ignore] matched
        self.assertTrue(plan.filter_matched([sel(0, 1, 0)]))   # matched, all failed

    def test_scratch_removal_is_bounded_and_sudo_backed(self):
        line = plan.remove_scratch_command("weftos-test-pi")
        self.assertIn("sudo -n rm -rf weftos-test-pi", line)
        self.assertTrue(line.startswith("cd && ") and line.endswith("test ! -e weftos-test-pi"))
        for bad in ("../x", "/etc", "", "a/../b"):
            with self.assertRaises(ValueError):
                plan.remove_scratch_command(bad)

    def test_sync_list_is_tracked_files_only(self):
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            git = lambda *a: subprocess.run(["git", "-C", d] + list(a), check=True,
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            git("init", "-q")
            os.makedirs(os.path.join(d, "crates", "x"))
            for f in ("crates/x/lib.rs", "crates/x/local-notes.txt"):
                open(os.path.join(d, f), "w").close()
            git("add", "crates/x/lib.rs")
            out = subprocess.run(plan.sync_files_command(d), stdout=subprocess.PIPE,
                                 check=True).stdout.split(b"\0")
        self.assertIn(b"crates/x/lib.rs", out)
        self.assertNotIn(b"crates/x/local-notes.txt", out)

    def test_weaver_restart_during_run_is_a_change(self):
        line = plan.pi_state_command()
        self.assertIn("-p MainPID", line)
        self.assertIn("-p ActiveEnterTimestampMonotonic", line)
        before = plan.parse_pi_state(0, pi_state_out("100"))
        after = plan.parse_pi_state(0, pi_state_out("100", "active pid=913 since=9900"))
        g = plan.operator_guard(before, after, 7, 7)
        self.assertFalse(g["pi_weaver_unchanged"])
        self.assertFalse(g["ok"])
        self.assertTrue(plan.operator_guard(before, dict(before), 7, 7)["ok"])

    def test_locally_deleted_tracked_file_does_not_break_rsync(self):
        import shutil
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            src, dst = os.path.join(d, "src"), os.path.join(d, "dst")
            os.makedirs(os.path.join(src, "crates", "x"))
            git = lambda *a: subprocess.run(["git", "-C", src] + list(a), check=True,
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            git("init", "-q")
            for f in ("crates/x/lib.rs", "crates/x/gone.rs"):
                open(os.path.join(src, f), "w").close()
            git("add", "crates")
            os.remove(os.path.join(src, "crates/x/gone.rs"))   # deleted, not committed
            listing = subprocess.run(plan.sync_files_command(src), stdout=subprocess.PIPE,
                                     check=True).stdout
            kept, missing = plan.present_files(src, listing)
            self.assertEqual(missing, ["crates/x/gone.rs"])
            lst = os.path.join(d, "files.lst")
            if shutil.which("rsync") is None:
                self.skipTest("rsync not installed")
            for data, want in ((kept, 0), (listing, 23)):   # 23: the unfiltered list fails
                with open(lst, "wb") as f:
                    f.write(data)
                rc = subprocess.run(["rsync", "-a", "--from0", "--files-from=" + lst,
                                     src + "/", dst + "/"], stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE).returncode
                self.assertEqual(rc, want)
            self.assertTrue(os.path.exists(os.path.join(dst, "crates/x/lib.rs")))

    def test_remote_timeout_runs_on_the_pi(self):
        line = plan.remote_test_command("/h/s", "/h/s/bin/t", "/h/s/src/c", [], timeout=60)
        self.assertIn("exec timeout -k 10 60 env -i ", line)
        self.assertNotIn("timeout", plan.remote_test_command("/h/s", "/h/s/bin/t", "/h/c", []))


class RunnerTimeout(unittest.TestCase):
    def test_captured_command_timeout_is_a_clean_failure(self):
        rc, _ = pi_lane.Runner()(["sleep", "5"], capture="stdout", timeout=0.3)
        self.assertEqual(rc, -9)


class BuildShWiring(unittest.TestCase):
    """scripts/build.sh itself, under the Mac's /bin/bash 3.2 + set -u."""

    def test_bare_test_pi_skips_cleanly_without_host(self):
        import subprocess
        root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        env = {k: v for k, v in os.environ.items() if k != "WEFTOS_PI_HOST"}
        p = subprocess.run(["/bin/bash", os.path.join(root, "scripts", "build.sh"), "test-pi"],
                           env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                           text=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stdout)
        self.assertIn("SKIP", p.stdout)
        self.assertNotIn("unbound variable", p.stdout)


class FakeProc:
    """A background process that stops when asked."""

    def __init__(self):
        self.stopped = False

    def terminate(self):
        self.stopped = True

    def kill(self):
        self.stopped = True

    def wait(self, timeout=None):
        return 0


class FakeRunner:
    """Scripted Pi: ssh/docker/rsync calls answered from `script`."""

    def __init__(self, fail_on=None, chain=("100", "100"), output=None, hook=None):
        self.dry_run, self.calls, self.fail_on = False, [], fail_on
        self.chain, self.output, self.hook = list(chain), output, hook
        self.spawned = []

    def spawn(self, cmd, log_path, cwd):
        self.spawned.append(cmd)
        return FakeProc()

    def __call__(self, cmd, capture="all", timeout=None):
        self.calls.append(cmd)
        line = " ".join(cmd)
        if self.hook and self.hook(line) is not None:
            return self.hook(line)
        if plan.STATE_END in line:
            mt = self.chain.pop(0)
            if mt is None:                       # probe failed (Pi dropped off)
                return 255, ""
            return 0, pi_state_out(mt)
        if "uname -m" in line:
            return 0, "aarch64\nldd (Debian GLIBC 2.41-12) 2.41\n/home/u\n"
        if "ldd --version" in line:
            return 0, "ldd (Debian GLIBC 2.36-9) 2.36\n"
        if "--no-run" in line:
            return 0, artifact("test", "e2e", "/target/debug/deps/e2e-2") + "\n"
        if " env -i " in line:
            if self.fail_on and self.fail_on in line:
                return 101, Results.BAD
            return 0, self.output or Results.OK
        return 0, ""


class LaneBehaviour(unittest.TestCase):
    def run_main(self, argv, runner, env=None):
        env = {"WEFTOS_PI_HOST": "u@pi5"} if env is None else env
        out = io.StringIO()
        with mock.patch.dict(os.environ, env, clear=False), \
                mock.patch.object(pi_lane, "Runner", lambda dry: runner), \
                mock.patch.object(pi_lane, "local_chain_mtime", return_value=7), \
                mock.patch.object(pi_lane.subprocess, "run") as git, \
                redirect_stdout(out):
            git.return_value.stdout = b"Cargo.toml\0"
            if "WEFTOS_PI_HOST" not in env:
                os.environ.pop("WEFTOS_PI_HOST", None)
            rc = pi_lane.main(argv)
        return rc, out.getvalue()

    def test_skips_without_host(self):
        runner = FakeRunner()
        rc, out = self.run_main(["clawft-kernel"], runner, env={})
        self.assertEqual(rc, 0)
        self.assertIn("SKIP", out)
        self.assertEqual(runner.calls, [])

    def test_green_run_cleans_up_scratch(self):
        runner = FakeRunner()
        rc, out = self.run_main(["clawft-kernel", "--filter", "chain"], runner)
        self.assertEqual(rc, 0, out)
        test_calls = [c for c in runner.calls if " env -i " in " ".join(c)]
        self.assertEqual(len(test_calls), 1)
        self.assertTrue(test_calls[0][-1].endswith("/home/u/weftos-test-pi/bin/e2e-2 chain"))
        self.assertIn("rm -rf weftos-test-pi", " ".join(runner.calls[-2]))

    def test_cog_stages_skip_without_a_verified_hash(self):
        runner = FakeRunner()
        with mock.patch.dict(os.environ, {"WEFTOS_PI_COG_MANIFEST": ""}):
            rc, out = self.run_main(["clawft-kernel", "--live-native", "--cogs", "--placement"],
                                    runner)
        self.assertEqual(rc, 0, out)
        for stage in ("live-native", "cogs", "placement"):
            self.assertIn("SKIP  %s: no verified sha256" % stage, out)
        self.assertNotIn("Traceback", out)
        self.assertEqual(len([c for c in runner.calls if " env -i " in " ".join(c)]), 1)
        self.assertFalse(any("conformance.py" in " ".join(c) for c in runner.calls))

    def test_manifest_flag_and_env_reach_the_parser(self):
        with mock.patch.dict(os.environ, {"WEFTOS_PI_COG_MANIFEST": "/m/env.json"}):
            self.assertEqual(pi_lane.parse_args(["--cogs"]).sha256_manifest, "/m/env.json")
            a = pi_lane.parse_args(["--cogs", "--sha256-manifest", "/m/flag.json"])
            self.assertEqual(a.sha256_manifest, "/m/flag.json")

    def test_failing_test_fails_the_lane_and_still_cleans_up(self):
        runner = FakeRunner(fail_on="e2e-2")
        rc, out = self.run_main(["clawft-kernel"], runner)
        self.assertEqual(rc, 1)
        self.assertIn("FAIL", out)
        self.assertIn("rm -rf weftos-test-pi", " ".join(runner.calls[-2]))

    def test_chain_change_is_critical(self):
        runner = FakeRunner(chain=("100", "101"))
        rc, out = self.run_main(["clawft-kernel"], runner)
        self.assertEqual(rc, 3)
        self.assertIn("CRITICAL", out)

    def test_failed_after_probe_is_critical_not_unchanged(self):
        runner = FakeRunner(chain=("100", None))
        rc, out = self.run_main(["clawft-kernel"], runner)
        self.assertEqual(rc, 3)
        self.assertIn("CRITICAL", out)

    def test_failed_before_probe_refuses_to_run(self):
        runner = FakeRunner(chain=(None,))
        with self.assertRaises(SystemExit):
            self.run_main(["clawft-kernel"], runner)
        self.assertFalse(any(" env -i " in " ".join(c) for c in runner.calls))

    def test_filter_matching_nothing_fails_the_lane(self):
        runner = FakeRunner(output=Results.NONE)
        rc, out = self.run_main(["clawft-kernel", "--filter", "typo_name"], runner)
        self.assertEqual(rc, 1, out)
        self.assertIn("matched no test", out)

    def probes(self, runner):
        return sum(plan.STATE_END in " ".join(c) for c in runner.calls)

    def test_abort_after_touching_the_pi_still_cleans_up_and_guards(self):
        runner = FakeRunner(hook=lambda l: (23, "") if l.startswith("rsync") else None)
        rc, out = self.run_main(["clawft-kernel"], runner)
        self.assertEqual(rc, 1, out)
        self.assertIn("lane aborted: test-pi: rsync to the Pi failed", out)
        self.assertEqual(self.probes(runner), 2)          # before + after
        self.assertIn("INFO  operator data:", out)
        self.assertIn("sudo -n rm -rf weftos-test-pi", " ".join(runner.calls[-2]))

    def test_abort_with_changed_operator_state_is_critical(self):
        runner = FakeRunner(chain=("100", "101"),
                            hook=lambda l: (23, "") if l.startswith("rsync") else None)
        rc, out = self.run_main(["clawft-kernel"], runner)
        self.assertEqual(rc, 3, out)
        self.assertIn("CRITICAL", out)

    def test_sigterm_mid_test_cleans_up_and_guards(self):
        import signal

        def hook(line):
            if " env -i " in line:
                os.kill(os.getpid(), signal.SIGTERM)
            return None
        saved = {s: signal.getsignal(s) for s in (signal.SIGTERM, signal.SIGHUP)}
        try:
            rc, out = self.run_main(["clawft-kernel"], FakeRunner(hook=hook))
        finally:
            for s, h in saved.items():
                signal.signal(s, h)
        self.assertEqual(rc, 1, out)
        self.assertIn("interrupted by signal %d" % signal.SIGTERM, out)
        self.assertIn("INFO  operator data:", out)

    def test_sigterm_during_cleanup_still_guards_and_reports(self):
        import signal
        import tempfile

        def hook(line):
            if "rm -rf" in line and "mkdir" not in line:      # the cleanup call
                os.kill(os.getpid(), signal.SIGTERM)
            return None
        runner = FakeRunner(hook=hook)
        saved = {s: signal.getsignal(s) for s in pi_lane.LANE_SIGNALS}
        with tempfile.TemporaryDirectory() as d:
            report = os.path.join(d, "r.json")
            try:
                rc, out = self.run_main(["clawft-kernel", "--report", report], runner)
            finally:
                for s, h in saved.items():
                    signal.signal(s, h)
            with open(report) as f:
                rep = json.load(f)
        self.assertEqual(rc, 1, out)
        self.assertEqual(self.probes(runner), 2)
        self.assertIn("INFO  operator data:", out)
        self.assertIn("signal %d during cleanup" % signal.SIGTERM, out)
        self.assertFalse(rep["ok"])
        self.assertTrue(rep["guard"]["ok"])

    def test_staging_first_removes_a_stale_scratch_dir(self):
        runner = FakeRunner()
        self.run_main(["clawft-kernel"], runner)
        mk = next(" ".join(c) for c in runner.calls if "mkdir -p" in " ".join(c))
        self.assertLess(mk.index("sudo -n rm -rf weftos-test-pi"), mk.index("mkdir -p"))

    def test_no_args_means_full_lane(self):
        a = pi_lane.parse_args([])
        self.assertEqual((a.crates, a.live_native, a.cogs), (["clawft-kernel"], True, True))
        a = pi_lane.parse_args(["clawft-types"])
        self.assertEqual((a.crates, a.live_native, a.cogs), (["clawft-types"], False, False))
        with self.assertRaises(SystemExit), redirect_stdout(io.StringIO()), \
                mock.patch("sys.stderr", io.StringIO()):
            pi_lane.parse_args(["--scratch", "../etc"])


if __name__ == "__main__":
    unittest.main()
