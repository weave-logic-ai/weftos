# Sonar **range** — detection vs localization (1.8 kHz copper stack)

**Date:** 2026-09-26  
**This is the range answer.** Bearing/beamwidth lived in `tow-detect-classify-resolution.md`; that note wrongly stopped at “tow cannot range.” Here: how far we **hear**, how far we **place**, active vs passive.

Grounding: Urick `SE = SL − TL − (NL − DI) − DT` (passive). Phase-economics §5.1 / P79 Mode F: vocal-fish school **SL ≈ 120 dB re 1 µPa @ 1 m**, NL_det ≈ 50 dB in ~100 Hz, DT = 10 dB, α ≈ **0.06 dB/km** at 1.8 kHz (spreading wins until tens of km). SE = 0 at **~1 km** for a **single** omni (DI = 0), spherical 20 log R = 60 dB.

Wenz-limited RX (oil+JFET). Numbers are **SE=0 detection**, not a guaranteed track. Lake 15–30 m is deep inside every budget below.

---

## 1. Three different “ranges”

| Word | Meaning | What sets it |
|------|---------|----------------|
| **Detection** | SE ≥ 0: energy / chirp replica is there | SL, spreading, NL, DI, processing |
| **Bearing** | which way | aperture L (hose 1.2 m → ~35°; 30 m triangle → ~1.6° if coherent) |
| **Localization (x,y)** | a place in the water | need **baseline comparable to range**. Far away, a small net only gives bearing |

A 1.2 m hose **never** gives source range (far-field from ~2 m). A **30 m taut triangle** localizes well inside a few× baseline; beyond that it is a compass.

---

## 2. Passive detection (how far we hear)

Scale the 1 km / DI=0 point: extra DI adds `10^(DI/20)` to range (spherical, α≈0).

| Aperture | DI (coherent) | Detect **SL=120 dB** school / chorus | Detect **SL≈160 dB** humpback unit | Detect **SL≈150 dB** small boat |
|----------|---------------|--------------------------------------|-------------------------------------|----------------------------------|
| 1 phone | 0 | **~1 km** | **~10 km** spherical; **tens of km** in a shallow waveguide | **~3 km** |
| **v1 hose 4-el** | ~6 dB | **~2 km** | **~20 km** sph. / still waveguide-limited | **~6 km** |
| Twin 8-el | ~9 dB | **~2.8 km** | same order | **~8 km** |
| 10-buoy mesh, 10 m (existing note) | ~10 dB inc. | **~3.2 km** | — | — |
| 20-el **100 m** tow (existing note) | ~23 dB | **~14 km** | further | further |

α = 0.06 dB/km does not matter until **>20 km**. Shipping noise, bottom bounce, and the 300 Hz BPF matter more than absorption.

**Mahi school:** not a 120 dB *voice*. That 1 km figure is **croaker/drum/chorus** (Ramcharitar). A mahi school is a **target** for **active** echo (or a quiet blob). Do not quote 2 km passive mahi.

**Odontocete clicks:** out of band on the mesh BPF — detection range **0** on 1.8 kHz until a wide/HF RX.

---

## 3. Localization range (how far we *place*)

| Geometry | Useful (x,y) | Beyond that |
|----------|----------------|-------------|
| Hose 1.2 m | **none** (bearing ~35° only) | — |
| **3-buoy taut 15–30 m** | **~50–150 m** (order **few × baseline**), 0.1–1 m in-frame when SNR is high and path is direct | **bearing only** (~1–2° if you coherently treat the triangle as one aperture) |
| 100 m class tow (not v1) | hundreds of m of (x,y) | bearing |

Wired PPS (~µs) is **not** the limit. Geometry is. A source at 1 km vs a 30 m net is a 3% baseline/range ratio — you get direction, not a GPS of the whale.

Fusion over **time** (tow sweep, many looks, RuVector track): the **detection** range stays as §2; the **track** on uvRTH is a corridor whose width is the beam at that range (at 1 km, 35° ≈ **600 m** across from the hose; ~30 m across from a coherent 30 m triangle).

---

## 4. Active (our chirp, two-way)

JANUS-class **185–195 dB** in `RANGING.md` is a **real projector**, not the 35 mm disc. Treat the cheap TX as **~130–150 dB** until measured.

Rough spherical two-way, TS_school ~+10 dB, DI=6, MF ~15 dB, NL~80, DT=10:

| TX SL | Echo detect (school-ish TS) |
|-------|-----------------------------|
| 130 dB (pessimistic disc) | **~50–80 m** |
| 150 dB (optimistic disc) | **~150–250 m** |
| 185 dB (JANUS / real projector) | **km-class** two-way |

v1 lake **15–30 m** closes even on a weak disc. Open-ocean mahi **search** wants the 185 dB class or a 38 kHz sounder, not the mesh disc.

---

## 5. Quote sheet

| Ask | v1 (4-el hose + 30 m triangle + PPS) |
|-----|--------------------------------------|
| Hear a **vocal school / chorus** (120 dB) | **~2 km** detect |
| Hear a **song whale** (loud unit) | **~10–20 km** detect (more in waveguide); **place** only as a **bearing corridor** |
| Hear a **mahi school** passively | **don’t count on it** |
| **Ping** a school with the 35 mm disc | **~50–200 m** echo |
| **Place** a loud source | **~50–150 m** (x,y) on the triangle; hose = direction only |
| Lake test 15–30 m | **not a range test** — everything is in the near budget |

Upgrade range with **aperture** (longer tow → 14 km class at 100 m / 20 el) or **SL** (real projector), not a quieter preamp.
