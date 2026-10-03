#!/usr/bin/env python3
"""Cog conformance harness driver (card mesh-placement-08; ADR-100; ADR-099 s2-s3).

Entry point: `scripts/build.sh cogs-conformance <command> [options]`.

Commands:
  sweep     run many cogs on one runtime + arch; write results/<label>/
            {results.json, summary.json, capabilities.json}
  probe     admission probe for one cog: run it in its expected mode and emit
            perf.cog.cycle_ms (provenance measured); with --node-facts, also
            upgrade the exercised arch / runtime capabilities to measured
  summarize re-classify an existing results.json against expectations
  selftest  run the unit tests (no containers, no network)

With --launcher (the `cog_adapter_run` example binary) and --adapter-runtime,
each cog runs through a WeftOS WorkloadRuntime adapter (native, docker, apple,
podman) under WorkloadHost governance instead of being spawned by the harness;
--runtime still says where the harness itself runs.

Docs: docs/cogs/conformance-harness.md
"""
import argparse
import datetime
import json
import os
import platform
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import classify  # noqa: E402
import runtimes  # noqa: E402

EXPECTATIONS = os.path.join(HERE, "expectations.json")
BASELINE = os.path.join(HERE, "baseline", "aarch64-once-2026-09-28.json")
RESULTS_DIR = os.path.join(HERE, "results")
CACHE_DIR = os.path.join(HERE, ".cache")


class InputError(Exception):
    """Malformed or unreadable input: reported as one line, exit code 2."""


def _load_json(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError) as e:
        raise InputError("%s: %s" % (path, e))


def driver_machine():
    """Machine of the host that drives the harness (and any container engine)."""
    return platform.machine()


def _write_json(path, doc):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(doc, f, indent=1, sort_keys=True)
        f.write("\n")


MEASURED_KEY_ATTRS = ("cog_id", "arch", "runtime")


def _measured_key(cap):
    attrs = cap.get("attrs") or {}
    return (cap.get("id"),) + tuple(attrs.get(k) for k in MEASURED_KEY_ATTRS)


def merge_measured(path, caps):
    """Merge measured perf.* capabilities into a daemon `perf.measured.json`.

    Only `perf.*` entries with provenance `measured` are written (the daemon
    keeps nothing else). A new entry replaces an older one for the same
    capability id, cog, arch and runtime; every other entry is left alone.
    The file is replaced atomically. Returns the number of entries written.
    """
    fresh = [c for c in caps if str(c.get("id", "")).startswith("perf.")
             and c.get("provenance") == "measured"]
    old = []
    if os.path.exists(path):
        doc = _load_json(path)
        old = doc.get("capabilities") if isinstance(doc, dict) else doc
        if not isinstance(old, list):
            raise SystemExit("%s: expected a list or {capabilities:[...]}" % path)
    replaced = {_measured_key(c) for c in fresh}
    merged = [c for c in old if _measured_key(c) not in replaced] + fresh
    merged.sort(key=lambda c: tuple(str(x) for x in _measured_key(c)))
    d = os.path.dirname(os.path.abspath(path))
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".perf.measured.")
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(merged, f, indent=1, sort_keys=True)
            f.write("\n")
        os.replace(tmp, path)
    except BaseException:
        os.unlink(tmp)
        raise
    return len(fresh)


def _now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def select_cogs(expectations, cogs_arg):
    if not cogs_arg:
        return sorted(expectations)
    ids = [c.strip() for c in cogs_arg.split(",") if c.strip()]
    bad = [c for c in ids if not classify.valid_cog_id(c)]
    if bad:
        raise SystemExit("invalid cog id(s): %s" % ", ".join(bad))
    return ids


ADAPTER_ARCH = {"aarch64": "aarch64", "arm": "armv7"}
LAUNCHER_NAME = "cog_adapter_run"


def launcher_plan(args, root):
    """Launcher argv for the plan (None without --launcher). The launcher
    binary is staged at <root>/bin/cog_adapter_run."""
    if not getattr(args, "launcher", None):
        return None
    if not args.adapter_runtime:
        raise SystemExit("--launcher needs --adapter-runtime")
    argv = ["%s/bin/%s" % (root, LAUNCHER_NAME), "--runtime", args.adapter_runtime,
            "--arch", ADAPTER_ARCH[args.arch], "--timeout-secs", str(int(args.timeout)),
            # interval runs stop themselves before the harness deadline
            "--run-secs", str(max(1, int(args.timeout) - 4))]
    for flag, value in (("--base-image", args.adapter_base_image),
                        ("--network", args.adapter_network),
                        ("--feed-port", args.adapter_feed_port),
                        ("--ingest-upstream", args.adapter_ingest_upstream),
                        ("--run-as", args.adapter_run_as)):
        if value is not None:
            argv += [flag, str(value)]
    if args.adapter_runtime != "native" and not args.adapter_base_image:
        raise SystemExit("container adapters need --adapter-base-image name@sha256:...")
    return argv


def execute(args, expectations, ids, sweep_mode):
    """Fetch binaries, stage, run through the adapter; return raw results."""
    cache = os.path.join(args.cache_dir, args.arch)
    manifest = None
    if args.sha256_manifest:
        try:
            manifest = runtimes.load_hash_manifest(args.sha256_manifest)
        except ValueError as e:
            raise InputError(str(e))
    if manifest is None and not args.binary_dir:
        raise InputError("downloaded cog binaries must be verified: pass --sha256-manifest "
                         "(a JSON map of cog-<id>-<arch> to sha256), or --binary-dir")
    specs, missing, binaries = [], [], []
    for cid in ids:
        name = runtimes.binary_name(cid, args.arch)
        expected = (manifest or {}).get(name)
        path, why = None, "no local binary"
        if args.binary_dir:
            cand = os.path.join(args.binary_dir, name)
            if os.path.isfile(cand):
                path, why = cand, None
                if expected and runtimes.sha256_file(cand) != expected:
                    path, why = None, "sha256 does not match the manifest"
        else:
            path, why = runtimes.fetch_binary(cid, args.arch, cache, expected_sha256=expected)
        if path is None:
            missing.append({"id": cid, "status": "missing-binary", "rc": None,
                            "timed_out": False, "mode": "once", "reason": why})
            continue
        binaries.append(path)
        spec = classify.plan_spec(cid, expectations.get(cid, {}), sweep_mode)
        if expected:
            spec["sha256"] = expected  # the harness re-checks it before running
        specs.append(spec)
    adapter = runtimes.make_adapter(args.runtime, args.arch, ssh_host=args.ssh_host,
                                    sudo=args.sudo, image=args.image,
                                    engine_args=args.harness_engine_arg or (),
                                    remote_dir=getattr(args, "remote_dir", None))
    workdir = tempfile.mkdtemp(prefix="run-", dir=_ensure(args.cache_dir))
    try:
        runtimes.stage_workdir(workdir, os.path.join(HERE, "harness.py"), binaries)
        root = adapter.binary_root(workdir)
        for spec in specs:
            spec["binary"] = "%s/bin/%s" % (root, runtimes.binary_name(spec["id"], args.arch))
        plan = {"timeout": args.timeout, "feed": args.feed, "cogs": specs,
                "udp_port": args.udp_port, "ingest_port": args.ingest_port,
                "ingest_bind": args.ingest_bind}
        launcher = launcher_plan(args, root)
        if launcher:
            dst = os.path.join(workdir, "bin", LAUNCHER_NAME)
            shutil.copy2(args.launcher, dst)
            os.chmod(dst, 0o755)
            plan["launcher"] = launcher
        _write_json(os.path.join(workdir, "plan.json"), plan)
        doc = {"results": [], "host": None}
        if specs:
            out = adapter.run(workdir, timeout=args.timeout * len(specs) + 300)
            doc = _load_json(out)
    finally:
        if not args.keep_workdir:
            shutil.rmtree(workdir, ignore_errors=True)
    by_id = {r["id"]: r for r in doc["results"] + missing}
    return [by_id[c] for c in ids if c in by_id], doc.get("host")


# --adapter-runtime value -> classify runtime key (the runtime that ran the cog).
ADAPTER_RUNTIME = {"native": "native", "docker": "docker", "apple": "apple-container",
                   "podman": "podman"}


def measured_runtime(args):
    """The runtime a measurement belongs to: the WeftOS adapter that ran the
    cog when --launcher is used, otherwise the harness runtime itself."""
    if getattr(args, "launcher", None):
        return ADAPTER_RUNTIME[args.adapter_runtime]
    return args.runtime


def _ensure(d):
    os.makedirs(d, exist_ok=True)
    return d


def cmd_sweep(args):
    expectations = classify.validate_expectations(_load_json(args.expectations))
    ids = select_cogs(expectations, args.cogs)
    results, host = execute(args, expectations, ids, args.mode)
    measured_at = _now()
    summary = classify.summarize(results, expectations, args.mode)
    runtime = measured_runtime(args)
    summary.update(runtime=runtime, harness_runtime=args.runtime, arch=args.arch,
                   feed=args.feed, timeout_s=args.timeout, measured_at=measured_at, host=host)
    label = args.label or "%s-%s-%s" % (runtime, args.arch, args.mode)
    out_dir = os.path.join(args.results_dir, label)
    dm = driver_machine()
    caps = classify.cycle_capabilities(results, args.arch, runtime, measured_at, args.runtime, dm)
    summary["emulated"] = classify.emulated_ids(results, args.arch, args.runtime, dm)
    _write_json(os.path.join(out_dir, "results.json"), {"results": results, "host": host})
    _write_json(os.path.join(out_dir, "summary.json"), summary)
    _write_json(os.path.join(out_dir, "capabilities.json"), caps)
    if args.measured_file:
        merge_measured(args.measured_file, caps)
    print(json.dumps({"label": label, "group_counts": summary["group_counts"],
                      "by_outcome": summary["by_outcome"],
                      "unexpected": summary["unexpected"]}, indent=1))
    rc = 0
    if args.check_baseline:
        problems = classify.compare_baseline(summary, _load_json(args.baseline))
        for p in problems:
            print("BASELINE MISMATCH: " + p)
        print("baseline: %s" % ("MATCH" if not problems else "MISMATCH"))
        rc = 1 if problems else 0
    elif summary["unexpected"]:
        rc = 1
    return rc


def cmd_probe(args):
    expectations = classify.validate_expectations(_load_json(args.expectations))
    if not classify.valid_cog_id(args.cog):
        raise SystemExit("invalid cog id")
    results, _ = execute(args, expectations, [args.cog], "expected")
    measured_at = _now()
    node_caps = []
    if args.node_facts:
        facts = _load_json(args.node_facts)
        node_caps = facts.get("capabilities", []) if isinstance(facts, dict) else facts
        if not isinstance(node_caps, list):
            raise InputError("node facts: expected a list or {capabilities:[...]}")
    dm = driver_machine()
    caps = classify.upgrade_provenance(node_caps, results, args.arch, measured_runtime(args),
                                       measured_at, args.runtime, dm)
    doc = {"cog": args.cog, "outcome": classify.classify(results[0]),
           "emulated": bool(classify.emulated_ids(results, args.arch, args.runtime, dm)),
           "result": results[0], "capabilities": caps}
    if args.measured_file:
        merge_measured(args.measured_file, caps)
    text = json.dumps(doc, indent=1, sort_keys=True)
    if args.out:
        with open(args.out, "w") as f:
            f.write(text + "\n")
    print(text)
    return 0 if doc["outcome"] == "clean" else 1


def cmd_summarize(args):
    expectations = classify.validate_expectations(_load_json(args.expectations))
    doc = _load_json(args.results)
    results = doc.get("results") if isinstance(doc, dict) else None
    if not isinstance(results, list) or not all(
            isinstance(r, dict) and classify.valid_cog_id(r.get("id")) for r in results):
        raise InputError("%s: expected {results:[{id,...}]}" % args.results)
    summary = classify.summarize(results, expectations, args.mode)
    print(json.dumps(summary, indent=1, sort_keys=True))
    if args.check_baseline:
        problems = classify.compare_baseline(summary, _load_json(args.baseline))
        for p in problems:
            print("BASELINE MISMATCH: " + p)
        return 1 if problems else 0
    return 0


def cmd_selftest(_args):
    import unittest
    suite = unittest.defaultTestLoader.discover(HERE, pattern="test_*.py")
    ok = unittest.TextTestRunner(verbosity=1).run(suite).wasSuccessful()
    return 0 if ok else 1


def _runner_opts(p):
    p.add_argument("--runtime", choices=sorted(runtimes.ADAPTERS), default="docker")
    p.add_argument("--arch", choices=sorted(runtimes.URL_ARCH), default="aarch64")
    p.add_argument("--feed", choices=("features", "vitals", "both"), default="features",
                   help="UDP feed: features (0xC5110003, baseline), vitals (0xC5110002) or both")
    p.add_argument("--timeout", type=float, default=15.0, help="per-cog cap in seconds")
    p.add_argument("--image", default=runtimes.DEFAULT_IMAGE)
    p.add_argument("--ssh-host", help="remote node for --runtime ssh "
                   "(default $COG_HARNESS_SSH_HOST)")
    p.add_argument("--remote-dir", help="ssh: work dir relative to the remote login "
                   "directory (default %s); removed and recreated per run"
                   % runtimes.SshAdapter.REMOTE_DIR)
    p.add_argument("--sudo", action="store_true",
                   help="ssh/native: run the harness via sudo -n (ingest stub binds :80)")
    p.add_argument("--binary-dir", help="use local binaries instead of downloading")
    p.add_argument("--sha256-manifest", help="JSON map of cog-<id>-<arch> to the expected "
                   "sha256 (from the registry or a package manifest). Downloaded binaries "
                   "must match it; local binaries are checked when listed")
    p.add_argument("--cache-dir", default=CACHE_DIR)
    p.add_argument("--expectations", default=EXPECTATIONS)
    p.add_argument("--keep-workdir", action="store_true")
    p.add_argument("--launcher", help="cog_adapter_run binary: run cogs through a WeftOS "
                   "runtime adapter (built for the node the harness runs on)")
    p.add_argument("--adapter-runtime", choices=("native", "docker", "apple", "podman"))
    p.add_argument("--adapter-base-image", help="digest-pinned base for container adapters")
    p.add_argument("--adapter-network", help="container network (e.g. host on OrbStack)")
    p.add_argument("--adapter-feed-port", type=int, help="host UDP port published to the feed")
    p.add_argument("--adapter-ingest-upstream", metavar="IP:PORT",
                   help="container adapters: relay the cog's 127.0.0.1:80 to this "
                   "ingest address (e.g. the Apple container VM gateway and --ingest-port)")
    p.add_argument("--adapter-run-as", help="UID:GID for native when the harness runs as root")
    p.add_argument("--udp-port", type=int, default=5006,
                   help="port the harness feed sends to (a published port for "
                   "container adapters driven from the host)")
    p.add_argument("--ingest-port", type=int, default=80,
                   help="port of the harness ingest stub (cogs post to 80)")
    p.add_argument("--ingest-bind", default="127.0.0.1",
                   help="address the harness ingest stub binds (the VM gateway, or "
                   "0.0.0.0, when a container relays ingest back to this host)")
    p.add_argument("--harness-engine-arg", action="append",
                   help="extra `docker run` / `container run` argument for the harness "
                   "container (repeatable), e.g. --net=host")


MEASURED_HELP = ("merge the measured perf.* capabilities into this node's "
                 "perf.measured.json (the file the daemon reads at its next facts "
                 "probe); other entries are kept, same cog/arch/runtime is replaced")


def build_parser():
    ap = argparse.ArgumentParser(prog="cogs-conformance", description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="command", required=True)
    s = sub.add_parser("sweep", help="run many cogs on one runtime + arch")
    _runner_opts(s)
    s.add_argument("--mode", choices=("once", "expected"), default="once",
                   help="once: every cog --once (baseline); expected: per-cog run mode")
    s.add_argument("--cogs", help="comma-separated ids (default: all in expectations)")
    s.add_argument("--label", help="results/<label>/ (default runtime-arch-mode)")
    s.add_argument("--results-dir", default=RESULTS_DIR)
    s.add_argument("--check-baseline", action="store_true")
    s.add_argument("--baseline", default=BASELINE)
    s.add_argument("--measured-file", help=MEASURED_HELP)
    s.set_defaults(fn=cmd_sweep)
    p = sub.add_parser("probe", help="admission probe for one cog")
    _runner_opts(p)
    p.add_argument("--cog", required=True)
    p.add_argument("--node-facts", help="JSON capabilities to upgrade")
    p.add_argument("--out")
    p.add_argument("--measured-file", help=MEASURED_HELP)
    p.set_defaults(fn=cmd_probe)
    m = sub.add_parser("summarize", help="re-classify a results.json")
    m.add_argument("results")
    m.add_argument("--mode", choices=("once", "expected"), default="once")
    m.add_argument("--expectations", default=EXPECTATIONS)
    m.add_argument("--check-baseline", action="store_true")
    m.add_argument("--baseline", default=BASELINE)
    m.set_defaults(fn=cmd_summarize)
    t = sub.add_parser("selftest", help="run unit tests")
    t.set_defaults(fn=cmd_selftest)
    return ap


def main(argv=None):
    args = build_parser().parse_args(argv)
    if getattr(args, "timeout", 1) <= 0 or getattr(args, "timeout", 1) > 600:
        raise SystemExit("--timeout must be in (0, 600]")
    try:
        return args.fn(args)
    except (InputError, ValueError) as e:
        print("error: %s" % e, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
