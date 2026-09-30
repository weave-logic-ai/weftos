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


class Helpers(unittest.TestCase):
    def test_serve_command_is_isolated_detached_and_on_the_test_port(self):
        line = plan.placement_serve_command("/home/u/weftos-test-pi", "/home/u/weftos-test-pi/bin/workload_node", PUB)
        self.assertIn("env -i", line)
        self.assertIn("WEFTOS_RUNTIME_DIR=/home/u/weftos-test-pi/runtime", line)
        self.assertIn("HOME=/home/u/weftos-test-pi/home", line)
        self.assertIn("--listen 0.0.0.0:9471", line)
        self.assertNotIn("9470", line)
        self.assertIn("--noise", line)
        self.assertTrue(line.endswith("& echo $!"))
        # Only the node is backgrounded (a backgrounded `cd && ...` list keeps
        # ssh's stdout open and the ssh call hangs).
        self.assertIn("|| exit 1; nohup env -i", line)
        with self.assertRaises(ValueError):
            plan.placement_serve_command("/s", "/s/bin/x", "not-a-key; rm -rf ~")

    def test_hostname_and_kill_backstop(self):
        self.assertEqual(plan.ssh_hostname("user@pi5.local"), "pi5.local")
        self.assertEqual(plan.ssh_hostname("pi5"), "pi5")
        with self.assertRaises(ValueError):
            plan.ssh_hostname("-oProxyCommand=x")
        kill = plan.placement_kill_all_command("weftos-test-pi")
        # The pattern must not match the ssh shell that runs it.
        pat = kill.split("'")[1]
        self.assertIsNotNone(re.search(pat, "/home/u/weftos-test-pi/bin/workload_node serve --noise"))
        self.assertIsNone(re.search(pat, "bash -c " + kill))
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
    def hook(self, stop_out):
        def h(line):
            if "workload_node keygen" in line:
                return 0, PUB + "\n"
            if " serve --listen" in line:
                return 0, "4242\n"
            if "grep -q LISTENING" in line:
                return 0, "NODE_ID %s\nLISTENING 0.0.0.0:9471\n" % NODE
            if " place --key" in line:
                return 0, GOOD_CTL
            if "kill -TERM 4242" in line:
                return 0, stop_out
            return None
        return h

    def run_lane(self, stop_out):
        runner = FakeRunner(hook=self.hook(stop_out))
        with mock.patch.object(pi_lane.Lane, "fetch_cog", return_value="/cache/cog-anomaly-detect"):
            rc, out = LaneBehaviour.run_main(LaneBehaviour(), ["--placement"], runner)
        return rc, out, runner

    def test_green_placement_run_stops_the_node_and_cleans_up(self):
        rc, out, runner = self.run_lane("NODE_ID x\n==CHAIN==" + json.dumps(GOOD_CHAIN))
        self.assertEqual(rc, 0, out)
        self.assertIn("PASS  placement", out)
        lines = [" ".join(c) for c in runner.calls]
        self.assertTrue(any("workload_node" in l and "--example" in l for l in lines))
        self.assertTrue(any("kill -TERM 4242" in l for l in lines))
        self.assertTrue(any("pkill -TERM -f '[w]eftos-test-pi/bin/workload_node serve'" in l for l in lines))
        self.assertTrue(any("rm -rf weftos-test-pi" in l for l in lines))
        # The cog binary is not rsynced to the Pi: it travels over the mesh.
        self.assertFalse(any("rsync" in l and "cog-anomaly-detect" in l for l in lines))

    def test_missing_pi_side_evidence_fails_the_lane(self):
        rc, out, _ = self.run_lane("NODE_ID x\n==CHAIN==[]")
        self.assertEqual(rc, 1, out)
        self.assertIn("Pi chain lacks", out)


if __name__ == "__main__":
    unittest.main()
