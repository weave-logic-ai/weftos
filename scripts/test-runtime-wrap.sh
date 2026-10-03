#!/usr/bin/env bash
# Give one test process (the nextest `run-wrapper`) or one test binary (the
# cargo `target.<triple>.runner` that `scripts/build.sh test` sets) its own
# throwaway WEFTOS_RUNTIME_DIR, so test binaries never share a runtime dir
# (cluster_peers.json, node.key, kernel.lock ...) and never touch a real one.
#
# `scripts/build.sh` creates WEFTOS_TEST_RUNTIME_ROOT and removes it on exit.
# Without it (or with a caller-chosen WEFTOS_RUNTIME_DIR) this is a plain exec.
set -u
root="${WEFTOS_TEST_RUNTIME_ROOT:-}"
if [ -z "$root" ] || [ ! -d "$root" ]; then
    exec "$@"
fi
name="$(basename "$1")"
dir="$(mktemp -d "$root/${name%%-*}.XXXXXX")" || exit 70
WEFTOS_RUNTIME_DIR="$dir" "$@"
status=$?
rm -rf "$dir"
exit "$status"
