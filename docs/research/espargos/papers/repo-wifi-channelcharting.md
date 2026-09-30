# Repo: Jeija/ESPARGOS-WiFi-ChannelCharting

https://github.com/Jeija/ESPARGOS-WiFi-ChannelCharting . Companion code for arXiv 2408.16377 (see espargos-datasets-2408-16377.md). Created 2024-08-23, last push 2024-08-31, Jupyter Notebook, 23 stars [V, GitHub API, 2026-09-30].

Tags: [V] read in the repo (README or notebook), [I] inference.

## Licence and availability

The GitHub API reports no licence (`license: null`) and the tree has no licence file: `.gitignore`, `README.md`, `SiameseNeuralNetwork.ipynb`, `TripletNeuralNetwork.ipynb`, `img/` [V]. Under default copyright this is "all rights reserved", so reading and reproducing the method is fine, but copying the notebooks into WeftOS or a cog is not clear without asking the author [I]. Datasets are CC BY 4.0 (see the datasets note).

## What is in it

Two notebooks, both TensorFlow/Keras, both reading `.tfrecords` from DaRUS by `wget` [V].

### SiameseNeuralNetwork.ipynb (espargos-0001, four separate 4x2 arrays, dominant LoS)

README result: CT 0.99, TW 0.99, KS 0.10, MAE 0.13 m, CEP 0.12 m [V, README]. "Augmented" channel charting: Siamese channel charting with a fused CSI/timestamp dissimilarity, plus classical triangulation in the loss. The notebook's own conclusion says the scenario is easy because LoS is almost always present, triangulation alone works, and augmented charting "performs similar to simple triangulation, with fewer complete outliers", acting "mostly as some kind of 'filter'" on noisy triangulation [V, notebook Conclusion]. That is an honest caveat: the 0.13 m figure is on a LoS-only dataset [I].

Recipe as coded [V, notebook cells]:
- Input: CSI with shape (arrays 4, rows 2, cols 4, subcarriers 117). Two random-walk files used for training.
- Residual sampling-time-offset removal, `shift_to_firstpeak`: try time shifts from -max_delay_taps (3 taps) to 0 in 40 steps, apply as a linear phase ramp across subcarriers, pick per antenna the earliest shift whose delay-domain power exceeds 0.4 of its maximum. This time-aligns the first arrival.
- Iterative global-phase alignment `csi_interp_iterative` (10 iterations): average a backlog of BACKLOGSIZE packets after estimating and removing a per-packet common phase by alternating w = mean(exp(-j phi) csi) and phi = angle(w^H csi). This is the per-packet transmitter phase problem solved by alternating projection, the same issue the hardware paper solves with a covariance eigenvector.
- Weight CSI by RSSI.
- AoA: unitary root-MUSIC per array (single source), plus a delay-spread estimate from the power delay profile with bandwidth 40 MHz. Delay spread sets the confidence.
- Triangulation: von Mises likelihood per array, maximised with scipy. kappa = (4e-8 / (delay_spread + 0.5e-8))^4 for azimuth, and kappa/10 for elevation. Bessel I0 uses a closed-form approximation.
- Dissimilarity: ADP-based (angle-delay profile) plus timestamp-based, fused (TIME_THRESHOLD = 2 s), then geodesic through a k-nearest-neighbour graph with n_neighbors = 20. Scaled to metres using the classical location estimates.
- FCF: BatchNorm and Dense 1024-512-256-128-64-2, linear output. Loss = CLASSICAL_WEIGHT x classical + (1 - CLASSICAL_WEIGHT) x Siamese, with CLASSICAL_WEIGHT = 0.85. Siamese term = mean of (||f(a) - f(b)||_pred - d_ab)^2 / (d_ab + margin), margin default 1.
- Adam, learning rate 1e-3 decaying to 1e-5 (100000 steps), batch size 3000.

### TripletNeuralNetwork.ipynb (espargos-0002, one 8x4 combined array, metal wall, NLoS)

README result: CT/TW 0.96, MAE 0.44 m after affine transform [V, README; matches paper Table I]. Chart coordinates are arbitrary under affine transform, no augmentation [V, README].

Recipe as coded [V, notebook cells]:
- Uses three of the espargos-0002 files (randomwalk-3, radial-meanders-1, spiral-1). Shape (4, 2, 4, 117).
- Same first-peak time alignment (max_delay_taps 4, 100 search steps, peak threshold 0.3), backlog 5, RSSI weighting. The notebook says the dataset "offers phase synchronization, but no time synchronization (sampling time offset)", so this step compensates residual sampling-time offset.
- Features: keep delay taps 56-69 (TAP_START 56, TAP_STOP 69), compute sample cross-correlations between every pair of antennas per tap, then feed real/imag. So the network sees the antenna covariance per delay tap, not raw CSI.
- Network: BatchNorm, Dense 512-256-128-64-2 with BatchNorm between layers, 2-D output.
- Triplet loss on squared mean distances: max(d(anchor, positive) - d(anchor, negative) + 1, 0), margin 1.
- Triplet mining by time: a positive is a sample within T_c seconds of the anchor. T_c shrinks over training, 5.2 s to 3.3 s across 5 sessions, 150,000 triplets per session, 10 epochs each, batch size 12000, learning rate 0.0066 to 5e-5. Negative selection was not read for this note [I: presumably samples outside the T_c window].
- Metrics computed on a random 10% subset: continuity and trustworthiness with n_neighbors = 5% of the subset, Kruskal stress with the optimal scale, MAE after optimal affine transform.

The triplet approach uses time adjacency as its only supervision: samples close in time are assumed close in space. That needs a continuous moving transmitter. It would fail for sparse or intermittent observations [I].

## Requirements

Python, TensorFlow, NumPy, SciPy, Matplotlib. Authors' machine: NVMe, EPYC, 64 GB RAM recommended, NVIDIA GPU; less powerful systems "may work" [V, README]. Not feasible on a Pi-class node for training [I].
