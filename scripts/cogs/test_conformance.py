"""Unit + integration tests for the cog conformance harness.

Run: scripts/build.sh cogs-conformance selftest
No containers and no network: adapters are driven through an injected
runner, downloads through an injected opener, and the end-to-end harness
test runs a fake cog (a Python script) against the real feed and stub.
"""
import io
import json
import os
import socket
import stat
import struct
import sys
import tempfile
import textwrap
import unittest
import urllib.error

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import classify  # noqa: E402
import conformance  # noqa: E402
import harness  # noqa: E402
import runtimes  # noqa: E402


def raw(cid, mode="once", rc=0, ingest=1, timed_out=False, cycle_ms=700.0, status="ran"):
    return {"id": cid, "mode": mode, "rc": rc, "ingest_posts": ingest, "status": status,
            "timed_out": timed_out, "cycle_ms": cycle_ms, "cycles": 1,
            "interval_s": 1 if mode == "interval" else None, "feed_id": "reference-v1",
            "harness_version": "1.0.0", "sha256": "ab" * 32}


class PacketFormat(unittest.TestCase):
    def test_feature_packet_is_48_bytes_magic_and_8_le_f32_at_offset_16(self):
        pkt = harness.feature_packet(0)
        self.assertEqual(len(pkt), 48)
        self.assertEqual(struct.unpack_from("<I", pkt, 0)[0], 0xC5110003)
        self.assertEqual(pkt[4:16], b"\x00" * 12)
        vals = struct.unpack_from("<8f", pkt, 16)
        for got, want in zip(vals, harness.feature_values(0)):
            self.assertAlmostEqual(got, want, places=6)

    def test_every_40th_feature_packet_is_a_spike(self):
        self.assertEqual(struct.unpack_from("<8f", harness.feature_packet(39), 16),
                         tuple([struct.unpack("<f", struct.pack("<f", 0.95))[0]] * 8))
        self.assertLess(max(abs(v) for v in harness.feature_values(38)), 0.11)

    def test_vitals_packet_matches_edge_vitals_pkt_t_layout(self):
        # Offsets as decoded by cog-sensor-sources decode_vitals().
        p = harness.vitals_packet(3, presence=True, breathing_bpm=18.0, heart_bpm=65.0,
                                  n_persons=2, motion_energy=0.25, presence_score=9.0)
        self.assertEqual(len(p), 32)
        self.assertEqual(struct.unpack_from("<I", p, 0)[0], 0xC5110002)
        self.assertEqual(p[5] & 1, 1)
        self.assertEqual(struct.unpack_from("<H", p, 6)[0] / 100.0, 18.0)
        self.assertEqual(struct.unpack_from("<I", p, 8)[0] / 10000.0, 65.0)
        self.assertEqual(p[13], 2)
        self.assertEqual(struct.unpack_from("<f", p, 16)[0], 0.25)
        self.assertEqual(struct.unpack_from("<f", p, 20)[0], 9.0)
        self.assertEqual(struct.unpack_from("<I", p, 24)[0], 60)

    def test_feed_selection(self):
        self.assertEqual([len(p) for p in harness.packets_for_tick(0, "features")], [48])
        self.assertEqual([len(p) for p in harness.packets_for_tick(0, "vitals")], [32])
        self.assertEqual([len(p) for p in harness.packets_for_tick(0, "both")], [48, 32])
        self.assertEqual([len(p) for p in harness.packets_for_tick(1, "both")], [48])
        with self.assertRaises(ValueError):
            harness.packets_for_tick(0, "bogus")


class ArgvAndCycles(unittest.TestCase):
    def test_build_argv(self):
        self.assertEqual(harness.build_argv("/b", "once", 1, None), ["/b", "--once"])
        self.assertEqual(harness.build_argv("/b", "interval", 5, ["--x"]),
                         ["/b", "--interval", "5", "--x"])
        for bad in [("nope", 1), ("interval", 0), ("interval", "1"), ("interval", 99999)]:
            with self.assertRaises(ValueError):
                harness.build_argv("/b", bad[0], bad[1], None)

    def test_once_cycle_is_wall_time_only_on_success(self):
        self.assertEqual(harness.cycle_stats("once", 812.34, 0, [500]), (812.3, 1))
        self.assertEqual(harness.cycle_stats("once", 812.34, 2, []), (None, 0))

    def test_interval_cycle_is_median_gap_between_events(self):
        self.assertEqual(harness.cycle_stats("interval", 9000, None, [100, 1100, 2150, 3100]),
                         (1000.0, 4))
        self.assertEqual(harness.cycle_stats("interval", 9000, None, [100]), (None, 1))


class Classification(unittest.TestCase):
    def test_outcomes(self):
        self.assertEqual(classify.classify(raw("a")), "clean")
        self.assertEqual(classify.classify(raw("a", ingest=0)), "no-output")
        self.assertEqual(classify.classify(raw("a", rc=2, ingest=0)), "cli-error")
        self.assertEqual(classify.classify(raw("a", mode="interval", rc=None, timed_out=True,
                                               ingest=11)), "clean")
        self.assertEqual(classify.classify(raw("a", rc=None, timed_out=True, ingest=3)),
                         "no-output")  # a --once run that never exited
        self.assertEqual(classify.classify({"id": "a", "status": "missing-binary"}),
                         "missing-binary")

    def test_expectations_file_is_valid_and_complete(self):
        doc = json.load(open(conformance.EXPECTATIONS))
        cogs = classify.validate_expectations(doc)
        self.assertEqual(len(cogs), 108)
        counts = {}
        for e in cogs.values():
            counts[e["group"]] = counts.get(e["group"], 0) + 1
        self.assertEqual(counts, {"clean": 93, "needs-interval": 5,
                                  "needs-extra-cli": 9, "no-build": 1})

    def test_expectations_rejects_bad_input(self):
        good = {"group": "clean", "run_mode": "once"}
        for bad in [{"cogs": {"../x": good}}, {"cogs": {"x": {"group": "zzz"}}},
                    {"cogs": {"x": {"group": "needs-interval", "run_mode": "once"}}},
                    {"cogs": {"x": dict(good, interval=0)}}, {"nope": 1}]:
            with self.assertRaises(ValueError):
                classify.validate_expectations(bad)

    def test_legacy_summary_reproduces_committed_baseline(self):
        base = json.load(open(conformance.BASELINE))
        text = open(os.path.join(HERE, "baseline", "aarch64-once-2026-09-28.summary.txt")).read()
        legacy = classify.parse_legacy_summary(text)
        self.assertEqual(len(legacy), 107)
        self.assertEqual(sum(1 for v in legacy.values() if v == "clean"), 93)
        for cid, o in legacy.items():
            self.assertEqual(base["outcomes"][cid], o, cid)

    def synthetic_once_sweep(self):
        exp = classify.validate_expectations(json.load(open(conformance.EXPECTATIONS)))
        results = []
        for cid, e in exp.items():
            o = e["once_outcome"]
            if o == "missing-binary":
                results.append({"id": cid, "status": "missing-binary"})
            else:
                results.append(raw(cid, rc=2 if o == "cli-error" else 0,
                                   ingest=1 if o == "clean" else 0))
        return exp, results

    def test_matching_sweep_matches_baseline(self):
        exp, results = self.synthetic_once_sweep()
        s = classify.summarize(results, exp, "once")
        self.assertEqual(s["group_counts"]["clean"], 93)
        self.assertEqual(s["group_counts"]["needs-interval"], 5)
        self.assertEqual(s["group_counts"]["needs-extra-cli"], 9)
        self.assertEqual(classify.compare_baseline(s, json.load(open(conformance.BASELINE))), [])

    def test_regression_is_detected(self):
        exp, results = self.synthetic_once_sweep()
        results = [dict(r, ingest_posts=0) if r["id"] == "fall-detect" else r for r in results]
        s = classify.summarize(results, exp, "once")
        self.assertEqual(s["groups"]["regressed"], ["fall-detect"])
        problems = classify.compare_baseline(s, json.load(open(conformance.BASELINE)))
        self.assertTrue(any("fall-detect" in p for p in problems))

    def test_expected_mode_wants_interval_cogs_clean(self):
        exp, _ = self.synthetic_once_sweep()
        bad = classify.summarize([raw("sleep-apnea", ingest=0)], exp, "expected")
        self.assertEqual(bad["unexpected"][0]["expected"], "clean")
        ok = classify.summarize([raw("sleep-apnea", mode="interval", rc=None, timed_out=True)],
                                exp, "expected")
        self.assertEqual(ok["unexpected"], [])
        self.assertEqual(classify.plan_spec("sleep-apnea", exp["sleep-apnea"], "expected"),
                         {"id": "sleep-apnea", "mode": "interval", "interval": 1})


class Capabilities(unittest.TestCase):
    def test_cycle_capability_is_measured_and_only_for_clean(self):
        caps = classify.cycle_capabilities([raw("a"), raw("b", ingest=0)],
                                           "aarch64", "docker", "T")
        self.assertEqual(len(caps), 1)
        c = caps[0]
        self.assertEqual((c["id"], c["provenance"]), ("perf.cog.cycle_ms", "measured"))
        self.assertEqual((c["attrs"]["cog_id"], c["attrs"]["value"]), ("a", 700.0))

    def test_upgrade_provenance(self):
        node = [{"id": "cpu.arch.aarch64", "provenance": "probed"},
                {"id": "runtime.container.docker", "provenance": "claimed"},
                {"id": "accel.gpu.metal", "provenance": "probed"},
                {"id": "perf.cog.cycle_ms", "provenance": "measured",
                 "attrs": {"cog_id": "a", "arch": "aarch64", "runtime": "docker", "value": 1}}]
        out = classify.upgrade_provenance(node, [raw("a")], "aarch64", "docker", "T")
        by = {(c["id"], (c.get("attrs") or {}).get("cog_id")): c for c in out}
        self.assertEqual(by[("cpu.arch.aarch64", None)]["provenance"], "measured")
        self.assertEqual(by[("runtime.container.docker", None)]["provenance"], "measured")
        self.assertEqual(by[("accel.gpu.metal", None)]["provenance"], "probed")
        perf = [c for c in out if c["id"] == "perf.cog.cycle_ms"]
        self.assertEqual([c["attrs"]["value"] for c in perf], [700.0])  # replaced
        self.assertEqual(node[0]["provenance"], "probed")  # input not mutated

    def test_no_upgrade_without_a_clean_run(self):
        node = [{"id": "cpu.arch.aarch64", "provenance": "claimed"}]
        out = classify.upgrade_provenance(node, [raw("a", rc=2, ingest=0)],
                                          "aarch64", "docker", "T")
        self.assertEqual(out, node)
        with self.assertRaises(ValueError):
            classify.upgrade_provenance(node, [], "sparc", "docker", "T")


class Runtimes(unittest.TestCase):
    ELF_A64 = b"\x7fELF\x02\x01\x01" + b"\x00" * 11 + (183).to_bytes(2, "little") + b"\x00" * 40

    def test_binary_url_and_validation(self):
        self.assertEqual(runtimes.binary_url("fall-detect", "arm"),
                         runtimes.BASE_URL + "/arm/cog-fall-detect-arm")
        self.assertTrue(runtimes.binary_url("x", "aarch64").endswith("/arm64/cog-x-aarch64"))
        for bad in ["../etc", "A", "", "x/y"]:
            with self.assertRaises(ValueError):
                runtimes.binary_url(bad, "aarch64")

    def test_check_elf(self):
        self.assertIsNone(runtimes.check_elf(self.ELF_A64, "aarch64"))
        self.assertIn("not aarch64", runtimes.check_elf(
            self.ELF_A64[:18] + (40).to_bytes(2, "little"), "aarch64"))
        self.assertEqual(runtimes.check_elf(b"<html>404", "aarch64"), "not an ELF file")

    def test_fetch_binary_caches_and_rejects(self):
        calls = []

        def opener(body=None, code=None):
            def f(url, timeout):
                calls.append(url)
                if code:
                    raise urllib.error.HTTPError(url, code, "x", {}, None)
                return io.BytesIO(body)
            return f
        with tempfile.TemporaryDirectory() as d:
            p, why = runtimes.fetch_binary("x", "aarch64", d, opener=opener(self.ELF_A64))
            self.assertIsNone(why)
            self.assertTrue(os.stat(p).st_mode & stat.S_IXUSR)
            p2, _ = runtimes.fetch_binary("x", "aarch64", d, opener=opener(code=500))
            self.assertEqual((p2, len(calls)), (p, 1))  # served from cache
            self.assertEqual(runtimes.fetch_binary("y", "aarch64", d, opener=opener(code=404)),
                             (None, "HTTP 404"))
            self.assertEqual(runtimes.fetch_binary("z", "aarch64", d, opener=opener(b"nope"))[1],
                             "not an ELF file")
            self.assertFalse(os.path.exists(os.path.join(d, "cog-z-aarch64")))

    def test_adapter_commands(self):
        d = runtimes.make_adapter("docker", "aarch64").commands("/w1")[0]
        self.assertEqual(d[:5], ["docker", "run", "--rm", "--platform", "linux/arm64"])
        self.assertIn("/w1:/w", d)
        a = runtimes.make_adapter("apple-container", "aarch64").commands("/w1")[0]
        self.assertEqual(a[:5], ["container", "run", "--rm", "--arch", "arm64"])
        self.assertEqual(runtimes.make_adapter("docker", "arm").commands("/w1")[0][4],
                         "linux/arm/v7")

    def test_ssh_adapter_remote_mode(self):
        seen = []

        class P:
            returncode = 0

        def runner(cmd, timeout):
            seen.append(cmd)
            return P()
        ad = runtimes.make_adapter("ssh", "aarch64", ssh_host="pi5", sudo=True, runner=runner)
        ad.run("/w1", timeout=10)
        self.assertEqual([c[0] for c in seen], ["ssh", "scp", "ssh", "scp"])
        self.assertIn("sudo -n python3 cog-conformance/harness.py", seen[2][2])
        self.assertEqual(ad.binary_root("/w1"), "cog-conformance")
        for bad in [None, "", "-oProxyCommand=x", "a b"]:
            with self.assertRaises(ValueError):
                runtimes.make_adapter("ssh", "aarch64", ssh_host=bad)

    def test_adapter_failure_raises(self):
        class P:
            returncode = 125
        ad = runtimes.make_adapter("docker", "aarch64", runner=lambda c, timeout: P())
        with self.assertRaises(RuntimeError):
            ad.run("/w1", timeout=10)


FAKE_COG = textwrap.dedent("""\
    #!{py}
    # Fake cog: reads the harness feed like cog-sensor-sources, posts one vector.
    import json, os, socket, struct, sys, urllib.request
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("127.0.0.1", int(os.environ["FAKE_UDP"]))); s.settimeout(3)
    feats, vit = [], None
    while len(feats) < 64:
        pkt, _ = s.recvfrom(256)
        magic = struct.unpack_from("<I", pkt, 0)[0]
        if magic == 0xC5110003 and len(pkt) >= 48:
            feats += struct.unpack_from("<8f", pkt, 16)
        elif magic == 0xC5110002:
            vit = struct.unpack_from("<H", pkt, 6)[0] / 100.0
    if "--once" not in sys.argv:
        sys.exit(2)
    body = json.dumps({{"vectors": [[0, feats[:8]]], "dedup": True, "breathing": vit}}).encode()
    req = urllib.request.Request("http://127.0.0.1:%s/api/v1/store/ingest" % os.environ["FAKE_INGEST"],
                                 data=body, method="POST")
    print(urllib.request.urlopen(req, timeout=3).read().decode())
""")


def free_port(kind):
    s = socket.socket(socket.AF_INET, kind)
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class HarnessEndToEnd(unittest.TestCase):
    """Real feed + real stub + a fake cog process: exercises run_cog end to end."""

    def run_fake(self, feed, mode="once"):
        udp, ingest = free_port(socket.SOCK_DGRAM), free_port(socket.SOCK_STREAM)
        with tempfile.TemporaryDirectory() as d:
            cog = os.path.join(d, "cog-fake-aarch64")
            with open(cog, "w") as f:
                f.write(FAKE_COG.format(py=sys.executable))
            os.environ.update(FAKE_UDP=str(udp), FAKE_INGEST=str(ingest))
            return harness.run_plan({"timeout": 10, "feed": feed, "udp_port": udp,
                                     "ingest_port": ingest,
                                     "cogs": [{"id": "fake", "binary": cog, "mode": mode}]})

    def test_fake_cog_decodes_feed_and_posts_ingest(self):
        r = self.run_fake("both")["results"][0]
        self.assertEqual((r["status"], r["rc"]), ("ran", 0))
        self.assertEqual((r["ingest_posts"], r["ingest_vectors"]), (1, 1))
        body = json.loads(r["ingest_samples"][0])
        got = body["vectors"][0][1]
        # The cog joins mid-stream, so match the decoded vector to some feed tick.
        self.assertTrue(any(all(abs(g - w) < 1e-6 for g, w in zip(got, harness.feature_values(t)))
                            for t in range(1000)), got)
        self.assertEqual(body["breathing"], 15.0)
        self.assertEqual(classify.classify(r), "clean")
        self.assertIsNotNone(r["cycle_ms"])
        self.assertEqual(r["argv"], ["cog-fake-aarch64", "--once"])

    def test_cli_error_is_captured(self):
        r = self.run_fake("features", mode="interval")["results"][0]
        self.assertEqual((r["rc"], r["ingest_posts"]), (2, 0))
        self.assertEqual(classify.classify(r), "cli-error")

    def test_launcher_wraps_the_cog_argv(self):
        """A plan launcher runs the cog as `<launcher> -- <argv>` (the launcher
        here is a pass-through shim standing in for cog_adapter_run)."""
        with tempfile.TemporaryDirectory() as d:
            shim = os.path.join(d, "launch")
            with open(shim, "w") as f:
                f.write("#!/bin/sh\n[ \"$1\" = --runtime ] && shift 2\n"
                        "[ \"$1\" = -- ] || exit 9\nshift\necho launched >&2\nexec \"$@\"\n")
            os.chmod(shim, 0o755)
            udp, ingest = free_port(socket.SOCK_DGRAM), free_port(socket.SOCK_STREAM)
            cog = os.path.join(d, "cog-fake-aarch64")
            with open(cog, "w") as f:
                f.write(FAKE_COG.format(py=sys.executable))
            os.environ.update(FAKE_UDP=str(udp), FAKE_INGEST=str(ingest))
            r = harness.run_plan({"timeout": 10, "udp_port": udp, "ingest_port": ingest,
                                  "launcher": [shim, "--runtime", "native"],
                                  "cogs": [{"id": "fake", "binary": cog}]})["results"][0]
        self.assertEqual(classify.classify(r), "clean")
        self.assertEqual(r["launcher"], ["launch", "--runtime", "native"])
        self.assertIn("launched", r["stderr_tail"])
        with self.assertRaises(ValueError):
            harness.run_plan({"launcher": "not-a-list", "cogs": []})

    def test_engine_args_precede_the_image(self):
        ad = runtimes.make_adapter("docker", "aarch64", engine_args=["--net=host"])
        cmd = ad.commands("/w1")[0]
        self.assertLess(cmd.index("--net=host"), cmd.index(runtimes.DEFAULT_IMAGE))
        with self.assertRaises(ValueError):
            runtimes.make_adapter("docker", "aarch64", engine_args=[""])

    def test_launcher_plan_arguments(self):
        a = conformance.build_parser().parse_args(
            ["sweep", "--launcher", "/x/cog_adapter_run", "--adapter-runtime", "docker",
             "--adapter-base-image", "b@sha256:" + "0" * 64, "--adapter-network", "host",
             "--timeout", "20"])
        argv = conformance.launcher_plan(a, "/w")
        self.assertEqual(argv[:5], ["/w/bin/cog_adapter_run", "--runtime", "docker",
                                    "--arch", "aarch64"])
        self.assertIn("--run-secs", argv)
        self.assertEqual(argv[argv.index("--run-secs") + 1], "16")
        self.assertEqual(argv[argv.index("--network") + 1], "host")
        a.adapter_base_image = None
        with self.assertRaises(SystemExit):
            conformance.launcher_plan(a, "/w")
        self.assertIsNone(conformance.launcher_plan(
            conformance.build_parser().parse_args(["sweep"]), "/w"))

    def test_ingest_stub_binds_the_requested_address(self):
        """The stub can bind a non-loopback address (for a container VM's
        relay); the fake cog still reaches it over 127.0.0.1 via 0.0.0.0."""
        udp, ingest = free_port(socket.SOCK_DGRAM), free_port(socket.SOCK_STREAM)
        with tempfile.TemporaryDirectory() as d:
            cog = os.path.join(d, "cog-fake-aarch64")
            with open(cog, "w") as f:
                f.write(FAKE_COG.format(py=sys.executable))
            os.environ.update(FAKE_UDP=str(udp), FAKE_INGEST=str(ingest))
            r = harness.run_plan({"timeout": 10, "udp_port": udp, "ingest_port": ingest,
                                  "ingest_bind": "0.0.0.0",
                                  "cogs": [{"id": "fake", "binary": cog}]})["results"][0]
        self.assertEqual((classify.classify(r), r["ingest_posts"]), ("clean", 1))
        with self.assertRaises(ValueError):
            harness.run_plan({"ingest_bind": "not-an-ip", "cogs": []})

    def test_launcher_plan_passes_the_ingest_upstream(self):
        a = conformance.build_parser().parse_args(
            ["sweep", "--launcher", "/x/cog_adapter_run", "--adapter-runtime", "apple",
             "--adapter-base-image", "b@sha256:" + "0" * 64,
             "--adapter-ingest-upstream", "192.0.2.10:18080", "--ingest-bind", "0.0.0.0"])
        argv = conformance.launcher_plan(a, "/w")
        self.assertEqual(argv[argv.index("--ingest-upstream") + 1], "192.0.2.10:18080")
        self.assertEqual(a.ingest_bind, "0.0.0.0")

    def test_missing_binary(self):
        doc = harness.run_plan({"cogs": [{"id": "gone", "binary": "/nonexistent/cog"}],
                                "ingest_port": free_port(socket.SOCK_STREAM)})
        self.assertEqual(classify.classify(doc["results"][0]), "missing-binary")


if __name__ == "__main__":
    unittest.main()
