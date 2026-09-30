# Passive Channel Charting (arXiv 2504.09924)

Euchner, Kellner, Stephan, ten Brink. "Passive Channel Charting: Locating Passive Targets using Wi-Fi Channel State Information." arXiv:2504.09924v2, 24 Apr 2025 (v1 14 Apr 2025), cs.IT / eess.SP. https://arxiv.org/abs/2504.09924 . Five pages. Accepted at SPAWC 2025 per the repo README and the AR paper's reference list [V].

Tags: [V] read in the source with location; [I] inference.

## Problem

Localise a passive target (a person or object that transmits nothing) from CSI measured between static transmitters and static receivers. Existing WiFi CSI approaches for passive sensing are mostly supervised fingerprinting, which needs labelled data and generalises poorly [V, Sec. I, V]. The paper argues that channel charting, a self-supervised dimensionality-reduction method for active transmitters, carries over unchanged: "the underlying principles ... are also applicable to a scenario with immobile transceivers in a static environment that contains a passive ... mobile entity" [V, Sec. I]. Only one earlier work (WiCluster, GLOBECOM 2021) is cited as similar [V, Sec. I-A].

## Method

### Setup and data [V, Sec. II]

- Dataset espargos-0007. Four ESPARGOS arrays (each 2x4), synchronised in frequency, time and phase. Four ceiling-mounted WiFi transmitters that are "neither synchronized to each other nor to the receivers and could be interpreted as non-cooperative access points".
- Target: either a robot wrapped in aluminium foil (to raise reflected energy) or a human, moving with fixed upright orientation. The measurement area is about 4.5 m x 4.5 m.
- Datapoint: (H, x, t, i_TX), with H in C^(B x Mr x Mc x N_sub), B = 4, Mr = 2, Mc = 4, N_sub = 53 non-zero L-LTF subcarriers. Carrier 2.472 GHz (WiFi channel 13). Bandwidth about 16.56 MHz (the occupied bandwidth of a 20 MHz channel). Target height is assumed known. Array centre positions z(b) and boresights n(b) are known.
- Splits: robot training 482,882 datapoints; robot test 139,427; human test 33,011.
- Labels x are used for evaluation only, except the supervised baseline which trains on them [V, Sec. II].

### Clutter removal (CRAP) [V, Sec. III]

- The direct paths and static reflections dominate. Clutter removal is hard because transmitters and receivers are not synchronised. The paper applies CRAP (Clutter Removal with Acquisitions Under Phase Noise, Henninger et al., 6GNet 2023), separately per transmitter.
- Vectorise CSI: h in C^Q, Q = B Mr Mc N_sub. Compute R = sum_l h h^H. Take the eigenvectors of the K largest eigenvalues as the clutter subspace C-hat in C^(Q x K), K being the "clutter order". K is not stated in the paper (the repo uses 2).
- Remove clutter: h_tgt = h - C-hat C-hat^H h.
- Deviation from CRAP: CRAP assumes an empty-room acquisition, but here it is applied to data with the target present and moving [V, Sec. III].
- Time clustering: datapoints within 1 s windows form a cluster containing CSI from all four transmitters. Cluster timestamp and position label are means over the window [V, Sec. III].

### Baseline 1: classical triangulation [V, Sec. IV]

- Per cluster c and array b, an azimuth array covariance R(c,b), summed over rows, subcarriers and datapoints of the cluster. Azimuth AoA alpha-hat(c,b) by root-MUSIC with a single-source assumption.
- Likelihood under von Mises angle errors: L_tri(x) = product over b of exp(kappa cos(angle_az(x - z(b), n(b)) - alpha-hat)) / (2 pi I0(kappa)). kappa is "heuristically derived from the magnitude of the root found with root-MUSIC". The position is the numerical argmax [V, Eq. 1].
- Not used: time or phase of arrival, Doppler [V].

### Baseline 2: supervised fingerprinting [V, Sec. V]

- Features: FFT over subcarriers of clutter-rejected CSI, keep N_tap = 12 taps (taps 22 to 34). Per cluster, transmitter, array and tap, a covariance-like matrix F = sum over l of vec(H') vec(H')^H (size 8x8 per array-tap). Real and imaginary parts vectorised to a feature vector of length 2 N_TX B N_tap (Mr Mc)^2.
- Dense network 1024-512-256-128-64 ReLU, linear 2-neuron output, MSE loss (Fig. 3a).

### Passive channel charting [V, Sec. VI]

- Dissimilarity: extension of the cosine-similarity metric of Le Magoarou (2021) over arrays and rows and columns: d_CS = B - sum over b, mr, mc of |H-bar_i^* H-bar_j|^2 / (||H-bar_i,b||_F^2 ||H-bar_j,b||_F^2). Subcarriers are combined per cluster first with a "subspace-based interpolation" into H-bar in C^(B x Mr x Mc). Phase-insensitive by construction, which is what makes it usable without transmitter phase sync [I].
- Fused with the cluster timestamp difference (Stephan et al., IEEE TCOM 2024) and made geodesic (Stahlke et al., 2023), giving d_CS-fuse,geo used for training.
- FCF: same dense architecture as the supervised net, but trained self-supervised as a Siamese pair (Fig. 3b) with L_siam(x, y) = (d - ||y - x||_2)^2 / (d + beta). beta trades absolute against normalised squared error. beta value is not given in the paper.
- Augmented PCC: use triangulation output to scale dissimilarities to metres, and combine the losses: L_comb = (1 - lambda) L_siam - lambda (L_tri(y) + L_tri(x)). The network then predicts in global coordinates directly. lambda is not given in the paper [V, Eq. 2-3].
- Evaluation of plain PCC uses an optimal affine transform T_opt fitted with labels, evaluated only at the end [V, Sec. VI-B].

### How passive/bistatic geometry is handled

It is not modelled. Each cluster's clutter-removed CSI from all Tx-Rx pairs is treated as a feature vector of target state. The target perturbs the channel by blocking or adding paths and "the resulting perturbation ... is solely determined by the state x of the mobile target" [V, Sec. I]. No bistatic ellipse or range equations are used. The triangulation baseline ignores the transmitter and treats each array's residual AoA as a direction to the target, which is a monostatic-style simplification [I]. The transmitters are unsynchronised, which is why the pipeline needs the phase-insensitive CRAP and cosine-similarity steps [I].

## Key numbers (Table I, exact)

Robot test set (S_rob,test), training on the robot set where needed:

| Method | MAE | DRMS | CEP | R95 | KS | CT/TW |
|---|---|---|---|---|---|---|
| Triangulation | 0.434 m | 0.694 m | 0.261 m | 1.368 m | 0.292 | 0.926/0.920 |
| Supervised NN | 0.123 m | 0.149 m | 0.104 m | 0.278 m | 0.069 | 0.996/0.996 |
| PCC (T_opt applied) | 0.257 m | 0.298 m | 0.231 m | 0.556 m | 0.146 | 0.986/0.988 |
| Augmented PCC | 0.258 m | 0.310 m | 0.219 m | 0.585 m | 0.139 | 0.985/0.988 |

Human test set (S_hum,test), still trained on the robot set:

| Method | MAE | DRMS | CEP | R95 | KS | CT/TW |
|---|---|---|---|---|---|---|
| Triangulation | 0.322 m | 0.499 m | 0.227 m | 0.775 m | 0.123 | 0.989/0.988 |
| Supervised NN | 0.487 m | 0.683 m | 0.311 m | 1.423 m | 0.206 | 0.961/0.975 |
| PCC | 0.532 m | 0.716 m | 0.387 m | 1.448 m | 0.263 | 0.935/0.960 |
| Augmented PCC | 0.558 m | 0.746 m | 0.370 m | 1.588 m | 0.249 | 0.951/0.965 |

Column order was reconstructed from PDF text extraction of Table I. The repo README table (a rerun) matches the paper on triangulation MAE, DRMS, CEP and R95, but differs on KS and on every NN/PCC row (for example augmented PCC on the robot: 0.246 m there, 0.258 m in the paper). See repo-passive-channelcharting.md [V].

Read: on a foil-wrapped robot, PCC halves triangulation MAE (0.257 vs 0.434 m). On a human, triangulation wins (0.322 vs 0.532 m) [V, Sec. VII]. The authors' explanation: higher human radar cross section helps the model-based method, while the NNs trained on the robot overfit to the target type [V, Sec. VII].

## Stated limitations

- Single moving target in an otherwise static environment [V, Sec. I-B]. Multi-target is "feasible in principle" if targets separate in angle, range or Doppler [V].
- "A major challenge of all NN-based techniques, including PCC, is generalization" [V, Sec. VII]. Overfitting to target type is unlike active channel charting [V, Sec. VIII].
- Known target height; fixed upright orientation; a foil-wrapped robot as the main target [V, Sec. II].
- Beacons are four fixed ceiling transmitters over a 4.5 x 4.5 m area, with LoS between transmitters and receivers, and four time-, phase- and frequency-synchronised receive arrays [V, Sec. II]. Non-LoS sensing is only speculated ("could enable some degree of (partial) non-LoS sensing"), not shown [V, Sec. VIII].
- The authors call the work "a confirmation of the attractiveness of PCC, with many open questions" [V, Sec. VIII].
- Clutter order K, beta and lambda are not reported in the paper. The repo gives K = 2, beta = 1% quantile of the dissimilarities, lambda = 0.05 [V, repo notebooks 1 and 6].

## Licence and code

Paper: arXiv, licence not checked. Code: Jeija/ESPARGOS-Passive-ChannelCharting, "partial source code", no licence file [V, GitHub API]. Dataset espargos-0007: CC BY 4.0 [V, DaRUS API].
