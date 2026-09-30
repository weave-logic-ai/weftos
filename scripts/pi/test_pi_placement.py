"""Unit tests for the two-node placement stage (pi_plan helpers, pi_placement).

Run: python3 -m unittest discover -s scripts/pi
"""
import io
import json
import os
import re
import unittest
from contextlib import redirect_stdout
from unittest import mock

import pi_lane
import pi_plan as plan
from test_pi_plan import FakeRunner, LaneBehaviour

NODE = "n-1a2b3c"
PUB = "ab" * 32
GOOD_CTL = ("PLACED on %s via aarch64-native (tier native, emulated false)\nRESULT ok\n" % NODE)
GOOD_CHAIN = [
    {"source": "mesh_artifact", "kind": "artifact.fetch"},
    {"source": "workload.host", "kind": "workload.place"},
    {"source": "workload.runtime", "kind": "workload.start"},
    {"source": "workload.runtime", "kind": "workload.stop"},
]


READY = ('==STATUS==\n' + json.dumps({"served_on": "0.0.0.0:9471", "targets": [{"tier": "pinned"}],
                                        "workload_host": {"node_id": NODE, "name": "workload-host"}},
                                       indent=2))
GOOD_CTL = "PEER %s (paired)\n" % NODE + GOOD_CTL


class Helpers(unittest.TestCase):
    S = "/home/u/weftos-test-pi"

    def test_weaver_starts_isolated_detached_and_never_on_the_system_port(self):
        line = plan.weaver_start_command(self.S, self.S + "/bin/weaver")
        self.assertIn("env -i", line)
        self.assertIn("WEFTOS_RUNTIME_DIR=/home/u/weftos-test-pi/runtime", line)
        self.assertIn("HOME=/home/u/weftos-test-pi/home", line)
        self.assertIn("/home/u/weftos-test-pi/bin/weaver kernel start --foreground", line)
        self.assertNotIn("9470", line)
        self.assertNotIn("/usr/local/bin/weaver", line)
        self.assertTrue(line.endswith("& echo $!"))
        # Only the daemon is backgrounded (a backgrounded `cd && ...` list
        # keeps ssh's stdout open and the ssh call hangs).
        self.assertIn("|| exit 1; nohup env -i", line)
        feed = plan.feed_start_command(self.S)
        self.assertIn("csi_feed.py --port 15006", feed)
        self.assertIn("env -i", feed)

    def test_ready_waits_for_the_served_host_and_parses_its_status(self):
        line = plan.weaver_ready_command(self.S, self.S + "/bin/weaver")
        self.assertIn("workload status --json", line)
        self.assertIn('0.0.0.0:9471', line)
        self.assertIn("env -i", line)
        self.assertEqual(plan.parse_ready("noise\n" + READY), (NODE, "0.0.0.0:9471"))
        self.assertEqual(plan.parse_ready("==STATUS==\n{}"), (None, None))
        self.assertEqual(plan.parse_ready("no marker"), (None, None))

    def test_stop_exports_the_daemon_chain_then_stops_daemon_and_feed(self):
        line = plan.weaver_stop_command(self.S, self.S + "/bin/weaver", 4242, 4343)
        self.assertLess(line.index("chain export --format json"), line.index("kill -TERM 4242 4343"))
        self.assertIn("==CHAIN==", line)
        with self.assertRaises(ValueError):
            plan.weaver_stop_command(self.S, "w", "1; rm -rf ~")

    def test_hostname_and_kill_backstop(self):
        self.assertEqual(plan.ssh_hostname("user@pi5.local"), "pi5.local")
        self.assertEqual(plan.ssh_hostname("pi5"), "pi5")
        with self.assertRaises(ValueError):
            plan.ssh_hostname("-oProxyCommand=x")
        kill = plan.placement_kill_all_command("weftos-test-pi")
        pats = [p for i, p in enumerate(kill.split("'")) if i % 2]
        self.assertEqual(len(pats), 2)
        self.assertIsNotNone(re.search(pats[0], "/home/u/weftos-test-pi/bin/weaver kernel start --foreground"))
        self.assertIsNone(re.search(pats[0], "/usr/local/bin/weaver kernel start --foreground"),
                          "never the system weaver")
        self.assertIsNotNone(re.search(pats[1], "python3 /home/u/weftos-test-pi/src/scripts/pi/csi_feed.py"))
        for p in pats:   # the patterns never match the ssh shell that runs them
            self.assertIsNone(re.search(p, "bash -c " + kill))
        with self.assertRaises(ValueError):
            plan.placement_kill_all_command("../x")

    def test_node_output_split(self):
        log, ev = plan.split_node_output("NODE_ID x\n==CHAIN==\n" + json.dumps(GOOD_CHAIN))
        self.assertIn("NODE_ID", log)
        self.assertEqual(len(ev), 4)
        self.assertIsNone(plan.split_node_output("log only")[1])
        self.assertIsNone(plan.split_node_output("x==CHAIN==not json")[1])

    def test_judge_needs_both_sides(self):
        self.assertEqual(plan.judge_placement(0, GOOD_CTL, NODE, GOOD_CHAIN), (True, []))
        ok, why = plan.judge_placement(0, GOOD_CTL.replace("tier native", "tier dev_fallback"), NODE, GOOD_CHAIN)
        self.assertFalse(ok)
        ok, why = plan.judge_placement(1, GOOD_CTL, NODE, GOOD_CHAIN)
        self.assertFalse(ok)
        ok, why = plan.judge_placement(0, GOOD_CTL, NODE, GOOD_CHAIN[:1])
        self.assertFalse(ok)
        self.assertTrue(any("workload.place" in w for w in why))
        self.assertFalse(plan.judge_placement(0, GOOD_CTL, NODE, None)[0])

    def test_leak_guard(self):
        self.assertIsNone(plan.LEAK_RE.search('{"csi_bind": "0.0.0.0:15006", "x": "127.0.0.1:80"}'))
        self.assertIsNotNone(plan.LEAK_RE.search('"peer": "10.1.2.3:9471"'))
        self.assertIsNotNone(plan.LEAK_RE.search('"/Users/someone/x"'))


class PlacementLane(unittest.TestCase):
    def hook(self, stop_out, ready=READY, peer=True):
        def h(line):
            if "workload_node keygen" in line:
                return 0, PUB + "\n"
            if "kernel start --foreground" in line:
                return 0, "4242\n"
            if "csi_feed.py" in line and "nohup" in line:
                return 0, "4343\n"
            if "workload status --json" in line:
                return 0, ready
            if " place --key" in line:
                return 0, GOOD_CTL if peer else GOOD_CTL.replace("PEER", "NOPE")
            if "kill -TERM 4242 4343" in line:
                return 0, stop_out
            return None
        return h

    def run_lane(self, stop_out, **kw):
        runner = FakeRunner(hook=self.hook(stop_out, **kw))
        with mock.patch.object(pi_lane.Lane, "fetch_cog", return_value="/cache/cog-anomaly-detect"):
            rc, out = LaneBehaviour.run_main(LaneBehaviour(), ["--placement"], runner)
        return rc, out, runner

    def test_green_run_places_onto_the_pi_weaver_daemon_and_cleans_up(self):
        rc, out, runner = self.run_lane("log\n==CHAIN==" + json.dumps(GOOD_CHAIN))
        self.assertEqual(rc, 0, out)
        self.assertIn("PASS  placement", out)
        lines = [" ".join(c) for c in runner.calls]
        self.assertTrue(any("-p clawft-weave --bin weaver" in l for l in lines),
                        "the real weaver is cross-built for the Pi")
        self.assertTrue(any("daemon-files --controller " + PUB in l for l in lines))
        self.assertTrue(any("rsync" in l and "workload-host.json" in l
                            and l.endswith("/home/u/weftos-test-pi/runtime/") for l in lines))
        self.assertTrue(any("rsync" in l and "debug/weaver" in l for l in lines))
        self.assertTrue(any("kill -TERM 4242 4343" in l for l in lines))
        self.assertTrue(any("[w]eftos-test-pi/bin/weaver kernel start" in l for l in lines))
        self.assertTrue(any("rm -rf weftos-test-pi" in l for l in lines))
        # The cog binary is not rsynced to the Pi: it travels over the mesh.
        self.assertFalse(any("rsync" in l and "cog-anomaly-detect" in l for l in lines))

    def test_missing_pi_side_evidence_fails_the_lane(self):
        rc, out, _ = self.run_lane("log\n==CHAIN==[]")
        self.assertEqual(rc, 1, out)
        self.assertIn("Pi chain lacks", out)

    def test_a_daemon_that_never_serves_aborts_and_is_stopped(self):
        rc, out, runner = self.run_lane("", ready="==STATUS==\n{}")
        self.assertEqual(rc, 1, out)
        self.assertIn("did not serve workload-host", out)
        lines = [" ".join(c) for c in runner.calls]
        self.assertTrue(any("[w]eftos-test-pi/bin/weaver kernel start" in l for l in lines))

    def test_placing_elsewhere_than_the_pi_daemon_fails(self):
        rc, out, _ = self.run_lane("log\n==CHAIN==" + json.dumps(GOOD_CHAIN), peer=False)
        self.assertEqual(rc, 1, out)
        self.assertIn("did not reach the Pi daemon", out)


if __name__ == "__main__":
    unittest.main()
