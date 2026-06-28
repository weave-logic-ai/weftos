# Sonar Buoy — Simple Build (Phase 1 test unit)

**Status**: Phase 1 — first buoy, fastest to build, designed to
validate the architecture before committing to a fleet build.
**Build time**: ~2 hours including PVC cutting and cable runs.
**Cost**: ~$0 if parts on hand; ~$45 if buying from scratch.

This is the simplest possible package: **one dry chamber** containing
both ESP32-S3 mini boards (TX and RX firmware), both piezo discs
(mounted against the inner PVC wall as acoustic windows), the
analog signal chain, battery, and antenna mast on top. No oil. No
epoxy potting. No sidecars. No waterproof connectors. Get a working
buoy in the pool the same day the parts arrive.

When you've measured baseline SNR from this build in the pool, you
graduate to either [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md)
or [`build-hydrophone-oil.md`](build-hydrophone-oil.md) for the fleet.

## Concept

```text
              ╔════════════╗
              ║  antenna   ║   ← whip antenna on threaded mast
              ║   mast     ║
              ╚════╦═══════╝
   ┌──────────────╨──────────────┐
   │  ┌──────────────────┐       │  ← threaded top cap (O-ring),
   │  │  S3 TX board     │       │     removable for service
   │  │                  │       │
   │  │  S3 RX board     │       │
   │  │                  │       │     ALL ELECTRONICS DRY
   │  │  DRV8837 H-bridge│       │
   │  │  MCP6022 BPF     │       │
   │  │  TP4056 / LDO    │       │
   │  │  18650 cell      │       │
   │  └──────────────────┘       │
   │                              │
   │ ◀── TX piezo (pressed against│  ← piezos mounted to INSIDE of
   │     inside PVC wall, sound   │     2" PVC wall with silicone
   │     radiates out through wall)│     adhesive / thermal grease.
   │                              │     PVC wall is the acoustic
   │                              │     window. ~2 dB transmission
   │ ◀── RX piezo (same trick,   │     loss at 1.8 kHz — fine for
   │     opposite side of pipe)   │     validation.
   │                              │
   │  ┌──────────┐                │  ← DS18B20 temp probe on
   │  │ DS18B20  │                │     short tether, lives in
   │  │ on tail  │                │     ballast / flooded section
   │  └─────╪────┘                │
   │        │                     │
   │ ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒│  ← internal sealed bulkhead
   │        ↓                     │     (epoxy + cable gland for
   │                              │     the temp probe wire)
   │  open vented ballast section │
   │  ▒▒▒ FLOODED WHEN DEPLOYED ▒▒│
   │                              │
   │       vent holes →           │  ← air escapes here
   │                              │
   └──────────────────────────────┘
              ↑ open bottom (water enters)
```

Two MCUs both ESP32-S3 mini — same board, same toolchain, same
firmware tree with a role flag (`tx` vs `rx`) selected by GPIO
strap or build config. Skip the S2 entirely for this unit. The S2
becomes interesting only when you're optimizing for fleet cost in
Phase 1b.

## Per-unit BOM

| Qty | Part | ~Price |
|-----|------|--------|
| 1 | 2" PVC pipe, ~30 cm length cut from 10' stock | $1.50 |
| 1 | 2" threaded cleanout adapter + plug (top, serviceable) | $5 |
| 1 | 2" flat test cap or end cap (bottom, vented) | $2 |
| 1 | PVC primer + solvent cement (bottom cap only) | $5 amortized |
| 2 | 35 mm piezo discs (existing parts) | $0 |
| 2 | ESP32-S3 mini boards (N8R8 — 8 MB flash + 8 MB PSRAM) | $20 |
| 1 | DRV8837 H-bridge breakout | $2 |
| 1 | MCP6022 dual op-amp | $1 |
| 1 | Resistors and caps for BPF (Sallen-Key, twin-T, or active) | $1 |
| 1 | DS18B20 waterproof temp probe | $4 |
| 1 | 18650 cell + holder + TP4056 + AP2112 LDO | $8 |
| 1 | WiFi whip antenna or chip antenna on flying lead | $2 |
| 1 | Small PVC mast (½" pipe coupler + ~10 cm of ½" pipe) | $2 |
| 1 | Cable gland for the temp probe pass-through | $1 |
| ~10 cm³ | Silicone adhesive / thermal grease for piezo-to-PVC bond | $1 |
|   | **Total if buying** | **~$45** |

## Assembly steps

**1. Prep the PVC.**
- Cut 30 cm of 2" PVC. Sand the cut ends.
- Drill 4 vent holes (~5 mm) around the lower 5 cm of the pipe for
  the ballast section air escape.
- Drill cable gland hole near the top of the ballast section for
  the temp probe pigtail.

**2. Bond the bottom cap.**
- Apply PVC primer to the inside of the test cap and the outside
  of the pipe's lower end.
- Apply solvent cement to both. Slip on, twist ¼ turn, hold 30 sec.
- Cure 24 hours before the buoy goes in water.

**3. Build the internal bulkhead.**
- Cut a circle of plastic (or use a 2" slip cap with the bottom
  drilled out as a frame) to fit inside the pipe ~5 cm above the
  bottom vent holes.
- This separates the **dry electronics chamber (top)** from the
  **flooded ballast (bottom)**.
- Mount the temp probe wire through a cable gland in this bulkhead
  so the probe sits in the flooded section.
- Epoxy the bulkhead in place. Let cure 24 hours.

**4. Mount the piezos.**
- Inside the dry chamber, ~10 cm apart along the pipe length:
  - **TX piezo**: silicone-grease the brass side, press flat
    against the inside wall of the PVC pipe. Hold in place with
    a small wedge of foam against the opposite wall. Avoid
    trapping air bubbles between piezo and PVC — the grease is
    the acoustic coupler.
  - **RX piezo**: same trick, on the opposite side of the pipe
    from the TX piezo (180° apart). Maximizes the through-pipe
    distance and reduces direct mechanical coupling.
- Tin the piezo leads and bring them up to the board layout area.

**5. Wire up the electronics.**
- Mount both S3 boards, DRV8837, MCP6022 BPF, TP4056, LDO, battery
  on a small protoboard or perfboard inside the dry chamber.
- TX S3 → DRV8837 → TX piezo (antiphase GPIO pair drives the
  H-bridge inputs; H-bridge outputs to piezo).
- RX piezo → MCP6022 BPF chain → RX S3 ADC.
- UART crossover between TX S3 and RX S3 (TX_GPIOx ↔ RX_GPIOy and
  vice versa) on two GPIOs each side.
- One GPIO from TX S3 = TX_ACTIVE → input GPIO on RX S3.
- DS18B20 1-Wire on a free GPIO of the RX S3.
- WiFi antenna on the S3 with the better RF performance (typically
  whichever has the IPEX connector + external antenna; the chip
  antenna version is fine too).

**6. Install antenna mast.**
- Drill the top cap for a ½" PVC stub.
- Cement the stub into the cap, ~10 cm long.
- Run the antenna lead up through the stub, out the top.
- Optional: cap the mast with a small flat cap drilled for the
  antenna feedline, with silicone sealant for waterproofing.

**7. Seal and test.**
- Power on, verify both MCUs boot, WiFi connects, temp probe
  reads, piezos respond to tap (tap the outside of the PVC,
  watch the RX ADC trace).
- Thread on the top cap with PTFE tape + O-ring.
- Pool dry-run: float in a bathtub or kitchen sink first. Check
  for leaks. Verify ballast vents properly (no trapped air
  keeping it from standing vertical).

## Exit test (Phase 1 criterion)

1. Power on, deploy in pool.
2. TX S3 sends a single MFSK frame: `[preamble][node_id=1][seq=0][type=BEACON][crc16]`.
3. RX S3 receives via the through-pipe path *and* via the through-water
  reflected path. Demodulator should recover the frame from either.
4. Frame logged as an `AcousticEvent` to the chain over WiFi.
5. Repeat 1000 times over 10 minutes. Compute BER (bit error rate)
   and SNR for the through-pipe and through-water-reflection paths.
6. Record this number — it's the baseline against which Phase 1b's
   epoxy and oil builds are measured.

## Known limitations of the simple build

These are accepted for Phase 1; they motivate the Phase 1b
specialization choice.

- **PVC-wall acoustic loss**: ~2 dB transmission through one PVC
  wall thickness. Acceptable but not optimal.
- **Bulk acoustic coupling**: with both piezos in the same rigid
  PVC tube, **mechanical (hull-borne) coupling between TX and RX
  is severe**. The RX will hear the TX through the pipe wall
  itself, not just through water. Plan for substantial TX_ACTIVE
  blanking duration here. This is the main reason to upgrade to
  the epoxy or oil build for the fleet.
- **Single-MCU-failure = whole-buoy failure**: no modularity. If
  one S3 dies, you open the buoy and swap it.
- **No serviceable piezo**: silicone-grease bond will degrade if
  you open the buoy repeatedly. Don't.
- **One TX, one RX, no expansion**: cannot add a second receiver
  for in-buoy TDoA bearing without a major rebuild.

## When to graduate

Move to one of the specialized builds once:

- The Phase 1 exit criterion passes in the pool.
- You have baseline SNR numbers logged for the comparison.
- The Phase 1b parts have arrived per
  [`build-hydrophone-oil.md`](build-hydrophone-oil.md) (recommended)
  or [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md)
  ordering checklists.

## 3D-printed components (Bambu P1S)

Even the simple build benefits from a handful of printed parts.
All of these are inside the sealed-dry electronics chamber, so
**PLA is fine** for any of them — coated with one pass of
Plasti-Dip or marine epoxy spray if you want extra service life.
The disposability framing applies: every printed part below costs
under $1 of filament.

| Printed part                       | Material              | Print params                     | Why                                                  |
|------------------------------------|-----------------------|----------------------------------|------------------------------------------------------|
| Internal bulkhead (separates dry chamber from flooded ballast) | PLA + Plasti-Dip topcoat OR PETG | 100% infill, 4 walls; OD = pipe ID − 0.4 mm | Replaces "circle of plastic" in Assembly Step 3. Print to fit the actual pipe ID. |
| Protoboard / battery cage          | PLA                   | 25% infill, 3 walls               | Holds the S3 boards, MCP6022, TP4056, 18650 cell in a slide-in carriage that drops out the top cap for service. |
| Piezo-positioning fixture          | PLA                   | 100% infill, 3 walls              | Two pockets at known separation, holds piezos flat against the inside of the PVC wall during silicone-grease cure. Disposable, single-use fixture. |
| Antenna mast spacer / drip cap     | PETG (UV exposure)    | 100% infill, 4 walls              | Lives outside the seal. PETG over PLA because UV. |
| Vent-hole spider screen            | PETG                  | 100% infill, 2 walls, 0.4 mm holes printed in | Keeps pool debris out of the ballast vents. |
| Top-cap interior cable comb        | PLA                   | 25% infill, 3 walls               | Holds the antenna feedline + temp-probe pigtail apart inside the cap, no chafing on threads. |

### Coating strategy

- **Plasti-Dip** (~$10/can): two coats over any PLA part that
  could ever see water exposure. Cures in 30 min. Reaches "I lost
  this buoy and don't care" durability with PLA underneath.
- **Marine epoxy spray** (Krylon Fusion or West System): harder
  finish for abrasion zones. Use on the bulkhead and cage if the
  buoy will be opened/closed repeatedly.
- **Conformal coating** on the populated PCB: standard. MG
  Chemicals 422 acrylic or similar, $20 a bottle.

### Heat-set inserts

Use **M3 brass heat-set inserts** (~$0.05 each, install with a
soldering iron in 5 seconds) anywhere a screw threads into a
printed part. Stronger than printed threads and don't strip out
when you service the buoy.
