#!/usr/bin/env python3
"""Bundle each sensor cog's ADR-104 guide/ folder into the catalog crate.

The console shows a sensor's full hook-up guide before anything is installed, so the guides ship
with the catalog (embedded by crates/weftos-cog-market/src/guides.rs) instead of being fetched
from a running cog. The output is the same JSON a cog serves at `GET /guide`:
{"toml": "...", "pages": {id: markdown}, "images": {name: base64}}.

Usage: scripts/bundle-cog-guides.py <cogs src/cogs dir> [out dir]
       (default out: crates/weftos-cog-market/catalog/guides)
Then update the table in guides.rs if a cog was added; its test fails when they differ.
"""
import base64, json, pathlib, re, sys, tomllib

def bundle(gdir: pathlib.Path) -> dict:
    toml_text = (gdir / "guide.toml").read_text()
    doc = tomllib.loads(toml_text)
    pages, images = {}, {}
    for pid in doc["pages"]:
        md = (gdir / f"{pid}.md").read_text()
        pages[pid] = md
        for name in re.findall(r"!\[[^\]]*\]\(([^)\s]+)\)", md):
            f = gdir / name
            if f.is_file() and name not in images:
                images[name] = base64.b64encode(f.read_bytes()).decode()
    return {"toml": toml_text, "pages": pages, "images": images}

def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    src = pathlib.Path(sys.argv[1])
    out = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else pathlib.Path(__file__).resolve().parent.parent / "crates/weftos-cog-market/catalog/guides"
    out.mkdir(parents=True, exist_ok=True)
    n = 0
    for d in sorted(src.iterdir()):
        g = d / "guide"
        if (g / "guide.toml").is_file():
            (out / f"{d.name}.json").write_text(json.dumps(bundle(g), indent=1, ensure_ascii=True, sort_keys=True) + "\n")
            n += 1
            print("bundled", d.name)
    print(f"{n} guide(s) -> {out}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
