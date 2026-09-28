# Copper-only bus + wired time — literature ground

**Date:** 2026-09-25  
**Status:** Session research (not an ADR)  
**Plan under test:** `docs/research/sonobuoy-min-test-and-copper-tow.md`  
**Constraint:** **No Wi‑Fi.** CAT5/7 braid carries power, packets, and PPS. WeftOS talks to a USB gateway.

Sibling notes (agents): [time-sync-papers.md](./time-sync-papers.md) · [array-telemetry-papers.md](./array-telemetry-papers.md) · [lora-plc-and-can.md](./lora-plc-and-can.md) · [weftos-stack-map.md](./weftos-stack-map.md)

---

## Plan, in one paragraph

Three (minimum) ESP32-S3 nodes, radio **off**, on one waterproof CAT5/7: **12–24 V**, **CAN/TWAI** for `AcousticEvent`, **RS-422 PPS** for time, analog phones on a spare pair if needed. Shore USB-CAN → WeftOS. TDoA budget **~10 µs**. Tow: 4 phones at λ/2 (~0.4 m). Lake triangle **15–30 m**. Taut copper = tape-measure baselines; slack copper = time+power only.

---

## What the literature already does (this matches)

| Claim in our plan | Prior art | Implication |
|-------------------|-----------|-------------|
| Do **not** put time in CAN payloads | AUTOSAR CanTSyn: arbitration makes a single TIME message inaccurate; SYNC+FUP + HW timestamps required. Software CanTSyn ≈ **50 µs** (Luckinger TII 2022) — **fails our 10 µs budget**. | Dedicated PPS pair is not a luxury. CAN is for data + `pps_seq`. |
| Distribute GPS PPS, capture the edge | Industrial PPS: RS-422/485 over twisted pair, terminate, compensate cable delay. GPS PPS ±50 ns. TSN uses PPS as the sanity check on 1588 (±1 µs Class C). | Green pair + MCPWM capture. Delay cal ~5 ns/m. |
| Common **sample/strobe** down the array | Towed-array patents: leading edge of a clock travels the hose and **holds all hydrophones at once**; trailing edge staggers telemetry (US4464739). Streamers: twisted-pair digital bus + **Kevlar tension members** (US5583824). | Our PPS strobe is the 2026 cheap version of “array sample clock.” |
| Power + data on copper tow | 160-el coherent array: Cat6 + copper power (300 VDC / 600 m) + **independent frame-sync** + PTP/GPS **~1 µs**. Two-wire sonar telemetry (US9501926) shares power+data on one pair (TDM). | Four-pair CAT5 is *easier* than two-wire. Do not skip the sync channel. |
| Clock cascade with known per-node delay | JWCN 2013 offshore linear array: sub-µs sync, ~18 m spacing, delay model `t_ns = t_d + 2 t_c + l_n t_p + t_nm`. | Same equation as our `delay_ns[node]`. |
| Acoustic TWTT if **not** wired | Syed-Heidemann TSHL 2006; Webster OWTT — already in `RANGING.md`. | Keep for any node off the braid. Not the v1 backhaul. |
| LoRa-like CSS on wire | PLC paper: LoRa-style CSS on power line, **−40 dB**, **seconds** cadence. | Confirms LoRa-on-pair is a slow PLC, not PPS, not PCM. |

**Shape of a towed array** still needs heading/T/P or tautness (classic: beamform assumes a straight line unless you sense curvature).

---

## Verdict

The copper-only plan is **orthodox array engineering** (common clock + telemetry + power in one hose), not a WeftOS invention. The only WeftOS-specific bits are: ESP32-S3 + TWAI, USB-CAN gateway, substrate identity, `clawft-sonobuoy-ranging` consuming taut `L_cable` as exact `D_ij`.

**Keep:** PPS pair, CAN data, power pair, radio off, USB gateway.  
**Drop:** LoRa-as-bus, CAN-as-time, Wi-Fi JSON as time, slack-cable “position.”  
**Still later:** fiber, RFSoC, CSAC, 100 m–5 km OWTT.

**GPS + IMU on every node:** [gps-imu-fusion-accuracy.md](./gps-imu-fusion-accuracy.md) — SPS georeferences; taut copper or RTK is what makes 1.8 kHz coherent across buoys.

**Twin CAT5 + nylon braid:** [twin-cat5-nylon-braid.md](./twin-cat5-nylon-braid.md) — parallel (not helix) for L/R; Dyneema load, nylon spacer; loop at the tail, not IBM Token Ring.

**Detect / classify / resolution:** [tow-detect-classify-resolution.md](./tow-detect-classify-resolution.md) — ~35° beam at 1.8 kHz on a 1.2 m hose; L/R from twin CAT5; ranging via taut triangle; class ID needs a wide RX.

**Bioacoustics + twin over time:** [bioacoustics-urth-ruvector.md](./bioacoustics-urth-ruvector.md) — mahi schools as 1.8 kHz resonance *cues*; mysticetes yes; dolphin clicks no; fusion paints corridors in RuVector / Urth. Rename candidate **uvRTH**.

**Range (detection vs place):** [sonar-range.md](./sonar-range.md) — ~2 km hear a 120 dB chorus on the 4-el hose; ~50–150 m (x,y) on a 30 m triangle; hose itself has no range.

**RF land node (not the buoy):** [../antihunter-weftos-crosswalk.md](../antihunter-weftos-crosswalk.md) · vs RuView/OccWorld: [../antihunter-vs-ruview-world-models.md](../antihunter-vs-ruview-world-models.md).
