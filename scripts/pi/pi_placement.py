"""Two-node placement stage of the Pi lane (card mesh-placement-12).

The Mac places the released anomaly-detect cog through the placement
control plane; the Pi 5 runs an isolated `workload_node serve` (its own
runtime dir under the scratch dir, mesh port 9471, Noise XX; the system
weaver on :9470 is never touched). The package is fetched from the Mac
over the same connection before load, admitted by the Pi's native adapter
and run against a synthetic feed on the Pi. The controller then pins the
Mac to show a chained admission refusal. Pure helpers are in pi_plan.
"""
import json
import os
import tempfile

import pi_plan as plan

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
COG_TOML = os.path.join(HERE, "anomaly-detect.cog.toml")
MAC_BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"),
                       "debug", "examples", "workload_node")


def pi_binary(target):
    return os.path.join(target, "debug", "examples", "workload_node")


def build_mac(run):
    """Native controller build on the Mac (same example, host target)."""
    print("── Building workload_node for the Mac controller")
    rc, _ = run(plan.placement_cargo_args() + ["--manifest-path", os.path.join(ROOT, "Cargo.toml")],
                capture="quiet")
    if rc != 0:
        raise SystemExit("test-pi: workload_node Mac build failed (rc %d)" % rc)


def run_placement(lane, cog_bin):
    """Run the stage; appends one result row per check to lane.results."""
    run, s = lane.run, lane.scratch
    remote_bin = s + "/bin/workload_node"
    with tempfile.TemporaryDirectory(prefix="weftos-placement-") as work:
        key = os.path.join(work, "controller.key")
        rc, pub = run([MAC_BIN, "keygen", key], capture="stdout")
        pub = (pub or "").strip() if not run.dry_run else "0" * 64
        if rc != 0 or not plan.PUBKEY_RE.match(pub):
            raise SystemExit("test-pi: controller keygen failed")
        print("── Starting the isolated workload-host on the Pi (:%d)" % plan.PLACEMENT_PORT)
        rc, out = lane.ssh(plan.placement_serve_command(s, remote_bin, pub), capture="stdout",
                           timeout=60)
        pid = (out or "").strip().splitlines()[-1:] or ["0"]
        if rc != 0 or not pid[0].isdigit():
            raise SystemExit("test-pi: could not start workload_node on the Pi")
        lane.placement_pid = int(pid[0])
        node_id = wait_listening(lane, s)
        report = os.path.join(work, "report.json")
        peer = "%s:%d" % (plan.ssh_hostname(lane.host), plan.PLACEMENT_PORT)
        print("── Placing anomaly-detect from the Mac (controller) onto the mesh")
        ctl = [MAC_BIN, "place", "--key", key, "--peer", peer, "--cog-toml", COG_TOML,
               "--binary", "aarch64=" + cog_bin, "--dir", os.path.join(work, "ctl"), "--noise",
               "--interval", "1", "--run-secs", "8", "--csi-port", str(plan.PLACEMENT_FEED_PORT),
               "--pin-local", "--out", report]
        ctl_rc, ctl_out = run(ctl, timeout=lane.a.timeout)
        rc, out = lane.ssh(plan.placement_stop_command(s, lane.placement_pid), capture="stdout",
                           timeout=60)
        lane.placement_pid = None
        log, events = plan.split_node_output(out)
        print("── Pi workload-host log\n" + log.strip())
        ok, why = plan.judge_placement(ctl_rc, ctl_out, node_id, events)
        if run.dry_run:
            ok, why = True, []
        for w in why:
            print("  FAIL  placement: " + w)
        lane.results.append(dict(stage="placement", name="mac->pi5 anomaly-detect",
                                 rc=0 if ok else 1, ok=ok, passed=int(ok), failed=int(not ok),
                                 ignored=0, suites=1))
        print("  %s  placement mac->pi5 (%d Pi chain events)" % (
            "PASS" if ok else "FAIL", len(events or [])))
        if lane.a.placement_evidence and not run.dry_run:
            write_evidence(lane.a.placement_evidence, report, events, ok)


def wait_listening(lane, s):
    """Poll the node log until it prints NODE_ID and LISTENING."""
    line = "for i in $(seq 1 30); do grep -q LISTENING %s/runtime/placement.log 2>/dev/null " \
           "&& break; sleep 1; done; cat %s/runtime/placement.log" % (s, s)
    rc, out = lane.ssh(line, capture="stdout", timeout=60)
    m = plan.NODE_ID_RE.search(out or "")
    if lane.run.dry_run:
        return "n-000000"
    if rc != 0 or not m or "LISTENING" not in out:
        raise SystemExit("test-pi: workload_node did not start on the Pi")
    print("  INFO  Pi workload-host node id %s" % m.group(1))
    return m.group(1)


def write_evidence(path, report_path, events, ok):
    """Committed evidence: decision, explain, attempts and chain kinds only,
    refused if anything address-like or path-like slipped in."""
    with open(report_path) as f:
        rep = json.load(f)
    ev = {
        "card": "mesh-placement-12",
        "ok": ok,
        "controller": rep.get("controller"),
        "mac_node": rep.get("local"),
        "explain": rep["place"]["explain"],
        "attempts": rep["place"]["attempts"],
        "placed": rep["place"]["placed"],
        "run": {k: rep.get("run", {}).get(k) for k in ("reports", "first_report")},
        "pin_mac": {"explain": rep.get("pin_local", {}).get("explain"),
                    "attempts": rep.get("pin_local", {}).get("attempts")},
        "controller_chain": [(e["source"], e["kind"]) for e in rep.get("chain", [])],
        "pi_chain": [(e.get("source"), e.get("kind")) for e in (events or [])],
    }
    text = json.dumps(ev, indent=2) + "\n"
    if plan.LEAK_RE.search(text):
        raise SystemExit("test-pi: placement evidence contains an address or path; not written")
    with open(path, "w") as f:
        f.write(text)
    print("  INFO  placement evidence written")


def cleanup(lane):
    """Backstop: a node still running (aborted stage) is stopped."""
    if getattr(lane, "placement_pid", None) or getattr(lane.a, "placement", False):
        lane.ssh(plan.placement_kill_all_command(lane.a.scratch), capture="quiet", timeout=60)

