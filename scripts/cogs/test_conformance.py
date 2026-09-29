"""Unit + integration tests for the cog conformance harness.

Run: scripts/build.sh cogs-conformance selftest
No containers and no network: adapters are driven through an injected
runner, downloads through an injected opener, and the end-to-end harness
test runs a fake cog (a Python script) against the real feed and stub.
"""
import contextlib
import io
import json
import os
import stat
import struct
import sys
import tempfile
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


class AdapterAttribution(unittest.TestCase):
    """Measurements belong to the adapter that ran the cog, not the runtime
    the harness itself runs on."""

    def _run(self, argv, node_caps=None):
        real = conformance.execute
        conformance.execute = lambda *_a, **_k: ([raw("anomaly-detect")], {"system": "Darwin"})
        try:
            with tempfile.TemporaryDirectory() as d:
                args = conformance.build_parser().parse_args(argv + (
                    ["--node-facts", os.path.join(d, "f.json"), "--out", os.path.join(d, "o.json")]
                    if argv[0] == "probe" else ["--results-dir", d, "--label", "x"]))
                if node_caps is not None:
                    with open(os.path.join(d, "f.json"), "w") as f:
                        json.dump(node_caps, f)
                with contextlib.redirect_stdout(io.StringIO()):
                    args.fn(args)
                if argv[0] == "probe":
                    with open(os.path.join(d, "o.json")) as f:
                        return json.load(f)["capabilities"], None
                with open(os.path.join(d, "x", "capabilities.json")) as f, \
                        open(os.path.join(d, "x", "summary.json")) as g:
                    return json.load(f), json.load(g)
        finally:
            conformance.execute = real

    LAUNCH = ["--launcher", "/x/cog_adapter_run", "--adapter-base-image",
              "b@sha256:" + "0" * 64]

    def test_sweep_labels_the_adapter_runtime(self):
        caps, summary = self._run(["sweep", "--runtime", "native", "--adapter-runtime", "apple"]
                                  + self.LAUNCH)
        self.assertEqual({c["attrs"]["runtime"] for c in caps}, {"apple-container"})
        self.assertEqual({c["attrs"]["harness_runtime"] for c in caps}, {"native"})
        self.assertEqual((summary["runtime"], summary["harness_runtime"]),
                         ("apple-container", "native"))

    def test_without_a_launcher_the_harness_runtime_is_the_measured_one(self):
        caps, summary = self._run(["sweep", "--runtime", "docker"])
        self.assertEqual({c["attrs"]["runtime"] for c in caps}, {"docker"})
        self.assertNotIn("harness_runtime", caps[0]["attrs"])
        self.assertEqual(summary["runtime"], "docker")

    def test_probe_upgrades_the_adapter_capability_not_the_harness_one(self):
        node = [{"id": "runtime.native", "provenance": "claimed"},
                {"id": "runtime.container.apple", "provenance": "claimed"}]
        caps, _ = self._run(["probe", "--cog", "anomaly-detect", "--runtime", "native",
                             "--adapter-runtime", "apple"] + self.LAUNCH, node)
        by = {c["id"]: c for c in caps if c["id"] != "perf.cog.cycle_ms"}
        self.assertEqual(by["runtime.container.apple"]["provenance"], "measured")
        self.assertEqual(by["runtime.native"]["provenance"], "claimed")

    def test_podman_adapter_has_a_capability(self):
        self.assertEqual(classify.RUNTIME_CAPABILITY[conformance.ADAPTER_RUNTIME["podman"]],
                         "runtime.container.podman")


class BuildScriptHelp(unittest.TestCase):
    def test_build_sh_help_documents_the_cogs_commands(self):
        """`scripts/build.sh --help` runs under `set -u`; an unescaped variable
        in the usage text aborts it before printing anything."""
        import subprocess
        script = os.path.join(HERE, "..", "build.sh")
        p = subprocess.run(["bash", script, "--help"], capture_output=True, text=True,
                           timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertNotIn("unbound variable", p.stderr)
        self.assertIn("cogs-launcher", p.stdout)
        self.assertIn("COG_LAUNCHER_BUILDER", p.stdout)

    def test_help_does_not_trip_set_u(self):
        """The cogs-launcher usage text must not expand unset variables."""
        import subprocess
        env = {k: v for k, v in os.environ.items() if k != "COG_LAUNCHER_BUILDER"}
        p = subprocess.run(["bash", os.path.join(HERE, "..", "build.sh"), "--help"],
                           capture_output=True, text=True, env=env, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertNotIn("unbound variable", p.stderr)
        self.assertIn("cogs-launcher", p.stdout)


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


if __name__ == "__main__":
    unittest.main()
