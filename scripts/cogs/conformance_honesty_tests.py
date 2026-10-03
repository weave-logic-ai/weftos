"""Honesty tests for the cog conformance harness (card b179280a).

One test class per card-08 finding. Imported by test_conformance.py, so
`python3 scripts/cogs/test_conformance.py` and `cogs-conformance selftest`
run them. No containers, no network, no real cog binaries: downloads go
through an injected opener and "binaries" are fixture bytes.
"""
import contextlib
import hashlib
import io
import json
import os
import socket
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

ELF = b"\x7fELF\x02\x01\x01" + b"\x00" * 11 + (183).to_bytes(2, "little") + b"\x00" * 40
SHA = hashlib.sha256(ELF).hexdigest()


def raw(cid="a", mode="once", rc=0, ingest=1, timed_out=False, host="aarch64", cycles=None,
        launcher=None):
    cycles = (1 if mode == "once" else 5) if cycles is None else cycles
    r = {"id": cid, "mode": mode, "rc": rc, "ingest_posts": ingest, "status": "ran",
         "timed_out": timed_out, "cycle_ms": 700.0, "cycles": cycles, "host_machine": host,
         "interval_s": 1 if mode == "interval" else None, "feed_id": "f",
         "harness_version": "1", "sha256": "ab" * 32}
    if launcher:
        r["launcher"] = launcher
    return r


NODE = [{"id": "cpu.arch.aarch64", "provenance": "probed"},
        {"id": "runtime.container.docker", "provenance": "claimed"}]


class EmulationIsNotMeasured(unittest.TestCase):
    def test_emulated_container_never_emits_cycle_ms(self):
        # An aarch64 container driven from an x86 host: the harness inside
        # reports aarch64 (qemu), the driver machine tells the truth.
        for runtime in ("docker", "apple-container", "podman"):
            caps = classify.cycle_capabilities([raw()], "aarch64", runtime, "T",
                                               runtime, driver_machine="x86_64")
            self.assertEqual(caps, [], runtime)
        native = classify.cycle_capabilities([raw()], "aarch64", "docker", "T", "docker",
                                             driver_machine="arm64")
        self.assertEqual([c["provenance"] for c in native], ["measured"])

    def test_emulated_run_never_upgrades_node_provenance(self):
        out = classify.upgrade_provenance(NODE, [raw()], "aarch64", "docker", "T", "docker",
                                          driver_machine="x86_64")
        self.assertEqual(out, NODE)
        out = classify.upgrade_provenance(NODE, [raw()], "aarch64", "docker", "T", "docker",
                                          driver_machine="arm64")
        self.assertEqual(out[0]["provenance"], "measured")

    def test_unknown_machine_is_not_native(self):
        self.assertEqual(classify.cycle_capabilities([raw(host=None)], "aarch64", "native",
                                                     "T"), [])
        self.assertEqual(classify.cycle_capabilities([raw()], "aarch64", "docker", "T",
                                                     driver_machine=None), [])

    def test_wrong_host_machine_is_emulated_even_for_native_runtime(self):
        for arch, host in (("aarch64", "x86_64"), ("arm", "x86_64")):
            self.assertEqual(classify.cycle_capabilities(
                [raw(host=host)], arch, "native", "T"), [], (arch, host))
        # a 32-bit arm cog on a real aarch64 node is native; in a container
        # on an arm64 host (Apple silicon has no AArch32) it is not.
        self.assertEqual(len(classify.cycle_capabilities(
            [raw(host="aarch64")], "arm", "native", "T")), 1)
        self.assertEqual(classify.cycle_capabilities(
            [raw(host="armv7l")], "arm", "docker", "T", "docker", driver_machine="arm64"), [])

    def test_sweep_and_probe_report_emulation_and_write_nothing_measured(self):
        real_exec, real_dm = conformance.execute, conformance.driver_machine
        conformance.execute = lambda *_a, **_k: ([raw("anomaly-detect")], {"machine": "x"})
        conformance.driver_machine = lambda *_a, **_k: "x86_64"
        try:
            with tempfile.TemporaryDirectory() as d:
                mf = os.path.join(d, "perf.measured.json")
                out = {}
                for argv in (["sweep", "--results-dir", d, "--label", "x"],
                             ["probe", "--cog", "anomaly-detect", "--out",
                              os.path.join(d, "p.json")]):
                    args = conformance.build_parser().parse_args(
                        argv + ["--runtime", "docker", "--measured-file", mf])
                    with contextlib.redirect_stdout(io.StringIO()):
                        args.fn(args)
                with open(os.path.join(d, "x", "summary.json")) as f:
                    self.assertEqual(json.load(f)["emulated"], ["anomaly-detect"])
                with open(os.path.join(d, "x", "capabilities.json")) as f:
                    self.assertEqual(json.load(f), [])
                with open(os.path.join(d, "p.json")) as f:
                    probe = json.load(f)
                self.assertTrue(probe["emulated"])
                self.assertEqual([c for c in probe["capabilities"]
                                  if c["id"] == "perf.cog.cycle_ms"], [])
                with open(mf) as f:
                    self.assertEqual(json.load(f), [])
        finally:
            conformance.execute, conformance.driver_machine = real_exec, real_dm


class HashVerifiedBinaries(unittest.TestCase):
    @staticmethod
    def opener(body):
        def f(url, timeout):
            return io.BytesIO(body)
        return f

    def fetch(self, d, body, sha=SHA):
        return runtimes.fetch_binary("x", "aarch64", d, expected_sha256=sha,
                                     opener=self.opener(body))

    def test_download_must_match_the_expected_sha256(self):
        with tempfile.TemporaryDirectory() as d:
            tampered = ELF + b"\x00"  # still a valid ELF header, different bytes
            path, why = self.fetch(d, tampered)
            self.assertIsNone(path)
            self.assertIn("sha256 mismatch", why)
            self.assertEqual(os.listdir(d), [], "an unverified download is never written")
            self.assertEqual(self.fetch(d, ELF)[1], None)

    def test_no_expected_hash_means_no_fetch(self):
        calls = []
        with tempfile.TemporaryDirectory() as d:
            for bad in (None, "", "ABC", "0" * 63):
                path, why = runtimes.fetch_binary(
                    "x", "aarch64", d, expected_sha256=bad,
                    opener=lambda *a, **k: calls.append(a))
                self.assertIsNone(path)
                self.assertIn("no expected sha256", why)
        self.assertEqual(calls, [])

    def test_a_tampered_cache_entry_is_discarded_and_refetched(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(self.fetch(d, ELF)[1])
            cached = os.path.join(d, runtimes.binary_name("x", "aarch64"))
            with open(cached, "ab") as f:
                f.write(b"evil")
            path, why = self.fetch(d, b"<html>404")
            self.assertEqual((path, why), (None, "not an ELF file"))
            self.assertFalse(os.path.exists(cached), "the bad cache entry is removed")
            path, why = self.fetch(d, ELF)
            self.assertEqual(runtimes.sha256_file(path), SHA)

    def test_manifest_loader_is_strict(self):
        with tempfile.TemporaryDirectory() as d:
            def write(doc):
                p = os.path.join(d, "m.json")
                with open(p, "w") as f:
                    f.write(doc if isinstance(doc, str) else json.dumps(doc))
                return p
            name = runtimes.binary_name("x", "aarch64")
            self.assertEqual(runtimes.load_hash_manifest(write({name: SHA})), {name: SHA})
            self.assertEqual(runtimes.load_hash_manifest(write({"binaries": {name: SHA}})),
                             {name: SHA})
            for bad in ("not json", [1], {name: "nope"}, {name: SHA.upper()}, {"a/b": SHA},
                        {name: 3}):
                with self.assertRaises(ValueError, msg=bad):
                    runtimes.load_hash_manifest(write(bad))
            with self.assertRaises(ValueError):
                runtimes.load_hash_manifest(os.path.join(d, "missing.json"))

    def test_sweep_refuses_to_download_without_a_manifest(self):
        args = conformance.build_parser().parse_args(["sweep", "--cogs", "x"])
        with self.assertRaises(conformance.InputError):
            conformance.execute(args, {"x": {}}, ["x"], "once")
        # main reports it as one line and exit code 2, no traceback
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = conformance.main(["sweep", "--cogs", "x"])
        self.assertEqual(rc, 2)
        self.assertIn("--sha256-manifest", err.getvalue())

    def test_listed_local_binary_with_the_wrong_hash_is_not_run(self):
        with tempfile.TemporaryDirectory() as d:
            bd = os.path.join(d, "bin")
            os.makedirs(bd)
            name = runtimes.binary_name("x", "aarch64")
            with open(os.path.join(bd, name), "wb") as f:
                f.write(ELF)
            man = os.path.join(d, "m.json")
            with open(man, "w") as f:
                json.dump({name: "0" * 64}, f)
            args = conformance.build_parser().parse_args(
                ["sweep", "--binary-dir", bd, "--sha256-manifest", man,
                 "--cache-dir", os.path.join(d, "cache")])
            results, _ = conformance.execute(args, {"x": {}}, ["x"], "once")
        self.assertEqual(results[0]["status"], "missing-binary")
        self.assertIn("does not match", results[0]["reason"])

    def test_harness_refuses_a_binary_that_changed_after_verification(self):
        with tempfile.TemporaryDirectory() as d:
            cog = os.path.join(d, "cog-x-aarch64")
            with open(cog, "wb") as f:
                f.write(b"#!/bin/sh\necho ran > %s/ran\n" % d.encode())
            os.chmod(cog, 0o755)
            ports = {"udp_port": free_port(socket.SOCK_DGRAM),
                     "ingest_port": free_port(socket.SOCK_STREAM)}
            r = harness.run_plan({"timeout": 5, **ports, "cogs": [
                {"id": "x", "binary": cog, "sha256": "0" * 64}]})["results"][0]
            self.assertFalse(os.path.exists(os.path.join(d, "ran")), "binary was executed")
        self.assertEqual(r["status"], "exec-error")
        self.assertIn("sha256 mismatch", r["stderr_tail"])
        self.assertEqual(classify.classify(r), "exec-error")


class IntervalClean(unittest.TestCase):
    def test_interval_clean_needs_two_cycles_and_still_running(self):
        c = classify.classify
        self.assertEqual(c(raw(mode="interval", rc=None, timed_out=True, ingest=6)), "clean")
        # one POST and then silence until the deadline: stuck, not cycling
        self.assertEqual(c(raw(mode="interval", rc=None, timed_out=True, ingest=1, cycles=1)),
                         "no-output")
        # quit by itself with rc 0 and no launcher stop: not interval behaviour
        self.assertEqual(c(raw(mode="interval", rc=0, ingest=6)), "no-output")
        # stopped by the launcher's --run-secs (rc 0): clean
        self.assertEqual(c(raw(mode="interval", rc=0, ingest=6, launcher=["l"])), "clean")
        self.assertEqual(c(raw(mode="interval", rc=0, ingest=1, cycles=1, launcher=["l"])),
                         "no-output")
        self.assertEqual(c(raw(mode="interval", rc=0, ingest=0, launcher=["l"])), "no-output")
        # a crash is still a cli-error
        self.assertEqual(c(raw(mode="interval", rc=2, ingest=6)), "cli-error")

    def test_once_classification_is_unchanged(self):
        c = classify.classify
        self.assertEqual(c(raw()), "clean")
        self.assertEqual(c(raw(ingest=0)), "no-output")
        self.assertEqual(c(raw(rc=None, timed_out=True, ingest=3)), "no-output")

    def test_committed_baseline_is_unchanged(self):
        base = conformance._load_json(conformance.BASELINE)
        counts = base["group_counts"]
        self.assertEqual((counts["clean"], counts["needs-interval"], counts["needs-extra-cli"]),
                         (93, 5, 9))


class MalformedInput(unittest.TestCase):
    def run_main(self, argv):
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            rc = conformance.main(argv)
        return rc, err.getvalue()

    def probe(self, node_facts_text):
        real = conformance.execute
        conformance.execute = lambda *_a, **_k: ([raw("anomaly-detect")], {"machine": "x"})
        try:
            with tempfile.TemporaryDirectory() as d:
                facts = os.path.join(d, "facts.json")
                with open(facts, "w") as f:
                    f.write(node_facts_text)
                return self.run_main(["probe", "--cog", "anomaly-detect", "--node-facts", facts])
        finally:
            conformance.execute = real

    def test_malformed_node_facts_are_a_clean_error(self):
        for text in ("{not json", "", '{"capabilities": "x"}', "42", '[1, 2]',
                     '[{"provenance": "probed"}]', '[{"id": "a", "attrs": [1]}]'):
            rc, err = self.probe(text)
            self.assertEqual(rc, 2, text)
            self.assertTrue(err.startswith("error: "), (text, err))
            self.assertNotIn("Traceback", err)
            self.assertEqual(err.count("\n"), 1, err)

    def test_missing_files_and_bad_results_are_a_clean_error(self):
        with tempfile.TemporaryDirectory() as d:
            missing = os.path.join(d, "nope.json")
            bad = os.path.join(d, "bad.json")
            with open(bad, "w") as f:
                json.dump({"results": [{"id": "../x"}]}, f)
            for argv in (["summarize", missing], ["summarize", bad],
                         ["probe", "--cog", "x", "--expectations", missing]):
                rc, err = self.run_main(argv)
                self.assertEqual(rc, 2, argv)
                self.assertTrue(err.startswith("error: "), (argv, err))

    def test_well_formed_probe_input_still_succeeds(self):
        rc, err = self.probe(json.dumps({"capabilities": NODE}))
        self.assertEqual((rc, err), (0, ""))


class LocalBinaryTrust(unittest.TestCase):
    def sweep_args(self, d, *extra):
        return conformance.build_parser().parse_args(
            ["sweep", "--binary-dir", d, "--cache-dir", os.path.join(d, "cache"), *extra])

    def test_binary_dir_without_a_manifest_needs_insecure_local(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(conformance.InputError) as cm:
                conformance.execute(self.sweep_args(d), {"x": {}}, ["x"], "once")
            self.assertIn("--insecure-local", str(cm.exception))
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                results, _ = conformance.execute(
                    self.sweep_args(d, "--insecure-local"), {"x": {}}, ["x"], "once")
            self.assertIn("NO hash verification", err.getvalue())
            self.assertEqual(results[0]["status"], "missing-binary")

    def test_a_manifest_refuses_binaries_it_does_not_list(self):
        with tempfile.TemporaryDirectory() as d:
            for cid in ("x", "y"):
                with open(os.path.join(d, runtimes.binary_name(cid, "aarch64")), "wb") as f:
                    f.write(ELF)
            man = os.path.join(d, "m.json")
            with open(man, "w") as f:
                json.dump({runtimes.binary_name("x", "aarch64"): SHA}, f)
            real = runtimes.make_adapter
            runtimes.make_adapter = lambda *a, **k: type(
                "A", (), {"binary_root": lambda s, w: "/w", "run": lambda s, w, timeout: (
                    json.dump({"results": [], "host": None},
                              open(os.path.join(w, "results.json"), "w")) and None
                    or os.path.join(w, "results.json"))})()
            try:
                args = self.sweep_args(d, "--sha256-manifest", man)
                results, _ = conformance.execute(args, {"x": {}, "y": {}}, ["x", "y"], "once")
            finally:
                runtimes.make_adapter = real
        by = {r["id"]: r for r in results if r["status"] == "missing-binary"}
        self.assertEqual(list(by), ["y"])
        self.assertIn("not listed", by["y"]["reason"])


def free_port(kind):
    import socket
    with socket.socket(socket.AF_INET, kind) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class ExecutedCopy(unittest.TestCase):
    def test_the_binary_runs_from_a_private_read_only_copy(self):
        with tempfile.TemporaryDirectory() as d:
            marker = os.path.join(d, "marker")
            cog = os.path.join(d, "cog-x-aarch64")
            with open(cog, "w") as f:
                f.write('#!/bin/sh\necho "$0" > %s\n[ -w "$0" ] && echo writable >> %s '
                        '|| echo readonly >> %s\n' % ((marker,) * 3))
            os.chmod(cog, 0o755)
            ports = {"udp_port": free_port(socket.SOCK_DGRAM),
                     "ingest_port": free_port(socket.SOCK_STREAM)}
            r = harness.run_plan({"timeout": 5, **ports, "cogs": [
                {"id": "x", "binary": cog, "sha256": hashlib.sha256(
                    open(cog, "rb").read()).hexdigest()}]})["results"][0]
            self.assertEqual(r["status"], "ran")
            ran_path, perm = open(marker).read().split("\n")[:2]
            self.assertNotEqual(ran_path, cog)
            self.assertIn("cog-run-", ran_path)
            self.assertEqual(perm, "readonly")
            self.assertFalse(os.path.exists(ran_path), "the private copy is removed")
            self.assertEqual(os.path.basename(ran_path), "cog-x-aarch64")
            # the hash is of the copy: a mismatch still stops it
            os.unlink(marker)
            bad = harness.run_plan({"timeout": 5, **ports, "cogs": [
                {"id": "x", "binary": cog, "sha256": "0" * 64}]})["results"][0]
            self.assertEqual(bad["status"], "exec-error")
            self.assertFalse(os.path.exists(marker))


class EngineArch(unittest.TestCase):
    class P:
        def __init__(self, out="", rc=0):
            self.stdout, self.returncode = out, rc

    def dm(self, runner, runtime="docker", docker_host="tcp://engine:2375"):
        env = dict(os.environ)
        env.pop("DOCKER_HOST", None)
        if docker_host:
            env["DOCKER_HOST"] = docker_host
        old, os.environ = os.environ, env
        try:
            return conformance.engine_machine(runtime, runner)
        finally:
            os.environ = old

    def test_a_remote_docker_engine_is_asked_for_its_architecture(self):
        seen = []

        def runner(cmd, **k):
            seen.append(cmd)
            return self.P("amd64\n")
        self.assertEqual(self.dm(runner), "x86_64")
        self.assertEqual(seen[0], ["docker", "info", "--format", "{{.Architecture}}"])
        self.assertEqual(self.dm(lambda c, **k: self.P("aarch64\n")), "aarch64")

    def test_a_failed_query_is_not_native(self):
        self.assertIsNone(self.dm(lambda c, **k: self.P("", 1)))
        self.assertIsNone(self.dm(lambda c, **k: self.P("  ", 0)))

        def boom(c, **k):
            raise OSError("no docker")
        self.assertIsNone(self.dm(boom))
        caps = classify.cycle_capabilities([raw()], "aarch64", "docker", "T", "docker",
                                           driver_machine=None)
        self.assertEqual(caps, [])

    def test_no_query_for_a_local_engine_or_another_runtime(self):
        def runner(c, **k):
            raise AssertionError("must not query")
        import platform
        self.assertEqual(self.dm(runner, docker_host=None), platform.machine())
        self.assertEqual(self.dm(runner, runtime="podman"), platform.machine())
