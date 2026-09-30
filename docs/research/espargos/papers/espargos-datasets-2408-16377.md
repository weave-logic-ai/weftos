# ESPARGOS datasets and a first channel chart (arXiv 2408.16377)

Euchner, ten Brink. "ESPARGOS: Phase-Coherent WiFi CSI Datasets for Wireless Sensing Research." arXiv:2408.16377v1, 29 Aug 2024, cs.IT. https://arxiv.org/abs/2408.16377 . Four pages. The later AR paper cites it as presented at the 2024 Kleinheubach Conference [V, arXiv 2608.25996 ref. [2]].

Tags: [V] read in the source with location; [I] inference.

## Problem

WiFi-sensing research mostly uses CSI from consumer devices that lack spatial diversity or phase synchronisation, and high-quality CSI datasets for massive-MIMO-style research are scarce [V, Abstract, Sec. I]. The authors' earlier sounder DICHASUS produces highly calibrated data but is SDR-based, not real-time and not standards-compliant [V, Sec. I]. ESPARGOS datasets aim to combine spatial diversity with standard WiFi and real-time capability, so results transfer to practice [V, Sec. I].

## Method

### System summary [V, Sec. II]

- One ESPARGOS array is eight antennas, each on an ESP32-S2, in two rows of four. CSI from all antennas streams over a bus to a central controller that aggregates and forwards over Ethernet.
- One shared crystal oscillator plus a distributed phase reference compensate PLL phase ambiguity (details: espargos-hardware-2502-09405.md).
- Normally a passive sniffer. For published datasets a dedicated transmitter continuously sends "very short WiFi packets" so that fresh CSI arrives at regular intervals.
- Several boards combine into one phase-synchronous array. Clock (40 MHz) and reference (about 2.4-2.5 GHz) share a coax, pass through a PA and a splitter cascade (Fig. 2, 4x8 combined array).
- The Python library pyespargos handles configuration and CSI streaming, with demos including AoA estimation.

### Datapoint definition [V, Sec. III]

- CSI comes from the HT-LTF of each WiFi preamble. A packet captured by all receivers is one datapoint.
- Array shape H in C^(B x 2 x 4 x N_sub), B boards. Fig. 3 uses B = 4 and N_sub = 117 (40 MHz HT).
- Per-receiver RSSI P in R^(B x 2 x 4) is stored, so CSI can be weighted for variable receiver gain.
- Fig. 3 shows H-bar obtained by "interpolating over 40 channel estimates measured within an interval of 310 ms" from espargos-0002.
- Metadata: timestamp t in seconds and transmitter position x in R^3. Dataset S = {(H, P, x, t)} for l = 1..L.
- Ground truth: a Leica MS60 tachymeter total station, "millimeter-level accuracy and high update rates", with timestamps aligned between CSI and reference system. The prism sits at the tip of the transmit antenna on a pole carried by a robot.
- Also published: exact array positions and orientations, photos, and for many datasets a 3-D pointcloud of the environment. One right-handed Cartesian frame in metres, arbitrary origin. Each dataset gets a DOI.

### Channel charting example [V, Sec. IV]

- Motivation for a model-free method on espargos-0002: with one array, triangulation does not apply; ToA multilateration is impossible without time sync between transmitter and receiver; RSSI ranging is unreasonable under multipath. Channel charting "makes no such assumption about the propagation environment" and relies on similarity between CSI samples.
- The forward charting function (FCF) C_theta maps CSI in C^(B x 2 x 4 x N_sub) to y in R^2. Cited approach: Triplet-based channel charting (Ferrand et al., GLOBECOM 2020), tuned as in the authors' SPAWC 2022 paper, with "tweaked hyperparameters". The paper gives no loss formula, margin, network layout or hyperparameters. They are in the repo notebook (see repo-wifi-channelcharting.md). [V by absence in paper]
- Chart-to-world alignment for evaluation only: least-squares affine map (A, b) = argmin over sum_l ||A y_l + b - x_l||^2.

## Hardware and datasets

Espargos-0002 details: four boards combined into one 4x8 array, a transmitter on a robot, a metal wall that blocks LoS in part of the area, indoor lab room [V, Sec. III, Fig. 4]. The training subset is L = 569190 datapoints, "though good results are also achievable with significantly fewer" [V, Sec. IV].

Dataset list as shown on https://espargos.net/research/ (fetched 2026-09-30). Sizes and descriptions are from that page [V]. I also queried the DaRUS API for each DOI's licence. All four report CC BY 4.0 [V, DaRUS API, 2026-09-30].

| Name | Size | Description | DOI |
|---|---|---|---|
| espargos-0001 | 23.9 GB | four arrays pointed at a small area in a lab room, LoS only | 10.18419/darus-4352 |
| espargos-0002 | 86.5 GB | four arrays combined into one 8x4 array, metal wall, LoS and NLoS | 10.18419/darus-4456 |
| espargos-0005 | 61.3 GB | four arrays in the corners of a lab room, synchronised in time and phase by wired clock and phase distribution | 10.18419/DARUS-4754 |
| espargos-0007 | 17.4 GB | passive target, four synchronised arrays, four ceiling transmitters | 10.18419/DARUS-4973 |

## Key numbers

Table I (Sec. IV), channel chart on espargos-0002 after the optimal affine transform:

| CT | TW | KS | MAE | CEP |
|---|---|---|---|---|
| 0.96 | 0.96 | 0.20 | 0.44 m | 0.42 m |

(CT continuity, TW trustworthiness, KS Kruskal stress, MAE mean absolute error, CEP circular error probable, as defined in Euchner et al., Asilomar 2023.) The authors call this "acceptable, but leaves room for improvement" [V, Sec. IV]. MAE and CEP were evaluated after the affine transform [V, Table I caption]. The repo README reports the same CT/TW and MAE 0.44 m for this notebook [V, repo].

## Stated limitations

- ESPARGOS CSI "is considerably more noisy and may exhibit additional impairments" than DICHASUS, "as we lack control over some aspects of the WiFi chip". Cheaper, backward-compatible and real-time "partially makes up for the lower data quality" [V, Sec. V].
- The successful FCF training "proves that the CSI quality is at least sufficient for this type of application" [V, Sec. V]. That is one application on one dataset [I].
- Chart alignment to metres needs labelled positions (affine fit) [V, Sec. IV]. The augmented variant in the Siamese notebook avoids this by adding triangulation to the loss (see repo-wifi-channelcharting.md).
- Datasets use a dedicated cooperative transmitter, not arbitrary traffic [V, Sec. II], so they do not show performance on uncontrolled traffic [I].

## Licence and code

Datasets: CC BY 4.0 per DaRUS metadata [V]. Code: Jeija/ESPARGOS-WiFi-ChannelCharting has no licence file (GitHub API reports none). pyespargos is LGPL-3.0. See the repo notes.
