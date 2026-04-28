#!/usr/bin/env bash
# Plane workflow CLI wrapper. Thin bash → exec python3 plane.py "$@".
# - Loads PLANE_API_KEY from env, or pulls it from ~/.claude.json.
# - Locates plane.py next to itself.
# - Forwards all args verbatim.
# Usage: scripts/plane.sh <subcommand> [args...]
# See: .claude/skills/plane-workflow/SKILL.md

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -z "${PLANE_API_KEY:-}" ]]; then
  if [[ -f "$HOME/.claude.json" ]]; then
    PLANE_API_KEY="$(python3 -c "
import json, sys
try:
    d = json.load(open('$HOME/.claude.json'))
    print(d.get('mcpServers',{}).get('plane',{}).get('env',{}).get('PLANE_API_KEY',''))
except Exception:
    sys.exit(0)
")"
  fi
fi

if [[ -z "${PLANE_API_KEY:-}" ]]; then
  echo "error: PLANE_API_KEY not found in env or ~/.claude.json mcpServers.plane.env" >&2
  exit 1
fi

export PLANE_API_KEY
exec python3 "$SCRIPT_DIR/plane.py" "$@"
