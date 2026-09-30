"""Two-node placement stage of the Pi lane (card mesh-placement-12).

The Pi 5 runs a real, isolated `weaver` daemon (cross-built here; its own
HOME, runtime dir and chain under the scratch dir; its workload-host served
on :9471 with Noise XX; the system weaver on :9470 and ~/.clawft are never
touched). Its runtime dir gets the operator policy the lane generates:
serve this controller, pin its package key, permit cogs. The Mac places the
released anomaly-detect cog through the placement control plane: the
package is fetched from the Mac over the same connection before load,
admitted by the Pi daemon's native adapter and run against a synthetic
feed on the Pi. The controller then pins the Mac to show a chained
admission refusal. Pure helpers are in pi_plan.
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
    """The cross-built weaver daemon for the Pi."""
    return os.path.join(target, "debug", "weaver")


def build_mac(run):
    """Native controller build on the Mac (workload_node example, host target)."""
    print("── Building workload_node for the Mac controller")
    rc, _ = run(plan.placement_cargo_args() + ["--manifest-path", os.path.join(ROOT, "Cargo.toml")],
                capture="quiet")
    if rc != 0:
        raise SystemExit("test-pi: workload_node Mac build failed (rc %d)" % rc)


def _pid(rc, out, what):
    pid = (out or "").strip().splitlines()[-1:] or ["0"]
    if rc != 0 or not pid[0].isdigit():
        raise SystemExit("test-pi: could not start %s on the Pi" % what)
    return int(pid[0])


def run_placement(lane, cog_bin):
    """Run the stage; appends one result row per check to lane.results."""
    run, s = lane.run, lane.scratch
    weaver = s + "/bin/weaver"
    with tempfile.TemporaryDirectory(prefix="weftos-placement-") as work:
        key = os.path.join(work, "controller.key")
        rc, pub = run([MAC_BIN, "keygen", key], capture="stdout")
        pub = (pub or "").strip() if not run.dry_run else "0" * 64
        if rc != 0 or not plan.PUBKEY_RE.match(pub):
            raise SystemExit("test-pi: controller keygen failed")
        policy = os.path.join(work, "pi-runtime")
        rc, _ = run([MAC_BIN, "daemon-files", "--controller", pub, "--listen",
                     "0.0.0.0:%d" % plan.PLACEMENT_PORT, "--out", policy], capture="quiet")
        if rc != 0:
            raise SystemExit("test-pi: could not write the Pi daemon policy files")
        lane.rsync_to([os.path.join(policy, f) for f in
                       ("workload-host.json", "workload-trust.json", "workload-permits.json")],
                      s + "/runtime/")
        print("── Starting an isolated weaver daemon on the Pi (workload-host :%d)"
              % plan.PLACEMENT_PORT)
        rc, out = lane.ssh(plan.weaver_start_command(s, weaver), capture="stdout", timeout=60)
        lane.placement_pid = _pid(rc, out, "weaver")
        rc, out = lane.ssh(plan.feed_start_command(s), capture="stdout", timeout=60)
        lane.feed_pid = _pid(rc, out, "the sensor feed")
        rc, out = lane.ssh(plan.weaver_ready_command(s, weaver), capture="stdout", timeout=200)
        node_id, served = plan.parse_ready(out)
        if run.dry_run:
            node_id, served = "n-000000", "0.0.0.0:%d" % plan.PLACEMENT_PORT
        if not node_id:
            lane.ssh("cat %s/runtime/placement.log | tail -40" % s, timeout=60)
            raise SystemExit("test-pi: the Pi weaver daemon did not serve workload-host")
        print("  INFO  Pi weaver daemon %s serves workload-host on %s" % (node_id, served))
        report = os.path.join(work, "report.json")
        peer = "%s:%d" % (plan.ssh_hostname(lane.host), plan.PLACEMENT_PORT)
        print("── Placing anomaly-detect from the Mac (controller) onto the mesh")
        ctl = [MAC_BIN, "place", "--key", key, "--peer", peer, "--cog-toml", COG_TOML,
               "--binary", "aarch64=" + cog_bin, "--dir", os.path.join(work, "ctl"), "--noise",
               "--interval", "1", "--run-secs", "8", "--csi-port", str(plan.PLACEMENT_FEED_PORT),
               "--pin-local", "--out", report]
        ctl_rc, ctl_out = run(ctl, timeout=lane.a.timeout)
        rc, out = lane.ssh(plan.weaver_stop_command(s, weaver, lane.placement_pid, lane.feed_pid),
                           capture="stdout", timeout=90)
        lane.placement_pid = lane.feed_pid = None
        log, events = plan.split_node_output(out)
        print("── Pi weaver daemon log (tail)\n" + "\n".join(log.strip().splitlines()[-25:]))
        ok, why = plan.judge_placement(ctl_rc, ctl_out, node_id, events)
        peers = plan.PEER_RE.findall(ctl_out or "")
        if node_id not in peers:
            ok, why = False, why + ["the controller did not reach the Pi daemon's workload-host"]
        if run.dry_run:
            ok, why = True, []
        for w in why:
            print("  FAIL  placement: " + w)
        lane.results.append(dict(stage="placement", name="mac->pi5 weaver anomaly-detect",
                                 rc=0 if ok else 1, ok=ok, passed=int(ok), failed=int(not ok),
                                 ignored=0, suites=1))
        print("  %s  placement mac->pi5 weaver daemon (%d Pi chain events)" % (
            "PASS" if ok else "FAIL", len(events or [])))
        if lane.a.placement_evidence and not run.dry_run:
            write_evidence(lane.a.placement_evidence, report, events, ok, served)


def write_evidence(path, report_path, events, ok, served):
    """Committed evidence: decision, explain, attempts and chain kinds only,
    refused if anything address-like or path-like slipped in."""
    with open(report_path) as f:
        rep = json.load(f)
    ev = {
        "card": "mesh-placement-12",
        "ok": ok,
        "controller": rep.get("controller"),
        "mac_node": rep.get("local"),
        "pi_node": {"runtime": "weaver daemon (isolated)", "workload_host_served_on": served},
        "explain": rep["place"]["explain"],
        "attempts": rep["place"]["attempts"],
        "placed": rep["place"]["placed"],
        "run": {k: rep.get("run", {}).get(k) for k in ("reports", "first_report")},
        "pin_mac": {"explain": rep.get("pin_local", {}).get("explain"),
                    "attempts": rep.get("pin_local", {}).get("attempts")},
        "controller_chain": [(e["source"], e["kind"]) for e in rep.get("chain", [])],
        # The daemon's placement-related chain rows (it chains its boot too).
        "pi_chain": [(e.get("source"), e.get("kind")) for e in (events or [])
                     if str(e.get("source", "")).startswith(("workload", "mesh_artifact"))],
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
        # Stops only processes started from the scratch dir.
        lane.ssh(plan.placement_kill_all_command(lane.a.scratch), capture="quiet", timeout=60)

