"""Pure classification, summary, baseline and capability logic for cog conformance.

No I/O beyond the explicit loaders. Everything here is unit-tested in
scripts/cogs/test_conformance.py.

Outcomes (per raw harness result):
  clean          ran, produced >= 1 ingest POST, and exited 0 (--once); or, for
                 --interval, was still cycling at the deadline (or stopped by
                 the launcher's --run-secs) after at least two cycle events
  no-output      ran without error but produced no ingest POST
  cli-error      exited non-zero (unknown flag, missing peer, etc.)
  missing-binary no binary for this arch (e.g. registry 404)
  exec-error     the binary could not be executed on this runtime / arch

Groups (the ADR-100 catalog taxonomy, from expectations.json):
  clean | needs-interval | needs-extra-cli | no-build
"""
import json
import re

OUTCOMES = ("clean", "no-output", "cli-error", "missing-binary", "exec-error")
GROUPS = ("clean", "needs-interval", "needs-extra-cli", "no-build")
RUN_MODES = ("once", "interval")
COG_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
ARCHES = ("aarch64", "arm")
ARCH_CAPABILITY = {"aarch64": "cpu.arch.aarch64", "arm": "cpu.arch.armv7"}
RUNTIME_CAPABILITY = {
    "docker": "runtime.container.docker",
    "apple-container": "runtime.container.apple",
    "podman": "runtime.container.podman",
    "native": "runtime.native",
    "ssh": "runtime.native",
}
PROVENANCE_RANK = {"claimed": 0, "probed": 1, "measured": 2}
CONTAINER_RUNTIMES = ("docker", "apple-container", "podman")
# uname machines that execute a cog binary of each arch without emulation.
NATIVE_MACHINES = {"aarch64": ("aarch64", "arm64"), "arm": ("armv7l", "armv8l", "armv7")}


def valid_cog_id(cid):
    return isinstance(cid, str) and bool(COG_ID_RE.match(cid))


def classify(result):
    """Map one raw harness result to an outcome."""
    status = result.get("status")
    if status in ("missing-binary", "exec-error"):
        return status
    rc = result.get("rc")
    if rc not in (0, None):
        return "cli-error"
    if rc is None and not result.get("timed_out"):
        return "exec-error"
    if result.get("mode") == "once" and rc != 0:
        # a --once run that had to be killed never completed a cycle
        return "no-output"
    posts = (result.get("ingest_posts") or 0) >= 1
    if result.get("mode") == "interval":
        # An interval cog must keep cycling: still running at the deadline, or
        # stopped by the launcher (rc 0 with a launcher). One POST and then
        # silence, or a cog that quit by itself, is not interval behaviour.
        running = bool(result.get("timed_out")) or (rc == 0 and bool(result.get("launcher")))
        return "clean" if running and posts and (result.get("cycles") or 0) >= 2 \
            else "no-output"
    return "clean" if posts else "no-output"


def native_run(result, arch, harness_runtime, driver_machine=None):
    """True only when the cog demonstrably ran on matching hardware.

    A binary run under emulation (an aarch64 container on an x86 host, an
    armv7 container on Apple silicon) says nothing about the node's real cycle
    time, so its result must never become measured provenance. Unknown
    machines count as not native: nothing is upgraded on a guess.

    `result["host_machine"]` is what the harness saw (inside the container
    that already reports the emulated machine); for a container harness the
    machine of the host driving the engine is checked too. A raw node
    (native / ssh) may run 32-bit arm on an aarch64 kernel.
    """
    ok = NATIVE_MACHINES[arch]
    machines = [result.get("host_machine")]
    if harness_runtime in CONTAINER_RUNTIMES:
        machines.append(driver_machine)
    elif arch == "arm":
        ok = ok + ("aarch64", "arm64")
    return all(isinstance(m, str) and m.lower() in ok for m in machines)


def emulated_ids(results, arch, harness_runtime, driver_machine=None):
    """Ids of results that did not run natively (see native_run)."""
    return sorted(r["id"] for r in results
                  if not native_run(r, arch, harness_runtime, driver_machine))


# ── Expectations ────────────────────────────────────────────────────────────

def validate_expectations(doc):
    """Validate expectations.json; return the cogs mapping or raise ValueError."""
    if not isinstance(doc, dict) or not isinstance(doc.get("cogs"), dict):
        raise ValueError("expectations: top-level 'cogs' object required")
    cogs = doc["cogs"]
    for cid, e in cogs.items():
        if not valid_cog_id(cid):
            raise ValueError("expectations: bad cog id %r" % cid)
        if not isinstance(e, dict):
            raise ValueError("expectations[%s]: object required" % cid)
        if e.get("group") not in GROUPS:
            raise ValueError("expectations[%s]: group must be one of %s" % (cid, GROUPS))
        if e.get("run_mode", "once") not in RUN_MODES:
            raise ValueError("expectations[%s]: bad run_mode" % cid)
        iv = e.get("interval", 1)
        if not isinstance(iv, int) or not 1 <= iv <= 3600:
            raise ValueError("expectations[%s]: interval must be int 1..3600" % cid)
        if e.get("once_outcome", "clean") not in OUTCOMES:
            raise ValueError("expectations[%s]: bad once_outcome" % cid)
        needs = e.get("needs", [])
        if not isinstance(needs, list) or not all(isinstance(n, str) for n in needs):
            raise ValueError("expectations[%s]: needs must be a list of strings" % cid)
        if e["group"] == "needs-interval" and e.get("run_mode") != "interval":
            raise ValueError("expectations[%s]: needs-interval implies run_mode interval" % cid)
    return cogs


def plan_spec(cid, expectation, mode):
    """Harness spec for one cog. mode: 'once' (baseline sweep) or 'expected'."""
    if mode == "once":
        return {"id": cid, "mode": "once"}
    if mode != "expected":
        raise ValueError("sweep mode must be once or expected")
    spec = {"id": cid, "mode": expectation.get("run_mode", "once")}
    if spec["mode"] == "interval":
        spec["interval"] = expectation.get("interval", 1)
    return spec


# ── Summary and baseline ────────────────────────────────────────────────────

def summarize(results, expectations, sweep_mode="once"):
    """Summarize raw results against expectations.

    For a --once sweep the expected per-cog outcome is `once_outcome`; for an
    'expected' sweep (each cog in its expected run mode) the needs-interval
    group is expected clean and needs-extra-cli keeps its once_outcome.
    """
    outcomes = {}
    groups = {g: [] for g in GROUPS + ("regressed", "unknown")}
    unexpected = []
    for r in results:
        cid = r["id"]
        o = classify(r)
        outcomes[cid] = o
        exp = expectations.get(cid)
        if exp is None:
            unexpected.append({"id": cid, "observed": o, "expected": None,
                               "why": "no expectation"})
            groups["clean" if o == "clean" else "unknown"].append(cid)
            continue
        want = exp.get("once_outcome", "clean")
        if sweep_mode == "expected" and exp["group"] == "needs-interval":
            want = "clean"
        if o == "clean":
            groups["clean"].append(cid)
        elif exp["group"] == "clean":
            groups["regressed"].append(cid)
        else:
            groups[exp["group"]].append(cid)
        if o != want:
            unexpected.append({"id": cid, "observed": o, "expected": want})
    by_outcome = {k: sum(1 for v in outcomes.values() if v == k) for k in OUTCOMES}
    swept = [r for r in results if classify(r) != "missing-binary"]
    return {
        "sweep_mode": sweep_mode,
        "total": len(results),
        "swept": len(swept),
        "by_outcome": by_outcome,
        "groups": {g: sorted(v) for g, v in groups.items()},
        "group_counts": {g: len(v) for g, v in groups.items()},
        "unexpected": sorted(unexpected, key=lambda u: u["id"]),
        "outcomes": dict(sorted(outcomes.items())),
    }


def compare_baseline(summary, baseline):
    """Return a list of human-readable mismatches (empty == matches)."""
    problems = []
    for g, want in baseline.get("group_counts", {}).items():
        got = summary["group_counts"].get(g)
        if got != want:
            problems.append("group %s: got %s, baseline %s" % (g, got, want))
    for cid, want in baseline.get("outcomes", {}).items():
        got = summary["outcomes"].get(cid)
        if got != want:
            problems.append("cog %s: got %s, baseline %s" % (cid, got, want))
    for cid in summary["outcomes"]:
        if cid not in baseline.get("outcomes", {}):
            problems.append("cog %s: not in baseline" % cid)
    if summary["unexpected"]:
        problems.append("unexpected outcomes: %s" % ", ".join(
            u["id"] for u in summary["unexpected"]))
    return problems


def parse_legacy_summary(text):
    """Parse the 2026-09-28 prototype summary.txt (id|rc|ing=N|out=N|stderr)."""
    outcomes = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        parts = line.split("|")
        if len(parts) < 4 or not parts[2].startswith("ing="):
            raise ValueError("bad legacy summary line: %r" % line)
        cid, rc, ing = parts[0], int(parts[1]), int(parts[2][4:])
        if rc != 0:
            outcomes[cid] = "cli-error"
        else:
            outcomes[cid] = "clean" if ing >= 1 else "no-output"
    return outcomes


# ── Capabilities (ADR-099 section 2 shape) ──────────────────────────────────

def cycle_capabilities(results, arch, runtime, measured_at, harness_runtime=None,
                       driver_machine=None):
    """perf.cog.cycle_ms capabilities (provenance measured) for clean results.

    `runtime` is the runtime that ran the cog (the WeftOS adapter when a
    launcher was used); `harness_runtime`, when it differs, records where the
    harness itself ran (e.g. an aarch64 container standing in for a Linux node).
    Results that did not run natively (native_run) emit nothing: an emulated
    cycle time is not this node's.
    """
    caps = []
    for r in results:
        if classify(r) != "clean" or r.get("cycle_ms") is None:
            continue
        if not native_run(r, arch, harness_runtime or runtime, driver_machine):
            continue
        caps.append({
            "id": "perf.cog.cycle_ms",
            "attrs": {"cog_id": r["id"], "value": r["cycle_ms"], "mode": r["mode"],
                      "interval_s": r.get("interval_s"), "samples": r.get("cycles"),
                      "feed": r.get("feed_id"), "arch": arch, "runtime": runtime,
                      "harness_version": r.get("harness_version"),
                      **({"harness_runtime": harness_runtime}
                         if harness_runtime and harness_runtime != runtime else {}),
                      "sha256": r.get("sha256"), "measured_at": measured_at},
            "provenance": "measured", "state": "available", "exclusive": False})
    return caps


def upgrade_provenance(node_caps, results, arch, runtime, measured_at, harness_runtime=None,
                       driver_machine=None):
    """Return node capabilities with the exercised arch/runtime ids upgraded.

    Only upgrades when at least one cog ran clean and natively (not emulated)
    on that arch + runtime, and never downgrades. Capabilities the run did not exercise are untouched.
    Adds the perf.cog.cycle_ms capabilities (replacing older ones for the same
    cog / arch / runtime).
    """
    if arch not in ARCH_CAPABILITY or runtime not in RUNTIME_CAPABILITY:
        raise ValueError("unknown arch or runtime")
    exercised = {ARCH_CAPABILITY[arch], RUNTIME_CAPABILITY[runtime]}
    hr = harness_runtime or runtime
    any_clean = any(classify(r) == "clean" and native_run(r, arch, hr, driver_machine)
                    for r in results)
    fresh = cycle_capabilities(results, arch, runtime, measured_at, harness_runtime,
                               driver_machine)
    fresh_keys = {(c["attrs"]["cog_id"], arch, runtime) for c in fresh}
    out = []
    for cap in node_caps:
        if not isinstance(cap, dict) or not isinstance(cap.get("id"), str):
            raise ValueError("node capability must be an object with an id")
        if not isinstance(cap.get("attrs") or {}, dict):
            raise ValueError("node capability %s: attrs must be an object" % cap["id"])
        cap = json.loads(json.dumps(cap))  # deep copy
        a = cap.get("attrs") or {}
        if cap["id"] == "perf.cog.cycle_ms" and (
                a.get("cog_id"), a.get("arch"), a.get("runtime")) in fresh_keys:
            continue
        cur = PROVENANCE_RANK.get(cap.get("provenance"), -1)
        if any_clean and cap["id"] in exercised and cur < PROVENANCE_RANK["measured"]:
            cap["provenance"] = "measured"
            cap.setdefault("attrs", {})["measured_by"] = "cog-conformance"
            cap["attrs"]["measured_at"] = measured_at
        out.append(cap)
    return out + fresh
