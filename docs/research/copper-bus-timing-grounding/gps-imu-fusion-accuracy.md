# GPS + orientation on every node — fusion / beamform / coherence budget

**Date:** 2026-09-25  
**Status:** Planning estimate (not a sea trial)  
**Stack:** copper-only CAN + RS-422 PPS, radio off (`sonobuoy-min-test-and-copper-tow.md`, `copper-bus-timing-grounding/`)  
**GPS numbers:** `RANGING.md` §1.1

Rule of thumb: **coherent beamforming wants element positions to ~λ/10 and relative time to ~λ/(10c).** Wired PPS already buys the time. GPS/IMU only help fusion if they buy the *geometry* at that scale.

---

## 1. Wavelengths we actually use

c ≈ 1480 m/s.

| Band | f | λ | λ/10 (coherent geometry) | Time for 36° phase (λ/10) |
|------|---|---|--------------------------|---------------------------|
| Mesh chirp | **1.8 kHz** | 0.82 m | **8 cm** | **56 µs** |
| JANUS-ish | 12 kHz | 0.12 m | 1.2 cm | 8 µs |
| 35 kHz mesh | 35 kHz | 4.2 cm | **4 mm** | 2.9 µs |
| Imaging | 200 kHz | 7.4 mm | 0.7 mm | 0.5 µs |

Wired PPS + MCPWM (GPS PPS ~20 ns, capture ~12.5 ns, 30 m CAT5 ~159 ns **calibrated out**) is **≪ 1 µs residual** if you subtract `delay_ns`. Time is **not** the limiter at 1.8 kHz. At 200 kHz it starts to matter unless delay cal is honest.

---

## 2. What each sensor is actually good for

| Sensor | Typical | Relative geometry | Absolute / heading |
|--------|---------|-------------------|--------------------|
| L1 SPS GPS (`RANGING.md`) | **2–5 m RMS** horiz, 5–10 m vert | **Useless for coherent BF** (many λ at 1.8 kHz) | Georeference the *whole* triangle/tow |
| GPS **PPS** | ~20 ns | — | **Time** (already on the green pair from one master; extra PPS per node is a check, not a second clock) |
| RTK / PPK / PPP (u-blox F9P class) | **1–3 cm** kinematic; ~1 cm static PPP (e.g. NSF MIMO-array test, 12×9 mm 95% ellipse) | **Enables 1.8 kHz BF across 15–30 m**; **fails 35 kHz** (need 4 mm) | Same + better georef |
| Dual-antenna GNSS heading | ~0.2–0.4° per metre of antenna boom | Rotates the array axis: 0.5° × 30 m ≈ **26 cm** | Best heading in steel/motors |
| Mag + 9-DoF IMU | **2–5°** after hard/soft-iron; 0.5–2° tilt | 3° × 30 m ≈ **1.6 m** — kills cross-buoy BF | Fine for **short** tow (1.6 m aperture) |
| Taut / cut copper | **mm–cm** if you measured the hose | **Best relative D_ij** | Not absolute |
| Class B 1 m / 2 m drops | cable length | Vertical baseline **known** | Tilt IMU → depth error ~ L sinθ |

**Per-node GPS does not replace taut copper or a surveyed tow.** It georeferences. RTK between buoys is the only GNSS that competes with a tape/hose at 1.8 kHz.

RANGING.md already said SPS 2–5 m saturates Tzirakis GCN / Grinstein / Grassmann DoA. That still holds **unless** you fuse RTK or `L_cable`.

---

## 3. Beamforming and coherence (what you will *see*)

### A. Four-phone copper tow (d ≈ 0.4 m, L ≈ 1.6 m)

- Design frequency ~1.8 kHz (d ≈ λ/2).
- Broadside HPBW ≈ **0.89 λ / L ≈ 26°**.
- Wired time: phase error ~ **< 1°** at 1.8 kHz → **full coherent gain ~ 6 dB** (10 log N) if phones are matched.
- IMU heading 2–5°: steers the beam by that amount — **inside the 26° beam**, so you still get gain; bearing bias ≈ heading error.
- GPS on the head: locates the tow in lat/lon to 2–5 m (SPS) or ~cm (RTK). Does **not** enter the steering vector if you use cut spacing.

**Expect:** real delay-and-sum at 1.8 kHz on the tow. Not imaging. Left/right ambiguity until heading or a second line.

### B. Three surface buoys, 15–30 m, **SPS GPS only** for `p_i`

- Relative error ~ **3–7 m** (√2 × SPS).
- Steering vector is random at 1.8 kHz (error ≫ λ).
- **No coherent array gain.** You get three *independent* phones: energy, SNR, detection, **incoherent** fusion.
- TDoA of a *loud source* still works **if clocks are wired** — but the *solver* needs `p_i`. Wrong `p_i` by 5 m on a 30 m baseline is a **large bearing bias** (order 10°).

### C. Same triangle, **taut known CAT7** (or RTK relative)

- Geometry to cm (hose) or 1–3 cm (RTK).
- Aperture 30 m, λ 0.82 m → HPBW ≈ **1.6°** if you really coherently sum (ambitious in a lake: multipath, SSP).
- Practical: treat as **three-element TDoA / GCC-PHAT** with good clocks. Source localization: range-scale error ~ (c σ_t) × dilution. σ_t ~ 1–10 µs → **0.15–1.5 cm** of *path*, geometric dilution ×3–10 on a 30 m triangle → **~0.05–1 m** source (x,y) **in the triangle frame** if SNR is high and you have a direct path.
- Then **one** GPS (or the average of three) pins that frame to Earth at SPS 2–5 m or RTK cm.

This is the `RANGING.md` 0.15–1.5 m story, except **time came from copper**, not CSAC+OWTT.

### D. 35 kHz and up

Need **4 mm** geometry and **~3 µs** time. Wired PPS can do time. **SPS GPS cannot do geometry. RTK is marginal. Only the hose/Class B measured baselines work.** Do not advertise coherent 35 kHz across buoys.

---

## 4. Sensor-fusion stack (honest layers)

```
Layer 0  Wired PPS              → common time  (done)
Layer 1  Hose / taut L_cable    → relative D_ij  (cm)
Layer 2  IMU / dual-GNSS heading → rotate figure, beam axis
Layer 3  RTK (optional)         → cm relative if no taut hose
Layer 4  SPS GPS                → georeference only
Layer 5  Acoustic TDoA / BF     → sources in that frame
Layer 6  K-STEMIT / Graph Views → uses D(t) from 1–3, not from SPS
```

**K-STEMIT / GCN:** feed `D_ij` from copper or RTK, **not** raw SPS. Then the 10–30× SNR claim in `RANGING.md` §1.2 actually applies.

**Coherence time:** re-PPS every 1 s; ESP32 oscillator between pulses is fine at 1.8 kHz. Lake multipath (T60) will limit coherent integration long before the clock does.

---

## 5. Numbers to quote

| Task | With SPS GPS + mag + wired PPS | With taut copper (or RTK) + IMU + wired PPS |
|------|--------------------------------|---------------------------------------------|
| Time across nodes | ~0.1–1 µs | same |
| Coherent BF on **tow** (1.8 kHz, 4 el) | **Yes** (geometry from hose) | **Yes** |
| Coherent BF **across 30 m buoys** at 1.8 kHz | **No** | **Yes** (cm geometry) |
| Source TDoA in triangle | Biased ~metres / ~10° | **~0.1–1 m** in-frame (high SNR, direct path) |
| Earth lat/lon of that fix | **2–5 m** SPS | **2–5 m** SPS or **cm** RTK |
| 35 kHz coherent across buoys | No | No (unless surveyed mm) |
| In-buoy vertical TDoA (Class B) | ~degree-scale bearing if 1 m baseline | same; tilt IMU at 1° → 1.7 cm depth |

**Bottom line:** GPS+orientation on every node **does not** give you a coherent 30 m array by itself. The copper already gave you time. **Hose/taut length (or RTK) gives you coherent 1.8 kHz.** SPS GPS gives you “where on the lake.” Mag/IMU points the short tow. That split is the fusion architecture.
