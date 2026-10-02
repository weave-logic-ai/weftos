# Single-board computers & compute nodes — WeftOS/WeaveLogic hardware KB

Public-facing hardware reference for the single-board computers and compute
nodes WeftOS / cogs work actually uses or has evaluated. Specs are drawn from
vendor product briefs and reputable coverage; one source URL per board. Facts
only — no credentials or confidential appliance internals.

Roles used below:
- **Seed appliance** — the Cognitum "Seed" sensing/agent appliance hardware.
- **Placement target** — a node the WeftOS governed placement layer can schedule
  cogs / inference / long-running jobs onto.
- **Sensing node** — a board carried for its I/O to host sensor cogs.
- **Evaluated-only** — considered as an alternative; not in active use.

---

## Raspberry Pi Zero 2 W
- **Role for us**: Seed appliance (original Cognitum Seed hardware)
- **SoC / CPU / arch**: Raspberry Pi RP3A0 system-in-package wrapping a Broadcom
  BCM2710A1 die — quad-core 64-bit Arm Cortex-A53 @ 1 GHz. Our original Seed
  unit is a Rev 1.0 running a 32-bit `armv7l` userland (the A53 is 64-bit
  capable, but the Seed image runs the 32-bit Raspberry Pi OS userland).
- **RAM / storage**: 512 MB LPDDR2 (in-package, fixed). Boot/storage via microSD.
- **I/O**: Unpopulated 40-pin HAT-compatible GPIO header (GPIO/I2C/SPI/UART/PWM);
  CSI-2 camera connector; 1× micro-USB OTG (data) + 1× micro-USB power; mini-HDMI
  video; on-board 2.4 GHz 802.11 b/g/n Wi-Fi + Bluetooth 4.2 BLE. 65 × 30 mm.
- **Notes/gotchas**: Only 512 MB RAM — the hard ceiling on what the Seed can run
  locally; swap-sensitive. Single-band 2.4 GHz Wi-Fi only. 32-bit `armv7l`
  userland means aarch64-only binaries will not run on this Seed; build for
  armv7/armhf.
- **Source**: https://datasheets.raspberrypi.com/rpizero2/raspberry-pi-zero-2-w-product-brief.pdf

## Raspberry Pi 5
- **Role for us**: Placement target + Seed appliance. One Pi 5 is **cog0**,
  running the Cognitum agent; a separate Pi 5 ("cognitum-weave") runs WeftOS
  natively.
- **SoC / CPU / arch**: Broadcom BCM2712, quad-core Arm Cortex-A76 @ 2.4 GHz,
  aarch64. 512 KB L2 per core + 2 MB shared L3; Arm Cryptographic Extension.
  VideoCore VII GPU.
- **RAM / storage**: LPDDR4X-4267 in 1 / 2 / 4 / 8 / 16 GB SKUs (our cog0 Seed is
  the 4 GB Model B). microSD; PCIe 2.0 x1 via the FPC connector for NVMe HATs.
- **I/O**: 40-pin GPIO header (GPIO/I2C/SPI/UART/PWM); 2× USB 3.0 + 2× USB 2.0;
  Gigabit Ethernet (with PoE+ via HAT); dual 4Kp60 micro-HDMI; 2× 4-lane
  MIPI CSI/DSI; PCIe 2.0 x1 FPC; dual-band Wi-Fi + Bluetooth 5.0; RP1 southbridge;
  on-board RTC + power button.
- **Notes/gotchas**: **cog0 Seed runs an aarch64 kernel but a 32-bit armhf
  userland** — kernel reports `aarch64`, but `dpkg --print-architecture` / the
  running toolchain is `armhf`, so native cogs for cog0 must be built armhf, not
  arm64. The "cognitum-weave" Pi 5 runs WeftOS natively (treat its arch
  separately — do not assume it matches cog0). Needs a 5V/5A USB-C PD supply to
  enable full USB current; active cooling recommended under sustained load.
- **Source**: https://datasheets.raspberrypi.com/rpi5/raspberry-pi-5-product-brief.pdf

## Orange Pi Zero 2W
- **Role for us**: Evaluated-only (alternate Pi-Zero-class node)
- **SoC / CPU / arch**: Allwinner H618, quad-core Arm Cortex-A53 @ up to 1.5 GHz,
  aarch64. Mali-G31 MP2 GPU.
- **RAM / storage**: LPDDR4 in 1 / 1.5 / 2 / 4 GB options; 16 MB SPI NOR flash
  on board; microSD. (No on-board eMMC; an optional expansion board adds it.)
- **I/O**: 40-pin header (GPIO/UART/I2C/SPI/PWM); USB-C (one OTG, one host); mini-
  HDMI (4K); on-board 2.4/5 GHz Wi-Fi 5 + Bluetooth 5.0 with onboard antenna.
  30 × 65 mm — Raspberry Pi Zero form factor.
- **Notes/gotchas**: Up to 4 GB RAM in the Pi-Zero footprint — a meaningful step
  up from the Zero 2 W's fixed 512 MB, which is why it was looked at. Allwinner
  BSP/mainline-kernel maturity and vendor OS support are the usual caveats vs the
  Raspberry Pi software ecosystem.
- **Source**: http://www.orangepi.org/html/hardWare/computerAndMicrocontrollers/details/Orange-Pi-Zero-2W.html

## Banana Pi BPI-M4 Zero
- **Role for us**: Evaluated-only (Pi-Zero-class replacement / sensing node)
- **SoC / CPU / arch**: Allwinner H618, quad-core Arm Cortex-A53 @ up to 1.5 GHz,
  aarch64. Mali-G31 GPU. (Same SoC family as the Orange Pi Zero 2W.)
- **RAM / storage**: 2 GB LPDDR4 + 8 GB eMMC on the common SKU (4 GB / 32 GB SKUs
  exist); microSD (SDIO 3.0).
- **I/O**: 40-pin header (28 usable GPIO; UART/SPI/I2C/PWM/I2S); 24-pin 0.5 mm FPC
  carrying 1× USB 2.0, IR, a 100 Mbps Ethernet lane, and 9 GPIO; 2× USB-C
  (one OTG/power, one host); mini-HDMI 2.0a (4Kp60 HDR10); 2.4/5 GHz Wi-Fi +
  Bluetooth 4.2. Raspberry Pi Zero 2 W footprint.
- **Notes/gotchas**: On-board eMMC and type-C are the draw over the Zero 2 W.
  Ethernet is only exposed via the FPC connector (needs a breakout), and it is
  100 Mbps, not GbE. Same Allwinner BSP maturity caveat as the Orange Pi.
- **Source**: https://wiki.banana-pi.org/Banana_Pi_BPI-M4_Zero

## Banana Pi BPI-M6
- **Role for us**: Evaluated-only (edge-AI sensing / inference node)
- **SoC / CPU / arch**: Synaptics (Senary) VideoSmart VS680 — quad-core Arm
  Cortex-A73 @ 2.1 GHz + a Cortex-M3, aarch64. Imagination GE9920 GPU. On-chip
  NPU (SyNAP DLA) rated ~6.75 TOPS.
- **RAM / storage**: 4 GB LPDDR4 + 16 GB eMMC (soldered); microSD.
- **I/O**: 4× USB 3.0; 1× Gigabit Ethernet; HDMI in + HDMI out; M.2 Key-E
  (PCIe + MIPI CSI); 40-pin GPIO header; USB-C power (5V/3A). 92 × 60 mm.
- **Notes/gotchas**: The NPU is the reason to care — it is an edge-AI/smart-
  display/camera SoC, so it is a candidate for on-node inference placement rather
  than a general Pi substitute. Toolchain/NPU SDK (SyNAP) is Synaptics-specific;
  ecosystem is far smaller than Raspberry Pi. HDMI-**in** is unusual and useful
  for capture workloads.
- **Source**: https://wiki.banana-pi.org/Banana_Pi_BPI-M6

## Banana Pi BPI-R4 Pro
- **Role for us**: Evaluated-only (high-throughput router / network gateway node)
- **SoC / CPU / arch**: MediaTek MT7988A (Filogic 880), quad-core Arm Cortex-A73
  @ 1.8 GHz, aarch64, with a hardware packet-processing engine (PPE) for
  NAT/flow offload.
- **RAM / storage**: 8 GB DDR4 + 8 GB eMMC; 256 MB SPI-NAND; microSD.
- **I/O**: 2× 10GbE SFP+ cages (multiplexed with adjacent 10GbE RJ45) + 4× 2.5GbE
  RJ45; WiFi 7 via add-in cards; 2× mini-PCIe (PCIe 3.0 x2), 2× M.2 M-key
  (PCIe 3.0 x1, NVMe), 3× M.2 B-key (USB 3.2, for 5G modems). Runs OpenWrt.
- **Notes/gotchas**: Not a general compute board — it is a router/gateway dev
  platform. Relevant to us as the network edge of a mesh (10G uplinks, PPE
  offload so routed traffic bypasses the CPU fast path). Needs a fan; OpenWrt is
  the primary OS story.
- **Source**: https://www.cnx-software.com/2025/10/20/banana-pi-bpi-r4-pro-board-offers-2x-10gbe-sfp-cages-6x-10gbe-2-5gbe-gbe-ports-wifi-7-support/

## Banana Pi BPI-WiFi6
- **Role for us**: Evaluated-only (low-cost WiFi 6 router / mesh edge node)
- **SoC / CPU / arch**: Triductor TR6560, dual-core Arm Cortex-A9 @ 1.2 GHz
  (32-bit armv7), with LSW line-card switching and hardware NAT up to ~5 Gbps.
  Wi-Fi handled by a companion Triductor TR5220 WiFi 6 chipset.
- **RAM / storage**: 512 MB DDR3; 128 MB SPI-NAND flash.
- **I/O**: 1× GbE WAN (optional PoE) + 3× GbE LAN; 2.4 GHz 802.11ax 2×2
  (≤573.5 Mbps) + 5 GHz 802.11ax 2×2 (≤2401.9 Mbps); 4 external antennas;
  6-pin debug UART; power/reset/WPS buttons; 12V barrel-jack power. 137 × 107 mm.
- **Notes/gotchas**: Cheapest evaluated option (~$30 kit). 32-bit Cortex-A9 and a
  vendor OpenWrt fork — limited headroom for anything beyond routing; not a
  compute placement target. Of interest only as an inexpensive WiFi 6 mesh edge.
- **Source**: https://www.cnx-software.com/2023/06/30/cheap-wifi-6-router-board-features-triductor-tr6560-tr5220-chips/

## x86-64 Ubuntu box (dual Xeon X5667)
- **Role for us**: Placement target (heavy / high-memory jobs)
- **SoC / CPU / arch**: Intel Xeon X5667 (Westmere-EP, 32 nm, released 2010),
  x86-64. Each socket is quad-core / 8-thread @ 3.06 GHz (3.46 GHz turbo),
  12 MB L3, LGA1366, 6.4 GT/s QPI, 95 W TDP. Our box reports **16 threads**,
  i.e. a dual-socket board (2 × 4C/8T). SSE4.2 + AES-NI, but **no AVX/AVX2**
  (predates AVX).
- **RAM / storage**: ~188 GB registered DDR3 (large multi-channel/dual-socket
  config). Running Ubuntu.
- **I/O**: Standard server I/O — PCIe (gen 2 era), SATA, multiple NICs; no GPIO/
  I2C/SPI (not a sensing node).
- **Notes/gotchas**: Huge RAM is the reason to use it — a target for
  memory-hungry jobs the Pis cannot hold. But the CPU is 2010-era: **no AVX2**,
  so any build/binary assuming AVX2 (many ML/vector kernels, some RVF/HNSW fast
  paths) will SIGILL — build with a conservative `-march` (Westmere/`nehalem`)
  or gate AVX2 code paths off. High idle power vs the Arm nodes.
- **Source**: https://en.wikipedia.org/wiki/Westmere_(microarchitecture)

---

## Comparison

| Board | CPU | Arch | RAM | Role for us | Notes |
|---|---|---|---|---|---|
| Raspberry Pi Zero 2 W | 4× Cortex-A53 @ 1 GHz (RP3A0 / BCM2710A1) | aarch64-capable, **armv7l userland** | 512 MB (fixed) | Seed appliance (original) | 512 MB ceiling; 2.4 GHz Wi-Fi only; build armhf |
| Raspberry Pi 5 | 4× Cortex-A76 @ 2.4 GHz (BCM2712) | aarch64 | 1–16 GB (cog0 = 4 GB) | Placement target + Seed (cog0; cognitum-weave runs WeftOS) | **cog0 = aarch64 kernel + armhf userland**; PCIe; 5V/5A PD |
| Orange Pi Zero 2W | 4× Cortex-A53 @ 1.5 GHz (Allwinner H618) | aarch64 | 1–4 GB LPDDR4 | Evaluated-only | Pi-Zero footprint, up to 4 GB; Allwinner BSP caveat |
| Banana Pi BPI-M4 Zero | 4× Cortex-A53 @ 1.5 GHz (Allwinner H618) | aarch64 | 2–4 GB LPDDR4 + eMMC | Evaluated-only | eMMC + USB-C; Ethernet only 100 Mbps via FPC |
| Banana Pi BPI-M6 | 4× Cortex-A73 @ 2.1 GHz + M3 (Synaptics VS680) | aarch64 | 4 GB LPDDR4 + 16 GB eMMC | Evaluated-only (edge-AI) | ~6.75 TOPS NPU; HDMI-in; SyNAP SDK; small ecosystem |
| Banana Pi BPI-R4 Pro | 4× Cortex-A73 @ 1.8 GHz (MediaTek MT7988A) | aarch64 | 8 GB DDR4 + 8 GB eMMC | Evaluated-only (router) | 2× 10GbE SFP+, 4× 2.5GbE, WiFi 7, PPE offload; OpenWrt |
| Banana Pi BPI-WiFi6 | 2× Cortex-A9 @ 1.2 GHz (Triductor TR6560) | armv7 (32-bit) | 512 MB DDR3 | Evaluated-only (router) | WiFi 6 2×2; ~$30; routing only, not compute |
| x86-64 Ubuntu box | 2× Xeon X5667 (Westmere-EP), 8C/16T @ 3.06 GHz | x86-64 | ~188 GB DDR3 | Placement target (heavy) | Huge RAM; **no AVX2** (SIGILL risk); high idle power |
