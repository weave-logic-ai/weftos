# Hanging-Hydrophone Optimal Depth — Research Stub

**Created**: 2026-05-11.
**Status**: 📋 **Survey only — no decisions.** Receive-side counterpart to the transmitter discussion in [`../projector-mma/`](../projector-mma/).

## Why this folder exists

User direction 2026-05-11: *"if a buoy had a hanging receiver, there is probably an optimal depth for receiving these even in ocean conditions."*

Correct intuition. There IS an optimum depth, it is NOT "as deep as the cable reaches," and it is frequency-dependent. This stub catalogs the physics drivers, gives band-by-band rules of thumb, and lays out the open research questions that gate a first-prototype depth choice.

## The four physics drivers

### 1. Lloyd mirror / sea-surface reflection

The sea surface is an almost-perfect pressure-release boundary — sound reflects with a **phase inversion**. A receiver at depth `h` sees the direct signal from a far source plus the surface-reflected signal arriving with extra path length and inverted phase. They interfere.

For a signal at frequency `f`, propagation speed `c`, source at grazing angle `θ`:

```
First null at:    h_null = c / (4 · f · sin θ)
First peak at:    h_peak = c / (2 · f · sin θ)
```

For near-horizontal arrivals (θ small, typical for far sources):
- **500 Hz, θ=5°**: first null at ~8.6 m, first peak at ~17.2 m
- **1 kHz, θ=5°**: first null at ~4.3 m, first peak at ~8.6 m
- **2 kHz, θ=5°**: first null at ~2.2 m, first peak at ~4.3 m

**Implication**: a hydrophone at 1–3 m depth lands in a Lloyd-mirror null for much of the band — terrible SNR. Below ~10 m depth the first null moves above you and the interference becomes peak-positive instead.

### 2. Surface duct / mixed-layer trapping

When sea-surface heating and wind-mixing produce a near-surface region where sound speed *increases* with depth (typical of summer / temperate / tropical conditions), sound rays bend upward — creating an upward-refracting waveguide called a **surface duct**.

Surface duct cutoff frequency:
```
f_c ≈ (c / H) · √(8 · Δc / c)
```
where `H` is duct depth (mixed-layer depth) and `Δc` is sound-speed difference between top and bottom of the duct.

For typical conditions (H = 50 m, Δc = 1 m/s): `f_c ≈ 100–200 Hz`.

- **Below `f_c`**: sound leaks out of the duct, propagation poor.
- **Above `f_c`**: sound trapped, propagates long distances inside the duct.

**Implication**: for the Fonseca-Alves band (10 Hz – 3 kHz), the lowest ~100 Hz may not propagate well in the surface duct. The upper band (200 Hz – 3 kHz) is ducted nicely if the receiver is **inside the duct** (above the thermocline).

### 3. Wave-noise depth profile

Surface wave noise dominates the 10–500 Hz band — same band as the Fonseca-Alves projector's most useful output. Wave noise spectrum level (Wenz curves) at the surface is typically 60–90 dB re 1 µPa²/Hz depending on sea state.

Wave noise decays with depth roughly as:
```
N(h) ≈ N₀ · exp(-h / d_decay)    where d_decay ≈ 5–15 m depending on f
```

So:
- 5 m down: noise ~ -5 to -10 dB vs surface.
- 20 m down: noise ~ -15 to -20 dB vs surface.
- 50 m down: noise ~ -30 dB or more vs surface.

**Implication**: deeper is quieter, asymptotic by ~50 m for the relevant band. The first 20 m gives most of the improvement.

### 4. Thermocline shadowing

Below the surface duct, the thermocline drops sound speed sharply. Sound rays from sources at moderate depth bend **downward** through the thermocline → a near-surface receiver placed **below** the thermocline sees a shadow zone for sources further than the convergence-zone range.

In shallow water this matters less because the bottom-bounce paths re-converge. In deep water the shadow zone can be 5–30 km wide.

**Implication**: in shallow coastal water (<200 m), a hydrophone at 20–50 m is below most wave noise but above the thermocline. In deep water (>500 m), there are two valid regimes: shallow (inside surface duct) or very deep (SOFAR channel at ~1000 m) — but the latter is well past our cable budget.

## Frequency-band-by-band guidance

| Receive band | Dominant constraint | Optimal depth range (coastal shelf) | Optimal depth (deep ocean, no SOFAR access) |
|---|---|---|---|
| 10–100 Hz | Wave noise (surface-dominant); below surface-duct cutoff | 30–80 m | 30–80 m |
| 100–500 Hz | Surface duct, Lloyd mirror | 10–40 m (inside duct) | 10–40 m |
| 500 Hz – 3 kHz | Lloyd mirror nulls near surface | 10–30 m | 10–30 m |
| 3–30 kHz | Wind/wave noise, absorption | 5–20 m | 5–20 m |
| >30 kHz | Absorption (short range anyway) | 2–10 m | 2–10 m |

For the **Fonseca-Alves 10 Hz – 3 kHz** band targeted by Tier-1 anchor broadcasts:

> **First-prototype recommendation: 20–30 m hanging depth.**

Rationale:
- Above the typical thermocline depth (so inside the surface duct when one exists).
- Below the Lloyd-mirror first null for the full operating band.
- Wave noise is ~15 dB below surface levels — useful but not yet asymptotic.
- Cable length is hobby-tier achievable (compare: 100 m cable is heavy, expensive, and a snag hazard).
- Survives in shallow coastal waters (>40 m bathymetry) without bottom contact.

## Site-condition variables that change the optimum

Before committing to a fixed-depth design, the following should be characterized for the deployment area:

| Variable | How to measure | Effect on optimal depth |
|---|---|---|
| **Mixed-layer depth** | CTD profile, or seasonal climatology from NOAA/Argo | Sets surface duct ceiling |
| **Thermocline depth** | Same | Sets surface duct floor |
| **Bathymetry** | Chart or single-beam echo | Sets cable-length upper bound |
| **Sea state distribution** | Local wind / wave records | Sets wave noise floor |
| **Shipping noise** | Hydrophone survey at deployment site | Sets traffic-noise floor |
| **Bioacoustic activity** | Same | Sets biological-noise floor (snapping shrimp etc.) |
| **Source SL of Tier-1 anchor** | Calibrated bench measurement (TBD) | Sets target SNR margin |

A reasonable first-instrumentation pass would be: hang the hydrophone on a winch-able cable, do a vertical survey 0–60 m at the deployment site recording ambient noise and a known test source, pick the depth with highest measured SNR. This calibration adds maybe a day per deployment site but front-loads the most impactful design uncertainty.

## Adaptive-depth variant (winchable)

If the optimal depth varies significantly with conditions (it does), a small winch on the buoy lets the hydrophone position adapt:

| Feature | Fixed-depth | Winchable |
|---|---|---|
| Cost | $5 cable + $5 fairing | + $30–60 winch motor + driver |
| Complexity | Trivial | Adds a moving part (failure mode) |
| Power | Zero | Brief draw during repositioning |
| Adaptability | None | Can chase the thermocline, dodge shipping, optimize for current conditions |
| Maintenance | Periodic inspection | Same, plus winch service |

A simple worm-gear stepper-motor winch (40 kg·cm torque, ~$20-30) can spool 50 m of Kevlar-reinforced cable in <2 min. Power per move is ~5 Wh; even a sub-daily reposition fits the Class A power budget.

**Open question**: is adaptive depth worth its complexity in practice, or is fixed 25 m + a known calibration "good enough"? Needs field test data.

## Vertical-stack hydrophone array (separate concept)

The hanging-receiver discussion above assumes ONE hydrophone at one depth. The project's existing build path actually uses a **vertical stack** (multiple hydrophones along the mooring line) — see `../../build/`. With a vertical stack:

- The stack itself spans a depth range (e.g., elements at 5, 15, 30, 50 m).
- The receive solution picks the *best* element per frequency band, OR coherently sums them with beamforming.
- This sidesteps the "single optimal depth" question entirely — the stack instruments the depth dimension and lets the post-processing pick the answer.
- See [`../projector-mma/transmitter-options-catalog.md`](../projector-mma/transmitter-options-catalog.md) "related potentials" section for the **magnetic-induction sync** option that ties a vertical stack into a single time-coherent receiver.

If the vertical stack works as the existing build doc envisions, the "hanging hydrophone optimal depth" question becomes "stack element spacing" — a much more interesting array-design problem.

## Open research questions

- **Lloyd-mirror null depth as a function of expected arrival angles** — narrow-vs-wide elevation distribution matters; need a propagation simulation (BELLHOP / KRAKEN) at the deployment site to model arrival angles. Existing analyses at `../analysis/bellhop-ray-tracing.md` and `../analysis/kraken-propagation.md` already cover the tools.
- **Surface duct stability** — coastal mixed layer can collapse in calm conditions or storm events; how much does the optimum shift?
- **Adaptive depth value vs cost** — is the winch overhead justified? Quantify the SNR gain from condition-tracking vs the cost.
- **Vertical stack vs single hydrophone trade** — at what range / SNR does the stack pay off vs a single optimal-depth element?
- **Stack element spacing optimization** — for the existing build's vertical stack, what are the element depths that maximize cross-band coverage?

## Cross-references in the corpus

- `../analysis/wenz-ambient-noise.md` — wave-noise depth profile.
- `../analysis/urick-sonar-equation.md` — Lloyd mirror, link-budget math.
- `../analysis/thermocline-film.md` — thermocline behavior.
- `../analysis/bellhop-ray-tracing.md` — site-specific ray-tracing for arrival angle prediction.
- `../analysis/kraken-propagation.md` — normal-mode propagation in surface duct.
- `../analysis/ssp-from-ranging.md` — sound speed profile inversion.
- `../projector-mma/analysis/fonseca-alves-2012-rigid-plate.md` — transmitter side of the link budget.
- `../../build/` — existing vertical-stack hydrophone build docs.
- `../../RANGING.md` — ranging design that consumes received signals.

## Quick first-prototype takeaway

For an initial Fonseca-Alves Tier-1 link test:

> **Single hanging hydrophone at 25 m depth, on a 35 m cable with strain relief.**

Reasoning: below Lloyd-mirror first null across the band; inside typical surface duct; ~15 dB wave-noise reduction vs surface; well above any reasonable coastal thermocline; cable length feasible. Calibrate with a vertical survey on first deployment; revise if data warrant.
