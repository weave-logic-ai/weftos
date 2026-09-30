"""Synthetic ESP32 feature feed for the Pi placement stage (card
mesh-placement-12): ADR-069 MAGIC_FEATURES packets at 50 Hz to
127.0.0.1:<port>, the same packets the native live test and
`workload_node serve --feed-port` send. Runs on the Pi next to the isolated
weaver daemon so the placed cog has input; stops after --secs.

Usage: python3 csi_feed.py --port 15006 --secs 900
"""
import argparse
import math
import socket
import struct
import time

MAGIC_FEATURES = 0xC5110003


def packet(tick):
    """One packet: magic (LE u32), 12 reserved bytes, 8 LE f32 features;
    every 40th tick is a spike the anomaly detector reports."""
    vals = [0.95 if tick % 40 == 39 else 0.1 * math.sin(tick / 5.0 + i) for i in range(8)]
    return struct.pack("<I", MAGIC_FEATURES) + bytes(12) + struct.pack("<8f", *vals)


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--secs", type=int, default=900)
    a = ap.parse_args(argv)
    if not 0 < a.port < 65536:
        raise SystemExit("--port out of range")
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    end = time.monotonic() + a.secs
    tick = 0
    while time.monotonic() < end:
        sock.sendto(packet(tick), ("127.0.0.1", a.port))
        tick += 1
        time.sleep(0.02)


if __name__ == "__main__":
    main()
