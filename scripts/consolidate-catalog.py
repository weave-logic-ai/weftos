#!/usr/bin/env python3
"""Consolidate near-duplicate parts in the WeftOS cog-market catalog.

The catalog (crates/weftos-cog-market/catalog/catalog.json) was merged from three
repos, so the same physical part can appear under several ids (e.g. bme280 /
bme280-env; icm-20948 / tdk-icm20948). This script applies a hand-curated alias
map (scripts/catalog-aliases.json) that folds each duplicate into its canonical
entry, then rewrites every project->module and module->chip reference to the
canonical ids.

Guarantees:
  * Deterministic   — no ordering depends on dict iteration of the catalog; the
                      alias map drives every change in a fixed order.
  * Idempotent      — a second run is a no-op (aliased-away ids are already gone,
                      references already point at canonical ids).
  * Type-safe       — merges happen WITHIN a collection only (module<->module,
                      chip<->chip, project<->project). A module that carries a
                      chip is never merged with that chip.
  * Provenance-safe — list fields (seen_in, notes, good_for, not_for, tags, pins,
                      chips) are unioned; `spec` keys are unioned (canonical wins
                      on conflict); `buy` offers are unioned; the canonical entry
                      keeps its scalars, falling back to the duplicate's value
                      only where the canonical's is empty/missing.

Validation replicates the Rust HwCatalog::validate(): ids unique within each
collection, every project.modules resolves to a module id, every module.chips
resolves to a chip id. Run with --check to validate without writing.

Usage:
    python3 scripts/consolidate-catalog.py            # apply + write catalog.json
    python3 scripts/consolidate-catalog.py --check     # validate only, no write
    python3 scripts/consolidate-catalog.py --dry-run   # show merges, no write
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
CATALOG = REPO / "crates" / "weftos-cog-market" / "catalog" / "catalog.json"
ALIASES = HERE / "catalog-aliases.json"

# List-valued fields that are unioned (canonical first, de-duplicated, order kept).
LIST_FIELDS = ["good_for", "not_for", "notes", "seen_in", "pins", "tags", "chips"]

# Scalar fields: the canonical entry keeps its own value; the duplicate's value is
# adopted only when the canonical's is missing or empty.
SCALAR_FIELDS = [
    "name", "vendor", "manufacturer", "kind", "role",
    "summary", "datasheet", "mouser_query", "photo",
]

COLLECTIONS = ["projects", "modules", "chips"]


def is_empty(v) -> bool:
    return v is None or v == "" or v == [] or v == {}


def union_list(canon: list, other: list) -> list:
    out = list(canon)
    for x in other:
        if x not in out:
            out.append(x)
    return out


def merge_buy(canon: list, other: list) -> list:
    out = list(canon)
    seen = {(b.get("vendor"), b.get("url")) for b in canon}
    for b in other:
        key = (b.get("vendor"), b.get("url"))
        if key not in seen:
            out.append(b)
            seen.add(key)
    return out


def merge_into(canon: dict, other: dict) -> None:
    """Fold duplicate `other` into canonical `canon`, in place."""
    for f in SCALAR_FIELDS:
        if f in other and not is_empty(other[f]):
            if is_empty(canon.get(f)):
                canon[f] = other[f]
    if "spec" in other:
        spec = dict(other["spec"])
        spec.update(canon.get("spec", {}))  # canonical wins on key conflict
        canon["spec"] = spec
    if "buy" in other:
        canon["buy"] = merge_buy(canon.get("buy", []), other["buy"])
    for f in LIST_FIELDS:
        if f in other:
            canon[f] = union_list(canon.get(f, []), other[f])


def remap(ids: list, amap: dict) -> list:
    out = []
    for i in ids:
        c = amap.get(i, i)
        if c not in out:
            out.append(c)
    return out


def consolidate(catalog: dict, aliases: dict) -> list:
    """Apply the alias map. Returns the list of (collection, old_id, canonical) merges."""
    merges = []
    for coll in COLLECTIONS:
        alias = aliases.get(coll, {})
        entries = catalog.get(coll, [])
        by_id = {e["id"]: e for e in entries}
        for old_id in sorted(alias):  # fixed order -> deterministic
            canon_id = alias[old_id]
            if old_id == canon_id:
                continue
            if old_id not in by_id:
                continue  # idempotent: already merged on a previous run
            if canon_id not in by_id:
                sys.exit(f"error: alias target '{canon_id}' missing in '{coll}'")
            merge_into(by_id[canon_id], by_id[old_id])
            merges.append((coll, old_id, canon_id))
        # Drop aliased-away entries, preserving original order.
        catalog[coll] = [e for e in entries if alias.get(e["id"], e["id"]) == e["id"]]

    mod_alias = aliases.get("modules", {})
    chip_alias = aliases.get("chips", {})
    for p in catalog.get("projects", []):
        if "modules" in p:
            p["modules"] = remap(p["modules"], mod_alias)
    for m in catalog.get("modules", []):
        if "chips" in m:
            m["chips"] = remap(m["chips"], chip_alias)
    return merges


def validate(catalog: dict) -> list:
    """Replicate the Rust HwCatalog::validate(): unique ids + resolvable cross-links."""
    errs = []
    for kind, coll in (("project", "projects"), ("module", "modules"), ("chip", "chips")):
        seen = set()
        for e in catalog.get(coll, []):
            if e["id"] in seen:
                errs.append(f"duplicate {kind} id '{e['id']}'")
            seen.add(e["id"])
    module_ids = {m["id"] for m in catalog.get("modules", [])}
    chip_ids = {c["id"] for c in catalog.get("chips", [])}
    for p in catalog.get("projects", []):
        for m in p.get("modules", []):
            if m not in module_ids:
                errs.append(f"project '{p['id']}' -> missing module '{m}'")
    for m in catalog.get("modules", []):
        for c in m.get("chips", []):
            if c not in chip_ids:
                errs.append(f"module '{m['id']}' -> missing chip '{c}'")
    return errs


def counts(catalog: dict) -> dict:
    return {c: len(catalog.get(c, [])) for c in COLLECTIONS}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="validate only; no merge, no write")
    ap.add_argument("--dry-run", action="store_true", help="apply + report, but do not write")
    ap.add_argument("--catalog", type=Path, default=CATALOG)
    ap.add_argument("--aliases", type=Path, default=ALIASES)
    args = ap.parse_args()

    catalog = json.loads(args.catalog.read_text())

    if args.check:
        errs = validate(catalog)
        print("counts:", counts(catalog))
        if errs:
            print("VALIDATION FAILED:")
            for e in errs:
                print("  -", e)
            return 1
        print("validation OK")
        return 0

    aliases = json.loads(args.aliases.read_text())
    before = counts(catalog)
    merges = consolidate(catalog, aliases)
    after = counts(catalog)

    errs = validate(catalog)
    if errs:
        print("VALIDATION FAILED after consolidation:")
        for e in errs:
            print("  -", e)
        return 1

    print("Merges applied ({}):".format(len(merges)))
    for coll, old, canon in merges:
        print(f"  {coll:8} {old:28} -> {canon}")
    print()
    print("Before:", before)
    print("After: ", after)
    print("Delta: ", {k: after[k] - before[k] for k in before})
    print("Validation OK ({} ids unique, all cross-links resolve)".format(
        sum(after.values())))

    if args.dry_run:
        print("\n--dry-run: catalog.json NOT written")
        return 0

    out = json.dumps(catalog, indent=2, ensure_ascii=True) + "\n"
    args.catalog.write_text(out)
    print(f"\nWrote {args.catalog}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
