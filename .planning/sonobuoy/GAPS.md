# Sonobuoy Project — Gap Tracker

**Created**: 2026-04-15
**Supersedes**: `SYNTHESIS.md` §10 as the canonical list of open technical gaps
**Update policy**: flip status as each gap is closed; link to the PR / ADR / analysis doc that closes it.

Seven gaps were identified in the round-1+round-2 synthesis. G1 is being closed by the in-flight ranging research. G2-G5 are open technical research problems with dedicated agents running. G6-G7 are administrative (track-only, no research needed).

---

## Status legend

- 🔴 **OPEN** — no active work
- 🟡 **IN PROGRESS** — research or implementation underway
- 🟢 **CLOSED** — resolved; link to resolution
- ⚪ **TRACKING** — not a research problem; monitor external state

---

## G1. Sensor-position uncertainty

**Status**: 🟢 **CLOSED** (2026-04-15)
**Severity**: P0 (blocks accurate spatial-branch output)
**Origin**: `SYNTHESIS.md` §2.2, §10 — Grinstein 2023 does not output calibrated position uncertainty; GPS on drifting buoys has 2-5 m noise; this degrades TDOA-based bearing estimates.

**Closing approach**: Active inter-buoy acoustic ranging via OWTT (one-way travel time) on JANUS-compatible LFM chirp waveforms, CSAC-class atomic clocks (~150 µW) GPS-disciplined, TSHL + D-Sync protocol. Expected accuracy: **0.15-1.5 m σ** at 1-5 km inter-buoy spacing (10× better than GPS). Fallback: Otero 2023's GNSS-disciplined pseudo-range TDoA at ~$50/buoy vs $3k/buoy with CSAC.

**Delivered** (2026-04-15):
- [x] `RANGING.md` (983 lines) with full protocol design
- [x] 8 verified paper analyses (Hunt 1974 LBL, Munk-Wunsch 1979 OAT, Webster 2012 / Eustice 2011 OWTT, Bahr 2009 cooperative EKF, Otero 2023 4-buoy GNSS, Potter 2014 JANUS, Syed 2006 TSHL, Lu 2018 D-Sync, Cornuelle 1999 + modern ML SSP, Stojanovic-Preisig 2009 Doppler)
- [x] ADR-078 "Inter-buoy active acoustic ranging as primary sensor-position source" drafted in RANGING.md
- [ ] `clawft-sonobuoy-ranging` crate scaffolding (scheduled for v2)

**Cross-cuts into other gaps**:
- **Doppler shift from the same ping** → 0.1 m/s velocity tracking per buoy → **partially closes G4** (Kiang 2022 unknown-velocity assumption)
- **Inter-buoy travel-time residuals + Munk-Wunsch OAT inversion** → live 3-5 EOF SSP coefficients → **supporting input for G2/G3** (physics-prior under thermocline; ranging-derived in-situ SSP can condition the PINN / FNO)

---

## G2. Helmholtz-PINN 3D collapse

**Status**: 🟢 **CLOSED** (2026-04-15) — see `gaps/G2-pinn-3d.md`
**Closing approach**: Phased two-path strategy.
- **Path A (v1, per-deployment)**: Feature-engineered PINN — sin activation + LAAF slope recovery + KRAKEN normal modes as deterministic dispersion-relation features (ocean analog of Schoder 2025's H5 result) + RAR-D adaptive sampling + optional 8-panel azimuthal XPINN decomposition (Jagtap 2020). Trains online at buoy drop in 6-16 min on L4 GPU.
- **Path B (v2, fleet-amortized)**: PINO (Li 2021) trained once on ~500 BELLHOP3D instances spanning (SSP, freq, src_depth). 200 MB weight bundle, 50 ms inference, no per-deployment training.
- **Path C (v3)**: Stack A+B for R² ≥ 0.97.
**Target**: R² ≥ 0.85 (both paths project R² ≥ 0.95, comfortably above target). Multi-scale Fourier PINN (Wang 2021) as NTK-theoretic fallback for spectral bias edge cases.
**ADR**: ADR-079 "3D PINN technique for Helmholtz physics-prior branch"
**Severity**: P1 (limits physics-prior branch to 2D)
**Origin**: `SYNTHESIS.md` §2.3, §10; analysis in `papers/analysis/pinn-ssp-helmholtz.md`.

**Problem**: Du 2023 Helmholtz-PINN achieves R²=0.99 / ~1 dB TL error in 2D but collapses to R²=0.48 in 3D. Real sonobuoy deployments see 3D propagation (range + depth + azimuth), so 2D-only is a hard limitation.

**Candidate solutions to research**:
- Domain decomposition PINNs (XPINN / DeepXDE parallel) — split 3D domain into 2D panels
- Multi-scale PINN architectures (hash-encoding, SIREN, Fourier features)
- Hybrid PINN + FNO approaches — use FNO for bulk propagation, PINN for boundary-layer corrections
- N-PINN or physics-informed neural operator variants that scale to higher dimensions
- Adjoint-based PINN training (vs standard residual-minimization) for better 3D convergence
- 3D BELLHOP3D (Porter 2019 JASA 146:2016) as the ground-truth surrogate target

**Research agent**: running (see §Research assignments below).

**Resolution criteria**:
- [ ] Verified literature review delivered
- [ ] 3D target R² ≥ 0.85 with acceptable compute budget
- [ ] Recommended approach documented in analysis file + SYNTHESIS.md update

---

## G3. FNO failure under strong vertical SSP gradients (thermocline regime)

**Status**: 🟢 **CLOSED** (2026-04-15) — see `gaps/G3-fno-thermocline.md`
**Closing approach**: **ThermoFno** hybrid — U-NO topology (Rahman 2023) + Legendre multiwavelets k=4 on depth axis (Gupta 2021 MWT-Operator, Gibbs-free) + GINO signed-distance-function thermocline channel (Li 2023) + PINO parabolic-equation residual loss with adaptive 4× depth super-resolution (Li 2021). Warm-start from Zheng 2025 coarse weights. Runtime selector routes smooth-SSP cases to original Zheng-FNO, thermocline cases to ThermoFno.
**Target**: 0.5-1.0 dB RMSE at ~3× inference cost (meets both G3 targets: <1 dB, <5×).
**ADR**: ADR-080 "Thermocline-robust neural operator for ocean acoustic propagation"
**Severity**: P1 (the thermocline regime is exactly where sonobuoys operate)
**Origin**: `SYNTHESIS.md` §2.3, §10; analysis in `papers/analysis/fno-propagation.md`.

**Problem**: Zheng 2025 FNO speedup (28.4% vs RAM, <0.04 dB RMSE) holds only outside the mixed layer. Strong vertical SSP gradients — the thermocline regime sonobuoys typically operate in — break the FNO's spectral assumptions.

**Candidate solutions to research**:
- Multi-scale neural operators (MSNO, GINO, FNO with adaptive modes)
- U-Net Neural Operator (UNO) — handles localized sharp features better
- Physics-informed Fourier Neural Operator (PINO) — adds physics residual loss
- Adaptive mode selection — more Fourier modes in thermocline layer, fewer elsewhere
- Depth-stratified FNO ensemble — separate operators for above/within/below thermocline
- Augment with learned residual correction on top of FNO output
- DeepONet variants that better handle sharp coefficient jumps

**Research agent**: running.

**Resolution criteria**:
- [ ] Verified literature review delivered
- [ ] Target: <1 dB RMSE in thermocline regime with <5x inference cost increase
- [ ] Recommended approach documented

---

## G4. Multistatic SAS with unknown platform velocities

**Status**: 🟢 **CLOSED** (2026-04-15) — see `gaps/G4-sas-unknown-velocity.md`
**Closing approach**: Two-stage hybrid joint estimator.
- **Stage 1**: Hierarchically-coupled VAE generalizing Xenaki 2024's 2-band coupling to N buoys; runs on inter-buoy cross-coherence tensors to produce per-buoy velocity priors `N(μ_VAE, Σ_VAE)`, self-supervised (no labels).
- **Stage 2**: Zhang-Achim 2022 alternating minimization on Kiang 2022's N-buoy extended non-stop-and-go forward operator `C(V)`. Alternate HOTV-regularized image updates (Scarnati-Gelb 2018 ADMM with Kuramoto-style 2-D phase sync) and per-buoy Levenberg-Marquardt velocity updates warm-started by Stage 1.
- **Cross-cut with G1**: Bayesian soft prior precision-weights G1 ranging-derived velocities as a convex-neighborhood stabilizer, *not* a hard constraint — preserving the joint loop's ability to refine velocities from target-aware image sharpness.
**Target**: ≤ 3 dB PSNR degradation at 0.1 m/s uncertainty (met). ≤ 5% target-velocity error vs Kiang baseline 3%.
**ADR**: ADR-079b "Joint velocity-and-image reconstruction for N-buoy multistatic SAS" (re-numbered 2026-05-11 by Deliverable 2 §4; the original ADR-081 slot was claimed by the sonobuoy symposium for "Adopt the WeftOS sensor framework for the sonobuoy wire format", see `.planning/symposiums/sonobuoy/adrs/ADR-081-sensor-framework-adoption.md`. This research-track ADR is parked under ADR-079b pending a v2 SAS-reconstruction crate; it has not yet been promoted to an ADR file).
**Severity**: P1 (blocks v4 quantum-SAS integration)
**Origin**: `SYNTHESIS.md` §2.4, §10; analysis in `papers/analysis/multistatic-sas.md`.

**Problem**: Kiang 2022 multistatic SAS with stationary sonobuoy assumes known platform velocities for its non-stop-and-go range model. Drifting buoys don't know their velocity precisely (typically ~0.5 m/s sustained drift with ~0.1 m/s uncertainty). Simultaneous-velocity-and-image reconstruction is the open problem.

**Candidate solutions to research**:
- Joint velocity/image optimization (iterated Kiang-model refinement)
- Ranging-derived velocity (differential range rates between buoys) — ties to G1 ranging subsystem
- Doppler-aided velocity estimation from the active ping itself
- Autofocus-via-velocity — treat velocity as a nuisance parameter and estimate it during autofocus
- Bundle adjustment methods from computer vision (photogrammetry analog)
- Deep learning approaches — extend Gerg-Monga 2021 to jointly estimate phase error + velocity
- Distributed SAR literature (space-based distributed aperture) may have prior art

**Research agent**: running.

**Resolution criteria**:
- [ ] Verified literature review
- [ ] Target: image PSNR degradation ≤ 3 dB with v uncertainty ≤ 0.1 m/s
- [ ] Integration path with G1 ranging subsystem documented

---

## G5. Federated learning under sub-kbps uplink

**Status**: 🟢 **CLOSED** (2026-04-15) — see `gaps/G5-sub-kbps-fl.md`
**Closing approach**: 5-layer compounding codec — (1) event-triggered participation gate, (2) HierFAVG two-tier (Liu 2020) with κ₁=10 local + κ₂=100 edge at Raft cluster leader, (3) DGC Top-k(s=0.001) with flash-backed residual, (4) error-feedback signSGD 1-bit (Bernstein 2018 + Karimireddy 2019 EF fix) or FetchSGD Count Sketch r=2 c=200 fallback (Rothchild 2020), (5) delta-varint + RLE + Ed25519 wire encoding via rvf-crypto. FedMD (Li & Wang 2019) runs parallel for heterogeneous hardware fleets via logit distillation on a firmware-baked 10k alignment set.
**Result**: **~210 B per buoy per cloud round** — fits single 340 B Iridium SBD MO packet; convergence in ≤10 cloud rounds.
**ADR**: ADR-080b "Sub-kbps federated learning protocol stack for sonobuoy network" (re-numbered 2026-05-11 by Deliverable 2 §4; the original ADR-082 slot was claimed by the sonobuoy symposium for "Three-class buoy architecture (A/B/C)", see `.planning/symposiums/sonobuoy/adrs/ADR-082-three-class-buoy-architecture.md`. This research-track ADR is parked under ADR-080b pending a v3 federated-learning crate; it has not yet been promoted to an ADR file. ADR-090 (`.planning/symposiums/sonobuoy/adrs/ADR-090-fl-gradient-compression.md`) operationalizes the closing decision and is the load-bearing reference for build-side work).
**Severity**: P1 (blocks v3 federated training across sonobuoys)
**Origin**: `SYNTHESIS.md` §4.1, §10; analyses in `papers/analysis/{fedavg-foundations,deep-gradient-compression,byzantine-robust-krum,split-learning}.md`.

**Problem**: Deep Gradient Compression (Lin 2018) achieves 270-600× bandwidth reduction but assumes tens of kbps available. Real sonobuoy RF back-haul is:
- Satellite (Iridium SBD): **340 bps** effective sustained
- Line-of-sight UHF: 10-100 kbps depending on sea state
- Acoustic modem (between buoys): 100 bps - 10 kbps

Combining DGC + FedAvg + Multi-Krum + Split Learning verbatim does NOT close the sub-kbps gap. Need extreme compression + error-coding + delayed-round protocols.

**Candidate solutions to research**:
- Quantized gradient methods (signSGD, TernGrad, 1-bit SGD)
- Sketch-based gradient aggregation (SketchFL, count-sketch gradient)
- Stochastic rounding and error feedback
- Hierarchical FL — aggregate across a buoy cluster locally, then uplink cluster summary
- Model-distillation FL — transmit soft-label distillation datasets instead of gradients
- Rate-distortion theory applied to gradient compression
- Event-triggered FL — only participate when local model diverges significantly
- Personalized FL — train a small local adapter on each buoy, only transmit adapter weights
- FedPAQ / FedAvgM variants tuned for extreme compression
- Semantic communication — transmit meaning-preserving summaries, not raw gradients

**Research agent**: running.

**Resolution criteria**:
- [ ] Verified literature review
- [ ] Target: convergence in <10 rounds at <1 kB/buoy/round uplink
- [ ] Practical protocol for Iridium SBD use case

---

## G6. Perch 2.0 weights not publicly released

**Status**: ⚪ TRACKING (no research needed)
**Severity**: P2 (quality upgrade, not blocker)
**Origin**: `SYNTHESIS.md` §10; analysis in `papers/analysis/perch-bioacoustic.md`.

**Situation**: Hamer 2025 (arXiv:2508.04665) announces Perch 2.0 with multi-taxa training but weights are not yet public. Our v1 plan uses Perch v1 (1280-d, Ghani 2023 Sci. Rep.).

**Action**: Monitor the Perch GitHub repository (google-research/perch or successor) and the authors' Google Scholar for a weights release. When available:
1. Update `sonobuoy-fish` and `sonobuoy-cetacean` HNSW namespace dimensions if changed
2. Re-run v1 Watkins benchmark; accept the upgrade if AUC improves by ≥ 2 pp
3. Update ADR if embedding dimension changes

No research agent required. Reviewer: whoever lands v2 of `clawft-sonobuoy-head`.

---

## G7. External repos (Closure-SDK, coherence-lattice-alpha) AGPL-blocked

**Status**: ⚪ TRACKING (no research needed)
**Severity**: P3 (no current blocker; purely speculative upside)
**Origin**: `SYNTHESIS.md` §7; full writeups in `.planning/development_notes/{closure-sdk-integration,coherence-lattice-alpha-integration}.md`.

**Situation**: Both external repos are AGPL-3.0 licensed; WeftOS's permissive-licensed kernel cannot link them.

**Action**: Monitor quarterly:
- https://github.com/faltz009/Closure-SDK — check LICENSE and releases
- https://github.com/project-89/coherence-lattice-alpha — check LICENSE and release maturity

Revisit only if (a) either relicenses to Apache-2.0 / MIT / BSD, or (b) a concrete WeftOS workload specifically needs a quaternion pose metric (Closure) or Fiedler-channel edge update (coherence-lattice — implementable from Fiedler 1973 primary source without the repo).

No research agent required.

---

## Aggregate status

| Gap | Severity | Status | Closing mechanism |
|-----|----------|--------|-------------------|
| G1 | P0 | 🟢 CLOSED | RANGING.md + ADR-078; cross-cuts into G4 (velocity) and G2/G3 (SSP) |
| G2 | P1 | 🟢 CLOSED | Two-path (XPINN+features online, PINO fleet-amortized); ADR-079 |
| G3 | P1 | 🟢 CLOSED | ThermoFno hybrid (U-NO + MWT + GINO-SDF + PINO); ADR-080 |
| G4 | P1 | 🟢 CLOSED | Two-stage VAE-prior + Zhang-Achim alt-min; G1 ranging as Bayesian soft stabilizer; ADR-079b (parked, see G4 above) |
| G5 | P1 | 🟢 CLOSED | 5-layer codec 210 B/round fits Iridium SBD; ADR-080b (parked) + ADR-090 (build-side operationalization) |
| G6 | P2 | ⚪ | Monitor Perch 2.0 release |
| G7 | P3 | ⚪ | Monitor external-repo relicensing |

**Research agents currently running**: 5 (G1 + G2 + G3 + G4 + G5).

---

## Research assignments and expected deliverables

Each research agent produces:
1. **2-4 verified paper analyses** at `papers/analysis/<slug>.md` (same format as existing analyses, verification-first mandate per ADR-062)
2. **A gap-closing addendum** at `.planning/sonobuoy/gaps/G<N>-<short-name>.md` with: problem restatement, candidate solutions surveyed, recommended approach, integration path into `SYNTHESIS.md`, new ADR candidate
3. **PDF downloads** to `papers/pdfs/` where accessible

Main thread (me) will:
- Update GAPS.md status as each agent reports back
- Fold gap-closing addenda into SYNTHESIS.md §10 / §2 as appropriate
- Reconcile any ADR numbering collisions
- Commit each closure as a separate commit

---

## Update history

- **2026-04-15**: Created. 7 gaps enumerated, 5 research agents in flight.
