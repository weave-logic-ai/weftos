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


class FakeRunner:
    """Scripted Pi: ssh/docker/rsync calls answered from `script`."""

    def __init__(self, fail_on=None, chain=("100", "100")):
        self.dry_run, self.calls, self.fail_on = False, [], fail_on
        self.chain = list(chain)

    def __call__(self, cmd, capture="all", timeout=None):
        self.calls.append(cmd)
        line = " ".join(cmd)
        if "stat -c %Y" in line:
            return 0, "%s active\n" % self.chain.pop(0)
        if "uname -m" in line:
            return 0, "aarch64\nldd (Debian GLIBC 2.41-12) 2.41\n/home/u\n"
        if "ldd --version" in line:
            return 0, "ldd (Debian GLIBC 2.36-9) 2.36\n"
        if "--no-run" in line:
            return 0, artifact("test", "e2e", "/target/debug/deps/e2e-2") + "\n"
        if "exec env -i" in line:
            if self.fail_on and self.fail_on in line:
                return 101, Results.BAD
            return 0, Results.OK
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
            git.return_value.stdout = b"crates/x\0"
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
        test_calls = [c for c in runner.calls if "exec env -i" in " ".join(c)]
        self.assertEqual(len(test_calls), 1)
        self.assertTrue(test_calls[0][-1].endswith("/home/u/weftos-test-pi/bin/e2e-2 chain"))
        self.assertIn("rm -rf weftos-test-pi", " ".join(runner.calls[-2]))

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
