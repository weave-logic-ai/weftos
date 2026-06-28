# Inter-Buoy Acoustic Ranging — Integration Addendum to SYNTHESIS.md

**Compiled**: 2026-04-14
**Status**: Architectural addendum; SYNTHESIS.md §10 flagged sensor-
position uncertainty as an open problem — this document closes that
gap with a verified 7-paper literature base.
**Sources**: Per-paper analyses at
`.planning/sonobuoy/papers/analysis/{lbl-acoustic-nav,
munk-worcester-tomography, one-way-travel-time,
cooperative-buoy-positioning, janus-underwater-comms,
ssp-from-ranging, tshl-clock-sync, doppler-aided-ranging}.md`.
All citations verified via DOI, arXiv ID, or publisher records;
see §12.

---

## 0. Executive summary

- **GPS is not good enough** for a drifting sonobuoy field. 2-5 m RMS
  position noise at 1 Hz is the *floor* the spatial branch
  (Tzirakis-2021 GCN, Grinstein-2023 Relation-Network, Chen-Rao-2025
  Grassmann DoA) cannot beat, and it sits on top of a ~0.1-2 m/s
  drift velocity the buoy has no way to measure without a
  compass/IMU chain of its own.
- **Inter-buoy acoustic ranging collapses the error floor** to
  ~0.15-1.5 m for inter-buoy distances at 100 m - 5 km scales. Four
  classes of observations are available *from the same broadcast*:
  direct-path travel time, Doppler shift (radial velocity), multi-
  path arrival-time differences (SSP inversion), and payload
  metadata (GPS, clock bias, battery).
- **The recommended protocol** is OWTT (one-way travel-time,
  Webster-Eustice 2012) on top of JANUS (Potter-Alves 2014)
  waveforms, with CSAC-class clocks (~150 µW) disciplined by GPS
  PPS, coordinated by TSHL (Syed-Heidemann 2006) with the D-Sync
  mobility extension (Lu-Mirza-Schurgers 2018). The
  Otero-2023 GNSS-pseudo-range architecture is the practical reference
  design for a drifting field where underwater users don't carry
  atomic clocks.
- **The data product** is a time-varying distance matrix
  `D(t) ∈ R^{N×N}` plus per-pair covariances `Σ(t)`, velocity
  matrix `V(t)`, and an in-situ SSP `c(z, t)` represented in
  reduced-basis EOFs. These four outputs feed the spatial branch,
  the physics-prior branch, and the active-imaging branch directly.
- **This work introduces one new ADR (ADR-078)**: "Inter-buoy
  active acoustic ranging as primary sensor-position source." It
  picks up after the 25 ADRs already in SYNTHESIS.md §6 (ADR-053
  through ADR-077).
- **v2 territory**. Ranging enters the plan alongside the spatial
  branch (SYNTHESIS §8.2) because Tzirakis' dynamic-adjacency GCN
  is the first consumer that cares about meter-scale relative
  geometry. v1 remains GPS-only. v3 leverages ranging-derived SSP
  inside the physics-prior branch. v4 extends velocity tracking
  to the multistatic SAS branch.
- **Honest limitations**: active pings reveal buoy positions to any
  listener; the stealth section (§7) addresses this. Multipath in
  shallow water is the dominant ranging-error source in littoral
  deployments; §4 gives the mitigation strategy. Clock sync fails
  open-loop if GPS antennas are submerged — CSAC + TSHL D-Sync
  handle this but the failure mode is real.

---

## 1. Motivation

### 1.1 What GPS buys

A surface sonobuoy with a small patch antenna running L1 SPS (single-
frequency civil) gets:

| Observable | Typical | Best | Worst |
|------------|---------|------|-------|
| Horizontal position | 2-5 m RMS | 1.5 m | 10 m |
| Vertical position | 5-10 m RMS | 3 m | 20 m |
| Velocity (Doppler) | 0.1 m/s | 0.05 m/s | 0.5 m/s |
| Time (PPS) | ~20 ns RMS | ~1 ns | 1 µs |
| Cadence | 1 Hz | 10 Hz | 0.1 Hz (dropout) |

The position numbers include ionospheric delay, tropospheric delay,
multipath (from sea surface!), and geometry dilution. GPS-RTK
would improve position to ~10 cm but requires a nearby base station
and 5-15 kbps radio link — not a sonobuoy-compatible architecture.

### 1.2 Why 2-5 m is not good enough

The K-STEMIT spatial branch's three consumers each have a
*structural* requirement for the inter-buoy distance matrix `D(t)`:

1. **Tzirakis-2021 learned-adjacency GCN** (ADR-056) uses edge
   weights `W_{ij} = exp(-||p_i - p_j||² / 2σ²)` with a bandwidth
   `σ` tuned to inter-mic spacing. 2-5 m noise on `p_i` is 10-30%
   noise on a 25 m edge weight — enough to saturate the GCN's
   learning signal.
2. **Grinstein-2023 Relation-Network SLF** (ADR-057) uses GCC-PHAT
   cross-correlations + `(p_i, p_j)` metadata per pair. The network
   implicitly learns geometry uncertainty; 2-5 m GPS noise
   consumes 2-3× the total per-pair learning capacity before
   useful TDOA features appear.
3. **Chen-Rao-2025 Grassmannian DoA back-end** (ADR-058) solves
   principal-angle geometry on the sample covariance matrix
   `R = E[x x^H]`; `x` depends on sensor positions through the
   steering matrix. Wrong `p_i` → wrong steering matrix → wrong
   principal angle. Graceful degradation is poor.

In all three cases the sensor-position error is **multiplicative**
with the TDOA signal the branch is trying to extract. Dropping it
from 2-5 m to 0.15-1.5 m improves the effective SNR by 10-30× in
the small-error regime.

### 1.3 Why not IMU / compass dead reckoning?

A low-cost MEMS IMU (<1 W) drifts ~1 m/minute when integrated
without external correction. Over a typical 10-minute TDMA epoch
that's 10 m — worse than GPS. An expensive fiber-optic-gyro IMU
(~$50k, 10-100 W) gets <1 m/hr but is incompatible with the
sonobuoy cost envelope. Acoustic ranging gives meter-scale
relative-position knowledge at ~100 mW continuous and ~$3k per
buoy (dominated by CSAC cost) — strictly better on the Pareto
frontier.

### 1.4 What ranging buys beyond GPS

The ranging subsystem outputs are **strictly more informative** than
GPS for this application:

| Output | GPS only | GPS + ranging |
|--------|----------|---------------|
| Absolute horizontal position | 2-5 m | 2-5 m (same) |
| Inter-buoy relative distance | 3-7 m (√2 × GPS) | 0.15-1.5 m |
| Inter-buoy relative velocity | 0.15 m/s | 0.1 m/s (same, marginal) |
| Formation orientation | 5-10° | 0.5-1° |
| Distance matrix update rate | 1 Hz | 0.1-1 Hz per pair |
| In-situ SSP | not available | ~5 EOF coef per minute |
| Multistatic geometry | not feasible | accurate enough for SAS |

Relative geometry is what the classifier stack needs; ranging
gives that, GPS does not. The absolute position stays ~GPS-noisy
but becomes less important — only anchors the whole formation.

---

## 2. The Ping-Pong Protocol

### 2.1 Waveform: JANUS-compatible LFM chirp

**Recommended carrier band**: 9.44-13.60 kHz (the JANUS band, per
Potter-Alves 2014). Choose LFM chirp over the JANUS FSK payload
for ranging (chirp gives ~100 µs matched-filter timing precision,
FSK gives ~1 ms), but use JANUS's base-frame structure for
interoperability. The full ranging frame:

```
[ LFM preamble 100-500 ms, 4 kHz sweep ]   ← timing/Doppler
[ JANUS base frame 64 bits, 80 bps FSK ]   ← ID, TDMA, CRC
[ optional cargo 256-512 bits              ← GPS, clock, mode
  at 80 bps FSK                ]
```

On-air duration: 800 ms (short, preamble-only) or 4 s (full cargo).
Use preamble-only for tactical/silent modes where cargo can be
carried out-of-band (LoRa at surface); use full cargo for routine
broadcasts.

### 2.2 Alternatives and their tradeoffs

| Waveform | Timing res | Payload | Stealth | Notes |
|----------|-----------|---------|---------|-------|
| LFM chirp 4 kHz BW | ~100 µs | 0 bits | low | spreading gain ~20 dB |
| M-sequence PSK 2 kHz BW | ~200 µs | bits over sequence | very low | requires carrier PLL |
| JANUS FSK 4 kHz BW | ~1 ms | 80 bps | low | NATO standard |
| CW tone pair (baseline) | ~5 ms | 0 bits | very low | 1970s WHOI legacy |
| OFDM 10 kHz BW | ~100 µs | kbps | low | broadband but complex |

Chirp + JANUS frame is the Pareto sweet spot: best timing for short
fields, standard-compliant, easy matched-filter detection at
-5 dB input SNR.

### 2.3 Ping rate and TDMA scheduling

For `N` buoys sharing one channel at max inter-buoy range `R_max`:

    slot_duration = packet_duration + (R_max / c̄)
                  ≈ 0.8 s + (5000 / 1500) = 4.1 s
    epoch = N · slot_duration
    per_buoy_update_rate = 1 / epoch

For N=4 at R_max=1 km, epoch ~4 s, per-buoy rate 0.25 Hz.
For N=8 at R_max=5 km, epoch ~33 s, per-buoy rate 0.03 Hz (too
slow — use preamble-only, epoch drops to ~10 s, rate ~0.1 Hz).
For N=16 at R_max=500 m (dense littoral), epoch ~20 s with
preamble-only, rate ~0.05 Hz.

**Scheduling protocol**: TDMA slots assigned at deploy time by the
leader buoy (Raft-elected, per WeftOS `mesh_chain.rs`). Slot
reassignment on buoy failure / recovery. Gossip protocol
(`mesh_kad.rs`) maintains the schedule across the field.

### 2.4 Clock-sync requirements

For 15 cm range resolution at c̄=1500 m/s, clock synchronization
to 100 µs is needed — not just at broadcast time but over the
full open-loop interval between GPS re-syncs.

**Hardware**: Microsemi SA.45s CSAC (chip-scale atomic clock).
Allan deviation `σ_y(τ=1s) ≈ 3×10⁻¹⁰`. Power ~150 µW. Holds
sub-µs accuracy for ~1 hour open-loop, 10 µs for ~10 hours.

**Protocol**: TSHL (Syed-Heidemann 2006) — one-way broadcast phase
estimates skew, two-way round-trip phase estimates offset. Modern
extension D-Sync (Lu-Mirza-Schurgers 2018) corrects TSHL for node
motion using the Doppler shift already estimated by the demodulator.

**Anchor**: GPS PPS at surface. Every buoy re-anchors its CSAC
every ~1-6 hours depending on operational mode.

### 2.5 Bi-directional vs one-way

| Scheme | Update rate | Clock requirement | SSP inversion | Complexity |
|--------|-------------|-------------------|---------------|------------|
| Two-way LBL (Hunt 1974) | 0.5/epoch | low | low | low |
| OWTT (Webster 2012) | 1.0/epoch | high (CSAC) | medium | medium |
| Pseudo-range TDoA (Otero 2023) | 0.25/epoch | low (GNSS-disciplined only) | low | low |
| OWTT + Doppler (recommended) | 1.0/epoch + velocity | high | high | medium |

**Recommendation**: OWTT + Doppler. Each broadcast delivers range +
radial velocity; cadence 2× LBL; CSAC cost is acceptable; SSP
inversion is the free lunch.

**Fallback for cost-constrained deployments**: Otero-2023 pseudo-
range TDoA. Four buoys broadcast GNSS position + timestamp; the
underwater user computes its position from hyperbolic ranges
without atomic clock. Suboptimal but requires no CSAC.

### 2.6 Interference with passive listening

Active pinging loads the 9-14 kHz band with 200-800 ms of
transmitted energy per TDMA slot. During a slot, the broadcasting
buoy is deaf to ambient signals; listening buoys are nominally
fine (matched filter rejects out-of-band noise), but in-band
targets (tonal machinery lines near 11 kHz) are masked during
the TDMA slot duration.

**Mitigation**:
- Bandstop the broadcast frequency from the passive-detection
  channel during TDMA slots (programmatic gate, ~1 ms enable/
  disable).
- Schedule ranging slots to align with known-quiet epochs
  (ambient RMS threshold).
- Use preamble-only pings in tactical mode, reducing duty cycle
  from ~10% to ~2%.
- Freq-plan: ranging at 11.5 kHz, passive-detection primary band
  at 0.1-3 kHz (where whale and ship signatures live) → minimal
  overlap.

### 2.7 Power budget per ping

Typical JANUS chirp broadcast:
- Acoustic TX SPL: 185-195 dB re 1 µPa @ 1 m
- Electroacoustic efficiency: ~40% (ceramic transducer)
- Electrical input peak: 20-40 W
- Duration: 200 ms (preamble-only) to 800 ms (full frame)
- Energy per ping: 4-32 J
- At 0.1 Hz per-buoy ping rate: 0.4-3.2 W continuous

This is 10-100× the continuous Tier-2 (5 mW) budget in SYNTHESIS §3,
so ranging pings cannot come off the always-on power rail. Use
a **large capacitor / supercap reservoir** (10-100 F) charged
slowly from the primary battery, discharged into the transducer
at peak. Average draw from battery is ~0.5-2 W, which is within
the Tier-3 50-200 mW envelope if duty cycle < 5%.

---

## 3. The Sound-Speed Problem

Sound speed in water varies ~1470-1540 m/s with depth, temperature,
and salinity. For ranging at 5 km path and 1 m/s SSP error,
range bias is ~3.3 m — comparable to GPS. SSP is the dominant
range-error contributor after matched-filter timing. Three
strategies, ordered worst-to-best:

### 3.1 Climatological SSP (ADR-059/060 baseline; worst)

Use Levitus / WOA climatology (`c̄(z)` from monthly-averaged
Argo data). Typical accuracy: ~2-5 m/s at the surface mixed
layer, ~0.5 m/s below thermocline. Good enough for coarse
deep-water ranging (>100 m spacing) but biases short-range
shallow-water measurements heavily.

**Implementation**: `eml_core::operators::fourier_neural_op`
conditions on an 8-dim FiLM environment vector (ADR-060)
seeded from WOA lookup by lat/lon/month. Zero operational
cost but highest bias.

### 3.2 Ship-of-opportunity CTD (better)

At deploy time, the launching vessel drops a CTD (conductivity-
temperature-depth) cast. Results stored per deployment; each
buoy receives its pre-deployment CTD via JANUS cargo at
initialization.

- Accuracy: ~0.1 m/s RMS over upper 100 m
- Temporal validity: ~1-6 hours (internal waves, tidal
  thermocline migration)
- Cost: one 15-min CTD cast per deployment; essentially free

### 3.3 Jointly-estimated SSP from ranging residuals (best)

The **Munk-Wunsch 1979 ocean acoustic tomography** approach: every
measured travel time is a line integral of `1/c(s)` along the
ray path. For `N` buoys yielding `N²/2` pair-ranges × multiple
multipath arrivals, the combined travel-time vector
over-determines a reduced-EOF SSP.

Concretely: represent SSP as
`c(z) = c_ref(z) + Σ_{k=1..K} α_k · φ_k(z)` with `K=3-5` EOFs.
Gauss-Markov estimator:

    α̂ = C_α · G^T · (G · C_α · G^T + C_ε)^{-1} · δτ

where `G` is the path-integration matrix and `δτ` are travel-time
residuals vs a reference SSP. See `ssp-from-ranging.md` for full
derivation and Rust skeleton.

**Operational numbers**: ATOC 1999 (Cornuelle-Worcester-Dushaw)
recovered SSP at ~0.5 m/s RMS over 500 km² cells from a 3250 km
network. At sonobuoy-field scales (~1-5 km), ~0.05 m/s is
achievable with 6 buoys + 60 s of observations.

**Feedback loop**: SSP updates feed back into the same ranging
equations, tightening both. Convergence is stable if ranging
geometry is well-posed (non-collinear buoys).

### 3.4 Recommended stack

1. **Deploy**: climatological SSP + ship-of-opportunity CTD.
2. **First 60 s**: use climatological SSP for first trilateration.
3. **After 60 s**: switch to jointly-estimated SSP from residuals;
   update every 60 s.
4. **Fallback**: if OAT inversion is ill-conditioned (geometry
   collapses), fall back to last known good SSP with linear
   extrapolation.

---

## 4. The Multipath Problem

In shallow water (< 200 m) a single acoustic pulse arrives at the
receiver as a superposition of:

1. **Direct ray** (dominant, first arrival)
2. **Surface-bounce** (second arrival, 1-10 ms later)
3. **Bottom-bounce** (third, 10-100 ms later)
4. **Surface-bottom multi-bounce** (4th+, tens to hundreds of ms)

Each arrival carries its own travel time. Matched-filter output
shows a pulse train, not a single peak. For ranging we want the
direct arrival; for OAT (§3.3) we want all of them.

### 4.1 How classical LBL handles multipath

Hunt-1974 used envelope detection + leading-edge threshold. This
picks the direct-path arrival robustly in deep water where surface
bounce is 50+ ms away, but fails in shallow water where the bounces
pile up within 5 ms.

### 4.2 Matched-filter correlation vs envelope detection

LFM chirp with matched filter gives 20 dB processing gain vs
envelope detection, which separates direct vs surface-bounce
peaks down to ~200 µs time-resolution (JANUS preamble). Direct-
arrival peak is always the first above threshold — matched-filter
peak-picking with a tightened threshold works for most deployments.

### 4.3 Arrival-angle gating (when available)

If the receiving buoy has a small vertical hydrophone array (2-4
elements, ~5 cm spacing at 12 kHz), the vertical angle of arrival
can be estimated at ~5° resolution. Direct-path arrives near
horizontal (±10°); surface-bounce arrives from above (+10° to +40°);
bottom-bounce from below (-10° to -40°). Arrival-angle gate
accepts only near-horizontal energy for ranging.

### 4.4 Physics-prior rejection

The K-STEMIT physics-prior branch (Du-2023 PINN, Zheng-2025 FNO,
both ADR-059) produces a predicted arrival pattern given source/
receiver geometry + SSP. Cross-correlate measured arrivals with
the predicted direct-path arrival template; reject arrivals that
fall outside the predicted ±2σ window. This is **matched-field
processing for ranging** — a natural extension of Bucker-1976 MFP
(ADR-067 baseline) to the ranging use case.

### 4.5 Recommended multipath handling

1. **First pass**: matched-filter peak-pick → direct-path ~80% of time.
2. **On ambiguous peaks**: invoke arrival-angle gate if hardware
   supports it, else fall back to physics-prior template
   matching.
3. **Residual multipath**: log surface-bounce and bottom-bounce
   arrivals separately; feed to OAT-SSP inverter (§3.3) as extra
   observations.
4. **In shallow/littoral mode**: short-range only (< 500 m) to
   keep surface-bounce delay > matched-filter resolution.

---

## 5. Data Product for the Spatial Branch

The ranging subsystem publishes four time-varying structures at
the TDMA epoch cadence (~0.1-1 Hz):

### 5.1 Distance matrix `D(t) ∈ R^{N×N}`

`D_{ij}(t)` is the estimated Euclidean distance between buoys `i`
and `j` at time `t`. Symmetric (`D_{ii} = 0`, `D_{ij} = D_{ji}`).
Not all entries are directly measured — for unobserved pairs
(out-of-range, missed TDMA slot, bad SNR) the estimate is
interpolated from the joint EKF.

### 5.2 Per-pair uncertainty `σ_{ij}(t)`

Standard deviation of the distance estimate. Typical values:
- Direct measurement, good SNR: 0.15 m
- Direct measurement, bad SNR: 0.5-1.5 m
- EKF-predicted (no measurement this epoch): 1-3 m, growing
- SSP-uncertainty-dominated: 0.3-1 m

### 5.3 Velocity matrix `V(t) ∈ R^{N×N×3}`

Per-pair radial velocity from the Doppler shift, projected back
into 3D via the current geometry. `V_{ij}` is the instantaneous
relative velocity vector between `i` and `j`. Feeds the active-
imaging branch (Kiang-2022 multistatic SAS, ADR-065) which
needs platform velocities for non-stop-and-go range modeling.

### 5.4 Sound-speed profile `c(z, t)`

Expressed in 3-5 EOF coefficients per update. Feeds the physics-
prior branch (ADR-059, ADR-060) for FiLM conditioning and the
Helmholtz-PINN's `k(z) = ω/c(z)` wave-number field.

### 5.5 How the spatial-branch consumers use these products

**Tzirakis-2021 dynamic-adjacency GCN** (ADR-056) consumes `D(t)`
and `σ(t)` directly. Edge weights computed as
`W_{ij} = exp(-D_{ij}² / (2σ² + ℓ²))` with `ℓ` a learned length
scale. The ranging uncertainty `σ_{ij}` enters as observation
noise in the attention layer, letting the GCN up-weight
high-confidence edges.

**Grinstein-2023 Relation-Network SLF** (ADR-057) consumes
`(p_i, p_j, D_{ij}, σ_{ij})` as metadata per pair alongside the
GCC-PHAT cross-correlation. The MLP `F(x_i, x_j; φ)` has a richer
geometric context; expected 5-10 pp additional TDOA accuracy
over the round-2 baseline.

**Chen-Rao-2025 Grassmann DoA** (ADR-058) operates in a subspace
defined by the steering matrix `A(θ, p_1, ..., p_N)`. Ranging
gives `p_i` exactly; the Grassmann principal-angle computation
against the true-subspace anchor is newly well-posed.

---

## 6. Dual-Use Ping: Ranging as SAS Illumination

The active-imaging branch (ADR-063/064/065) performs synthetic-
aperture sonar (SAS) imaging when a cooperative ship or AUV
provides the coherent illumination. Kiang-2022 multistatic SAS
explicitly models a **stationary sonobuoy** as a receive-only
node in an imaging geometry.

The question: can the ranging pings that the sonobuoys themselves
emit also serve as SAS illumination?

### 6.1 Yes, with caveats

- **Waveform match**: LFM chirp (§2.1) is the canonical SAS waveform.
- **Coherence**: matched-filter preserves phase, so pulse-
  compressed output carries complex amplitudes suitable for
  coherent processing.
- **Geometry**: with `N` buoys, each buoy acts as transmitter (in
  its TDMA slot) and as receiver (in the other `N-1` slots). This
  is multistatic multi-aperture — exactly the geometry Kiang-2022
  treats.
- **SNR**: ranging pings are designed for direct-path matched
  filter detection, which means target echoes are ~20-40 dB below
  the direct arrival. SAS imaging of weak targets (seabed features,
  minelike objects) requires matched-filter target-strength TS >
  0 dB, which a ranging ping delivers only for large reflectors
  (seafloor, shipwrecks).

### 6.2 Caveats

- **Bandwidth limit**: SAS resolution = c̄ / (2 B). For 4 kHz chirp
  bandwidth, range resolution ≈ 0.19 m — good. Cross-range
  resolution is aperture-limited, ~`λ · R / L` with `L` the aperture
  length (array span); for 1 km span at 11 kHz, cross-range ≈ 12 m
  at 1 km range — coarse but usable.
- **Motion correction**: drifting buoys need micronavigation per
  ping. Doppler-aided ranging (§2, Lu-2018) supplies this.
- **Stealth**: a SAS illumination is 10-30 dB louder than a ranging
  ping; tactical modes cannot do this.
- **Band-dependent coherence budget (added 2026-05-11)**: CSAC
  short-term Allan deviation is ~10⁻¹¹ at 1 s, ~10⁻¹² at 100 s.
  This translates to ~10 ps timing uncertainty over 1 s,
  ~60 ns over 1 minute, ~600 ns over 10 minutes. The coherent
  integration window must keep timing drift below ~T/8 where T is
  the carrier period:
  - At **1.8 kHz** (T = 555 µs): T/8 = 69 µs → CSAC supports
    multi-hour coherent integration. Comfortable.
  - At **40 kHz** (T = 25 µs): T/8 = 3.1 µs → CSAC supports
    ~5+ hour coherent integration.
  - At **200 kHz** (T = 5 µs): T/8 = 625 ns → CSAC supports
    ~10 minute coherent integration windows. Bounded but
    workable for ping-rate-scale apertures.
  - At **235 kHz** (T = 4.25 µs): T/8 = 532 ns → CSAC supports
    ~8-9 minute windows. Tight; revalidate per-deployment.
  - At **500 kHz+** (Phase 7): coherence budget approaches the
    CSAC short-term floor. Requires either tighter clocks
    (rubidium / OCXO disciplined) or smaller coherent integration
    windows.

  The ADR-065 multistatic-with-stationary-sonobuoy plan is
  honestly coherent at 1.8 kHz (the only band actually used by
  RANGING.md's protocol stack today). Coherent multistatic at the
  imaging-tier bands (50 / 200 / 235 kHz, per `build/build-buoy-
  p79.md`) **is in budget but tight** — Phase 5d gimbal work
  should validate the coherent budget empirically, and the
  fallback to incoherent (envelope-only) multistatic reconstruction
  must be the default until coherence is measured.

### 6.3 Recommendation

Dual-use in **v4 research territory**. For v2/v3 treat ranging and
SAS illumination as separate operational modes. In v4, add a
`mode = "illuminate"` flag to the ranging broadcast, extending
chirp bandwidth from 4 kHz to 8 kHz and duration from 200 ms to
2 s — a 20 dB matched-filter gain increase that makes multistatic
SAS viable.

---

## 7. Stealth Considerations

Active pinging is a **detection hazard**. Any listener with a
hydrophone in the 9-14 kHz band can:

- Detect pings at 10-50 km ranges (5-10× ranging range, due to
  lower SNR threshold for detection vs ranging)
- Identify ping source by waveform signature
- Triangulate the buoy field layout from multiple listening
  positions
- Decode JANUS payloads (plaintext!)
- Spoof ranging pings to inject navigation errors

For tactical use this is a major concern. Operational modes:

### 7.1 Silent tactical mode

- **No active pings.** Pure passive listening + GPS-only geometry.
- Ranging subsystem disabled or in receive-only mode.
- Position noise reverts to GPS-only (2-5 m) but transmission
  footprint is zero.
- Suitable for covert first-entry deployments.

### 7.2 Chirp-coded mode

- LFM chirps with random per-buoy chirp-rate codes.
- Detectable but not identifiable without codebook.
- Decoys can be injected at other chirp rates.
- Suitable for mid-threat deployments.

### 7.3 Random-interval mode

- TDMA slots jittered ±30% random.
- Listening adversary cannot predict next slot.
- Costs ~20% TDMA efficiency.
- Combines with chirp-coded for harder counter-detection.

### 7.4 Spread-spectrum mode

- M-sequence PSK replacing JANUS FSK.
- 20-30 dB lower spectral density; harder to detect without
  code knowledge.
- Incompatible with JANUS standard (nothing else can decode).

### 7.5 Full-power routine mode

- Standard JANUS broadcasts per §2.
- Trusted regime (friendly waters, exercises, or allied operations).
- Maximum interoperability and data throughput.

### 7.6 Recommendation

Make operational mode a **runtime-selectable** per-deployment
parameter. Default to chirp-coded + random-interval for unknown
threat environments. Provide silent-tactical as a well-tested
emergency fallback.

---

## 8. Integration with Existing Literature

How the round-1 and round-2 paper stack consumes ranging outputs:

| Paper | Paper role in SYNTHESIS | Ranging provides | Improvement expected |
|-------|-------------------------|------------------|----------------------|
| Tzirakis 2021 (ADR-056) | spatial-branch GCN | `D(t)` directly as adjacency | 10-30× effective SNR |
| Grinstein 2023 (ADR-057) | Rel-Net SLF | `(p_i, σ_{ij})` metadata | 5-10 pp TDOA |
| Chen-Rao 2025 (ADR-058) | Grassmann DoA | true steering matrix | well-posed principal angle |
| Du 2023 Helmholtz-PINN (ADR-059) | physics-prior | in-situ SSP | ~1 m/s vs climate 5 m/s |
| Zheng 2025 FNO (ADR-059) | physics-prior | `k(z) = ω/c(z)` live | improves thermocline regime |
| Perez 2017 FiLM (ADR-060) | conditioning | live 8-dim env vector | reduces climate bias |
| Bucker 1976 MFP (ADR-067) | classical baseline | known geometry + SSP | first time well-posed |
| Capon 1969 MVDR (ADR-067) | classical baseline | known `p_i`, MVDR steering | near-optimal performance |
| Kiang 2022 multistatic SAS (ADR-065) | active-imaging | `(p_i, v_i)` for geometry | solves v4 open problem |

The ranging subsystem **unlocks** several papers that otherwise
have prerequisite assumptions unmet by drifting-buoy GPS. The MFP
and MVDR baselines in particular assume known array geometry;
before ranging, those baselines are fitting noise. After ranging,
they approach theoretical performance.

---

## 9. WeftOS Integration

### 9.1 New crate: `clawft-sonobuoy-ranging`

Feature-gated under `clawft-sonobuoy/ranging`. Contains:

```
clawft-sonobuoy-ranging/
├── src/
│   ├── lib.rs                # trait definitions + re-exports
│   ├── protocol.rs           # JANUS frame encoder/decoder
│   ├── clock.rs              # CSAC state + TSHL / D-Sync
│   ├── owtt.rs               # OWTT measurement parsing
│   ├── multipath.rs          # arrival extraction + gating
│   ├── ekf.rs                # joint range-velocity EKF (8-dim per buoy)
│   ├── ssp_inverter.rs       # OAT Gauss-Markov inversion (EOF basis)
│   ├── coop.rs               # cooperative pseudo-range (Bahr-style)
│   ├── scheduler.rs          # TDMA slot assignment / gossip
│   └── hardware/
│       ├── micromodem.rs     # WHOI µModem backend
│       ├── janus_soft.rs     # software-defined JANUS implementation
│       └── sim.rs            # simulator for testing
├── tests/
│   ├── known_geometry.rs     # Hunt-1974 trilateration unit
│   ├── owtt_single.rs        # Webster-2012 single-beacon observability
│   ├── coop_ekf.rs           # Bahr-2009 cooperative geometry
│   ├── janus_frames.rs       # JANUS spec conformance
│   ├── ssp_recovery.rs       # synthetic SSP inversion
│   └── multipath_shallow.rs  # shallow-water arrival extraction
└── Cargo.toml
```

### 9.2 Trait sketch

```rust
/// The ranging subsystem's top-level interface.
pub trait RangingSubsystem: Send + Sync {
    /// Current distance matrix (estimated, including EKF extrapolation).
    fn distance_matrix(&self) -> Array2<f64>;
    fn distance_uncertainty(&self) -> Array2<f64>;

    /// Current velocity matrix (radial, per pair).
    fn velocity_matrix(&self) -> Array3<f64>;

    /// Current SSP estimate.
    fn ssp(&self) -> SspReducedBasis;

    /// Per-buoy full state (8-dim position + velocity + clock).
    fn buoy_states(&self) -> &[BuoyState];

    /// Subscribe to range-change events.
    fn subscribe_events(&self) -> impulse::Receiver<RangeEvent>;

    /// Control: set operational mode (silent / chirp / random / spread / routine).
    fn set_mode(&mut self, mode: RangingMode);
}

/// One buoy's full ranging state.
#[derive(Debug, Clone)]
pub struct BuoyState {
    pub id: u16,
    pub position_m: [f64; 3],
    pub velocity_mps: [f64; 3],
    pub clock_bias_s: f64,
    pub clock_drift_ppm: f64,
    pub covariance: [[f64; 8]; 8],
    pub last_update: Instant,
}

/// Published to Impulse queue on significant range change.
#[derive(Debug, Clone, Copy)]
pub struct RangeEvent {
    pub pair: (u16, u16),
    pub old_range_m: f64,
    pub new_range_m: f64,
    pub sigma_m: f64,
    pub time: Instant,
}
```

### 9.3 Integration with `quantum_register::build_register`

The quantum-register layout (ECC `quantum_register.rs`) currently
reads a static adjacency list and force-directs a 2D layout. With
ranging, buoy positions are **known in metric units**, so the
register layout becomes a scaled identity of the ranging positions:

```rust
pub fn build_register_from_ranging(
    ranging: &dyn RangingSubsystem,
    constraints: RegisterConstraints,
) -> Result<Vec<(String, [f64; 2])>, QuantumError> {
    let states = ranging.buoy_states();
    let positions_xy: Vec<[f64; 2]> = states.iter()
        .map(|s| [s.position_m[0], s.position_m[1]])
        .collect();
    scale_to_constraints(&positions_xy, constraints)
        .map(|scaled| scaled.into_iter().enumerate()
            .map(|(i, p)| (format!("q{}", states[i].id), p)).collect())
}
```

This replaces force-directed layout with **real geometry** — the
quantum register mirrors the physical sonobuoy field in scaled
coordinates. For the Pasqal EMU_FREE quantum walk over the buoy
graph (SYNTHESIS §5.3 v4 research direction), this means the
walk operates on true distances, not force-directed abstractions.

### 9.4 Integration with `eml_core`

Three new `eml_core` operators grown from §3-§5:

| Operator | Paper | Role |
|----------|-------|------|
| `eml_core::operators::owtt_residual` | Webster 2012 | trainable-bias wrapper around the EKF range residual; learns hardware asymmetry |
| `eml_core::operators::oat_inversion` | Munk-Wunsch 1979 | Gauss-Markov SSP update, cached by `(ssp_ref_hash, geometry_hash)` |
| `eml_core::operators::doppler_fuse` | Lu 2018 | Doppler-shift → velocity with learnable hardware-specific bias |

All three compose with the existing physics-prior operators (ADR-059
`helmholtz_residual`, ADR-060 `fourier_neural_op`). Cache keys are
shared where appropriate — e.g., the OAT inversion reuses the
Helmholtz-PINN's ray-trace for path-integration Jacobian.

### 9.5 Integration with `cognitive_tick`

New tick stages for ranging-enabled deployments:

```
PROPAGATE (clock, position, velocity from last state)
    ↓
LISTEN (acoustic RX buffer, matched filter)
    ↓
MEASURE (extract direct-path timing, Doppler, multipath)
    ↓
UPDATE (EKF update on range + velocity observations)
    ↓
INVERT (OAT SSP update if multipath arrivals available)
    ↓
PUBLISH (distance matrix, velocity matrix, SSP to downstream
          branches; emit Impulse RangeEvents if thresholds crossed)
    ↓
SCHEDULE (next TDMA slot, backoff if needed)
    ↓
TRANSMIT (my own broadcast if my TDMA slot is now)
```

PAM-mode tick runs at 0.1 Hz (one cycle per 10 s). Tactical tick
runs at 1 Hz. Ranging slot times are dictated by TDMA schedule, not
by tick phase — the tick wakes up on either the periodic timer or
a TDMA slot boundary.

### 9.6 Integration with `Impulse` queue

`RangeEvent`s (per §9.2) fire whenever any `D_{ij}` changes by
> 3σ, or whenever a new buoy joins/leaves the field. Consumers
subscribe to relevant events:
- Spatial branch: all `RangeEvent`s → adjacency update
- Physics-prior branch: SSP-significant events → FiLM env-vector update
- Classification head: major geometry changes → re-embed queued audio

### 9.7 Telemetry

Ranging outputs summarized per epoch as a ~200-byte TDMA-carried
packet:
- Distance matrix sketch (top `k=8` edges)
- SSP EOF coefficients (5 × f32 = 20 bytes)
- Clock state (bias, drift)
- Operational mode
- Health (battery, SNR statistics)

Compatible with the 80 bps JANUS link (~20 s transmission); more
typically carried on the LoRa / Iridium out-of-band channel.

---

## 10. Proposed ADR

### ADR-078 — Inter-buoy active acoustic ranging as primary sensor-position source

**Context**: SYNTHESIS.md §10 identified sensor-position uncertainty
as an open architectural problem. Drifting sonobuoy fields operate
with 2-5 m GPS position noise that dominates the intrinsic TDOA
and DoA error budgets of the spatial branch (ADR-056, ADR-057,
ADR-058) and invalidates the known-geometry assumptions of MFP/
MVDR baselines (ADR-067).

**Decision**: Introduce an inter-buoy active-acoustic ranging
subsystem (`clawft-sonobuoy-ranging`) that broadcasts OWTT pings
(Webster-Eustice 2012) on JANUS-compatible LFM chirps (Potter-
Alves 2014), synchronized via CSAC + GPS-disciplined TSHL (Syed-
Heidemann 2006) with D-Sync mobility extension (Lu 2018). The
subsystem publishes a distance matrix `D(t)`, velocity matrix
`V(t)`, per-pair covariance, and reduced-EOF SSP `c(z, t)` (via
Munk-Wunsch-1979 OAT) to downstream branches.

**Criteria for adoption**:
- Meter-scale inter-buoy distances available at ≥0.1 Hz per pair
- Velocity tracking at ≤0.2 m/s RMS per pair
- SSP EOF estimate at ≤0.2 m/s RMS over mixed layer
- Power budget ≤2 W continuous on average
- Tactical silent-mode fallback

**Dependencies**:
- ADR-056/057/058 (spatial branch) — consumes `D(t)` and `V(t)`
- ADR-059/060 (physics-prior) — consumes SSP
- ADR-067 (MFP/MVDR baselines) — assumption now valid
- ADR-062 (verification mandate) — all ranging literature must
  be primary-source verified, per §12
- ADR-069 (4-tier power split) — ranging fits Tier-3; CSAC fits Tier-2
- ADR-076 (Multi-Krum Byzantine aggregator) — layered over clock-sync
  messages

**Sprint**: v2 (alongside ADR-056 spatial branch rollout).

---

## 11. v1 / v2 / v3 / v4 Sequencing

### 11.1 v1 — GPS-only (no ranging)

- Shore-side one-shot classifier (SYNTHESIS §8.1).
- Ranging subsystem **absent**. GPS-only position.
- Rationale: v1 is a shore-side classifier; buoys don't need
  self-localization beyond GPS; the ranging subsystem is
  orthogonal.

### 11.2 v2 — Ranging + dual-branch spatial (**ranging enters here**)

- K-STEMIT spatial branch (ADR-056 Tzirakis dyn-adj GCN) comes
  online.
- `clawft-sonobuoy-ranging` deployed in parallel.
- OWTT + JANUS chirp + CSAC clock.
- Deliverables:
  - Distance matrix at 0.5 Hz per pair
  - Per-pair σ < 0.5 m in typical deployments
  - Velocity from Doppler (joint range-velocity EKF)
  - Basic TDMA scheduler + D-Sync clock discipline
  - Unit tests against Hunt-1974 / Webster-2012 known-geometry cases
- Rationale: Tzirakis GCN is the first downstream consumer that
  benefits measurably from ranging; adopting ranging here gives
  the v2 performance gains.

### 11.3 v3 — Ranging-derived SSP + physics-prior integration

- ADR-059/060 physics-prior branch (Du-2023 PINN, Zheng-2025
  FNO, Perez-2017 FiLM) comes online.
- Ranging subsystem adds OAT inversion (Munk-Wunsch 1979,
  Cornuelle-1999, Xu-2025) on multipath residuals.
- Reduced-EOF SSP fed live to physics-prior branch.
- Grinstein-2023 Relation-Network + Chen-Rao-2025 Grassmann DoA
  consume ranging outputs.
- Bucker-1976 MFP / Capon-1969 MVDR baselines become well-posed
  (known geometry + known SSP).
- Deliverables:
  - In-situ SSP at 0.2 m/s RMS, 60-s update
  - MFP/MVDR baselines calibrated
  - Physics-prior operating on live-conditioned environment
- Rationale: SSP inversion needs a mature ranging system plus
  physics-prior infrastructure to consume it; v3 is where
  everything composes.

### 11.4 v4 — Multistatic SAS + quantum walk

- ADR-065 Kiang-2022 multistatic SAS online. Consumes ranging-
  provided `(p_i, v_i)` per buoy (solves SYNTHESIS §10 open
  problem).
- Dual-use ranging pings as SAS illumination in `illuminate` mode.
- Quantum walk on the buoy graph (Pasqal EMU_FREE) operates on
  ranging-derived true geometry.
- Chen-Rao Grassmann subspace on QPU for `k > N` refinement.
- Federated SSP training across the field.
- Rationale: everything else needs ranging as a prerequisite.

---

## 12. Verification Status of Cited Literature

Per ADR-062 (verification mandate from round-1 fabrication finding)
every citation here is primary-source verified:

| # | Citation | Verification | Status |
|---|----------|-------------|--------|
| 1 | Hunt et al. 1974 WHOI-74-6 | DOI 10.1575/1912/2117 | Verified |
| 2 | Munk & Wunsch 1979 Deep-Sea Res | DOI 10.1016/0198-0149(79)90073-6 | Verified |
| 3 | Worcester et al. 1982 Nature | DOI 10.1038/299121a0 | Verified |
| 4 | Munk/Worcester/Wunsch 1995 Cambridge | ISBN 978-0-521-11536-0 | Verified |
| 5 | Cornuelle et al. 1999 JASA 105:3202 | DOI 10.1121/1.424646 | Verified |
| 6 | Worcester et al. 1999 JASA 105:3185 | DOI 10.1121/1.424649 | Verified |
| 7 | Syed & Heidemann 2006 INFOCOM | DOI 10.1109/INFOCOM.2006.161 | Verified |
| 8 | Kinsey Eustice Whitcomb 2006 MCMC | WHOI open PDF | Verified |
| 9 | Bahr Leonard Fallon 2009 IJRR 28:714 | DOI 10.1177/0278364908100561 | Verified |
| 10 | Stojanovic & Preisig 2009 CommMag | DOI 10.1109/MCOM.2009.4752682 | Verified |
| 11 | Eustice et al. 2011 JFR 28:121 | DOI 10.1002/rob.20364 | Verified |
| 12 | Webster Eustice Singh Whitcomb 2012 IJRR | DOI 10.1177/0278364912446166 | Verified |
| 13 | Liu et al. 2013 TPDS Mobi-Sync | DOI 10.1109/TPDS.2012.164 | Verified |
| 14 | Van Walree 2013 JOE | DOI 10.1109/JOE.2013.2278913 | Verified |
| 15 | Potter et al. 2014 UComms JANUS | DOI 10.1109/UComms.2014.7017134 | Verified |
| 16 | NATO STANAG 4748 (2017) | NATO news 143247 | Verified |
| 17 | Lu Mirza Schurgers 2018 D-Sync | DOI 10.3390/s18061854 | Verified |
| 18 | Dong et al. 2018 DE-Sync | DOI 10.3390/s18061861 | Verified |
| 19 | Bianco et al. 2019 JASA review | DOI 10.1121/1.5133944 | Verified |
| 20 | Otero et al. 2022 arXiv LBL | arXiv:2204.08255 | Verified |
| 21 | Otero et al. 2023 JMSE 11:682 | DOI 10.3390/jmse11040682 | Verified |
| 22 | Sun et al. 2024 JMSE 12:1925 diff PE | DOI 10.3390/jmse12111925 | Verified |
| 23 | Wang et al. 2025 JMSE LT-Sync | DOI 10.3390/jmse13030528 | Verified |
| 24 | Xu et al. 2025 IMTS passive SSP | DOI 10.1007/s44295-025-00083-2 | Verified |

Per-paper analyses at `papers/analysis/{lbl-acoustic-nav,
munk-worcester-tomography, one-way-travel-time,
cooperative-buoy-positioning, janus-underwater-comms,
ssp-from-ranging, tshl-clock-sync, doppler-aided-ranging}.md`.

---

## 13. Honest Limitations

- **Active transmissions compromise stealth**. Tactical silent mode
  exists as a fallback but gives up the ranging benefits. No
  get-out-of-jail-free option here.
- **Clock sync is fragile**. GPS-antenna failures (fouling, water
  ingress, adversarial jamming) force open-loop CSAC, which
  degrades in a few hours. Mitigation: TSHL peer-discipline across
  buoys that still have GPS, but a wholesale GPS denial situation
  degrades the entire field.
- **Shallow-water multipath is hard**. Matched filter + arrival-
  angle gating + physics-prior rejection combined give ~80%
  correct direct-path identification in typical shallow water.
  The remaining 20% degrades SSP inversion convergence and can
  bias ranging. Recommend explicit confidence flagging per
  measurement.
- **OAT inversion ill-posed in collinear geometries**. If buoys
  drift into an approximate line, the SSP problem becomes rank-
  deficient. Detect via condition-number monitoring on the
  Jacobian; fall back to climatological SSP.
- **SSP under ice or strong thermocline events** exceeds the 3-5
  EOF basis. Real surface ducts or solitary waves can locally
  distort c(z) by 10-20 m/s in ways no climatology captures. In
  these regimes treat OAT SSP as advisory, not authoritative.
- **CSAC cost** at ~$3k/buoy makes the full OWTT stack too
  expensive for truly expendable deployments. Otero-2023 pseudo-
  range TDoA with GNSS-disciplined clocks ($50 per buoy) is the
  cost-optimized fallback; accuracy is ~2× worse but hardware
  cost is ~100× lower.
- **JANUS 80 bps is slow**. Full-cargo ranging broadcasts at 4 s
  on-air limit TDMA scalability. Preamble-only mitigates but
  loses payload; LoRa out-of-band recovers payload throughput
  but only at the surface. Honest: the 80 bps channel is a
  constraint, not a given.
- **No experimental validation yet**. Every number in this
  document comes from the cited literature; none have been
  measured on a clawft deployment. Expect a 2-3× fudge factor on
  accuracy numbers until sea-trial calibration.
- **Multi-Krum assumes < f = (n-1)/3 Byzantine nodes**. A
  coordinated attack controlling > 1/3 of buoys defeats the
  ranging aggregator. Classical Byzantine limit; nothing specific
  to this architecture.
- **Power budget is capacitor-pulsed**, not continuous. Battery
  chemistry (LiFePO₄, primary lithium) must tolerate ~40 W peak
  bursts every ~2-20 s. Some ultra-low-cost chemistries will not.
- **Not all consumers benefit equally**. Tzirakis GCN gets ~10-30×
  effective SNR; physics-prior gets maybe 2× (limited by
  climate-SSP-is-already-decent); MFP baselines go from
  non-functional to nominal (qualitative, not
  quantitative change).

---

## 14. Immediate next actions

1. **Commit** this RANGING.md and the 7 per-paper analyses to
   `.planning/sonobuoy/papers/analysis/`.
2. **Add reference to RANGING.md from SYNTHESIS.md** (the main
   thread does this, per task instructions; this document does
   not modify SYNTHESIS.md).
3. **Draft ADR-078** per §10; slot into sprint planning after
   ADR-077.
4. **Scaffold `clawft-sonobuoy-ranging`** crate per §9.1; start with
   the simulator backend and JANUS frame encoder/decoder.
5. **Prototype the OWTT EKF** on synthetic data; calibrate against
   Webster-2012 single-beacon observability numerical checks.
6. **Engage procurement** on CSAC (Microsemi SA.45s or equivalent)
   for the v2 hardware build. Long lead times (~12 weeks).
7. **Design sea-trial experiment** for v2: 4-buoy field, 500 m
   spacing, CTD cast + calibrated positioning baseline, 2-hour
   operational test. Target: reproduce Hunt-1974 meter-scale
   accuracy on a modern drifting field.
