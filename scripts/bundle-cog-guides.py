#!/usr/bin/env python3
"""Bundle each sensor cog's ADR-104 guide/ folder into the catalog crate.

The console shows a sensor's full hook-up guide before anything is installed, so the guides ship
with the catalog (embedded by crates/weftos-cog-market/src/guides.rs) instead of being fetched
from a running cog. The output is the same JSON a cog serves at `GET /guide`:
{"toml": "...", "pages": {id: markdown}, "images": {name: base64}}.

Images are NOT bundled unless --with-images is given: the Raspberry Pi header photo that three
guides carry has no recorded source or licence, so the bundles leave it out and drop the lines that
embed it (each guide still draws the header pin diagram from its own data and links pinout.xyz).
Add an image to a bundle only once its provenance and licence are recorded.

Usage: scripts/bundle-cog-guides.py [--with-images] <cogs src/cogs dir> [out dir]
       (default out: crates/weftos-cog-market/catalog/guides)
Then update the table in guides.rs if a cog was added; its test fails when they differ.
"""
import base64, json, pathlib, re, sys, tomllib

IMG_ONLY_LINE = re.compile(r"^\s*!\[[^\]]*\]\([^)\s]+\)\s*$")
PHOTO_SENTENCE = re.compile(r"\s*The photo `[^`]+` next to this guide[^.]*\.")


def strip_images(md: str) -> str:
    """Drop lines that only embed an image, and sentences that point at an image next to the guide."""
    lines = [l for l in md.split("\n") if not IMG_ONLY_LINE.match(l)]
    return PHOTO_SENTENCE.sub("", "\n".join(lines))


def bundle(gdir: pathlib.Path, with_images: bool = False) -> dict:
    toml_text = (gdir / "guide.toml").read_text()
    doc = tomllib.loads(toml_text)
    pages, images = {}, {}
    for pid in doc["pages"]:
        md = (gdir / f"{pid}.md").read_text()
        if with_images:
            for name in re.findall(r"!\[[^\]]*\]\(([^)\s]+)\)", md):
                f = gdir / name
                if f.is_file() and name not in images:
                    images[name] = base64.b64encode(f.read_bytes()).decode()
        else:
            md = strip_images(md)
        pages[pid] = md
    return {"toml": toml_text, "pages": pages, "images": images}

def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--with-images"]
    with_images = "--with-images" in sys.argv[1:]
    if not args:
        print(__doc__)
        return 2
    src = pathlib.Path(args[0])
    out = pathlib.Path(args[1]) if len(args) > 1 else pathlib.Path(__file__).resolve().parent.parent / "crates/weftos-cog-market/catalog/guides"
    out.mkdir(parents=True, exist_ok=True)
    n = 0
    for d in sorted(src.iterdir()):
        g = d / "guide"
        if (g / "guide.toml").is_file():
            (out / f"{d.name}.json").write_text(json.dumps(bundle(g, with_images), indent=1, ensure_ascii=True, sort_keys=True) + "\n")
            n += 1
            print("bundled", d.name)
    print(f"{n} guide(s) -> {out}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
