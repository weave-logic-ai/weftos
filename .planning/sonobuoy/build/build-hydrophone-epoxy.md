# Sonar Buoy — Epoxy-Potted Hydrophone Build

**Status**: Phase 1b — option A. Permanent, cheapest, smallest. Use
when you don't need to service the hydrophone after build.
**Per-unit cost**: ~$3 in parts.
**Build time**: ~30 min per unit once practiced.
**Improvement over Phase 1 simple build**: ~20 dB SNR, immune to
cable capacitance, direct water-coupled face (no PVC-wall loss).

This recipe is the long-standing Lang Elliott / Aquarian H1a-clone
tradition used by underwater field recordists. Piezo + JFET
preamp + bias resistors all live inside a small PVC slip cap,
sealed in marine epoxy. The result is a small black puck with a
cable coming out. Hard-mount it to the buoy and forget about it.

For a *serviceable* alternative with better acoustic match and
modular swappability, see
[`build-hydrophone-oil.md`](build-hydrophone-oil.md).

## Circuit (3-wire JFET source follower, 3.3 V)

The JFET source follower buffers the piezo's high-impedance output
down to a few kΩ so cable capacitance and the downstream op-amp
input don't roll off the signal. Gain is ~0.95 — the job is
impedance transformation, not amplification.

```text
   Hydrophone end (potted):              Buoy end:

   piezo+ ───── Gate
                │                        +V wire ←── 3.3 V (clean LDO)
                10 MΩ to GND
                │
                │       ┌── Drain ←──── +V wire
                │       │
            ── Gate     │
              J201      │
                │       │
                └─── Source ────┬────── Signal wire ──→ 1 µF DC block
                                │                          │
                                R_s = 10 kΩ                 ▼
                                │                       MCP6022 input
                                ▼                       (existing BPF chain
   piezo- ────────── GND wire ──┴── ── ── GND wire ──── stays unchanged)
```

**Bias point** (J201 at Idss ≈ 0.5 mA, Vp ≈ -1.5 V, R_s = 10 kΩ,
3.3 V supply): self-bias solves to V_source ≈ 0.87 V, Id ≈ 87 µA,
Vds ≈ 2.4 V. Plenty of saturation headroom. Output impedance ≈ 3–10 kΩ.

This same circuit is used by the oil-filled build; only the
packaging differs.

## Physical layout

```text
                  ╔══════════════════════╗
   To buoy        ║     PVC slip cap     ║   Approx scale:
   ADC ←──────────╫─cable gland          ║   ½" or ¾" slip
                  ║  ┃                   ║   cap from hardware
                  ║  ┃ ~10 cm cable      ║   store (~$0.50).
                  ║  ┃ (3-conductor)     ║
                  ║  ▼                   ║
                  ║ ┌────┐               ║
                  ║ │J201│               ║
                  ║ └─┬──┘               ║
                  ║   │  ┌── 10 MΩ ──┐   ║
                  ║   ├──┤           │   ║
                  ║ ╔═════════════════╗  ║
                  ║ ║  27 mm piezo    ║  ║   piezo lies FLAT
                  ║ ╚═════════════════╝  ║   against the inside
                  ║▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓║   bottom of the cap,
                  ║▓▓▓ MARINE EPOXY ▓▓▓▓▓║   brass side OUT.
                  ╚══════════════════════╝
                       ↑
                   sound entry face
                   (1–2 mm of epoxy as acoustic window)
```

Final form factor: small black puck with a cable coming out one
side. Roughly 25 mm tall, 25 mm diameter for a ½" cap.

## Per-unit BOM

| Qty | Part | ~Price |
|-----|------|--------|
| 1 | Piezo disc, 27 mm or 35 mm (existing 35 mm parts work) | $0 |
| 1 | J201 N-channel JFET, TO-92 (or MMBFJ201 SOT-23) | $0.50 |
| 1 | 10 MΩ resistor, ¼ W | $0.02 |
| 1 | 10 kΩ resistor, ¼ W | $0.02 |
| ~10 cm | 3-conductor shielded cable | $0.30 |
| 1 | 1 µF film cap (DC block at buoy end) | $0.30 |
| 1 | 0.1 µF ceramic cap (VCC bypass at buoy end) | $0.05 |
| 1 | ½" PVC slip cap | $0.50 |
| ~5 mL | Marine 2-part epoxy (West System G/flex 650 recommended) | $0.50 amortized |
| ~3 cm | Heat shrink tubing | $0.05 |
| 1 | Cable gland or epoxy plug at cap's drilled hole | $0.50 |
|   | **Total per unit** | **~$2.75** |

**JFET alternatives**: 2N5457, BF862 (SMD), MMBFJ201 (SMD) all
work. J201 has the best price/availability/noise tradeoff.

**Epoxy choices**: West System G/flex 650 (flexible, best acoustic
match, ~$25/kit), J-B Marine Weld (rigid, cheap), Loctite EA E-30CL
(clear, lets you see bubbles). Avoid 5-minute epoxy (cures too fast
for bubbles to escape) and polyester resin (brittle underwater).

## Ordering checklist for Phase 1b fleet (3 hydrophones)

| Qty | Part | Source | ~Total |
|-----|------|--------|--------|
| 5 | J201 JFETs (extras for breakage) | Amazon, DigiKey | $3 |
| 1 | bag of 100× 10 MΩ ¼ W resistors | Amazon | $3 |
| 1 | bag of 50× 1 µF film caps, 50 V | DigiKey | $5 |
| 3 m | 3-conductor shielded mic cable | Amazon | $5 |
| 3 | ½" PVC slip caps | hardware store | $2 |
| 1 | West System G/flex 650 epoxy kit | West Marine, Amazon | $25 |
| 1 | Vacuum bagging kit (Tupperware + hand pump) — recommended | Amazon | $20 |
|   | **Subtotal** | | **~$60** |

## Assembly steps

**1. Pre-fit the cap.** Drill a 4–5 mm hole in the side of the slip
cap (not the bottom) near the open rim for the cable to exit.

**2. Build the circuit "in air" first.**
- Strip 5 cm of cable, tin the three conductors.
- Solder the JFET's three legs splayed: Drain to +V wire, Source
  to Signal wire, Gate held with a 10 MΩ resistor to GND wire.
  Verify the J201 pinout against the datasheet — TO-92 pinout
  varies by manufacturer.
- Solder a 10 kΩ resistor between Source and GND wire.
- Solder the piezo's signal terminal (brass-side inner dot) to
  the Gate. Solder the piezo's ground (surrounding brass) to the
  GND wire.

**3. Bench-test before potting.** Power +V (3.3 V) and GND, feed
the Signal wire through a 1 µF cap into a scope or the buoy's
MCP6022 input. Tap the piezo — expect clean 10–100 mV transients.
**If you see nothing here, you cannot debug a potted unit. Fix
now.**

**4. Position in the cap.**
- Thread cable through the side hole with strain relief.
- Place the piezo flat against the inside bottom of the cap,
  brass side facing the cap's *outer* wall (this is the
  sound-entry face).
- Fold the JFET + resistors above the piezo, dangling in the
  cap interior. The epoxy will hold everything.

**5. Pot.**
- Mix epoxy slowly — fast stirring whips in bubbles.
- Pour in stages: thin layer first to wet the piezo, wait 5 min
  for bubbles to rise, then fill.
- Tap to release trapped air. A cheap vacuum chamber ($20
  Tupperware + hand pump) dramatically improves yield on three+
  units.
- Cure 24 hours at room temperature for marine epoxy. Don't
  shortcut.

**6. Final test.** Repeat the tap-test underwater (glass of water
on the bench is fine). Expect 5–10× higher output than the bare
disc + op-amp because the JFET preserves the piezo's full output
instead of loading it into the op-amp input plus cable
capacitance.

## Integration with the buoy

The hydrophone *replaces* the bare-piezo RX input of the Phase 1
simple build. Three changes:

1. Add a **3.3 V → cable +V wire** connection from the existing
   clean LDO, with a **0.1 µF ceramic** bypass to GND right at
   the cable entry.
2. Add a **1 µF film DC-blocking cap** between the cable's Signal
   wire and the existing first op-amp's input (replacing the
   direct piezo connection).
3. **GND wire** ties to system ground. Shield (if present) ties
   to GND at the buoy end only.

The two-stage gain + 1.8 kHz BPF chain is unchanged. The
hydrophone just looks like a much better piezo to it.

## Mounting on the buoy

Once potted, the puck needs to be physically attached to the
buoy somewhere that exposes its sound-entry face to water and
keeps the cable run short and dry-routed:

- **In the flooded ballast section**: drill a small mounting
  hole in the side of the ballast PVC, run the cable through a
  cable gland into the dry electronics compartment, and use a
  zip-tie or stainless hose clamp to hold the puck against the
  ballast pipe with the sound face outward. Simple but the puck
  is acoustically coupled to the pipe wall — some hull-borne
  vibration coupling.
- **Hanging on a short tether from the bottom of the buoy**:
  10–20 cm of stiff cable + a slip-knot mount. Gets the puck
  away from the hull. Better acoustic isolation, more fragile.
- **Recessed into a saddle clamp**: 3D-printed mount that grips
  the ballast pipe and holds the puck. Best of both. See the
  oil-filled build for clamp-mount geometry that adapts to
  pucks.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| No signal after potting | Piezo crushed, JFET leg shorted by stray solder, or gate-bias resistor open | Build a spare. Cannot un-pot. Always bench-test before pouring. |
| 60 Hz hum | Cable shield grounded at both ends | Ground shield only at buoy end. |
| Loud crack on power-up | DC offset on coupling cap charging into op-amp | Add 1 MΩ bleed resistor across the 1 µF DC block. |
| Saturation at low SPL | Source resistor too low; JFET running hot | Increase R_s to 22 kΩ; recheck bias. |
| Rolls off above ~5 kHz | Cable too long or capacitive | Shorten, or add common-source gain stage on hydrophone side. |
| Sensitivity worse than bare piezo + op-amp | Bubbles trapped near piezo face | Visible in clear epoxy. Re-pot. Vacuum bagging prevents this. |

## 3D-printed components (Bambu P1S)

The epoxy-potted build is almost all chemistry and very little
mechanics, but a few printed parts make the build repeatable and
the failure rate lower. Because the whole assembly is encapsulated
in marine epoxy at the end, **the internal printed parts never
see water** — PLA is fine for all of them.

| Printed part                              | Material            | Print params              | Why                                                  |
|-------------------------------------------|---------------------|---------------------------|------------------------------------------------------|
| **JFET + bias-resistor positioning jig**  | PLA                 | 100% infill, 3 walls      | Two-piece clamshell that holds the J201 and the two resistors at correct spacing during soldering. Reused across the whole batch. |
| **Piezo-against-cap-bottom seating disc** | PLA                 | 100% infill, 3 walls      | Thin disc that sits between the piezo's signal-side face and the cap interior, keeps the piezo flat and parallel during pour. Stays inside the puck. |
| **Pour-stand holder**                     | PLA                 | 25% infill, 3 walls       | Vertical holder for the slip-cap during epoxy pour and cure. Holds 6 caps upright at once for a fleet build. |
| **3D-printed slip-cap (alternative to PVC)** | PETG, 0.16 mm layers | 100% infill, 6 walls    | Optional: replace the bought ½" PVC slip cap with a printed one if you want a custom internal volume (less epoxy used / different form factor / cable-exit position). PETG handles epoxy chemistry better than PLA at the wall interface. |
| **Cable strain-relief boot**              | TPU 95A             | 100% infill, 3 walls      | Slips over the cable at the cap exit and is captured in the epoxy pour. Prevents the cable from being yanked out of the cured puck. |

### Coating strategy

The puck is sealed in marine epoxy as the build's terminal step,
so post-coating is unnecessary. If you opt for a printed slip-cap
(PETG) instead of a bought PVC one, a thin coat of marine epoxy
*outside* the PETG belt-and-braces against pinhole leaks. PLA
internal parts need nothing — they live inside the cured epoxy
matrix.

### Disposability note

A failed pot (bubble blocking the piezo face, JFET shorted) is
unrecoverable — but the printed-jig + printed-cap approach means
rebuilding takes 30 minutes and ~$3 in materials. The cost of a
ruined puck is dominated by the JFET, not the plastic.

## When to choose oil instead

Pick [`build-hydrophone-oil.md`](build-hydrophone-oil.md) over this
recipe if you want any of:

- **Serviceable units** — drain, swap JFET, refill in 10 minutes
  instead of building a new puck.
- **Better acoustic match** — oil ~1.4 MRayl matches water 1.5
  MRayl almost perfectly; epoxy ~3.0 MRayl loses a few dB at the
  water interface.
- **Modular fleet** — sidecar oil chambers attach to a buoy with
  waterproof connectors. Build a buoy that's TX-only, add an RX
  sidecar later, swap a dead unit on a dock.
- **Multiple RX per buoy** — stack sidecars on different sides /
  quadrants of the ballast for in-buoy TDoA bearing.
- **Sensor sidecars** — same chassis as RX sidecars but with
  light / pressure / IMU instead of a piezo. Modular sensor mesh.

The epoxy build remains the right choice if you want the
absolutely cheapest, smallest, most permanent unit and you'll
never need to open it.
