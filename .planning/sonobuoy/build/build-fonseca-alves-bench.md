---
title: Fonseca-Alves Rigid-Plate Bench Prototype — BOM and Build Sequence
author: clawft sonobuoy project
date: 2026-05-13
geometry: margin=0.8in
fontsize: 10pt
---

# Fonseca-Alves Rigid-Plate Bench Prototype

**Purpose**: bench-validate the three load-bearing claims of the Fonseca-Alves
moving-magnet underwater acoustic projector (patent WO2012095780A1, lapsed
2013, public domain) at clawft-relevant scale before committing it as the
project's primary acoustic transmitter:

1. **±3 dB flat magnitude** response from 10 Hz to 3 kHz
2. **Flat phase response** across the same band (the property that makes LFM
   chirps clean and matched-filter pulse-compression effective)
3. **Absolute SPL** of a scaled-up plate (paper validates 30 mm plate; this
   build uses ~100 mm to test the authors' claim that larger plates are more
   efficient at low frequencies)

**Scope**: bench / lab prototype only. NOT a deployable buoy build. No
pressure compensation hardware, no marine sealing, no ESP32-side firmware
integration. Just the actuator, the drive electronics, and the measurement
chain to characterise it.

**Reference**: Fonseca & Maia Alves (2012), *A new concept in underwater high
fidelity low frequency sound generation*, Rev. Sci. Instrum. 83(5):055007.
DOI 10.1063/1.4717680. Full analysis card at
`.planning/sonobuoy/papers/projector-mma/analysis/fonseca-alves-2012-rigid-plate.md`.

---

## 1. BOM — Actuator core ($55–80)

| # | Part | Spec | Source | Qty | Unit | Subtotal |
|---|---|---|---|---|---|---|
| A1 | Cylindrical neodymium magnet | N42, axially magnetized, 25 mm Ø × 25 mm tall | K&J Magnetics `DY0Y0-N52` or similar `D88` series | 1 | $18 | $18 |
| A2 | Rigid plate radiator | Al 6061 disc, 100 mm Ø × 2 mm thick, no holes | McMaster-Carr `89015K54` or Amazon | 1 | $10 | $10 |
| A3 | Magnet wire | 24 AWG enameled copper, 200 g spool (~155 m) | Amazon "BNTECHGO 24 AWG" | 1 | $14 | $14 |
| A4 | 3D-printed coil bobbins | PETG, 4 bobbins (2 excitation + 2 bias) sized to fit over magnet OD and slide inside housing ID | Print yourself on the project's Bambu P1S; ~30 g PETG total | 4 | print | ~$2 |
| A5 | PVC pipe housing | 4" schedule 40 PVC, 200 mm long section (gives ID ~102 mm) | Home Depot / Lowe's | 1 | $8 | $8 |
| A6 | PVC end caps | 4" sch 40 cap, 2× (one drilled for plate-rod feedthrough) | Home Depot | 2 | $3 | $6 |
| A7 | Coupling rod | M5 stainless threaded rod, ~150 mm | McMaster | 1 | $3 | $3 |
| A8 | Hardware | M5 nuts/washers, structural epoxy, JB Weld | Amazon | — | — | ~$10 |
|   |   |   |   |   | **Subtotal** | **~$71** |

## 2. BOM — Drive electronics ($25–35)

| # | Part | Spec | Source | Qty | Unit | Subtotal |
|---|---|---|---|---|---|---|
| D1 | Class-D amp board (AC excitation) | TPA3116D2, 2× 50 W stereo, 12–24 V supply, screw-terminal speaker output | Amazon "TPA3116D2 stereo amp board" | 1 | $12 | $12 |
| D2 | LM317T adjustable regulator (DC bias) | TO-220 package | Amazon / DigiKey | 2 | $1 | $2 |
| D3 | TO-220 heatsink + thermal pad | For the LM317 — it'll dissipate ~3.6 W standing | Amazon | 1 | $4 | $4 |
| D4 | Current-set resistor | 4.16 Ω, 5 W wirewound (gives 300 mA from LM317's 1.25 V reference) | Amazon | 2 | $1 | $2 |
| D5 | 12 V / 5 A bench power supply | Already have, or borrow | — | — | — | $0 |
| D6 | Audio interface / function gen | Use clawft laptop + Audacity OR existing function gen → 3.5 mm to amp input | — | — | — | $0 |
| D7 | Misc passives | Speaker wire, screw terminals, breadboard | Amazon | — | — | ~$5 |
|   |   |   |   |   | **Subtotal** | **~$25** |

## 3. BOM — Measurement / verification ($0–195)

Two paths depending on whether a calibrated reference hydrophone is on hand or
can be borrowed.

| # | Part | Spec | Source | Qty | Unit | Subtotal |
|---|---|---|---|---|---|---|
| M1 | Reference hydrophone | Aquarian H1c (calibrated, –211 dB re 1 V/µPa, 10 Hz – 100 kHz) | aquarianaudio.com | 1 | $190 | $190 |
| M1-alt | Project hydrophone | Use existing JFET preamp build per `build-hydrophone.md` | clawft project | — | — | $0 |
| M2 | Audio recorder / 96 kHz interface | Tascam DR-05X or laptop audio interface | Already have | — | — | $0 |
| M3 | Sample-rate-accurate timestamp | ESP32 sample-capture firmware or laptop direct | clawft project | — | — | $0 |
|   |   |   |   |   | **Subtotal A (project hydrophone)** | **$0** |
|   |   |   |   |   | **Subtotal B (reference hydrophone)** | **~$190** |

For Goal 1 (frequency-response shape) and Goal 2 (phase flatness), the project
hydrophone is sufficient because both are *relative* measurements. For Goal 3
(absolute SPL at 1 m) a calibrated reference is required unless the project
hydrophone has been previously calibrated against a known source.

## 4. BOM — Test tank ($20–50)

| # | Part | Spec | Source | Qty | Unit | Subtotal |
|---|---|---|---|---|---|---|
| T1 | Plastic tote tub | ≥60 L (large enough for 1 m hydrophone-to-actuator path without strong surface bounce) — e.g. 27-gallon Rubbermaid Roughneck | Home Depot | 1 | $25 | $25 |
| T2 | Distilled water or tap | Fill the tub | — | — | — | $0 |
| T3 | Foam isolation pads | 2× small foam pads to decouple from bench vibration | Amazon | 1 | $5 | $5 |
|   |   |   |   |   | **Subtotal** | **~$30** |

## 5. Grand totals

| Path | Cost |
|---|---|
| **Minimum** (project hydrophone for measurement, ~$71 actuator + ~$25 drive + ~$30 tank) | **~$126** |
| **With reference hydrophone** (Aquarian H1c for calibrated SPL) | **~$316** |

Inside the $200 target if the calibrated reference can be borrowed or
substituted with the project's already-calibrated JFET preamp; $316 if a
calibrated reference must be bought. Both still sit well within the
$50–500 BOM range estimated in the analysis card.

---

## 6. Mechanical layout

```
                       water column
                          │
                          ▼
          ┌────────────────────────────────┐
          │  100 mm Al disc (A2)           │   ← radiating face
          │                                 │
          └───────────────┬─────────────────┘
                          │  M5 rod (A7)
          ┌───────────────┴─────────────────┐
          │ PVC end-cap (A6, front)         │
          │  with rod feedthrough           │
          ├─────────────────────────────────┤
          │                                 │
          │  ┃ Bias coil  (~300 turns 24 AWG)│   ← DC, same winding
          │  ┃ on bobbin (A4)                │
          │  ┃                              │
          │  ┃ Excitation (~300 t)           │   ← AC, CW winding
          │  ┃                              │
          │  ╔═══ Magnet (A1) ═══╗           │   ← N42, 25 × 25
          │  ║                   ║           │
          │  ╚═══════════════════╝           │
          │  ┃ Excitation (~300 t)           │   ← AC, CCW winding
          │  ┃                              │
          │  ┃ Bias coil (~300 turns)        │   ← DC, same winding
          │  ┃                              │
          ├─────────────────────────────────┤
          │ PVC end-cap (A6, back, sealed)  │
          └─────────────────────────────────┘
                          ▲
                          │ housing flooded
                          │ (water-filled)
                          │
                       water column
```

Notes:

- Excitation coils wound in *opposite* directions and connected in series:
  when current flows in one direction in the series-connected pair, the two
  coils produce opposing fields, which combine with the magnet's poles to
  produce net axial force on the magnet.
- Bias coils wound in *same* direction and connected in series: DC current
  through them produces fields on both sides of the magnet that, when
  symmetric, balance to zero net force at the geometric centre — this is the
  "magnetic spring" that replaces the rubber surround on a conventional
  speaker.
- The housing is flooded with water in deployment — for bench testing in a
  tank, that happens naturally as the prototype is submerged. No internal
  air cavity means no pressure-dependent compliance.

---

## 7. Build sequence

1. **Print 4 PETG bobbins**. Outer Ø ≤ 102 mm (PVC ID). Inner Ø ≥ 27 mm
   (magnet OD + 2 mm clearance). Axial length 25–35 mm each. Recommend
   PETG over PLA for mild water resistance and easier winding tension.
2. **Wind ~300 turns of 24 AWG** on each bobbin. Mark each coil's two leads
   with the winding direction (e.g. red = CW lead, blue = CCW lead). Coil
   DCR target: ~2–4 Ω each. Inductance is dominated by air-core so will be
   modest; coil-self-resonance well above the 3 kHz operating band.
3. **Stack inside the PVC housing**: from front to back — front bias coil →
   front excitation coil → centre cavity for magnet → rear excitation coil
   → rear bias coil. Glue each bobbin to the PVC inner wall with structural
   epoxy once positioned. Leave the magnet cavity unobstructed; the magnet
   floats axially within it.
4. **Attach plate to magnet via M5 coupling rod** through the front end cap.
   The rod must slide axially in the feedthrough with minimal play (a brass
   bushing or oil-filled gland in deployment; for bench, a snug clearance
   hole is fine). Plate glued or screwed to the rod end.
5. **Wire excitation coils in series** (CW lead of front coil → CCW lead of
   rear coil) so a single AC drive produces opposing fields. Connect series
   pair to TPA3116D2 speaker output.
6. **Wire bias coils in series** (both same winding direction) to the LM317
   current-source output. The 4.16 Ω resistor between LM317 OUT and ADJ
   sets the regulated current at I = 1.25 V / 4.16 Ω ≈ 300 mA.
7. **Glue rear end cap on** with structural epoxy or PVC cement. Drill a
   small hole in the rear cap for water flooding (or fit a cable feedthrough
   if leads exit the rear).
8. **Bench test #1 (in air)**: apply a 100 Hz sine to excitation at ~1 V
   amplitude. Confirm the plate visibly oscillates. Confirm the magnet
   doesn't bottom out against either coil-bobbin face (if it does, increase
   bias current to stiffen the magnetic spring).
9. **Bench test #2 (in water)**: submerge fully in the tank. Position the
   hydrophone at 30 cm distance, on-axis. Sweep 10 Hz → 3 kHz at constant
   drive amplitude (~1 V peak), record the received signal.
10. **Analyse** with an FFT-based transfer function (the same method
    Fonseca-Alves used in the paper). Plot magnitude (dB) and phase (deg)
    vs frequency. Plot coherence as a measurement-validity check.

---

## 8. Acceptance criteria

| Metric | Acceptable | Target | Notes |
|---|---|---|---|
| Frequency response, 10 Hz – 3 kHz, magnitude | ±6 dB | ±3 dB | Paper claim is ±3 dB on a 30 mm plate; a 100 mm plate should match or improve |
| Phase response, same band | < 90° deviation | < 30° deviation | The load-bearing claim for chirp compatibility |
| Coherence | > 0.7 | > 0.9 | Measurement validity |
| LFM chirp compression sidelobe level | < –10 dB | < –20 dB | Pulse-compression test for matched filter |
| DC bias current | 250–400 mA | ~300 mA | Sanity check the regulator |
| Peak AC drive current | < 3 A | 1–2 A | Coil DCR + voltage swing |
| Absolute SPL at 1 m, 1 kHz | ≥ 130 dB re 1 µPa | ≥ 150 dB re 1 µPa | Reference-hydrophone path only |

If the first prototype hits "Acceptable" on rows 1–4, the architectural bet
is validated and the design moves forward into the Tier-1-anchor build path
(per ADR-084 §1.6 and §"Option E"). If it misses, adjust bias current first
(changes magnetic-spring stiffness → resonance), then plate diameter, then
coil-stack length, in that order, before declaring the approach unworkable.

---

## 9. What this validates / does NOT validate

**Validates if successful**:

- The Fonseca-Alves flat-phase no-resonance claim holds at clawft scale
- LFM chirps generated through the actuator are clean-compressible at the
  receiver (the property that makes this attractive as the primary chirp
  transmitter, not just a beacon)
- The drive electronics ($25 BOM) are sufficient for bench characterisation
- A scaled-up plate matches the authors' "larger discs are more efficient"
  claim at low frequency

**Does NOT validate** (deferred to later builds):

- Pressure-cycling survival (no pressure pot here)
- Long-term drift / coil-temperature stability
- Coexistence with other clawft-band acoustic traffic (1.8 kHz mesh chirps)
- 4-face TDM scheduling on a Tier-1 anchor
- Marine biofouling resistance of the radiating plate
- Cable feedthrough sealing for deployment

These all become the Phase 5 build doc's concerns once the bench prototype
validates the underlying physics.

---

## 10. References

- Fonseca & Maia Alves (2012), *Rev. Sci. Instrum.* 83(5):055007 —
  `.planning/sonobuoy/papers/projector-mma/pdfs/fonseca-alves-2012-rsi-rigid-plate-projector.pdf`
- Analysis card —
  `.planning/sonobuoy/papers/projector-mma/analysis/fonseca-alves-2012-rigid-plate.md`
- Patent WO2012095780A1 (ceased 2013-07-10, public domain) —
  https://patents.google.com/patent/WO2012095780A1/en
- Transmitter options catalog (why this design over alternatives) —
  `.planning/sonobuoy/papers/projector-mma/transmitter-options-catalog.md`
- ADR-084 §"Option E" + §1.6 (the architectural commitment this bench
  prototype gates) —
  `.planning/symposiums/sonobuoy/adrs/ADR-084-acoustic-time-sequencing.md`
- Project hydrophone build —
  `.planning/sonobuoy/build/build-hydrophone.md` and `build-hydrophone-oil.md`
