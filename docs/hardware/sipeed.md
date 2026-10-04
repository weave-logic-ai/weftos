# Sipeed catalog — WeftOS/WeaveLogic hardware KB

Vendor-focused reference for **Sipeed** (Shenzhen Sipeed Technology) — a RISC-V /
AI-edge / FPGA / SBC maker whose whole line is interesting to us because it is the
most complete **RISC-V-first** hardware catalog going, and because its FPGA and
improved-radio RISC-V parts bear directly on our SDR / radio-sensing work. Specs
are vendor-claimed (Sipeed wiki + reputable coverage); figures are nominal and
should be re-verified against the exact board revision before design work. Public
facts only. Last compiled 2026-10-04.

Roles used below (same vocabulary as [sbcs.md](sbcs.md) / [ai-edge.md](ai-edge.md)):
- **Placement target** — a node the WeftOS governed placement layer can schedule
  cogs / inference / long-running jobs onto.
- **Sensing / Fleet node** — a board carried for its I/O or radio to drive and
  forward a sensor (the microcontroller-class story, cf. [microcontrollers.md](microcontrollers.md)).
- **SDR back-end** — FPGA fabric you pair with an *external* RF front-end.
- **Evaluated-only / Candidate** — of interest, not in active use.

> **SDR honesty flag, up front.** Sipeed does **not** sell a dedicated SDR /
> RF-transceiver board — there is no Sipeed equivalent of a bladeRF, HackRF, or
> PlutoSDR (no on-board wideband ADC + tuner). Sipeed's relevance to SDR is
> **two indirect paths**: (1) the **Tang FPGA** boards as a cheap digital
> back-end you wire to an external RF front-end (AD936x / RTL2832U / discrete
> ADC+mixer), the same role the Zynq-7010 and LimeSDR's Cyclone IV play in
> [sdr-radio.md](sdr-radio.md); and (2) the **Bouffalo-Lab RISC-V wireless MCUs**
> (BL616/BL618, BL808) as "ESP32-class parts with a *better* radio" — Wi-Fi 6 +
> 802.15.4 (Thread/Zigbee) rather than Wi-Fi 4 + BLE. Those are **protocol
> radios, not wideband SDR**; the closest Sipeed-adjacent "SDR-ish" sensing is
> CSI extraction à la ESPARGOS ([sdr-radio.md](sdr-radio.md)), not IQ capture.

---

## 1. SDR & improved-radio parts (the headline)

### Tang FPGA boards as SDR back-ends → see §4 for the full family

The Tang family (Gowin FPGAs) is the SDR-relevant half of Sipeed. An FPGA with
LVDS/parallel I/O is exactly the **digital back-end** of an SDR: it clocks samples
in/out of an RF front-end and runs the DSP (DDC/DUC, filters, matched filters) in
fabric. Community work already puts **openwifi-class** full-stack 802.11 and
GNU-Radio-fed pipelines on Tang-class FPGAs. What Sipeed does **not** give you is
the RF front-end — you bring an AD9361/AD9363 FMC/discrete board, an RTL2832U tap,
or a direct-sampling ADC. Tang Primer 25K (GW5A, 4-lane MIPI I/O, PMOD) and Tang
Mega 60K/138K (DDR3, SFP, PCIe on the Pro) are the realistic SDR-DSP candidates;
Tang Nano 9K/20K are learning-tier (narrow I/O, small fabric). Detail + table in §4.

### Bouffalo Lab BL616 / BL618 (Sipeed M0S Dock / M0P Dock) — the improved-radio ESP32-class MCU

- **Role for us**: the forward-looking **Fleet / sensing node radio** — a direct
  "ESP32-C6 competitor" with a comparable-or-better radio stack at a lower price,
  and the specific "ESP32-type with improved radio" the ask is about.
- **Core / arch**: single-core **32-bit RISC-V (RV32IMAFCP)** @ 320 MHz (384 MHz
  OC), **480 KB SRAM**, 4 MB flash on the Sipeed module. Hardware FPU + packed-SIMD
  (the `P` extension) — Bouffalo pitches it for TinyML.
- **Radios (the point)**: **Wi-Fi 6 (802.11ax) 2.4 GHz** + **Bluetooth 5.2
  dual-mode** + **802.15.4** (Zigbee / Thread / Matter). That is a strictly richer
  radio than a classic ESP32 (Wi-Fi 4 + BLE) and matches/edges the ESP32-C6 — the
  only Espressif part with 802.15.4 (cf. [microcontrollers.md](microcontrollers.md)).
- **I/O**: USB 2.0 OTG (up to 480 Mbps — unusual at this price), RGB LCD, DVP
  camera, SDIO, Ethernet RMII, I2C/UART/SPI. **BL616 = 19 GPIO; BL618 = 35 GPIO**;
  M0S module is 11 × 10 mm.
- **Notes/gotchas**: RISC-V (upstream toolchain, **no espup** — contrast the Xtensa
  ESP32-S3 pain in [microcontrollers.md](microcontrollers.md)); Zephyr board support
  exists upstream. **Not in-repo** — no WeftOS firmware runs on Bouffalo silicon
  today, so treat the WeftOS substrate/ed25519 story as *unverified for us*. Vendor
  SDK (`bl_mcu_sdk`) is the primary path; the radio-stack maturity vs Espressif's
  esp-radio is the open question. M0S Dock is ~$4. *(verify current firmware/HAL
  maturity before committing)*
- **Source**: <https://wiki.sipeed.com/hardware/en/maixzero/m0s/m0s.html> ·
  SoC: <https://www.cnx-software.com/2022/12/29/bouffalo-lab-bl616-bl618-risc-v-mcu-wifi-6-bluetooth-5-2-zigbee/>

### Bouffalo Lab BL808 (Sipeed M1s Dock) — tri-core RISC-V + NPU + multi-radio

- **Role for us**: a **sensing node with on-board inference** — the "camera/audio
  cog that classifies locally and forwards over its own radio" class, a step above
  the BL616 in compute but with an older radio.
- **Core / arch**: **three** RISC-V cores — T-Head **C906 64-bit (RV64GCV)**
  @ 480 MHz (Linux-capable) + T-Head **E907 32-bit (RV32GCP)** @ 320 MHz + a
  **RV32EMC** LP core @ 160 MHz — plus a **BLAI-100 NPU** for audio/video detect.
  **768 KB SRAM + 64 MB PSRAM**.
- **Radios**: **2.4 GHz Wi-Fi** (Wi-Fi 4-class, b/g/n) + **Bluetooth 5.x dual-mode**
  + **Zigbee** (802.15.4), usable concurrently. Older radio than the BL616 — no
  Wi-Fi 6 — so it is the compute pick, not the radio pick.
- **I/O**: MIPI CSI camera (optional 2 MP), 1.69" 240×280 touch option, analog mic,
  TF-card, USB-OTG over USB-C, on-board USB-UART debugger.
- **Notes/gotchas**: the C906 core runs Linux (buildroot), the E907 runs RTOS — a
  genuinely heterogeneous part, which is also the complexity tax. PSRAM is large
  (64 MB) for the class. Same **not-in-repo / vendor-SDK** caveat as the BL616.
  M1s Dock is ~$11. The NPU TOPS figure is not published cleanly — treat BLAI-100
  as "small audio/vision accelerator", not a quantified tier. *(verify)*
- **Source**: <https://wiki.sipeed.com/hardware/en/maix/m1s/m1s_dock.html>

---

## 2. Single-board computers — the Lichee family (RISC-V-first)

Sipeed's SBCs are almost all **RISC-V** (the exception is the Allwinner-ARM
Longan Pi). They are the most credible RISC-V answer to a Raspberry Pi, and the
natural **placement-target** candidates if we want a RISC-V node in the mesh.

### Lichee Pi 4A (T-Head TH1520)
- **Role for us**: Placement-target candidate — the flagship RISC-V "Pi 4 rival".
- **SoC / CPU / arch**: **T-Head TH1520** — quad-core **Xuantie C910** @ 1.85 GHz
  (RV64GCV, vector) + a C906 audio DSP + an E902 low-power core. Imagination GPU
  (~50 GFLOPS) + **4 TOPS @ INT8 NPU**.
- **RAM / storage**: 4 / 8 / 16 GB LPDDR4X; 8 / 32 / 128 GB eMMC + microSD. Sold as
  the **LM4A** SoM on a carrier (same module the Console/Cluster use).
- **I/O**: 2× Gigabit Ethernet, Wi-Fi + BT, HDMI 2.0, 4-lane MIPI DSI, dual MIPI
  CSI, USB. 
- **Notes/gotchas**: performance lands roughly at **Raspberry Pi 4** class (≈2× the
  older VisionFive2) — RISC-V software maturity (Debian/Fedora RISC-V, mainline
  kernel for TH1520) is the usual caveat, not the silicon. Good reference RISC-V
  node; the NPU toolchain is T-Head-specific.
- **Source**: <https://wiki.sipeed.com/hardware/en/lichee/th1520/lpi4a/1_intro.html>

### Lichee Pi 3A (SpacemiT K1)
- **Role for us**: Placement-target candidate — the newer, interesting RISC-V node.
- **SoC / CPU / arch**: **SpacemiT K1** — **octa-core X60** @ 1.6 GHz (RV64GCV with
  **RVV 1.0** ratified vector — the standout), **2 TOPS @ INT8 NPU**, IMG BXE-2-32
  GPU (~20 GFLOPS, OpenGL ES 3.2 / Vulkan 1.2 / OpenCL 3.0).
- **RAM / storage**: 8 / 16 GB LPDDR4X; 32 / 128 GB eMMC. LM3A SoM + carrier.
- **I/O**: **2× Gigabit Ethernet, 2× PCIe Gen2, 4× USB 3.0**, Wi-Fi 4 + BT 5, dual
  1080p display, 16 MP camera, H.265/H.264 1080p60.
- **Notes/gotchas**: **ratified RVV 1.0** (not the draft 0.7.1 the TH1520 ships) is
  the reason to prefer this for any vectorized RVF/DSP work — our vector kernels
  target RVV 1.0. 8 cores + 2× PCIe makes it a stronger I/O node than the 4A. Newer
  → verify distro/kernel maturity. *(verify RVV-1.0 autovectorization toolchain state)*
- **Source**: <https://www.cnx-software.com/2024/09/05/licheepi-3a-a-spacemit-k1-risc-v-development-board-with-som-and-carrier-board/>

### Lichee RV Dock (Allwinner D1)
- **Role for us**: Evaluated-only — the cheapest way onto RISC-V Linux.
- **SoC / CPU / arch**: **Allwinner D1** — single **T-Head C906** @ 1 GHz (RV64GCV,
  the first mass-market RISC-V Linux SoC). 512 MB / 1 GB DDR3.
- **I/O**: HDMI (4K@30 via Display Shell), RTL8723DS **Wi-Fi 4 + BT 4.2**, USB-A
  host + USB-C OTG, 2× 20-pin GPIO, electret mic (+ optional 6-MEMS R6 mic-array).
  65 × 40 mm.
- **Notes/gotchas**: single-core, 512 MB-class — a *learning / single-sensor* board,
  not a compute node. Historically important (~$16 board that opened RISC-V Linux).
  The 6-mic array option is mildly interesting for acoustic cogs.
- **Source**: <https://wiki.sipeed.com/hardware/en/lichee/RV/RV.html>

### Lichee Console 4A / Cluster 4A / Cluster 3A (TH1520 / K1 form factors)
- **Console 4A**: TH1520 **mini-laptop** — 7" 1280×800 touch, 72-key keyboard +
  pointing stick, 16 GB LPDDR4X / 128 GB eMMC, M.2 SSD slot, Wi-Fi 6 + BT 5,
  3000 mAh. ~$299. A self-contained RISC-V dev box, not a headless node.
- **Cluster 4A / 3A**: **Mini-ITX cluster carrier** holding up to **7× LM4A (or
  LM3A) SoMs** — up to 128 GB LPDDR4X and ~896 GB storage aggregate, per-slot
  USB 3.0 + microSD, BMC over USB2.0, HDMI on slot 1. The only off-the-shelf way to
  get a **RISC-V compute cluster** in one box — directly relevant to a RISC-V mesh
  placement experiment.
- **Source**: <https://www.cnx-software.com/2023/08/28/sipeed-unveils-risc-v-tablet-portable-linux-console-and-cluster/>

### Longan Pi 3H (Allwinner H618) — the ARM outlier
- **Role for us**: Evaluated-only. **ARM, not RISC-V** — Allwinner **H618**, quad
  Cortex-A53 (aarch64), the *same SoC family* as the Orange Pi Zero 2W / Banana Pi
  M4 Zero already in [sbcs.md](sbcs.md). Carry it only as a Pi-class ARM alt; no
  RISC-V reason to prefer it over the boards we already track.
- **Source**: <https://wiki.sipeed.com/hardware/en/longan/h618/lpi3h.html> *(verify)*

---

## 3. AI-edge — the Maix family (on-device vision / audio)

Sipeed's AI line. The through-line is "a small SoC with an NPU/TPU + a camera +
a screen, programmable in MaixPy (MicroPython) or MaixCDK". **Accelerators
attached to a tiny host**, in the [ai-edge.md](ai-edge.md) sense, but self-contained.

### MaixCAM / MaixCAM-Pro (SOPHGO SG2002) — current gen
- **Correction to the brief**: MaixCAM uses the **SOPHGO SG2002**, *not* the
  Kendryte K230. (K230 is used in the CanMV-K230 and HuskyLens 2, not MaixCAM.)
- **Role for us**: on-device vision cog — object tracking, pose, OCR, YOLO/MobileNet.
- **SoC / compute**: SG2002 — **1 GHz C906 RISC-V** (+ optional ARM A53 for Linux)
  + a 700 MHz C906 running RTOS + an 8051 LP core; **1 TOPS @ INT8 NPU** (BF16
  model support). **256 MB DDR3**.
- **I/O**: 2.3" HD IPS touch (552×368), up to **5 MP** camera (4-lane MIPI CSI;
  GC4653 / OS04A10), **Wi-Fi 6 + BLE 5.4** module on board, TF-card. ~$40.
- **Notes/gotchas**: 256 MB RAM caps it to vision/audio, not LLMs. SOPHGO's TPU
  toolchain (tpu-mlir) is the ingest path. The on-board **Wi-Fi 6** is notable for
  the class. A credible Coral/Hailo-adjacent *vision* cog, self-hosted.
- **Source**: <https://wiki.sipeed.com/hardware/en/maixcam/maixcam.html>

### MaixCAM2 (Axera AX630C) — next gen
- **SoC / compute**: **Axera AX630C**, **3.2 TOPS NPU**, 4K imaging. Modular open
  camera platform (2026 crowdfunding). The step up from the SG2002 for heavier
  vision nets / 4K. *(verify shipping specs — newer/crowdfunded)*
- **Source**: <https://www.cnx-software.com/2026/01/27/maixcam2-modular-4k-ai-camera-is-based-on-axera-ax630-soc-with-3-2-tops-npu/>

### Maix Bit / Maixduino / MaixCube (Kendryte K210) — the original Maix gen
- **SoC / compute**: **Kendryte K210** — dual-core 64-bit RISC-V @ 400 MHz (600 OC),
  **8 MB SRAM**, **KPU** NPU (~0.5–1 TOPS), APU audio unit (up to **8-mic** input),
  hardware FFT accelerator. QVGA@60 / VGA@30 vision.
- **Boards**: **Maix Bit** (breadboard module + LCD + camera); **Maixduino**
  (Arduino form factor, pairs the K210 with an **ESP32** for Wi-Fi/BT — so this one
  *does* have a radio, via the ESP32); **MaixCube** (all-in-one 40×40 mm with 1.3"
  TFT + VGA cam, via the M1n module).
- **Notes/gotchas**: K210 is aging — small models only, quirky toolchain (kendryte
  SDK / MaixPy), no DVFS headroom. The **8-mic APU + FFT accelerator** is the
  genuinely interesting bit for acoustic/beamforming cogs on a cheap part. The
  Maixduino's integrated ESP32 is an easy Wi-Fi bridge.
- **Source**: <https://wiki.sipeed.com/hardware/en/maix/maixpy_develop_kit_board/maix_bit.html>

### Maix-II Dock (M2-Dock, Allwinner V831) — 1-liner
Allwinner **V831** (ARM Cortex-A7 + ~0.2 TOPS NPU) Linux AI-cam dev board — the
ARM middle gen between the K210 and the SG2002. Low priority vs MaixCAM. *(verify)*

---

## 4. FPGA — the Tang family (Gowin) · SDR / DSP / soft-core

Why we care (beyond SDR §1): FPGAs are the only way to put **hard-real-time DSP**
(matched filters, DDC/DUC, high-rate ADC capture) in fabric beside a radio — the
same reason the bladeRF's Cyclone V matters in [sdr-radio.md](sdr-radio.md). Tang
boards are the cheapest Gowin-FPGA on-ramp; the **Gowin EDA** toolchain (free for
these parts) is the gate, analogous to Quartus for the bladeRF.

| Board | Gowin FPGA | LUT4 | FF | BSRAM | DSP (18×18) | PLL | Memory on board | Notable I/O | SDR/DSP fit |
|---|---|---|---|---|---|---|---|---|---|
| Tang Nano 9K | GW1NR-9 | 8,640 | 6,480 | 468 Kb | — | 2 | 64 Mb PSRAM, 32 Mb SPI flash | HDMI, RGB/SPI LCD, BL702 USB-JTAG/UART, 27 MHz | Learning; soft-core (PicoRV), video out — too small for real SDR DSP |
| Tang Nano 20K | GW2AR-18 | 20,736 | 15,552 | 828 Kb | 48 | 2 | 64 Mb 32-bit SDRAM | HDMI, retro/RISC-V soft-core | Entry SDR/DSP; runs a RISC-V soft core, custom wireless protocols |
| Tang Primer 20K | GW2A-18 | 20,736 | 15,552 | 828 Kb | 48 | 4 | 128 MB DDR3, 32 Mb NOR | Core board + Dock/Lite ext, PMOD | DDR3 + PMOD → usable SDR back-end tier |
| Tang Primer 25K | GW5A-25 | 23,040 | 23,040 | 1,008 Kb | 28 | 6 | 64 Mb NOR | **4-lane MIPI I/O**, 3× PMOD, USB host, 23×18 mm core | GW5A (newer) + fast I/O + PMOD → best small SDR front-end candidate |
| Tang Mega 60K | GW5AT-60 | ~60K | — | — | — | — | DDR3 | PMOD/expansion | Mid SDR-DSP fabric *(verify)* |
| Tang Mega 138K (Pro) | GW5AT-138 | ~138K | — | — | — | — | DDR3 | **SFP, PCIe (Pro)** | Largest fabric — serious DSP / multi-channel SDR *(verify exact resources)* |

Smaller/older: **Tang Nano (original) / 1K / 4K** (GW1N tiny parts — blinky/learning),
**Tang Console** (retro-gaming), **Tang PMOD** accessories.

- **Honest fit**: for real wideband SDR DSP, the **Primer 25K** (modern GW5A, 4-lane
  MIPI, PMOD for an ADC/RF board) or a **Tang Mega** is the realistic pick; the Nano
  9K/20K are learning-tier (narrow I/O, small fabric, no DDR on the 9K). You still
  supply the RF front-end externally. The free Gowin EDA + an open openwifi-style
  stack is the software story.
- **Source**: <https://wiki.sipeed.com/hardware/en/tang/tang-primer-25k/primer-25k.html>
  · Nano 20K: <https://wiki.sipeed.com/hardware/en/tang/tang-nano-20k/nano-20k.html>

---

## 5. RISC-V MCU — Longan Nano (no radio)

- **Role for us**: Evaluated-only — a bare "Blue Pill-class" RISC-V MCU, **no radio**.
- **SoC / arch**: GigaDevice **GD32VF103CBT6** — **RV32IMAC**, Nuclei Bumblebee core
  @ 108 MHz, **128 KB flash / 32 KB RAM**. 0.96" TFT (160×80) option, USB-C, microSD,
  3× USART / 2× I2C / 3× SPI / 2× I2S / 2× CAN / USB-FS-OTG / 2× ADC / 2× DAC.
- **Notes/gotchas**: cheap, well-documented RISC-V-101 board; **no wireless at all**,
  so it is a sensor/GPIO MCU, not a Fleet radio node. Zephyr board support exists.
  Nothing SDR about it — listed for completeness of the RISC-V-MCU line.
- **Source**: <https://wiki.sipeed.com/hardware/en/longan/Nano/Longan_Nano.html>

---

## Comparison — Sipeed boards by class

| Board | SoC / FPGA | Arch | Compute | RAM | Radio | Our role |
|---|---|---|---|---|---|---|
| Lichee Pi 4A | T-Head TH1520 | RV64GCV (vec 0.7.1) | 4× C910 @1.85 GHz + 4 TOPS NPU | 4–16 GB LPDDR4X | Wi-Fi + BT | RISC-V placement node (Pi 4-class) |
| Lichee Pi 3A | SpacemiT K1 | RV64GCV (**RVV 1.0**) | 8× X60 @1.6 GHz + 2 TOPS NPU | 8–16 GB LPDDR4X | Wi-Fi 4 + BT 5 | RISC-V node; **RVV 1.0** + 2× PCIe |
| Lichee RV Dock | Allwinner D1 | RV64GCV | 1× C906 @1 GHz | 0.5–1 GB | Wi-Fi 4 + BT 4.2 | Cheapest RISC-V Linux; learning |
| Lichee Cluster 4A | 7× TH1520 | RV64GCV | up to 28 C910 cores | up to 128 GB | per-SoM | RISC-V cluster-in-a-box |
| MaixCAM | SOPHGO SG2002 | RISC-V + ARM | 1 TOPS NPU | 256 MB | **Wi-Fi 6 + BLE 5.4** | Self-hosted vision cog |
| MaixCAM2 | Axera AX630C | — | 3.2 TOPS NPU | — | — | 4K vision cog (newer) |
| Maix Bit/Cube | Kendryte K210 | RV64 | KPU ~0.5–1 TOPS + 8-mic APU | 8 MB SRAM | — (Maixduino adds ESP32) | Cheap vision/acoustic edge |
| **M0S Dock (BL616)** | Bouffalo BL616 | RV32IMAFCP | 320 MHz + TinyML | 480 KB SRAM | **Wi-Fi 6 + BT 5.2 + 802.15.4** | **Improved-radio Fleet node** |
| **M0P Dock (BL618)** | Bouffalo BL618 | RV32IMAFCP | 320 MHz, 35 GPIO | 480 KB SRAM | **Wi-Fi 6 + BT 5.2 + 802.15.4** | Improved-radio Fleet node (more I/O) |
| **M1s Dock (BL808)** | Bouffalo BL808 | RV64GCV + RV32 ×2 | C906+E907+LP + BLAI NPU | 768 KB + 64 MB PSRAM | Wi-Fi 4 + BT 5 + Zigbee | Sensing node w/ on-board inference |
| Tang Primer 25K | Gowin GW5A-25 | FPGA | 23K LUT4, MIPI/PMOD I/O | — | — (bring RF) | **Best small SDR back-end** |
| Tang Mega 138K | Gowin GW5AT-138 | FPGA | ~138K LUT4, SFP/PCIe | DDR3 | — (bring RF) | Serious SDR-DSP fabric |
| Longan Nano | GD32VF103 | RV32IMAC | 108 MHz | 32 KB | none | RISC-V-101 sensor MCU |

---

## Register Sipeed as a vendor

Already added to [manufacturer-catalogs.md](manufacturer-catalogs.md) and
`manufacturer-catalogs.json` under the **boards** area. Primary sources are the
**Sipeed wiki** (`wiki.sipeed.com`, mirrored at `github.com/sipeed/sipeed_wiki`)
and the store/AliExpress; Sipeed is **not** a first-party Mouser line — a few
parts reach Mouser/LCSC via distributors, but the wiki + store are authoritative.

## Candidates to add to the sensor-explorer parts pool later

Standout boards worth staging into the pool (see the sensor-explorer parts MCP):
- **Sipeed M0S Dock (BL616)** — the improved-radio ESP32-class pick (Wi-Fi 6 +
  Thread/Zigbee, RISC-V, ~$4). The single most on-target part for the stated
  SDR/improved-radio interest.
- **Sipeed M1s Dock (BL808)** — tri-core RISC-V + NPU + multi-radio sensing node.
- **Tang Primer 25K** and a **Tang Mega** — the realistic FPGA SDR back-ends; pair
  with an external AD936x / RTL2832U front-end.
- **Lichee Pi 3A (SpacemiT K1)** — a RISC-V placement node with **RVV 1.0** (matters
  for our vector DSP/RVF kernels) + 2× PCIe.
- **MaixCAM (SG2002)** — self-hosted vision cog with on-board Wi-Fi 6.

## Related
- SDR context + baselines: [sdr-radio.md](sdr-radio.md) (bladeRF/Zynq/ESPARGOS).
- ESP32 Fleet constraints these parts compete with: [microcontrollers.md](microcontrollers.md).
- Other SBCs / the arch-per-target rule: [sbcs.md](sbcs.md), [README.md](README.md).
- AI-edge accelerator framing (TOPS not cross-comparable): [ai-edge.md](ai-edge.md).
