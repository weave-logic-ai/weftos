# Sonobuoy Project — Unified Synthesis

**Compiled**: 2026-04-15 (round 1), **re-blended 2026-04-15** after round 2
**Status**: Supersedes the round-1-only SYNTHESIS.md. Round-1 substitutions stand; round-2 adds classical grounding (Urick, Wenz, KRAKEN, BELLHOP, MFP/MUSIC/MVDR), synthetic aperture sonar, soundscape ecology, edge/tiny-ML, and federated learning. The architecture grows a 5th branch (active-imaging) and a 2nd deployment profile (long-term PAM).
**Sources**: 42 paper analyses in `papers/analysis/`, K-STEMIT mapping in `k-stemit-sonobuoy-mapping.md`, FL mini-synthesis merged in from `SYNTHESIS-FL.md`, plus two general-WeftOS evaluations (`closure-sdk-integration.md`, `coherence-lattice-alpha-integration.md`).

---

## 0.5. Hardware / build companion (added 2026-05-11)

A complementary **build / hardware / mechanical / cost** corpus lives
at `./build/` (moved here on 2026-05-11 from `docs/plans/sonar/`).
That corpus grounds the architecture and ADRs in this document in
physical buoy designs:

- **`build/requirements.md`** + **`build/architecture.md`** — base
  2"-PVC vertical buoy, ESP32-S2 (TX) + ESP32-S3 (RX) MCU split,
  1.8 kHz audio-band piezo, vented ballast. **Tier-2 / tier-3 of
  the §3 power hierarchy in physical form.**
- **`build/build-hydrophone-*.md`** — three packaging variants for
  the v1 hydrophone: bare PVC-wall window (simple), epoxy-potted
  JFET puck (Aquarian H1a clone), oil-filled M8-connected sidecar
  (recommended; sensor-mesh-compatible).
- **`build/build-buoy-p79.md`** — Phase 5 imaging-tier prototype:
  Airmar P79 (50 / 200 kHz) + Airmar D 235 kHz (P/N 44-053-1-02),
  shared 3-band pulser + LNA + ADC daughterboard, bistatic mode E.
  **Hardware grounding for ADRs 063 (SAS 5th branch), 064 (deep
  autofocus), 065 (Kiang multistatic with stationary sonobuoy —
  these buoys are the C-nodes in Kiang's geometry).** Phase 5d
  adds a fully-submerged BLDC gimbal in oil; Phase 5e adds a
  cast-urethane acoustic dome.
- **`build/commercial-comparison.md`** — taxonomy of commercial
  sonar classes, gap analysis, visualization surfaces (chart-style
  PPI, 3D water column, AR fly-through, ML-classifier overlay,
  bathymetric mosaic, temporal change detection).
- **`build/phase-economics.md`** — per-phase cost (~$45 → ~$11k for
  10-buoy fleet with two high-band nodes), capability ladder, and
  the **vector-DB-native data-plane advantage** that distinguishes
  clawft from commercial sonar at the system level. References
  §3 (4-tier power), §4 (FL stack), §5.4 (HNSW namespaces) and the
  v1/v2/v3/v4 plan in §8 of this document.
- **`build/roadmap.md`** — Phases 1 / 1b / 2 / 3 / 4 / 5a-f / 6 / 7
  must stay aligned with this document's v1/v2/v3/v4 ladder in §8.

The build corpus is the practical implementation surface on which
the K-STEMIT-extended architecture in this document will run.

---

## 0. Executive summary

- **42 papers analyzed across 12 categories in two rounds.** Round 1 (18 papers) was compiled from LLM memory and 14/18 citations were fabricated; round-2 was verification-first and all 24 new citations were confirmed via arXiv/DOI/publisher/DTIC. ADR-062 formalizes the verification mandate.
- **The architecture grows from 4 to 5 branches.** Round 1 delivered temporal, spatial, physics, and classification-head branches (all magnitude / incoherent). Round 2 adds **active-imaging** — a coherent (phase-preserving) branch for synthetic aperture sonar, seeded by Kiang & Kiang 2022 which explicitly uses a sonobuoy as a node in multistatic SAS.
- **Two deployment profiles emerge**. `sonobuoy-tactical` (hours-days, bearing-first, continuous UHF, expendable) vs `sonobuoy-pam` (months-years, index-first, sparse Iridium daily, refurbishable HARP-class). Different physics-prior weight, different telemetry, different hardware.
- **A 4-tier on-buoy/at-shore power hierarchy is grounded** by Rybakov-2020, MLPerf Tiny 2021, MCUNet 2020, and acoupi 2026: µW analog gate → 5 mW Cortex-M4 trigger → 50 mW Cortex-M7 confirmation → 200 W shore GPU.
- **Classical grounding is now explicit**. Urick's sonar equation is the universal dB loss contract; Wenz curves are the zero-cost noise prior; KRAKEN/BELLHOP are the propagation ground truth that Zheng-2025's FNO surrogate is measured against; Capon 1969 MVDR / Schmidt 1986 MUSIC / Bucker 1976 MFP are the baselines the round-1 GNN-BF / Grinstein-TDOA / Chen-Rao-subspace papers "beat".
- **Federated learning is defined as a layer**. FedAvg (macro-protocol) + Deep Gradient Compression (bandwidth codec) + Multi-Krum (Byzantine-robust aggregator) + Split Learning (edge/shore partitioning). Maps cleanly onto the existing WeftOS mesh substrate (`mesh_*.rs`, Raft, gossip, rvf-crypto).
- **Two external repos evaluated and deferred.** Closure-SDK (quaternion cognitive architecture, AGPL blocker) and coherence-lattice-alpha (physics preprint, AGPL + CC BY-NC + 5 commits + broken internal deps). Recommendation on both: conceptual-only, revisit on relicensing or concrete workload need.
- **Six-figure verified benchmark targets** ground the v1/v2/v3 plan: DEMONet 80.45% DeepShip, SIR+LMR ~80% at -15 dB SNR, Perch 0.98 ROC-AUC Watkins 32-way, BEATs 48.6 mAP AudioSet-2M, Grinstein-2023 29% 4-mic error reduction, Kiang-2022 <3% velocity-vector error at -17 dB SNR.
- **25 ADR candidates** emerge: ADR-053 through ADR-077 (central renumbering performed in §6 after collisions from parallel agent output).

---

## 1. Verification status across both rounds

### 1.1 Round 1 (2026-04-15 morning) — memory-compiled survey

| Section | Original cite | Status | Substitute |
|---------|---------------|--------|------------|
| 1.1 Passive sonar / UATD-Net | Yang et al., IEEE JOE 2024 | Fabricated | DEMONet (Xie 2024, arXiv:2411.02758) |
| 1.2 Smoothness-UATR | Xu/Ren, arXiv:2306.06945 | **Verified** (author corrected to Xu/Xie/Wang) | — |
| 1.3 UATR survey | Luo, arXiv:2503.01718 | Fabricated (arXiv ID is cancer paper) | Feng et al. 2024 (MDPI Remote Sensing 16(17):3333) |
| 2.1 AST fish classification | Waddell, Ecol. Inf. 2024 | Fabricated | SurfPerch (Williams 2024, arXiv:2404.16436) |
| 2.2 FishGraph | Martinez, OCEANS 2024 | Fabricated | Grinstein 2023 (arXiv:2306.16081) |
| 2.3 Echosounder SSL | Brautaset, ICES 2023 | Fabricated | Pala 2024 (Ecol. Inf. 84:102878) |
| 3.1 BirdNET cetaceans | Ghani, MEE 2024 | Fabricated (= Perch paper misremembered) | Ghani 2023 (Sci. Rep. 13:22876) |
| 3.2 NOAA DIFAR Conformer | Allen, JASA-EL 2024 | Fabricated | Allen 2021 (Frontiers Mar. Sci. 8:607321) + Nihal 2025 (arXiv:2502.20838) + Thode 2019 DIFAR |
| 3.3 Orca Siamese | Bergler, Sci. Rep. 2023 | Fabricated | Bergler 2019 (TSD LNAI 11697:274) — autoencoder, not Siamese |
| 4.1 GNN-BF | Tzirakis, ICASSP 2024 | Verified (year correction: 2021, arXiv:2102.06934) | — |
| 4.2 GNN-TDOA uncertain | Comanducci, arXiv:2311.00866 | Fabricated | Grinstein 2023 (same as 2.2) |
| 4.3 Sparse neural BF | Chen/Wang, IEEE TSP 2024 | Fabricated | Chen & Rao 2025 (arXiv:2408.16605) |
| 5.1 PINN/SSP | Yoon, JASA 2024 | Fabricated | Du 2023 (Frontiers Mar. Sci.) |
| 5.2 FNO | Sanford, arXiv:2402.07341 | Fabricated (arXiv ID is bandits paper) | Zheng 2025 (Frontiers) |
| 5.3 Thermocline FiLM | Nguyen, JOE 2023 | Fabricated | Perez 2017 (AAAI) + Vo 2025 (arXiv:2506.17409) |
| 6.1 AudioMAE | Huang, NeurIPS 2022 | **Verified** (arXiv:2207.06405) | — |
| 6.2 BEATs | Chen/Wu, ICML 2023 | **Verified** (arXiv:2212.09058) | — |
| 6.3 Perch | Hamer, arXiv:2307.15008 | Fabricated (arXiv ID is Carlini AI-Guardian) | Ghani 2023 (Sci. Rep. 13:22876) |

**Round 1 verification rate**: 3/18 correct, 1/18 needed a year/author fix, 14/18 required substitution. The architectural buckets survive because substitutes were chosen to preserve the slot-level thesis.

### 1.2 Round 2 (2026-04-15 afternoon) — verification-first

| # | Category | Paper | Status |
|---|----------|-------|--------|
| 7.1 | Classical foundations | Urick, *Principles of Underwater Sound* 3rd ed. (1983) | Verified (ISBN 978-0-932146-62-5 + multi-source) |
| 7.2 | Classical foundations | Wenz 1962, JASA 34(12):1936 | Verified (DOI 10.1121/1.1909155) |
| 7.3 | Classical foundations | Porter 1991/92, KRAKEN (SACLANTCEN SM-245, DTIC AD-A252 409) | Verified |
| 7.4 | Classical foundations | Porter & Bucker 1987 + Porter 2011 BELLHOP (DOI 10.1121/1.395269) | Verified |
| 8.1 | SAS | Hayes & Gough 2009, IEEE JOE review (DOI 10.1109/JOE.2009.2020853) | Verified |
| 8.2 | SAS | Callow 2003, Stripmap PGA thesis (U. Canterbury) | Verified |
| 8.3 | SAS | Gerg & Monga 2021, IGARSS / arXiv:2103.10312 | Verified |
| 8.4 | SAS | Kiang & Kiang 2022, IEEE TGRS (DOI 10.1109/TGRS.2022.3220708) | Verified — **stationary sonobuoy explicitly modeled** |
| 9.1 | MFP/adaptive BF | Bucker 1976, JASA (DOI 10.1121/1.380872) | Verified |
| 9.2 | MFP/adaptive BF | Schmidt 1986, IEEE Trans. AP (DOI 10.1109/TAP.1986.1143830) | Verified |
| 9.3 | MFP/adaptive BF | Capon 1969, Proc. IEEE (DOI 10.1109/PROC.1969.7278) | Verified |
| 9.4 | MFP/adaptive BF | Sun/Fu/Teng 2024, Remote Sensing (DOI 10.3390/rs16081391) | Verified |
| 10.1 | Soundscape/PAM | Pijanowski 2011, BioScience 61(3):203 | Verified (DOI 10.1525/bio.2011.61.3.6) |
| 10.2 | Soundscape/PAM | Sueur 2008, PLoS ONE (DOI 10.1371/journal.pone.0004065) | Verified |
| 10.3 | Soundscape/PAM | Wiggins & Hildebrand 2007 HARP (DOI 10.1109/UT.2007.370760) | Verified |
| 10.4 | Soundscape/PAM | Staaterman 2014, MEPS (DOI 10.3354/meps10911) | Verified (content via multi-source) |
| 11.1 | Edge/tiny-ML | Rybakov 2020 KWS, arXiv:2005.06720 | Verified |
| 11.2 | Edge/tiny-ML | Banbury 2021 MLPerf Tiny, arXiv:2106.07597 | Verified |
| 11.3 | Edge/tiny-ML | Lin 2020 MCUNet, arXiv:2007.10319 | Verified |
| 11.4 | Edge/tiny-ML | Vuilliomenet 2026 acoupi, arXiv:2501.17841 / MEE 17(1):67 | Verified |
| 12.1 | Federated | McMahan 2017 FedAvg, arXiv:1602.05629 | Verified |
| 12.2 | Federated | Lin 2018 Deep Gradient Compression, arXiv:1712.01887 | Verified |
| 12.3 | Federated | Blanchard 2017 Krum, NeurIPS | Verified |
| 12.4 | Federated | Gupta & Raskar 2018 Split Learning (DOI 10.1016/j.jnca.2018.05.003) | Verified |

**Round 2 verification rate**: 24/24 verified. Mandate from ADR-062 works.

---

## 2. Unified architecture (5 branches, 2 deployment profiles)

Round 1 gave us 4 incoherent branches. Round 2's SAS work requires adding a 5th coherent branch, and the soundscape/PAM work requires splitting into two deployment profiles.

```
                                                              +------------------+
                                                              | HNSW retrieval   |
                                                              | (per-namespace:  |
                                                              |  fish 1280,      |
                                                              |  cetacean 1024,  |
                                                              |  vessel 768)     |
                                                              +---------+--------+
                                                                        ^
    PHYSICS PRIOR BRANCH        TEMPORAL BRANCH         SPATIAL BRANCH  |
    +---------------------+     +----------------+      +---------------+
    | Du 2023 Helmholtz   |     | DEMONet MoE    |      | Tzirakis 2021 |
    |  PINN               |     | (Xie 2024)     |      | dyn-adj GCN   |
    | Zheng 2025 FNO      |---->| +SIR+LMR regs  |<---->| Grinstein 2023|
    | Perez 2017 FiLM     |     | +DINO SSL      |      | Rel-Net SLF   |
    | Wenz 1962 noise prior|    | Pretrain:      |      | Chen-Rao 2025 |
    | KRAKEN/BELLHOP GT   |     |  BEATs/Perch   |      | Grassmann DoA |
    +---------------------+     +----------------+      +---------------+
             |                          |                       |
             v                          v                       v
    +--------------------------------------------------------------------+
    |           adaptive alpha fusion (K-STEMIT dual-branch)             |
    +--------------------------------------------------------------------+
             |                                                  |
             v                                                  v
    +---------------------+                     +------------------------+
    | CLASSIFICATION HEAD |                     | ACTIVE-IMAGING BRANCH  |   <-- NEW (round 2)
    |                     |                     |  (phase-coherent)      |
    | SurfPerch fish 1280 |                     |                        |
    | BirdNET-2.3 1024    |                     | Hayes-Gough 2009 SAS   |
    | Bergler orca 512    |                     | Callow 2003 SPGA       |
    | DSMIL localization  |                     | Gerg-Monga 2021 deep   |
    |                     |                     |  autofocus             |
    |  classical detector |                     | Kiang 2022 multistatic |
    |  grounded on        |                     |  w/ stationary buoy    |
    |  Urick signal excess|                     +------------------------+
    +---------------------+
              |
              v
    +---------------------+      Deployment profiles:
    | TACTICAL  |   PAM   |      - Tactical: hours-days, bearing-first,
    |  mode     |  mode   |        continuous UHF, expendable, K-STEMIT full
    |           |         |      - PAM: months-years, index-first (Sueur),
    |           |         |        sparse Iridium daily, HARP-class hardware
    |           |         |        (Wiggins 2007), Staaterman rhythmicity
    +---------------------+
```

### 2.1 Temporal branch

**Encoder**: DEMONet MoE atop ResNet-18 with DEMON-based routing + pretrained cross-temporal VAE. Real SOTA: 80.45% DeepShip, 97.88% DTIL, 84.92% ShipsEar. +0.535 MB params over baseline.

**Pretraining**:
- Perch 1280-d (Ghani 2023 Sci. Rep.) — bioacoustic default. 0.98 ROC-AUC on Watkins 32-way at 32 shots/class.
- BEATs 768-d (arXiv:2212.09058) — general audio default. 48.6 mAP on AudioSet-2M, cleaner embeddings than AudioMAE for retrieval.
- AudioMAE (arXiv:2207.06405) — secondary pretrainer; 16 kHz front-end is a hard ceiling.

**Regularization** (Xu 2023, verified round 1):
- SIR: symmetric-KL between clean and simulated-perturbed posteriors.
- LMR: spectrogram cut-paste augmentation.
- Both hold ~80% accuracy at -15 dB SNR where baselines collapse. ~40 LOC each.

**Label-free SSL**: Pala 2024 DINO + intensity-sampling. 77.55% frozen-kNN vs 71.93% supervised.

### 2.2 Spatial / graph branch

Three-stage pipeline, all real:

1. **Tzirakis 2021** learned-adjacency GCN. Beats CRNN-C by 4.92 dB SDR at -7.5 dB SNR. Drop-in for static haversine GraphSAGE.
2. **Grinstein 2023** Relation-Network SLF: pair-wise `F(x_i, x_j; φ) = MLP(GCC-PHAT + metadata)` summed then fused. 29% reduction on 4-mic arrays, generalizes to unseen counts.
3. **Chen-Rao 2025** Grassmannian subspace DoA: WRN-16-8 → Gram matrix → principal-angle loss. Resolves `M-1` sources with `N<M` sensors via MRA co-array.

Classical baselines: **Capon 1969 MVDR** gives `w = R̂⁻¹a / (aᴴR̂⁻¹a)`; **Schmidt 1986 MUSIC** establishes signal/noise subspace orthogonality (Chen-Rao is a direct neural generalization); **Bucker 1976 MFP** turns multipath into signal via Green's-function replicas (informs a drifting-buoy adaptation).

### 2.3 Physics-prior branch

- **Du 2023 Helmholtz-PINN**: solves `∇²ψ + k(z)²ψ = f`, `k = ω/c(z)`, conditioned on SSP. 2D R²=0.99 / ~1 dB TL error. 3D collapses to R²=0.48 — use 2D only first pass.
- **Zheng 2025 FNO**: real speedup is **28.4%** at <0.04 dB RMSE, 4 Fourier modes, 6-channel input `(Re ψ, Im ψ, k, ρ, r, z)`. **Not 1000×** (round-1 survey fabrication). Fails under strong vertical SSP gradients (thermocline regime).
- **FiLM conditioning**: Perez 2017 canonical `γ ⊙ F + β` + Vo 2025 SWELLEX-96 for environment vector. 8-dim: thermocline depth, mixed-layer gradient, sea state, bottom type, wind, current, SST, salinity. Init γ-bias=1, β-bias=0 preserves baseline.
- **Wenz 1962 noise prior**: three-band turbulent/shipping/wind/thermal decomposition. Zero-cost noise augmentation; ~60 lines of Rust.
- **Propagation ground truth**: KRAKEN (range-independent deep water) + BELLHOP (range-dependent, shallow, mid/high freq). Both are the real solvers the ML surrogates are benchmarked against.

### 2.4 Active-imaging branch (NEW, round 2)

The 5th branch — phase-coherent, distinct from the 4 incoherent branches.

- **Hayes & Gough 2009 IEEE JOE review**: SAS vocabulary — stripmap vs spotlight, PRF/range/velocity coupling, 6-class error taxonomy, three reconstruction families (RDA, omega-k, chirp scaling).
- **Callow 2003 SPGA**: stripmap PGA via target-region windowing + wavenumber remap. Diffraction-limited D/2 in 3-4 iterations. DPCA/RPC micronavigation generalizes to inter-buoy coherence.
- **Gerg & Monga 2021 deep autofocus**: DenseNet-121+MLP predicts 10-deg phase polynomial in one forward pass, self-supervised sharpness-improvement loss. 19× faster than iterative.
- **Kiang & Kiang 2022 multistatic SAS with sonobuoy**: the most relevant SAS paper — explicit transceiver A + towed receiver B + **stationary sonobuoy C** geometry. Non-stop-and-go range models, joint Doppler-centroid velocity-vector estimation, <3% error at -17 dB SNR.

### 2.5 Classification head

- **Watkins Marine Mammal** (32-way): Perch → 0.98 ROC-AUC at 32 shots/class (Ghani 2023).
- **Orca call types** (12-way): Bergler 2019 autoencoder + cross-entropy head → 96% best / 94% mean.
- **Fish species** (12-way reef): SurfPerch (Williams 2024) → 0.900 AUC-ROC at 4 shots.
- **Binary humpback**: Allen 2021 ResNet-50 + PCEN + 4-round active learning → AP 0.97 / AUC 0.992 on 187,000 h NOAA HARP.
- **Bearing estimate**: DIFAR phase-difference channel layout (Thode 2019 azigrams) + Grinstein SLF + Chen-Rao refinement.

Head is grounded in **Urick's signal excess** `SE = SL - TL + TS - (NL - DI) - DT` — the universal dB loss contract. Every ML head output must be convertible to/from this.

### 2.6 Deployment profiles

| | `sonobuoy-tactical` | `sonobuoy-pam` |
|---|---|---|
| Lifetime | hours to days | months to years |
| Compute priority | bearing + species-ID first | indices first, species-ID batched |
| Telemetry | continuous UHF | sparse Iridium daily |
| Hardware | expendable, low-cost | refurbishable, HARP-class (Wiggins 2007: 200 kHz/16-bit, 250 mW, 1-year-at-30-kHz) |
| Physics prior | heavy (full K-STEMIT) | minimal (Wenz + SSP only) |
| Pipeline | detect → bearing → species | acoustic-index telemetry → batch species-ID on recovery |
| Index set | — | Sueur H (`H = Ht · Hf`), Pieretti ACI, Kasten NDSI |
| Rhythmicity analysis | — | Staaterman diel/lunar (27.32-day sidereal-lunar cycle) |

---

## 3. 4-tier power hierarchy (edge/tiny-ML grounding)

The round-1 synthesis assumed at-shore GPU inference. Round 2's edge/tiny-ML papers ground the actual split:

| Tier | Power | Hardware | Model | Purpose |
|------|-------|----------|-------|---------|
| 1 | 10-50 µW | analog gate / Cortex-M0+ | — | RMS level trigger |
| 2 | 5 mW | Cortex-M4 @ 120 MHz (e.g., NUCLEO-L4R5ZI) | MLPerf Tiny DS-CNN (52 KB INT8, 91.6% KWS, ~100 µJ/inf) | Always-on 4-class trigger |
| 3 | 50 mW | Cortex-M7 (e.g., STM32H743) | MCUNet-distilled student (~470 KB, 70 KB SRAM, 91% Speech Commands @ 10 FPS) | 16-class on-buoy confirmation |
| 4 | 200 W | shore-side GPU | Full K-STEMIT + DEMONet + Perch + active imaging | Species ID, tracking, SAS, physics |

Rybakov 2020 streaming-KWS pattern (Keras Stream-wrapper) is the canonical on-buoy trigger architecture; acoupi 2026 is the canonical detect-and-transmit-metadata-only pattern (60 KB/day vs 100 GB/day of raw audio).

---

## 4. Federated learning layer (across buoys)

Sonobuoys' uplink is ~kbps at best. Centralized training requires a federated stack.

- **FedAvg** (McMahan 2017): weighted parameter averaging, 10-100× fewer rounds than FedSGD. Baseline macro-protocol.
- **Deep Gradient Compression** (Lin 2018): top-0.1% sparsification + residual accumulation + momentum correction/clip/mask/warmup. **270×-600× bandwidth cut at zero accuracy loss.** Makes FedAvg fit in a radio window.
- **Multi-Krum** (Blanchard 2017): pick densest cluster (`2f+2 < n`). Drop-in replacement for FedAvg's averaging step; provably Byzantine-robust.
- **Split Learning** (Gupta & Raskar 2018): cut at stem/trunk boundary. Buoy runs ~10% of FLOPs, ships activation tensors (KB not MB). Convergence identical to centralized.

### 4.1 WeftOS mesh integration

- **Service advertisement** (`mesh_service_adv.rs`) + gossip (`mesh_kad.rs`) handle FL client discovery/selection.
- **Raft leader** (`mesh_chain.rs`) runs Multi-Krum aggregation; each FL round becomes one Raft log entry + exochain anchor.
- **rvf-crypto** (identity/signing) is the complement to Krum — identity plane vs gradient plane.
- Two new crates: `weftos-sonobuoy-fl-codec` (DGC) and `weftos-sonobuoy-fl-aggregator` (Multi-Krum/Bulyan).

---

## 5. Mapping to WeftOS subsystems

### 5.1 ECC (`clawft-kernel/src/{causal,eml_*,hnsw_*,quantum_*,mesh_*}.rs`)

| ECC component | Paper-derived extension |
|---------------|-------------------------|
| `CausalGraph` | Buoy-array graph: nodes = buoys, edges = GCC-PHAT or haversine-propagation-delay. Reuses `causal.rs` `add_node/link/traverse`. |
| `CognitiveTick` | Sonobuoy tick: SENSE (audio) → EMBED (BEATs/Perch) → GRAPH-UPDATE (TDOA edges) → FUSE (K-STEMIT) → CLASSIFY (HNSW). PAM-mode tick at 0.1 Hz vs tactical 20 Hz. |
| `CrossRef` | Cross-links: detection → track → species ID → vocalization cluster → rhythmicity bucket. |
| `Impulse` queue | Asynchronous detection events; tactical propagates to DEMOCRITUS; PAM writes to daily index. |
| `mesh_*.rs` | FL protocols (see §4.1). |
| `quantum_register` | Buoy-array register — same abstraction as atom register. Swap force-directed layout for GPS-derived layout. |

### 5.2 EML (`crates/eml-core/` + 17 wrappers)

Seven new EML-core roles, grown from 6 in the round-1 synthesis:

1. `eml_core::regularizers::SmoothnessPenalty` — Xu 2023 SIR
2. `eml_core::regularizers::LocalMaskReplicate` — Xu 2023 LMR
3. `eml_core::operators::helmholtz_residual` — Du 2023 Helmholtz residual via AD. Cache key `(ssp_hash, freq, bathy, src_depth)`.
4. `eml_core::operators::fourier_neural_op` — Zheng 2025 FNO, ONNX-exportable.
5. `eml_core::operators::wenz_noise_model` — **NEW**, Wenz 1962 three-band noise spectrum; zero-cost noise augmentation.
6. `eml_core::conditioning::FiLM` — Perez 2017 `γ ⊙ F + β` with init γ-bias=1, β-bias=0.
7. `eml_core::aggregators::RelationNetwork` — Grinstein 2023 pair-wise `Σ F(x_i, x_j; φ)`.

Plus two new `eml_core::coherence::Penalty` impls implied by round 2:
- `UrickSignalExcess` — constrains classifier confidence to Urick's SE framework.
- `RhythmicityPrior` — Staaterman diel/lunar phase-consistency loss for PAM-mode.

### 5.3 Quantum cognitive layer (`quantum_register.rs`, `quantum_backend.rs`)

- `build_register` works unchanged for buoy-array registers — `RegisterConstraints` applies to both atom and buoy layouts.
- BEATs discrete tokens (K=1024) map onto `HypothesisSuperposition` basis states — 1024-state quantum basis per frame.
- Chen-Rao Grassmann DoA is a classical analog of reduced density matrix `ρ = |ψ><ψ|` on atom-ordered basis; a future integration runs it on Pasqal EMU_FREE.
- **Quantum walk for multi-target data association** — `k` sources across `N` buoys is a graph-matching problem, natural for `quantum_state.rs` walks. P1 use case for Pasqal beyond ECC-only scope.
- **New**: classical SAS synthetic-aperture / sonobuoy-graph connection. The Kiang multistatic geometry is a 3-body phase-coherent problem; phase accumulation matches quantum interference structure. Worth a research exploration (not an ADR yet).

### 5.4 HNSW (`hnsw_service.rs`) — dimension strategy

Per-namespace dimensions, grown from round-1 plan:

| Namespace | Dim | Source |
|-----------|-----|--------|
| `sonobuoy-fish` | 1280 | Perch / SurfPerch |
| `sonobuoy-cetacean` | 1024 | BirdNET-2.3 |
| `sonobuoy-vessel` | 768 | BEATs |
| `sonobuoy-orca-calltype` | 512 | Bergler 2019 autoencoder |
| `sonobuoy-pam-index` | 8-32 | Sueur H + ACI + NDSI + derived |
| `memory-search` | 384 | generic |

Round-1 proposed three options (raise default, per-namespace, projection). Pick per-namespace + optional InfoNCE 1280→384 projection as a v2 optimization.

### 5.5 Mesh (`mesh_*.rs`, `chain.rs`) — FL integration

See §4.1. The four FL papers all compose over the existing WeftOS mesh substrate — no net-new infrastructure is required beyond two small crates (`fl-codec`, `fl-aggregator`) and one protocol extension to the Raft log entries to carry compressed gradients.

---

## 6. Unified ADR numbering

Central renumbering to resolve collisions. Current last-assigned ADR per `MEMORY.md` is ADR-047; the KG sprint reserved 048-052. Sonobuoy project picks up at ADR-053.

| ADR | Title | Paper source | Sprint |
|-----|-------|--------------|--------|
| **ADR-053** | Dual-branch spatio-temporal architecture for sensor systems | K-STEMIT (Liu & Rahnemoonfar 2026, arXiv:2604.09922) | Foundational |
| **ADR-054** | DEMONet MoE as sonobuoy temporal encoder | Xie 2024, arXiv:2411.02758 | v2 |
| **ADR-055** | SIR + LMR regularizers in eml-core | Xu 2023, arXiv:2306.06945 | v2 |
| **ADR-056** | Dynamic-adjacency GCN for variable sonobuoy arrays | Tzirakis 2021, arXiv:2102.06934 | v2 |
| **ADR-057** | Relation-Network aggregation for count-invariant sensor graphs | Grinstein 2023, arXiv:2306.16081 | v3 |
| **ADR-058** | Grassmannian subspace DoA back-end | Chen & Rao 2025, arXiv:2408.16605 | v3 |
| **ADR-059** | Helmholtz-PINN physics-prior branch | Du 2023 Frontiers Mar. Sci. | v3 |
| **ADR-060** | FiLM conditioning on ocean environment | Perez 2017 + Vo 2025 arXiv:2506.17409 | v3 |
| **ADR-061** | DINO + intensity sampling SSL for hydrophone data | Pala 2024, Ecol. Inf. 84:102878 | v3 |
| **ADR-062** | Literature verification required before ADR citation | policy (from round-1 fabrication finding) | v1 |
| **ADR-063** | Active-imaging (SAS) 5th branch in K-STEMIT-extended architecture | Hayes & Gough 2009 IEEE JOE review | v3 |
| **ADR-064** | Deep SAS autofocus as primary phase-error corrector | Gerg & Monga 2021, arXiv:2103.10312 | v3 |
| **ADR-065** | Multistatic SAS with stationary sonobuoy as receiver node | Kiang & Kiang 2022, IEEE TGRS | v4 |
| **ADR-066** | Wenz 1962 three-band noise prior as zero-cost augmentation | Wenz 1962, JASA 34(12):1936 | v1 |
| **ADR-067** | Classical propagation solvers (KRAKEN/BELLHOP) as ground truth | Porter 1991/92 + Porter 2011 | v3 |
| **ADR-068** | Urick signal-excess as universal loss contract | Urick 1983 (3rd ed.) | v2 |
| **ADR-069** | 4-tier on-buoy/at-shore power split | Rybakov + MLPerf Tiny + MCUNet + acoupi | v2 |
| **ADR-070** | Dual deployment profiles: `sonobuoy-tactical` vs `sonobuoy-pam` | Pijanowski 2011 + Staaterman 2014 | v1 |
| **ADR-071** | Acoustic indices (Sueur H + Pieretti ACI + Kasten NDSI) as primary PAM telemetry | Sueur 2008 | v2 |
| **ADR-072** | HARP-class hardware profile for `sonobuoy-pam` | Wiggins & Hildebrand 2007 | v2 |
| **ADR-073** | Rhythmicity pipeline (diel/lunar/seasonal) as mandatory PAM workflow | Staaterman 2014 | v3 |
| **ADR-074** | FedAvg as FL macro-protocol | McMahan 2017, arXiv:1602.05629 | v3 |
| **ADR-075** | Deep Gradient Compression codec | Lin 2018, arXiv:1712.01887 | v3 |
| **ADR-076** | Multi-Krum Byzantine-robust aggregator | Blanchard 2017 NeurIPS | v3 |
| **ADR-077** | Split Learning stem/trunk architecture | Gupta & Raskar 2018 | v4 |

**25 ADRs total**. Sprint team may merge related ADRs (e.g., collapse 074-077 into one "Federated learning protocol stack" ADR if preferred).

---

## 7. External-project evaluations

Two general-WeftOS integration evaluations completed alongside the sonobuoy work.

### 7.1 Closure-SDK (faltz009/Closure-SDK)

Full doc: `.planning/development_notes/closure-sdk-integration.md`.

- **What it is**: Rust + Python quaternion-based cognitive architecture. ~12k lines, 43 commits, single author, ~4 weeks old. 55 Rust + 151 Python tests pass. Five-layer stack (substrate → memory → execution → brain → learning), three-cell ingest loop, DNA/epigenetic genome, Hopf W/RGB incident classification, σ-gap prediction error.
- **Closest WeftOS adjacency**: `quantum_state.rs` (both use continuous manifolds and coherence metrics, but at very different scales — 4 reals vs 2N complex amplitudes).
- **Blocker**: **AGPL-3.0**. Incompatible with WeftOS's permissive-licensed kernel.
- **Recommendation**: **defer, conceptual-only**. Monitor for relicensing. Don't port speculatively. Revisit if (a) license changes or (b) a concrete workload (robotics pose composition, packet integrity, cognitive-state coherence) specifically needs a 4-d manifold metric.

### 7.2 coherence-lattice-alpha (project-89/coherence-lattice-alpha)

Full doc: `.planning/development_notes/coherence-lattice-alpha-integration.md`.

- **What it is**: Not a library. A 5-commit single-author physics preprint attempting to derive α = 1/137.036 from a classical oscillator model on the diamond lattice. ~1.5k LaTeX lines, ~28 NumPy/SciPy verification scripts, two of which hard-depend on an unpublished private module and won't run from a fresh clone.
- **WeftOS contact**: paper's "Fiedler channel" uses graph-Laplacian Fiedler vector as a structural forcing term on edges. WeftOS's existing `eml_coherence.rs` predicts the Laplacian's algebraic connectivity λ₂. Same object, opposite directions — overlap is from standard spectral graph theory (Fiedler 1973), not from anything novel in this repo.
- **Blockers**: **AGPL-3.0** code + **CC BY-NC 4.0** paper/figures. Also category (preprint, not library) and maturity (5 commits, internal-path breakage, load-bearing steps labeled "conjecture" by the author).
- **Recommendation**: **defer, conceptual-only**. Single transferable idea (two-channel Shannon + budget-conserving Fiedler edge update) is a reasonable pattern but can be implemented cleanly from Fiedler 1973 primary source without needing this repo. No ADR or sprint change. Optional: add a disambiguation note in `eml_coherence.rs` module doc so readers don't confuse WeftOS's λ₂-based "coherence" with Sharpe's physics-based "coherence."

---

## 8. Expanded v1 → v4 plan

### 8.1 v1 — shortest path to working detector (~2 weeks)

**Same as round-1 plan, grounded by classical benchmarks:**

1. `crates/clawft-sonobuoy-head` scaffold.
2. Perch ONNX loader via existing `ort` crate.
3. HNSW namespace `sonobuoy-fish` (1280-d), seeded from Watkins Marine Mammal.
4. One-shot classifier: embed → k-NN → majority vote.
5. **Target**: reproduce Ghani 2023's ROC-AUC ≥ 0.95 on 32-way Watkins at 32 shots/class.
6. **New from round 2**: add Wenz 1962 noise-model augmentation (~60 LOC Rust, free quality boost).
7. **New from round 2**: add Urick signal-excess calibration on classifier outputs — report SE in dB alongside posterior.

### 8.2 v2 — K-STEMIT dual-branch with real regularizers (~6-8 weeks)

1. `clawft-sonobuoy-temporal` with DEMONet MoE. Port from open-source reference, retrain on DeepShip.
2. `clawft-sonobuoy-spatial` with Tzirakis learned-adjacency GCN.
3. Learnable α fusion; train end-to-end on DeepShip + ShipsEar.
4. SIR + LMR regularizers from day one (ADR-055, in `eml_core::regularizers::*`).
5. 4-tier on-buoy/shore power split (ADR-069). Tier-2 MLPerf DS-CNN trigger on-buoy; Tier-4 full K-STEMIT at shore.
6. **New from round 2**: PAM deployment profile (ADR-070), Sueur indices as daily telemetry, HARP-class hardware reference.
7. **Targets**: DEMONet 80.45% DeepShip and 84.92% ShipsEar (within 2 pp); Tier-2 on-buoy trigger at <5 mW continuous; PAM profile transmits <100 KB/day.

### 8.3 v3 — Physics + active imaging + FL (~3-4 months)

1. `clawft-sonobuoy-physics` with Helmholtz-PINN (2D) + FNO (outside mixed layer) + FiLM.
2. `clawft-sonobuoy-spatial` gains Grinstein Relation-Network + Chen-Rao Grassmann DoA.
3. `clawft-sonobuoy-active` **NEW** crate for SAS imaging:
   - SPGA stripmap autofocus (Callow 2003) as classical baseline.
   - Gerg-Monga 2021 deep autofocus as default phase-error corrector.
   - Kiang 2022 multistatic formalism as core model.
4. `clawft-sonobuoy-fl-codec` + `clawft-sonobuoy-fl-aggregator` FL stack.
5. DINO + intensity-sampling SSL pretraining on aggregated unlabeled buoy data.
6. **Targets**: beat DEMONet baseline by ≥ 3 pp at <-10 dB SNR; SAS autofocus at >5 frames/sec on shore GPU; Multi-Krum FL rounds with <100 kB/buoy/round.

### 8.4 v4 — Full stack, research directions (6+ months)

1. Full N-buoy multistatic SAS with Kiang geometry generalized to N nodes.
2. Quantum walk on the buoy graph for multi-target data association (Pasqal EMU_FREE).
3. Chen-Rao Grassmann subspace on Pasqal QPU for `k > N` refinement.
4. Federated DINO pretraining across tactical + PAM buoys.
5. Research: quantum-token-based acoustic representation (BEATs tokens → `HypothesisSuperposition` basis).

---

## 9. Second-degree references worth following

### 9.1 Foundation-model lineage

- **ViT** (Dosovitskiy 2021, arXiv:2010.11929)
- **MAE** (He 2022, arXiv:2111.06377)
- **wav2vec 2.0** (Baevski 2020, arXiv:2006.11477)
- **DINO / DINOv2** (Caron 2021, arXiv:2104.14294 / Oquab 2023, arXiv:2304.07193)
- **BIRB** bioacoustic benchmark (Hamer 2023, arXiv:2312.07439)

### 9.2 Graph neural network lineage

- **GraphSAGE** (Hamilton 2017, arXiv:1706.02216)
- **GAT** (Veličković 2018, arXiv:1710.10903)
- **Kipf-Welling GCN** (2017, arXiv:1609.02907)
- **MPNN** (Gilmer 2017, arXiv:1704.01212)

### 9.3 Classical underwater-acoustics references

- **Jensen/Kuperman/Porter/Schmidt 2011** — *Computational Ocean Acoustics* (2nd ed.), the canonical textbook that unifies KRAKEN, BELLHOP, and RAM.
- **Van Trees** — *Detection, Estimation, and Modulation Theory* (4 vols). The Neyman-Pearson / ROC lineage behind Urick's detection threshold `DT`.
- **Baggeroer/Kuperman/Mikhalevsky 1993** — MFP tutorial review, IEEE JOE.
- **Dahl & Dall'Osto 2025** — 60-year JASA retrospective on Wenz curves (ambient noise).

### 9.4 Physics-informed neural networks

- **Raissi 2019** J. Comp. Phys. 378:686 — original PINN.
- **Li 2020, arXiv:2010.08895** — original FNO.
- **Perez 2017, arXiv:1709.07871** — canonical FiLM.

### 9.5 Federated learning lineage

- **FedProx** (Li et al. 2020, MLSys) — stable FedAvg under heterogeneous clients.
- **FedMD** (Li & Wang 2019) — model distillation FL for heterogeneous architectures.
- **SCAFFOLD** (Karimireddy et al. 2020) — variance reduction for FedAvg.
- **FedGen** (Zhu 2021) — generative FL under extreme data heterogeneity.

### 9.6 SAS lineage

- **Callow 2003** (thesis, U. Canterbury) — the definitive SPGA derivation.
- **Jakowatz et al. 1996** — *Spotlight-Mode Synthetic Aperture Radar: A Signal Processing Approach*. The PGA foundation Callow extends.
- **Porter 2019** JASA 146:2016 — BELLHOP3D, relevant when SAS geometry needs full-3D propagation.

### 9.7 Bioacoustic data sources

- **Watkins Marine Mammal Sound Database** (public).
- **NOAA HARP archive** (187,000 h, public).
- **MobySound archive**.
- **DeepShip / ShipsEar / OceanShip** — canonical UATR benchmarks (flagged by Feng 2024).
- **Xeno-Canto** — birds; source of Perch pretraining corpus.

---

## 10. Honest limitations and risks

- **14/18 round-1 citations were fabricated**. Substituted with real papers in the same architectural slots; architecture thesis holds but the reader must not trust any round-1 numerical claim without checking against the analysis files. Round 2 was verification-first; 24/24 verified.
- **Four unsupported round-1 claims** (still true after round 2 re-examination): Sanford "1000× FNO speedup" (real: 28.4%), Nguyen "6-12 dB FiLM SNR gain" (no literature source), Martinez FishGraph, Waddell AST fish. All these numerical/citation claims must not land in any ADR.
- **Sensor-position uncertainty is a gap**. Grinstein 2023 is excellent at TDOA inference but does not output calibrated uncertainty. Budget 2-3 weeks of v3 work for training-time Gaussian GPS-noise augmentation + joint optimization at inference.
- **Helmholtz-PINN 3D collapses to R²=0.48**. Use Du 2023 in 2D first-pass only; couple with Zheng FNO for 3D regions where thermocline gradients are weak.
- **Zheng FNO fails under strong vertical SSP gradients** — the thermocline regime sonobuoys operate in. Keep Zheng FNO for the BL-above-thermocline regime only; use KRAKEN/BELLHOP at shore for through-thermocline.
- **Kiang 2022 assumes known platform velocities** for its non-stop-and-go range model. Drifting buoys don't know their velocity precisely. v4 multistatic SAS needs a simultaneous-velocity-and-image reconstruction that Kiang doesn't provide — this is the open problem.
- **FL under ~kbps uplink is not solved by combining the four FL papers verbatim**. DGC assumes tens of kbps. Sonobuoy RF back-haul is 100s-1000s bps for satellite, ~10-100 kbps for line-of-sight UHF. Need a further compression layer (quantization + error-coding) beyond DGC — budget as a v3/v4 research problem.
- **Both external evaluated repos are AGPL**. Closure-SDK and coherence-lattice-alpha. Permissive-kernel constraint blocks adoption. If either relicenses, revisit.
- **Two round-2 PDFs did not download**: Staaterman 2014 (MEPS subscription) and Feng 2024 (MDPI anti-bot). Content was extracted from multi-source abstracts + mirrors. The analyses document this.
- **Perch 2.0** (Hamer 2025, arXiv:2508.04665) is announced with multi-taxa training but weights are not yet public. Track and swap when available.

---

## 11. Immediate next actions

1. **Commit** this unified SYNTHESIS.md (supersedes round-1) + 24 new round-2 analyses + 2 external-project evaluations + FL mini-synthesis (to be folded into this doc and then deleted).
2. **Update `papers/survey.md`** banner pointing to this synthesis; mark round-1 citations with their verification status in-place.
3. **Draft the 25 ADRs** per §6. Sprint team decides whether to merge related ADRs (e.g., collapse the four FL ADRs into one "FL protocol stack" ADR).
4. **Scaffold `crates/clawft-sonobuoy/`** as a feature-gated umbrella crate per §5 of round-1 SYNTHESIS (still valid), grown now to include `clawft-sonobuoy-active` for SAS.
5. **v1 spike** per §8.1: Perch ONNX + HNSW one-shot classifier + Wenz augmentation + Urick signal-excess reporter. Target: Watkins ROC-AUC ≥ 0.95 by end of next sprint.
6. **Add disambiguation note** to `clawft-kernel/src/eml_coherence.rs` module doc distinguishing WeftOS's λ₂ "coherence" from Sharpe's physics "coherence" (reactive to the coherence-lattice-alpha evaluation).
7. **Delete `SYNTHESIS-FL.md`** after confirming its content is fully merged into §4 of this unified doc.

---

## 12. File inventory

### Round-1 analyses (18, all re-grounded against verified substitutes)

`ast-fish-classification.md`, `audio-mae.md`, `beats.md`, `birdnet-cetaceans.md`, `echosounder-ssl.md`, `fishgraph-gnn.md`, `fno-propagation.md`, `gnn-bf.md`, `gnn-tdoa-uncertain.md`, `neural-beamforming-sparse.md`, `noaa-difar-conformer.md`, `orca-siamese.md`, `perch-bioacoustic.md`, `pinn-ssp-helmholtz.md`, `smoothness-uatr.md`, `thermocline-film.md`, `uatd-net.md`, `uatr-survey-2025.md`.

### Round-2 analyses (24, all verification-first)

**Classical foundations**: `urick-sonar-equation.md`, `wenz-ambient-noise.md`, `kraken-propagation.md`, `bellhop-ray-tracing.md`.
**SAS**: `hayes-gough-sas-review.md`, `sas-autofocus-spga.md`, `ml-sas-autofocus.md`, `multistatic-sas.md`.
**MFP/adaptive BF**: `bucker-mfp.md`, `schmidt-music.md`, `capon-mvdr.md`, `modern-ml-mfp.md`.
**Soundscape/PAM**: `pijanowski-soundscape.md`, `sueur-acoustic-indices.md`, `wiggins-harp.md`, `staaterman-diel-lunar.md`.
**Edge/tiny-ML**: `tiny-ml-audio-kws.md`, `mlperf-tiny-benchmark.md`, `mcunet-tinyml.md`, `bioacoustic-edge-acoupi.md`.
**Federated**: `fedavg-foundations.md`, `deep-gradient-compression.md`, `byzantine-robust-krum.md`, `split-learning.md`.

### External-project evaluations (2)

`.planning/development_notes/closure-sdk-integration.md`, `.planning/development_notes/coherence-lattice-alpha-integration.md`.

### PDFs

33 of 42 papers have PDFs in `papers/pdfs/` (gitignored). 9 did not download due to paywalls, auth, or anti-bot measures. Each affected analysis documents the source used instead.
