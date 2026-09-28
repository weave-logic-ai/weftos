#!/usr/bin/env python3
"""Build / query the WeftOS Plane dependency DAG.

Used by scripts/plane-dag.sh. Can also be invoked directly:

  python3 scripts/plane-dag-build.py --root .                 # full refresh
  python3 scripts/plane-dag-build.py --root . --cmd ready -- --cycle 0.8.x

Requires PLANE_API_KEY for refresh. Offline cmds (ready/waves/show/path/mermaid)
only need docs/plans/plane-dag.json.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter, defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# Domain-inferred dependency chains (blocker → blocked)
# Override by commenting on Plane; re-run refresh after description edits.
# ---------------------------------------------------------------------------

INFERRED_NAME_EDGES: list[tuple[str, str, str]] = [
    # LeWM
    (r"LeWM — codify ADR-058", r"LeWM — create weftos-worldmodel-core", "LeWM invariants before core"),
    (r"LeWM — decide 192-dim", r"LeWM — create weftos-worldmodel-core", "dim decision before core types"),
    (r"LeWM — create weftos-worldmodel-core", r"LeWM — create weftos-worldmodel-impls", "core traits before impls"),
    (r"LeWM — create weftos-worldmodel-impls", r"LeWM — create weftos-worldmodel facade", "impls before facade"),
    (r"LeWM — create weftos-worldmodel facade", r"LeWM — create weftos-sensor-pipeline", "facade before sensor"),
    (r"LeWM — create weftos-worldmodel facade", r"LeWM — create clawft-worldmodel-service", "facade before service"),
    (r"LeWM — create weftos-worldmodel facade", r"LeWM — create clawft-delegation", "facade before delegation"),
    (r"LeWM — create weftos-sensor-pipeline", r"LeWM — add mesh\.sensor", "sensor crates before mesh topics"),
    (r"LeWM — create weftos-worldmodel facade", r"LeWM — implement LatticeApi", "facade before LatticeApi"),
    (r"LeWM — implement LatticeApi", r"LeWM — wire SIGReg", "Lattice before SIGReg"),
    (r"LeWM — implement LatticeApi", r"LeWM — implement pred_φ", "Lattice before planner"),
    (r"LeWM — implement pred_φ", r"LeWM — implement four-condition AND rollback", "planner before rollback"),
    (r"LeWM — wire SIGReg", r"LeWM — implement four-condition AND rollback", "SIGReg before rollback"),
    (r"LeWM — create weftos-worldmodel facade", r"LeWM — ExoChain attestation", "facade before attestation"),
    # Mesh / K6
    (r"define missing K6 protocol struct", r"implement K6\.4 chain replay", "protocol types before chain replay"),
    (r"define missing K6 protocol struct", r"implement K6\.4 tree Merkle", "protocol types before merkle"),
    (r"define Clock trait", r"implement S10 key-rotation", "clock before key rotation"),
    (r"InMemoryTransport / MockPeer", r"implement K6\.4 chain replay", "test harness before chain replay"),
    (r"decide chain merge strategy", r"implement K6\.4 chain replay", "merge decision before replay"),
    (r"add MutationEvent Ed25519", r"implement K6\.4 tree Merkle", "signing before merkle mutations"),
    (r"implement DelegationCert lifecycle", r"full K1 ACL engine", "DelegationCert before full ACL"),
    (r"ADR-057 substrate per-path read ACLs", r"wire real ADR-012 governance::Gate", "path ACLs before gate wire"),
    # Vector / memory (architect 2026-07-30: DROP hard 640→641; SOFTEN 641→642)
    (r"DiskAnnBackend::search hardcodes SearchResult\.id=0", r"HybridBackend merges cosine", "fix id=0 before hybrid metrics"),
    # SOFT: AtomRegistry preferred before HNSW join validation, not a hard gate —
    # 642 can ship as surgical democritus chain_seq fix without full registry.
    # (r"AtomRegistry \+ atom\.locate", r"ECC brain HNSW cannot join", "soft prefer only"),
    (r"MCP server runs 'npx ruflo@latest'", r"memory_import drops the tags", "pin schema before import fixes"),
    (r"RMM retrospective", r"RMM prospective", "citation ids + non-dump inject before topic merge is observable"),
    # Voice (architect: DROP hard 644→615 Silero; KEEP 628→615 Phase1)
    (r"ADR-068 Phase 1", r"Re-enable barge-in", "Phase1 ERL before barge-in"),
    (r"decide the disposition of clawft-plugin/src/voice", r"Voicelab parity", "disposition before more voice ports"),
    (r"Re-enable barge-in", r"Voice cutover eventually retires TalkForest", "barge-in path before cutover"),
    # Agent (architect: SOFTEN 605→651; DROP hard 651→673)
    # Release
    (r"cargo-dist stopped publishing", r"release Docker image strategy", "binaries before docker strategy"),
    # NOTE: do NOT couple WEFT-663 (Send-future) with WEFT-672 (strip_think) —
    # they are independent WASM breaks (architect + builder both reject).
    # NOTE: do NOT hard-edge Real embedder (640) → AtomRegistry (641) — ADR-069
    # panopticon mints at anchor_turn; embedder quality is parallel.
]


LANE_TABLE = {
    "A": "Release/CI (ws14)",
    "B": "Kernel/Mesh (ws02, ws13)",
    "C": "Memory/Vector (ws06, ws12)",
    "D": "Agent/Hermes (ws11, ws07)",
    "E": "Voice (ws10)",
    "F": "UI/Surface (ws08, ws09, ws18)",
    "G": "WASM/Browser (ws16)",
    "H": "Channels (ws05)",
    "I": "Research/LeWM (ws17)",
    "J": "Tooling/Plane (ws15, tests)",
}


def pri_rank(p: str) -> int:
    return {"urgent": 0, "high": 1, "medium": 2, "low": 3, "none": 4}.get(p or "none", 5)


def ws_of(x: dict) -> str:
    for l in x.get("labels") or []:
        if re.match(r"ws\d+", l or ""):
            return l
    m = re.match(r"^(ws\d+)", (x.get("name") or ""), re.I)
    return m.group(1).lower() if m else "other"


def ac_quality(desc: str) -> dict:
    d = desc or ""
    has_ac = bool(re.search(r"(?i)acceptance", d))
    checks = len(re.findall(r"(?m)^\s*(?:-\s*)?\[[ xX]?\]", d))
    has_source = bool(re.search(r"(?i)(?:^|\n)\s*#*\s*Source\b", d))
    has_problem = bool(re.search(r"(?i)(?:^|\n)\s*#*\s*(Problem|gap)\b", d))
    has_deps = bool(re.search(r"(?i)(?:^|\n)\s*#*\s*Dependencies\b", d))
    thin = len(d.strip()) < 150
    score = sum([has_ac, checks >= 2, has_source, has_problem, has_deps, not thin])
    if thin or (not has_ac and score <= 2):
        level = "weak"
    elif score <= 3 or not has_ac:
        level = "ok"
    else:
        level = "strong"
    return {
        "level": level,
        "score": score,
        "checkboxes": checks,
        "has_ac": has_ac,
        "thin": thin,
        "has_source": has_source,
        "has_problem": has_problem,
        "has_deps_section": has_deps,
    }


def lane_of(n: dict) -> str:
    ws = n.get("workstream") or ""
    name = (n.get("name") or "").lower()
    if ws.startswith("ws14") or any(k in name for k in ("wasmtime", "cargo-dist", "docker", "release")):
        return "A"
    if ws.startswith("ws02") or ws.startswith("ws13") or "mesh" in name:
        return "B"
    if ws.startswith("ws06") or ws.startswith("ws12") or any(
        k in name for k in ("vector", "atom", "embedder", "diskann", "hnsw")
    ):
        return "C"
    if ws.startswith("ws11") or ws.startswith("ws07") or "hermes" in name or "agent loop" in name:
        return "D"
    if ws.startswith("ws10") or any(k in name for k in ("voice", "barge", "silero", "tts")):
        return "E"
    if ws.startswith("ws08") or ws.startswith("ws09") or ws.startswith("ws18") or "leaf" in name:
        return "F"
    if ws.startswith("ws16") or "browser" in name or "wasm" in name:
        return "G"
    if ws.startswith("ws05"):
        return "H"
    if ws.startswith("ws17") or "LeWM" in (n.get("name") or ""):
        return "I"
    if ws.startswith("ws15") or "plane" in name or name.startswith("tests:"):
        return "J"
    return "B"


def load_plane(root: Path):
    sys.path.insert(0, str(root / ".claude/skills/plane-workflow/scripts"))
    from plane import request, load_ids, paginate  # type: ignore

    return request, load_ids, paginate


def fetch_all_issues(root: Path) -> tuple[list[dict], dict[str, list[str]], dict]:
    request, load_ids, paginate = load_plane(root)
    ids = load_ids()

    raw_states = request("GET", "/states/")
    state_list = raw_states.get("results", raw_states) if isinstance(raw_states, dict) else raw_states
    state_by_id = {s["id"]: s for s in state_list}

    label_list = list(paginate("/labels/"))
    label_by_id = {l["id"]: l for l in label_list}

    all_issues = list(paginate("/issues/"))

    cycle_map: dict[str, list[str]] = {}
    for cname, cid in ids["cycles"].items():
        for r in paginate(f"/cycles/{cid}/cycle-issues/"):
            iid = r.get("issue") or r.get("id")
            if isinstance(iid, dict):
                iid = iid.get("id")
            if not iid and isinstance(r.get("issue_detail"), dict):
                iid = r["issue_detail"].get("id")
            if iid:
                cycle_map.setdefault(iid, []).append(cname)

    slim = []
    for i in all_issues:
        sid = i.get("state")
        if isinstance(sid, dict):
            state = sid
        else:
            state = state_by_id.get(sid, {"id": sid, "name": "?"})
        labels_raw = i.get("labels") or []
        label_names = []
        for l in labels_raw:
            if isinstance(l, dict):
                label_names.append(l.get("name"))
            else:
                label_names.append((label_by_id.get(l) or {}).get("name") or str(l))
        for l in i.get("label_details") or []:
            if isinstance(l, dict) and l.get("name") and l["name"] not in label_names:
                label_names.append(l["name"])
        desc = i.get("description_stripped") or ""
        if not desc and i.get("description_html"):
            desc = re.sub(r"<[^>]+>", " ", i["description_html"])
            desc = re.sub(r"\s+", " ", desc).strip()
        deps_mentioned = sorted(set(re.findall(r"WEFT-\d+", desc + " " + (i.get("name") or ""))))
        slim.append(
            {
                "id": i.get("id"),
                "sequence_id": i.get("sequence_id"),
                "weft": f"WEFT-{i.get('sequence_id')}",
                "name": i.get("name"),
                "priority": i.get("priority") or "none",
                "state": state.get("name") if isinstance(state, dict) else str(state),
                "state_group": state.get("group") if isinstance(state, dict) else None,
                "labels": [x for x in label_names if x],
                "cycles": cycle_map.get(i.get("id"), []),
                "description": (desc or "")[:8000],
                "parent": i.get("parent"),
                "created_at": i.get("created_at"),
                "updated_at": i.get("updated_at"),
                "completed_at": i.get("completed_at"),
                "assignees": i.get("assignees") or [],
                "sub_issues_count": i.get("sub_issues_count") or 0,
                "estimate_point": i.get("estimate_point"),
                "deps_mentioned": deps_mentioned,
            }
        )
    return slim, cycle_map, ids


def build_edges(raw: list[dict]) -> tuple[dict, dict, list]:
    by_weft = {x["weft"]: x for x in raw}
    blocked_by_map: dict[str, set[str]] = defaultdict(set)
    blocks_map: dict[str, set[str]] = defaultdict(set)
    inferred: list[tuple[str, str, str]] = []

    def add_edge(blocker: str, blocked: str) -> None:
        if blocker == blocked or blocker not in by_weft or blocked not in by_weft:
            return
        blocked_by_map[blocked].add(blocker)
        blocks_map[blocker].add(blocked)

    for x in raw:
        w = x["weft"]
        desc = x.get("description") or ""
        name = x.get("name") or ""
        text = name + "\n" + desc
        m = re.search(
            r"(?is)(?:^|\n)\s*#*\s*Dependencies\s*(.*?)(?:\n\s*#*\s*[A-Z][a-z].{0,30}\n|\Z)",
            desc,
        )
        section = m.group(1) if m else ""
        for line in (section or desc).splitlines():
            if re.search(r"(?i)blocked[- ]by|depends|requires|after|parent", line):
                for d in re.findall(r"WEFT-\d+", line):
                    add_edge(d, w)
            if re.search(r"(?i)\bblocks\b|unblocks|enables|follow-?ups?", line):
                for d in re.findall(r"WEFT-\d+", line):
                    add_edge(w, d)
        for m in re.finditer(
            r"(?i)(?:blocked[- ]by|depends on|blocked on|requires|follow-?up to|follow-?up of|after)\s*[:=]?\s*((?:WEFT-\d+[\s,]*)+)",
            text,
        ):
            for d in re.findall(r"WEFT-\d+", m.group(1)):
                add_edge(d, w)
        for m in re.finditer(
            r"(?i)(?:blocks|unblocks|enables)\s*[:=]?\s*((?:WEFT-\d+[\s,]*)+)",
            text,
        ):
            for d in re.findall(r"WEFT-\d+", m.group(1)):
                add_edge(w, d)
        if x.get("parent"):
            for y in raw:
                if y["id"] == x["parent"]:
                    add_edge(y["weft"], w)
                    break

    open_ = [x for x in raw if x["state"] in ("Todo", "Backlog", "In Progress")]

    def first(pat: str) -> str | None:
        for x in open_:
            if re.search(pat, x.get("name") or "", re.I):
                return x["weft"]
        return None

    for a_pat, b_pat, reason in INFERRED_NAME_EDGES:
        wa, wb = first(a_pat), first(b_pat)
        if wa and wb:
            inferred.append((wa, wb, reason))
            add_edge(wa, wb)

    return blocked_by_map, blocks_map, inferred


def compute_waves(open_wefts: set[str], blocked_by_map, blocks_map, nodes) -> tuple[list, list]:
    remaining = {
        w: set(d for d in blocked_by_map.get(w, set()) if d in open_wefts) for w in open_wefts
    }
    waves: list[list[str]] = []
    placed: set[str] = set()
    while True:
        ready = [w for w in open_wefts if w not in placed and not remaining[w]]
        if not ready:
            break
        ready.sort(
            key=lambda w: (
                0
                if "0.8.x" in nodes[w]["cycles"]
                else 1
                if "0.7.x" in nodes[w]["cycles"]
                else 2
                if "0.9.x" in nodes[w]["cycles"]
                else 3,
                -{"urgent": 4, "high": 3, "medium": 2, "low": 1, "none": 0}.get(
                    nodes[w]["priority"], 0
                ),
                nodes[w]["sequence_id"] or 0,
            )
        )
        waves.append(ready)
        for w in ready:
            placed.add(w)
            for child in blocks_map.get(w, set()):
                if child in remaining:
                    remaining[child].discard(w)
        if len(waves) > 80:
            break
    stuck = sorted(open_wefts - placed, key=lambda w: nodes[w]["sequence_id"] or 0)
    return waves, stuck


def build_dag(raw: list[dict]) -> dict:
    open_states = {"Todo", "Backlog", "In Progress"}
    open_ = [x for x in raw if x["state"] in open_states]
    blocked_by_map, blocks_map, inferred = build_edges(raw)

    nodes = {}
    for x in raw:
        q = ac_quality(x.get("description") or "")
        nodes[x["weft"]] = {
            "weft": x["weft"],
            "id": x["id"],
            "sequence_id": x["sequence_id"],
            "name": x["name"],
            "state": x["state"],
            "priority": x["priority"],
            "labels": x.get("labels") or [],
            "cycles": x.get("cycles") or [],
            "workstream": ws_of(x),
            "blocked_by": sorted(blocked_by_map.get(x["weft"], [])),
            "blocks": sorted(blocks_map.get(x["weft"], [])),
            "ac": q,
            "description": (x.get("description") or "")[:2000],
            "open": x["state"] in open_states,
            "updated_at": x.get("updated_at"),
            "lane": None,  # filled below
        }
    for w, n in nodes.items():
        n["lane"] = lane_of(n)

    open_wefts = {x["weft"] for x in open_}
    waves, stuck = compute_waves(open_wefts, blocked_by_map, blocks_map, nodes)

    return {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "totals": {
            "all": len(raw),
            "open": len(open_),
            "done": sum(1 for x in raw if x["state"] == "Done"),
            "cancelled": sum(1 for x in raw if x["state"] == "Cancelled"),
            "in_progress": sum(1 for x in raw if x["state"] == "In Progress"),
        },
        "state_counts": dict(Counter(x["state"] for x in raw)),
        "cycle_counts_all": dict(
            Counter((x["cycles"][0] if x["cycles"] else "none") for x in raw)
        ),
        "cycle_counts_open": dict(
            Counter((x["cycles"][0] if x["cycles"] else "none") for x in open_)
        ),
        "workstream_open": dict(Counter(ws_of(x) for x in open_)),
        "priority_open": dict(Counter(x["priority"] for x in open_)),
        "ac_open": dict(Counter(nodes[w]["ac"]["level"] for w in open_wefts)),
        "inferred_edges": [{"from": a, "to": b, "reason": r} for a, b, r in inferred],
        "nodes": nodes,
        "waves": waves,
        "stuck": stuck,
        "edges": {
            "blocked_by": {k: sorted(v) for k, v in blocked_by_map.items() if v},
            "blocks": {k: sorted(v) for k, v in blocks_map.items() if v},
        },
    }


def esc(s: str) -> str:
    return (s or "").replace("|", "\\|").replace("\n", " ")


def write_inventory(root: Path, raw: list[dict], dag: dict) -> None:
    nodes = dag["nodes"]
    waves = dag["waves"]
    open_wefts = {w for w, n in nodes.items() if n["open"]}
    done_wefts = {w for w, n in nodes.items() if n["state"] == "Done"}
    lines = [
        "# Plane Board Inventory — WeftOS",
        "",
        f"> Generated: {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}",
        "> Source: Plane workspace `weftos`",
        f"> Machine-readable DAG: [`plane-dag.json`](./plane-dag.json)",
        f"> Wave plan: [`plane-wave-plan.md`](./plane-wave-plan.md)",
        "",
        "## Summary",
        "",
        "| Metric | Count |",
        "|--------|------:|",
        f"| Total tickets | {dag['totals']['all']} |",
        f"| Open | {dag['totals']['open']} |",
        f"| In Progress | {dag['totals']['in_progress']} |",
        f"| Done | {dag['totals']['done']} |",
        f"| Cancelled | {dag['totals']['cancelled']} |",
        f"| Dependency edges | {sum(len(v) for v in dag['edges']['blocked_by'].values())} |",
        f"| Inferred domain edges | {len(dag['inferred_edges'])} |",
        f"| Parallel waves | {len(waves)} |",
        "",
        "### Open by cycle",
        "",
    ]
    for s, c in sorted(dag["cycle_counts_open"].items(), key=lambda kv: -kv[1]):
        lines.append(f"- **{s}**: {c}")
    lines += ["", "### Open by workstream", ""]
    for s, c in sorted(dag["workstream_open"].items(), key=lambda kv: -kv[1]):
        lines.append(f"- **{s}**: {c}")
    lines += ["", "### Open by priority", ""]
    for s, c in sorted(dag["priority_open"].items(), key=lambda kv: pri_rank(kv[0])):
        lines.append(f"- **{s}**: {c}")
    lines += [
        "",
        "---",
        "",
        "## Complete ticket table",
        "",
        "| WEFT | State | Pri | Cycle | WS | Lane | AC | Blocked-by | Blocks | Name |",
        "|------|-------|-----|-------|----|------|----|------------|--------|------|",
    ]
    for x in sorted(raw, key=lambda z: z["sequence_id"] or 0):
        n = nodes[x["weft"]]
        cyc = ",".join(n["cycles"]) or "—"
        bb = ",".join(n["blocked_by"][:4]) or "—"
        bl = ",".join(n["blocks"][:3]) or "—"
        lines.append(
            f"| {n['weft']} | {n['state']} | {n['priority']} | {cyc} | {n['workstream']} | {n['lane']} | {n['ac']['level']} | {bb} | {bl} | {esc(n['name'])[:90]} |"
        )

    lines += ["", "---", "", "## Open tickets by workstream", ""]
    by_ws: dict[str, list] = defaultdict(list)
    for x in raw:
        if x["state"] in ("Todo", "Backlog", "In Progress"):
            by_ws[ws_of(x)].append(x)
    for ws in sorted(by_ws.keys()):
        items = sorted(by_ws[ws], key=lambda z: (pri_rank(z["priority"]), z["sequence_id"]))
        lines += [f"### {ws} ({len(items)} open)", ""]
        for x in items:
            n = nodes[x["weft"]]
            cyc = ",".join(n["cycles"]) or "—"
            bb = ", ".join(n["blocked_by"]) or "none"
            bl = ", ".join(n["blocks"]) or "none"
            wave_idx = next((i for i, w in enumerate(waves) if x["weft"] in w), None)
            wave_s = f"W{wave_idx}" if wave_idx is not None else "?"
            d = (x.get("description") or "").strip()
            pm = re.search(
                r"(?is)Problem\s*/\s*gap\s*(.*?)(?:Acceptance|Dependencies|Notes|\Z)", d
            )
            problem = (pm.group(1).strip() if pm else d[:280]).replace("\n", " ")
            problem = re.sub(r"\s+", " ", problem)[:320]
            plan_bits = []
            open_blockers = [b for b in n["blocked_by"] if b in open_wefts]
            if open_blockers:
                plan_bits.append(f"Wait for {', '.join(open_blockers)}")
            if n["priority"] in ("high", "urgent") and "0.8.x" in n["cycles"]:
                plan_bits.append("0.8.x gate — early wave")
            if n["ac"]["level"] == "weak":
                plan_bits.append("Strengthen AC before coding")
            if not plan_bits:
                plan_bits.append("Ready when claimed; implement to AC; close with commit+tests+build")
            lines += [
                f"#### {n['weft']} — {esc(n['name'])}",
                "",
                f"- **State**: {n['state']} · **Priority**: {n['priority']} · **Cycle**: {cyc} · **Wave**: {wave_s} · **Lane**: {n['lane']} · **AC**: {n['ac']['level']}",
                f"- **Labels**: {', '.join(n['labels']) or '—'}",
                f"- **Blocked by**: {bb}",
                f"- **Blocks**: {bl}",
                f"- **Gap**: {esc(problem)}",
                f"- **Plan**: {'; '.join(plan_bits)}",
                "",
            ]

    out = root / "docs/plans/plane-board-inventory.md"
    out.write_text("\n".join(lines) + "\n")


def write_wave_plan(root: Path, dag: dict) -> None:
    nodes = dag["nodes"]
    waves = dag["waves"]
    open_wefts = {w for w, n in nodes.items() if n["open"]}
    blocked_by_map = {k: set(v) for k, v in dag["edges"]["blocked_by"].items()}
    blocks_map = {k: set(v) for k, v in dag["edges"]["blocks"].items()}
    inferred = dag["inferred_edges"]

    wlines = [
        "# Plane Dependency DAG & Wave Plan",
        "",
        f"> Generated: {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}",
        "> Full inventory: [`plane-board-inventory.md`](./plane-board-inventory.md)",
        "> DAG data: [`plane-dag.json`](./plane-dag.json)",
        "> Skill: `.grok/skills/plane-dag/SKILL.md`",
        "> Helper: `scripts/plane-dag.sh`",
        "",
        "## Graph model",
        "",
        "```",
        "Node  = Plane work item (WEFT-N)",
        "Edge  = blocked_by (A → B means B waits for A Done)",
        "Wave  = open nodes whose open blockers are empty",
        "Lane  = parallel track inside a wave (A–J)",
        "```",
        "",
        "### Inferred edges",
        "",
        "| From | To | Reason |",
        "|------|----|--------|",
    ]
    for e in inferred:
        wlines.append(f"| {e['from']} | {e['to']} | {esc(e['reason'])} |")

    wlines += [
        "",
        "## Wave overview",
        "",
        "| Wave | Tickets | High/Urgent | In 0.8.x |",
        "|-----:|--------:|------------:|---------:|",
    ]
    for i, wave in enumerate(waves):
        highs = sum(1 for w in wave if nodes[w]["priority"] in ("high", "urgent"))
        c08 = sum(1 for w in wave if "0.8.x" in nodes[w]["cycles"])
        wlines.append(f"| {i} | {len(wave)} | {highs} | {c08} |")
    if dag["stuck"]:
        wlines.append(f"| stuck | {len(dag['stuck'])} | — | — |")

    # critical paths
    memo: dict[str, tuple[int, list[str]]] = {}

    def depth(w: str, stack: set[str] | None = None) -> tuple[int, list[str]]:
        stack = stack or set()
        if w in memo:
            return memo[w]
        if w in stack:
            return (0, [])
        kids = [c for c in blocks_map.get(w, set()) if c in open_wefts]
        if not kids:
            memo[w] = (1, [w])
            return memo[w]
        stack.add(w)
        best = (0, [])
        for c in kids:
            d, p = depth(c, stack)
            if d + 1 > best[0]:
                best = (d + 1, [w] + p)
        stack.discard(w)
        memo[w] = best
        return best

    paths = []
    for w in open_wefts:
        if not any(b in open_wefts for b in blocked_by_map.get(w, set())):
            d, p = depth(w)
            if d > 1:
                paths.append((d, p))
    paths.sort(reverse=True)

    wlines += ["", "## Critical paths", ""]
    for d, p in paths[:12]:
        wlines.append(f"- **len {d}**: {' → '.join(p)}")

    wlines += [
        "",
        "## Lanes",
        "",
        "| Lane | Focus |",
        "|------|-------|",
    ]
    for k, v in LANE_TABLE.items():
        wlines.append(f"| {k} | {v} |")

    wlines += ["", "## Wave 0 — ready now (0.8.x high/medium first)", ""]
    if waves:
        wave0 = waves[0]
        show = [
            w
            for w in wave0
            if nodes[w]["priority"] in ("high", "urgent", "medium")
            or "0.8.x" in nodes[w]["cycles"]
            or nodes[w]["state"] == "In Progress"
        ]
        show.sort(
            key=lambda w: (
                0 if nodes[w]["state"] == "In Progress" else 1,
                0 if "0.8.x" in nodes[w]["cycles"] else 1,
                pri_rank(nodes[w]["priority"]),
                nodes[w]["sequence_id"] or 0,
            )
        )
        for w in show[:80]:
            n = nodes[w]
            cyc = ",".join(n["cycles"]) or "—"
            flag = " 🔧WIP" if n["state"] == "In Progress" else ""
            if n["ac"]["level"] == "weak":
                flag += " ⚠️weak-spec"
            unlock = f" → unlocks {', '.join(n['blocks'][:5])}" if n["blocks"] else ""
            wlines.append(
                f"- **{w}** [L{n['lane']}/{n['priority']}/{cyc}]{flag} — {esc(n['name'])[:100]}{unlock}"
            )
        extra = len(wave0) - len(show)
        if extra > 0:
            wlines.append(f"- _…plus {len(wave0) - min(len(show), 80)} more ready items (see inventory)_")

    wlines += [
        "",
        "## Commands",
        "",
        "```bash",
        "scripts/plane-dag.sh refresh",
        "scripts/plane-dag.sh ready --cycle 0.8.x --priority high",
        "scripts/plane-dag.sh show WEFT-593",
        "scripts/plane-dag.sh claim WEFT-593",
        "scripts/plane-dag.sh done WEFT-593 --shipped '...' --commits abc123 --tests 'scripts/build.sh test' --build 'scripts/build.sh check'",
        "```",
        "",
        "## Lifecycle",
        "",
        "| Event | Action |",
        "|-------|--------|",
        "| Claim | `plane-dag.sh claim WEFT-N` |",
        "| Note | `plane-dag.sh note WEFT-N \"...\"` |",
        "| Done | `plane-dag.sh done WEFT-N --shipped ... --commits ...` then `refresh` |",
        "| Defer | `plane.sh defer <uuid> 0.9.x --reason \"...\"` |",
        "",
    ]
    (root / "docs/plans/plane-wave-plan.md").write_text("\n".join(wlines) + "\n")


def full_refresh(root: Path) -> None:
    print("fetching Plane issues…", flush=True)
    raw, _, _ = fetch_all_issues(root)
    print(f"  {len(raw)} issues", flush=True)
    (root / "docs/plans/plane-issues-raw.json").write_text(json.dumps(raw, indent=2))
    open_ = [x for x in raw if x["state"] in ("Todo", "Backlog", "In Progress")]
    (root / "docs/plans/plane-issues-open.json").write_text(
        json.dumps(sorted(open_, key=lambda z: z.get("sequence_id") or 0), indent=2)
    )
    dag = build_dag(raw)
    (root / "docs/plans/plane-dag.json").write_text(json.dumps(dag, indent=2))
    write_inventory(root, raw, dag)
    write_wave_plan(root, dag)
    print(
        f"open={dag['totals']['open']} waves={len(dag['waves'])} "
        f"sizes={[len(w) for w in dag['waves']]} stuck={dag['stuck']}",
        flush=True,
    )


def load_dag(root: Path) -> dict:
    return json.loads((root / "docs/plans/plane-dag.json").read_text())


def cmd_ready(dag: dict, args: list[str]) -> None:
    cycle = priority = lane = None
    limit = 50
    i = 0
    while i < len(args):
        if args[i] == "--cycle" and i + 1 < len(args):
            cycle = args[i + 1]
            i += 2
        elif args[i] == "--priority" and i + 1 < len(args):
            priority = args[i + 1]
            i += 2
        elif args[i] == "--lane" and i + 1 < len(args):
            lane = args[i + 1].upper()
            i += 2
        elif args[i] == "--limit" and i + 1 < len(args):
            limit = int(args[i + 1])
            i += 2
        else:
            i += 1
    if not dag["waves"]:
        print("no waves")
        return
    nodes = dag["nodes"]
    ready = dag["waves"][0]
    out = []
    for w in ready:
        n = nodes[w]
        if cycle and cycle not in n["cycles"]:
            continue
        if priority and n["priority"] != priority:
            continue
        if lane and n.get("lane") != lane:
            continue
        out.append(n)
    out.sort(key=lambda n: (pri_rank(n["priority"]), n["sequence_id"] or 0))
    for n in out[:limit]:
        cyc = ",".join(n["cycles"]) or "—"
        print(
            f"{n['weft']:10} L{n.get('lane','?')} {n['priority']:7} {n['state']:12} {cyc:8} {n['name'][:90]}"
        )
    print(f"# {min(len(out), limit)}/{len(out)} ready (wave 0 filters applied)", file=sys.stderr)


def cmd_waves(dag: dict, _args: list[str]) -> None:
    nodes = dag["nodes"]
    for i, wave in enumerate(dag["waves"]):
        highs = sum(1 for w in wave if nodes[w]["priority"] in ("high", "urgent"))
        c08 = sum(1 for w in wave if "0.8.x" in nodes[w]["cycles"])
        print(f"wave {i}: n={len(wave)} high={highs} cycle0.8={c08}")
    if dag["stuck"]:
        print(f"stuck: {', '.join(dag['stuck'])}")


def cmd_show(dag: dict, args: list[str]) -> None:
    weft = args[0] if args else ""
    if not weft.startswith("WEFT-"):
        if weft.isdigit():
            weft = f"WEFT-{weft}"
    n = dag["nodes"].get(weft)
    if not n:
        print(f"unknown {weft}", file=sys.stderr)
        sys.exit(1)
    wave_idx = next((i for i, w in enumerate(dag["waves"]) if weft in w), None)
    print(json.dumps({**n, "wave": wave_idx}, indent=2))


def cmd_path(dag: dict, args: list[str]) -> None:
    weft = args[0] if args else ""
    if weft.isdigit():
        weft = f"WEFT-{weft}"
    blocks = {k: v for k, v in dag["edges"]["blocks"].items()}
    seen = set()
    q = [weft]
    print(weft)
    while q:
        w = q.pop(0)
        for c in blocks.get(w, []):
            if c not in seen:
                seen.add(c)
                n = dag["nodes"].get(c, {})
                print(f"  → {c} [{n.get('state')}/{n.get('priority')}] {n.get('name','')[:70]}")
                q.append(c)


def cmd_mermaid(dag: dict, args: list[str]) -> None:
    cycle = None
    priority = None
    i = 0
    while i < len(args):
        if args[i] == "--cycle" and i + 1 < len(args):
            cycle = args[i + 1]
            i += 2
        elif args[i] == "--priority" and i + 1 < len(args):
            priority = args[i + 1]
            i += 2
        else:
            i += 1
    nodes = dag["nodes"]
    focus = set()
    for w, n in nodes.items():
        if not n["open"]:
            continue
        if cycle and cycle not in n["cycles"]:
            continue
        if priority and n["priority"] != priority:
            if n["priority"] not in ("high", "urgent") and not priority:
                pass
        if cycle or priority:
            if (not cycle or cycle in n["cycles"]) and (
                not priority or n["priority"] == priority
            ):
                focus.add(w)
        elif n["priority"] in ("high", "urgent") and (
            "0.8.x" in n["cycles"] or (n["sequence_id"] or 0) >= 590
        ):
            focus.add(w)
    for e in dag["inferred_edges"]:
        if e["from"] in focus or e["to"] in focus:
            focus.add(e["from"])
            focus.add(e["to"])
    print("flowchart TD")
    for w in sorted(focus, key=lambda x: nodes[x]["sequence_id"] or 0):
        n = nodes[w]
        safe = w.replace("-", "_")
        label = f"{w}\\n{(n['name'] or '')[:36]}".replace('"', "'")
        print(f'  {safe}["{label}"]')
    for w in focus:
        for b in nodes[w]["blocks"]:
            if b in focus:
                print(f"  {w.replace('-','_')} --> {b.replace('-','_')}")


def cmd_resolve(dag: dict, args: list[str]) -> None:
    weft = args[0]
    if weft.isdigit():
        weft = f"WEFT-{weft}"
    n = dag["nodes"].get(weft)
    if not n:
        sys.exit(f"unknown {weft}")
    print(n["id"])


def cmd_patch_state(dag: dict, args: list[str], root: Path) -> None:
    weft, state = args[0], args[1]
    if weft.isdigit():
        weft = f"WEFT-{weft}"
    if weft in dag["nodes"]:
        dag["nodes"][weft]["state"] = state
        if state == "In Progress":
            dag["nodes"][weft]["open"] = True
        (root / "docs/plans/plane-dag.json").write_text(json.dumps(dag, indent=2))


def cmd_stats(dag: dict, _args: list[str]) -> None:
    print(json.dumps({k: dag[k] for k in ("generated_at", "totals", "state_counts", "cycle_counts_open", "priority_open", "ac_open") if k in dag}, indent=2))
    print(f"waves: {[len(w) for w in dag['waves']]}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=".")
    ap.add_argument("--cmd", default=None, help="query command; omit for full refresh")
    ap.add_argument("rest", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    root = Path(args.root).resolve()
    # strip leading -- from remainder
    rest = args.rest
    if rest and rest[0] == "--":
        rest = rest[1:]

    if args.cmd is None:
        full_refresh(root)
        return

    dag = load_dag(root)
    if args.cmd == "ready":
        cmd_ready(dag, rest)
    elif args.cmd == "waves":
        cmd_waves(dag, rest)
    elif args.cmd == "show":
        cmd_show(dag, rest)
    elif args.cmd == "path":
        cmd_path(dag, rest)
    elif args.cmd == "mermaid":
        cmd_mermaid(dag, rest)
    elif args.cmd == "resolve":
        cmd_resolve(dag, rest)
    elif args.cmd == "patch-state":
        cmd_patch_state(dag, rest, root)
    elif args.cmd == "stats":
        cmd_stats(dag, rest)
    else:
        sys.exit(f"unknown cmd {args.cmd}")


if __name__ == "__main__":
    main()
