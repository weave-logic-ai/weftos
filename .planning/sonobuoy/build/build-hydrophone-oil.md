# Sonar Buoy — Oil-Filled Sidecar Build

**Status**: Phase 1b — option B (recommended). Modular,
serviceable, expandable. Designed for the three-buoy pool fleet
and forward-compatible with the open-water and multi-band roadmap.
**Per-sidecar cost**: ~$8 in parts.
**Build time**: ~1 hour per sidecar including oil fill.
**Improvement over Phase 1 simple build**: ~25 dB SNR, near-perfect
acoustic match to water, swappable on a dock.

This build packages each transducer (or sensor) in its own small
oil-filled "sidecar" tube that clamps to the outside of the buoy's
ballast section. Sidecars connect to the dry electronics chamber
via **waterproof connectors** on a pigtail from the bottom of the
dry chamber. Each sidecar is independently swappable; a dead unit
can be replaced in 10 minutes. A buoy's role (TX-only, TX+RX,
multi-RX, sensor-only) is determined by which sidecars are
installed.

The circuit (JFET source follower) is identical to the epoxy
build — see [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md#circuit-3-wire-jfet-source-follower-33-v).
Only the packaging differs.

## Overall buoy layout

```text
   │ Antenna │ ← whip antenna on threaded mast
   ├═════════┤
   │  Dry    │
   │  electr-│ ← S2 TX MCU, S3 RX MCU, MCP6022 BPF, DRV8837 H-bridge,
   │  onics  │   battery, charge controller, LDOs. All inside the
   │  chamber│   sealed top section. No transducer here.
   │         │
   ├─────────┤ ← internal bulkhead with 4× M8 IP67 bulkhead
   │ ╪╪ ╪╪╪╪ │   connectors (cable glands for pigtails). Each
   │ ▒▒ ▒▒▒▒ │   pigtail runs down through the flooded ballast
   │         │   and out the bottom.
   │ ballast │
   │ ▒▒▒▒▒▒▒ │
   │ ▒▒▒▒▒▒▒ │
   │ ▒▒▒▒▒▒▒ │
   │ ▒▒▒▒▒▒▒ │
   │ ▒▒▒▒▒▒▒ │ ← flooded, vented (water enters from open bottom,
   │ ▒▒▒▒▒▒▒ │   air escapes through vent holes near top of ballast)
   │ ▒▒▒▒▒▒▒ │
   │ ▒▒▒▒▒▒▒ │
   ├─────────┤
        │
        │ ← pigtails terminate in IP67 connectors at buoy bottom
        ↓
   [ S | S | S | S ]  ← 4 sidecar mounting positions in quadrants
   (TX, RX1, RX2,         around the bottom of the buoy. Each is
    sensor, etc.)         a separate 1.5" PVC oil chamber clamped
                          or 3D-printed-mounted to the ballast.
        │
        │ ← optional: stainless steel eye bolt at the very bottom
        │   for a depth tether
        │
        ─── paracord 1–3 m ───
        │
        ◆ ← optional weight (1–3 lb dive weight) and/or
            depth-stabilizer fin
```

## Sidecar mechanical design

Each sidecar is a small oil-filled cartridge with one transducer
or sensor inside, terminated in a waterproof connector.

```text
   Sidecar (looking through cutaway):

   ┌────────────────────┐  ← threaded service cap with O-ring +
   │ ▓ thread tape    ▓ │     PTFE tape. Removable for refill /
   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓     JFET swap. DO NOT cement this.
   ▓ ┌────┐             ▓
   ▓ │J201│ floating in ▓  ← JFET + bias resistors suspended in
   ▓ │+R's│ mineral oil ▓     mineral oil, held by cable strain.
   ▓ └─┬──┘             ▓     Air-gap top ~5–10% for thermal
   ▓   │                ▓     expansion of oil.
   ▓ ╔═══════════════╗  ▓
   ▓ ║ 27 mm piezo  ║   ▓
   ▓ ║ disc, brass  ║   ▓  ← piezo at the bottom, brass side facing
   ▓ ║ side DOWN    ║   ▓     the sound-window cap.
   ▓ ╚═══════════════╝  ▓
   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
   │ ▓ flat test cap, ▓ │  ← bottom cap is solvent-cemented
   │ ▓ permanently   ▓ │     PERMANENTLY (PVC primer + cement).
   │ ▓ bonded         ▓ │     This is the acoustic window.
   └────────────────────┘
              ▲
              │ sound entry (water side)
              │
   ╪╪ ← waterproof connector (M8 IP67 socket) at the side or top
        of the sidecar, with EPDM gland sealing the cable entry.
        Mates with the pigtail from the dry chamber.
```

Sidecar dimensions:

- 1.5" Sch 40 PVC pipe, ~50–80 mm long
- 1.5" flat test cap (bottom, solvent-cemented permanently)
- 1.5" threaded cleanout adapter + plug (top, removable)
- Total assembled length: ~80–100 mm
- Total assembled OD: ~48 mm
- Mass: ~80 g with oil

## Mounting to the buoy

Three mounting strategies, pick by what you can fabricate:

**(a) 3D-printed quadrant clamp** — recommended.
Print a clamshell or rail that wraps the 2" buoy ballast pipe and
holds 2 or 4 sidecars in slots around the perimeter. Sidecars
slide into the slots from below; a single stainless bolt or
zip-tie locks each one in place. Slide-out design means a swap
takes 30 seconds on a dock.

```text
   Cross-section through ballast at sidecar mount:

         ╔════════════╗
         ║   2" PVC   ║       ← buoy ballast pipe (centered)
         ║  ballast   ║
         ║   pipe     ║
         ╚════════════╝
        ┌─┘ ╪╪ │ ╪╪ └─┐
        │ S1│ │ │S2 │       ← two sidecars in opposing quadrants
        │   │ │ │   │            (or four, if you print a quadrant
        │TX │ │ │RX │            cross). Each clamped into a 3D-
        │   │ │ │   │            printed slot.
        └───┘ │ └───┘
              │
            3D-printed clamp ring grips the 2" PVC
            and provides slide-in slots for sidecars.
```

**(b) Stainless hose clamps + foam spacer** — no print required.
Wrap two stainless hose clamps around the ballast pipe + sidecar
sandwich. Foam spacer between sidecar and ballast prevents
mechanical coupling. Functional, looks rough.

**(c) Epoxy-bonded saddle** — permanent.
Glue the sidecar to a PVC saddle clamp (a commercial 2" pipe
saddle for adding a side connection), then clamp the saddle to
the ballast. Removable but slow.

For the v1 pool fleet, **(a)** is the right choice — print three
quadrant clamps, you can carry around a box of pre-built TX/RX/
sensor sidecars and reconfigure buoys on the fly.

## Sidecar variants

The same chassis houses different payloads:

| Sidecar type | Contents | Pigtail signals |
|--------------|----------|-----------------|
| **TX** | 35 mm piezo + TVS diodes only. Drive wires come from H-bridge in dry chamber. | 2× drive wires, GND. 3 pins. |
| **RX (JFET hydrophone)** | 27/35 mm piezo + J201 + 10 MΩ + 10 kΩ. | +V, Signal, GND. 3 pins. |
| **Light sensor** | VEML7700 lux sensor + clear epoxy window on the bottom cap. | I²C SDA, SCL, +V, GND. 4 pins. |
| **Depth/pressure** | MS5837-02BA pressure sensor (rated for water). | I²C SDA, SCL, +V, GND. 4 pins. |
| **IMU (orientation)** | BNO055 9-DOF IMU. Useful when interpreting bearing estimates. | I²C SDA, SCL, +V, GND. 4 pins. |
| **Temp** | DS18B20 (replaces the in-ballast probe). | 1-Wire data, +V, GND. 3 pins. |

All variants use the same 1.5" PVC chassis, the same M8 IP67
connector, the same mounting clamp. A buoy's capability is just
the sum of its installed sidecars.

## Waterproof connector

**Recommended**: M8 4-pin IP67 bulkhead connectors (panel-mount on
the dry chamber bulkhead, cable-mount on each sidecar pigtail).
~$5 per mating pair from Amazon or AliExpress. Threaded knurled
nut, EPDM gasket, rated to 1 bar (10 m water depth) — more than
adequate for a pool buoy and fine for shallow open-water.

**Alternatives**:

- **SP13 / SP16 aviation-style** — popular in hobby ROV builds,
  ~$3/pair. Bigger, more pins, also IP67-rated.
- **M12 IP68** — overkill for a pool, useful if you want >5 pins
  per sidecar (e.g., SPI sensor).
- **Wet-mate Subconn / Macartney** — pro-grade, ~$60+ per pair.
  Don't bother for the pool.

Pin assignments (M8 4-pin):

| Pin | RX / sensor sidecar | TX sidecar |
|-----|---------------------|------------|
| 1 | +V (3.3 V) | Drive A |
| 2 | Signal | Drive B |
| 3 | GND | GND |
| 4 | (reserved / I²C SCL or 1-Wire data) | (unused) |

Use color-coded cable consistent across all pigtails; a swapped
TX/RX connection at the connector can cook a JFET.

## Per-sidecar BOM

| Qty | Part | ~Price |
|-----|------|--------|
| 1 | 1.5" PVC pipe, ~80 mm | $0.50 |
| 1 | 1.5" flat test cap (bottom, sound window) | $1 |
| 1 | 1.5" threaded cleanout adapter + plug (top, serviceable) | $3 |
| 1 | Nitrile or silicone O-ring (for threaded top) | $0.30 |
| 1 | M8 4-pin IP67 connector pair (panel + cable) | $5 |
| 1 | PG7 cable gland with EPDM seal (signal exit on sidecar) | $1 |
| 1 | Transducer (piezo) or sensor module | $1–5 |
| 1 | J201 + 10 MΩ + 10 kΩ (RX only) | $0.60 |
| ~30 mL | USP-grade mineral oil (fragrance-free) | $0.20 |
| ~5 mL | PVC primer + solvent cement (bottom cap permanent bond) | $0.20 amortized |
| ~5 mL | Silicone caulk (cable gland double-seal) | $0.10 |
|   | **Total per sidecar** | **~$8** |

Per-buoy total with 1× TX + 1× RX sidecar = ~$16 plus the 3D-printed
clamp and the M8 bulkhead connectors on the dry chamber.

## Ordering checklist (Phase 1b, 3 buoys × 2 sidecars + clamps)

| Qty | Part | Source | ~Total |
|-----|------|--------|--------|
| 6 | 1.5" flat test caps | hardware store | $6 |
| 6 | 1.5" threaded cleanout + plug | hardware store | $18 |
| 10 | M8 4-pin IP67 connector pairs (extras for damage) | Amazon | $40 |
| 10 | PG7 cable glands EPDM | Amazon | $10 |
| 10 | J201 JFETs | Amazon, DigiKey | $5 |
| 1 | bag of 100× 10 MΩ resistors | Amazon | $3 |
| 1 | 16 oz USP mineral oil (fragrance-free pharmacy grade) | drugstore | $5 |
| 1 | PVC primer + cement | hardware store | $10 |
| 1 | Nitrile O-ring assortment | Amazon | $8 |
| 1 | 3D filament (PETG or ABS recommended — UV/water stable) | Amazon | $20 |
| 6 | 2" PVC ballast pigtail cables (M8 cable + 3-conductor + sleeving) | Amazon | $25 |
|   | **Subtotal** | | **~$150** |

Plus the per-buoy electronics (S2, S3, DRV8837, battery, etc.)
from [`requirements.md`](requirements.md).

## Assembly sequence (per RX sidecar)

**1. Bond the bottom cap permanently.**
Apply PVC primer to the inside of the flat test cap and the
outside of the 1.5" pipe's lower end. Apply solvent cement to
both. Slip on, twist ¼ turn, hold 30 sec. Cure 24 hours.

**2. Drill and install the M8 connector.**
Drill the side of the sidecar near the top (just below where the
threaded service cap will land) for the M8 panel-mount thread.
Install the bulkhead connector with its EPDM gasket. Wire the
connector pins to short pigtails inside the sidecar — these are
what the JFET subassembly will solder to.

**3. Build the piezo + JFET subassembly.**
- Solder J201 with the splayed-leg pattern from
  [`build-hydrophone-epoxy.md`](build-hydrophone-epoxy.md#assembly-steps).
- Bias resistors (10 MΩ gate-to-GND, 10 kΩ source-to-GND) in place.
- Piezo's signal lead to Gate, piezo's ground to GND wire.
- Three short wires (Drain ↔ +V pin, Source ↔ Signal pin, GND ↔
  GND pin) solder to the M8 bulkhead pigtails inside the sidecar.

**4. Bench-test in air.**
Apply 3.3 V to +V via a temporary patch lead, ground to GND, scope
the Signal pin. Tap the piezo. Expect 10–100 mV transients. If
nothing, fix now — same rule as the epoxy build.

**5. Position the piezo at the bottom.**
Lower the assembly into the sidecar so the piezo lies flat against
the inside of the bonded bottom cap, brass side down. The JFET +
resistors dangle above, supported by the wires to the M8
connector.

**6. Fill with mineral oil.**
- Use USP-grade, **fragrance-free** mineral oil. NOT scented baby
  oil — the fragrance is solvent-aggressive on PVC and silicone
  over months.
- Pour slowly through the open top, tilting the sidecar to release
  bubbles.
- Stop at ~90% full. Leave a 5–10% air bubble at the top for
  thermal expansion of the oil (mineral oil expands 0.07%/°C; a
  30 °C pool swing on a 50 mL chamber = ~1 mL expansion).
- Let sit 5 min. Top up if bubbles cleared more space.

**7. Thread on the service cap.**
PTFE thread tape on the threaded cleanout adapter, O-ring on the
plug, hand-tight + ¼ turn. Optional: thin bead of silicone caulk
around the cap-to-tube joint as a secondary seal.

**8. Final test.**
Connect a pigtail to the M8 connector, feed into the buoy's MCP6022
chain, tap the bottom cap underwater (glass of water on bench).
Compare SNR to the Phase 1 simple build baseline. Expect ~25 dB
improvement.

## TX sidecar specifics

The TX sidecar is simpler:

- No JFET, no preamp.
- Piezo + TVS diodes across its leads (TVS is essential — piezos
  generate flyback when mechanically struck or when the drive
  switches at the end of a burst).
- Two drive wires from the H-bridge in the dry chamber connect
  via the M8 pins (Drive A, Drive B), with shared GND.
- Oil-fill same as RX. The oil also helps damp ringing after the
  drive stops, tightening pulse decay.

**Keep the H-bridge in the dry chamber, not in the oil.** The
H-bridge dissipates ~100 mW and pushes 30 Vpp signals at moderate
current; better thermal and EMI environment dry-side.

## Optional: depth tether

For ideal acoustic conditions, the buoy can be lowered 5–10 ft
below the surface where surface chop, refraction, and bubble noise
no longer dominate. Add to the very bottom of the buoy:

- **Stainless steel eye bolt** through the bottom cap, sealed with
  silicone caulk and a backing nut + washer.
- **D-ring** on the eye bolt for a tether.
- **Paracord or dive line**, 1.5–3 m, with a snap clip on the far
  end.
- **Weight or stabilizer**: a 1–3 lb dive weight clipped to the
  far end keeps the buoy oriented vertically in current. A small
  fin instead (or in addition) damps rotation.

Above-water comms remain unaffected — the antenna mast still
breaks the surface as long as the buoy itself is positively
buoyant and the tether is the only thing dropping below the
surface. Or you can let the buoy itself hang below a small surface
float for full submersion; in that case the antenna goes on the
float, not the buoy.

This makes the same buoy work for surface-only deployment and for
"lowered-instrument" deployment by just clipping a weight on or
off.

## Service workflow

When a sidecar fails:

1. Disconnect the M8 connector at the bulkhead.
2. Slide the sidecar out of its quadrant clamp (with the 3D-printed
   mount design) or unscrew the hose clamps (with the cheap mount).
3. On a clean surface: unscrew the threaded service cap, drain oil
   into a container.
4. Replace the failed component (most commonly the JFET).
5. Bench-test before re-filling.
6. Refill oil, re-cap, re-install.

Total: ~10 minutes per sidecar. Compare to the epoxy build:
fabricate a new puck, ~30 minutes, oil sidecar wins decisively
once you've replaced your first one.

## 3D-printed components (Bambu P1S)

The oil-sidecar build is the most print-heavy of the hydrophone
options, because the modularity story depends on parts that don't
exist as off-the-shelf PVC fittings. The matrix below distinguishes
*structural* (load-bearing or marine-exposed) from *internal*
(inside the dry chamber or inside an oil sidecar — protected).

| Printed part                       | Material              | Print params              | Notes                                                |
|------------------------------------|-----------------------|---------------------------|------------------------------------------------------|
| **Quadrant clamp** (sidecar mount around 2" ballast pipe) | **ASA** or **PA-CF** | 100% infill, 5 walls       | UV-exposed, structural. ASA for cost ($25/kg), PA-CF for stiffness if the fleet sees real wave loads. |
| **Sidecar slide-in saddle** (inside the quadrant clamp)   | ASA                   | 100% infill, 4 walls       | Holds each sidecar in place. Captured M3 nut + bolt is the lock. |
| **Internal piezo holder** (inside each sidecar, in oil)   | PETG-CF or PA-CF      | 100% infill, 4 walls       | Keeps the piezo flat against the bottom cap and centered in the chamber. Oil-immersed; PA-CF preferred for dimensional stability. |
| **JFET + R-network cradle** (inside each sidecar, in oil) | PETG-CF or PA-CF      | 100% infill, 3 walls       | Same chamber as above; holds the JFET subassembly suspended in oil. |
| **M8 connector backing plate** (inside dry chamber bulkhead) | PLA + Plasti-Dip OR PETG | 100% infill, 4 walls    | Dry-side, distributes the M8 panel-mount torque across the PVC bulkhead. |
| **Internal sidecar cap retainer (top, threaded)** | PETG                  | 100% infill, 6 walls       | Optional replacement for the threaded cleanout adapter if you want a cleaner cap. Use heat-set M3 brass inserts for the cap-thread interface. |
| **Pigtail comb / strain-relief tree**       | PLA                   | 50% infill, 3 walls         | Lives in the dry chamber; organizes the 4 sidecar pigtails so they don't tangle when sidecars are swapped. |
| **3D-printed M8 cable boot** (over the cable side of each connector) | TPU 95A | 100% infill, 3 walls    | Vibration / wave-slap protection on the cable entry into each sidecar. |
| **Sidecar oil-fill funnel**         | PLA                   | 20% infill, 2 walls         | Disposable, single-use. Print one per pour; throw away when oily. |
| **Drainage capture tray**           | PETG                  | 50% infill, 3 walls         | Catches old oil during sidecar service. Reusable across the fleet. |

### Material choices, justified

- **PA-CF (carbon-fiber nylon)** is the right answer for any part
  that lives in oil and needs to hold tolerance. Doesn't swell.
  Doesn't creep. Dimensionally stable. Requires a hardened nozzle
  on the P1S (factory-installed on the P1S Combo).
- **ASA** is the right answer for everything outside the buoy
  exposed to sunlight (the quadrant clamps especially). UV-stable;
  doesn't yellow or embrittle the way ABS does.
- **PETG / PETG-CF** is the middle-ground for non-load-bearing
  oil-immersed parts. Cheaper than PA-CF, less hygroscopic than
  plain PA.
- **PLA + Plasti-Dip** is fine for any part inside the sealed dry
  chamber. Disposability framing applies.

### Coating strategy

- **Plasti-Dip** (~$10/can): two coats over any printed part with
  marine exposure. Even ASA benefits — the dip seals print-layer
  microvoids that water finds over months of immersion.
- **Marine epoxy spray** on the quadrant clamps: harder finish for
  the parts that take wave abrasion against the ballast pipe.
- **Internal oil-immersed PA-CF parts**: anneal in the P1S
  enclosure at 80–110 °C for 1 hour to reduce water-uptake
  sensitivity. Skip the dip on these; the dip would only delaminate
  in oil.

### Heat-set inserts

Every threaded interface on a printed part should use **M3 brass
heat-set inserts** ($0.05 each). The quadrant clamps in particular
take repeated assembly/disassembly during fleet reconfiguration;
printed threads strip within ~20 cycles, heat-set inserts last
forever.

### Disposability note

A complete printed set per buoy (1× quadrant clamp + 4× saddles +
4× internal cradles + assorted small parts) costs ~$3 of filament
even in PA-CF. A lost buoy with this packaging is a $60 incident,
not a $600 one.

## Why this design composes well

- **In-buoy TDoA bearing** (Phase 3 stretch goal): just install 2
  or 3 RX sidecars on different quadrants. Same firmware, more
  detectors, the shore service automatically uses them.
- **Sensor mesh expansion**: a light-sensor sidecar gives the
  buoy a `light.event` stream onto the chain without changing the
  electronics — just plug it in.
- **Failure isolation**: a flooded sidecar (worst case) doesn't
  flood the dry chamber. The buoy keeps working with reduced
  capability.
- **Open-water upgrade**: the same sidecars work in salt water as
  long as the M8 connectors stay dry-mated and don't get
  galvanically attacked. For long deployments, upgrade to
  bronze or stainless M8 housings.
- **Manufacturing**: identical sidecars across a fleet means
  bulk-build them, bin-test them, install at deployment time.
