# Underwater Acoustic Transmitter Options — Research Catalog

**Created**: 2026-05-11.
**Status**: 📋 **Survey only — no decisions, no commitments.** This is the brainstorm-capture pass; each entry needs follow-up research before any can be quoted as a buildable option.

## Architectural framing (user direction 2026-05-11)

User asked to write down *all* the ways to generate underwater acoustic pulses / waveforms for time-encoded broadcast. Two relevant context notes that shape this catalog:

1. **"Land node" variant** — instead of every buoy carrying its own transmitter, a single "land" (shore-station) node with an attachable hull piece (wired tether → connects to the buoy when in port) can carry an expensive high-grade transmitter. "A single one is more than sufficient." Useful if any of the candidates below turn out to be cost-prohibitive or too bulky for the in-water buoy.
2. **"Cheap, lots of nodes" variant** — alternatively, if a transmitter method lands cheaply enough, each buoy carries one and the mesh becomes symmetric. Both variants stay live until research shows which class of method we're actually working with.

## How to read this catalog

Each entry has:
- **Mechanism** — the physics
- **Band / bandwidth** — useful frequency range
- **SPL ballpark** — order-of-magnitude source-level at 1 m, where known
- **Cost / size posture** — hobby-tier feasibility flag
- **Literature pointer** — where to look first; not exhaustive

---

## A. Electromagnetic linear-stroke drivers (the speaker-class family)

### A1. Moving-magnet actuator (MMA) — Wallin 2017
- **Mechanism**: Permanent magnet "trapped" inside a stationary coil, modulated AC current produces axial Lorentz force on the magnet, magnet drives a piston / piston pumps water.
- **Band**: 1 Hz – ~10 kHz (broad, single transducer).
- **SPL**: 125 dB @ 1 Hz, 180 dB @ 30 Hz (Wallin 2017 numbers).
- **Cost / size**: Off-the-shelf Bose / industrial actuator $200–500; small Tang Band / Dayton subwoofer drivers $30–100 if hobby-tier acceptable. Underwater packaging (piston + bellows + oil-fill) is the cost driver.
- **Pros**: Broad band in one driver. Symmetric mesh feasible if low-cost variant works.
- **Cons**: Form factor; pressure compensation under stroke.
- **Lit pointer**: Wallin 2017 URI thesis ([already filed in this folder](./README.md)). Also: NUWC USRD Leesburg work; H2W Technologies catalog; Bose ProFlex datasheet.

### A2. Voice coil / moving-coil projector
- **Mechanism**: Coil in fixed magnetic-gap field, current produces force on the coil. Reverse of A1.
- **Band**: 10 Hz – ~30 kHz typical.
- **SPL**: ~150–170 dB depending on coil power and piston area.
- **Cost / size**: Commodity subwoofer voice coils $20–100; full units (drivers as sold) $50–500.
- **Pros**: Same as A1, mature consumer-electronics technology.
- **Cons**: Lower stroke than MMA at very low frequencies (<10 Hz); the moving coil dissipates more I²R than a moving magnet of equal force.
- **Lit pointer**: Hunt 1954 *Electroacoustics* still the canonical reference. Sherman & Butler 2007 *Transducers and Arrays for Underwater Sound* chapter on moving-coil projectors.

### A3. Fonseca-Alves rigid-plate magnetically-suspended actuator (the build candidate)
- **Mechanism**: A permanent magnet is embedded in a cylindrical assembly capped by a rigid plate. **The magnet has no mechanical support — no bearings, no bushings, no spider, no flexure.** Two coil pairs provide all positional control:
  - **First coil pair (excitation)**: driven with variable AC current → drives the magnet axially → drives the rigid plate → radiates sound.
  - **Second coil pair (equilibrium / damping)**: DC-biased → magnetic spring that centers the magnet and replaces the elastic suspension found in conventional voice coils.
  - The whole device is **flooded with water** — no compressible air anywhere, so operation is depth-independent. Pressure compensation is automatic.
- **Band**: 5 Hz – 50 kHz claimed (broad), 15 Hz – 5 kHz refined (Claim 11 of patent).
- **SPL**: Not specifically quoted in patent. RSI paper presumably has it (paywalled). User direction puts this in the 150–170 dB band.
- **Cost / size**: Rigid plate ~10–15 cm (Al / Mg / composite); neodymium ring or cylindrical magnet; 4 air-core coils on 3D-printed bobbins; HDPE/PVC cylindrical housing. Estimated $50–200 BOM.
- **Pros**:
  - **Patent has lapsed (ceased 2013-07-10) — design is public domain.** Free to build, modify, and publish.
  - No elastic suspension → no fatigue failure mode under hydrostatic pressure.
  - Depth-independent (no air cavity).
  - Wider band than any other electromechanical option in this catalog (5 Hz to 50 kHz in one device).
  - Mechanically minimal — no precision-machined moving parts, just a magnet floating in coils.
- **Cons**:
  - DC equilibrium coil draws standing current → adds to the always-on power budget. Magnitude depends on magnet weight vs. coil-pair geometry; worth a back-of-envelope before committing.
  - Without mechanical centering, the magnet drifts if DC current drops or if the device tilts → may need a startup self-centering routine.
  - SPL claim needs verification against the RSI paper (user-drop) or our own bench prototype.
- **Lit pointer**:
  - Fonseca, P. J. & Maia Alves, J. *A new concept in underwater high fidelity low frequency sound generation.* Review of Scientific Instruments **83**(5):055007, May 2012. doi:[10.1063/1.4717680](https://doi.org/10.1063/1.4717680). **PAYWALLED (Unpaywall is_oa=false); needs user-drop or institutional access.** PDF URL: https://pubs.aip.org/aip/rsi/article-pdf/doi/10.1063/1.4717680/9950910/055007_1_online.pdf
  - Patent: [WO2012095780A1](https://patents.google.com/patent/WO2012095780A1/en) — "Underwater sound generator." Inventors Fonseca + Maia Alves, assignee Universidade de Lisboa, **legal status CEASED 2013-07-10**. Full technical extraction already captured in `analysis/fonseca-alves-2012-rigid-plate.md` (this folder).
  - Related hobby-tier predecessor: de Jong, K.; Schulte, G.; Heubel, K.U. *The noise egg: a cheap and simple device to produce low-frequency underwater noise for laboratory and field experiments.* Methods Ecol. Evol. **8**(2):268-274, 2017. doi:[10.1111/2041-210x.12653](https://doi.org/10.1111/2041-210x.12653). €10/unit. Less sophisticated (uses a vibration motor, not magnetic suspension) but a useful sanity-check that hobby-tier underwater sources at low cost are a working real-world thing.
- **Build readiness**: Highest of any A-class option. Patent gives complete mechanical drawings; the electronics are simple (one variable-current amplifier + one DC current source). This is the most "buildable" entry in the catalog.

#### A3-bis: Why this could be the PRIMARY transmitter, not just a beacon

User direction 2026-05-11 (later in same session): *"This may be a VERY good way to build the transmitters; these low frequency waves can be swept across frequencies and get very good imaging from them I believe because it will be very clean signal, no resonance."*

This is the architectural reframe worth tracking carefully. The point: a transmitter with **no mechanical resonance** has a flat phase response across its band, which means a swept-frequency (chirp / LFM) pulse out of it is **clean** — the matched filter at the receiver gets the full theoretical pulse-compression gain instead of the smeared / ringy response you get from a resonant piezo. So this isn't just "a time beacon"; it's potentially:

1. **The chirp ranging transmitter**, replacing the 38 kHz piezo entirely. Range resolution from a 4.985 kHz sweep (15 Hz–5 kHz refined band) is Δr = c/(2B) = 1500/(2·4985) ≈ **0.15 m theoretical**, comparable to the 38 kHz piezo system.
2. **The continuous time/ID beacon**, time-multiplexed with the chirp pulses (chirp burst → quiet listen window → beacon CW → repeat).
3. **A sub-bottom profiler** at the lowest end of the band — sub-1 kHz energy penetrates seabed sediment by tens of meters, matching commercial CHIRP sub-bottom systems (Edgetech 3300, GeoAcoustics) which use exactly this 500 Hz–24 kHz band.

The propagation advantage of the low band is enormous: absorption at 500 Hz is ~0.06 dB/km vs ~5 dB/km at 38 kHz — for the same source level, the low-band signal travels ~80× further before noise-floor limited. The chirp-band piezo's range is currently the dominant constraint on inter-buoy spacing in the mesh design; moving to this transmitter could relax that constraint by an order of magnitude.

**Open questions that need research, not commitment:**

- Does the SPL claim (150–170 dB band) hold across the *full* sweep, or does it dip at band extremes? The RSI paper's frequency response plot is the load-bearing source for this — needs user-drop or institutional access.
- What's the actual phase linearity / group-delay flatness across the band? Patent says "high fidelity" but doesn't give numbers. Bench characterisation would settle it.
- What does the receiver side look like if we move primary ranging to sub-5 kHz? Current hydrophones are sized for kHz–tens-of-kHz; broadband hydrophones cost more or have lower sensitivity. Trade-off needs sketching.
- How does the rigid-plate radiator's directivity compare to the piezo? At λ = 30 cm (5 kHz) the plate is roughly λ/2 → near-omnidirectional. At λ = 100 m (15 Hz) the plate is λ/700 → also near-omni. Probably no beamforming gain from a single element; but the mesh-of-mics gives spatial gain at the receive side anyway.
- Are there nonlinearities at high drive? Magnetic actuators saturate when the magnet exits the linear coil-field region; matters for clean chirp generation.

### A4. Magnetostrictive transducer (Tonpilz-style)
- **Mechanism**: A magnetostrictive rod (Terfenol-D, Galfenol, nickel) elongates under magnetic field. Coupled to a head mass and tail mass — the classic Tonpilz longitudinal-vibrator geometry.
- **Band**: 100 Hz – 30 kHz, mostly narrowband around mechanical resonance.
- **SPL**: 180–200 dB possible at resonance; the high-grade Navy stuff.
- **Cost / size**: Expensive (Terfenol-D is $$$). Probably out of hobby budget unless using nickel.
- **Pros**: Robust, well-characterized.
- **Cons**: Cost, narrowband, needs bias magnet.
- **Lit pointer**: Sherman & Butler ch. 5. Olabi & Grunwald 2008 review on Terfenol-D actuators.

### A5. Electrostrictive transducer
- **Mechanism**: Electrostrictive ceramic (PMN-PT, PIN-PMN-PT) elongates under E-field; cousin of piezoelectric but quadratic in field (so needs bias).
- **Band**: kHz–MHz.
- **SPL**: Comparable to piezo at high freq, better linearity.
- **Pros**: Higher coupling than PZT at room temp.
- **Cons**: Bias requirement; ceramic cost.
- **Lit pointer**: Park & Shrout 1997 *J. Appl. Phys.* on relaxor PMN-PT; modern Navy Tonpilz papers.

---

## B. Cavitation / phase-change methods (the impulsive-source family)

### B1. Pistol-shrimp-style cavitation snap
- **Mechanism**: Rapid mechanical closure of a chamber drives a water jet at speeds >25 m/s, the jet's low-pressure region collapses into a vapor cavity, the cavity collapse re-emits as a broadband acoustic shock.
- **Band**: Broadband impulse, dominant energy 2–200 kHz; peak in the 5–50 kHz range.
- **SPL**: Pistol shrimp itself measured at ~200 dB re 1 µPa peak @ 1 cm; engineered analog at distance unknown but high.
- **Cost / size**: Mechanically intricate (the shrimp's claw geometry has been studied as a fast water-jet generator); could be replicated with a solenoid-driven spring-loaded paddle.
- **Pros**: Cheap, all-mechanical, broadband impulse. Time-encoded by gating multiple snaps.
- **Cons**: One-shot per cocking cycle; recharge time may limit pulse rate. Mechanical wear.
- **Lit pointer**: Versluis et al. 2000 *Science* "How snapping shrimp snap"; Lohse, Schmitz, Versluis 2001 *Nature*; commercial: not widely productized for sonar.

### B2. Spark-gap source (sparker)
- **Mechanism**: High-voltage capacitor discharges across an electrode pair in seawater, plasma channel forms, plasma collapse radiates a broadband shock.
- **Band**: 100 Hz – 10 kHz typical (dominant), tail to MHz.
- **SPL**: 200–230 dB peak @ 1 m (commercial seismic sparkers).
- **Cost / size**: Capacitor bank + HV switch + electrodes. Hobby-feasible at lower energy.
- **Pros**: Very loud, broadband impulse, electrically tunable rate.
- **Cons**: Electrodes erode; high voltage = safety concern; significant power per shot.
- **Lit pointer**: Applied Acoustics CSP-D series datasheets; AA Boomer/Sparker manual. Academic: Caulfield 1962 *J. Acoust. Soc. Am.*; Mosher 1999.

### B3. Laser-induced cavitation
- **Mechanism**: Pulsed laser focuses into water, optical breakdown produces a plasma bubble, bubble collapse radiates.
- **Cost / size**: Expensive (pulsed Nd:YAG); not hobby-tier.
- **Lit pointer**: Vogel & Lauterborn 1988 *J. Acoust. Soc. Am.* Skip unless lab access available.

---

## C. Pneumatic / fluid-release methods (the air-gun family)

### C1. Air gun
- **Mechanism**: Compressed air (typically 2000–3000 psi) released through a fast-acting valve into water, the expanding-then-collapsing air bubble radiates.
- **Band**: 5–500 Hz dominant.
- **SPL**: 215–230 dB peak (industrial seismic).
- **Cost / size**: Industrial — Sercel G/G·I gun, Bolt 1500LL. Hobby variant: paintball air system + custom valve?
- **Pros**: Very loud at very low frequency.
- **Cons**: Needs compressed-air tank → tether or onboard compressor; environmental impact (this is the marine-mammal-controversy source).
- **Lit pointer**: Caldwell & Dragoset 2000 *Leading Edge*; Sercel air-gun manuals.

### C2. Pneumatic whistle / valve oscillator
- **Mechanism**: Compressed air pulsed through a Helmholtz resonator or whistle aperture, audio-frequency tone generated.
- **Band**: 100 Hz – 5 kHz tunable by chamber dimensions.
- **SPL**: Lower than air-gun (~150–170 dB @ 1m possible).
- **Pros**: Cheaper than air-gun; continuous CW tone possible.
- **Cons**: Same compressed-air supply problem.
- **Lit pointer**: Less common in literature; search for "underwater pneumatic siren" — some military WWII-era references.

### C3. Pulse-jet / propellant-charge
- **Mechanism**: Small chemical charge or electrolytic gas pulse, single shot.
- **Cost / size**: Hobby-feasible but consumable.
- **Lit pointer**: USGS minisparker variants; ELECTROLYTIC-SOURCE papers from the 1970s.

---

## D. Mechanical-impact methods (the hammer family)

### D1. Solenoid striker / mechanical hammer on pole
- **Mechanism**: Electromechanical solenoid drives a hammer mass to strike a metal pole, plate, or the buoy hull. Strike radiates as a broadband impulse into water.
- **Band**: Broadband impulse, dominant energy depends on plate stiffness/area (typically 100 Hz – 5 kHz).
- **SPL**: 140–170 dB @ 1 m feasible from a 24V/several-watt solenoid.
- **Cost / size**: Very cheap. Solenoid $5–20; plate is structural.
- **Pros**: Cheapest entry on this catalog. Mechanically robust. Time-encoded by pulse train.
- **Cons**: Wears the strike surface; not great for fine waveform shaping; broadband impulse, can't be modulated for a carrier wave.
- **Lit pointer**: Geophone-source / Betsy-gun literature; OBSEA seafloor observatory uses simple solenoid clickers for ranging.

### D2. ERM (eccentric rotating mass) vibrator
- **Mechanism**: Off-center mass on a motor shaft, rotation produces an oscillating radial force on the housing. Same physics as phone-vibrator motors at a larger scale.
- **Band**: Determined by motor RPM; typically 10–200 Hz.
- **SPL**: Low–moderate (depends on housing coupling to water).
- **Cost / size**: Trivial; consumer drone motors are exactly this.
- **Pros**: Cheapest possible CW low-frequency source; near-zero electronics.
- **Cons**: Narrowband (single tone tied to RPM); efficiency depends on housing being a good radiator.
- **Lit pointer**: No academic literature found — this would be a novel-application angle. Search "rotating eccentric underwater" turns up vibratory pile-driving and seabed compaction work (very different scale).

### D3. ERM in a resonant chamber
- **Mechanism**: D2 driver coupled into a Helmholtz resonator (chamber + aperture). Resonator amplifies the chosen tone, rejects the others. User-noted variant.
- **Band**: Single resonance, Q tunable by aperture / chamber ratio.
- **SPL**: Significant amplification possible — Helmholtz Q can reach 20–50 in well-tuned chambers, so a 100 dB source can drive a 130–140 dB output.
- **Pros**: Cheap; tone is exactly the broadcast frequency we want.
- **Cons**: Single frequency per chamber; chamber size is set by target frequency (large for VLF).
- **Lit pointer**: Helmholtz resonator literature is huge; under water specifically — Strasberg 1953 *J. Acoust. Soc. Am.* on submerged Helmholtz. Apply this to D2 = open research.

### D4. LRA (linear resonant actuator) / haptic actuator scaled up
- **Mechanism**: Spring-mass-coil system driven at its mechanical resonance; same physics as a phone haptic-buzzer.
- **Band**: Sharp resonance, typically 100–300 Hz in haptic devices; could be designed for any band.
- **SPL**: Moderate; better than ERM at the same power level.
- **Cost / size**: Haptic LRAs are $1–5; scaled versions for underwater would be custom.
- **Pros**: Efficient at resonance; quieter on power than ERM.
- **Cons**: Narrowband.
- **Lit pointer**: Haptics literature (Choi & Kuchenbecker 2013 review); underwater scaling is open research.

### D5. Falling-weight / pendulum strike
- **Mechanism**: A mass is released and falls under gravity to strike a plate or seabed.
- **Cost / size**: Mechanical only.
- **Pros**: Very high impulse energy from cheap parts.
- **Cons**: Slow recharge; not directly time-modulatable.
- **Lit pointer**: Seismic-survey "weight drop" sources; Betsy gun.

---

## E. Resonant-chamber methods (acoustic amplifiers, not standalone)

### E1. Submerged Helmholtz resonator with small electromagnetic driver
- See D3. The "amplifier" piece is universal — pair with any of A1/A2/D1/D4 to get more SPL out of less driver power.
- **Lit pointer**: Strasberg 1953; modern restatements in van der Heyden & Heesterman 2014 *J. Sound Vib.*

### E2. Tuning-fork / submerged-bell sources
- **Mechanism**: Mechanically struck or motor-driven oscillator; resonant body radiates.
- **Cost / size**: Cheap if found surplus.
- **Cons**: Highly Q'd, narrowband, hard to time-modulate.
- **Lit pointer**: Hardly studied in modern literature; historical interest only.

---

## F. Flextensional and cymbal transducers (Navy mid-tier)

### F1. Class IV / Class V flextensional
- **Mechanism**: Stack of piezo elements drives a curved metal shell radially in-and-out; the shell radiates broadside.
- **Band**: 500 Hz – 5 kHz typical, narrowband.
- **SPL**: 200+ dB possible.
- **Cost / size**: Expensive (purpose-built Navy hardware).
- **Lit pointer**: Sherman & Butler ch. 6. Recent: Le Letty et al. 2002 *Sensors and Actuators A*.

### F2. Cymbal transducer (miniature flextensional)
- **Mechanism**: Two cymbal-shaped metal caps over a small piezo disk; converts thickness vibration into much-larger flexural radial motion.
- **Band**: 1–30 kHz typical.
- **SPL**: Moderate.
- **Cost / size**: Built from off-the-shelf piezo disks + machined caps.
- **Pros**: Hobby-feasible; small.
- **Cons**: Lower SPL than Tonpilz or MMA.
- **Lit pointer**: Newnham et al. 1993 *J. Mater. Sci.*; review in Tressler et al. 1999 *J. Electroceram.*

---

## G. Hydraulic / hydrodynamic methods

### G1. Modulated water-jet / oscillating valve
- **Mechanism**: Pump produces continuous water flow, valve modulates the flow at acoustic frequencies; the modulated jet radiates.
- **Band**: <1 kHz typically.
- **Pros**: Could be very loud at very low frequency.
- **Cons**: Power-hungry; mechanically complex.
- **Lit pointer**: Helle 1995 *Ultrasonics Symp.* on modulated-flow sources.

### G2. Vortex shedding from oscillating bluff body
- **Mechanism**: Body in flow sheds vortices at the Strouhal frequency; if the body or flow is modulated, the shedding modulates accordingly.
- **Niche**: Probably not a practical clawft source but interesting for biomimetic angles.

---

## H. Thermal / plasma / exotic

### H1. Thermoacoustic source
- **Mechanism**: Modulated heating element produces modulated bubble or expansion → sound.
- **Band**: Low frequency.
- **SPL**: Low.
- **Niche**: Research curiosity.
- **Lit pointer**: Backhaus & Swift 2000 *Nature* on thermoacoustic engines (different application but same physics).

### H2. Plasma source (continuous arc in water)
- See B2 (sparker). For CW operation rather than impulse, plasma sources have been demonstrated but are inefficient.

---

## Quick-reference frequency-band coverage map

| Method | 1 Hz | 10 Hz | 100 Hz | 1 kHz | 10 kHz | 100 kHz |
|---|---|---|---|---|---|---|
| A1 MMA Wallin-style | ●●● | ●●● | ●●● | ●●● | ●● | – |
| A2 Voice coil (sub) | – | ●● | ●●● | ●●● | ●●● | – |
| A3 Rigid-plate (boomer) | – | – | ●● | ●●● | ●● | – |
| A4 Magnetostrictive | – | – | – | ●●● | ●●● | ●● |
| B1 Pistol-shrimp click | – | – | ● | ●● | ●●● | ●●● |
| B2 Sparker | – | – | ●●● | ●●● | ●● | ● |
| C1 Air gun | ● | ●●● | ●●● | – | – | – |
| C2 Pneumatic whistle | – | – | ●● | ●●● | ●● | – |
| D1 Solenoid striker | – | – | ●● | ●●● | ●● | – |
| D2 ERM | – | ●● | ●● | – | – | – |
| D3 ERM in Helmholtz | – | ●●● | ●●● | – | – | – |
| F1 Flextensional | – | – | ●● | ●●● | ●● | – |
| F2 Cymbal | – | – | – | ●●● | ●●● | ●● |
| Piezo (baseline reference) | – | – | – | ● | ●●● | ●●● |

●●● = strong native fit ● = possible with effort – = not practical

---

## Cross-cutting research questions

These apply to nearly all the above:

1. **Pressure compensation at hobby depths (0–50 m)** — every method needs a way to equalize hydrostatic pressure across the moving / radiating element. Oil-fill, bellows, and air-spring methods all viable; choice affects bandwidth and SPL.
2. **Time-encoding modulation** — for each method, what's the cleanest way to embed UTC time + node ID? BPSK on a CW carrier (A/F methods), pulse-position modulation on impulse (B/D methods), or burst-rate modulation (C methods)?
3. **Multi-node carrier slot allocation** — if each node carries a low-cost transmitter, how do we assign non-overlapping frequency slots so receivers can discriminate sources?
4. **Power budget** — Class A buoys have ~5 W solar trickle; Class C are battery-only. The CW vs. pulsed trade matters more on Class C.
5. **Marine-mammal impact** — air-gun (C1) and sparker (B2) are loud enough to be regulated near critical habitat. Hobby use of these is a real issue. The lower-SPL methods (D1, D2, A2-low) avoid this.

## Next research steps (when this gets prioritized)

- Compile a literature search per method letter, file under `papers/transmitters/<method>/` with the same per-paper analysis convention as the rest of the corpus.
- Build a parametric SPL-vs-power-vs-cost spreadsheet for the top 3 candidates (likely A1 MMA, A2 voice coil, D3 ERM-Helmholtz).
- Identify the off-the-shelf parts list for the cheapest viable approach (D3 hypothesis: $10 ERM motor + 3D-printed Helmholtz chamber + waterproof housing).
- Decide whether to prototype any of these or wait for the architecture review.

## Cross-references

- [`README.md`](./README.md) — MMA-specific deep dive (A1) and the three-tier sync hierarchy framing.
- `../analysis/jmse-13-528-lt-sync.md` — LT-Sync protocol that any of these transmitters would carry.
- `../../build/` — buoy hardware build docs; whichever transmitter wins integrates into this.
- `../../RANGING.md` — ranging design that consumes the time-encoded broadcast.
