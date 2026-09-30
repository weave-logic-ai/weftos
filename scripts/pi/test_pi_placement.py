"""Unit tests for the two-node placement stage (pi_plan, pi_ctl_plan, pi_placement).

Run: python3 -m unittest discover -s scripts/pi
"""
import io
import json
import os
import re
import shlex
import unittest
from contextlib import redirect_stdout
from unittest import mock

import pi_ctl_plan as ctl
import pi_lane
import pi_placement
import pi_plan as plan
from test_pi_plan import FakeRunner, LaneBehaviour

NODE = "n-1a2b3c"
MAC = "n-0f0f0f"
PUB = "ab" * 32
GOOD_CHAIN = [
    {"source": "mesh_artifact", "kind": "artifact.fetch"},
    {"source": "workload.host", "kind": "workload.place"},
    {"source": "workload.runtime", "kind": "workload.start"},
    {"source": "workload.runtime", "kind": "workload.stop"},
    {"source": "workload.host", "kind": "workload.refuse"},
]
MAC_CHAIN = [
    {"source": "workload.placement", "kind": "workload.place"},
    {"source": "workload.placement", "kind": "workload.refuse"},
    {"source": "workload.host", "kind": "workload.refuse"},
]
READY = ('==STATUS==\n' + json.dumps({"served_on": "0.0.0.0:9471", "targets": [{"tier": "pinned"}],
                                        "workload_host": {"node_id": NODE, "name": "workload-host"}},
                                       indent=2))
MAC_STATUS = {"controller": MAC, "targets": [
    {"node_id": MAC, "tier": "pinned", "reachable": True},
    {"node_id": NODE, "tier": "paired", "reachable": True, "public_key": "ef" * 32}]}
EXPLAIN = {"explain": "PLACED on %s via aarch64-native (tier native, emulated false)\n"
                      "  %s rejected: no variant fits\n" % (NODE, MAC)}
PLACE = dict(EXPLAIN, placed={"node_id": NODE, "instance_id": "i-1", "variant": "aarch64-native"},
             attempts=[{"node_id": NODE, "outcome": "placed"}])
REFUSED_BY_PI = {"node_id": NODE, "outcome": "refused", "code": "admission",
                 "reason": "admission refused: ELF machine 62 is not aarch64"}
BAD = {"placed": None, "attempts": [REFUSED_BY_PI]}
PIN = {"placed": None, "attempts": [], "decision": {"placement": None}}
# With --mac-container: the Mac's Docker adapter is a real next candidate.
BAD_CONTAINER = {"placed": None, "attempts": [REFUSED_BY_PI, {
    "node_id": MAC, "outcome": "refused", "code": "admission",
    "reason": "admission refused: ELF machine 62 is not aarch64"}]}
PIN_CONTAINER = {"placed": {"node_id": MAC, "instance_id": "m-1", "variant": "aarch64-container"},
                 "attempts": [{"node_id": MAC, "outcome": "placed"}]}


def good_result():
    return {"explain": EXPLAIN, "place": PLACE, "place_rc": 0,
            "status": {"status": {"state": "running"}}, "reports": 3, "bad": BAD, "bad_rc": 1,
            "pin": PIN, "pin_rc": 1, "mac_chain": MAC_CHAIN, "pi_chain": GOOD_CHAIN,
            "peer_key_pinned": True}


def good_container_result():
    r = good_result()
    r.update(bad=BAD_CONTAINER, pin=PIN_CONTAINER, pin_rc=0,
             pin_status={"status": {"state": "running"}})
    return r


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
        self.assertEqual(len(ev), len(GOOD_CHAIN))
        self.assertIsNone(plan.split_node_output("log only")[1])
        self.assertIsNone(plan.split_node_output("x==CHAIN==not json")[1])

    def test_leak_guard(self):
        self.assertIsNone(plan.LEAK_RE.search('{"csi_bind": "0.0.0.0:15006", "x": "127.0.0.1:80"}'))
        self.assertIsNotNone(plan.LEAK_RE.search('"peer": "10.1.2.3:9471"'))
        self.assertIsNotNone(plan.LEAK_RE.search('"/Users/someone/x"'))


class CtlHelpers(unittest.TestCase):
    def test_mac_side_is_isolated_under_env_i(self):
        cmd = ctl.env_cmd("/w", ["/t/weaver", "kernel", "start"])
        self.assertEqual(cmd[:2], ["env", "-i"])
        self.assertIn("HOME=/w/home", cmd)
        self.assertIn("WEFTOS_RUNTIME_DIR=/w/runtime", cmd)
        self.assertFalse(any(c.startswith(("USER=", "SSH_AUTH_SOCK=")) for c in cmd))
        self.assertTrue(ctl.socket_fits("/var/folders/ab/cd/T/wfp-x"))
        self.assertFalse(ctl.socket_fits("/" + "x" * 100))

    def test_keys(self):
        self.assertEqual(ctl.parse_keygen("wrote k\nkey id x\npublic key %s\n" % PUB), PUB)
        self.assertIsNone(ctl.parse_keygen("public key zz"))
        self.assertEqual(ctl.seed_bytes("ab" * 32 + "\n"), bytes([0xab]) * 32)
        with self.assertRaises(ValueError):
            ctl.seed_bytes("abc")

    def test_mislabeled_binary_is_an_x86_64_elf(self):
        b = ctl.mislabeled_elf()
        self.assertEqual((b[:4], b[5], int.from_bytes(b[18:20], "little")), (b"\x7fELF", 1, 62))

    def test_ready_needs_local_pinned_and_the_pi_paired(self):
        self.assertEqual(ctl.mac_ready(MAC_STATUS, NODE), MAC)
        disc = json.loads(json.dumps(MAC_STATUS))
        disc["targets"][1]["tier"] = "discovered"
        self.assertIsNone(ctl.mac_ready(disc, NODE))
        self.assertIsNone(ctl.mac_ready(MAC_STATUS, "n-other"))
        self.assertIsNone(ctl.mac_ready(None, NODE))

    def test_reports(self):
        n, first = ctl.count_reports('noise\n{"stats": {"n": 1}}\n{"x": 1}\n{"stats": {}}\n')
        self.assertEqual((n, first), (2, {"stats": {"n": 1}}))

    def test_judge(self):
        self.assertEqual(ctl.judge(good_result(), NODE, MAC), (True, []))
        cases = {
            "explain": lambda r: r.update(explain={"explain": "PLACED on %s via x" % MAC}),
            "place": lambda r: r.update(place_rc=1),
            "running": lambda r: r.update(status={"status": {"state": "exited"}}),
            "reports": lambda r: r.update(reports=0),
            "self-check": lambda r: r["bad"]["attempts"][0].update(reason="no native adapter"),
            "without a container adapter was dispatched": lambda r: r.update(bad=BAD_CONTAINER),
            "pinning this Mac without a container adapter": lambda r: r.update(pin=PIN_CONTAINER),
            "not by its pinned key": lambda r: r.update(peer_key_pinned=False),
            "Pi chain lacks workload.host/workload.refuse": lambda r: r.update(pi_chain=GOOD_CHAIN[:4]),
            "Mac chain lacks": lambda r: r.update(mac_chain=None),
        }
        for want, spoil in cases.items():
            r = json.loads(json.dumps(good_result()))
            spoil(r)
            ok, why = ctl.judge(r, NODE, MAC)
            self.assertFalse(ok, want)
            self.assertTrue(any(want in w for w in why), (want, why))

    def test_a_native_only_mac_host_never_refuses_on_its_own_chain(self):
        r = good_result()
        r["mac_chain"] = MAC_CHAIN[:2]
        self.assertEqual(ctl.judge(r, NODE, MAC), (True, []))

    def test_judge_with_the_mac_container_adapter(self):
        self.assertEqual(ctl.judge(good_container_result(), NODE, MAC, True), (True, []))
        cases = {
            "next candidate was not tried": lambda r: r.update(bad=BAD),
            "did not run the cog in its container": lambda r: r.update(pin=PIN, pin_rc=1),
            "not running in its container": lambda r: r.update(pin_status={}),
            "Mac chain lacks workload.host/workload.refuse": lambda r: r.update(
                mac_chain=MAC_CHAIN[:2]),
        }
        for want, spoil in cases.items():
            r = json.loads(json.dumps(good_container_result()))
            spoil(r)
            ok, why = ctl.judge(r, NODE, MAC, True)
            self.assertFalse(ok, want)
            self.assertTrue(any(want in w for w in why), (want, why))

    def test_peer_key_and_container_config(self):
        self.assertEqual(ctl.pi_key(MAC_STATUS, NODE), "ef" * 32)
        self.assertIsNone(ctl.pi_key(MAC_STATUS, "n-other"))
        self.assertEqual(ctl.peers_json("pi5", 9471, "ef" * 32)[0]["key"], "ef" * 32)
        with self.assertRaises(ValueError):
            ctl.peers_json("pi5", 9471, "zz")
        img = "debian@sha256:" + "a" * 64
        self.assertEqual(ctl.container_json(img)["base_image"], img)
        for bad in ("debian:trixie", "debian@sha256:abc", "Debian@sha256:" + "a" * 64):
            with self.assertRaises(ValueError):
                ctl.container_json(bad)
        self.assertEqual(ctl.container_name("cog.A_1"), "weftos-cog-a-1")
        env = ctl.mac_env("/w", ("/opt/d/bin", "unix:///d.sock"))
        self.assertTrue(env["PATH"].startswith("/opt/d/bin:"))
        self.assertEqual(env["DOCKER_HOST"], "unix:///d.sock")
        self.assertNotIn("DOCKER_HOST", ctl.mac_env("/w"))


class PlacementLane(unittest.TestCase):
    def hook(self, stop_out, ready=READY, mac_status=MAC_STATUS, placed=PLACE):
        def arg(line, flag):
            parts = shlex.split(line)
            return parts[parts.index(flag) + 1]

        def h(line):
            mac = "PATH=" + ctl.MAC_PATH in line
            if "workload keygen" in line:
                with open(arg(line, "--out"), "w") as f:
                    f.write("cd" * 32 + "\n")
                return 0, "public key %s\n" % PUB
            if "daemon-files" in line:
                out = arg(line, "--out")
                os.makedirs(out, exist_ok=True)
                for n in ("workload-host.json", "workload-trust.json", "workload-permits.json"):
                    with open(os.path.join(out, n), "w") as f:
                        f.write("{}")
                return 0, ""
            if "kernel start --foreground" in line:
                return 0, "4242\n"
            if "csi_feed.py" in line and "nohup" in line:
                return 0, "4343\n"
            if mac and "workload status i-1" in line:
                return 0, json.dumps({"status": {"state": "running"}})
            if mac and "workload status --json" in line:
                return 0, json.dumps(mac_status)
            if "workload status --json" in line:
                return 0, ready
            if mac and "workload explain" in line:
                return 0, json.dumps(EXPLAIN)
            if mac and "workload place" in line and "--pin" in line:
                return 1, json.dumps(PIN)
            if mac and "workload place" in line and "bad-pkg" in line:
                return 1, json.dumps(BAD)
            if mac and "workload place" in line:
                return 0, json.dumps(placed)
            if mac and "workload logs" in line:
                return 0, '{"stats": {"anomalies": 0}}\n'
            if mac and "chain export" in line:
                return 0, json.dumps(MAC_CHAIN)
            if "kill -TERM 4242 4343" in line:
                return 0, stop_out
            return None
        return h

    def run_lane(self, stop_out, **kw):
        runner = FakeRunner(hook=self.hook(stop_out, **kw))
        with mock.patch.object(pi_lane.Lane, "fetch_cog", return_value="/cache/cog-anomaly-detect"), \
                mock.patch.object(pi_placement, "RUN_SECS", 0), \
                mock.patch.object(pi_placement.shutil, "copy"), \
                mock.patch.object(pi_placement.time, "sleep"):
            rc, out = LaneBehaviour.run_main(LaneBehaviour(), ["--placement"], runner)
        return rc, out, runner

    def test_green_run_places_from_the_mac_weaver_cli_onto_the_pi_weaver_daemon(self):
        rc, out, runner = self.run_lane("log\n==CHAIN==" + json.dumps(GOOD_CHAIN))
        self.assertEqual(rc, 0, out)
        self.assertIn("PASS  placement", out)
        lines = [" ".join(c) for c in runner.calls]
        self.assertTrue(any("-p clawft-weave --bin weaver" in l and "pi-aarch64" in l for l in lines),
                        "the real weaver is cross-built for the Pi")
        self.assertTrue(any("-p clawft-weave --bin weaver --manifest-path" in l for l in lines),
                        "and built natively for the Mac controller")
        # The Mac controller is a weaver daemon started isolated, and every
        # placement step goes through its CLI.
        self.assertEqual(len(runner.spawned), 1)
        started = " ".join(runner.spawned[0])
        self.assertTrue(started.startswith("env -i ") and started.endswith("kernel start --foreground"))
        for verb in ("workload keygen", "workload pack", "workload explain", "workload place",
                     "workload status i-1", "workload stop i-1", "workload logs i-1",
                     "workload unload i-1", "--pin " + MAC):
            self.assertTrue(any(verb in l and "debug/weaver" in l for l in lines), verb)
        self.assertFalse(any("workload_node place" in l or " place --key" in l for l in lines),
                         "the example binary never places")
        self.assertTrue(any("daemon-files --controller " + PUB in l for l in lines))
        self.assertTrue(any("rsync" in l and "workload-host.json" in l
                            and l.endswith("/home/u/weftos-test-pi/runtime/") for l in lines))
        self.assertTrue(any("kill -TERM 4242 4343" in l for l in lines))
        self.assertTrue(any("[w]eftos-test-pi/bin/weaver kernel start" in l for l in lines))
        self.assertTrue(any("rm -rf weftos-test-pi" in l for l in lines))
        self.assertFalse(any("rsync" in l and "cog-anomaly-detect" in l for l in lines))

    def test_missing_pi_side_evidence_fails_the_lane(self):
        rc, out, _ = self.run_lane("log\n==CHAIN==[]")
        self.assertEqual(rc, 1, out)
        self.assertIn("Pi chain lacks", out)

    def test_a_pi_daemon_that_never_serves_aborts_and_is_stopped(self):
        rc, out, runner = self.run_lane("", ready="==STATUS==\n{}")
        self.assertEqual(rc, 1, out)
        self.assertIn("did not serve workload-host", out)
        lines = [" ".join(c) for c in runner.calls]
        self.assertTrue(any("[w]eftos-test-pi/bin/weaver kernel start" in l for l in lines))

    def test_a_mac_daemon_that_never_pairs_the_pi_aborts(self):
        disc = json.loads(json.dumps(MAC_STATUS))
        disc["targets"][1]["tier"] = "discovered"
        with mock.patch.object(pi_placement.MacController, "wait_ready", return_value=None):
            rc, out, runner = self.run_lane("", mac_status=disc)
        self.assertEqual(rc, 1, out)
        self.assertIn("never saw the Pi as a paired target", out)
        lines = [" ".join(c) for c in runner.calls]
        self.assertTrue(any("[w]eftos-test-pi/bin/weaver kernel start" in l for l in lines),
                        "the Pi daemon is still stopped")

    def test_placing_elsewhere_than_the_pi_daemon_fails(self):
        elsewhere = dict(PLACE, placed=dict(PLACE["placed"], node_id=MAC))
        rc, out, _ = self.run_lane("log\n==CHAIN==" + json.dumps(GOOD_CHAIN), placed=elsewhere)
        self.assertEqual(rc, 1, out)
        self.assertIn("did not place on the Pi", out)


if __name__ == "__main__":
    unittest.main()
