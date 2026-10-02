#!/usr/bin/env python3
"""Bulk-pull parts from the Mouser Product Search API into a large pool, by category keyword.

Paginates each keyword (50/call) up to a per-keyword page cap, dedupes by manufacturer part number,
and writes/merges a pool file (one JSON object: {"generated","parts":{mpn: record}}). This pool is
SEPARATE from the curated catalog.json — it's the "very large list with initial value" you promote
curated parts out of. Respects a global daily call budget and a gentle delay (free tier: ~30/min,
~1000/day). Key: MOUSER_PRODUCT_API_KEY (reuses mouser-enrich.load_key). Never printed.

Usage:
  scripts/mouser-pull.py --categories scripts/mouser-categories.json --pool catalog/mouser-pool.json \
      --max-calls 200 --max-pages 4
  scripts/mouser-pull.py --keyword "mmWave radar sensor" --label radar --pool catalog/mouser-pool.json
"""
import argparse, importlib.util, json, os, re, sys, time, datetime, urllib.request, urllib.error

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("me", os.path.join(HERE, "mouser-enrich.py"))
me = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(me)
API = "https://api.mouser.com/api/v1/search/keyword"


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", (s or "").lower()).strip("-")[:64] or "part"


def search_page(key, keyword, start, records=50):
    body = json.dumps({"SearchByKeywordRequest": {
        "keyword": keyword, "records": records, "startingRecord": start,
        "searchOptions": "", "searchWithYourSignUpLanguage": "false"}}).encode()
    req = urllib.request.Request(f"{API}?apiKey={key}", data=body, headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=25) as r:
        return json.loads(r.read().decode())


def record(part, keyword, label, today):
    pbs = part.get("PriceBreaks") or []
    price = f"{pbs[0].get('Price','')} {pbs[0].get('Currency','')}".strip() if pbs else ""
    return {
        "mpn": part.get("ManufacturerPartNumber") or "",
        "name": part.get("ManufacturerPartNumber") or part.get("MouserPartNumber") or "",
        "manufacturer": part.get("Manufacturer") or "",
        "summary": (part.get("Description") or "")[:300],
        "category": label or part.get("Category") or "",
        "mouser_category": part.get("Category") or "",
        "datasheet": part.get("DataSheetUrl") or "",
        "image": part.get("ImagePath") or "",
        "availability": part.get("Availability") or "",
        "buy": [{"vendor": "Mouser", "price": price, "url": part.get("ProductDetailUrl") or "", "ships_from": "Mouser"}],
        "attributes": {a.get("AttributeName"): a.get("AttributeValue") for a in (part.get("ProductAttributes") or []) if a.get("AttributeName")},
        "source": f"mouser:{keyword}",
        "imported": today,
        "status": "imported",
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--categories", help="JSON: [{keyword,label}]")
    ap.add_argument("--keyword")
    ap.add_argument("--label", default="")
    ap.add_argument("--pool", default="crates/weftos-cog-market/catalog/mouser-pool.json")
    ap.add_argument("--max-calls", type=int, default=200, help="global cap on API calls this run (daily budget guard)")
    ap.add_argument("--max-pages", type=int, default=4, help="pages (x50) per keyword")
    ap.add_argument("--delay", type=float, default=2.1)
    a = ap.parse_args()

    key = me.load_key()
    if not key:
        print("no MOUSER_PRODUCT_API_KEY", file=sys.stderr); return 2
    cats = json.load(open(a.categories)) if a.categories else [{"keyword": a.keyword, "label": a.label}]
    if not cats or not cats[0].get("keyword"):
        print("give --categories or --keyword", file=sys.stderr); return 2

    pool = {"generated": "", "parts": {}}
    if os.path.exists(a.pool):
        try: pool = json.load(open(a.pool))
        except Exception: pass
    parts = pool.setdefault("parts", {})
    today = datetime.date.today().isoformat()
    calls = 0
    for cat in cats:
        kw, label = cat["keyword"], cat.get("label", "")
        got = 0
        for page in range(a.max_pages):
            if calls >= a.max_calls:
                print(f"hit --max-calls {a.max_calls}; stopping"); break
            try:
                resp = search_page(key, kw, page * 50)
            except urllib.error.HTTPError as e:
                print(f"  {kw}: HTTP {e.code}"); break
            except Exception as e:
                print(f"  {kw}: {e}"); break
            calls += 1
            errs = resp.get("Errors") or []
            if errs:
                print(f"  {kw}: {[x.get('Message') for x in errs]}"); break
            found = (resp.get("SearchResults") or {}).get("Parts") or []
            if not found:
                break
            for pt in found:
                mpn = pt.get("ManufacturerPartNumber")
                if mpn and mpn not in parts:
                    parts[mpn] = record(pt, kw, label, today)
                    got += 1
            time.sleep(a.delay)
            if len(found) < 50:
                break
        print(f"  {kw} [{label}]: +{got} new (calls used {calls}/{a.max_calls})")
        if calls >= a.max_calls:
            break
    pool["generated"] = today
    os.makedirs(os.path.dirname(a.pool), exist_ok=True)
    json.dump(pool, open(a.pool, "w"), indent=1)
    print(f"\npool: {len(parts)} parts total in {a.pool} ({calls} API calls this run)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
