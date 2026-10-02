#!/usr/bin/env python3
"""Merge discovered sensor inventories into the hardware catalog.

Reads the existing catalog (authoritative: its hand-curated entries win) and the discovery JSONs
({"modules":[...],"chips":[...]}), merges by exact id (union of list fields, existing scalars kept,
empty scalars backfilled), drops any module->chip link that doesn't resolve, and writes the catalog
back. Exact-id dedup only; near-duplicates of the same part under different ids are left for a later
consolidation pass. Coerces spec values to strings so the typed loader parses cleanly.

Usage: scripts/merge-catalog.py <catalog.json> <discovery1.json> [discovery2.json ...]
"""
import json, sys


def s(v):
    return v if isinstance(v, str) else json.dumps(v) if isinstance(v, (dict, list)) else str(v)


def norm_module(m):
    return {
        "id": m["id"], "name": m.get("name") or m["id"], "vendor": m.get("vendor", ""),
        "kind": m.get("kind", "sensor"), "summary": m.get("summary", ""),
        "good_for": [s(x) for x in m.get("good_for", [])], "not_for": [s(x) for x in m.get("not_for", [])],
        "notes": [s(x) for x in m.get("notes", [])],
        "spec": {str(k): s(v) for k, v in (m.get("spec") or {}).items()},
        "pins": [s(x) for x in m.get("pins", [])], "chips": [s(x) for x in m.get("chips", [])],
        "photo": m.get("photo", ""), "datasheet": m.get("datasheet", ""),
        "mouser_query": m.get("mouser_query", ""), "buy": m.get("buy", []),
        "seen_in": [s(x) for x in m.get("seen_in", [])],
    }


def norm_chip(c):
    return {
        "id": c["id"], "name": c.get("name") or c["id"], "manufacturer": c.get("manufacturer", ""),
        "role": c.get("role", ""), "tags": [s(x) for x in c.get("tags", [])], "summary": c.get("summary", ""),
        "spec": {str(k): s(v) for k, v in (c.get("spec") or {}).items()},
        "mouser_query": c.get("mouser_query", ""), "datasheet": c.get("datasheet", ""),
        "seen_in": [s(x) for x in c.get("seen_in", [])],
    }


def merge_into(existing, new, list_fields):
    for k, v in new.items():
        if k in list_fields:
            have = existing.setdefault(k, [])
            for x in v:
                if x not in have:
                    have.append(x)
        elif k == "spec":
            existing.setdefault("spec", {})
            for sk, sv in v.items():
                existing["spec"].setdefault(sk, sv)
        elif not existing.get(k):  # backfill only empty scalars
            existing[k] = v


def main():
    cat_path, *srcs = sys.argv[1:]
    cat = json.load(open(cat_path))
    mods = {m["id"]: norm_module(m) for m in cat.get("modules", [])}
    chips = {c["id"]: norm_chip(c) for c in cat.get("chips", [])}
    m_list = ["good_for", "not_for", "notes", "pins", "seen_in"]
    c_list = ["tags", "seen_in"]
    added_m = added_c = 0
    for sp in srcs:
        d = json.load(open(sp))
        for m in d.get("modules", []):
            nm = norm_module(m)
            if nm["id"] in mods:
                merge_into(mods[nm["id"]], nm, m_list)
            else:
                mods[nm["id"]] = nm
                added_m += 1
        for c in d.get("chips", []):
            nc = norm_chip(c)
            if nc["id"] in chips:
                merge_into(chips[nc["id"]], nc, c_list)
            else:
                chips[nc["id"]] = nc
                added_c += 1
    # drop dangling chip references
    chipset = set(chips)
    dangling = 0
    for m in mods.values():
        kept = [c for c in m["chips"] if c in chipset]
        dangling += len(m["chips"]) - len(kept)
        m["chips"] = kept
    cat["modules"] = [mods[k] for k in sorted(mods)]
    cat["chips"] = [chips[k] for k in sorted(chips)]
    cat["generated"] = "2026-10-02"
    json.dump(cat, open(cat_path, "w"), indent=2)
    open(cat_path, "a").write("\n")
    print(f"catalog: {len(cat['projects'])} projects, {len(cat['modules'])} modules (+{added_m}), {len(cat['chips'])} chips (+{added_c}); dropped {dangling} unresolved chip refs")


if __name__ == "__main__":
    main()
