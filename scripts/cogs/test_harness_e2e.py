"""End-to-end tests for the cog conformance harness: a real feed and a real
ingest stub against a fake cog (a Python script). No containers, no network.

Run: scripts/build.sh cogs-conformance selftest
"""
import json
import os
import socket
import sys
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import classify  # noqa: E402
import conformance  # noqa: E402
import harness  # noqa: E402
import runtimes  # noqa: E402


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

    def test_recorded_launcher_hides_host_addresses(self):
        """Committed results must not carry private or public host IPs."""
        self.assertEqual(harness.public_arg("192.168.64.1:18080"), "<host>:18080")
        self.assertEqual(harness.public_arg("10.0.0.7"), "<host>")
        self.assertEqual(harness.public_arg("127.0.0.1:5006"), "127.0.0.1:5006")
        self.assertEqual(harness.public_arg("0.0.0.0"), "0.0.0.0")
        self.assertEqual(harness.public_arg("1.2.3"), "1.2.3")
        with tempfile.TemporaryDirectory() as d:
            shim = os.path.join(d, "launch")
            with open(shim, "w") as f:
                f.write("#!/bin/sh\nexit 0\n")
            os.chmod(shim, 0o755)
            r = harness.run_plan({"timeout": 5, "ingest_port": free_port(socket.SOCK_STREAM),
                                  "launcher": [shim, "--ingest-upstream", "192.168.64.1:18080"],
                                  "cogs": [{"id": "gone", "binary": "/nonexistent/cog"}]})
        self.assertEqual(r["results"][0]["launcher"],
                         ["launch", "--ingest-upstream", "<host>:18080"])

    def test_missing_binary(self):
        doc = harness.run_plan({"cogs": [{"id": "gone", "binary": "/nonexistent/cog"}],
                                "ingest_port": free_port(socket.SOCK_STREAM)})
        self.assertEqual(classify.classify(doc["results"][0]), "missing-binary")


if __name__ == "__main__":
    unittest.main()
