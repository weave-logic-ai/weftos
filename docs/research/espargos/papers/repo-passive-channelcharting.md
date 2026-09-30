# Repo: Jeija/ESPARGOS-Passive-ChannelCharting

https://github.com/Jeija/ESPARGOS-Passive-ChannelCharting . "Partial source code" for arXiv 2504.09924 (see passive-channel-charting-2504-09924.md). Created 2025-04-13, last push 2025-07-05, Jupyter Notebook plus Python modules, 29 stars [V, GitHub API, 2026-09-30].

Tags: [V] read in the repo, [I] inference.

## Licence and availability

No licence: the API reports `license: null` and the tree has none (`.gitignore`, notebooks 0-6, `CCEvaluation.py`, `CRAP.py`, `FeatureEngineering.py`, `cluster_utils.py`, `espargos_0007.py`, `neural_network_utils.py`, `poster.pdf`, `README.md`, `img/`) [V]. Same position as the sibling repo: read and re-implement, do not copy without asking [I]. The README says the code is "partial" [V].

## Pipeline (README order)

Notebooks are numbered and must run in order [V, README]:

0. `0_DownloadDataset.ipynb`: fetch the needed parts of espargos-0007 (17.4 GB total; individual `.tfrecords` are up to several GB) [V].
1. `1_ClutterChannels.ipynb`: CRAP clutter estimate per transmitter. The code calls `CRAP.acquire_clutter(csi_filtered, order = 2)`, so the clutter order K is 2 [V, notebook]. The paper does not state K.
2. `2_SupervisedBaseline.ipynb`: fingerprinting NN.
3. `3_AoA_Estimation.ipynb`: unitary root-MUSIC per cluster and array.
4. `4_Triangulation.ipynb`: von Mises triangulation.
5. `5_DissimilarityMatrix.ipynb`: fused (angle-delay-profile plus timestamp) dissimilarity, geodesic version via kNN graph with n_neighbors = 20, a subtractive threshold `adp_thresh` (dissimilarity shifted down and clipped at zero), and scaling to metres using the triangulation estimates [V, notebook].
6. `6_ChannelCharting.ipynb`: Siamese FCF training, then an augmented FCF that adds the AoA estimates to the loss.

Training settings in notebook 6 [V]: 10 epochs, learning rate 1e-2 decaying to 1e-5, growing batch-size schedule 64, 128, 256, ..., 4096 across training, Adam. The dissimilarity margin `beta` is the 1% quantile of the training dissimilarity matrix: `np.quantile(dissimilarity_matrix, 0.01)`. The loss is mean of (d_pred - d)^2 / (d + margin). The paper's lambda for augmentation is `classical_weight`, set to 0.05 for the augmented model and 0.0 for the plain one [V, notebook 6]. That is a very small weight on the triangulation term, so the augmented chart is mostly the Siamese loss with a light anchor to global coordinates [I].

## Numbers differ between README and paper

README performance table (robot / human, metres) [V, README]:

| Method | Target | MAE | DRMS | CEP | R95 | KS | CT / TW |
|---|---|---|---|---|---|---|---|
| Classical AoA | Robot | 0.434 | 0.694 | 0.261 | 1.368 | 0.273 | 0.927/0.922 |
| Fingerprinting | Robot | 0.120 | 0.145 | 0.104 | 0.268 | 0.067 | 0.996/0.996 |
| Augmented PCC | Robot | 0.246 | 0.295 | 0.208 | 0.581 | 0.129 | 0.988/0.990 |
| Classical AoA | Human | 0.322 | 0.499 | 0.227 | 0.775 | 0.123 | 0.989/0.988 |
| Fingerprinting | Human | 0.465 | 0.630 | 0.308 | 1.352 | 0.206 | 0.966/0.978 |
| Augmented PCC | Human | 0.550 | 0.724 | 0.383 | 1.565 | 0.236 | 0.959/0.972 |

The paper's Table I gives 0.258 m (augmented PCC, robot) and 0.558 m (augmented PCC, human), 0.123 m and 0.487 m for the supervised NN. The differences (a few mm to about 2 cm) are consistent with a rerun with different seeds [I]. Use the paper as the citable number and the README as an indication of run-to-run spread. The README also shows no plain (non-augmented) PCC row.

## Requirements

Python, TensorFlow, NumPy, SciPy, Matplotlib; authors used an EPYC 8-core machine with an RTX 4080 [V, README]. Each notebook's datasets are too large for a Pi-class node [I].
