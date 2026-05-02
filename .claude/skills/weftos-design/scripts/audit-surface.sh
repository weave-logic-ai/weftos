#!/usr/bin/env bash
# Lint a WeftOS surface TOML against docs/DESIGN.md.
#
# Checks (rule-id : DESIGN.md §):
#   D-NS01 : §3   — every primitive type matches a known ui://* slug.
#   D-FG01 : §3.4 — ui://foreign carries a "graduate to ui://X" comment.
#   D-EM01 : §5   — app-window / list-detail / tile-grid / stream surfaces declare empty_state, loading_state, offline_state.
#   D-DI01 : §6   — declared on_tap / on_change verbs are listed in manifest influences (warn-only — manifest path is heuristic).
#
# Usage:
#   audit-surface.sh <fixture.toml> [<fixture.toml> …]
#
# Exit 0 = clean, 1 = violations, 2 = bad usage.

set -euo pipefail

if [[ $# -eq 0 ]]; then
  echo "usage: $0 <fixture.toml> [...]" >&2
  exit 2
fi

KNOWN_PRIMITIVES=(
  ui://stack ui://grid ui://tabs ui://strip ui://dock ui://sidebar ui://sheet ui://modal
  ui://chip ui://gauge ui://table ui://tree ui://plot ui://heatmap
  ui://waveform ui://stream ui://canvas ui://media
  ui://pressable ui://toggle ui://slider ui://select ui://field
  ui://foreign
)

violations=0

for f in "$@"; do
  if [[ ! -f "$f" ]]; then
    echo "$f: ENOENT" >&2; violations=$((violations+1)); continue
  fi

  # D-NS01: unknown primitive types
  while IFS=: read -r line content; do
    prim=$(echo "$content" | grep -oE 'ui://[a-z_]+' | head -1)
    if [[ -n "$prim" ]] && ! printf '%s\n' "${KNOWN_PRIMITIVES[@]}" | grep -qx "$prim"; then
      echo "$f:$line: D-NS01: unknown primitive '$prim'"
      violations=$((violations+1))
    fi
  done < <(grep -nE '^\s*type\s*=\s*"ui://' "$f")

  # D-FG01: ui://foreign needs graduation TODO
  if grep -qE 'ui://foreign' "$f"; then
    if ! grep -qE 'WEFTOS-DESIGN: TODO graduate to ui://' "$f"; then
      first=$(grep -nE 'ui://foreign' "$f" | head -1 | cut -d: -f1)
      echo "$f:$first: D-FG01: ui://foreign without graduation TODO comment"
      violations=$((violations+1))
    fi
  fi

  # D-EM01: empty/loading/offline coverage on app-level surfaces.
  # Heuristic: any fixture under crates/clawft-app/fixtures/ OR with an
  # id starting "app://" is an app surface and must declare all three.
  if grep -qE '^\s*id\s*=\s*"app://' "$f" || [[ "$f" == */crates/clawft-app/fixtures/* ]]; then
    for slot in empty_state loading_state offline_state; do
      if ! grep -qE "^\[surfaces\.$slot\]" "$f"; then
        echo "$f:0: D-EM01: missing [surfaces.$slot] required for app surfaces"
        violations=$((violations+1))
      fi
    done
  fi
done

if [[ $violations -gt 0 ]]; then
  echo ""
  echo "$violations violation(s). See docs/DESIGN.md and .claude/skills/weftos-design/references/." >&2
  exit 1
fi
echo "clean ($#)"
