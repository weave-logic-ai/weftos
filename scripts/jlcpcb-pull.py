#!/usr/bin/env python3
"""Pull parts from JLCPCB's public component API (the LCSC catalog) into the parts pool, by category
keyword. No API key, no CSV — this is the real complete-catalog engine. Paginates each keyword,
dedupes by manufacturer part number, and merges into the SAME pool as mouser-pull / lcsc-import
(unions buy offers). Gentle by default; bounded by --max-pages and a global --max-total guard.

Usage:
  scripts/jlcpcb-pull.py --categories scripts/sensor-categories.json \
      --pool crates/weftos-cog-market/catalog/mouser-pool.json --max-pages 2
  scripts/jlcpcb-pull.py --keyword "radar sensor" --label radar/presence
"""
import argparse, datetime, json, os, sys, time, urllib.request, urllib.error

API = "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList"
HEADERS = {"Content-Type": "application/json", "User-Agent": "Mozilla/5.0 (weftos sensor-explorer pool builder)"}


def page(keyword, current, size=100):
    body = json.dumps({"currentPage": current, "pageSize": size, "keyword": keyword,
                       "firstSortName": "", "secondSortName": "", "searchSource": "search"}).encode()
    req = urllib.request.Request(API, data=body, headers=HEADERS, method="POST")
    with urllib.request.urlopen(req, timeout=25) as r:
        return json.loads(r.read().decode())


def price_str(prices):
    if not prices:
        return ""
    p = prices[0].get("productPrice")
    return f"${p} USD" if p is not None else ""


def record(c, keyword, label, today):
    code = c.get("componentCode") or ""
    mpn = (c.get("componentModelEn") or "").strip() or code
    return {
        "mpn": mpn, "name": mpn, "manufacturer": (c.get("componentBrandEn") or "").strip(),
        "summary": (c.get("describe") or "")[:300],
        "category": label or c.get("componentTypeEn") or c.get("secondSortName") or "",
        "jlc_category": f"{c.get('firstSortName','')} / {c.get('secondSortName','')}".strip(" /"),
        "datasheet": c.get("dataManualUrl") or c.get("dataManualOfficialLink") or "",
        "image": c.get("componentImageUrl") or "",
        "lcsc_part": code, "package": c.get("componentSpecificationEn") or "",
        "stock": c.get("stockCount"),
        "buy": [{"vendor": "LCSC/JLCPCB", "price": price_str(c.get("componentPrices")),
                 "url": f"https://www.lcsc.com/product-detail/{code}.html" if code else "", "ships_from": "LCSC/China"}],
        "attributes": {}, "source": f"jlcpcb:{keyword}", "imported": today, "status": "imported",
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--categories")
    ap.add_argument("--keyword")
    ap.add_argument("--label", default="")
    ap.add_argument("--pool", default="crates/weftos-cog-market/catalog/mouser-pool.json")
    ap.add_argument("--max-pages", type=int, default=2)
    ap.add_argument("--max-total", type=int, default=8000, help="stop once the pool reaches this size")
    ap.add_argument("--delay", type=float, default=1.0)
    a = ap.parse_args()

    cats = json.load(open(a.categories)) if a.categories else [{"keyword": a.keyword, "label": a.label}]
    if not cats or not cats[0].get("keyword"):
        print("give --categories or --keyword", file=sys.stderr); return 2

    pool = {"generated": "", "parts": {}}
    if os.path.exists(a.pool):
        try: pool = json.load(open(a.pool))
        except Exception: pass
    parts = pool.setdefault("parts", {})
    today = datetime.date.today().isoformat()

    for cat in cats:
        kw, label = cat["keyword"], cat.get("label", "")
        got = 0
        for cur in range(1, a.max_pages + 1):
            if len(parts) >= a.max_total:
                print(f"hit --max-total {a.max_total}; stopping"); break
            try:
                resp = page(kw, cur)
            except urllib.error.HTTPError as e:
                print(f"  {kw}: HTTP {e.code}"); break
            except Exception as e:
                print(f"  {kw}: {e}"); break
            data = resp.get("data") or {}
            info = data.get("componentPageInfo") or data
            lst = info.get("list") or []
            if not lst:
                break
            for c in lst:
                rec = record(c, kw, label, today)
                mpn = rec["mpn"]
                if not mpn:
                    continue
                exist = parts.get(mpn)
                if exist:
                    if not any(b.get("vendor") == "LCSC/JLCPCB" for b in exist.get("buy", [])):
                        exist.setdefault("buy", []).append(rec["buy"][0])
                    exist.setdefault("lcsc_part", rec["lcsc_part"])
                    if not exist.get("datasheet"):
                        exist["datasheet"] = rec["datasheet"]
                else:
                    parts[mpn] = rec; got += 1
            time.sleep(a.delay)
            if len(lst) < 100:
                break
        print(f"  {kw} [{label}]: +{got} new (pool {len(parts)})")
        if len(parts) >= a.max_total:
            break
    pool["generated"] = today
    os.makedirs(os.path.dirname(a.pool), exist_ok=True)
    json.dump(pool, open(a.pool, "w"), indent=1)
    print(f"\npool: {len(parts)} parts in {a.pool}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
