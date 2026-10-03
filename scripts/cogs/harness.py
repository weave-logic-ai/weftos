#!/usr/bin/env python3
"""In-target cog conformance runner (card mesh-placement-08, ADR-100, ADR-099 s2/s3).

Runs on the node being tested (inside a container, natively on a Linux ARM
node, or over SSH). Standard library only, Python 3.8+, so it runs in a stock
`python:3-slim` image or on a Raspberry Pi OS / Debian host without installs.

For each cog it provides:
  * a fake ESP32 UDP feed on 127.0.0.1:<udp-port> (default 5006):
      - ADR-069 MAGIC_FEATURES 0xC5110003: 48 bytes, 8 LE f32 at offset 16,
        50 Hz, a steady sine with a 0.95 spike every 40th packet;
      - optional MAGIC_VITALS 0xC5110002: 32 bytes (edge_vitals_pkt_t);
  * a stub Seed ingest endpoint on <ingest-bind>:<ingest-port> (default
    127.0.0.1:80; bind the VM gateway for Apple container relays) that
    accepts POST /api/v1/store/ingest and answers 404 to everything else;
  * a supervised run of the cog with `--once` or `--interval N`, capturing
    timestamped stdout / stderr lines and ingest POSTs. With a plan
    `launcher` (argv list), the cog is started as `<launcher> -- <cog argv>`,
    so a WeftOS runtime adapter (examples/cog_adapter_run.rs) runs it under
    governance instead of this process spawning it directly.

It writes one JSON result per cog. Classification and summaries happen on
the host (scripts/cogs/classify.py) so results stay raw evidence.

Usage:
  harness.py --plan plan.json --out results.json
  harness.py --binary ./cog-x-aarch64 --id x --mode once [--out r.json]
"""
import argparse
import hashlib
import http.server
import ipaddress
import json
import math
import os
import platform
import re
import shutil
import socket
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time

HARNESS_VERSION = "1.0.0"
FEED_ID = "reference-v1"
MAGIC_FEATURES = 0xC5110003
MAGIC_VITALS = 0xC5110002
FEATURE_PKT_SIZE = 48
VITALS_PKT_SIZE = 32
INGEST_PATH = "/api/v1/store/ingest"
FEEDS = ("features", "vitals", "both")
MODES = ("once", "interval")
MAX_LINES = 400          # per stream, keeps results bounded
MAX_BODY = 1 << 20       # ingest bodies larger than 1 MiB are rejected
TAIL_CHARS = 1500


# ── Packet builders (pure, unit-tested) ─────────────────────────────────────

def feature_values(tick):
    """Reference feed values for one tick: sine, spike every 40th packet."""
    if tick % 40 == 39:
        return [0.95] * 8
    return [0.1 * math.sin(tick / 5 + i) for i in range(8)]


def feature_packet(tick):
    """ADR-069 MAGIC_FEATURES packet: magic u32, 12 pad bytes, 8 LE f32."""
    pkt = struct.pack("<I12x8f", MAGIC_FEATURES, *feature_values(tick))
    assert len(pkt) == FEATURE_PKT_SIZE
    return pkt


def vitals_packet(tick, presence=True, breathing_bpm=15.0, heart_bpm=72.0,
                  n_persons=1, motion_energy=0.3, presence_score=12.5):
    """ADR-069 MAGIC_VITALS packet (edge_vitals_pkt_t, packed, little-endian).

    0 magic u32 | 4 node_id u8 | 5 flags u8 | 6 breathing u16 (bpm*100)
    8 heartrate u32 (bpm*10000) | 12 rssi i8 | 13 n_persons u8 | 14 reserved[2]
    16 motion_energy f32 | 20 presence_score f32 | 24 timestamp_ms u32 | 28 pad
    """
    pkt = struct.pack(
        "<IBBHIbB2xffI4x", MAGIC_VITALS, 1, 0x01 if presence else 0x00,
        int(round(breathing_bpm * 100)), int(round(heart_bpm * 10000)), -50,
        n_persons, motion_energy, presence_score, (tick * 20) & 0xFFFFFFFF)
    assert len(pkt) == VITALS_PKT_SIZE
    return pkt


def packets_for_tick(tick, feed):
    """Packets sent on one 20 ms tick. Vitals go out at 5 Hz (every 10th)."""
    if feed not in FEEDS:
        raise ValueError("feed must be one of %s" % (FEEDS,))
    out = []
    if feed in ("features", "both"):
        out.append(feature_packet(tick))
    if feed in ("vitals", "both") and tick % 10 == 0:
        out.append(vitals_packet(tick))
    return out


# ── Cycle-time measurement (pure, unit-tested) ──────────────────────────────

def cycle_stats(mode, elapsed_ms, rc, event_times_ms):
    """Return (cycle_ms, cycles) for a run.

    once:     one cycle; its wall time is the process lifetime, only when the
              cog exited 0 (a failed run has no valid cycle).
    interval: the median gap between consecutive cycle events (ingest POSTs,
              else stdout lines); None until at least two events were seen.
    """
    if mode == "once":
        if rc == 0:
            return round(elapsed_ms, 1), 1
        return None, 0
    times = sorted(event_times_ms)
    if len(times) < 2:
        return None, len(times)
    gaps = [b - a for a, b in zip(times, times[1:])]
    return round(statistics.median(gaps), 1), len(times)


_IPV4 = re.compile(r"(?<![\d.])(\d{1,3}(?:\.\d{1,3}){3})(?![\d.])")


def public_arg(arg):
    """Recorded results are committed to a public repo: replace any IPv4
    address that is not loopback or unspecified with `<host>`."""
    def sub(m):
        try:
            ip = ipaddress.ip_address(m.group(1))
        except ValueError:
            return m.group(0)
        return m.group(0) if ip.is_loopback or ip.is_unspecified else "<host>"
    return _IPV4.sub(sub, arg)


def launcher_argv(launcher, argv):
    """Prefix a cog argv with an adapter launcher (`<launcher> -- <argv>`)."""
    if not launcher:
        return list(argv)
    if not isinstance(launcher, list) or not all(
            isinstance(a, str) and a and "\x00" not in a for a in launcher):
        raise ValueError("launcher must be a list of non-empty strings")
    return list(launcher) + ["--"] + list(argv)


def build_argv(binary, mode, interval_s, extra_args):
    if mode not in MODES:
        raise ValueError("mode must be once or interval")
    argv = [binary]
    if mode == "once":
        argv.append("--once")
    else:
        if not isinstance(interval_s, int) or not 1 <= interval_s <= 3600:
            raise ValueError("interval must be an integer 1..3600")
        argv += ["--interval", str(interval_s)]
    for a in extra_args or []:
        if not isinstance(a, str) or "\x00" in a:
            raise ValueError("extra args must be plain strings")
        argv.append(a)
    return argv


# ── Fixtures: ingest stub and UDP feed ──────────────────────────────────────

class _IngestState:
    def __init__(self, t0):
        self.t0 = t0
        self.lock = threading.Lock()
        self.posts = []

    def record(self, path, body):
        entry = {"t_ms": round((time.monotonic() - self.t0) * 1000, 1),
                 "path": path, "bytes": len(body), "vectors": None}
        try:
            doc = json.loads(body.decode("utf-8"))
            if isinstance(doc, dict) and isinstance(doc.get("vectors"), list):
                entry["vectors"] = len(doc["vectors"])
        except (ValueError, UnicodeDecodeError):
            pass
        entry["sample"] = body[:300].decode("utf-8", errors="replace")
        with self.lock:
            self.posts.append(entry)


def _handler_for(state):
    class Ingest(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            try:
                length = int(self.headers.get("Content-Length", "0"))
            except ValueError:
                length = -1
            if length < 0 or length > MAX_BODY:
                self.send_response(413)
                self.end_headers()
                return
            body = self.rfile.read(length)
            if self.path.split("?", 1)[0] != INGEST_PATH:
                self.send_response(404)
                self.end_headers()
                return
            state.record(self.path, body)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"ok":true}')

        def do_GET(self):
            self.send_response(404)
            self.end_headers()

        def log_message(self, *args):
            pass
    return Ingest


class Fixtures:
    """UDP feed plus ingest stub for the lifetime of one cog run."""

    def __init__(self, feed, udp_port, ingest_port, ingest_bind="127.0.0.1"):
        self.feed, self.udp_port, self.ingest_port = feed, udp_port, ingest_port
        self.ingest_bind = ingest_bind
        self.state = _IngestState(time.monotonic())
        self._stop = threading.Event()
        self.packets_sent = 0

    def __enter__(self):
        self.srv = http.server.ThreadingHTTPServer(
            (self.ingest_bind, self.ingest_port), _handler_for(self.state))
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.sender = threading.Thread(target=self._send, daemon=True)
        self.sender.start()
        return self

    def _send(self):
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        tick = 0
        while not self._stop.is_set():
            for pkt in packets_for_tick(tick, self.feed):
                try:
                    s.sendto(pkt, ("127.0.0.1", self.udp_port))
                    self.packets_sent += 1
                except OSError:
                    pass
            tick += 1
            self._stop.wait(0.02)
        s.close()

    def __exit__(self, *exc):
        self._stop.set()
        self.sender.join(timeout=2)
        self.srv.shutdown()
        self.srv.server_close()


# ── Supervised run ──────────────────────────────────────────────────────────

def _reader(stream, sink, t0):
    for raw in iter(stream.readline, b""):
        if len(sink) < MAX_LINES:
            sink.append((round((time.monotonic() - t0) * 1000, 1),
                         raw.decode("utf-8", errors="replace").rstrip("\n")))
    stream.close()


def _sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 16), b""):
            h.update(chunk)
    return h.hexdigest()


def _tail(lines):
    return "\n".join(text for _, text in lines).strip()[-TAIL_CHARS:]


def _is_json(text):
    try:
        json.loads(text)
        return True
    except ValueError:
        return False


def run_cog(spec, defaults):
    """Run one cog per `spec` and return its raw result dict.

    The binary is copied into a directory this process owns and made
    read-only; that copy is what gets hashed and executed, so nothing can
    swap the file between the hash check and exec.
    """
    source = spec["binary"]
    if not os.path.isfile(source):
        return _run_cog(spec, defaults, source)
    shared_uid = any(a == "--run-as" for a in (defaults.get("launcher") or []))
    # a launcher --run-as runs the cog as another uid, which must read it
    d_mode, f_mode = (0o755, 0o555) if shared_uid else (0o700, 0o500)
    private = tempfile.mkdtemp(prefix="cog-run-")
    try:
        os.chmod(private, d_mode)
        copy = os.path.join(private, os.path.basename(source))
        shutil.copyfile(source, copy)
        os.chmod(copy, f_mode)
        return _run_cog(spec, defaults, copy)
    finally:
        shutil.rmtree(private, ignore_errors=True)


def _run_cog(spec, defaults, binary):
    cid = spec["id"]
    mode = spec.get("mode", "once")
    interval_s = spec.get("interval", 1)
    timeout = float(spec.get("timeout", defaults["timeout"]))
    feed = spec.get("feed", defaults["feed"])
    argv = build_argv(binary, mode, interval_s, spec.get("args"))
    result = {"id": cid, "harness_version": HARNESS_VERSION, "feed_id": FEED_ID,
              "feed": feed, "mode": mode,
              "interval_s": interval_s if mode == "interval" else None,
              "argv": [os.path.basename(argv[0])] + argv[1:], "timeout_s": timeout,
              "host_machine": platform.machine()}
    launcher = defaults.get("launcher")
    if launcher:
        result["launcher"] = [os.path.basename(launcher[0])] + [public_arg(a) for a in launcher[1:]]
    if not os.path.isfile(binary):
        result.update(status="missing-binary", rc=None, timed_out=False)
        return result
    result["sha256"] = _sha256(binary)
    expected = spec.get("sha256")
    if expected is not None and expected != result["sha256"]:
        # Checked here too, after staging: the cache or the copy to the node
        # may not be the file that was verified when it was fetched.
        result.update(status="exec-error", rc=None, timed_out=False,
                      stderr_tail="sha256 mismatch: expected %s" % expected)
        return result
    with Fixtures(feed, defaults["udp_port"], defaults["ingest_port"],
                  defaults["ingest_bind"]) as fx:
        time.sleep(0.1)  # let the feed and stub come up
        t0 = time.monotonic()
        fx.state.t0 = t0
        out, err = [], []
        try:
            proc = subprocess.Popen(launcher_argv(launcher, argv),
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    stdin=subprocess.DEVNULL)
        except OSError as e:
            result.update(status="exec-error", rc=None, timed_out=False,
                          stderr_tail=str(e)[:TAIL_CHARS])
            return result
        readers = [threading.Thread(target=_reader, args=(proc.stdout, out, t0), daemon=True),
                   threading.Thread(target=_reader, args=(proc.stderr, err, t0), daemon=True)]
        for r in readers:
            r.start()
        timed_out = False
        try:
            rc = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
            rc = None
        elapsed_ms = (time.monotonic() - t0) * 1000
        for r in readers:
            r.join(timeout=2)
        time.sleep(0.05)  # in-flight POSTs
        with fx.state.lock:
            posts = list(fx.state.posts)
        packets = fx.packets_sent
    events = [p["t_ms"] for p in posts] or [t for t, s in out if s.strip()]
    cycle_ms, cycles = cycle_stats(mode, elapsed_ms, rc, events)
    result.update(
        status="ran", rc=rc, timed_out=timed_out, elapsed_ms=round(elapsed_ms, 1),
        cycle_ms=cycle_ms, cycles=cycles, packets_sent=packets,
        ingest_posts=len(posts),
        ingest_vectors=sum(p["vectors"] or 0 for p in posts),
        ingest_samples=[p["sample"] for p in posts[:3]],
        stdout_lines=len(out),
        stdout_json_lines=sum(1 for _, s in out if _is_json(s)),
        stdout_tail=_tail(out), stderr_tail=_tail(err),
        first_event_ms=events[0] if events else None)
    return result


def run_plan(plan):
    defaults = {"timeout": float(plan.get("timeout", 15)),
                "feed": plan.get("feed", "features"),
                "udp_port": int(plan.get("udp_port", 5006)),
                "ingest_port": int(plan.get("ingest_port", 80)),
                # A container VM reaches the stub through its gateway, so the
                # stub may bind that interface (or 0.0.0.0) instead.
                "ingest_bind": str(ipaddress.ip_address(plan.get("ingest_bind", "127.0.0.1"))),
                "launcher": plan.get("launcher")}
    if defaults["feed"] not in FEEDS:
        raise ValueError("bad feed")
    launcher_argv(defaults["launcher"], [])  # validate once, up front
    results = []
    for spec in plan["cogs"]:
        r = run_cog(spec, defaults)
        print("%-28s %-14s rc=%s ingest=%s cycle_ms=%s" % (
            r["id"], r["status"], r.get("rc"), r.get("ingest_posts"), r.get("cycle_ms")),
            flush=True)
        results.append(r)
    return {"harness_version": HARNESS_VERSION,
            "host": {"machine": platform.machine(), "system": platform.system(),
                     "release": platform.release(),
                     "python": platform.python_version()},
            "results": results}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--plan", help="JSON plan: {cogs:[{id,binary,mode,interval,args}],...}")
    ap.add_argument("--binary")
    ap.add_argument("--id")
    ap.add_argument("--mode", choices=MODES, default="once")
    ap.add_argument("--interval", type=int, default=1)
    ap.add_argument("--timeout", type=float, default=15)
    ap.add_argument("--feed", choices=FEEDS, default="features")
    ap.add_argument("--out", help="write JSON here (default stdout)")
    a = ap.parse_args(argv)
    if a.plan:
        with open(a.plan) as f:
            plan = json.load(f)
    elif a.binary:
        plan = {"timeout": a.timeout, "feed": a.feed, "cogs": [{
            "id": a.id or os.path.basename(a.binary), "binary": a.binary,
            "mode": a.mode, "interval": a.interval}]}
    else:
        ap.error("--plan or --binary is required")
    doc = run_plan(plan)
    text = json.dumps(doc, indent=1, sort_keys=True)
    if a.out:
        with open(a.out, "w") as f:
            f.write(text + "\n")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
