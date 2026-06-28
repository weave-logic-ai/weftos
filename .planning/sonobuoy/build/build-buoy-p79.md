# Sonar Buoy — Imaging Test Buoy (Phase 5 prototype, P79 + 235 kHz)

**Status**: Planning. Transducers ordered 2026-05-11.
**Date opened**: 2026-05-11
**Owner**: Mathew Beane
**Transducer A**: Airmar **P79**, MFR P/N **31-234-2-01** — in-hull
dual-frequency **50 / 200 kHz** with integrated thermistor. Surplus,
$35.
**Transducer B**: Airmar **D**, MFR P/N **44-053-1-02 rev A**,
customer P/N **10000843** — **235 kHz** narrow-beam (depth /
high-resolution). Specs to verify on receipt (Appendix B).

**Goal**: Build **one** experimental buoy that pairs the existing
1.8 kHz omnidirectional mesh element with **two** downward-pointing
Airmar transducers — a P79 covering the 50 kHz and 200 kHz bands and
a dedicated 235 kHz narrow-beam unit — and prove out the **imaging
tier** end-to-end: pulser → acoustic → matched-filter RX → chain
stream → shore reconstruction, across **three** bands plus the
existing 1.8 kHz mesh.

This is the prototype that lights up Phase 5 of `roadmap.md`. If it
works, it becomes the template for the rest of the fleet. It is a
deliberately separate buoy from the v1 three-buoy fleet — the rest
of the fleet stays on the bare-piezo / JFET-hydrophone build until
this one is validated.

## Companion docs

- `requirements.md` — the base 1.8 kHz buoy this one inherits from.
- `architecture.md` — chain envelope conventions; new streams below
  follow them.
- `commercial-comparison.md` §8.1 / §8.3 — why a dual-band Airmar
  is the right Phase 5 part and what gap it closes.
- `roadmap.md` Phase 5 — imaging-tier rationale.
- `build-hydrophone-oil.md` — the oil-filled chamber pattern this
  buoy re-uses (scaled up for the P79 puck).

### Research-half anchors (`.planning/sonobuoy/` one level up)

This buoy is the **hardware grounding for the active-imaging
branch** of the 5-branch K-STEMIT-extended architecture documented
in `../SYNTHESIS.md` §2.4. Specifically:

- **ADR-063** — Active-imaging (SAS) as the 5th, phase-coherent
  branch in the K-STEMIT architecture. Source: Hayes & Gough 2009
  IEEE JOE review.
- **ADR-064** — Deep SAS autofocus (Gerg & Monga 2021,
  arXiv:2103.10312) as the primary phase-error corrector. The
  shore-side reconstruction service that consumes this buoy's
  `acoustic.imaging` stream is where that model runs.
- **ADR-065** — Multistatic SAS with **stationary sonobuoy as
  receiver node** (Kiang & Kiang 2022, IEEE TGRS). The clawft
  buoy fleet are the **C-nodes in Kiang's transceiver-A +
  receiver-B + stationary-C geometry**. The bistatic Mode E in
  this build (TX on P79, RX on 235 kHz D within the same buoy)
  extends the multistatic geometry to *per-buoy* as well as
  *fleet-scale* — Kiang assumes a single C-node, we have C-nodes
  with internal A/B faces.

  **Band-dependent coherence honesty (added 2026-05-11)**: the
  ADR-065 plan is **coherent at 1.8 kHz** (which RANGING.md's
  CSAC + TSHL stack actually supports cleanly). At the imaging
  tier (50 / 200 / 235 kHz) the CSAC timing budget is **in budget
  but tight** — see `.planning/sonobuoy/RANGING.md` §6.2 for
  the per-band Allan-deviation calculation. The Phase 5d gimbal
  work below must validate the coherent budget empirically;
  **the default until measured is incoherent (envelope-only)
  multistatic reconstruction**. Treat coherent SAS at 200/235 kHz
  as a Phase 5d→7 stretch goal, not a guaranteed capability.
- **ADR-066** — Wenz 1962 three-band noise prior. The on-buoy
  thermistor (P79 internal, no DS18B20 on this buoy) feeds the
  wind / sea-state inputs that condition the noise prior in the
  shore-side service.
- **ADR-067** — KRAKEN / BELLHOP propagation solvers run on the
  shore host as the ground truth that the FNO surrogate
  (ADR-080 ThermoFno from `../GAPS.md` G3) is benchmarked against
  before consuming this buoy's stream.

The Phase 5d gimbal upgrade is what makes the buoy a true
mechanical-scan SAS-class node — per-buoy angular aperture plus
mesh sparse aperture from the fleet. The Phase 5e cast-urethane
acoustic dome makes the omnidirectional-pointing claim acoustically
honest at the bands ADR-063-065 care about.

### Acoustic-physics budget for this buoy

Numerical grounding from panel P1
(`.planning/symposiums/sonobuoy/panels/P1-acoustic-physics.md`),
load-bearing for the operating modes below and the Stage 1-4
validation plan:

- **Sonar-equation SE (Mode A, 235 kHz active monostatic onto
  pool bottom at R=2 m)**: SL = 200 dB, 2·TL = 12 dB,
  TS_bottom ≈ −20 dB (mud), NL_det = 77 dB, DI = 25 dB,
  DT = 10 dB → **SE = +106 dB**. Massively over-budget at pool
  depth; the build-doc ±5 cm depth claim sits on ~80 dB of
  margin. P1 §1.1.6.
- **Mode A scaled to 100 m open water**: 2·TL_spreading = 80 dB,
  2·α·R = 13 dB (235 kHz seawater) → **SE = +25 dB**. Practical
  imaging range to bottom is **200–300 m** before absorption
  dominates and SE → 0. Beyond that range the 235 kHz channel
  is not a usable imager.
- **Mode F (passive listening, 1.8 kHz, vocal-fish school SL =
  120 dB @ 1 m)**: SE = 0 at **~1 km single-buoy**, **~3.2 km
  10-buoy mesh × 10 m**, **~14 km 20-buoy × 100 m tow-line**.
  See P1 §1.1.7. Density × aperture is the load-bearing variable,
  not transducer quality.
- **Near-field / far-field**: at R = 2 m pool depth, all bands
  are in the **far-field** (r/r_FF ≥ 5–24). Stage 2 (10 cm
  bucket) is in the **near-field** at 200/235 kHz — expect
  ringing / phase variation that disappears at Stage 3. The
  ±5 cm depth acceptance criterion is honest at Stage 3, not
  Stage 2. See P1 §1.7.
- **Cavitation threshold**: at 2 m pool depth, threshold is
  ~1.2 atm peak negative pressure ≈ 120 kPa. The build-doc
  cap "50 Vpp on bench, push to 80 Vpp once we trust the loop"
  yields 80 kPa at 50 Vpp (safe, factor 0.7×) and **~130 kPa at
  80 Vpp (AT THRESHOLD)**. Going above 50 Vpp without drive
  monitoring will silently cavitate; cavitation onset shows as
  a sudden ≥20 dB broadband noise rise during the TX pulse
  envelope. Phase 7 HV pulser at 500 kHz/200 Vpp cavitates
  without question and needs explicit mitigation. See P1 §1.9.
- **PVC half-wave window**: a 6 mm PVC wall is naturally
  transparent at 200 kHz (f_match = c_PVC/(2t) = 2395/0.012 =
  200 kHz). At 235 kHz the wall sits at t/λ_wall ≈ 0.59 —
  near-perfect transmission. Validates the Path D gimbal claim
  that the PVC wall in every radial direction is the acoustic
  window at 235 kHz. See P1 §1.6.

## One-line summary

A standard 2" PVC buoy with everything above the waterline reused
verbatim, plus **two** in-hull transducers (P79 and 235 kHz D)
downward-pointed inside their own mineral-oil couplant chambers,
plus a small pulser/LNA/ADC daughterboard under the existing
ESP32-S3 that band-switches a shared signal chain across all three
imaging bands (50, 200, 235 kHz).

### Why two transducers, not one

The P79 already covers 50 and 200 kHz, so adding a 235 kHz channel
is a deliberate architectural choice, not a redundancy:

- **Resolution**: at 235 kHz, λ ≈ 6.3 mm vs. 7.4 mm at 200 kHz —
  marginal alone, but a different element means *different beam
  width and different mechanical aperture*, often tighter than
  the P79's 200 kHz beam (which is wide because the same puck has
  to also radiate at 50 kHz).
- **Frequency diversity**: pinging at 200 and 235 kHz in alternation
  on the same target gives a two-point spectral signature — useful
  for classifying bottom hardness, biological targets, and
  distinguishing real returns from sidelobe artefacts.
- **Independent geometry**: two physically separate transducer
  faces means we can run **bistatic** at the imaging tier on a
  single buoy (P79 transmits 200 kHz, 235 kHz unit listens) once
  we have enough drive isolation. Bistatic separation kills the
  near-field saturation problem that monostatic imaging suffers
  at close range.
- **Failure independence**: P79 dies → 235 kHz still works for
  depth and high-res imaging; 235 kHz dies → P79 still covers
  the long-range 50 kHz band.

## What's different about this buoy

| Subsystem            | Standard fleet buoy        | Imaging test buoy                              |
|----------------------|----------------------------|------------------------------------------------|
| Above-water section  | unchanged                  | unchanged                                      |
| 1.8 kHz mesh element | piezo disc + JFET hydro    | piezo disc + JFET hydro (kept, runs in parallel) |
| Temperature probe    | DS18B20                    | **P79 internal thermistor** (DS18B20 omitted; 235 kHz unit is "D" = depth-only, no thermistor) |
| Downward acoustic    | none                       | **P79 puck + 235 kHz D**, both oil-coupled, both pointing down |
| Pulser               | DRV8837 H-bridge @ 6.6 Vpp | **half-bridge MOSFET pulser @ ~50–80 Vpp**, band-multiplexed across 50 / 200 / 235 kHz |
| RX front end         | MCP6022 BPF                | MCP6022 BPF *and* AD8021 LNA + T/R switch + 3-way band-selectable BPF |
| ADC                  | S3 internal SAR @ 16 kS/s  | **External SAR @ 1 MS/s** alongside internal   |
| New chain streams    | —                          | `acoustic.depth`, `acoustic.imaging`           |

The 1.8 kHz mesh path is *unchanged and unaffected*. The P79 daughter-
board hangs off the existing S3 as a coprocessor; if it fails, the
buoy degrades gracefully to a standard fleet node.

## Bill of materials

### Transducers

- **Transducer A — Airmar P79, MFR P/N 31-234-2-01** — $35.
  - Dual-frequency 50 / 200 kHz, in-hull (shoot-through), with
    integrated 10 kΩ NTC thermistor.
  - **Connector and pinout to be confirmed on receipt** (Appendix A).
    The 31-234 family ships with several connector variants; we
    will cut the OEM connector off and solder direct in any case.
  - Beam: ~45° at 50 kHz, ~11° at 200 kHz, both conical.
  - Element capacitance ≈ 1–3 nF per band (to verify).

- **Transducer B — Airmar "D", MFR P/N 44-053-1-02 Rev A,
  customer P/N 10000843** — price TBD (eBay listing).
  - **Single-frequency 235 kHz.** Confirmed from label photo
    (Appendix B).
  - **"D" suffix = depth-only**, no thermistor. The P79 covers
    temperature on this buoy.
  - Date code **01/04** (manufactured January 2004) — surplus
    stock. Functional condition to be confirmed on receipt.
  - US Patent 4,961,178 B1 referenced on the label (early-1990s
    Airmar element design).
  - Likely OEM source: early-2000s Garmin / Lowrance bottom
    machine high-frequency element. Mount style (in-hull,
    transom, thru-hull) and beam width to be confirmed on
    receipt (Appendix B).
  - **Connector and pinout to be confirmed on receipt**
    (Appendix B); will cut OEM connector and solder direct as
    with the P79.

### Pulser daughterboard (DIY)

| Part                                       | Qty | Approx. cost |
|--------------------------------------------|-----|--------------|
| TC4427A dual non-inverting MOSFET gate drv | 1   | $1.50        |
| IRLZ44N N-channel MOSFET, logic-level      | 2   | $1.20        |
| Ferrite step-up xfmr (FT50-43 toroid, hand-wound 8:24) | 1 | $3 |
| 100 µF / 100 V electrolytic (rail bulk)    | 1   | $1.00        |
| Bidirectional TVS, ~33 V SMBJ              | 1   | $0.50        |
| Snubber: 10 nF X7R + 10 Ω 1 W              | 1   | $0.20        |
| **Pulser subtotal**                        |     | **~$8**      |

### Receive front end

| Part                                       | Qty | Approx. cost |
|--------------------------------------------|-----|--------------|
| ADG704 4-channel analog mux (T/R + transducer + band sel) | 2 | $4.00 |
| AD8021 low-noise op-amp                    | 2   | $8.00        |
| Three-band switchable Sallen-Key BPF passives (50 / 200 / 235 kHz) | 1 set | $3.00 |
| Bias / decoupling                          | —   | $1.00        |
| **Front-end subtotal**                     |     | **~$16**     |

### ADC + interface

| Part                                       | Qty | Approx. cost |
|--------------------------------------------|-----|--------------|
| MCP33131D-10 SAR ADC, 1 MSPS 16-bit, SPI   | 1   | $5.00        |
| (Optional alt: ADS9224R 3 MSPS)            | —   | $15.00       |

### Power for the pulser rail

| Part                                       | Qty | Approx. cost |
|--------------------------------------------|-----|--------------|
| TPS61023 boost converter @ 12 V intermediate | 1 | $2.50        |
| LMR16006Y or charge-pump cascade to ~50 V  | 1   | $3.00        |
| Output caps + inductor                     | —   | $2.00        |
| **Power subtotal**                         |     | **~$7.50**   |

### Mechanical (two couplant chambers — one per transducer)

| Part                                       | Qty | Approx. cost |
|--------------------------------------------|-----|--------------|
| 2" PVC short coupler / cap (custom-cut)    | 2   | $6.00        |
| 2" PVC end cap, drilled for fill port      | 2   | $4.00        |
| Mineral oil, ~160 ml total                 | —   | $3.00        |
| M5 nylon screw + O-ring (oil fill port)    | 2   | $2.00        |
| Marine epoxy / silicone sealant            | —   | $3.00        |
| **Mechanical subtotal**                    |     | **~$18**     |

### Reused from base buoy (no new cost)

- 1× ESP32-S2 mini (1.8 kHz TX MCU)
- 1× ESP32-S3 mini N8R8 (1.8 kHz RX MCU + P79 daughterboard host)
- 1× 35 mm 1.8 kHz omnidirectional piezo TX
- 1× JFET hydrophone (1.8 kHz RX)
- 1× WiFi antenna + threaded PVC mast
- 1× 1S 18650 battery + TP4056 + AP2112 LDO
- 1× existing 2" PVC body + vented ballast section

**New BOM total: ~$85** (+ whatever the 235 kHz unit lists for; user-quoted as eBay-priced)
**Whole-buoy BOM: ~$130** (excluding the surplus 235 kHz unit)

## Mechanical: transducer mounting in the buoy

Both the P79 and the 235 kHz D are in-hull / shoot-through-style
transducers designed to fluid-couple through a plastic wall (the P79
explicitly; the 235 kHz D verify-on-receipt — if it turns out to be
a transom-mount or thru-hull puck, the chamber wall it presses
against still works as a coupling window). Our 2" PVC buoy wall is
well within the thickness range either tolerates.

The two imaging transducers each get their **own couplant
chamber**, stacked vertically below the electronics section and
above the vented ballast. Two chambers, not one, because:

- The two pucks have different physical faces and beam axes —
  pressing them against opposite walls of a single chamber gives
  each a clean acoustic path with no inter-element shadowing.
- One transducer failing (oil leak, element crack) does not
  contaminate the other's chamber.
- The 235 kHz D dates to 2004; mounting it in a separately
  serviceable chamber lets us swap it for a modern part later
  without disturbing the P79.

```
                 (electronics section, sealed above)
─── threaded coupler with O-ring + grease ───────────
│   1.8 kHz JFET hydrophone (existing)               │
│   (oil-filled chamber per build-hydrophone-oil.md) │
├────────────────────────────────────────────────────┤
│                                                    │
│   P79 COUPLANT CHAMBER  (this build)               │
│   - short 2" PVC section, oil-filled               │
│   - P79 puck face DOWN against bottom cap          │
│   - mineral-oil acoustic couplant                  │
│   - M5 nylon fill port through side wall           │
│                                                    │
├────────────────────────────────────────────────────┤
│                                                    │
│   235 kHz D COUPLANT CHAMBER  (this build)         │
│   - short 2" PVC section, oil-filled               │
│   - 235 kHz puck face DOWN against bottom cap      │
│   - mineral-oil acoustic couplant                  │
│   - M5 nylon fill port through side wall           │
│   - both cables feedthrough via shared             │
│     waterproof gland to electronics section        │
│                                                    │
├────────────────────────────────────────────────────┤
│   Vented ballast section (unchanged)               │
│   Vent holes 1 cm below rim                        │
│                                                    │
                  (open to water below)
```

Mounting notes:

- **Pucks face down**, lightly spring-loaded against the inside of
  each chamber's bottom cap. A foam ring above each puck supplies
  the pressure; oil fills the gap.
- **No air gaps** anywhere along either acoustic path. Bleed air
  through the fill ports after filling; top up with oil.
- **Cable strain relief** before each cable's feedthrough. Trim
  OEM cables to ~25 cm inside their respective chambers and splice
  through a small marine terminal block in the electronics section.
- **Vertical stacking order matters**: P79 above 235 kHz means the
  235 kHz puck has the more direct downward acoustic view (no
  intervening chamber). The P79 is dual-band and forgives slightly
  worse mounting geometry; the high-frequency 235 kHz is the one
  to put in the cleanest position.
- **The 1.8 kHz JFET hydrophone keeps its own sidecar chamber** per
  `build-hydrophone-oil.md`. Do not combine with either imaging
  chamber — different bands, different electrical requirements,
  different failure modes.

## Electrical: signal chain

### TX (pulser) path

```
                                                         ┌─► P79 50 kHz pair
   ESP32-S3 GPIO ──► TC4427A ──► IRLZ44N half-bridge ────► step-up xfmr ──► ADG704 ──┼─► P79 200 kHz pair
   (square pulse                 (low-side switching       (8:24 turns,    (band sel)│
    or chirped FM)                of 12 V rail)            ~50–80 Vpp out)           └─► 235 kHz D element
                                                              │
                                                              └─── TVS clamp + RC snubber to ground
```

One pulser, one transformer, one analog mux selecting the
destination element. Only one band fires per ping. Cost in BOM is a
single extra ADG704 ($2); cost in code is a 2-bit band-select GPIO
group.

Pulse parameters at v1:

- 235 kHz mode: 8–24 cycles of square or linear-FM chirp,
  repetition 1 Hz.
- 200 kHz mode: 8–16 cycles of square or linear-FM chirp,
  repetition 1 Hz.
- 50 kHz mode: 4–8 cycles of square or short-chirp, repetition 1 Hz.
- All modes: average duty < 1 %.

**Peak voltage at the element — safety-locked structure** (per
P2 §2.8, consuming P1 §1.9 cavitation analysis):

- **Default: 50 Vpp.** Firmware ships with this value. Produces
  ≈ 80 kPa peak pressure at 1 m; the cavitation threshold at 2 m
  pool depth is ≈ 120 kPa. 50 Vpp clears threshold by a factor
  of 1.5×; SE for Mode A (235 kHz onto pool bottom) lands at
  +106 dB margin per P1 §1.1.6.
- **Enhanced: 80 Vpp.** Allowed **only** when firmware
  cavitation monitoring is active and reports healthy. 80 Vpp
  produces ≈ 130 kPa — AT THRESHOLD at 2 m depth. Cavitation
  monitor samples RX-side broadband noise during/after TX; a
  ≥10 dB rise above the calibrated 50 Vpp baseline triggers
  `pulser.cavitation_alarm` and reverts drive to 50 Vpp.
  Enabling enhanced mode requires an ED25519-signed operator
  command per ADR-081, and is capped by the `max_drive_vpp`
  chain-streamable parameter (default 100 Vpp).
- **Forbidden: ≥100 Vpp in this build.** 100 Vpp produces
  ≈ 160 kPa, cavitates at any pool depth shallower than ~6 m
  hydrostatic, and produces broadband noise that defeats
  matched-filter detection on the same buoy's RX path. Do not
  exceed 80 Vpp without revisiting the cavitation analysis
  (P1 §1.9.2) and adding explicit deeper-water or single-cycle
  mitigations.

The 2004 235 kHz D unit is more voltage-tolerant than its label
suggests (vintage Airmar elements were rated 600 W peak), so the
50/80 Vpp cap is not transducer-limited — it is **water-
physics-limited**.

### RX (front end) path

```
   P79     ─►┐
   235 kHz ─►┤ ADG704 ──► T/R switch ──► AD8021 stage 1 ──► ADG704 BPF select ──► AD8021 stage 2 ──► MCP33131D ──► S3 (SPI DMA)
   (transducer  (RX                       (20 dB)              ┌─► 50 kHz BPF       (20–40 dB)         (SAR ADC)
    select)      arming)                                       ├─► 200 kHz BPF                       
                                                               └─► 235 kHz BPF
                       │
                       └── (during TX) shunts input through 100 Ω to mid-rail
```

Notes:

- **Transducer-select mux at the front** — same ADG704 family
  routes which transducer's RX leg is connected to the LNA. Default
  routing matches the TX band, but bistatic modes (TX on one
  transducer, RX on the other) are a single GPIO-group change.
- **T/R switch is mandatory.** A 50 Vpp transmit pulse will instantly
  destroy an AD8021 input. The ADG704 (or a back-to-back schottky
  diode clamp + series resistor as a low-cost fallback) routes the
  LNA input to mid-rail bias during TX.
- **Switchable BPF** — three Sallen-Key sections, ADG704 selects
  which is in the path. 50 kHz (Q ≈ 6), 200 kHz (Q ≈ 8), 235 kHz
  (Q ≈ 10). The 200 and 235 kHz filters are close enough that they
  can share a common topology with different cap values.
- **Stage 2 gain** is software-selectable via a second mux feeding
  different feedback resistors. We will not need full 60 dB at close
  range.
- **ADC SPI** at 16 MHz to the S3, DMA into PSRAM, ring-buffered.

### Microcontroller integration

The existing 1.8 kHz pipeline already uses the S3's PSRAM and one
core for matched filtering. The P79 daughterboard re-uses the
*other* core and the *other* SPI peripheral:

| S3 resource              | 1.8 kHz pipeline             | Imaging pipeline (P79 + 235 kHz) |
|--------------------------|------------------------------|---------------------------------|
| Core 0                   | analog ISR + Goertzel bank   | (idle)                          |
| Core 1                   | protocol + chain publish     | **matched filter + bottom-pick**|
| SPI host 2               | (unused)                     | **MCP33131D ADC**               |
| GPIO group A             | TX_ACTIVE, status            | unchanged                       |
| GPIO group B             | (unused)                     | **pulser gate, T/R, transducer sel, BPF sel** |
| I²C                      | DS18B20 / temp               | **P79 thermistor via daughter ADC** |
| ADC1 ch3                 | analog front end             | unchanged                       |
| PSRAM region A           | 1.8 kHz ring buffer          | unchanged                       |
| PSRAM region B           | (unused)                     | **imaging capture ring + FFT scratch** |

If S3 throughput becomes the bottleneck, the daughterboard already
has the right shape to grow an iCE40 UP5K coprocessor between the
ADC and the S3 (per `commercial-comparison.md` §8 / FPGA discussion).
Not in this build.

## Firmware

### New chain streams

```rust
// One record per downward ping
AcousticDepth {
    buoy_id: u8,
    band_khz: u16,                 // 50 or 200
    timestamp_local_us: u64,
    timestamp_wallclock_ms: u64,
    ping: {
        pulse_kind: "cw" | "chirp",
        duration_us: u32,
        peak_correlation: f32,
    },
    bottom: Option<{
        time_us: u64,              // µs from TX to bottom-pick leading edge
        depth_m: f32,              // bottom_time_us × c(temp) / 2
        return_strength_db: f32,
        confidence: f32,
    }>,
    water_temp_c: f32,             // from P79 thermistor
    battery_mv: u16,
}

// One record per imaging window (heavier; published at lower rate or filtered)
AcousticImaging {
    buoy_id: u8,
    band_khz: u16,
    timestamp_local_us: u64,
    pulse_kind: "cw" | "chirp",
    window_samples: u32,
    sample_rate_hz: u32,
    samples: Bytes,                // raw I/Q or compressed envelope
    water_temp_c: f32,
}
```

### Operating modes

| Mode | Pulse                                                       | Cadence    | Use                                                |
|------|-------------------------------------------------------------|------------|----------------------------------------------------|
| A    | 235 kHz linear-FM chirp on 235 kHz D unit, 16 cycles        | 1 Hz       | Highest-resolution depth + bottom imagery          |
| B    | 200 kHz linear-FM chirp on P79, 15 cycles                   | 1 Hz       | Comparable-resolution baseline, dual-element diversity |
| C    | 50 kHz linear-FM chirp on P79, 8 cycles                     | 1 Hz       | Long-range depth, structure                        |
| D    | A / B / C round-robin                                       | 3 s cycle  | All three bands sampled at 1/3 Hz each             |
| E    | TX on P79 200 kHz, RX on 235 kHz D                          | 1 Hz       | **Bistatic** — TX/RX on separate transducer faces; cancels near-field saturation |
| F    | TX disabled, RX on selected transducer                      | continuous | Directional passive listening on the chosen face   |

Mode is software-selectable, defaulting to **D** in normal
operation. Mode **E** is the bistatic mode that the two separate
transducers structurally enable — worth its own validation pass
during Stage 4 to characterize the cross-coupling between the two
oil chambers (acoustic, not electrical).

### Timing

Each ping cycle:

```
 t=0:   assert TX_ACTIVE, switch T/R to "isolate", arm pulser
 t≈10µs: pulser fires (one band at a time)
 t≈90µs: pulse complete, hold T/R isolate for ~150 µs (ringdown blanking)
 t≈250µs: release T/R, ADC capture begins, matched filter armed
 t≈70ms: capture window ends (covers ~50 m one-way at 200 kHz)
 t≈70-150ms: matched filter + bottom-pick on core 1
 t≈150ms: publish `AcousticDepth` to chain, optionally `AcousticImaging`
 t=1s:  next ping
```

The 850 ms of slack per cycle is deliberate — leaves room for the
1.8 kHz mesh to do its own thing without contention.

## Bench validation plan

### Stage 1 — dry bench, no transducer in water

- Connect a **dummy load** (a 2 nF capacitor in parallel with a 30 Ω
  resistor) in place of the P79.
- Scope across the dummy load: verify clean ~50 Vpp pulse with no
  ringing past 20 µs after pulse end.
- Inject a known signal (function generator into a 1 µF coupling cap
  → BPF input) and verify the LNA + BPF + ADC chain captures it
  cleanly. Sweep 30–250 kHz; verify both BPF center frequencies.
- Verify T/R isolation: pulse with the dummy load + LNA input
  connected; the LNA output should not saturate or show pulse
  bleed-through above mid-rail noise.
- Read the **P79 thermistor** (still on the bench) — confirm ~10 kΩ
  at room temp.

### Stage 2 — bucket of water

- Mount the P79 in the couplant chamber, fill with mineral oil, bleed
  air.
- Submerge the couplant chamber in a 5-gallon bucket, puck face down
  at ~10 cm above the bucket floor.
- Single ping at 200 kHz. Expect a bottom-floor echo at ~135 µs
  (10 cm × 2 / 1480 m/s).
- Verify matched-filter peak at the right delay.
- Repeat at 50 kHz.

### Stage 3 — pool, P79 only (1.8 kHz disabled)

- Drop the test buoy into the pool.
- Run Mode A (200 kHz chirp) at 1 Hz for 5 minutes.
- Confirm `AcousticDepth` records arrive on chain with valid
  `bottom.depth_m` near the pool depth.
- Confirm thermistor temperature looks sensible.
- Switch to Mode B (50 kHz); compare bottom-pick consistency.
- Capture one `AcousticImaging` window per band; reconstruct shore-
  side and visually inspect for the bottom return.

### Stage 4 — pool, both bands simultaneously

- Re-enable the 1.8 kHz mesh path.
- Run Mode C (50 / 200 kHz interleaved) and 1.8 kHz MFSK beacons
  concurrently for 10 minutes.
- Verify **no regression** in 1.8 kHz mesh: same SNR on the 1.8 kHz
  RX as observed in the standard buoy.
- Verify no pulser-induced supply sag (scope the 3.3 V rail during
  a ping).

## Acceptance criteria

- All Stage 1–4 checks pass.
- Pool floor depth measured to ±5 cm at 200 kHz, ±15 cm at 50 kHz.
  **Acceptance applies at Stage 3 only** (pool, full far-field at
  the most-restrictive 235 kHz P79 band, r/r_FF ≥ 5). Stage 2
  bucket tests (10 cm range, near-field at 200/235 kHz) target
  the looser **±20 cm bottom-pick** acceptance — near-field
  ringing / phase variation expected and not a failure. See P2
  §2.7 and P1 §1.7.
- Chain receives `AcousticDepth` records continuously at the chosen
  cadence with valid temperature.
- 1.8 kHz mesh detections on this buoy are within 1 dB SNR of those
  on a sibling fleet buoy.
- No visible electrical interference between the pulser and the
  1.8 kHz analog front end on a captured scope trace.
- **Cavitation-noise baseline measured at 50 Vpp default drive**
  (P2 §2.8): RX-side broadband-noise (10–500 kHz integrated)
  recorded during a 100-ping pool calibration run; this baseline
  is stored as the cavitation-monitor reference. Subsequent drive
  levels (e.g., enhanced 80 Vpp) are compared against this
  baseline with a **≥10 dB rise** flagged as cavitation onset.
  Stage 3 pool-baseline measurement is the formal exit-criterion
  test for the cavitation monitor.

## Open questions / risks

- **Connector pinout** (see Appendix A) — must be identified before
  energizing. Mis-wiring a P79 to a 50 V rail is a $35 mistake.
- **Pool near-field**: 200 kHz at 2 m depth is borderline near-field
  for an Airmar puck. Bottom return may be the only target with
  consistent geometry. Open-water testing is where 200 kHz really
  shines.
- **WiFi noise** coupling into the pulser rail and back into the
  1.8 kHz analog front end. Mitigation: star ground at the battery
  terminals; separate analog and digital ground returns until the
  star point.
- **Battery sag during ping** — peak pulser current can hit a few
  amps for tens of µs. Local bulk cap (the 100 µF on the pulser
  rail) should handle it; verify under scope.
- **Pulser failure mode**: if a MOSFET shoots through, the rail
  collapses. Add a 1 A polyfuse on the boosted rail.
- **Couplant chamber leak**: oil-filled chamber loses oil and
  acoustic coupling degrades. Pressure-test the chamber overnight
  before pool deployment.

## Schedule

- **Day 0** (today, 2026-05-11): order P79. Order pulser + LNA + ADC
  parts in the same DigiKey order.
- **+1–2 weeks**: P79 arrives; identify pinout; characterize
  electrical impedance per band; write Appendix A.
- **+2–3 weeks**: hand-assemble pulser + LNA + ADC daughterboard on
  perfboard. Stage 1 bench validation.
- **+3 weeks**: Stage 2 bucket test.
- **+4 weeks**: mechanical buoy build (couplant chamber + buoy
  integration). Stage 3 + 4 pool deployment.
- Plane work item to be opened once the P79 arrives and Appendix A
  is filled in (the part-number details are the load-bearing input).

## 3D-printed components (Bambu P1S)

This buoy is the most print-intensive of the family, especially
once the Phase 5d gimbal lands. The matrix below is the single
source of truth for which part is printed in which material.

| Printed part                                  | Phase | Material      | Print params               | Why                                                  |
|-----------------------------------------------|-------|---------------|----------------------------|------------------------------------------------------|
| **Pulser daughterboard chassis / mount frame** | 5a    | PETG          | 100% infill, 4 walls       | Dry-side, mechanical only. Holds the perfboard rigid under the S3. |
| **P79 couplant-chamber retainer + foam ring** | 5a    | PETG          | 100% infill, 4 walls       | Spring-loads the puck face against the bottom cap. Oil-immersed; PETG holds up. |
| **235 kHz D couplant-chamber retainer**       | 5a    | PETG          | 100% infill, 4 walls       | Same as above for the second chamber.                 |
| **Cable comb / strain-relief tree**           | 5a    | PLA + Plasti-Dip | 50% infill, 3 walls    | Inside the dry electronics chamber; organizes the imaging-tier cables. |
| **Connector terminal-block carrier**          | 5a    | PETG          | 100% infill, 4 walls       | Mounts the marine terminal block where OEM cables are spliced. |
| **Gimbal frame (Phase 5d, oil-immersed)**     | 5d    | **PA-CF**     | 100% infill, 5 walls, hardened nozzle, annealed 100 °C × 1 h | Holds the 235 kHz puck on the gimbal arm. Oil-immersed, needs dimensional stability under thermal cycling. |
| **Pitch-axis arm**                            | 5d    | PA-CF         | 100% infill, 5 walls       | Cantilevered load against gravity in oil; PA-CF for stiffness. |
| **BLDC motor mount (top-cap interior)**       | 5d    | PA-CF         | 100% infill, 5 walls       | Bolts the gimbal motor to the chamber top cap. Heat-set inserts for the motor mount screws. |
| **Magnetic-encoder rotor magnet holder**      | 5d    | PETG-CF       | 100% infill, 4 walls       | Holds the diametrically-magnetized rotor magnet on the gimbal shaft 1–3 mm above the AS5048A chip. |
| **Encoder PCB mount**                         | 5d    | PETG          | 100% infill, 4 walls       | Holds the AS5048A breakout in the chamber.            |
| **Urethane-dome casting mold (Phase 5e)**     | 5e    | **PETG, 0.1 mm layers** | 100% infill, 6 walls, ironing on the interior surface | Single-use casting mold for the PR-1547 / Sorta-Clear pour. Print's interior surface becomes the dome's exterior surface — polish carefully before casting. |
| **Threaded "lighthouse" dome housing (Phase 5f)** | 5f    | PA-CF         | 100% infill, 6 walls       | Production-form-factor threaded dome carrier. Heat-set inserts for the dome retention screws. |
| **Antenna mast clamps**                       | 5a    | ASA           | 100% infill, 4 walls       | Above-waterline, UV exposure. ASA over PETG.          |

### Coating strategy for this buoy

- **Plasti-Dip on the dry-side PLA parts** — same as the other
  builds. Cheap insurance.
- **Marine epoxy spray on the quadrant / chamber clamps** that ride
  the buoy exterior — harder finish for the abrasion zone.
- **PA-CF parts get annealed**, not coated. Annealing in the P1S
  enclosure at 100 °C for 1 hour reduces water uptake significantly;
  Plasti-Dip on PA-CF tends to delaminate in oil and is not
  recommended for the gimbal frame.
- **Conformal coat the daughterboard** before final assembly — the
  pulser rail will spray switching transients all over the analog
  side and a conformal coat (MG Chemicals 422) catches the
  occasional droplet of oil that finds its way past the chamber
  seals.

### Disposability and per-buoy print cost

A full print set for this buoy (all phases through 5d) is roughly
~120 g of filament across PA-CF, PETG, PLA, ASA, and TPU — **~$8
of filament** at retail prices ($80/kg PA-CF, $25/kg PETG, $20/kg
PLA, $30/kg ASA, $25/kg TPU). The dome-mold print is single-use
and ~10 g of PETG. Cost is dominated by the transducers and the
gimbal motor, not the plastic.

A lost P79 buoy is a ~$400 incident even at production-build
prices. The print cost is in the noise.

### Heat-set inserts and threaded interfaces

Every screw-in interface on a printed part uses **M3 brass
heat-set inserts** ($0.05 each, install with a soldering iron).
This matters most on:

- The BLDC motor mount (axial load + rotational shock)
- The dome retention screws (Phase 5f)
- The daughterboard chassis (repeated service access)

Printed threads strip in 10–20 cycles; brass inserts last for the
life of the print.

## Path from experimental to production

This build deliberately uses two surplus transducers — a P79 and a
2004-vintage 235 kHz D — because the goal of *this* buoy is to
prove out the imaging-tier pipeline end-to-end before committing to
fleet-scale parts orders. The right framing is:

| Phase                  | Transducers                              | Buoy count | Purpose                                            |
|------------------------|------------------------------------------|------------|----------------------------------------------------|
| **5a (this doc)**      | P79 (surplus) + 235 kHz D (2004 surplus) | 1          | Prove pipeline, identify all design problems       |
| **5b** (after 5a pass) | Modern Airmar (e.g. B45 / P19 / B260)    | 1          | Validate against a clean, in-spec, current part    |
| **5c** (fleet rollout) | Same modern part × 3                     | 3          | Retrofit the v1 pool fleet with imaging tier       |
| **5d** (stretch)       | Add gimbal to the 235 kHz channel        | 1 then 3   | Mechanical scanning → FLS + 360° PPI from single element |

The 2004 235 kHz D will not be the part that ships in the
production fleet — its element may have aged drift, its cable
insulation may be brittle, and we have no calibration data for it.
But it is *more than good enough* to discover what the firmware,
daughterboard layout, mounting chamber design, and chain-stream
schema actually need to look like. **A $35 mistake is a cheap
education.**

## Phase 5d upgrade path — gimbal-scanned imaging

Once the static dual-transducer build is validated, the natural
next step is mounting the 235 kHz unit on a gimbal. This is the
move that brings whole new commercial sonar product categories —
forward-looking sonar, single-element 360° PPI, swept bathymetry —
into the family of things this buoy can do, with no change to the
transducer or the daughterboard.

### What a gimbal unlocks

- **Forward-looking sonar (FLS) mode** — tilt the 235 kHz beam from
  vertical-down to horizontal-forward. Garmin LiveScope mental
  model from one transducer.
- **Mechanical-scan 360° PPI** — full pan + tilt, every ping
  aimed at a specific `(bearing, elevation)`, accumulate over a
  scan to build a polar-coordinate image. This is exactly how
  Tritech Micron-DST, Imagenex 881A, BlueView P900 work today.
- **Bottom-profile sweep** — fan through pitch angles, get a
  multi-beam-like swath of depth picks under the buoy.
- **Tilt-stabilized down-look** — IMU on the buoy senses
  roll/pitch, gimbal counter-rotates, beam stays truly vertical
  in waves.
- **Adaptive pointing** — shore service detects a contact at
  some bearing, commands the gimbal to dwell on it.

This collapses several "out-of-family" verdicts in
`commercial-comparison.md` §3 into "in-family with one degree of
freedom added".

### Mechanical decision tree

Four viable paths. Path D (added 2026-05-11) is the recommended
approach and supersedes Path C; Paths A–C are kept here for the
record of what was considered.

| Path | Mechanism | Cost | Durability | Notes |
|------|-----------|------|------------|-------|
| **A. Oil-submerged hobby servo** | MG996R-class digital servo inside the 2" couplant chamber, linkage to the puck. | ~$10 + linkage | Weeks–months | Oil eventually kills the pot/encoder. Considered, rejected. |
| **B. Dry-side stepper + sealed shaft** | NEMA-17 stepper inside the sealed electronics section, drive shaft through a marine rotary seal into the oil chamber. | ~$50 incl. seal | Years | Standard ROV/AUV approach. Seal is a maintenance item. Considered, rejected. |
| **C. Magnetic coupling** | Diametrically-magnetized NdFeB pair: drive magnet inside the dry electronics section, follower magnet on an external gimbal carrying the puck. No shaft penetration. | ~$45 | Indefinite | Used in dive lights. Considered, rejected in favor of D. |
| **D. Fully-submerged BLDC gimbal in 4" oil chamber** | A widened (3" or 4") fully-oil-filled chamber. Brushless gimbal motor (no brushes, no pot) bolted to the top cap, gimbal arm carrying the puck, magnetic absolute encoder for closed-loop position. The entire mechanism — motor, encoder, gimbal, puck — lives in oil. Driver IC stays dry-side on the daughterboard. | ~$30 | Indefinite | **Recommended.** No moving seal anywhere. Acoustic window is the PVC wall in every direction the puck can point. |

**Path D** is the right answer for this buoy. Three properties make it
dominate the other three:

1. **No moving seal exists.** The only penetration is the cable
   gland at the top cap, which is static. There is nothing to leak.
2. **The motor technology stops being a constraint.** A BLDC gimbal
   motor has no brushes, no commutator, no pot, no contact mechanism
   that oil degrades. Magnetic encoders (Hall-effect) work through oil
   trivially. Oil immersion is *neutral or beneficial* (cooler running,
   no dust, no corrosion) over the motor lifetime.
3. **The acoustic geometry becomes omnidirectional through the PVC
   wall.** With no air gap anywhere in the chamber, the transducer's
   beam exits through whichever wall section it is pointed at on a
   given ping. The PVC wall acts as the acoustic window in every
   direction — at 235 kHz a typical 6 mm PVC wall sits near λ/2 in the
   wall material (sound speed in rigid PVC ≈ 2400 m/s → λ ≈ 10 mm), a
   transmission maximum. The buoy becomes a full-sphere-capable
   acoustic node at the imaging band, limited only by where the gimbal
   physically points the puck.

### Path D mechanical layout

```
                 (electronics section, sealed above)
─── threaded coupler with O-ring + grease ───────────
│   cable gland for BLDC + encoder + transducer feeds │
├────────────────────────────────────────────────────┤
│                                                    │
│   GIMBAL CHAMBER — 3" or 4" PVC, oil-filled        │
│   ────────────────────────────────────────         │
│                                                    │
│    [ BLDC gimbal motor ]  ← bolted to top cap      │
│            │                                       │
│            ▼ shaft                                 │
│    [ pitch gimbal frame ]                          │
│            │                                       │
│            ▼                                       │
│    [ 235 kHz puck ] ← swings through pitch arc     │
│    [ AS5048A encoder ] ← reads pitch angle         │
│                                                    │
│   Optional second motor on yaw axis for full       │
│   2-DOF pan+tilt.                                  │
│                                                    │
│   Oil completely fills the chamber. No air gap.    │
│   The PVC wall in every radial direction is the    │
│   acoustic window at 235 kHz.                      │
│                                                    │
├────────────────────────────────────────────────────┤
│   (P79 chamber stays here, separate 2" section,    │
│    fixed-down, unchanged.)                         │
├────────────────────────────────────────────────────┤
│   Vented ballast section (unchanged)               │
                  (open to water below)
```

### Path D BOM additions (over the static build)

| Part                                            | Qty | Approx. cost |
|-------------------------------------------------|-----|--------------|
| 3" or 4" PVC section, ~30 cm                    | 1   | $5           |
| 3" or 4" PVC caps (top: drilled for gland; bottom: solid) | 2 | $4 |
| Cable gland, IP68, suitable for 6-conductor cable | 1 | $4           |
| Additional mineral oil (~1.5–2.5 L)             | —   | $5           |
| BLDC gimbal motor (GBM2208, GM2804 class)       | 1   | $15          |
| AS5048A magnetic absolute encoder + magnet      | 1   | $3           |
| DRV8313 or TMC2208 3-phase driver IC            | 1   | $3           |
| Gimbal frame parts (3D-printed PETG or Delrin)  | 1 set | $3         |
| M3 stainless hardware                           | —   | $2           |
| **Path D subtotal**                             |     | **~$44**     |

### Buoyancy budget impact (real)

Confirmed by P2 §2.3
(`.planning/symposiums/sonobuoy/panels/P2-build-mechanical.md`):

- 4" Sch 40 PVC, ID 10.16 cm, 30 cm tall → oil volume **2430 cm³
  ≈ 2.07 kg of oil at ρ=0.85**; with the 5% air headspace and the
  cumulative buoy mass-budget tolerance, **~2.5 kg total oil mass
  per Phase 5d chamber** is the correct working number.
- Phase 5d buoy mass budget (P2 §2.1): static dry mass ~3.2 kg,
  sealed top section + oil-lift contribution ~660 g → **net
  buoyancy deficit of ~2.1 kg** without a foam collar. With a
  6 cm × 30 cm closed-cell foam pipe-insulation collar
  (density ~30 kg/m³, volume ~2.5 L), foam-collar lift is **~3 kg**
  — restores positive buoyancy by ~0.9 kg of net margin.

Two practical options:

- **Stack a foam float collar around the 4" section** — ~$5 of
  closed-cell foam, ~150 g collar mass, ~3 kg of net lift. Cheap,
  unsealed, lives outside the pressure boundary. **Recommended.**
- **Use a 3" gimbal chamber instead of 4"** — 3" Sch 40 ID is
  7.79 cm; oil mass halves to ~1.4 kg; foam-collar requirement
  halves to ~1.5 kg. Pitch-arm clearance: r_arm_max = 2.10 cm in
  3", still allows ±90° pitch with a shorter arm (235 kHz beam
  is 10° narrow so slight asymmetry is acoustically negligible).
  **P2 §2.3 recommends 3" as the first Phase 5d build**, falling
  back to 4" only if arc clearance proves wrong on fabrication.

Additionally, P2 §2.3 flags a **slosh-baffle requirement** in the
4" chamber: the 5% gas headspace sets up a ~2 Hz sloshing mode
that modulates the gimbal pitch axis. Mitigation is an internal
PA-CF disc with a 5 mm bleed hole at the chamber midpoint
(added to the 3D-print matrix at P2 §2.4 row #33).

Measure on a single test float before fleet rollout. Roll-period
target: 5 s < T_roll < 10 s for gimbal-servo compatibility (P2
§2.3 derivation: T_roll ≈ 7.6 s on the modeled Phase 5d geometry).
Until that measurement exists, the buoyancy budget is the riskiest
part of the Path D upgrade.

### Firmware additions for the gimbal

Once Path C lands:

- New chain stream `acoustic.gimbal_state` — gimbal angle and
  velocity per timestamped sample.
- New gimbal driver task on the S3 — drives the stepper, reads
  back step counts, optionally closes a loop against an
  AS5600-style magnetic absolute encoder ($3) glued under the
  external gimbal arm.
- `acoustic.imaging` records carry `(bearing_deg, elevation_deg)`
  alongside `band_khz` and the sample window.
- New operating modes: **G — sector scan** (sweep through a
  bearing/elevation arc, ping at each step), **H — track**
  (point at a bearing fed in over chain from the shore service).
- IMU integration (a $2 MPU-6050 or LSM6DSM on the daughterboard)
  for tilt-stabilization in mode G.

### What does *not* change

The daughterboard, the pulser, the LNA, the ADC, the chain stream
schemas (other than two new fields and one new stream), the
1.8 kHz mesh path, and the couplant-chamber design for the P79.
The gimbal is a strict superset of the static build.

### Pre-condition

**Do not build the gimbal until the static build passes Stage 4.**
A gimballed transducer pointed in the wrong direction tells you
nothing about whether the bottom-pick algorithm works; a static
transducer pointed straight down with a known target *is* the
debug fixture. Build static, validate, then add motion.

## Plane / work tracking

This buoy is not yet a Plane item. Once Appendix A is filled in
(transducer in hand), open `WEFT-N`-style items for:

- `Imaging daughterboard schematic + perfboard layout (3-band)`
- `Dual couplant-chamber mechanical drawing`
- `S3 firmware: pulser driver + ADC capture path`
- `S3 firmware: matched filter + bottom-pick`
- `Chain stream: AcousticDepth schema`
- `Chain stream: AcousticImaging schema`
- `Stage 1–4 bench/pool validation runs`
- `Phase 5b: source and characterize a modern Airmar replacement`
- `Phase 5d (stretch): gimbal mount per Path C, magnetic coupling`

All in Plane cycle `0.7.x` if we want this in the next must-ship
release; otherwise `0.8.x` per project policy.

## Appendix A: connector identification (fill in on receipt)

The 31-234-2-01 part number ships with one of several OEM
connectors (Garmin 6-pin / Lowrance Blue 7-pin / Furuno 10-pin /
Humminbird / bare wires) depending on the surplus channel. We
will cut the OEM connector and solder direct, but the conductor
mapping inside the cable must be identified first.

On receipt:

1. **Photograph** the OEM connector and the full cable, both ends.
2. **Strip 5 cm** of outer jacket back from the *transducer* end
   of the cable (the end you will keep) to expose the inner
   conductors and shield.
3. **Measure DC resistance** between each unique conductor pair.
   Record below. The piezo elements should read open (high
   impedance); the thermistor pair should read ~10 kΩ at room
   temp.
4. **Measure AC impedance** with a signal generator + scope around
   50 kHz and 200 kHz. The element pairs will show a resonance
   dip in current at their respective bands.
5. **Identify shield** — should be braid / outer foil and connect
   to the cable's outer drain wire. This becomes the analog ground
   return.

Conductor map (fill in):

| Wire colour | Function (50 kHz / 200 kHz / thermistor / shield) | DC R | Notes |
|-------------|--------------------------------------------------|------|-------|
|             |                                                  |      |       |
|             |                                                  |      |       |
|             |                                                  |      |       |
|             |                                                  |      |       |
|             |                                                  |      |       |
|             |                                                  |      |       |
|             |                                                  |      |       |

**Once filled in, this appendix is the source of truth for the
daughterboard connector wiring.** Update the schematic to match.

## Appendix B: 235 kHz D connector identification (fill in on receipt)

Confirmed from the eBay listing label photograph (2026-05-11):

| Field                | Value                                       |
|----------------------|---------------------------------------------|
| Manufacturer         | Airmar                                      |
| Marketing name       | "235 kHz - D"                               |
| MFR P/N              | 44-053-1-02 Rev A                           |
| Customer P/N         | 10000843                                    |
| Date of manufacture  | 01/04 (January 2004)                        |
| Patent reference     | US 4,961,178 B1                             |

To verify on physical receipt:

1. **Photograph** the connector and cable end-to-end before doing
   anything else.
2. **Mount style** — confirm whether the body is in-hull
   (shoot-through puck), transom-mount (bracket + tilt screws),
   or thru-hull (threaded barrel with nut). Affects the couplant
   chamber design — an in-hull puck drops straight in; a transom
   unit needs its bracket removed and the puck face mounted
   against the chamber wall.
3. **Strip 5 cm** of outer jacket from the transducer end of the
   cable.
4. **Measure DC resistance** between each unique conductor pair.
   For a depth-only "D" unit, we expect:
   - One conductor pair → piezo element (open at DC, ~30 Ω
     resonance at 235 kHz under signal-generator drive).
   - Shield → outer braid, becomes analog ground.
   - **No thermistor pair expected** (the "D" suffix means
     depth-only). If a third conductor pair is present and reads
     ~10 kΩ, it is a thermistor and the "D" designation is wrong
     — flag and re-document.
5. **AC resonance check**: drive across the element pair with a
   function generator (1 Vpp into 50 Ω source, ramp 200–270 kHz)
   and watch current with a 1 Ω sense resistor on a scope. A
   resonance dip in current near 235 kHz confirms the element is
   tuned and intact. Absence of a sharp resonance is a sign the
   element is cracked or de-poled — at 21 years old this is a
   real risk.

Conductor map (fill in):

| Wire colour | Function (235 kHz element / shield / thermistor?) | DC R | AC behaviour | Notes |
|-------------|---------------------------------------------------|------|--------------|-------|
|             |                                                   |      |              |       |
|             |                                                   |      |              |       |
|             |                                                   |      |              |       |

If the element fails the AC resonance check, the unit becomes a
mechanical/pinout study specimen rather than a usable transducer
— still useful, but the imaging-tier 235 kHz validation slips
until Phase 5b (modern part).
