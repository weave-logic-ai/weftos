# Sonar Buoy — Initial Requirements (v1, pool fleet)

**Scope**: Three buoys, swimming-pool deployment, presence + ranging.
**Status**: Hardware committed; firmware not yet written.

## Functional goals (in priority order)

1. **Presence** — each buoy detects every other buoy and reports
   "I see node N" as a stream event. This is the v1 milestone.
2. **Ranging** — pairwise distance estimates between buoys via
   two-way travel time (TWTT). Streamed as raw timing events;
   trilateration runs on shore.
3. **Sensor telemetry** — water temperature at the receiver depth,
   battery voltage, RSSI/SNR for each acoustic detection.
4. **Above-water gossip** — WiFi / ESP-NOW links between surface
   sections so shore can pull data without a wired bridge.

## Hardware spec (locked)

### Piezo transducers (TX and RX, identical part)

- Resonant frequency: **1.8 ± 0.3 kHz**
- Resonant impedance: **≤ 300 Ω**
- Static capacitance: **50 000 pF ± 30%** (≈ 50 nF)
- External diameter: 35 mm (1.38")
- Inside diameter: 24 mm (0.94") — annular ring, brass + ceramic
- Lead length: 10 cm

Implications:

- **Audio band**, not ultrasonic. Loud and audible in air.
- Q ≈ 6, usable bandwidth ≈ **300 Hz**, modulation rate **50–200 bps**
  with FSK or chirp + FEC.
- Wavelength in water (c ≈ 1480 m/s) ≈ **82 cm**.
- Disc diameter ≪ wavelength → omnidirectional point source. No
  beam-forming from a single element.
- 1.8 kHz attenuation in water is negligible (≈ 0.05 dB/km in
  seawater). Spherical spreading dominates the link budget.

### Compute (per full buoy)

| Role | Part | Notes |
|------|------|-------|
| TX MCU | **ESP32-S2 mini** | Modulator state machine + H-bridge GPIOs only. Single core, no DSP. |
| RX MCU | **ESP32-S3 mini** (N8R8 — 8 MB flash + 8 MB PSRAM) | Acoustic DSP, matched filter, demodulation, sensor I/O. |
| Radio MCU (surface buoys only) | **ESP32-S2 or S3 mini** | WiFi / ESP-NOW only. Optional — can collapse into RX MCU if WiFi noise tolerated. |

Submerged-only nodes (if any): RX MCU + TX MCU only. No radio MCU.

### Inter-MCU link

- **UART** at 115 200 baud between TX, RX, (and optional radio) MCUs.
- **TX_ACTIVE GPIO**: TX MCU asserts during transmission. RX MCU
  uses it to gate the analog front end and to timestamp
  start-of-symbol for self-loopback calibration.
- Optional: a shared symbol-clock GPIO if precision ranging needs
  better than UART-jitter timing.

### Analog signal chain (RX)

The RX path has three packaging options, gated by build phase:

- **Phase 1 (single buoy validation)**: bare 35 mm piezo discs
  mounted against the inside of the 2" PVC wall with silicone
  grease as acoustic coupler. Both piezos and both ESP32-S3 minis
  live in one dry chamber. See
  [`build-hydrophone-simple.md`](build-hydrophone-simple.md).
- **Phase 1b — epoxy option (A)**: JFET source-follower hydrophone
  potted in a small PVC slip cap, hard-mounted to the buoy. ~20 dB
  SNR improvement over the simple build. Permanent, unserviceable.
  See [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md).
- **Phase 1b — oil option (B, recommended)**: JFET source-follower
  in an oil-filled 1.5" sidecar chamber clamped to the outside of
  the ballast and connected via M8 IP67 waterproof connector.
  ~25 dB improvement plus near-perfect acoustic match. Swappable,
  expandable to multi-RX and sensor sidecars. See
  [`build-hydrophone-oil.md`](build-hydrophone-oil.md).

Pick by serviceability needs and fleet ambition. The downstream
BPF chain in the dry electronics chamber is identical for all
three options — only the transducer packaging differs.

The downstream chain (identical for both phases):

- AC-coupled non-inverting two-stage op-amp, total gain ~60 dB.
  Suggested part: **MCP6022** dual op-amp.
- Active band-pass filter centered at 1.8 kHz, Q ≈ 4 (Sallen-Key
  or active twin-T).
- Mid-rail bias (1.65 V from a divider).
- Sample on the S3's internal SAR ADC at ~16 kS/s, or use I²S DMA
  mode for cleaner capture. Fallback: external **ADS1115** if SAR
  noise is unacceptable.

### Driver (TX)

- **DRV8837** H-bridge driven antiphase from two GPIOs → 6.6 Vpp
  across the disc (logic-rail).
- Optional boost: **TPS61023** to 12 V rail for ~24 Vpp when range
  matters.
- TVS diodes across the disc (piezos generate flyback when
  mechanically struck).
- *Avoid* Class D audio amps (PAM8302 etc.); they are unstable
  into capacitive loads.

### Pulser drive safety (imaging-tier buoys; P79 + 235 kHz D)

For the half-bridge MOSFET pulser used on the Phase 5+ imaging
buoy (`build-buoy-p79.md`), pulser peak voltage is **safety-locked
against cavitation**:

- **Default: 50 Vpp.** Produces ≈ 80 kPa peak pressure at 1 m; the
  cavitation threshold at 2 m pool depth is ≈ 120 kPa (P_atm +
  ρgh = 100 + 2 × 9.8). 50 Vpp is below threshold by a factor of
  1.5×.
- **Enhanced: 80 Vpp**, allowed **only** with firmware cavitation
  monitoring active. 80 Vpp produces ≈ 130 kPa — AT THRESHOLD at
  2 m depth. The cavitation monitor watches RX-side broadband
  noise during/after the TX pulse; a ≥10 dB rise reverts drive to
  default.
- **Forbidden: ≥100 Vpp** in this build. 100 Vpp produces
  ≈ 160 kPa, cavitates outright at any pool depth shallower than
  ~6 m hydrostatic, and produces broadband noise that defeats
  matched-filter detection.

References: P1 §1.9 cavitation analysis
(`.planning/symposiums/sonobuoy/panels/P1-acoustic-physics.md`),
P2 §2.8 pulser drive safety lock
(`.planning/symposiums/sonobuoy/panels/P2-build-mechanical.md`).
Apfel & Holland 1991 *Ultrasound Med. Biol.* 17:179 + Urick §3.3
are the canonical sources.

### Sensors

- **DS18B20** waterproof 1-Wire temperature probe at the receiver
  depth. Sound-speed correction is load-bearing for accurate
  ranging — temperature is not optional.
- Battery voltage divider into an ADC channel.
- Optional: **TMP117** I²C if ±0.1 °C lab-grade is needed later.

### Power

- 1S Li-ion (18650 or LiPo) per buoy.
- **TP4056** charging IC.
- **AP2112** 3.3 V LDO.
- Estimated current budget:
  - RX MCU continuous listening: ~80 mA
  - TX MCU light-sleep average: ~1–5 mA
  - Radio MCU duty-cycled WiFi: ~50 mA average
  - Analog chain: ~5 mA
  - Total surface-buoy average: ~140 mA
  - 3000 mAh 18650 → ~21 hours continuous, several days with
    heavier RX duty-cycling. To be measured.

## Mechanical form factor

- **2" PVC pipe** vertical buoy.
- **Sealed top section** with foam packing for buoyancy. Houses
  electronics, antenna mast, and (above the seal) any GPS module.
- **Open ballast section** below the seal, fills with water through
  bottom; oriented vertically by gravity. **Vent holes drilled
  ~1 cm below the rim of the ballast section** so trapped air
  escapes (no buoyant ballast) but holes stay above the maximum
  waterline of the ballast (no gurgling acoustic noise).
- **Optional stacked lower unit** via another 2" coupler below the
  ballast — gives a second receiver position with a known vertical
  baseline. Lower unit must not shadow ballast vents.
- **Antenna mast** above the seal via a **threaded PVC coupler with
  O-ring** + dielectric grease. Single sealed cable gland for the
  antenna feedline. Glued joints crack in sun; threaded couplers
  with O-rings are the durable answer.
- **Thermometer** mounted at the receiver depth, inside the open
  ballast section so it reads the water column the acoustic signal
  is travelling through.

## v1 fleet

- **Phase 1**: one buoy, bare-piezo RX, S2 TX MCU + S3 RX MCU,
  battery, WiFi antenna on mast, temp probe. Built first to validate
  the architecture on real hardware before committing to the fleet.
- **Phase 1b**: build out two more buoys with JFET hydrophone RX
  from the start, and retrofit Buoy 1 with a JFET hydrophone. End
  state: **three identical JFET-equipped buoys** for pool deployment.
- Each fleet buoy: 1× TX disc, 1× JFET hydrophone (RX), 1× S2, 1× S3,
  1× temp probe, 1× battery, 1× WiFi antenna on mast.
- One buoy may carry a *second* JFET hydrophone + S3 stack as a
  stretch goal, to validate in-buoy TDoA bearing.

## Out-of-scope for v1

- GPS modules (no sky view in pool; defer to open-water phase).
- LoRa 915 MHz (defer to open-water phase).
- Marine VHF / AIS (licensed band; not a hobby fit).
- Ultrasonic 40 kHz secondary band for fine ranging (deferred;
  see roadmap).
- Multi-RX-per-buoy beam-forming (beyond the single stretch buoy).
- Solar charging.
