#!/usr/bin/env python3
"""Enroll a plugged-in ESP32 edge node into the fleet roster.

Reads the board's real chip type, MAC and flash size with esptool, then fills the next reserved slot
in fleet/nodes.json (or updates the matching MAC). Idempotent: re-running with the same board updates
it, never duplicates. Plug in each of the 10 C6s and run this once per board.

Usage: scripts/enroll-node.py [--port /dev/cu.usbmodemXXXX] [--role fleet-heartbeat] [--firmware name]
"""
import argparse, glob, json, os, re, subprocess, sys, datetime

ROSTER = os.path.expanduser("~/weftos/crates/weftos-cog-host/fleet/nodes.json")


def find_port():
    ports = sorted(glob.glob("/dev/cu.usbmodem*") + glob.glob("/dev/cu.usbserial*") + glob.glob("/dev/cu.SLAB_USBtoUART*"))
    return ports[0] if ports else None


def esptool(port, *args):
    exe = "esptool" if subprocess.run(["which", "esptool"], capture_output=True).returncode == 0 else "esptool.py"
    return subprocess.run([exe, "--port", port, *args], capture_output=True, text=True, timeout=40).stdout


def detect(port):
    out = esptool(port, "chip-id") or esptool(port, "chip_id")
    chip = None
    m = re.search(r"Detecting chip type\.\.\.\s*(ESP32[-\w]*)", out) or re.search(r"Chip is\s*(ESP32[-\w]*)", out)
    if m:
        chip = m.group(1)
    macs = re.findall(r"BASE MAC:\s*([0-9A-Fa-f:]{17})", out) or re.findall(r"MAC:\s*([0-9A-Fa-f:]{17})", out)
    mac = macs[0].lower() if macs else None
    fout = esptool(port, "flash-id") or esptool(port, "flash_id")
    fm = re.search(r"Detected flash size:\s*(\d+)\s*MB", fout)
    flash_mb = int(fm.group(1)) if fm else None
    return chip, mac, flash_mb


def chip_id(chip):
    c = (chip or "esp32").lower().replace("esp32-", "").replace("esp32", "") or "esp32"
    return c  # e.g. 'c6'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port")
    ap.add_argument("--role", default=None)
    ap.add_argument("--firmware", default=None)
    a = ap.parse_args()

    port = a.port or find_port()
    if not port:
        print("no ESP32 serial port found (plug in a board)", file=sys.stderr)
        return 1
    chip, mac, flash_mb = detect(port)
    if not mac:
        print(f"could not read a MAC from {port} (is it an ESP32 in bootloader?)", file=sys.stderr)
        return 1

    roster = json.load(open(ROSTER))
    nodes = roster["nodes"]
    today = datetime.date.today().isoformat()

    node = next((n for n in nodes if n.get("mac") == mac), None)  # already enrolled?
    if node is None:
        node = next((n for n in nodes if n.get("mac") in (None, "", "pending")), None)  # next reserved slot
    if node is None:  # grow the roster
        prefix = chip_id(chip)
        n = sum(1 for x in nodes if x["id"].startswith(prefix)) + 1
        node = {"id": f"{prefix}-{n:02d}"}
        nodes.append(node)

    node.update({
        "chip": (chip or "esp32").lower(),
        "mac": mac,
        "flash_mb": flash_mb or node.get("flash_mb"),
        "status": "enrolled",
        "port_last": port,
        "enrolled": node.get("enrolled") or today,
        "seen": today,
    })
    if a.role:
        node["role"] = a.role
    if a.firmware is not None:
        node["firmware"] = a.firmware

    json.dump(roster, open(ROSTER, "w"), indent=2)
    open(ROSTER, "a").write("\n")
    enrolled = sum(1 for n in nodes if n.get("status") == "enrolled")
    print(f"enrolled {node['id']}: {node['chip']} mac={mac} flash={node.get('flash_mb')}MB on {port}")
    print(f"roster: {enrolled}/{len(nodes)} nodes enrolled")
    return 0


if __name__ == "__main__":
    sys.exit(main())
