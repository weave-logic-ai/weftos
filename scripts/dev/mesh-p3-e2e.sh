#!/usr/bin/env bash
# ADR-103 Phase 3 end to end, as the current user (P3 package X).
#
# Runs the machine mesh service and two user daemons in one test process on
# tempdirs (crates/clawft-weave/tests/mesh_p3_e2e.rs): register, service mode
# under the service's node id, a scoped weft:// send between the two daemons
# (delivered and stamped), address takeover refused, bind.rebind revoking the
# old key, observe admission journalling a bad peer, service restart with the
# daemon reconnecting and bindings/journal intact, service = off collapsed.
# The second account is an injected peer identity; the real two-uid check is
# scripts/dev/mesh-two-uid.sh (needs sudo, never in the gate).
#
# Never runs as root; never touches /var, /etc, launchd, systemd or the real
# ~/.clawft / ~/.weftos (HOME and WEFTOS_RUNTIME_DIR point at tempdirs).
#
# Usage: scripts/dev/mesh-p3-e2e.sh
#   WEAVER_BIN=/path/to/weaver  also smoke the real binary: `weaver mesh serve`
#                               on tempdirs, then `weaver mesh status --json`.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ "$(id -u)" = "0" ]; then
    echo "refusing to run as root: the mesh service and the daemons never run as root" >&2
    exit 1
fi

base="$(mktemp -d "${TMPDIR:-/tmp}/weftos-p3-e2e.XXXXXX")"
serve_pid=""
cleanup() {
    if [ -n "$serve_pid" ]; then
        kill "$serve_pid" 2>/dev/null || true
        wait "$serve_pid" 2>/dev/null || true
    fi
    rm -rf "$base"
}
trap cleanup EXIT
mkdir -p "$base/home" "$base/run"
# Keep the toolchain and crate cache where they are; only the WeftOS view of
# HOME moves to the tempdir.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export HOME="$base/home" WEFTOS_RUNTIME_DIR="$base/run"
unset WEFTOS_MESH_SOCKET WEFTOS_MESH_STATE_DIR

echo "== mesh-p3-e2e: in-process service + two user daemons"
cargo test -p clawft-weave --features mesh \
    --test mesh_p3_e2e --test mesh_service_client --test mesh_boot_user --test mesh_boot_plain --test mesh_boot_guard

if [ -n "${WEAVER_BIN:-}" ]; then
    echo "== mesh-p3-e2e: process smoke with $WEAVER_BIN"
    # Short paths: unix socket paths are length-limited.
    sock="$base/r/s"
    "$WEAVER_BIN" mesh serve --state-dir "$base/st" --socket "$sock" \
        --listen 127.0.0.1:0 --health-listen off >"$base/serve.log" 2>&1 &
    serve_pid=$!
    for _ in $(seq 1 100); do
        [ -S "$sock" ] && [ -f "$base/r/service.json" ] && break
        kill -0 "$serve_pid" 2>/dev/null || { cat "$base/serve.log" >&2; exit 1; }
        sleep 0.1
    done
    [ -S "$sock" ] || { echo "the service did not create its socket" >&2; cat "$base/serve.log" >&2; exit 1; }
    status="$(WEFTOS_MESH_SOCKET="$sock" "$WEAVER_BIN" mesh status --json)"
    printf '%s\n' "$status" | grep -q '"node_id"' || { echo "unexpected status: $status" >&2; exit 1; }
    echo "service status ok"
fi
echo "== mesh-p3-e2e: ok"
