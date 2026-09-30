# ESP-SDR and Linux CSI nodes (BPI-M4 Zero)

Date: 2026-09-30. Status: research, nothing built. Tags: [V] read in the cited source; [I] inference.

Two questions from the same thread:

1. What is ESPARGOS's ESP-SDR, and what does it add for RF sensing?
2. Could a Banana Pi BPI-M4 Zero be a sensing node, given its Wi-Fi?

The paper survey for the rest of ESPARGOS is in [README.md](README.md).

## 1. ESP-SDR

ESP-SDR is firmware that gets **raw I/Q samples** out of the Wi-Fi receiver of ordinary ESP32-family chips. It does this through an undocumented debug path that bypasses the fixed-function Wi-Fi modem. The modem's sample-dump engine writes the I/Q into internal SRAM ring buffers, and the CPU copies it out. ([project page](https://espargos.net/espsdr/), [esp-sdr README](https://github.com/ESPARGOS/esp-sdr)) [V]

| Property | Value | Source |
|---|---|---|
| Chips | ESP32, C3, C5, C6, C61, S2, S3, S31 (C2, H2, H21, H4 unsupported; P4 has no radio) | README chip table [V] |
| Sample rates | 80, 40, 20, 16, 10, 8, 4 MS/s (rate indices 0-6) | README "Commands and transport" [V] |
| Sample format | `CAP16` signed 8-bit I/Q, `CAP20` packed signed 10-bit I/Q; each reply is CRC32-checked | README [V] |
| Analog RX bandwidth | about 13-54 MHz | project page [V] |
| Frequency | Project page: 2.2-2.7 GHz on all chips, 4.8-6.0 GHz on C5 only. README: every chip *accepts tuning attempts* from 100-6000 MHz in 1 MHz steps. | [V] both; the difference is between "accepts" and "useful" [I] |
| Duty cycle | Low. The modem writes 2,560 Mbit/s into SRAM, but output is UART 3 Mbit/s, SPI about 13.3 Mbit/s, USB about 480 Mbit/s (S31), GbE 1,000 Mbit/s. Captures are **short bursts**, not a continuous stream. | project page [V] |
| Continuous streaming | `SoapyESPSDR` (GNU Radio / gqrx) on S31 over GbE at 8 and 16 MS/s is listed as under development. The README says the former Ethernet/vendor-USB streaming app "is no longer included". | project page, README [V] |
| Protocol | Newline ASCII commands (`INFO`, `CAPS`, `LIMITS?`, `FREQ`, `BANDWIDTH`, `GAIN`, `CAP16`/`CAP20`, `SYNC`, `RELEASE`); one client at a time, released after 5 s idle | README [V] |
| Licence | `ESPARGOS/esp-sdr` and `esp-web-sdr` have **no licence file** (both created 2026-09-28). `pyespargos` is LGPL-3.0. | GitHub API, 2026-09-30 [V] |
| Provenance note | "Parts of the firmware code are AI-generated"; IQ sampling is best understood on the ESP32-C61 (the ESPARGOS One chip) | README [V] |
| ESPARGOS One | An 8-channel phased array; it now does phase-coherent raw I/Q capture over internal SPI with triggering, so it can localize **any** 2.4 GHz ISM signal (Bluetooth, Zigbee, Wi-Fi), not only Wi-Fi CSI | project page [V] |

### What it changes for sensing

- **CSI vs I/Q.** Vendor CSI (ESP-IDF's callback) is one channel estimate per received Wi-Fi frame, over 52-114 subcarriers. ESP-SDR gives the time-domain signal itself. It can see non-Wi-Fi emitters (BLE, Zigbee, microwave ovens, ESP-NOW), measure the channel between frames, and run custom estimators such as a better CFO/SFO correction or super-resolution delay. [I]
- **But only in bursts.** At a low duty cycle it's a snapshot tool, not a replacement for continuous CSI streams. The realistic early use is spectrum and interference surveys, calibration, and emitter localization, not continuous pose. [I]
- **Licence blocks shipping.** With no licence file, the code is "all rights reserved" by default. We can read it and run it for research, but we can't vendor it into WeftOS, a cog, or RuView until ESPARGOS adds a licence. [I]
- **Undocumented silicon path.** A future ESP-IDF or ROM change could break it, and Espressif doesn't support it. Treat it as experimental hardware capability. [I]

### Where it would sit in WeftOS

ADR-099 lets a node advertise capability ids that aren't yet in the vocabulary. Unknown ids are carried and matched, and experimental ids use `x.`. If an ESP-SDR node were probed, it would advertise something like:

- `x.radio.iq.esp-sdr`, with attrs `chip`, `rates_msps[]`, `bands[]`, `max_burst_samples`, `transport` (uart | usb | spi | gbe), `provenance: probed`. `INFO`/`CAPS`/`LIMITS?` are exactly the probe it needs.
- The existing `feed.esp32-csi-udp` stays the CSI path.

Nothing here needs code yet. This is where a capture workload would ask for it. [I]

## 2. BPI-M4 Zero as a sensing node

### The Wi-Fi is the question, and it depends on the board revision

- The BPI-M4 Zero is an Allwinner H618 (4x Cortex-A53, 1.5 GHz), 2-4 GB LPDDR4, 8-32 GB eMMC, 2.4/5 GHz Wi-Fi, BT 4.2, 100 Mbit Ethernet over an FPC adapter. It costs from $24.50. ([Banana Pi wiki](https://wiki.banana-pi.org/Banana_Pi_BPI-M4_Zero), [CNX](https://www.cnx-software.com/2023/12/12/banana-pi-bpi-m4-zero-allwinner-h618-sbc-raspberry-pi-zero-2-w/)) [V]
- The wiki names no Wi-Fi chip. Armbian users report **two variants**: newer boards with SDIO Wi-Fi on a Broadcom/Cypress **CYW43455**, and older boards with a Realtek chip over USB. Wi-Fi on 6.x kernels needed an overlay (`bananapi-m4-sdio-wifi-bt`) and had reported problems. ([Armbian forum](https://forum.armbian.com/topic/51743-banana-pi-bpi-m4-zero-standard-support/), [Armbian board page](https://www.armbian.com/bananapi-m4-zero/)) [V, community reports]
- **Why the variant matters.** Nexmon CSI supports the `bcm43455c0` (firmware 7_45_189) on the Raspberry Pi 3B+/4B/5. It gives 64/128/256 subcarriers at 20/40/80 MHz, sent as UDP to port 5500. Realtek chips have no comparable CSI tool. ([nexmon_csi README](https://github.com/seemoo-lab/nexmon_csi)) [V]
- Nexmon's firmware patch is tied to one firmware build, and its tooling targets Raspberry Pi OS kernels. On an Allwinner board under Armbian we'd need the same `43455c0` silicon revision, that exact firmware loaded, and a working `brcmfmac` path with monitor mode on a vendor-ish kernel. **Plausible, unproven, and likely fragile.** Nobody has reported it working on the M4 Zero that we found. [I]

### Verdict

- **As a CSI capture node: not the first choice.**
  - An ESP32-S3 (about $5-10) is cheaper, already RuView's native node, and has Secure Boot and a provisioning path.
  - RuView's own review found that "Nexmon-on-Pi is not obviously a win" over the ESP32-S3 mesh, on cost, security posture and provisioning (RuView `docs/research/sota/2026-Q2-rf-sensing-and-edge-rust.md` section 1.3). [V]
  - The 80 MHz / 256-subcarrier Nexmon data is the real draw, and **our Pi 5 already has a bcm43455c0** that Nexmon supports officially. Prove Nexmon on the Pi 5 before buying anything. [I]
- **As a Linux edge node beside the ESP32s: yes, a good fit.** It could run the RuView sensing server or aggregator, a WeftOS node, or an ESP-SDR host (USB to an ESP32-C5/S3), with 2-4 GB RAM and eMMC, for about $25-37. Ethernet over the FPC adapter keeps the Wi-Fi radio free for sensing. [I]
- **BFI needs no Nexmon.** RuView's `tools/bfi` (upstream `aac41555`, 2026-09-29) captures beamforming feedback in plain monitor mode. Either M4 Zero variant could serve as a BFI node if its driver supports 5 GHz monitor mode. Untested. [I]
- **Check the revision before buying.** Buy only boards confirmed to carry the CYW43455 if Nexmon matters. `lsusb` showing a Realtek device means the USB variant. [I]

### Rejected: BPI-WiFi6 router ($30)

- The router is a Triductor TR6560 SoC (2x Cortex-A9) with a TR5220 radio. Wi-Fi 6 is 2x2 on 2.4 and 5 GHz, and the software is a vendor fork of OpenWrt on Linux 5.10. The advertised memory is 512 MB DDR3 and 128 MB NAND. ([CNX](https://www.cnx-software.com/2024/03/12/banana-pi-30-wifi-6-router-triductor-tr6560-openwrt/)) [V]
- The Wi-Fi driver is a closed binary. `iw list` shows only `managed` and `AP` modes, with **no monitor mode**. There is no Triductor code in mainline, so upstream OpenWrt support is unlikely. One teardown reported 128 MB RAM, not 512 MB. ([OpenWrt forum](https://forum.openwrt.org/t/bpi-wifi6-git-repo/169574)) [V, forum reports]
- The consequence: no CSI, no BFI sniffing, and no Nexmon-style patching. The most it could do is act as an ordinary AP that generates traffic, which any AP can do. For a sensing-capable router, look at MediaTek boards on the open `mt76` driver instead, such as the BPI-R3 or R3 Mini (MT7986), which have monitor mode and upstream OpenWrt. That is unverified for sensing. [I]

### The Pi Zero 2 W (Cognitum Seed) can't do this

The Seed's Wi-Fi is the 43430/43436 family, and Nexmon CSI does not list it. The Seed takes CSI from ESP32 nodes over UDP 5006 instead. We confirmed the board on 2026-09-30 from `/proc/device-tree/model`: "Raspberry Pi Zero 2 W Rev 1.0", CPU part 0xd03. [V for the board; I for the chip-support inference from the nexmon_csi support list]

## Next steps if we pursue it

1. Nexmon CSI on our Pi 5 (Raspberry Pi OS, `Makefile.rpi`, [discussion #395](https://github.com/seemoo-lab/nexmon_csi/discussions/395)). Record the subcarrier count, frame rate and stability. The Pi 5 must keep its `kernel8.img`; check that Nexmon builds against that kernel.
2. Flash ESP-SDR onto an ESP32-S3 or C5 we already have (browser flasher) and capture a 2.4 GHz survey next to a CSI node.
3. Only then consider a BPI-M4 Zero, CYW43455 revision, as a cheap Linux aggregator/Nexmon node.
