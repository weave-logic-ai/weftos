# Infineon sensor ingest — 2026-10-05

Ticket `c980b664-fd50-4721-837f-d5b370aa020f`. Staging file only: `docs/hardware/infineon-sensor-ingest.json`.

`catalog.json` was not edited. The Hi-Link ingest owns that file. Live D1 was not touched. No cogs, no wrangler, no commit.

Pulled from https://www.infineon.com/products (sensor tree plus the products nav). Spec strings are copied from the part or eval page. Where that page does not print a number, it is not in `spec`.

## What was missing

Catalog when this lane started: 245 modules, 308 chips. A later read of the same file, while the Hi-Link ingest was writing it, showed 777 modules and 308 chips. These ids were checked against that dirty file and do not collide. Radar, IMU, microphones, ToF, and temperature/humidity are already thick. Infineon was the gap in current sensing (10 current chips, all shunt amplifiers, none Infineon), photoacoustic CO2, barometric pressure (Bosch/ST only), Infineon MEMS mics, 3D Hall, TMR angle, and REAL3 ToF.

## Added

2 modules, 10 chips.

| id | kind | why this one |
|---|---|---|
| `tli4971-a120t5-e0001` | chip | Current family lead. `/part/TLI4971` 404s; the orderable page is the ±120 A OPN, active and preferred. Other ranges and TLE4972/73/78 were not copied. |
| `infineon-pasco2v01` | module | PAS CO2 the CO2 page names for WELL / Title 24. SMD sensor, not bare silicon. Page status is **discontinued**. PASCO2V15 (5 V) is discontinued too, so it is not a second entry. |
| `dps368` | chip | IoT pressure page features this part, and it has a part page (active and preferred). `/part/DPS310` and `/part/DPS422` redirect to the family page. |
| `im73d122` | chip | Current preferred digital PDM mic (73 dB SNR). IM69D120 and IM69D130 are not for new design. The other 24 mics were left. |
| `tli493d-a2b6` | chip | Industrial 3D Hall the family page specifies (I²C, 7 nA power-down). TLE493D is the automotive name of the same family; one part, not every OPN. |
| `tle5501-e0001` | chip | TMR angle lead (QM). E0002 is the ASIL twin and was not added. |
| `irs2877a` | chip | Packaged REAL3 VGA ToF imager (9x9 mm BGA). IRS2976C is the newer consumer die and is bare die only. |
| `bgt60ltr11saip` | chip | Cost-down 1Tx/1Rx Doppler next to BGT60LTR11AIP, which is already in the catalog. |
| `bgt60ltr11baip` | chip | Named on the 60 GHz IoT page as the Japanese 1Tx/1Rx variant. `/part/BGT60LTR11BAIP` returned 404 (also `/ja/`), so this row has no OPN and no numeric specs. |
| `bgt60cutr13aip` | chip | 60 GHz CMOS FMCW, 1Tx/3Rx, on-chip hardware accelerator. Not in the catalog. |
| `bgt24ltr22` | chip | 24 GHz 2Tx/2Rx lead. BGT24LTR11 is already `infineon-bgt-24ltr11n16-e6327`. |
| `infineon-kit-csk-bgt60cutr13` | board | One radar eval kit, not DEMO-BGT60TR13C. Active and preferred connected-sensor kit for the CMOS radar above. |

## Families scanned and left out

Skipped because the catalog already has several good parts of that kind: temperature; capacitive sensing (`infineon-cypress-cy8cmbr3110-sx2i`, FDC2214, FDC1004); AIROC UWB ranging (the TSL100 page describes FiRa/Aliro ranging, and the catalog already has DW1000, DW3000, DWM1001, DWM3000, DW3110, MDEK1001). Radar parts already stocked were not duplicated: BGT60TR13C, BGT60LTR11AIP, BGT60UTR11AIP, BGT24LTR11, DEMO-BGT60TR13C.

Also looked at and not added: the rest of each family above (one current part, not every OPN); DEMO-DISTANCE2GOL and DEMO-SENSE2GOL-PULSE (the other 24 GHz kits named on that page); automotive 24/60/77 GHz radar, magnetic speed, TPMS, side-crash pressure, and battery monitors (TLE9012/TLE9018); digital X-ray and CT; power MOSFETs, AURIX, PSOC-as-MCU, NOR flash, USB-PD, automotive Ethernet, gate drivers, PMICs. The sensor nav lists inductive position sensors; no part page was opened there.

Hydrogen and refrigerant gas pages describe the sensing principle, but the product-table JSON came back empty (HTTP 200, 0 bytes) and the HTML has no `/part/` link, so no part was invented.

## Page facts that disagree with themselves

- BGT60LTR11SAIP parameters say max range 14 m and min 0.5 m. The Benefits tab also says “Up to 6 m detection range for humans” and “Up to 14 m”. Both strings are in `spec`.
- BGT60CUTR13AIP features say “less than 200 µW” and “More than 20 m”. Parameters say max range 20 m, deep sleep 0.1 mW, and low-power sensing mode less than 1 mW. All three are kept.
- TLE5501 E0001 parameters label supply voltage range “-0.5 V to 6.5 V”. Features print supply current ~2 mA and do not print a separate operating voltage. The -0.5 V figure was not rewritten into an operating range.
- The 24 GHz family page says distances up to 100 m for the transceivers as a group. BGT24LTR22’s own table says 20 m. The part-page number is the one stored.
- TLI493D-A2B6’s ±160 mT figure is the page description meta, not the features list. Features do print 12-bit and 7 nA.

## Counts

- Modules in the ingest: 2. Chips: 10.
- Already in `catalog.json` and not repeated: 7 ids, listed in `already_present`.
- `catalog.json` and live D1: unchanged.
