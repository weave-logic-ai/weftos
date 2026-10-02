# RF sensing head: radio, timing, synced antennas

Date: 2026-10-02. Status: research, nothing bought. Tags: [V] read in the cited source or measured; [I] inference.

A design for a fixed RF sensing head: a coherent SDR, a shared time base across nodes, phase-synced antenna sets, UWB for position ground truth, and local compute for post-processing. It extends [lora-sdr-sensing.md](lora-sdr-sensing.md) (the cheap RTL-SDR path) and [espargos/espsdr-and-linux-csi-nodes.md](espargos/espsdr-and-linux-csi-nodes.md) (CSI nodes).

## 1. Radio: Nuand bladeRF 2.0 micro xA9

| | |
|---|---|
| Price / stock | **$860 direct from Nuand, in stock, "Made in USA"** ([Nuand](https://www.nuand.com/product/bladerf-xa9/), 2026-10-02) [V] |
| RF | AD9361, 47 MHz-6 GHz, 2×2 MIMO, 61.44 MS/s, USB 3.0 [V] |
| FPGA | Intel Cyclone V, 301k LE: room for decimation, channelizing and LoRa dechirp before USB [V] |
| Clock | VCTCXO that can be tamed to an external **10 MHz** reference; 38.4 MHz clock in/out for chaining boards. libbladeRF exposes `tamer=external` (10 MHz) and `tamer=external_1pps` ([gr-osmosdr](https://gitea.osmocom.org/sdr/gr-osmosdr/commit/2b798113503be73d67348dec5ed9f944de6fe7b6)) [V]. 1PPS taming on the 2.0 micro specifically is unconfirmed [I] |
| Add-ons | micro enclosure $20, tri-band antenna $25, BT-200 LNA $30, BT-100 PA $30 [V] |

Why it beats the $240-296 B210 clone: transmit and receive share one clock (no CFO), two phase-matched RX channels, a known FPGA image and driver (libbladeRF, SoapySDR, GNU Radio). The clone's advantage is only its onboard GPS input. [I]

## 2. Timing

Three layers, each with a different job:

| Layer | Mechanism | Accuracy | Use |
|---|---|---|---|
| Node ↔ node | **PTP (IEEE 1588) over wired Ethernet** | ~±100 ns | shared timestamps for every sensor event |
| Node ↔ absolute time | GPS **PPS on a GPIO** (`pps-gpio` + chrony) on one node, which is the **PTP grandmaster** | ~µs to UTC | logs and chain events line up with the world |
| Radio frequency/phase | **GPSDO 10 MHz (or 1PPS) into the bladeRF** reference input | ppb-class frequency | coherent RF across heads (multistatic) |

### BCM2712 (Pi 5 / CM5) is PTP-capable out of the box

- **Pi 5:** RP1's Ethernet MAC (Cadence GEM, IEEE 1588-capable) hardware-timestamps packets and exposes a PHC at `/dev/ptp0`. Canonical's guide syncs two Pi 5s with `ptp4l` + `phc2sys` to **offsets around 0 ± 100 ns** within seconds. One caveat: it needed `--neighborPropDelayThresh 17000` because the measured path delay between the boards was ~16.9 µs ([Canonical](https://canonical-industrial-documentation.readthedocs-hosted.com/en/latest/iot-communication-protocols/how-to/rpi5-ptp-time-sync)) [V]
- **CM5:** the onboard Gigabit PHY "supports IEEE 1588" ([Mouser CM5 page](https://www.mouser.com/new/raspberry-pi/raspberry-pi-compute-modules-5/)) [V]
- **Hardware sync pins:** the CM4 exposed PHY SYNC_IN/SYNC_OUT on its IO board, and only one was wired correctly. Whether the CM5 breaks out an equivalent pin, for hard PPS into the PHC, is **unconfirmed**. It would be the tier above software PPS. [V for CM4; I for CM5]
- **Limit:** the BCM2712 takes no external reference clock. Nodes are *disciplined* to a common time, not *clocked* from one oscillator. That suits timestamps; RF phase coherence comes from the radio's own reference (row 3). [I]
- **Wi-Fi does not hardware-timestamp**, so PTP nodes need wired Ethernet. Extra NICs on CM5 carriers must be checked chip by chip for PTP. [I]

**What ±100 ns buys:** about 0.15 mm of acoustic path in water (sonar fusion is far beyond adequate), and plenty for event fusion across CSI, mmWave, UWB and LoRa. It is **not** RF phase coherence: 100 ns is about 30 m of radio path. [I, arithmetic]

**Measured on our hardware: not yet.** On 2026-10-02 the Pi 5 (`cog0`, now running the Cognitum Seed image) had **no `eth0` at all**: interfaces were `usb0`/`usb1` (gadget), `wlan0`, `wlan0_ap` and `tailscale0`, and there was no `/dev/ptp*`. `ethtool` and `linuxptp` are now installed there. Testing PTP needs either a plain Raspberry Pi OS card or the Seed image's Ethernet enabled, plus a cable. [V]

## 3. Synced antennas

"Synced" here means several antennas sampled by **one** receiver with **one** local oscillator. The phase differences between them are then stable, which enables angle-of-arrival and beamforming. Separate boxes are only time-synced (§2), not phase-synced.

| Option | Antennas, phase-coherent | Bandwidth | Status / access | Notes |
|---|---|---|---|---|
| **ASUS RT-AC86U** (Broadcom **BCM4366c0**) + `nexmon_csi` | **4 RX cores** (3 external + 1 internal), CSI per core and spatial stream | 80 MHz | open (Nexmon), firmware 10_10_122_20 ([nexmon_csi](https://github.com/seemoo-lab/nexmon_csi)); used units about €57-195 on eBay [V] | Best-supported **router** with the right Broadcom. Per-core phase is available, but chain-offset calibration for AoA is not well established in the community [V, forum]. Used by WiROS [V] |
| ASUS RT-AX86U-class (Broadcom **BCM43684**) + **AX-CSI** | 4×4 | **160 MHz**, 802.11ax HE | tool by request only (axcsi@unibs.it) ([UniBS](https://ans.unibs.it/projects/ax-csi/)) [V] | Wider and newer; access and licence terms unknown until requested [V] |
| Raspberry Pi 5's own **bcm43455c0** + Nexmon | 1 | 80 MHz | open [V] | Single antenna, so no AoA. Useful as a cheap extra viewpoint |
| **Intel AX210** in a CM5 M.2/PCIe slot + **FeitCSI** | 2 | 160 MHz, 6 GHz | GPL, x86-oriented; RuView already ingests FeitCSI (RuView ADR-292, Accepted) [V] | Puts coherent 2-chain wideband CSI next to a BCM2712 with PTP. **ARM build unverified** [I] |
| **bladeRF xA9** | 2 RX (+2 TX) | 56 MHz | open | Fully controllable waveform plus GPS reference (§1) |
| **ESPARGOS One** | 8, phase-coherent | 2.4 GHz | open (pyespargos LGPL); see the espargos note [V] | The cheapest true phased array; extends to any 2.4 GHz emitter |

No router uses the Pi's BCM2712. "The right Broadcom" for routers means the **Wi-Fi** chip that Nexmon or AX-CSI supports (BCM4366c0, BCM43684). [I]

## 4. UWB ground truth

Qorvo DWM3000-class nodes give ~10 cm two-way-ranging positions for heads and targets: the labels for training and checking RF sensing. Already working: the **NUCLEO-N657 + DWM3000EVB** reads `DEV_ID 0xDECA0302` (`scripts/n6.sh dw3000-id`). Add 2-3 Makerfabs ESP32 UWB DW3000 boards ($43.80 each) as anchors. RuView's RF encoder registry already has a `dw3000` adapter (RuView ADR-274). [V]

## 5. Post-processing compute

Signal processing (FFT, dechirp, channel estimation, beamforming) belongs on the **FPGA or GPU**. An NPU only runs **inference** on the resulting features. [I]

| Host | Price | Fit |
|---|---|---|
| Jetson Orin NX 16 GB (Seeed J4012) | $1,277 | CUDA DSP + 16 GB for small local models; mature |
| Arduino Ventuno Q | $299 preorder | 40 TOPS NPU, A78 cores, USB 3, 2.5 GbE; best value, new hardware |
| CM5 node (cog tier) | ~$100-480 | PTP endpoint, cogs, UWB/LoRa glue; not the SDR host |

Prices are from [mesh-placement/sbc-inventory-2026-10.md](mesh-placement/sbc-inventory-2026-10.md).

## 6. Bill of materials (indicative)

| Item | Budget | Proven |
|---|---|---|
| bladeRF xA9 + enclosure + antenna | $905 | $905 |
| GPS | DFR1103 PPS, $26 | GPSDO with 10 MHz out, ~$150 [I] |
| UWB anchors (2×) | $88 | $88 |
| Host | Ventuno Q, $299 | Jetson Orin NX, $1,277 |
| Synced-antenna CSI (optional) | used RT-AC86U, ~€60-200 | + AX-CSI router on request |
| **Total** | **~$1,320 (+ router)** | **~$2,420 (+ router)** |

Transmitting from the bladeRF stays inside the 902-928 MHz ISM rules (see the LoRa note).

## 7. Next steps

1. **PTP on BCM2712:** two Pi 5/CM5 on a switch, Raspberry Pi OS, `ethtool -T eth0`, then `ptp4l -H` / `phc2sys`. Record the steady-state offset.
2. **GPS grandmaster:** DFR1103 PPS → GPIO → chrony on that node; serve PTP to the second node.
3. **RT-AC86U:** buy one used and bring up `nexmon_csi`. Record per-core CSI and test chain-phase stability against a fixed transmitter, which is the calibration question the forums leave open.
4. **bladeRF:** confirm 10 MHz (and 1PPS) taming with a reference source before choosing between a GPSDO and plain PPS.
