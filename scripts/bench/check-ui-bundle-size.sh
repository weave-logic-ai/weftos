#!/usr/bin/env bash
# Dashboard UI JS bundle size gate (WEFT-315 / S1.2 exit criteria).
#
# Measures the gzipped size of the largest emitted JS chunk under
# `clawft-ui/dist/assets/` and fails if it exceeds the documented
# budget. Gzipped size is the load-bearing metric — that is what the
# browser actually downloads — but raw size is reported alongside.
#
# The S1.2 phase plan targeted < 200 KB gzipped for the entry chunk;
# the current bundle is ~130 KB gzipped, so we set the gate at 200 KB
# with documented headroom and tighten over time as we code-split
# heavy modules (canvas advanced renderers, code editor, charting).
#
# Usage: scripts/bench/check-ui-bundle-size.sh [dist-dir] [max-gz-kb] [max-raw-kb]
#
# Defaults:
#   dist-dir:   clawft-ui/dist
#   max-gz-kb:  200
#   max-raw-kb: 700
#
# Exit codes:
#   0  — bundle within budget
#   1  — bundle exceeds budget OR dist missing OR tooling missing
#   2  — usage error

set -euo pipefail

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info() { printf "${CYAN}[INFO]${NC}  %s\n" "$*"; }
ok()   { printf "${GREEN}[PASS]${NC}  %s\n" "$*"; }
warn() { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
fail() { printf "${RED}[FAIL]${NC}  %s\n" "$*" >&2; }

# --- Resolve workspace root ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# --- Defaults ---
DEFAULT_DIST="$WORKSPACE_ROOT/clawft-ui/dist"
DEFAULT_MAX_GZ_KB=200
DEFAULT_MAX_RAW_KB=700

DIST="${1:-$DEFAULT_DIST}"
MAX_GZ_KB="${2:-$DEFAULT_MAX_GZ_KB}"
MAX_RAW_KB="${3:-$DEFAULT_MAX_RAW_KB}"

# --- Validate ---
if [ ! -d "$DIST" ]; then
    fail "dist directory not found: $DIST"
    fail ""
    fail "Build it first:"
    fail "  (cd clawft-ui && npm run build)"
    exit 1
fi

if ! command -v gzip >/dev/null 2>&1; then
    fail "gzip not installed"
    exit 1
fi

ASSETS_DIR="$DIST/assets"
if [ ! -d "$ASSETS_DIR" ]; then
    fail "assets dir not found: $ASSETS_DIR"
    exit 1
fi

# --- Measure ---
# Find the largest .js bundle (typically the entry chunk).
LARGEST_JS=$(find "$ASSETS_DIR" -maxdepth 1 -name '*.js' -printf '%s %p\n' | sort -rn | head -n1 | awk '{print $2}')
if [ -z "$LARGEST_JS" ] || [ ! -f "$LARGEST_JS" ]; then
    fail "no .js files found in $ASSETS_DIR"
    exit 1
fi

RAW_BYTES=$(wc -c < "$LARGEST_JS")
RAW_KB=$(( RAW_BYTES / 1024 ))

GZ_BYTES=$(gzip -9 -c "$LARGEST_JS" | wc -c)
GZ_KB=$(( GZ_BYTES / 1024 ))

# --- Report ---
echo "=== Dashboard UI JS Bundle Size Gate ==="
echo "Bundle:    $LARGEST_JS"
echo "Raw:       ${RAW_KB} KB (${RAW_BYTES} bytes)"
echo "Gzipped:   ${GZ_KB} KB (${GZ_BYTES} bytes)"
echo "Budget:    ${MAX_RAW_KB} KB raw / ${MAX_GZ_KB} KB gzipped"
echo ""

# Per-chunk breakdown for visibility.
echo "All JS chunks:"
find "$ASSETS_DIR" -maxdepth 1 -name '*.js' -printf '%s %p\n' \
    | sort -rn \
    | while read -r bytes path; do
        kb=$(( bytes / 1024 ))
        gz_b=$(gzip -9 -c "$path" | wc -c)
        gz_kb=$(( gz_b / 1024 ))
        printf '  %5d KB raw / %4d KB gz   %s\n' "$kb" "$gz_kb" "$(basename "$path")"
    done
echo ""

# --- Gate ---
PASSED=true
if [ "$RAW_KB" -le "$MAX_RAW_KB" ]; then
    ok "Raw size ${RAW_KB} KB <= ${MAX_RAW_KB} KB"
else
    fail "Raw size ${RAW_KB} KB > ${MAX_RAW_KB} KB (over by $((RAW_KB - MAX_RAW_KB)) KB)"
    PASSED=false
fi

if [ "$GZ_KB" -le "$MAX_GZ_KB" ]; then
    ok "Gzipped ${GZ_KB} KB <= ${MAX_GZ_KB} KB"
else
    fail "Gzipped ${GZ_KB} KB > ${MAX_GZ_KB} KB (over by $((GZ_KB - MAX_GZ_KB)) KB)"
    PASSED=false
fi
echo ""

# --- GitHub Actions step summary (when running under Actions) ---
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "## Dashboard UI JS Bundle Size"
        echo ""
        echo "| Metric  | Size       | Budget       | Status |"
        echo "|---------|------------|--------------|--------|"
        echo "| Raw     | ${RAW_KB} KB | ${MAX_RAW_KB} KB | $([ "$RAW_KB" -le "$MAX_RAW_KB" ] && echo PASS || echo FAIL) |"
        echo "| Gzipped | ${GZ_KB} KB | ${MAX_GZ_KB} KB | $([ "$GZ_KB" -le "$MAX_GZ_KB" ] && echo PASS || echo FAIL) |"
        echo ""
        echo "Tracked in WEFT-315; raise the budget in scripts/bench/check-ui-bundle-size.sh when intentional."
    } >> "$GITHUB_STEP_SUMMARY"
fi

if [ "$PASSED" = true ]; then
    ok "Dashboard UI JS bundle is within budget."
    exit 0
else
    fail "Dashboard UI JS bundle exceeds budget."
    fail ""
    fail "Investigate with rollup-plugin-visualizer or:"
    fail "  (cd clawft-ui && npx vite build --mode analyze)"
    fail ""
    fail "Likely wins: code-split /canvas advanced renderers and the code editor"
    fail "via dynamic import(). Tracked in WEFT-315."
    fail ""
    fail "If the growth is intentional, raise the budget in:"
    fail "  scripts/bench/check-ui-bundle-size.sh"
    exit 1
fi
