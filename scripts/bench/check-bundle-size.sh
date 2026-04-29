#!/usr/bin/env bash
# Browser WASM bundle size gate (WEFT-389 / M5-A).
#
# Measures the post-`wasm-bindgen` (and optional `wasm-opt -Oz`) bundle
# at `crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm` and fails if it
# exceeds the documented budget. Gzipped size is the load-bearing
# metric — that's what the browser actually downloads — but raw size
# is reported alongside for context.
#
# The budget rationale lives in `docs/architecture/wasm-bundle-size.md`.
# When you intentionally land a feature that pushes the bundle past the
# threshold, bump the threshold there, document the reason in the
# changelog, and re-run.
#
# Usage: scripts/bench/check-bundle-size.sh [bundle.wasm] [max-raw-kb] [max-gz-kb]
#
# Defaults:
#   bundle:     crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm
#   max-raw-kb: 1600  (current ~1340 KB after bindgen, leaves ~260 KB headroom)
#   max-gz-kb:  600   (current ~471 KB after gzip -9, leaves ~130 KB headroom)
#
# Exit codes:
#   0  — bundle within budget
#   1  — bundle exceeds budget OR file missing OR tooling missing
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

# --- Defaults (kept in sync with docs/architecture/wasm-bundle-size.md) ---
DEFAULT_BUNDLE="$WORKSPACE_ROOT/crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm"
DEFAULT_MAX_RAW_KB=1600
DEFAULT_MAX_GZ_KB=600

BUNDLE="${1:-$DEFAULT_BUNDLE}"
MAX_RAW_KB="${2:-$DEFAULT_MAX_RAW_KB}"
MAX_GZ_KB="${3:-$DEFAULT_MAX_GZ_KB}"

# --- Validate ---
if [ ! -f "$BUNDLE" ]; then
    fail "bundle not found: $BUNDLE"
    fail ""
    fail "Build it first:"
    fail "  scripts/build.sh browser"
    exit 1
fi

if ! command -v gzip >/dev/null 2>&1; then
    fail "gzip not installed"
    exit 1
fi

# --- Measure ---
RAW_BYTES=$(wc -c < "$BUNDLE")
RAW_KB=$(( RAW_BYTES / 1024 ))

GZ_BYTES=$(gzip -9 -c "$BUNDLE" | wc -c)
GZ_KB=$(( GZ_BYTES / 1024 ))

# --- Report ---
echo "=== Browser WASM Bundle Size Gate ==="
echo "Bundle:    $BUNDLE"
echo "Raw:       ${RAW_KB} KB (${RAW_BYTES} bytes)"
echo "Gzipped:   ${GZ_KB} KB (${GZ_BYTES} bytes)"
echo "Budget:    ${MAX_RAW_KB} KB raw / ${MAX_GZ_KB} KB gzipped"
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
        echo "## Browser WASM Bundle Size"
        echo ""
        echo "| Metric  | Size       | Budget       | Status |"
        echo "|---------|------------|--------------|--------|"
        echo "| Raw     | ${RAW_KB} KB | ${MAX_RAW_KB} KB | $([ "$RAW_KB" -le "$MAX_RAW_KB" ] && echo PASS || echo FAIL) |"
        echo "| Gzipped | ${GZ_KB} KB | ${MAX_GZ_KB} KB | $([ "$GZ_KB" -le "$MAX_GZ_KB" ] && echo PASS || echo FAIL) |"
        echo ""
        echo "Gate rationale: docs/architecture/wasm-bundle-size.md"
    } >> "$GITHUB_STEP_SUMMARY"
fi

if [ "$PASSED" = true ]; then
    ok "Browser WASM bundle is within budget."
    exit 0
else
    fail "Browser WASM bundle exceeds budget."
    fail ""
    fail "Investigate with:"
    fail "  twiggy top crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm"
    fail "  cargo bloat --target wasm32-unknown-unknown -p clawft-wasm \\"
    fail "    --no-default-features --features browser --profile release-wasm --crates"
    fail ""
    fail "If the growth is intentional, raise the budget in:"
    fail "  scripts/bench/check-bundle-size.sh"
    fail "  docs/architecture/wasm-bundle-size.md"
    exit 1
fi
