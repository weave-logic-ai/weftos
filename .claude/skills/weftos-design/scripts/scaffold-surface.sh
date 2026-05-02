#!/usr/bin/env bash
# Scaffold a new WeftOS surface fixture from an archetype.
#
# Usage:
#   scaffold-surface.sh --archetype <id> --substrate <path> --out <file>
#
# Archetypes: app-window | chip-detail | tile-grid | list-detail | stream
# Substrate:  e.g. substrate/fs, substrate/kernel/services
# Out:        target TOML path
#
# Source: docs/DESIGN.md §4 + .claude/skills/weftos-design/references/archetypes.md.

set -euo pipefail

ARCH=""
SUB=""
OUT=""
ID=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --archetype) ARCH="$2"; shift 2 ;;
    --substrate) SUB="$2"; shift 2 ;;
    --out)       OUT="$2"; shift 2 ;;
    --id)        ID="$2"; shift 2 ;;
    -h|--help)
      sed -n '3,20p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$ARCH" || -z "$SUB" || -z "$OUT" ]]; then
  echo "missing required arg" >&2
  echo "usage: $0 --archetype <id> --substrate <path> --out <file> [--id <surface-id>]" >&2
  exit 2
fi

if [[ -z "$ID" ]]; then
  ID="app://weftos.$(basename "$SUB")"
fi

if [[ -e "$OUT" ]]; then
  echo "refuse to overwrite: $OUT" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUT")"

case "$ARCH" in
  app-window)
    cat > "$OUT" <<TOML
# Scaffolded by weftos-design skill — see docs/DESIGN.md §4.1
[[surfaces]]
id     = "$ID/main"
modes  = ["desktop"]
inputs = ["pointer", "hybrid"]
title  = "$(basename "$SUB" | sed 's/.*/\u&/')"
subscriptions = ["$SUB/*"]

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "horizontal" }

  [[surfaces.root.children]]
  type  = "ui://dock"
  id    = "/root/dock"
  attrs = { position = "left" }

  [[surfaces.root.children]]
  type  = "ui://stack"
  id    = "/root/content"
  attrs = { axis = "vertical" }

# REQUIRED — DESIGN.md §5
[surfaces.empty_state]
type = "ui://stack"
id   = "/empty"
attrs = { axis = "vertical" }

[surfaces.loading_state]
type = "ui://stack"
id   = "/loading"
attrs = { axis = "vertical" }

[surfaces.offline_state]
type     = "ui://chip"
id       = "/offline"
bindings = { tone = '"crit"', label = '"◉ Demo mode — kernel daemon offline"' }
TOML
    ;;
  chip-detail)
    cat > "$OUT" <<TOML
# Scaffolded by weftos-design skill — see docs/DESIGN.md §4.2
[[surfaces]]
id     = "$ID"
modes  = ["desktop"]
inputs = ["pointer"]
title  = "$(basename "$SUB" | sed 's/.*/\u&/')"
subscriptions = ["$SUB"]

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "vertical" }

  [[surfaces.root.children]]
  type     = "ui://chip"
  id       = "/root/state"
  bindings = { label = "\$$SUB.state", tone = "\$$SUB.state" }
TOML
    ;;
  tile-grid)
    cat > "$OUT" <<TOML
# Scaffolded by weftos-design skill — see docs/DESIGN.md §4.3
[[surfaces]]
id     = "$ID/main"
modes  = ["desktop"]
inputs = ["pointer"]
title  = "$(basename "$SUB" | sed 's/.*/\u&/')"

[surfaces.root]
type  = "ui://grid"
id    = "/root"
attrs = { cols = 6, gap = 12 }
TOML
    ;;
  list-detail)
    cat > "$OUT" <<TOML
# Scaffolded by weftos-design skill — see docs/DESIGN.md §4.4
[[surfaces]]
id     = "$ID/main"
modes  = ["desktop"]
title  = "$(basename "$SUB" | sed 's/.*/\u&/')"

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "horizontal" }

  [[surfaces.root.children]]
  type     = "ui://tree"
  id       = "/root/list"
  bindings = { root = "\$$SUB" }
  attrs    = { depth_limit = 3, lazy = true }

  [[surfaces.root.children]]
  type  = "ui://stack"
  id    = "/root/detail"
  attrs = { axis = "vertical" }
TOML
    ;;
  stream)
    cat > "$OUT" <<TOML
# Scaffolded by weftos-design skill — see docs/DESIGN.md §4.5
[[surfaces]]
id     = "$ID/main"
modes  = ["desktop"]
title  = "$(basename "$SUB" | sed 's/.*/\u&/')"
subscriptions = ["$SUB/*"]

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "vertical" }

  [[surfaces.root.children]]
  type     = "ui://stream"
  id       = "/root/tail"
  bindings = { lines = "\$$SUB.lines" }
  attrs    = { tail = true, filter_chips = ["info", "warn", "error"] }
TOML
    ;;
  *)
    echo "unknown archetype: $ARCH" >&2
    echo "valid: app-window | chip-detail | tile-grid | list-detail | stream" >&2
    exit 2 ;;
esac

echo "wrote $OUT"
echo "next: bash $(dirname "$0")/audit-surface.sh $OUT"
