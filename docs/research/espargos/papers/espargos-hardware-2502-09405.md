# ESPARGOS hardware architecture (arXiv 2502.09405)

Euchner, Schneider, Gauger, ten Brink. "ESPARGOS: An Ultra Low-Cost, Realtime-Capable Multi-Antenna WiFi Channel Sounder." arXiv:2502.09405v1, 13 Feb 2025, eess.SP. https://arxiv.org/abs/2502.09405 . Six pages. The arXiv text is the paper as read for this note. Its own reference list places the paper at the 26th ITG Workshop on Smart Antennas (VDE, 2023) [V, ref. [8] of arXiv 2408.16377 and ref. [1] of 2608.25996].

Tags: [V] read in the source, with location; [I] my inference. Everything below was read from the PDF text, not from the abstract page.

## Problem

Multi-antenna channel sounders are usually built from many SDR receivers, so they are expensive and hard to operate [V, Abstract, Sec. I]. Commodity WiFi CSI tools give few antennas: "usually only three antennas or even fewer" [V, Sec. I]. Simply using several WiFi chips is not phase-coherent, so AoA estimation fails [V, Sec. I]. Antenna switching (SWAN) needs multiple frame transmissions per sensing operation [V, Sec. I]. The goal is a single-board, many-antenna, phase-coherent, real-time WiFi sounder that hobbyists can afford [V, Abstract, Sec. I].

## Method

### Array geometry and synchronisation hardware

- One four-layer FR4 board carries several ESP32 microcontrollers, "operated in receive-only mode", with eight ceramic antennas, one per ESP32. It is inspired by KrakenSDR [V, Sec. II]. A 2x4 layout (2 rows of 4) is confirmed in the sibling datasets paper, which names the chip as the ESP32-S2 [V, arXiv 2408.16377 Sec. II].
- An SPI header exposes the board to a computer [V, Sec. II]. (The datasets paper describes a later arrangement: a central controller streams CSI over a bus interface and forwards it over Ethernet [V, arXiv 2408.16377 Sec. II].)
- A daisy-chained 40 MHz reference oscillator clocks all ESP32s. This gives frequency synchronisation [V, Sec. II, Fig. 2].

### Why a shared clock is not enough

- The chip internals are not public. The authors assume the LO is generated from the reference by a PLL that "may exhibit phase uncertainty" [V, Sec. II-A].
- Suspected sources: phase-detector ambiguity, unknown initial digital counter states in the forward 1/r divider, VCO centre-frequency inaccuracy [V, Sec. II-A].
- Measured: the initial phase changes after a chip reset and after changing the LO frequency (WiFi channel), "likely every time the PLL has to acquire a new frequency lock" [V, Sec. II-A].
- Analog components can add receiver-specific phase shifts through process variation or temperature. So "an additional, periodically performed phase calibration step is vital" [V, Sec. II-A].

### Phase reference signal distribution

- Each ESP32 sits behind an RF switch that alternates between its antenna and a WiFi-based phase reference signal [V, Sec. II-B, Fig. 3].
- The reference reaches all receivers through a resistive power splitter and microstrip lines. The reference generator "must simply produce valid WiFi frames" [V, Sec. II-B].
- Expected inter-receiver phase differences for the reference are known from the distribution-network geometry, so per-receiver offsets can be calibrated [V, Sec. II-B].
- To join several boards, the clock and the phase reference come from external sources. To save cabling both share one coax and are separated on the board by high-pass and low-pass filters [V, Sec. II-B]. The datasets paper adds that the 40 MHz clock and a reference at "around 2.4 - 2.5 GHz" are frequency-multiplexed onto one coax, amplified by a PA, and split by a cascade of power splitters. With matched cable lengths and low splitter phase unbalance the boards are phase-coherent; otherwise a constant phase offset remains, "accounted for in software" [V, arXiv 2408.16377 Sec. II].

### CSI acquisition and estimation

- The ESP32 driver gives CSI from the L-LTF and HT-LTF. In promiscuous mode this is fully passive [V, Sec. III].
- Frames are received independently per chip, so they are clustered by MAC address, receiver timestamp (usable because sampling clocks are synchronised) and packet headers [V, Sec. III-A].
- Model: H[t] in C^(M x N), M antennas, N usable subcarriers. Measured r = h e^(j phi) + n, where phi is an unknown per-packet transmitter phase (oscillator offset and drift) and n is i.i.d. noise with covariance sigma^2 I [V, Sec. III-A].
- Estimation that tolerates the transmitter phase: compute the covariance C = E[r r^H] = h h^H + sigma^2 I, which is invariant to the common e^(j phi). The sample covariance uses only packets seen by both antennas i and j: C_ij = (1/|T_ij|) sum over t in T_ij of r_i[t] r_j[t]*. This handles antennas that miss frames [V, Sec. III-A, Eq. 1-2 and following]. The paper says the reasons for lost frames "are unknown and hard to analyze" [V].
- The channel estimate is the least-squares rank-1 fit in Frobenius norm. It is the principal eigenvector of C-hat, scaled so that ||h||^2 = lambda_max - sigma^2 [V, Sec. III-A, Eq. 3-4]. It is applied per subcarrier independently [V].

### Phase and power calibration

- h_CAL,m = (h_OTA,m / h_REF,m) e^(-j phi_path,m), where h_OTA is the over-the-air estimate, h_REF is the estimate from the reference signal, and phi_path,m is the constant per-antenna delay of the reference distribution path [V, Sec. III-B].
- OTA and reference frames are distinguished by the RF switch state or, "more reliably", by a reference indicator embedded in the reference frames [V, Sec. III-B].
- Assuming negligible attenuation difference across reference paths, reference amplitudes serve as power calibration [V, Sec. III-B].
- For a combined multi-board array, phi_path must also include the external splitters and coax [V, Sec. III-B].
- The paper does not describe CSI sanitisation beyond this (no STO/SFO removal, no packet-detection-delay handling). [V by absence]. The AR paper adds a delay-alignment step, see ar-visualization-2608-25996.md.

## Hardware and datasets

ESP32 chips (S2 per the datasets paper), eight ceramic antennas per board, 40 MHz reference, one 20 MHz WiFi channel in the experiments [V, Sec. IV-A]. The experiments use one board and a mobile ESP32 dev board as transmitter [V, Fig. 5]. No dataset is published in this paper.

## Key numbers

The paper is qualitative. It states no accuracy, no error in degrees or metres, no sample counts and no bandwidth beyond "a single 20 MHz wide WiFi channel" [V, Sec. IV-A]. It states "Quantitative performance testing and further characterizations of the system remain a subject for future study" [V, Sec. V].

- Phase-stability test (Fig. 6): phase differences relative to antenna 0, moving-average over 20 samples, about 400 s, transmitter relocated at t about 150 s and back at t about 300 s. The result is qualitative: unless the transmitter is moved, "the variance in the phase difference between any two of the eight antennas ... is small" [V, Sec. IV-A]. This test deliberately skips the phase reference calibration and relies on the shared clock alone [V]. Read carefully, it shows stability over time in a static setup. It does not show that phases are correct after a reset or channel change, which the authors say the PLL breaks [I].
- Frequency-flat assumption: the per-antenna phase is taken from the sum over all N subcarriers, "only sensible for sufficiently frequency-flat channels such as indoor channels with low delay spread" [V, Sec. IV-A].
- AoA test (Figs. 7-9): three transmitter placements (left, frontal, right, a few metres away). Phase patterns across antennas differ visibly. MUSIC pseudo-spectra "easily permit distinguishing" them [V, Sec. IV-B]. No angular error is given.

## Stated limitations

- Prototype only, evaluated qualitatively [V, Sec. V].
- "ESPARGOS could never replace dedicated multi-antenna channel sounders employed in wireless research" [V, Sec. V].
- Chip internals unknown, so the PLL explanation is a hypothesis backed by measurement [V, Sec. II-A].
- Lost frames per antenna are unexplained [V, Sec. III-A].
- Later papers add: CSI is "considerably more noisy" than SDR sounders [V, arXiv 2408.16377 Sec. V]; 8-bit signed CSI limits dynamic range and sync quality depends on reference amplitude and modulation (Euchner, Q&A, SDR Academy 2025, see video-sdr-academy-2025.md).

## Licence and code

The paper states no licence and links no code. The client library pyespargos is LGPL-3.0 (see repo-pyespargos.md). Hardware design licence: not reviewed, espargos.net did not state one in the fetched content. Firmware licence: not reviewed.
