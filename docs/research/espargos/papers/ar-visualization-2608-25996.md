# AR visualisation of WiFi channel measurements (arXiv 2608.25996)

Euchner, ten Brink. "Visualizing Wireless Propagation and Polarization in Augmented Reality with ESPARGOS." arXiv:2608.25996v1, 26 Aug 2026, eess.SP. https://arxiv.org/abs/2608.25996 . Four pages. Funded under the BMFTR SENSATION project [V, footnote].

Tags: [V] read in the source with location; [I] inference.

## Problem

RF fields are invisible, so coherent multi-antenna CSI is normally inspected through abstract plots or fed to algorithms. That makes it hard for non-specialists, and slow for researchers who want to debug an experiment [V, Sec. I]. Prior AR/VR radio work visualises predicted radio maps, signal strength or planning metrics. This system overlays the measured RF signal's angular structure, delay and polarisation on a camera view [V, Sec. I].

## Method

### Receiver

- "ESPARGOS One": a 2x4 patch antenna array with two switchable feeds per element for +/-45 degree slant polarisation, 2.4 GHz WiFi, shared clock and phase-reference distribution. Larger apertures combine several boards into one rectangular array (Fig. 1-2) [V, Sec. II-A].
- The raw CSI is corrected with the chip-reported receiver gains, phase-calibrated across receivers, and arranged by a configured antenna map into H[m,n,k], with m rows, n columns, k subcarriers, spacing d about lambda/2 [V, Sec. II-A].
- Each receiver sees one feed at a time, so receivers "switch randomly and independently" between feeds R and L. Measurements are tagged with switch state, kept in a short backlog and combined under a quasi-static scene assumption [V, Sec. II-A].
- CSI sanitisation and coherent combination (this is the paper's answer to per-packet phase): channel estimates are time-aligned so that the first significant impulse-response peak sits on a fixed reference tap. Averaging over the backlog improves SNR and recovers the relative phase of both feeds. Carrier frequency offset gives every packet an unknown global phase, so "an iterative global phase alignment algorithm is needed" to combine packets coherently. The algorithm is not given in the paper [V, Sec. II-A].

### Angular spectrum

- For half-wavelength spacing, a plane wave from azimuth phi and elevation theta gives phase steps Psi_x = pi cos(theta) sin(phi) and Psi_y = pi sin(theta) between neighbouring elements. The visible region is the disc Psi_x^2 + Psi_y^2 <= pi^2 [V, Sec. II-B, Eq. 1-2].
- Beamspace: b_k(Psi_x, Psi_y) = (1/MN) sum over m, n of h[m,n,k] exp(-j(m Psi_y + n Psi_x)). Implemented as a zero-padded 2-D FFT. Power P = mean over subcarriers of ||b_k||^2 [V, Eq. 3].
- Speed-up: transform to the delay domain first, keep a short window around the aligned first-arrival tap, beamform only that window, then go back to frequency [V, Sec. II-B].
- The paper says the implementation also supports MUSIC and other estimators, but the FFT method is chosen because it is simple and fast [V, Sec. II].

### Camera registration

Beamspace to angles: theta = arcsin(Psi_y/pi), phi = arcsin(Psi_x / (pi cos theta)) (Eq. 4). Then a pinhole model with field-of-view angles gives normalised image coordinates u = 1/2 + (1/2) tan(phi)/tan(Phi_x/2) and v = 1/2 + (1/2) tan(theta)/(cos(phi) tan(Phi_y/2)) (Eq. 5). Boresights are assumed aligned, parallax between camera and array is neglected, and fixed azimuth/elevation offsets fix small misalignment [V, Sec. II-C]. Lens distortion is not modelled in the paper; the talk mentions it as an option [V, SDR Academy talk].

### Delay and polarisation layers

- Relative delay per beam from the adjacent-subcarrier phase increment: tau-hat = arg(sum over k of b_(k+1)^H b_k) / (2 pi Delta f). It assumes one dominant delay per beam. Delay maps to hue, power to brightness [V, Sec. II-D].
- Polarisation: convert R/L feed vectors to a V/H Jones vector using an "empirically determined" Jones matrix per element, h_VH = J^(-1) h_RL. Estimate the per-beam Jones vector eta-hat, fix its gauge with the centre-subcarrier phase, and animate the electric-field phasor Re(eta e^(j omega t)) as dots with trails. Circular polarisation shows as rotating dots [V, Sec. II-D, Eq. 6].
- Rendering: the CPU computes low-resolution beamspace textures. The GPU vertex shader does the beamspace-to-camera mapping and the fragment shader blends with the video and draws the field traces [V, Sec. II-E].

## Hardware and datasets

Four ESPARGOS boards (32 antennas, a combined 4x8 array), a webcam, and a laptop over Ethernet, all rendered live [V, Sec. III, Fig. 3]. No dataset is published with this paper. Code: the open-source implementation is in pyespargos (`demos/camera`, `demos/azimuth-delay`, `demos/polarization`) [V, Sec. II footnote; README of pyespargos].

## Key numbers

There are no quantitative metrics (no angular resolution figures, latency, or frame rate in numbers). The paper says only that the overlay "updates continuously at high frame rate" [V, Sec. III]. Results are screenshots (Fig. 4a-f): a wider array field of view than a webcam; outdoor paths coloured by delay, with direct and ground reflection earliest and a building-corner reflection later; two dipoles with different polarisation and phase; an LHCP transmit signal rotating counterclockwise on the LoS path and clockwise after a metal-wall reflection; a wire rack acting as a polarisation filter [V, Sec. III].

## Stated limitations

- Static screenshots only; no quantitative validation [V, Sec. III].
- Assumes a single dominant delay per beam and approximately constant V/H components over the band per beam [V, Sec. II-D].
- Assumes aligned boresights and negligible parallax [V, Sec. II-C].
- Polarisation relies on an empirical Jones matrix per element [V, Sec. II-A].
- Quasi-static scene assumption for the feed-combining backlog [V, Sec. II-A].
- Future work: a compact handheld device and radar sensing with phase-coherent transmission [V, Sec. IV]. So this paper's system is receive-only.

## Licence and code

pyespargos is LGPL-3.0 [V, GitHub API]. The paper states no licence.
