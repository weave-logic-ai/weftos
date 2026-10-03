#!/usr/bin/env bash
# Give one test process (the nextest `run-wrapper`) or one test binary (the
# cargo `target.<triple>.runner` that `scripts/build.sh test` sets) its own
# throwaway WEFTOS_RUNTIME_DIR, so test binaries never share a runtime dir
# (cluster_peers.json, node.key, kernel.lock ...) and never touch a real one.
#
# `scripts/build.sh` creates WEFTOS_TEST_RUNTIME_ROOT and removes it on exit.
# Without a root it refuses (exit 70): running a test through this wrapper
# unpinned would silently use a real runtime dir.
set -u
root="${WEFTOS_TEST_RUNTIME_ROOT:-}"
if [ -z "$root" ] || [ ! -d "$root" ]; then
    echo "test-runtime-wrap: WEFTOS_TEST_RUNTIME_ROOT is unset or not a directory; run tests through scripts/build.sh" >&2
    exit 70
fi
name="$(basename "$1")"
dir="$(mktemp -d "$root/${name%%-*}.XXXXXX")" || exit 70
WEFTOS_TEST=1 WEFTOS_RUNTIME_DIR="$dir" "$@"
status=$?
rm -rf "$dir"
exit "$status"
