# Paper Analysis — DE-Sync (Zhou, Wang, Nie, Qiao 2018)

**Citation**: Zhou, F.; Wang, Q.; Nie, D.; Qiao, G. *DE-Sync: A Doppler-Enhanced Time Synchronization for Mobile Underwater Sensor Networks.* Sensors **2018**, *18*(6), 1710. doi:10.3390/s18061710.
**DOI**: [10.3390/s18061710](https://doi.org/10.3390/s18061710)
**License**: CC BY 4.0 (open access)
**PDF**: `.planning/sonobuoy/papers/lt-sync-citations/pdfs/[14]-zhou-2018-de-sync.pdf` (15 pages, 2.4 MB, acquired via Europe PMC mirror)
**Affiliation**: Acoustic Science and Technology Laboratory, Harbin Engineering University (HEU); Key Lab of Marine Information Acquisition and Security; College of Underwater Acoustic Engineering — Harbin, China. HEU is one of China's two anchor institutions for underwater acoustics (the other is Xi'an / NTSC, which produced LT-Sync). The same Zhou/Wang/Qiao group later produced [15] APE-Sync 2019.
**Funding**: NNSF China grants 61571151, 61501134, 11774074, 61431004; National Key R&D 2017YFC0305702.
**Analyzed by**: sonobuoy symposium analyst, 2026-05-11.
**Verification status**: ✅ verified by direct PDF read of all 15 pages (PDF metadata: F. Zhou, Q. Wang, D. Nie, G. Qiao; title matches; LaTeX with hyperref / pdfTeX-1.40.18 producer consistent with MDPI 2018 template).

## Priority for clawft

**P0 — direct predecessor to LT-Sync.** LT-Sync explicitly cites DE-Sync as the closest Doppler-enhanced antecedent ([14] in LT-Sync's reference list) and inherits its core insight — that the unknown clock skew α must be folded into the Doppler-scale estimate to avoid a bias that grows with skew magnitude. Understanding DE-Sync is **required reading** for `WeftAcousticTSF` (ADR-084 §1) because the LT-Sync schema we're implementing is essentially DE-Sync's correction-loop wrapped in a tri-message lightweight envelope.

## Why this paper matters for clawft

In the LT-Sync analysis card (`papers/analysis/jmse-13-528-lt-sync.md`) we identified six adaptations clawft needs to make to deploy a Class C / Class A buoy time-sync primitive. **Three of those six trace directly to DE-Sync**:

1. The **calibration loop** that re-estimates Doppler scale after the first linear regression
   (DE-Sync §3.3 "iterates until skew delta < 50 ppm OR max 2 iterations"; ADR-084 §1 adaptation 5).
2. The **transmission of the Doppler scale factor in the message payload** rather than
   the relative velocity (DE-Sync §3.2 motivation; avoids the c-dependence that introduces
   sound-speed-profile error).
3. The **least-squares regression formulation** on the six-tuple (T1,T2,T3,T4, aAB, aBA)
   per round (DE-Sync eq. 13; LT-Sync inherits the structure unchanged in its tri-message
   variant).

If `WeftAcousticTSF` cannot land DE-Sync's calibration loop, the residual clock-skew–Doppler-bias coupling will dominate the per-buoy timing error, and the multi-buoy TDOA bearing solution (the project's whole point) will inherit a quadratic-in-skew error term that ruins the bearing CRLB.

## Problem (the gap DE-Sync addresses)

The 2018 state of the art for **mobile** underwater sync was:
- **D-Sync** (Lu, Mirza, Schurgers 2010 [28]): Doppler-based, but **estimates α=1** during Doppler-scale extraction, so the Doppler estimate is biased by the true skew. As initial skew grows, sync error grows linearly with skew × velocity.
- **MU-Sync** (Chirdchoo, Soh, Chua 2008 [26]): adds mobility to TSHL but **assumes one-way delay = RTT/2**, which is false for mobile nodes — performance crashes under realistic motion.
- **TSMU** (Liu, Wang, Peng, Zuba 2011 [29]): considers the skew during Doppler-scale estimation (similar to DE-Sync's central insight), but **directly uses sound-speed c** in the time-sync calculation. Since c varies with depth/temperature/salinity, the c-estimation error feeds back as a sync error. Also has three tunable initial-skew parameters (operationally fragile).

DE-Sync's two contributions (paper §3, p.4):
1. Account for clock skew α in the Doppler-scale factor estimation.
2. Carry the **Doppler-scale factor** in the payload (not the relative velocity), which removes the c-dependence at the regression stage.

## The schema

### Three-phase synchronization

Each sync round consists of:

```
Phase 1: Data collection
  N rounds of two-way message exchange between beacon A (synchronized)
  and unsynchronized B. Each round captures (T1, T2, T3, T4) and the two
  measured Doppler-scale factors (aAB at B's receiver, aBA at A's receiver).

Phase 2: Linear regression
  Form the 2-parameter LS estimate [α̂, β̂] = (HᵀH)⁻¹ Hᵀ Y
  with Y[i] = 2·T1ⁱ + T4ⁱ·(2 − (aABⁱ + aBAⁱ))
  H[i,1] = 2·T3ⁱ + T2ⁱ·(2 − (aABⁱ + aBAⁱ))
  H[i,2] = (4 − (aABⁱ + aBAⁱ))

Phase 3: Calibration
  Use α̂ to recompute the *physical* Doppler scale am = α̂(1 + aBA) − 1
  Repeat Phase 2 with the corrected am. Terminate when:
    - iterations ≥ 2, OR
    - |α̂_k − α̂_{k−1}| < 50 ppm
  In practice, 2 iterations is enough (simulation §4.2.1).
```

### The skew–Doppler coupling

The paper's central equation (paper eq. 5):

```
am = ((1 + aAB) / α) − 1     for A → B direction
am = α · (1 + aBA) − 1        for B → A direction
```

**`aAB` is what the receiver actually measures (sample-rate ratio).** `am` is the *physical* Doppler scale (vm/c). The two differ by α — the clock skew of the receiver — because a fast clock at the receiver makes A's signal look "slow" both due to relative motion AND due to its own time-base error.

D-Sync's mistake: it sets α=1 in the above. DE-Sync's fix: it iterates between solving for α via LS regression and updating am.

### Clock model (the standard linear form)

```
T = α · t + β                          (paper eq. 1)
T1 = α · t1 + β                        (B's send time)
T4 = α · t4 + β                        (B's receive time)
T2 = t2                                (A's receive time — A is synchronized)
T3 = t3                                (A's send time)
```

With τ1 = forward propagation (B→A) and τ2 = return propagation (A→B), and noting the mobility makes τ1 ≠ τ2, the paper derives (eq. 12):

```
2T1 + T4·(2 − (aAB + aBA)) = α·(T2·(2 − (aAB + aBA)) + 2T3) + β·(4 − (aAB + aBA)) + ε
```

The trick: this is **linear in (α, β)** under the assumption that the small term containing `(aAB + aBA)` is treated as a known regressor. Each round of message exchange gives one equation in two unknowns, so N ≥ 2 rounds suffice (N=25 in their simulation; LT-Sync uses N=1 round of three messages instead, achieving the same effect through a different geometry).

## Key simulation results (paper §4)

| Parameter | DE-Sync mean error | D-Sync mean error | D-Sync degradation |
|---|---|---|---|
| Initial skew 0.05·10⁶ ppm | ≈0.1 ms | ≈10 ms | linear in skew |
| Response time 1 → 25 s | flat ≈1 ms | 1 → 18 ms | linear in time |
| Max relative speed 1 → 5 m/s | flat ≈1 ms | 1 → 14 ms | linear in v |
| Max relative accel 0.01 → 0.1 m/s² | 0.3 → 0.5 ms | 1 → 4 ms | linear in a |
| Message interval 20 → 120 s | flat ≈0.1 ms | flat ≈0.9 ms | ~9× DE-Sync |
| Number of messages 5 → 45 | std-dev decreases | std-dev decreases | DE-Sync mean < D-Sync mean |

**Energy efficiency**: DE-Sync achieves ~2× the re-sync interval of D-Sync at any fixed error tolerance (paper Fig. 10), because higher α-accuracy means fewer re-sync rounds are needed to keep error below a tolerance e.

**Simulation parameters** (paper Table 2):
- Max distance 1000 m, max relative speed 5 m/s
- Max accel 0.1 m/s², clock skew up to 10⁵ ppm
- Backoff time 1 s, N=25 messages, message interval 3 s
- Clock granularity 1 µs, reception jitter 15 µs
- Sim window: 2 hours post-sync error reported

## Direct equations to port into `WeftAcousticTSF`

```
// 1. The Doppler-skew coupling. THE central insight.
//    In esp-dsp on the S3: keep aAB / aBA as f32, α as f64 internally.
am = α * (1.0 + a_BA) - 1.0;   // when computing on the beacon-side measurement
am = (1.0 + a_AB) / α - 1.0;   // when computing on the receiver-side

// 2. The LS regressor structure (paper eq. 13). For N rounds:
for i in 0..N {
    Y[i]    =  2.0 * T1[i] + T4[i] * (2.0 - (a_AB[i] + a_BA[i]));
    H[i][0] =  2.0 * T3[i] + T2[i] * (2.0 - (a_AB[i] + a_BA[i]));
    H[i][1] =  4.0 - (a_AB[i] + a_BA[i]);
}
// Then α̂, β̂ = (Hᵀ H)⁻¹ Hᵀ Y   — a 2×2 inversion, trivial.

// 3. Calibration loop.
//    Run for at most 2 iterations OR until |Δα| < 50 ppm.
loop {
    let alpha_new = ls_estimate(...);
    if iterations >= 2 || (alpha_new - alpha_old).abs() < 50e-6 {
        break;
    }
    recompute_am_with(alpha_new);
    alpha_old = alpha_new;
}
```

## Integration into clawft (`WeftAcousticTSF` / ADR-084 §1)

### Where DE-Sync fits in the stack

```
                          ┌─────────────────────────┐
   GPS / 1PPS (Class S) ──┤  WeftAuthoritativeTSF   │  (already specified, ADR-084 §0)
                          └────────────┬────────────┘
                                       │ broadcasts time + Doppler ref
                                       v
                          ┌─────────────────────────┐
   Class C (drifting   ──>│  WeftAcousticTSF        │  ← LT-Sync + DE-Sync core
   buoy in current,         │  (this analysis card)   │
   ESP32-S3 + CSAC)         └────────────┬────────────┘
                                       │ 50-200 µs sync to beacon
                                       v
                          ┌─────────────────────────┐
   Beam-forming / TDOA  ──┤  WeftSyncedDataPlane    │
   downstream consumer    └─────────────────────────┘
```

### Adaptations clawft needs vs. paper DE-Sync

| What DE-Sync assumes | What clawft has | Required change |
|---|---|---|
| Two-way message (4 timestamps + 2 Doppler factors) per round, N=25 rounds | Class C buoys are energy-budget-bound; we cannot afford 50 acoustic exchanges per sync | **Use LT-Sync's tri-message variant** which collapses to 3 messages by adding one Doppler measurement; DE-Sync's calibration loop still applies. See `papers/analysis/jmse-13-528-lt-sync.md` |
| Doppler scale measured from CP-OFDM preamble (§3.3) | clawft uses LFM chirps for matched-filter ranging; we can derive Doppler scale from chirp-rate compression error (Doppler-aided ranging, see `papers/analysis/doppler-aided-ranging.md`) | Re-derive `aAB` from chirp-rate residue, not OFDM CP. This changes the **measurement model** but not the **regression model**. |
| 5 m/s max relative speed (their Class B-like drift) | clawft Class A drift: 0.1–0.5 m/s; Class C diver-placed: 0–2 m/s | Easier regime; the DE-Sync improvements over D-Sync **narrow** in this regime (paper Fig. 5). Question: is the calibration loop still worth running, or does plain D-Sync suffice at low speeds? **Recommendation: keep the calibration loop**; it costs 1 extra LS solve (negligible on S3 LX7) and protects us in the worst case. |
| Crystal skew up to 10⁵ ppm (very loose) | clawft Class A with CSAC: ~10⁻⁹/day, effectively zero at sync timescales. Without CSAC (Class C low-cost), TCXO ~5 ppm | At 5 ppm skew, DE-Sync vs. D-Sync gap is < 0.5 ms (paper Fig. 3 below 10⁴ ppm region). **At low skew, calibration loop is overkill** — but again, we keep it for portability across hardware tiers. |
| 1 µs clock granularity, 15 µs reception jitter (DSP-driven) | ESP32-S3 timer at 80 MHz → 12.5 ns granularity; matched-filter leading-edge picker, ~10–30 µs jitter | We are at-parity or better on granularity, comparable on jitter. **The regression is not jitter-bound on our hardware.** |
| Sound speed c assumed (only enters through the Doppler-scale → velocity conversion they avoid) | Same; we explicitly carry `aAB` to avoid c entirely (their key insight 2) | No change. |

### What DE-Sync does NOT solve (and we still must address)

1. **Initial coarse offset** — DE-Sync's LS converges from a starting point but doesn't bootstrap from zero. clawft needs a separate **coarse-sync** step (chirp arrival → coarse offset → handoff to DE-Sync refinement). The LT-Sync paper papers over this; their three-message scheme inherits the gap. **Action**: add coarse-sync mechanism to `WeftAcousticTSF` design — leading-edge picker output from the first chirp gives ~τ ± 100 µs, hand that to the LS as the initial β estimate.
2. **Multi-beacon scenarios** — DE-Sync's regression is per-(beacon, receiver) pair. For our 3-buoy mesh, each buoy is potentially a beacon for the others (Class C–Class C peer sync once any one is anchored). **Action**: run pairwise DE-Sync and use the WeftConsensus layer to fold the pairwise estimates into a single mesh time (see G2 in `GAPS.md`).
3. **Doppler aliasing at high mobility** — DE-Sync assumes the Doppler measurement is unambiguous. At v > c·Δf/(2·f_center) the Doppler bin wraps. For our 38 kHz chirp center and 0.1 m/s typical drift, we are nowhere near this limit; safe to ignore.

## Cross-references in the corpus

- **LT-Sync paper card**: `papers/analysis/jmse-13-528-lt-sync.md` — the successor that adds the tri-message lightweight envelope.
- **D-Sync**: `pdfs/[12]-lu-2010-d-sync.pdf` — the direct ancestor DE-Sync corrects.
- **Tri-Message paper**: `pdfs/[16]-tian-2009-tri-message.pdf` — the lightweight message-count base that LT-Sync grafts onto DE-Sync.
- **TSHL** (DE-Sync ref 25): `papers/analysis/tshl-clock-sync.md` — foundational static UWA sync.
- **Doppler-aided ranging**: `papers/analysis/doppler-aided-ranging.md` — measurement-model bridge between chirp matched-filter and DE-Sync's `aAB`.
- **Cooperative buoy positioning** (covers D-Sync alternatives): `papers/analysis/cooperative-buoy-positioning.md`.
- **One-way travel time**: `papers/analysis/one-way-travel-time.md` — the alternative to two-way sync if we can't close the message loop.

## DE-Sync's own references that matter most for downstream reading

The paper cites 32 references. The ones in `papers/lt-sync-citations/` already include several. Cross-relevance:

| DE-Sync ref | Identity | In our corpus? |
|---|---|---|
| [4] Elson 2002 RBS | foundational terrestrial sync | ✅ acquired (this folder) |
| [25] TSHL Syed/Heidemann 2006 | first UWA sync | ✅ already in main corpus |
| [26] MU-Sync Chirdchoo 2008 | mobile UWA sync | still 📥 user-drop |
| [27] Mobi-Sync Liu/Zhou 2010 | spatial-correlation UWA sync | not in LT-Sync refs; may be worth chasing |
| [28] D-Sync Lu 2010 | Doppler UWA sync | ✅ in this folder |
| [29] TSMU Liu/Wang 2011 | mobile UWA sync, Doppler+Kalman | still 📥 user-drop |
| [20] Mason/Berger 2008 multicarrier Doppler | OFDM-preamble Doppler estimation | ✅ acquired (extras) |

**Two new TODOs surfaced by DE-Sync that aren't in LT-Sync's own citations**:
- DE-Sync ref [27] **Mobi-Sync** (Liu, Zhou, Peng, Cui — GLOBECOM 2010) — uses spatial correlation of node velocities. Not a clawft fit (we don't have neighbor velocity priors) but worth one read for completeness.
- DE-Sync ref [31] **Pallares/Bouvet/Rio TS-MUWSN** (IEEE J. Ocean. Eng. 2016) — recent mobile UWA sync, mentioned only in passing. Worth checking if it has a public preprint.

## Status / verdict

- ✅✅ **ACQUIRED** at `pdfs/[14]-zhou-2018-de-sync.pdf` (full paper, 15 pages).
- ✅ **Verified** by direct read of all 15 pages.
- 📌 Update `papers/lt-sync-citations/README.md` to flip [14] from "📥 try-fetch" to "✅✅ ACQUIRED".
- 📌 No change required to ADR-084 §1 from this reading — the LT-Sync analysis card already incorporated DE-Sync's central equations (1, 5, 13) by reference. This card now serves as the **primary reference** for those equations.
- 📌 Downstream: when scaffolding the `WeftAcousticTSF` Rust module, the LS regression in eq. 13 is the core routine. Implement once, share between LT-Sync (tri-message) and DE-Sync (N-message fallback) callers.
