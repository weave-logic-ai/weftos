# Towed twin-CAT5 braid — resolution, detect, classify

**Date:** 2026-09-26  
**Setup:** one tow, 4 omni JFET phones at 0.40 m along-track (aperture **L = 1.2 m**), second CAT5 trapped parallel in a flat 4-plait (`twin-cat5-nylon-braid.md`). Wired PPS. Optional second line of 4 phones. 1.8 kHz mesh path (oil sidecar, ~300 Hz BPF). No Wi‑Fi.

These are **budget numbers**, not a sea trial.

---

## 1. Resolution (what the aperture can resolve)

Passive line array: **bearing**, not range. Range needs the 3-buoy triangle or a turn.

| Quantity | 1.8 kHz (λ ≈ 0.82 m) | 12 kHz (λ ≈ 0.12 m) | 35 kHz |
|----------|----------------------|---------------------|--------|
| Along-track HPBW (broadside) | **~35°** | **~5°** | grating lobes if d still 0.4 m — **don’t** |
| Time-delay bearing (high SNR) | a few degrees | ~1° | n/a at this spacing |
| Twin `d⊥` 1–1.5 cm (tight plait) | L/R **weak** (~5° of phase) | L/R **usable** | L/R **good** |
| Twin `d⊥` 8–12 cm (spacers) | L/R **usable** | L/R strong | over-spaced |
| Range from **tow alone** | none (far-field starts ~ L²/λ ≈ 2 m) | same | — |
| Source (x,y) with **3 taut buoys** + PPS | **~0.1–1 m in-frame** (direct path, high SNR) | better TDoA | — |
| Earth fix | GPS 2–5 m SPS | same | — |

Left/right is a **binary / cardioid**, not a second angle, until `d⊥` is a decent fraction of λ.

Do not quote 35 kHz imaging on this hose: 0.4 m is ~10 λ, grating lobes everywhere. Imaging stays on P79 / a different spacing.

---

## 2. Detection

Phones are **Wenz-limited**, not preamp-limited (oil + JFET: NE-SPL ~50 dB re 1 µPa in 300 Hz; SS1 ambient ~80–90 dB). Extra silicon does not buy quieter. Array gain and the **chirp matched filter** do.

| Trick | Gain | Notes |
|-------|------|--------|
| 4-phone coherent DS | **+6 dB** | needs hose geometry + PPS (we have both) |
| Twin 8-phone coherent | **+9 dB** | if both lines populated and `d⊥` known |
| 100 ms / 300 Hz chirp MF | **~15 dB** (10 log TB, TB≈30) | active or cooperative ping |
| Incoherent 4-phone | +3–4 dB | if you refuse to beamform |

**What you will detect in a lake (honest):**

- Own / other-buoy **1.8 kHz chirps** at 15–30 m: easy (MF + AG).
- Outboard / small boat at hundreds of metres: **maybe**, broadband energy, not a track-quality SNR.
- Quiet swimmer / fish: **no** on the 1.8 kHz mesh channel.
- JANUS-class 9–14 kHz: only if you **open or add** a wider RX (the 300 Hz mesh BPF will reject it).

Detection is “is there energy / a chirp, which beam, L or R.” It is not a 80% DeepShip classifier.

---

## 3. Classification

The mesh BPF is a **ranging hole**, ~300 Hz around 1.8 kHz. That is enough to say **chirp vs click vs rumble-in-band**. It is **not** enough for species ID, hull type, or DEMON shaft rate unless you add a second, **wide** path (e.g. 500 Hz–8 kHz into the S3 / USB audio).

| Task | On 1.8 kHz mesh only | If you add a wideband RX (16 kS/s+) |
|------|----------------------|-------------------------------------|
| Chirp / ping vs noise | **Yes** (matched filter) | yes |
| Left vs right | **Weak–yes** (twin `d⊥`) | yes |
| Duration / bandwidth class | coarse | better |
| Boat vs splash vs speech-in-water | poor | **plausible** (tiny-ML / energy + flux) |
| DeepShip / Watkins 32-way | **No** | still no with 4 cheap discs; those numbers need corpora + bandwidth |
| K-STEMIT GCN | uses `D(t)` from copper — **geometry yes**, class head still needs spectrograms | same |

K-STEMIT mapping already said: detection ≈ temporal, bearing ≈ spatial, species ≈ both. v1 tow gives you **temporal + a fat bearing**. Classification head waits on a wide ADC.

---

## 4. One picture

```
detect:     chirps at lake range  —  yes
            quiet biologics       —  no (mesh band)
bearing:    ~35° beam @ 1.8 kHz   —  4-el hose
            L/R                   —  twin CAT5 (better if 8–12 cm)
range:      tow alone             —  no
            + 3-buoy taut PPS     —  0.1–1 m in-frame
classify:   ping vs other         —  yes
            ship/species          —  not until a wide RX
```

v1 is a **coherent 1.8 kHz line + L/R bit + timed triangle**. That is already a real sonar. It is not PAM-grade taxonomy and not P79 imaging.
