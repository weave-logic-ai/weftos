# Repo: ESPARGOS/pyespargos (and the two new ESPARGOS SDR repos)

https://github.com/ESPARGOS/pyespargos . The client library and demo suite for ESPARGOS. Created 2024-02-22, last push 2026-09-28, Python, 1014 stars, licence LGPL-3.0 [V, GitHub API, 2026-09-30].

Tags: [V] read in the README or GitHub API, [I] inference.

## What it is

- "Real-time-capable, phase-synchronous 2 x 4 WiFi antenna array built from Espressif ESP32 chips", with support for combining several boards into a larger array [V, README].
- CSI preambles: L-LTF, HT20, HT40, and HE20 per the docs summary [V, README and docs index]. Flexible calibration for multi-board setups [V, README].
- Two hardware generations. The `main` branch is for the current board; the older prototype needs `legacy-prototype` and is "no longer supported" [V, README].
- Passive operation, except a radar mode that transmits [V, README summary]. The AR paper lists phase-coherent transmit / radar sensing as future work [V, arXiv 2608.25996 Sec. IV], so treat radar mode as new.
- Architecture: `Board` objects talk to controllers over HTTP/UDP or USB serial; `CSIPool` clusters packets across boards; `CSIBacklog` gives windowed access for noise reduction by packet averaging [V, docs summary].
- Requirements: Python 3.11 or newer; Linux, Raspberry Pi, Windows, macOS; PyQt6, matplotlib and PyYAML for demos [V, README].
- Demos (`demos/`): iq-signal-analyzer, music-spectrum, phases-over-space, instantaneous-csi, phases-over-time, tdoas-over-time, azimuth-delay, polarization, speedtest, combined-array, combined-array-calibration, camera, radiation-pattern-3d [V, README table]. `camera` and `azimuth-delay` use precompiled Qt shaders (`.qsb`) [V].

## Licence

LGPL-3.0. A library under LGPL can be linked from a differently licensed program if the LGPL terms (relinking, source for the library itself) are met [I, general licence knowledge, not legal advice]. Reimplementing the protocol in Rust from the paper avoids the question but pyespargos is the only place the controller wire protocol is documented in code [I]. The wire protocol was not read for this note.

## Hardware availability

espargos.net says "ESPARGOS One is now available", with a buy link at buy.espargos.net. The ESPARGOS One is a 4x2 dual-polarised array [V, espargos.net home, fetched 2026-09-30]. Price was not on the fetched page. In the SDR Academy talk the author is "currently working on trying to get a small-scale manufacturing run of ESPARGOS" and cannot promise a date (Aug 2025) [V, video transcript]. Hardware and firmware licences: not reviewed, none stated in the fetched content.

## The 2026-09-28 SDR repos

- ESPARGOS/esp-sdr: "uses the undocumented raw I/Q capture functionality of ESP32 family chips to turn them into (low-duty-cycle) Software Defined Radios". C, created 2026-09-28, last push 2026-09-29, 57 stars, licence: none (API reports null) [V, GitHub API].
- ESPARGOS/esp-web-sdr: web UI showing the spectrum and waterfall for esp-sdr. JavaScript, created 2026-09-28, 4 stars, licence: none [V, GitHub API].
- Other org repos: `espargos.github.io` (website and docs), `t-dongle-c5-transmitter` (ESPARGOS test transmitter firmware for the LILYGO T-Dongle C5, created 2026-04-21). Neither has a licence reported [V, GitHub API].

The esp-sdr analysis is in [espsdr-and-linux-csi-nodes.md](../espsdr-and-linux-csi-nodes.md).
