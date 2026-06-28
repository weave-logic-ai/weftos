# Paper Analysis — Fonseca & Maia Alves 2012, Rigid-Plate Magnetically-Suspended Projector

**Citation**: Fonseca, P.J.; Maia Alves, J. *A new concept in underwater high fidelity low frequency sound generation.* Review of Scientific Instruments **83**(5):055007, 18 May 2012. doi:10.1063/1.4717680.
**License**: AIP (paywalled at source); PDF acquired via user-drop 2026-05-11 (ResearchGate copy).
**PDF**: `.planning/sonobuoy/papers/projector-mma/pdfs/fonseca-alves-2012-rsi-rigid-plate-projector.pdf` (4 pages, 449 KB).
**Patent**: WO2012095780A1 (PT pending PT105474). **Legal status: CEASED 2013-07-10.** Design is fully public domain to build, modify, and publish on.
**Affiliation**: Faculdade de Ciências, Universidade de Lisboa, Centro de Biologia Ambiental and SESUL, Lisboa 1749-016, Portugal.
**Funding**: Portugal FCT project PDCT/MAR/68868/2006; pluriannual program UI&D 329.
**Analyzed by**: sonobuoy symposium analyst, 2026-05-11.
**Verification status**: ✅ verified by direct read of all 4 pages of the PDF.

## Priority for clawft

**P0 — primary transmitter candidate.** Per user direction 2026-05-11: *"This is actually a REALLY good method, we can build these, and make it specific to our application... these low frequency waves can be swept across frequencies and get very good imaging from them I believe because it will be very clean signal, no resonance."* The paper's key claim — measured ±3 dB flat magnitude AND flat phase from 10 Hz to 3 kHz — is the technical foundation for using this as the **primary** acoustic transmitter rather than merely a time-sync beacon. See `../transmitter-options-catalog.md` §A3 for the catalog entry and §A3-bis for the architectural implication.

## Why this paper matters for clawft

Every other electromechanical underwater transmitter (piezo, voice-coil-with-spider, flextensional, magnetostrictive Tonpilz) carries a mechanical resonance that dominates its frequency response. That resonance:

1. Smears swept-frequency (LFM chirp) pulses through group-delay distortion → corrupts the receiver's matched filter → reduces range resolution and pulse-compression gain `T·B`.
2. Rings *after* the drive pulse ends → buries close-range echoes in the ringdown.
3. Constrains the band to a narrow region around the Q peak → can't be a primary transmitter AND a beacon AND a sub-bottom profiler from the same aperture.

Fonseca & Alves eliminate the mechanical resonance entirely by replacing the elastic suspension (rubber surround, spider, spring) with an **electromagnetic spring** — DC-biased coil pair that creates a force null at the equilibrium position. The result is a transducer whose frequency response is dominated by the radiation impedance and the magnet/plate mass, neither of which has a high-Q peak. The measured frequency response (Fig. 3a) confirms this: **smooth, ±3 dB, 10 Hz to 3 kHz**, with phase angles "astonishingly even and close to zero" across the band.

If those numbers hold up at clawft-relevant scale and power levels, this is the cleanest transmitter we've found for combined ranging + time-sync + sub-bottom in a single aperture.

## Device construction (from Figure 2 schematic)

```
                 housing                            water
   ┌─────────────────────────────┐    ╔════════════╗
   │  ┌──┬─────┬─┐               │    ║  RIGID     ║
   │  │  │     │ │      ┌────────┼────╣  PLATE     ║
   │  ├──┤ MAG ├─┤  ←──→│ rod    │    ║  Ø = 30 mm ║   ← radiates to water
   │  │  │     │ │      └────────┼────╣            ║
   │  └──┴─────┴─┘               │    ╚════════════╝
   │  ┃┃     ┃┃ ┃┃     ┃┃        │       3 mm gap
   │  ┃┃     ┃┃ ┃┃     ┃┃        │       (plate-to-housing standoff)
   │  ║║excit║║ ║║posit║║        │
   │  ║║coils║║ ║║coils║║        │
   │  ║║(2)  ║║ ║║(2)  ║║        │
   │  ┃┃     ┃┃ ┃┃     ┃┃        │
   │  ┃┃     ┃┃ ┃┃     ┃┃        │
   └─────────────────────────────┘
```

Key physical dimensions (from Fig. 2 callouts):

| Part | Value |
|---|---|
| Rigid plate diameter | **30 mm** (3 cm) |
| Plate-to-housing standoff | **3 mm** |
| Permanent magnet | Cylindrical, embedded inside the rigid attachment to the plate |
| Excitation coils | **One pair, wound in *opposite* directions** (one CW, one CCW). Drive: variable AC current. |
| Positioning/damping coils | **One pair, wound in *same* direction**. Drive: voltage-controlled DC current. |

The 3 cm plate is the surprising part — this is *tiny* for low-frequency radiation. The authors explicitly note (p.2): *"This prototype was developed to meet the needs for playback experiments with small fishes. For this reason the dimensions of the device were kept considerably small. However larger devices should, in principle, perform even better, because larger discs will be more efficient in low frequency sound generation."* A scaled-up plate is part of the clawft adaptation path.

## How the magnetic suspension works

The two coil pairs play distinct, decoupled roles:

**Positioning / damping pair (DC-driven, same winding direction)**: When energized, each coil creates a magnetic field that pushes the embedded permanent magnet *toward* one or *away from* the other. With equal current in both and opposing field directions on either side of the magnet, the magnet sees zero net force at the geometric midpoint — that midpoint becomes the equilibrium position. Adjusting the DC current scales the *stiffness* of this magnetic spring (analogous to changing the spring rate of a mechanical spring). The paper calls this the "dumping" current (their typo for "damping").

**Excitation pair (AC-driven, opposite winding directions)**: Because the two coils are wound oppositely, an AC current in series flips the direction of force the two coils apply to the magnet at any given instant. The result is a net axial force on the magnet that follows the AC waveform. The magnet drives the rigid plate, the plate displaces water, sound propagates outward.

**Why this gives flat frequency response**: A mechanical spring has a stiffness `k` and the mover has mass `m`, giving a mechanical resonance `f₀ = (1/2π)√(k/m)`. Below `f₀` the response rolls off; above `f₀` it rolls off too. The Q at resonance can be 5–50 depending on damping.

In the Fonseca-Alves design, the magnetic spring constant is set by the DC current, and the damping is set by the same current. By **tuning the DC current to push f₀ well below the operating band** (or by making the magnetic spring much weaker than the radiation impedance loading), the response in the operating band is dominated by the radiation impedance, not the mechanical resonance. There's no high-Q peak because there's no high-Q mechanical system.

## Measured performance (the load-bearing numbers)

### Frequency response — Figure 3a

- Test: sine sweep 0–3000 Hz, 20 ms duration, FFT-based transfer function, 25-stimulus averaging.
- Reference: Brüel & Kjær 8103 hydrophone (response 0.1 Hz–180 kHz, sensitivity −211 dB re 1 V/µPa) into B&K 2238 Mediator sound level meter.
- I/O: NI USB-6251 multifunction board, 100 kHz throughput.
- Compute: 2048-point FFT in LabVIEW; delay-compensated to remove electronics+propagation time.

**Result: ±3 dB flat, 10 Hz to 3 kHz.** Confirmed against single-frequency calibrations of two separate prototype units (circles and triangles in Fig. 3a) — both agree, so the response is reproducible.

### Phase response — Figure 3a/b

**Phase "astonishingly even and close to zero" across the full 0–3000 Hz band.** This is the genuinely remarkable claim, and the one that matters most for chirp / LFM ranging. A flat phase response means *group delay is constant across frequency* — a swept-frequency pulse arrives at the receiver as a coherent compressed pulse, not a smeared one.

### Coherence — Figure 3a/b

Coherence function close to 1.0 across the band → the transfer function measurement is statistically valid (output is causally driven by input, not by noise).

### Radiation pattern (Figure 3, comparison of positions)

- **Centered** (on-axis, in front of disc): flat magnitude and phase.
- **Disc edge** (off-axis but still in front): flat magnitude, slight phase variation.
- **At disc plane** (in the equatorial plane of the disc): magnitude drops at higher frequencies, phase swings wildly due to interference between the front and rear radiation of the disc.

**Conclusion: the device acts as an acoustic dipole.** Sound field is even in the front (and rear, by symmetry) cone, with a null in the disc plane.

### Distance behavior (Figure 3b)

Measured at 1.7 cm, 3 cm, 6 cm, 12 cm from the disc front. Magnitude and phase characteristics are stable with distance — meaning the near-field-to-far-field transition is smooth and the device behaves predictably across the tested range.

### Dynamic range — Figure 4b

**>36 dB** demonstrated by playing back a painted goby (*Pomatoschistus pictus*) sound at six successive 6-dB attenuation steps. Each step reproduces faithfully, with the lowest still above the system noise floor. The upper bound wasn't tested because the maximum playback amplitude already exceeded natural fish-sound amplitudes.

### Drive currents

- DC positioning/damping current: "**a few tenths of an ampere**" (i.e., ~0.1–0.5 A standing).
- AC excitation peak current: "**a few amperes**" (i.e., 1–5 A peak).

At a coil resistance on the order of 1 Ω (typical for a multi-turn copper coil of this size), this puts standing power at <1 W and peak excitation power around 10–25 W. Average power depends entirely on duty cycle and waveform.

## Important nuances and gaps

### Paper validates 10 Hz – 3 kHz; patent claims 5 Hz – 50 kHz

The paper's measured band is **10 Hz to 3 kHz**. The patent claims a wider range (5 Hz to 50 kHz, refined claim 15 Hz to 5 kHz). The 3 kHz upper limit in the paper is the **measurement upper limit of the sweep stimulus**, not necessarily the device's upper limit — but it has not been demonstrated above 3 kHz in this paper. The patent's 50 kHz claim is unsupported by published data we have.

For clawft purposes: **trust the 10 Hz – 3 kHz number as bench-validated**, treat anything above 3 kHz as needing our own bench characterization.

### No absolute SPL figures

The paper reports the transfer function magnitude in dB — this is *output / input ratio*, NOT absolute source level re 1 µPa @ 1 m. The user's earlier framing of "150–170 dB" comes from a *different* projector class (Wallin 2017 MMA, or general voice-coil scaling); it is not confirmed by this paper.

We genuinely do not know what SPL this specific 3-cm-plate prototype produces. The paper acknowledges this and points out that "larger discs will be more efficient in low frequency sound generation" — so a scaled-up clawft version would be louder, but by how much is open research.

**Best estimate from physics**: a 3 cm piston at 1 kHz with a few-amp drive in water is probably in the **130–150 dB re 1 µPa @ 1 m** range; at lower frequencies the radiation resistance drops as (ka)² so SPL drops too. The bigger plates the authors recommend (say 10–15 cm) would give 10–14 dB more low-frequency output by area alone, putting the scaled version plausibly in the **150–170 dB band the user mentioned** — but this is order-of-magnitude reasoning, not measured.

### Dipole behavior is not omnidirectional

The disc is an acoustic dipole — strong front/back, null in the disc plane. For a buoy with a horizontal mesh of peers, this matters: orienting the disc *vertically* gives a horizontal omni-pattern (good for inter-buoy comms) but a null straight down (bad for sub-bottom profiling). Orienting *horizontally* gives a strong sub-bottom signal but a null toward the horizontal mesh.

**Implication**: clawft probably needs *two* rigid plates per buoy, one horizontal (sub-bottom + downward imaging) and one vertical (mesh comms + bearing). Or one tilted at 45° with both modes degraded. This deserves a separate design exercise.

### Pressure-rating is asserted but not demonstrated

The paper *claims* depth independence because there's no compressible air. This is mechanically correct in principle — every internal void is water-flooded, so external pressure doesn't deform anything. But the paper does NOT report pressure-cycling tests. Magnet adhesives, coil potting compounds, and housing seals all need to survive pressure transitions; that's a separate verification.

### No chirp / LFM characterization

The paper validates fish-call playback but not swept-frequency chirps. The flat magnitude + flat phase responses *strongly imply* chirps would work well, but this hasn't been demonstrated in the paper. **This is the single most important characterisation to redo on our own prototype** before committing to chirp ranging on this transducer class.

### No multi-user / multi-node studies

The paper is about a single transducer in a tank. Coexistence questions (multiple buoys broadcasting on different carrier slots, intermodulation, mutual interference) are entirely outside its scope.

## Clawft adaptation plan (research outline, not commitment)

### Geometric scaling

| Parameter | Paper prototype | Clawft target | Rationale |
|---|---|---|---|
| Plate diameter | 30 mm | **100–150 mm** | Radiation resistance ∝ (ka)² → 11× to 25× more output power at the same drive level. Still fits in a 4" or 6" buoy. |
| Plate material | Unspecified (rigid) | **Aluminium 6061 disc, 2–3 mm thick** | Easy to source, stiff enough, corrosion-acceptable with anodizing. Magnesium would be stiffer but harder to source. |
| Magnet | Cylindrical permanent | **N42 neodymium cylinder, 12 mm × 25 mm or similar** | Off-the-shelf from K&J Magnetics, ~$15. |
| Coils | Custom-wound | **Custom-wound on 3D-printed PETG bobbins, 200–400 turns 24 AWG** | Hobby-tier; same technique as DIY voice coils. |
| Housing | Cylindrical, unspecified material | **HDPE or PVC pipe section, 100 mm ID** | Matches existing buoy build (see `../../../build/`). |

### Drive electronics

| Function | Paper | Clawft proposal |
|---|---|---|
| DC bias source | Voltage-controlled current source (discrete) | **LM317 in current-source config, or AnyTinyDC-DC buck regulated to 500 mA** | $1 BOM |
| AC excitation amp | Transconductance amp (discrete) | **TPA3116D2 class-D audio amp board (50 W stereo, ~$10)**, configured as transconductance via a current-sense resistor and feedback | $10 BOM |
| Waveform synthesis | LabVIEW + NI USB-6251 | **ESP32-S3 internal DAC at 8 kHz, or external I²S codec at 192 kHz** | Already on every buoy |

### Verification benchmark order

1. **Reproduce the paper's frequency response measurement** on our bench prototype — confirm ±3 dB / flat phase from 10 Hz to 3 kHz on the scaled-up version.
2. **Add chirp characterisation** — LFM sweep at 100 Hz–3 kHz, 1 s duration, 1 ms duration; verify matched-filter pulse compression on the received signal.
3. **Absolute SPL measurement** — calibrated hydrophone at 1 m, measure source level vs frequency for a 1 V or 1 A drive reference.
4. **Pressure cycling** — pressure pot or aquarium with weights; cycle from 0 to 2 bar (20 m equivalent) and verify the frequency response doesn't drift.
5. **Field range test** — two prototypes in a lake/pond, measure detectable range vs frequency.
6. **Multi-source carrier-slot coexistence** — three prototypes broadcasting on three different VLF carriers, confirm a receiver can separate them.

## Tiered transmitter architecture (user direction 2026-05-11)

User direction evolved in two steps during the same session. First: *"can we build it so there are 4, and they are essentially pointed horizontally in 4 directions. This would allow receivers to get more than one signal and we can encode which one is sending into it..."* Then the scoping refinement: *"Only main nodes need a full power one, a mesh would be able to use this from a large distance, so having more than one is not needed (20km+). It may be that we find we want to use this same feature for return signals, a node can have more than one driver but only fire the one in the direction of the main body of the mesh it is on. It also may have other ways to communicate on top of this."*

This resolves into a **two-tier transmitter architecture**:

| Tier | Role | Transmitter config | Power | Notes |
|---|---|---|---|---|
| **Tier 1 — Main / anchor node** | One per ~20 km region | **4-face omnidirectional** rigid-plate cluster | High (tens of watts peak, scaled-up plates) | Broadcasts time + position + ID continuously; reachable by entire local mesh from many km away |
| **Tier 2 — Mesh node** | N per cluster | **Multiple drivers per node, but only one fires at a time**, aimed at the cluster centroid | Modest (single-face power, time-multiplexed) | Local mesh comms; doesn't waste energy radiating in directions where no peer is listening |
| **Tier 2 supplement** | Same nodes | **Other comms layers**: WiFi/LoRa above surface, magnetic induction in the intra-mooring vertical stack | n/a | Acoustic transmitter is one of several comms paths, not the only one |

The big architectural win of this scoping is that **only one buoy in the region needs the expensive full-power 4-face configuration**. Everyone else is cheaper.

### Range plausibility check (Tier 1, 20+ km)

For a 20 km link at typical seawater conditions, with low-band carrier ~500 Hz:

| Term | Value | Source |
|---|---|---|
| Absorption coefficient @ 500 Hz | ~0.06 dB/km | Francois-Garrison formula |
| Total absorption loss over 20 km | ~1.2 dB | A·r |
| Cylindrical spreading loss over 20 km | 20 log₁₀(20000) ≈ **86 dB** | dominant term in shallow water |
| Ambient noise in 100 Hz band around 500 Hz | ~55–75 dB re 1 µPa | Wenz, sea state 2–4 |
| Receiver SNR target (matched-filter, with ~30 dB processing gain) | ~−15 dB pre-correlation | Standard chirp ranging |

**Required source level**: 86 + 1.2 + 75 − 30 ≈ **132 dB re 1 µPa @ 1 m**

That's a perfectly reasonable SPL for a scaled-up Fonseca-Alves (15 cm plate, few-amp drive). The user's "20 km+" claim **does** check out for the Tier-1 anchor, particularly in the low-band where absorption is negligible. The constraint is more about ambient noise floor and beamforming gain at the receiver than about transmitter power.

### Tier 1: 4-face omnidirectional anchor

This is a strong configuration for the dipole geometry — each plate's null plane contains the next plate, so the four discs naturally don't drown each other out at the source.

### Geometry

```
            top view of buoy
              N-face
                ●
                ║
                ║
    W-face ════╬════ E-face       ● = rigid-plate disc
                ║                  ║ = disc axis (radiates ⊥ to ║)
                ║                  Each disc radiates as a dipole:
                ●                  strong forward & backward,
              S-face                null in its own disc plane
```

The four discs are mounted radially around the buoy's vertical axis, separated by 90°. Each radiates in its own front+back lobe (dipole). The geometry has a key property: **the null plane of any one disc passes through both adjacent discs**. That means firing one disc puts almost no energy into the adjacent discs' axes — they don't deafen each other when simultaneously active, and they don't shake each other mechanically as hard as a tighter spacing would.

### What this configuration buys

1. **Bearing-from-amplitude-ratio (virtual compass at the source)**. A receiver hears all four faces with different SPLs depending on its azimuth relative to the source buoy. The ratio of received SPLs gives the receiver an estimate of which side of the source it's on — without either buoy needing an actual compass. Specifically, if the four faces are calibrated and we treat the dipole pattern as cos(θ - θ_face) where θ_face is each face's bearing, the receiver's azimuth relative to the source is recoverable from the four-amplitude tuple.

2. **Per-face ID in the encoding**. Each face transmits its node-ID *plus* its face-tag (N/E/S/W or a per-face code). The receiver knows not just "buoy 7 fired" but "buoy 7's E-face fired." This shrinks the combinatorics of arrival matching in the mesh, and provides a sanity check (the four faces of one buoy should all arrive within milliseconds of each other since they're co-located).

3. **Redundancy against shadowing**. If one face is occluded — by another buoy, surface reflection geometry, or a fish school — the other three usually still reach the receiver. The strongest of the four dominates the matched-filter peak; the others provide TDOA cross-checks.

4. **TDOA consistency check**. Each face should yield a TDOA estimate that converts to the same range (since all four faces are co-located within the buoy hull). Spread across the four estimates is a measure of leading-edge-picker noise — useful for setting confidence weights in the multilateration solver.

5. **Mechanically simple, electrically modular**. Four identical driver assemblies bolted radially to a central column. Either one shared driver IC that time-multiplexes the four coil pairs, or four independent class-D amps. Both are hobby-feasible.

### Encoding scheme options (research, not decision)

How does a face announce *which* face it is? Three options worth comparing:

| Scheme | Per-face slot | Refresh | Receiver complexity | SNR per face |
|---|---|---|---|---|
| **Time-division (TDM)** | One face fires at a time, ~25 ms chirp per face, round-robin | ~100 ms full rotation | Lowest (one matched filter at a time) | Highest (full chirp energy per face) |
| **Frequency-division (FDM)** | Each face on its own carrier sub-band within 10 Hz – 3 kHz (e.g. 100–800 Hz, 800–1500 Hz, 1500–2200 Hz, 2200–2900 Hz) | Continuous, all 4 always-on | Medium (4 parallel matched filters) | Reduced by 1/4 (per-face bandwidth) |
| **Code-division (CDMA)** | All 4 on same carrier with orthogonal codes (e.g. m-sequences, Gold codes, Zadoff-Chu) | Continuous, all 4 always-on | Highest (4 parallel correlators) | Full chirp energy per face, but cross-correlation noise |

TDM is the path of least resistance for an initial build. CDMA is the right answer if/when the mesh gets crowded. FDM sits in between but constrains each face to a narrower band → worse range resolution per face.

### Power-budget consequence

Sequential / TDM firing keeps peak power equal to a single-face configuration. Simultaneous firing (FDM or CDMA) multiplies peak power by 4 — for a Class A buoy on ~5 W solar trickle, this likely forces TDM unless the duty cycle is very low. Worth a separate sizing analysis.

### Buoy form factor

Four 100 mm discs mounted radially make the transducer ring roughly 4" PVC (100 mm) ID minimum if discs are flush, or 6" (150 mm) with some standoff. This is in family with the existing buoy build (see `../../../build/`); the discs replace patches of the hull wall. Smaller discs (60–80 mm) work in 2"-PVC buoys but trade low-frequency efficiency for compactness.

### Open design questions for Tier 1 (research, not decision)

- **Face count alternatives**: 3-face (120°, minimum for unambiguous bearing) is cheaper but loses redundancy; 6-face (60°) gives finer bearing but adjacent faces start coupling acoustically. 4-face is the sweet spot the user picked.
- **Disc-plane null exploitation**: since each face's null contains the adjacent faces, can two faces fire *simultaneously* (FDM or CDMA) without much mutual interference? Bench measurement needed.
- **Receiver-side multilateration math**: with 4 source faces × N mesh buoys, the TDOA observation count goes from N to 4N. Does this actually improve the multilateration CRLB, or does it just add redundant rows to an already-overdetermined system? Statistical analysis pending.
- **Calibration burden**: each face's absolute SPL and beam pattern must be characterized for amplitude-ratio bearing to work accurately. Per-buoy bench cal is feasible at hobby scale but adds production-time overhead.

### Tier 2: Mesh node with selectable-direction firing

Mesh nodes (Class C/A buoys, the cheap-and-many tier) carry multiple drivers but **fire only one at a time** — pointed at the local cluster's center of mass. The benefits:

- **Lower peak power** (single driver active, not four).
- **Lower average power** (most of the time the cluster is in a known direction; mesh node need only fire that face).
- **Cheaper BOM** (still 4 drivers physically, but only 1 amplifier needed if the amp output is switched between coil pairs).
- **Direction is dynamic**: as the buoy drifts and the cluster geometry changes, the "active face" rotates to track the cluster centroid. The cluster centroid is itself derivable from the Tier-1 anchor's broadcasts.

The mesh node still benefits from having 4 drivers (rather than 1) for two reasons:

1. **Bearing recovery from the anchor**: the mesh node listens to the anchor's 4-face transmission on its 4 receive hydrophones (separate from these transmit drivers). The amplitude ratios across its own hydrophones reveal its bearing relative to the anchor — which then tells it which transmit face to use to talk back.
2. **Fault tolerance**: if one face is damaged (debris, biofouling, electrical fault), another can be selected.

Whether mesh nodes always have 4 drivers or just 1 with a passive bearing receiver is a build-cost vs flexibility trade-off best decided after a Tier-1 prototype is built and characterized.

### Other comms layers (anticipated, not yet specified)

User direction: *"It also may have other ways to communicate on top of this."* The acoustic transmitter is one of several comms paths. Likely siblings in the comms stack:

- **WiFi / LoRa** above water surface (line-of-sight RF between buoy antennas).
- **Magnetic induction** intra-mooring vertical stack (sub-µs latency, see `../transmitter-options-catalog.md` § related potentials).
- **ESP-NOW** between Class-C buoys within ~200 m RF range.
- **Cellular (LTE/Cat-M)** on Tier 1 nodes for shore connectivity.

The acoustic transmitter handles the sub-surface inter-buoy timing/bearing/imaging traffic; the surface-RF layers handle bulk data uplink. Both layers are needed and they don't compete for the same medium.

## Cross-references in the corpus

- `../README.md` — projector-mma folder index; the original Wallin MMA stub is sibling research.
- `../transmitter-options-catalog.md` §A3 and §A3-bis — catalog entry that points to this card.
- `../../analysis/jmse-13-528-lt-sync.md` — LT-Sync time-sync protocol that this transmitter would carry as time-encoded modulation.
- `../../lt-sync-citations/analysis/[14]-zhou-2018-de-sync.md` — DE-Sync calibration math the time-sync layer inherits.
- `../../../build/` — buoy hardware build docs that any practical implementation has to integrate with.
- `../../../RANGING.md` — ranging design that consumes whatever transmitter wins.

## Status / verdict

- ✅✅ **ACQUIRED** at `pdfs/fonseca-alves-2012-rsi-rigid-plate-projector.pdf` (4 pp).
- ✅ **Verified** by direct read of all pages.
- ✅ **Patent ceased 2013-07-10 → public domain.** Design free to reproduce, adapt, and publish.
- 📌 **Open research questions documented above** — primarily (a) does the band extend usefully above 3 kHz on a scaled prototype, (b) what's the absolute SPL of a 10–15 cm-plate version, (c) does the chirp pulse-compression work as the theory predicts.
- 📌 **No commitment yet** — this is the strongest single-transducer candidate in the catalog, but the architectural question of "does clawft adopt this as primary transmitter" requires bench prototyping first.
