#!/usr/bin/env python3
"""Host-side mesh leaf simulator — exercises the daemon's leaf-push path
without ESP32 hardware.

Mimics crates/clawft-edge-pad/src/mesh.rs byte-for-byte:
  - connects to the daemon mesh transport (plaintext TCP :9470)
  - sends a `mesh.subscribe` MeshIpcEnvelope for this leaf's push topic
  - fires `weaver leaf push --target <leaf-id> text ...`
  - reports every `[4-byte BE len][JSON envelope]` frame received

If the daemon forwards the push, the frame is printed here. If nothing
arrives, the break is daemon-side (subscribe registration or topic
forwarding), not in the firmware.

Usage:  scripts/mesh-leaf-sim.py [leaf-id-hex] [daemon-host] [port]
"""
import socket
import subprocess
import sys
import threading
import time

LEAF_ID = sys.argv[1] if len(sys.argv) > 1 else "aabbccddeeff"
HOST = sys.argv[2] if len(sys.argv) > 2 else "127.0.0.1"
PORT = int(sys.argv[3]) if len(sys.argv) > 3 else 9470
TOPIC = f"mesh.leaf.{LEAF_ID}.push"

# Byte-identical to mesh.rs subscribe_envelope().
SUBSCRIBE = (
    '{"source_node":"' + LEAF_ID + '","dest_node":"daemon","message":{'
    '"id":"leaf-sub-1","from":0,"target":{"Topic":"mesh.subscribe"},'
    '"payload":{"Json":{"topic":"' + TOPIC + '"}},'
    '"timestamp":"2026-05-14T00:00:00Z"},'
    '"hop_count":0,"envelope_id":"leaf-env-sub-1"}'
).encode()


def frame(payload: bytes) -> bytes:
    return len(payload).to_bytes(4, "big") + payload


def read_exact(sock, n):
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            return None
        buf += chunk
    return buf


def push():
    time.sleep(1.5)
    print(f"[sim] firing: weaver leaf push --target {LEAF_ID} text ...", flush=True)
    r = subprocess.run(
        ["weaver", "leaf", "push", "--target", LEAF_ID, "text",
         "--text", "hello-from-sim", "--layer", "alert"],
        capture_output=True, text=True,
    )
    print("[sim] push stdout:", r.stdout.strip(), flush=True)
    if r.stderr.strip():
        print("[sim] push stderr:", r.stderr.strip(), flush=True)


def main():
    print(f"[sim] leaf-id={LEAF_ID} topic={TOPIC} -> {HOST}:{PORT}", flush=True)
    s = socket.create_connection((HOST, PORT), timeout=5)
    s.sendall(frame(SUBSCRIBE))
    print(f"[sim] subscribe sent ({len(SUBSCRIBE)} bytes)", flush=True)

    threading.Thread(target=push, daemon=True).start()

    s.settimeout(12)
    got = 0
    try:
        while True:
            ln = read_exact(s, 4)
            if ln is None:
                print("[sim] peer closed connection", flush=True)
                break
            n = int.from_bytes(ln, "big")
            body = read_exact(s, n)
            if body is None:
                print("[sim] peer closed mid-frame", flush=True)
                break
            got += 1
            print(f"[sim] RX FRAME #{got}: {n} bytes", flush=True)
            print("       " + body.decode("utf-8", "replace")[:600], flush=True)
    except socket.timeout:
        print(f"[sim] read timeout — {got} frame(s) received total", flush=True)
    s.close()
    if got == 0:
        print("[sim] VERDICT: daemon did NOT forward the push -> daemon-side break", flush=True)
        sys.exit(1)
    print("[sim] VERDICT: daemon forwarded the push -> daemon path OK", flush=True)


if __name__ == "__main__":
    main()
