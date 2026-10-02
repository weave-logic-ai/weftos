#!/usr/bin/env python3
"""Import the LCSC / JLCPCB component library CSV into the parts pool, filtered to the categories we
care about (sensors, RF, positioning, SBC-adjacent). Merges into the SAME pool file as mouser-pull,
deduped by manufacturer part number (unions buy offers so a part can carry both an LCSC and a Mouser
price). Offline — no API, no rate limit; just point it at the downloaded CSV.

Where to get the CSV:
  - JLCPCB "SMT Assembly Parts" library (jlcpcb.com) — "Download" the parts list as CSV, or
  - the yaqwsx/jlcparts dataset, or any LCSC export with MPN/Manufacturer/Category/Description/Datasheet/Price.

Usage:
  scripts/lcsc-import.py path/to/lcsc.csv --pool crates/weftos-cog-market/catalog/mouser-pool.json
  scripts/lcsc-import.py path/to/lcsc.csv --all        # import everything, don't category-filter
"""
import argparse, csv, datetime, json, os, re, sys

# Keep rows whose LCSC category or description matches our domain. Lowercased substring match.
KEEP = [
    "sensor", "radar", "mmwave", "accelerom", "gyro", "imu", "magnetom", "compass", "hall",
    "gps", "gnss", "positioning", "tof", "time of flight", "lidar", "proximity", "ambient light",
    "thermal", "temperature", "humidity", "barometric", "pressure", "altimeter", "microphone",
    "mems", "ultrasonic", "gas", "co2", "voc", "particulate", "pm2.5", "current sensor",
    "optical", "photodiode", "color sensor", "heart rate", "ppg", "spo2", "biosensor", "ecg",
    "encoder", "flow sensor", "uwb", "transceiver", "gnss", "sbc", "single board",
]
# Flexible header lookup: our field -> list of candidate CSV column names (case-insensitive).
COLS = {
    "mpn": ["mfr.part", "mfr part", "manufacturer part", "manufacturer part number", "mpn", "mfrpart"],
    "manufacturer": ["manufacturer", "mfr", "brand"],
    "description": ["description", "desc"],
    "category": ["first category", "category", "second category", "catalog"],
    "subcategory": ["second category", "subcategory"],
    "datasheet": ["datasheet", "datasheet url", "datasheetlink"],
    "price": ["price", "unit price", "price(usd)"],
    "lcsc": ["lcsc part number", "lcsc part", "lcsc", "lcsc#"],
    "package": ["package", "footprint"],
    "stock": ["stock", "quantity"],
}


def norm(s):
    return re.sub(r"[^a-z0-9]+", "", (s or "").lower())


def build_index(header):
    idx = {}
    nh = [norm(h) for h in header]
    for field, cands in COLS.items():
        for c in cands:
            nc = norm(c)
            if nc in nh:
                idx[field] = nh.index(nc)
                break
    return idx


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("csv")
    ap.add_argument("--pool", default="crates/weftos-cog-market/catalog/mouser-pool.json")
    ap.add_argument("--all", action="store_true", help="import every row (skip the category filter)")
    ap.add_argument("--limit", type=int, default=0, help="max rows to import (0 = no cap)")
    a = ap.parse_args()

    pool = {"generated": "", "parts": {}}
    if os.path.exists(a.pool):
        try: pool = json.load(open(a.pool))
        except Exception: pass
    parts = pool.setdefault("parts", {})
    today = datetime.date.today().isoformat()

    # sniff delimiter
    with open(a.csv, newline="", encoding="utf-8", errors="replace") as f:
        sample = f.read(4096); f.seek(0)
        delim = ";" if sample.count(";") > sample.count(",") else ","
        reader = csv.reader(f, delimiter=delim)
        header = next(reader, None)
        if not header:
            print("empty CSV", file=sys.stderr); return 1
        idx = build_index(header)
        missing = [k for k in ("mpn", "manufacturer", "description") if k not in idx]
        if missing:
            print(f"CSV is missing expected column(s) {missing}; headers seen: {header}", file=sys.stderr)
            return 1
        def get(row, field):
            i = idx.get(field)
            return (row[i].strip() if i is not None and i < len(row) else "")
        kept = scanned = 0
        for row in reader:
            scanned += 1
            cat = (get(row, "category") + " " + get(row, "subcategory")).lower()
            desc = get(row, "description").lower()
            if not a.all and not any(k in cat or k in desc for k in KEEP):
                continue
            mpn = get(row, "mpn")
            if not mpn:
                continue
            price = get(row, "price")
            lcsc = get(row, "lcsc")
            rec = parts.get(mpn)
            buy = {"vendor": "LCSC", "price": price, "url": (f"https://www.lcsc.com/product-detail/{lcsc}.html" if lcsc else ""), "ships_from": "LCSC/China"}
            if rec:  # merge an LCSC offer onto an existing (e.g. Mouser-pulled) part
                rec.setdefault("buy", [])
                if not any(b.get("vendor") == "LCSC" for b in rec["buy"]):
                    rec["buy"].append(buy)
                rec.setdefault("lcsc_part", lcsc)
                if not rec.get("datasheet"):
                    rec["datasheet"] = get(row, "datasheet")
            else:
                parts[mpn] = {
                    "mpn": mpn, "name": mpn, "manufacturer": get(row, "manufacturer"),
                    "summary": get(row, "description")[:300],
                    "category": get(row, "category"), "mouser_category": "",
                    "datasheet": get(row, "datasheet"), "image": "",
                    "lcsc_part": lcsc, "package": get(row, "package"), "stock": get(row, "stock"),
                    "buy": [buy], "attributes": {}, "source": "lcsc", "imported": today, "status": "imported",
                }
                kept += 1
            if a.limit and kept >= a.limit:
                break
    pool["generated"] = today
    os.makedirs(os.path.dirname(a.pool), exist_ok=True)
    json.dump(pool, open(a.pool, "w"), indent=1)
    print(f"scanned {scanned} rows, imported {kept} new parts; pool now {len(parts)} total -> {a.pool}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
