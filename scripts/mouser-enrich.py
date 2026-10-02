#!/usr/bin/env python3
"""Enrich the hardware catalog from the Mouser Product Search API.

For every module/chip with a `mouser_query`, queries Mouser, and fills `buy` (a Mouser offer with
price + link) and `datasheet` (Mouser's manufacturer datasheet URL) from the best match. Parts Mouser
doesn't stock (generic AliExpress modules) are left on their existing vendor datasheet links.

Key: MOUSER_PRODUCT_API_KEY (fallback MOUSER_API_KEY), from the environment or ~/weftos/.env /
~/.config/cognitum/env. Never printed.

Usage: scripts/mouser-enrich.py [path/to/catalog.json]   (default: the market crate's catalog)
"""
import json, os, sys, time, urllib.request, urllib.error

API = "https://api.mouser.com/api/v1/search/keyword"
DEFAULT = os.path.expanduser("~/weftos/crates/weftos-cog-market/catalog/catalog.json")


def load_key():
    # The Product/Search API key (MOUSER_PRODUCT_API_KEY) is distinct from the Order API key
    # (MOUSER_API_KEY) and is the one the search endpoint accepts — always prefer it.
    found = {}
    for k in ("MOUSER_PRODUCT_API_KEY", "MOUSER_API_KEY"):
        if os.environ.get(k):
            found[k] = os.environ[k]
    for f in ("~/weftos/.env", "~/.config/cognitum/env"):
        p = os.path.expanduser(f)
        if not os.path.exists(p):
            continue
        for line in open(p):
            line = line.strip()
            for k in ("MOUSER_PRODUCT_API_KEY", "MOUSER_API_KEY"):
                if line.startswith(k + "=") and k not in found:
                    found[k] = line.split("=", 1)[1].strip().strip('"').strip("'")
    return found.get("MOUSER_PRODUCT_API_KEY") or found.get("MOUSER_API_KEY")


def search(key, query):
    body = json.dumps({"SearchByKeywordRequest": {
        "keyword": query, "records": 3, "startingRecord": 0,
        "searchOptions": "", "searchWithYourSignUpLanguage": "false"}}).encode()
    req = urllib.request.Request(f"{API}?apiKey={key}", data=body,
                                 headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.loads(r.read().decode())


def best_part(resp, query):
    sr = (resp or {}).get("SearchResults") or {}
    parts = sr.get("Parts") or []
    if not parts:
        return None
    # Prefer a part whose description shares a word with the query; else the first.
    qwords = {w.lower() for w in query.replace("-", " ").split() if len(w) > 2}
    for p in parts:
        desc = (p.get("Description") or "").lower() + " " + (p.get("ManufacturerPartNumber") or "").lower()
        if qwords & set(desc.replace("-", " ").split()):
            return p
    return parts[0]


def first_price(part):
    pbs = part.get("PriceBreaks") or []
    if not pbs:
        return ""
    pb = pbs[0]
    return f"{pb.get('Price', '')} {pb.get('Currency', '')}".strip()


def enrich(item, key, kind):
    q = item.get("mouser_query") or ""
    if not q.strip():
        return "no query"
    try:
        resp = search(key, q)
    except urllib.error.HTTPError as e:
        return f"HTTP {e.code}"
    except Exception as e:
        return f"error: {e}"
    errs = (resp or {}).get("Errors") or []
    if errs:
        return "api error: " + "; ".join(str(e.get("Message", e)) for e in errs)[:120]
    part = best_part(resp, q)
    if not part:
        return "no match (kept vendor link)"
    changed = []
    ds = part.get("DataSheetUrl") or ""
    if ds:
        item["datasheet"] = ds
        changed.append("datasheet")
    url = part.get("ProductDetailUrl") or ""
    price = first_price(part)
    if url:
        offer = {"vendor": "Mouser", "price": price, "url": url, "ships_from": "Mouser"}
        item["buy"] = [b for b in item.get("buy", []) if b.get("vendor") != "Mouser"] + [offer]
        changed.append("buy")
    mpn = part.get("ManufacturerPartNumber") or ""
    return f"{part.get('Manufacturer','?')} {mpn} ({price or 'n/a'}) -> {'+'.join(changed) or 'nothing'}"


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT
    key = load_key()
    if not key:
        print("no MOUSER_PRODUCT_API_KEY / MOUSER_API_KEY found", file=sys.stderr)
        return 2
    cat = json.load(open(path))
    for kind in ("chips", "modules"):
        print(f"== {kind} ==")
        for item in cat.get(kind, []):
            res = enrich(item, key, kind)
            print(f"  {item['id']:22} {res}")
            time.sleep(2)  # be gentle with the rate limit
    json.dump(cat, open(path, "w"), indent=2)
    open(path, "a").write("\n")
    print(f"\nwrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
