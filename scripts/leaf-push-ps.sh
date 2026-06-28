#!/usr/bin/env bash
# leaf-push-ps.sh — push `kernel.ps` to a CrowPanel leaf display as a
# vector scene. Phase E shim around `weaver leaf scene ps`.
#
# Pre-Phase-E this script did its own kernel.ps → bash → row-text
# decomposition and fired one `weaver leaf push text` per row. Phase E
# moves the producer into Rust (`crates/clawft-weave/src/commands/
# leaf_cmd.rs` :: `run_scene_ps`) where it can use
# `weftos-scene-builder` to emit a stable, path-keyed scene + diff
# against the previous frame. This script is now a thin loop wrapper
# around the Rust producer.
#
# Usage:
#   scripts/leaf-push-ps.sh <leaf-pubkey-hex> [refresh-seconds]
#
#   <leaf-pubkey-hex>   the CrowPanel's node pubkey (hex, 0x optional).
#                       The firmware prints this on boot once node
#                       identity is wired (edge-pad task 4).
#   [refresh-seconds]   0 / omitted = one-shot. >0 = loop, repainting
#                       every N seconds.
#
# Snapshot vs delta — see `weaver leaf scene ps --help`. State is
# cached at `~/.clawft/leaf-state/<pubkey>-<display>.cbor`; delete it
# (or pass `--snapshot`) to force a fresh `Replace(Scene)`.

set -euo pipefail

TARGET_RAW="${1:?usage: leaf-push-ps.sh <leaf-pubkey-hex> [refresh-seconds]}"
REFRESH="${2:-0}"
TARGET="${TARGET_RAW#0x}"

push_once() {
    weaver leaf scene ps --target "$TARGET"
}

if [ "$REFRESH" -gt 0 ] 2>/dev/null; then
    echo "live mode: repainting every ${REFRESH}s (Ctrl-C to stop)"
    while true; do
        push_once
        sleep "$REFRESH"
    done
else
    push_once
fi
