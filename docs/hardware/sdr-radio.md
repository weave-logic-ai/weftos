# SDR & radio hardware — WeftOS/WeaveLogic hardware KB

Software-defined radio and radio-sensing hardware we have evaluated for WeftOS /
cogs radio-analysis work, plus the common comparison baselines. Specs are public
facts with a source per item; figures are nominal manufacturer/datasheet values
and should be re-verified against the specific board revision before design work.

Last reviewed: 2026-10-02.

---

## Nuand bladeRF 2.0 micro xA4
- **Role for us**: radio analysis + FPGA cog host — the one device here where we
  considered running cogs on the on-board FPGA fabric alongside the RF datapath.
- **Freq range / bandwidth / ADC**: 47 MHz – 6 GHz tuning; up to 61.44 MHz sample
  rate (hardware capable of 122.88 MHz); up to ~56 MHz usable channel bandwidth,
  2×2 MIMO. 12-bit ADC/DAC in the AD9361 RF front-end.
- **On-board compute**: Intel/Altera Cyclone V FPGA — xA4 variant is the 49 kLE
  part, of which ~32 kLE are free and user-programmable. This free fabric is what
  we eyed for FPGA-resident cogs running beside the radio pipeline. RF transceiver
  is the Analog Devices AD9361 (2×2 wideband). Cypress FX3 handles USB.
- **Host interface**: USB 3.0 SuperSpeed (5 Gbps, full-duplex; FX3 can saturate
  the link for 2×2 streaming).
- **Notes/fit**: Best-fit SDR of the lot for serious analysis — true duplex TX/RX,
  wide instantaneous bandwidth, and open FPGA space for offloading matched filters
  / DSP / cog logic into fabric instead of the host. Higher cost than the Pluto-
  class clones. Cyclone V toolchain (Quartus) is the gate for FPGA cog work.
- **Source**: https://www.nuand.com/product/bladerf-xa4/

## Low-cost 70 MHz–6 GHz Zynq-7010 SDR (PlutoSDR-class clone)
- **Role for us**: radio analysis — low-cost evaluation board; also an FPGA+SoC
  path (Zynq PS+PL) worth noting for on-device processing experiments.
- **Freq range / bandwidth / ADC**: 70 MHz – 6 GHz tuning (AD9363 front-end; many
  boards unlock AD9364/AD9361-class behavior); up to 61.44 MHz sample rate; 12-bit
  ADC/DAC; typically sold as 2×TX / 2×RX.
- **On-board compute**: Xilinx Zynq-7010 SoC — dual-core ARM Cortex-A9 PS plus
  Artix-7-class programmable logic (PL). RF is the Analog Devices AD9363 (the
  "Pluto" transceiver); boards commonly ship 512 MB RAM + 32 MB flash and a
  40 MHz 0.5 ppm VCTCXO reference. (Note: AD9363 is the lower-tier sibling of the
  AD9361 in the bladeRF above.)
- **Host interface**: Gigabit Ethernet + USB 2.0 OTG.
- **Notes/fit**: Cheapest way onto the AD936x + Zynq software/IIO stack
  (libiio / GNU Radio / MATLAB), ADALM-PLUTO-compatible, and a target for openwifi.
  USB 2.0 caps sustained streaming well below the bladeRF; the Zynq-7010 is the
  smaller PL part (vs 7020 on some variants), so less room for heavy fabric DSP.
  Good analysis/learning bench; not the device for wide-band capture at full rate.
- **Source**: https://www.amazon.com/Transceiver-70MHz-6GHz-Compatible-Open-Source-Development/dp/B0FSWWL3M5

## ESPARGOS / ESP-SDR (ESP32 WiFi phased array)
- **Role for us**: phased-array / channel-sounding research — pulled their
  research articles into our research pool; ESP-SDR added to RuView/WeftOS/cogs
  docs as the low-cost phase-coherent WiFi-sensing reference.
- **Freq range / bandwidth / ADC**: 2.4 GHz WiFi band only (not a general SDR). It
  does not expose IQ or a wideband ADC — it extracts per-packet WiFi Channel State
  Information (CSI) from standard 802.11 frames. Supports L-LTF / HT20 / HT40
  preamble formats; phase reference distributed ~2.4–2.5 GHz with a 40 MHz clock.
- **On-board compute**: 8× Espressif ESP32-S2FH4 MCUs (one per antenna) feeding a
  controller-board ESP32 that provides the shared clock + phase reference and
  streams CSI out. No FPGA — the "array processing" (beamforming, AoA) happens off-
  board in software (pyespargos).
- **Host interface**: Ethernet from the controller board (CSI stream); boards
  chain over a single coax carrying frequency-multiplexed clock + phase reference.
- **Notes/fit**: A phase-coherent 2×4 (8-element) WiFi antenna array for real-time
  sensing — angle-of-arrival, localization, passive sensing — at a tiny fraction of
  SDR-array cost. The fit for us is phased-array/channel-sounding research and
  distributed RF-sensing cogs, NOT wideband spectrum analysis. Phase coherence
  across cheap ESP32s is the interesting trick; calibration is the hard part.
- **Source**: https://espargos.net/ (research: https://arxiv.org/pdf/2502.09405, https://arxiv.org/pdf/2408.16377; code: https://github.com/ESPARGOS/pyespargos)

---

## Reference baselines (not ours — comparison points)

One-line reference points we measure the above against. Not evaluated for adoption.

- **RTL-SDR (RTL2832U dongle)**: RX-only, ~24 MHz–1.766 GHz (E4000/R820T2
  dependent), ~2.4 MHz usable bandwidth, 8-bit ADC, USB 2.0. The ~$30 floor and
  the reason "SDR" is cheap. Source: https://www.rtl-sdr.com/about-rtl-sdr/
- **HackRF One**: half-duplex TX/RX, 1 MHz–6 GHz, up to 20 MHz bandwidth, 8-bit
  ADC, USB 2.0. Wide tuning, modest dynamic range; popular for survey/TX work.
  Source: https://greatscottgadgets.com/hackrf/one/
- **LimeSDR (USB)**: full-duplex 2×2 MIMO, 100 kHz–3.8 GHz, up to 61.44 MHz sample
  rate, 12-bit ADC (LMS7002M), USB 3.0, with an Altera Cyclone IV FPGA. Closest
  open competitor to the bladeRF class. Source: https://limemicro.com/products/boards/limesdr/
- **USRP B210 (Ettus/NI)**: full-duplex 2×2 MIMO, 70 MHz–6 GHz, up to 56 MHz
  bandwidth / 61.44 MHz sample rate, 12-bit ADC (AD9361), USB 3.0, Spartan-6 FPGA.
  The research/industry reference; UHD/GNU Radio first-class. Source: https://www.ettus.com/all-products/ub210-kit/
