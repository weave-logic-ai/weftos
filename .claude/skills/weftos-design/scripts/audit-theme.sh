#!/usr/bin/env bash
# Audit the egui GUI crate for raw color literals that drift from
# docs/DESIGN.md §2 (the Tokens struct in
# crates/clawft-gui-egui/src/theming.rs).
#
# Rule:
#   D-TK01 : Color32::from_rgb / from_rgba_unmultiplied calls outside
#            theming.rs are flagged. Existing offenders are tracked
#            under WEFTOS-DESIGN-1 — graduate them to tokens.
#
# Usage: audit-theme.sh [repo-root]
# Exit 0 if no new offenders; non-zero if drift detected.

set -euo pipefail

ROOT="${1:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
GUI="$ROOT/crates/clawft-gui-egui/src"
THEME="$GUI/theming.rs"

if [[ ! -d "$GUI" ]]; then
  echo "no GUI crate at $GUI" >&2; exit 2
fi

# Find all color literals outside theming.rs
offenders=$(grep -rnE 'Color32::from_(rgb|rgba_unmultiplied)\(' "$GUI" \
  --include="*.rs" \
  | grep -v "theming.rs" \
  || true)

if [[ -z "$offenders" ]]; then
  echo "clean: no color drift outside theming.rs"
  exit 0
fi

count=$(echo "$offenders" | wc -l | tr -d ' ')
echo "$offenders"
echo ""
echo "D-TK01: $count color literal(s) outside theming.rs"
echo "  see docs/DESIGN.md §2 — every color belongs in Tokens"
exit 1
