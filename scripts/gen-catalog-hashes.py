#!/usr/bin/env python3
"""Populate a stable unique `hash` on every catalog item.

hash = "wh_" + sha256("<type>:<id>").hexdigest()[:16], type in {chip, module, project}.
Matches the Sensor Explorer's item hash so a part is addressable the same everywhere.
Idempotent; re-run after adding catalog items.

Usage: scripts/gen-catalog-hashes.py [path/to/catalog.json]
"""
import json, hashlib, os, sys
PATH = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
    "~/weftos/crates/weftos-cog-market/catalog/catalog.json")


def item_hash(kind, ident):
    return "wh_" + hashlib.sha256(f"{kind}:{ident}".encode()).hexdigest()[:16]


def main():
    cat = json.load(open(PATH))
    n = 0
    for kind, key in (("chip", "chips"), ("module", "modules"), ("project", "projects")):
        for it in cat.get(key, []):
            it["hash"] = item_hash(kind, it["id"])
            n += 1
    json.dump(cat, open(PATH, "w"), indent=2)
    open(PATH, "a").write("\n")
    print(f"hashed {n} items in {PATH}")


if __name__ == "__main__":
    main()
