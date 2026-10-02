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
    mkdir -p "$base/r"
    sock="$base/r/s"
    st="$base/st"

    printf 'admin_uids = [%s]\nprobe_facts = false\n' "$(id -u)" >"$base/mesh.toml"

    start_service() { # state-dir
        rm -f "$sock" "$base/r/service.json"
        "$WEAVER_BIN" mesh serve --config "$base/mesh.toml" --state-dir "$1" --socket "$sock" \
            --listen 127.0.0.1:0 --health-listen off >>"$base/serve.log" 2>&1 &
        serve_pid=$!
        for _ in $(seq 1 100); do
            [ -S "$sock" ] && [ -f "$base/r/service.json" ] && return 0
            kill -0 "$serve_pid" 2>/dev/null || { cat "$base/serve.log" >&2; return 1; }
            sleep 0.1
        done
        echo "the service did not create its socket" >&2; cat "$base/serve.log" >&2; return 1
    }
    stop_service() {
        kill "$serve_pid" 2>/dev/null || true
        wait "$serve_pid" 2>/dev/null || true
        serve_pid=""
    }
    mesh() { WEFTOS_MESH_SOCKET="$sock" "$WEAVER_BIN" mesh "$@"; }
    node_id_of() { mesh status --json | sed -n 's/.*"node_id": *"\([0-9a-f]*\)".*/\1/p' | head -n1; }

    start_service "$st"
    status="$(mesh status --json)"
    printf '%s\n' "$status" | grep -q '"node_id"' || { echo "unexpected status: $status" >&2; exit 1; }
    echo "service status ok"
    first_id="$(node_id_of)"
    [ -n "$first_id" ] || { echo "no node id in: $status" >&2; exit 1; }
    stop_service

    command -v python3 >/dev/null || { echo "python3 is required for the journal-corruption smoke steps" >&2; exit 1; }

    echo "-- adopted node key keeps the node id (install-service --adopt-node-key)"
    mkdir -p "$base/st2"
    chmod 700 "$base/st2"
    cp "$st/node.key" "$base/st2/node.key"
    chmod 600 "$base/st2/node.key"
    start_service "$base/st2"
    [ "$(node_id_of)" = "$first_id" ] || { echo "adopted key changed the node id" >&2; exit 1; }
    grep -q '"key_origin":"adopted"' "$base/st2/journal.jsonl" || { echo "journal does not record the adoption" >&2; exit 1; }
    stop_service

    echo "-- a crash-torn journal tail is accepted by the service itself"
    start_service "$st"
    stop_service
    printf '{"v":1,"seq":999,"ts":1,"prev":"00' >>"$st/journal.jsonl"
    start_service "$st"
    jv="$(mesh journal verify --json)"
    printf '%s' "$jv" | grep -q '"read_only": false' || { echo "a torn tail must not leave the journal read-only" >&2; exit 1; }
    printf '%s' "$jv" | grep -q '"last_auto_accept": {' || { echo "the auto-accept is not surfaced" >&2; exit 1; }
    grep -q '"auto":"torn_tail"' "$st/journal.jsonl" || { echo "the auto-accept is not journalled" >&2; exit 1; }
    stop_service

    echo "-- a corrupt journal tail is quarantined; the CLI accepts it"
    # Corrupt the signature of the last record (a complete line, so it is
    # corruption, not a crash-torn tail).
    python3 - "$st/journal.jsonl" <<'PY'
import sys
p = sys.argv[1]
lines = open(p).read().splitlines()
l = lines[-1]
at = l.rfind('"sig":"') + 7
lines[-1] = l[:at] + ('1' if l[at] == '0' else '0') + l[at + 1:]
open(p, "w").write("\n".join(lines) + "\n")
PY
    start_service "$st"
    jv="$(mesh journal verify --json)"
    printf '%s' "$jv" | grep -q '"read_only": true' || { echo "journal should be read-only" >&2; exit 1; }
    mesh journal verify --accept-truncate >/dev/null
    jv="$(mesh journal verify --json)"
    printf '%s' "$jv" | grep -q '"read_only": false' || { echo "accept-truncate did not clear read-only" >&2; exit 1; }
    stop_service
    ls "$st"/journal.corrupt.* >/dev/null 2>&1 || { echo "the quarantine file is missing" >&2; exit 1; }
fi
echo "== mesh-p3-e2e: ok"
