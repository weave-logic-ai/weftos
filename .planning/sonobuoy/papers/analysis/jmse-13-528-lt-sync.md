# Paper Analysis — LT-Sync (Zhang & Wu 2025)

**Citation**: Zhang, C.; Wu, H. *LT-Sync: A Lightweight Time Synchronization Scheme for High-Speed Mobile Underwater Acoustic Sensor Networks.* J. Mar. Sci. Eng. 2025, **13**(3), 528.
**DOI**: [10.3390/jmse13030528](https://doi.org/10.3390/jmse13030528)
**License**: CC BY 4.0 (open access)
**PDF**: `.planning/sonobuoy/papers/jmse-13-00528.pdf`
**Affiliation**: National Time Service Center, Chinese Academy of Sciences, Xi'an (the same NTSC that maintains China's national time standard — a relevant credential).
**Funding**: Youth Innovation Promotion Association CAS, grant Y2023109.
**Analyzed by**: sonobuoy symposium analyst, 2026-05-11.
**Verification status**: ✅ verified by direct PDF read (citation skeleton from URL, content from PDF).

## Why this paper matters for clawft

User direction 2026-05-11: *"this has key insights. I think unless someone comes up with something better we should try to implement this schema deep in this primitive, although this may be more on the acoustic implementation."*

LT-Sync is **a concrete, simulation-validated time-synchronization scheme** designed precisely for the architectural regime clawft operates in: underwater acoustic, mobile (diver-placed Class C, drifting Class A in current), energy-constrained, low-message-count. The scheme drops directly under the WeftTSF abstract primitive of ADR-084 as the **foundational baseline implementation of WeftAcousticTSF**, subject to adaptations documented in §"Integration into clawft" below.

## Problem (the gap LT-Sync addresses)

Time-sync for **high-speed mobile UWASNs** (Underwater Acoustic Sensor Networks). The authors enumerate the prior-art failure modes:

| Scheme | Year | Authors | Failure mode |
|---|---|---|---|
| RBS | 2002 | Elson et al. | Ignores propagation delay; terrestrial-only |
| TPSN | 2003 | Ganeriwal et al. | Ignores propagation delay |
| FTSP | 2004 | Maróti et al. | Ignores propagation delay |
| **TSHL** | 2006 | Syed & Heidemann | First UWA-aware; ignores node mobility |
| **MU-Sync** | 2008 | Chirdchoo et al. | Adds mobility to TSHL; assumes symmetric channel (false in real practice) |
| **D-Sync** | 2010 | Lu, Mirza, Schurgers | Adds Doppler; needs ≥25 message exchanges (high energy cost) |
| **DA-Sync** | 2014 | Liu et al. | Doppler-assisted; same energy cost |
| **DE-Sync** | 2018 | Zhou et al. | Doppler-enhanced; same energy cost |
| **APE-Sync** | 2019 | Zhou et al. | Adaptive power; still needs many messages |
| **Tri-Message** | 2009 | Tian et al. | Lightweight (3 messages); but assumes all three delays equal — fails for moving nodes |
| **DC-Sync** | 2024 | Sun et al. | Handles complex motion; high energy cost |
| **SFDM** | 2024 | Wang et al. | Sync-free detection; particle filter heavy |

The gap LT-Sync fills: **lightweight (3 messages) AND mobile-aware**. None of the prior art is both.

## The schema (the central contribution)

### Three-message exchange (Sender–Receiver scheme)

Beacon node **A** (synchronized) and unsynchronized node **B** (the mobile node) exchange three messages. B moves with **uniform velocity v in a single direction** for the duration of the sync.

```
                Beacon A               Unsynchronized B
                                       (moves uniformly at speed v)

    Message 1   A0 ─────────────────►  B0  (captures arrival time)
                                       |
                                       | (waiting interval Δt)
                                       v
    Message 2   A1 ◄─────────────────  B1  (sends own transmit time)
                |
                | (waiting interval)
                v
    Message 3   A2 ─────────────────►  B2  (with payload {A0, A1, A2, Δf})

  After 3 messages, B has 6 timestamps + Doppler shift Δf.
```

After the exchange, B knows:
- A0, A1, A2 (beacon's transmit timestamps embedded in messages 1 and 3)
- B0, B1, B2 (B's own local timestamps)
- Δf (Doppler shift, measured by A on B's transmission in message 2)

### Mathematical formulation

**Clock model** (linear, equation 1):

```
B_i = α · A_i + β
```

where α is B's clock skew (drift rate) and β is B's clock offset.

**Six-timestamp relationship** (equation 3):

```
A_0 = t_0
B_0 = α(t_0 + d_0) + β
B_1 = α · t_1 + β
A_1 = t_1 + d_1
A_2 = t_2
B_2 = α(t_2 + d_2) + β
```

where d_0, d_1, d_2 are the three message propagation delays.

**Clock-skew and clock-offset solutions** (equations 4 and 5):

```
α = (B_2 − B_0) / (A_2 − A_0 + d_2 − d_0)

β = [(B_0 + B_1) − α(A_0 + A_1 + d_0 − d_1)] / 2
```

**Doppler-to-velocity** (equation 6):

```
v = (Δf / f) · c
```

where c ≈ 1500 m/s (acoustic in water), f is the carrier frequency.

**Propagation delays in terms of Doppler shift** (equation 10):

```
d_0 = l / [c(1 − Δf/f)]
d_1 = [l + (B_1 − A_0)·(Δf/f)·c] / c
d_2 = [l + (A_2 − A_0)·(Δf/f)·c] / [c(1 − Δf/f)]
```

where l is the initial distance between A and B at the time of message 1.

**Composite α / β expressions** (equations 11 and 12, using Doppler shift to estimate propagation delays):

```
α = (B_2 − B_0)(1 − Δf/f) / [ (A_2 − A_0)(1 − Δf/f) + (A_2 − A_0)·Δf/f ]

β = { (B_0 + B_1) − α[(A_0 + A_1) + (Δf/f)·( l/[c(1−Δf/f)] − (A_0−B_1)/f )] } / 2
```

### Signal acquisition (DSSS with FFT-based parallel correlator)

LT-Sync rides on a **direct-sequence spread-spectrum** (DSSS) physical layer:

- **Carrier**: ~20 kHz acoustic
- **Modulation**: Composite Spread Spectrum Sequence (**CCOS**) = bitwise XOR of:
  - A Walsh sequence (orthogonal, well-known synchronization-friendly)
  - A **logistic chaotic sequence** (provides additional spreading + security; chaotic dynamics x_{n+1} = r·x_n·(1−x_n))
- **CCOS length**: 256 bits (used in paper's simulation)
- **Data rate** (baseband): ~100 Hz (the "frequency of synchronization signal" in Table 1)
- **Frequency search range**: 19.5-20.5 kHz around the 20 kHz carrier
- **Frequency search step**: 100 Hz
- **Doppler search resolution after acquisition**: half the frequency search step = ~50 Hz

### FFT-based parallel acquisition algorithm

The receiver acquires the carrier frequency and code phase through a parallel FFT-based correlation:

1. Multiply received signal by carrier reference; extract in-phase (I) and quadrature (Q) components.
2. FFT both I/Q channels.
3. FFT the complex-conjugate of the native CCOS spread-spectrum sequence.
4. Multiply the two FFT outputs element-wise.
5. IFFT the product → cyclic correlation across all code phases.
6. Threshold the peak: if above threshold, acquisition succeeded.

The output gives **Doppler shift estimate AND code phase**, both simultaneously.

### Equations from §3 (in original notation)

```
I + jQ = Σ_{i=1..L} r(k) · exp[−j(ω_d − ω̂_d)(i·T_s)]                  (13)

[Y_1, Y_2, ..., Y_L]^T = FFT [r_1·exp(−jΔω·T_s), r_2·exp(−jΔω·2T_s), ...]^T  (14)

[C_1, ..., C_L]^T = FFT*[C_{PN,1}, ..., C_{PN,L}]^T                     (15)

[R(1,ω̂_d), ..., R(L−ω̂_d)]^T = IFFT [C_1·Y_1, ..., C_L·Y_L]^T          (16)
```

where ω_d is the true Doppler shift, ω̂_d is the search hypothesis, T_s is the sample period, L is the CCOS length, and FFT* denotes the complex-conjugate FFT.

## Simulation results

**Simulation environment**: BELLHOP ray-acoustic model (Porter; one of the canonical underwater propagation solvers already in the clawft research corpus per `papers/analysis/bellhop-ray-tracing.md`).

**Parameter table** (Table 1 in paper):

| Parameter | Value |
|---|---|
| Original distance (l) | 1485 m |
| Speed of unsynchronized node | **15 m/s** |
| Speed of sound | 1500 m/s |
| Frequency of synchronization signal | 100 Hz |
| Clock skew | 40 ppm |
| Clock offset | 80 µs |
| Interval between two messages | 5 s |
| Clock granularity | 1 µs |
| Reception jitter | 15 µs |
| Number of messages (TSHL baseline) | 25 |

**Key result**: LT-Sync produces only **~5 s of clock error after 10⁶ s** (~11.6 days) of synchronization. Tri-Message produces *more* error than no synchronization at all (its uniform-delay assumption causes incorrect skew estimation). TSHL (25 messages) performs much worse on energy efficiency.

**Consistency across initial skews**: LT-Sync maintains performance across skews from 10⁻⁵ to 10⁻⁴ ppm.

**Consistency across speeds**: LT-Sync maintains performance from 11 m/s to 20 m/s. Tri-Message's error grows monotonically with speed.

**Energy efficiency**: LT-Sync uses 3 messages vs TSHL's 25 → roughly 8× lower energy per sync round, plus fewer re-sync rounds needed because of higher accuracy → cumulative energy reduction is significantly larger.

**Acquisition under noise**: SNR = −10 dB with 256-bit CCOS, peak-to-second-peak ratio = 1.58, Doppler measurement error 3.04 Hz at 100 Hz search step (within half-step, indicating successful acquisition).

## Limitations (stated by the authors)

1. **The equation-9 approximation** (treating αA_0 + β ≈ A_0) creates systematic error that grows with synchronization interval. Authors note this doesn't dominate accuracy but recommend future revision.
2. **Uniform-single-direction motion constraint**: the unsynchronized node MUST move uniformly in one direction throughout the sync round. Hard to constrain in large fleets. This is the single biggest practical limitation.
3. **Initial-distance estimation**: l (initial distance at message 1) must be obtainable. The paper notes "it can be obtained according to the time when the beacon node receives the first synchronization message echo" — i.e., an echo-based pre-measurement. This implies the protocol requires a prior ranging step.
4. **No physical-implementation validation**: simulation-only; no field test reported. The CCOS DSSS and FFT acquisition are simulated via BELLHOP, not against a real hydrophone deployment.
5. **Carrier 20 kHz** — chosen "to adapt to the low-frequency characteristics" of the UWA channel. Note that this is HIGHER than clawft's 1.8 kHz mesh band; the choice has implications for retargeting (see §"Integration").

## Integration into clawft (adaptation plan)

LT-Sync is the **recommended foundational implementation of WeftAcousticTSF** under ADR-084 §1, with five adaptations to fit the clawft deployment surface:

### Adaptation 1 — Generalize the motion constraint via cross-modal range

LT-Sync's "uniform-single-direction motion" assumption is the limitation that most restricts deployment. ADR-084's **primary-broadcaster + thunder-and-lightning pattern** removes this assumption by giving the receiver a **direct clock-free range** from optical + acoustic arrival-time difference. With direct range known, the propagation-delay equations (10) no longer require the velocity assumption — they reduce to known constants for that ping.

The adaptation: **use LT-Sync's clock-skew/offset solution math (equations 4, 5) with the d_i directly measured from cross-modal Δt** instead of derived from Doppler velocity. The Doppler-derived d_i remains a fallback when only acoustic channel is available (Phase 1b before optical lands).

### Adaptation 2 — Retarget the carrier band

LT-Sync uses 20 kHz carrier. clawft's existing 1.8 kHz mesh band is committed in `requirements.md` based on piezo resonance. Two options:

- **Option A**: Adopt 20 kHz as a new "WeftAcousticTSF sync band" separate from the 1.8 kHz mesh data band. Cheap piezos at 20 kHz exist (HC-SR04-style sender + receiver, $2 each). Adds a new transducer per buoy.
- **Option B**: Retarget LT-Sync to 1.8 kHz. CCOS spreading at 1.8 kHz with 300 Hz bandwidth supports shorter chips than 20 kHz, lower data rate, but works on existing hardware.

**Recommendation: Option B for Phase 1b-3** (no new hardware), **Option A for Phase 5+** (add a 20 kHz transducer alongside the imaging tier for dedicated sync band).

### Adaptation 3 — Adopt CCOS modulation for sync chirps

CCOS (Walsh × logistic-chaotic XOR) is a strong choice for sync because:
- Walsh sequences are orthogonal → low inter-emitter cross-correlation
- Chaotic sequences add per-emitter uniqueness
- The combination is robust to UWA channel multipath

Replace the chirp-spread modulation in `architecture.md` §"Modulation v2" with **chirp-spread + CCOS** for the sync-pulse role; keep chirp-only for data-band ranging chirps. This is one parameter in the chirp generator.

### Adaptation 4 — Implement the FFT-based parallel acquisition on ESP32-S3

The S3's ESP-DSP library provides SIMD-optimized FFT/IFFT/element-wise multiply. The LT-Sync acquisition algorithm maps directly:

- 256-bit CCOS at 1.8 kHz carrier with 300 Hz BPF
- ADC at 4 kS/s (Nyquist + margin)
- FFT size: 1024 samples (256 chips × 4 samples/chip)
- Per-FFT cost on the S3 single core: ~100 µs
- Full acquisition with 100 Hz frequency search step over 19.5-20.5 kHz: ~20-100 ms per ping

For the 1.8 kHz retargeted version: ~10× lower compute cost due to lower sample rate. Comfortably fits the existing S3 budget per `embedded-acoustic-firmware` persona.

### Adaptation 5 — Generalize the 3-message exchange

LT-Sync uses 3 messages per sync round (A0→B0, B1→A1, A2→B2). Our mesh emits chirps continuously per the "every chirp is a sync pulse" rule in ADR-084. The LT-Sync equations apply to any 3 messages where:
- Two are from the beacon to the unsynchronized node
- One is from the unsynchronized node to the beacon
- The Doppler shift is measured on the round-trip exchange

With many chirps in flight, the joint-inference solver (ADR-083) selects the best 3 per period to solve LT-Sync's equations, then aggregates across rounds for higher precision.

Or, equivalently: treat LT-Sync as the **per-pair clock-sync primitive** and run it independently per pair, then aggregate across the mesh in the shore-side joint solver.

## Performance projection for clawft

With LT-Sync's results (5 s error after 10⁶ s) and clawft's lower mobility (Class C nodes at <1 m/s drift vs LT-Sync's 15 m/s test) and faster re-sync (1 Hz beacons vs LT-Sync's 0.2 Hz), the clawft-adapted LT-Sync should achieve:

- **Sub-second sync error** per pair after a single 3-message exchange
- **Sub-µs sync error** across a 10-minute deployment with continuous chirp updates
- **Sub-100 ns sync error** with cross-modal range elimination of the propagation-delay unknown (when optical is available)

The cross-modal pattern is **strictly stronger** than LT-Sync alone for this reason.

## What clawft adopts and what clawft doesn't

| LT-Sync element | clawft adoption | Rationale |
|---|---|---|
| 3-message sync exchange | ✅ **Yes** (as one mode) | Lightweight per-pair primitive |
| Doppler-shift propagation-delay estimation | ✅ **Yes** (as fallback when optical unavailable) | Falls back gracefully from cross-modal range |
| CCOS spreading (Walsh × logistic) | ✅ **Yes** (replaces chirp-spread for sync pulses) | Better cross-correlation properties |
| FFT-based parallel acquisition | ✅ **Yes** | ESP-DSP supports it directly |
| 20 kHz carrier | ⚠️ **Deferred** (use 1.8 kHz Phase 1b-3, 20 kHz Phase 5+) | Hardware constraint |
| Uniform-single-direction motion constraint | ❌ **Replaced** by cross-modal range | Architectural improvement via thunder-and-lightning |
| 5-s waiting interval between messages | ⚠️ **Tuned** per phase | Faster re-sync at lake-test deployment scale |
| 256-bit CCOS length | ✅ **Yes** (or shorter for power saving) | Reasonable starting point |

## Open questions for follow-on work

1. **Field validation**: LT-Sync is simulation-only. clawft Phase 2 lake test is the natural empirical validation; results feed back into ADR-084 §1.
2. **CCOS vs PN sequence vs Gold codes**: the paper doesn't compare CCOS to other spread-spectrum sequences. Worth measuring against Gold codes (used in GPS) in our deployment.
3. **Chaotic sequence parameter sensitivity**: the logistic-map parameter r affects the chaos regime; the paper uses but doesn't specify the exact r value used in simulation. May need to be tuned for our deployment.
4. **Integration with low-frequency "song" channel** (ADR-084 §0 low-frequency floor channel): can LT-Sync run on the 200 Hz LF band? The longer pulse duration (10s of seconds per song) suggests the 3-message protocol becomes very slow — but the mobile-tolerance is exactly what we want for long-range nodes.

## ADR linkage

LT-Sync integration is documented in **ADR-084 §1 WeftAcousticTSF implementation**. The implementation is non-blocking on the rest of the symposium work — it can land alongside the P3 firmware panel as the concrete protocol P3 implements.

## Cross-references

- ADR-078 (RANGING.md, OWTT + JANUS + CSAC + TSHL/D-Sync stack): LT-Sync extends the TSHL/D-Sync lineage with a lighter-weight, mobile-aware variant.
- ADR-082 (three-class buoy architecture): LT-Sync handles Class C's mobile nature gracefully.
- ADR-083 (shore-side calibration service): consumes LT-Sync's outputs (clock skew α + offset β per pair) as inputs to the joint inference.
- ADR-084 (WeftTSF abstract primitive): LT-Sync is the recommended implementation for the WeftAcousticTSF concrete protocol.
- `papers/analysis/bellhop-ray-tracing.md`: the simulation environment LT-Sync uses; already in clawft corpus.
- `papers/analysis/multistatic-sas.md` (Kiang 2022): LT-Sync's mobile-node assumption is similar to Kiang's stationary-sonobuoy + moving-platform geometry, but inverted (LT-Sync handles MOVING node, not moving illuminator).

## Verbatim references the paper cites that clawft should track

The paper cites 34 prior works. Items already in clawft's corpus (verified):

- [10] TSHL — Syed & Heidemann 2006 (in `papers/analysis/tshl-clock-sync.md`)
- [12] D-Sync — Lu, Mirza, Schurgers 2010 (in `papers/analysis/cooperative-buoy-positioning.md`)
- [16] **Tri-Message** — Tian et al. 2009 (foundational; **not yet in clawft corpus** — recommend adding)
- [31] BELLHOP — Porter (in `papers/analysis/bellhop-ray-tracing.md`)

Items the clawft corpus should consider acquiring:

- [16] Tian et al. 2009 Tri-Message — the protocol LT-Sync extends
- [13] DA-Sync — Liu et al. 2014 (IEEE TMC)
- [14] DE-Sync — Zhou et al. 2018 (Sensors)
- [15] APE-Sync — Zhou et al. 2019 (IEEE Access)
- [18] DC-Sync — Sun et al. 2024 (IEEE Access)
- [19] SFDM — Wang et al. 2024 (Ocean Engineering)
- [21] Li et al. 2021 Gold-code-spreading underwater — for the CCOS-vs-Gold comparison study.
- [27] Xiao et al. 2018 — improved chaotic spread-spectrum sequence (foundation for CCOS).
- [30] Kim & Kong 2014 — FFT-based TDCC for GNSS acquisition (foundation for LT-Sync's FFT acquisition).

## TL;DR

LT-Sync provides clawft's **WeftAcousticTSF baseline protocol**: a 3-message Doppler-aware time-sync scheme with DSSS+CCOS modulation and FFT-based acquisition. The clawft adaptation replaces LT-Sync's restrictive uniform-motion assumption with the cross-modal range trick from ADR-084 §0 (primary broadcaster + thunder-and-lightning), preserving the math while eliminating the operational constraint. The protocol is energy-efficient (3 messages per sync vs TSHL's 25), simulation-validated to ~5 s error per 10⁶ s, and maps cleanly onto the ESP32-S3 + ESP-DSP firmware stack. Recommendation: **adopt as the foundational implementation of WeftAcousticTSF in ADR-084 §1**.
