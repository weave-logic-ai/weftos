#!/usr/bin/env python3
"""Post-batch annotation: stamp each audit doc with the WEFT-N range that
covers its triaged items.

Reads:
  - .planning/reviews/0.7.0-release-gate/triage/*.json (per-batch specs)
  - .planning/reviews/0.7.0-release-gate/triage/weft-mapping.json
    (the {name → "WEFT-N"} map produced by `plane.sh batch-create
    --map-out`)

Then for each audit doc (`NN-*.md`), appends a "Triaged" footer with the
ws-prefixed range and a per-cycle breakdown.

Usage:
    python3 stamp-audit.py
"""

from __future__ import annotations

import json
import re
import sys
from collections import defaultdict
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
AUDIT_DIR = ROOT / ".planning" / "reviews" / "0.7.0-release-gate"
TRIAGE = AUDIT_DIR / "triage"
MAPPING = TRIAGE / "weft-mapping.json"


WS_TO_DOC = {
    "ws01-core":             "01-core-platform.md",
    "ws02-kernel":           "02-kernel-governance.md",
    "ws03-pipeline":         "03-pipeline-routing.md",
    "ws04-plugin-skills":    "04-plugin-skills.md",
    "ws05-channels":         "05-channels.md",
    "ws06-memory":           "06-memory-workspace.md",
    "ws07-multi-agent":      "07-multi-agent-routing.md",
    "ws08-weftos-gui":       "08-weftos-gui.md",
    "ws09-clawft-dashboard": "09-clawft-agent-dashboard.md",
    "ws10-voice":            "10-voice.md",
    "ws11-agent-core-v1":    "11-agent-core-v1.md",
    "ws12-knowledge-graph":  "12-knowledge-graph-graphify.md",
    "ws13-app-substrate":    "13-app-substrate-surface.md",
    "ws14-deployment":       "14-deployment-release.md",
    "ws15-mcp":              "15-mcp-integration.md",
    "ws16-browser-wasm":     "16-browser-wasm.md",
    "ws17-research":         "17-research-streams.md",
}


def main() -> int:
    if not MAPPING.exists():
        print(f"error: mapping not found: {MAPPING}", file=sys.stderr)
        return 1
    mapping: dict[str, str] = json.loads(MAPPING.read_text())

    by_ws: dict[str, list[tuple[int, str, str, str]]] = defaultdict(list)

    for spec_path in sorted(TRIAGE.glob("*.json")):
        if spec_path.name == "weft-mapping.json":
            continue
        items = json.loads(spec_path.read_text())
        for item in items:
            name = item["name"]
            weft = mapping.get(name)
            if not weft:
                continue
            num = int(weft.removeprefix("WEFT-"))
            ws = next((l for l in item["labels"] if l.startswith("ws")), "")
            cycle = item.get("cycle", "")
            by_ws[ws].append((num, weft, cycle, name))

    today = date.today().isoformat()
    for ws, rows in sorted(by_ws.items()):
        doc_name = WS_TO_DOC.get(ws)
        if not doc_name:
            print(f"warn: no audit-doc mapping for {ws}", file=sys.stderr)
            continue
        doc = AUDIT_DIR / doc_name
        if not doc.exists():
            print(f"warn: audit doc missing: {doc}", file=sys.stderr)
            continue
        rows.sort()
        nums = [r[0] for r in rows]
        per_cycle = defaultdict(int)
        for _, _, c, _ in rows:
            per_cycle[c] += 1

        stamp = build_stamp(ws, nums, per_cycle, today)
        text = doc.read_text()
        # Strip an existing stamp before re-applying
        text = re.sub(
            r"\n<!-- TRIAGED-STAMP:BEGIN -->.*?<!-- TRIAGED-STAMP:END -->\n",
            "\n",
            text,
            flags=re.DOTALL,
        )
        text = text.rstrip() + "\n\n" + stamp
        doc.write_text(text)
        print(f"  stamped {doc_name}: {len(rows)} items, "
              f"WEFT-{min(nums)}..WEFT-{max(nums)}")

    return 0


def build_stamp(ws: str, nums: list[int], per_cycle: dict, today: str) -> str:
    cycle_summary = ", ".join(
        f"{c}: {per_cycle[c]}" for c in ("0.7.x", "0.8.x", "0.9.x", "1.0.x")
        if per_cycle.get(c, 0) > 0
    )
    return (
        "<!-- TRIAGED-STAMP:BEGIN -->\n"
        f"## Triaged into Plane — {today}\n\n"
        f"All open items in this audit have been filed as Plane work items in "
        f"the WeftOS workspace under the `{ws}` label.\n\n"
        f"- **Range**: WEFT-{min(nums)} … WEFT-{max(nums)} ({len(nums)} items)\n"
        f"- **Per cycle**: {cycle_summary}\n"
        f"- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`\n"
        f"- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/"
        f"triage/weft-mapping.json`\n\n"
        f"Per the project rule (CLAUDE.md → \"Plane is the authoritative work "
        f"tracker\"): future updates to these items happen in Plane, not in "
        f"this audit doc. This doc remains the source-of-truth for the "
        f"original survey.\n"
        "<!-- TRIAGED-STAMP:END -->\n"
    )


if __name__ == "__main__":
    sys.exit(main())
