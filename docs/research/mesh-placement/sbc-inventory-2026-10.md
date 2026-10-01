# SBC inventory for cog and local-model hosting (Electromaker, 2026-10-01)

Date: 2026-10-01. Status: research, nothing bought. Tags: [V] read from the source or measured; [I] inference.

**Scope.** Every product in Electromaker's "single board computers" category: **195 items**, pulled from the store's own JSON filter endpoint (`/shop/filter?mouser-category=single%20board%20computers`, 7 pages × 32). Each one is graded on whether it can host Cognitum cogs (ADR-100) as they are built today, and on how much local AI it brings (small local models, NPU TOPS). Prices and stock are Electromaker's (mostly Mouser-fed) on 2026-10-01. [V]

**The question is compatibility first.** Accelerator TOPS are a bonus for some cogs and for local inference (ADR-101). They don't rescue a board the cog binaries can't run on.

## 1. What "compatible" means, measured

Cog binaries as released, measured on our Pi 5 (`~/cogs-smoke/bin`, `file` + highest `GLIBC_` symbol): [V]

| Cog | Arch | Linking | Highest glibc symbol |
|---|---|---|---|
| anomaly-detect | aarch64 | dynamic | **GLIBC_2.34** |
| baby-cry, fall-detect, sleep-apnea | aarch64 | dynamic | GLIBC_2.28 |

So a host needs:

1. **CPU:** 64-bit Arm (aarch64). armv7 builds exist too (ADR-100), but **ARMv6 (Pi 1, Pi Zero W/WH) can't run armv7**. There is no x86_64 or riscv64 build yet, though ADR-100 lists x86_64 as "later". Cogs are Rust, so other targets are a build-matrix change, not a port. [V for the ADR; I for the effort]
2. **glibc 2.34 or newer** to run every sampled cog. glibc 2.28 runs most of them.

   | OS | glibc | Runs all sampled cogs? |
   |---|---|---|
   | Ubuntu 18.04 / JetPack 4 | 2.27 | no |
   | Ubuntu 20.04 / JetPack 5 | 2.31 | partly (not anomaly-detect) |
   | Debian 11 | 2.31 | partly |
   | Debian 10 | 2.28 | partly |
   | Ubuntu 22.04 / JetPack 6 | 2.35 | **yes** |
   | Debian 12 | 2.36 | yes |
   | Ubuntu 24.04 | 2.39 | yes |
   | Debian 13 | 2.41 | yes |

   [I, distro facts from memory]
   Our own builds should keep a **glibc 2.34 baseline or link statically with musl**, so they don't drift upward. A Debian 13 build environment can emit newer symbols. [I]
3. **4 KB memory pages** for armv7 cogs (the Pi 5 stock 16 KB kernel segfaults them; see the cogs repo `docs/devices/pi5.md`). aarch64 cogs don't care. [V]
4. **Memory limits that actually work.** Raspberry Pi firmware adds `cgroup_disable=memory`; append `cgroup_enable=memory cgroup_memory=1`. Measured on our Pi 5 on 2026-09-30. [V]
5. **The node must be on the sensor network.** ESP32 UDP on port 5006 is same-network only in v1 (ADR-100). That favours small boxes at each site over one big server. [V]

**Accelerators are a separate capability.** Released cogs are CPU-only. An NPU or GPU only helps cogs written for it and local inference (ADR-101). Advertise it through the ADR-099 capability vocabulary so placement can match it, for example `x.npu.hailo8`, `x.gpu.cuda`, `x.npu.qnn`, `x.npu.metis`, `x.npu.ti-c7x`. [I]

## 2. The inventory at a glance

| Group | Count | Cog verdict |
|---|---|---|
| A1. BCM2712 (Pi 5, CM5) | 8 | **Identical to our reference node.** Runs everything we have proven |
| A2. Other 64-bit Arm Linux | 36 | Runs aarch64 cogs if the OS ships glibc ≥ 2.34 (see per-board notes) |
| B. x86_64 | 91 | Needs an x86_64 cog build. Good local-model hosts once that exists |
| C. RISC-V | 2 | Needs a riscv64 build; experimental |
| D. 32-bit ARMv7 only | 14 | armv7 cogs only; old SoCs and 256 MB-1 GB of RAM |
| E. ARMv6 | 5 | **Not compatible** |
| F. Android-only | 4 | Not a Linux host |
| G. Microcontrollers and non-Linux | 31 | Not cog hosts (Pico, micro:bit, FEZ, Omega2 MIPS, Rabbit, ESP32, Z80) |
| H. Accessories and a gateway | 4 | TPM and Wi-Fi modules; a LoRaWAN gateway that is **EU868** (wrong band for North America) |

The full per-product list is in the appendix.

## 3. The boards that matter

### Exact match: BCM2712 (Pi 5 / CM5)

| Board | Price | Notes |
|---|---|---|
| Raspberry Pi 5, 1/2/4/8/16 GB | $45 / $65 / $110 / $197 / $305 | The reference node. Electromaker's 8 GB and 16 GB prices look well above Raspberry Pi list prices; buy plain Pis elsewhere [I] |
| EDATEC Mini-ITX CM5, 4 GB / 32 GB eMMC | $388 | Industrial carrier, 1 GbE **with PoE**, Wi-Fi/BT |
| Seeed reComputer AI Industrial R2135-12, CM5 8 GB / 32 GB + **Hailo-8 (26 TOPS)** | $480 | DIN rail, 9-36 V, −20 to 65 °C, RTC and watchdog. 1 GbE per Switch Science's spec; the SSD slot runs over USB because the Hailo-8 takes the PCIe lane; TPM optional (see the 2026-09-30 notes) [V] |
| Seeed reComputer AI R2140-12, Pi 5 16 GB + **Hailo-8** | $598 | Same accelerator in a desk/edge box rather than an industrial one [V title] |

### 64-bit Arm with local-model or TOPS headroom

| Board | Price | CPU / RAM | Accelerator | OS → glibc | Cog fit |
|---|---|---|---|---|---|
| **Arduino Ventuno Q** | $306 (on order) | Qualcomm Dragonwing IQ8, octa Cortex-A78/A55; **16 GB LPDDR5**, 64 GB eMMC, NVMe M.2, 2.5 GbE | **40 dense TOPS** NPU; plus an STM32H5 MCU (Zephyr) for real-time I/O and CAN-FD | upstream Ubuntu (Debian coming) → 2.39 if 24.04 [I] | **Best value for cogs plus small local models**, if the Qualcomm NPU toolchain works for us. New (launched 2026-08) [V, CNX] |
| Seeed reComputer Super J4012, Jetson Orin NX 16 GB | $1,277 (on order) | 8× A78AE; 16 GB | Ampere GPU, CUDA | JetPack 6 → 2.35 [I] | The mature choice for local LLMs (7-8B quantized fits in 16 GB) [I] |
| Seeed reComputer J3011, Orin Nano 8 GB | $814 | 6× A78AE; 8 GB | 40 TOPS, 67 in Super mode | JetPack 6 → 2.35 [I] | Small LLMs plus vision; 2× RJ45, NVMe [V] |
| Seeed reComputer J3010, Orin Nano 4 GB | $705 | 6× A78AE; 4 GB | GPU | JetPack 6 | Too little RAM for LLMs next to cogs [I] |
| Axelera Metis Compute Board | $940 | RK3588 (4× A76 + 4× A55), 16 GB | **Metis AIPU, 214 TOPS** (own 4 GB) plus a 6 TOPS NPU | vendor Linux; check glibc | Heavy vision at a site. Not an LLM box [V, CNX] |
| Particle Tachyon 4 GB / 64 GB | $350 (on order) | QCM6490, 1× A78 at 2.7 GHz + 3× A78 + 4× A55; 4 GB | 12 TOPS NPU, **5G**, Wi-Fi 6 | Ubuntu 24.04 → 2.39 | A cellular remote node. RAM is tight for LLMs [V, CNX] |
| BeagleY-AI | $87 (on order) | TI AM67A, 4× A53; 4 GB | **4 TOPS** | Debian [I] | The cheapest Arm board with TOPS. Pi form factor, PCIe [V title] |
| D-Robotics RDK X3 | $100 | 4× A53; 4 GB | 5 TOPS | vendor Ubuntu; check glibc | ROS-oriented. Its RDK X5 sibling (10 TOPS) was in the earlier sensor list [V title] |
| BeagleBone AI-64 | $200 | TI TDA4VM, 2× A72 | DSP/MMA, about 8 TOPS [I] | Debian [I] | Older and pricier than BeagleY-AI |

### Cheap 64-bit Arm leaves and gateways

| Board | Price | Notes |
|---|---|---|
| **Arduino UNO Q 4 GB / 32 GB** (2 GB / 16 GB at $59) | $79 | Qualcomm QRB2210, 4× A53 at 2.0 GHz, **upstream Debian**, plus an STM32U585 MCU for sensor I/O. A natural "sensor-adjacent" cog leaf [V, Arduino docs] |
| Raspberry Pi Zero 2 W | $18 | The same class as the Seed (6 s cycle at `--interval 1`) [V, ADR-100] |
| EDATEC CM0 Nano (512 MB) | $52-103 | CM0 is Zero 2 W-class silicon on an industrial carrier [I] |
| Raspberry Pi 4 (1/2/4/8 GB), Pi 400, Seeed R1125 (CM4) | $35-312 | A72, aarch64. Fine for cogs, slower than a Pi 5 |
| Raspberry Pi 3 B/B+/A+ | $25-40 | A53, aarch64 OS required; low RAM |
| BeaglePlay | $113 | TI AM625, 4× A53 [I]; sub-GHz radio on board [I] |
| Adafruit Vivid Unit (RK3399, 4 GB, 5.5" screen) | $187 | A touchscreen panel, not a server |
| i.MX 8M Mini (iWave $432, SoMLabs Titan $150), IBASE i.MX93 ($256, 2× GbE) | | Yocto/vendor BSPs; check glibc per image [I] |
| DFRobot Unihiker M10 | $98 | RK3308 (A35), small RAM; Debian 10 → glibc 2.28, so anomaly-detect won't run [I] |

### Avoid for cogs

| Board | Why |
|---|---|
| Seeed reComputer J1010 (Jetson Nano) | JetPack 4 / Ubuntu 18.04, glibc 2.27, **below every sampled cog**; end of life [I] |
| Jetson Xavier NX boxes (A203, A205E, J2022) | JetPack 5 / Ubuntu 20.04, glibc 2.31, so anomaly-detect fails; superseded by Orin [I] |
| Pi 1 A+/B+, Pi Zero W/WH, SparkFun Zero W kit | ARMv6 can't run armv7 cogs [I] |
| Inforce 6560 (Snapdragon 660/845) | Android OS |
| Onion Omega2 family | MIPS |
| ARMv7 i.MX6, AM335x, Zynq-7000, A20 boards | armv7-only, old, little RAM; only if a site already has one |

### x86_64 (91 boards), if we add an x86_64 cog build

Not compatible today. Rust makes the build cheap, but ADR-099's tiers prefer real ARM. Worth a look only for local-model hosting where an ARM option doesn't fit: [I]

| Board | Price | Why it's interesting |
|---|---|---|
| LattePanda Iota N150, 16 GB / 128 GB | $260 | Cheapest sensible x86 node with useful RAM |
| LattePanda Mu (N100 module) | $216 | A compute module for custom carriers |
| AAEON UP Xtreme i14 (Core Ultra 5 125H, 16 GB) | $1,275 | NPU plus Arc iGPU for OpenVINO or llama.cpp; expensive here |
| Axiomtek / IEI pico-ITX Core Ultra 5/7 | $979-1,778 | The same Core Ultra class on industrial boards |

Electromaker's x86 industrial boards (IEI, IBASE, Kontron, AAEON, SECO, ADLINK, PICMG cards) are priced for industrial supply chains. Most come without RAM, storage or an enclosure.

## 4. Recommendation

| Role | Pick | Why |
|---|---|---|
| **Standard cog node** (each sensor network) | **Pi 5 8 GB** (bench), **R2135-12 or EDATEC CM5** (field) | Identical silicon to everything proven; the CM5 boxes add industrial power, PoE and a watchdog |
| **Cogs plus vision TOPS** | R2140-12 (Pi 5 16 GB + Hailo-8), or R2135-12 | Same CPU as the reference node, plus a mature vision NPU |
| **Cogs plus small local LLMs, best value** | **Arduino Ventuno Q** ($306) | 16 GB, A78-class cores, 40 TOPS, 2.5 GbE, NVMe, upstream Ubuntu, plus an MCU for real-time sensor I/O. **Verify NPU tooling and glibc on a sample before standardizing** |
| **Cogs plus local LLMs, proven stack** | Jetson Orin NX 16 GB (J4012) | CUDA llama.cpp is mature; JetPack 6 glibc 2.35 clears the cog floor |
| **Heavy site vision** | Axelera Metis | 214 TOPS on an RK3588 host |
| **Cheap sensor-adjacent leaf** | Arduino UNO Q 4 GB ($79), or BeagleY-AI ($87) if 4 TOPS helps | Upstream Debian plus an MCU, or a cheap NPU |

Before buying any non-Pi board for cogs, run the same check on it that this note ran on the Pi 5:
1. `ldd --version` (glibc 2.34 or newer).
2. `getconf PAGESIZE`.
3. Look for `cgroup_disable` on `/proc/cmdline`.
4. Run `scripts/build.sh test-pi`-style admission on the real board: copy the conformance binaries over, run them, and record the results.

ADR-099's rule that ARM evidence comes from real hardware applies to every board here, not just the Pi.

## Sources

- Electromaker category feed, `https://www.electromaker.io/shop/filter?mouser-category=single%20board%20computers&page=1..7`, read 2026-10-01
- Cog binary glibc: measured on the Pi 5 `cognitum-weave`, 2026-09-30
- [CNX: Arduino Ventuno Q](https://cnx-software.com/2026/08/25/299-arduino-ventuno-q-sbc-combines-qualcomm-dragonwing-iq8-soc-and-stm32h5-mcu/), [CNX: Particle Tachyon](https://www.cnx-software.com/2024/07/31/tachyon-business-card-sized-sbc-based-on-qualcomm-qcm6490-arm-ai-soc-with-5g-and-wifi-6-connectivity/), [CNX: Axelera Metis Compute Board](https://www.cnx-software.com/2025/04/16/axelera-metis-compute-board-pairs-rockchip-rk3588-soc-with-214-tops-metis-ai-accelerator/), [Arduino UNO Q docs](https://docs.arduino.cc/hardware/uno-q/), [Switch Science R2135-12](https://www.switch-science.com/products/10473), [Switch Science J3011](https://www.switch-science.com/products/10475)
- WeftOS ADR-099, ADR-100, ADR-101; cogs repo `docs/devices/pi5.md`

## Appendix: all 195 products by group

### A1. BCM2712 (Pi 5 / CM5): identical to the reference node (8)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| EDATEC | Mini-itx Industrial Single Board Computer Based On Raspberry Pi Cm5, With Wifi & Bluetooth,4gb Ram, 32gb Emmc,… | $387.65 | 2 In Stock |
| Raspberry Pi | Raspberry Pi5/1gb | $45.00 | 269 On Order |
| Raspberry Pi | Raspberry Pi 5 Board with 2 GB RAM - SC1642 | $65.00 | 1236 On Order |
| Raspberry Pi | Raspberry Pi 5 Board 4GB - Quad-Core Cortex-A76, Dual 4K, Wi-Fi & Bluetooth | $110.00 | 4215 On Order |
| Raspberry Pi | Raspberry Pi 5 Board with 8 GB RAM - SC1432 | $197.41 | 1805 In Stock |
| Raspberry Pi | Raspberry Pi 5 Single-Board Computer, 16 GB RAM - SC1113 | $305.00 | 1186 In Stock |
| Seeed Studio | Seeed Studio reComputer AI Industrial R2135-12, CM5, Hailo-8 26 TOPS, 8 GB RAM, 32 GB eMMC - 114993595 | $479.88 | 1 In Stock |
| Seeed Studio | Seeed Studio reComputer AI R2140-12, Raspberry Pi 5, Hailo-8 26 TOPS, 16 GB RAM - 114993627 | $598.13 | 1 In Stock |

### A2. Other 64-bit Arm Linux (36)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| Adafruit | Raspberry Pi 400 Desktop - Computer Only | $75.00 | 119 In Stock |
| Adafruit | Adafruit Vivid Unit RK3399 Single-Board Computer, 4 GB RAM, 32 GB eMMC, 5.5-Inch Touchscreen - 5894 | $187.44 | 1 In Stock |
| Arduino | Arduino Uno Q 2g Ram 16gb | $59.00 | 1383 In Stock |
| Arduino | Arduino UNO Q Single-Board Computer, 4 GB RAM, 32 GB Storage - ABX00173 | $79.00 | 2422 In Stock |
| Arduino | Ventuno Q High-performance Edge Ai Computer Designed Specifically For Next-gen Ai And Robotics. | $306.48 | 291 On Order |
| Axelera AI | Metis Compute Board 16gb (sbc) With 1x Aipu, 4 Gb Of Ram And Active Cooling, Rev1 | $940.46 | 4 In Stock |
| BeagleBoard | BeagleY-AI Edge AI Single Board Computer with TI AM67A Processor, 4 TOPS AI Acceleration, Wi-Fi 6 & PCIe | $87.05 | 1100 On Order |
| BeagleBoard | Beagleboard's Beagleplay | $112.72 | 416 In Stock |
| BeagleBoard | BeagleBoard BeagleBone AI-64 Single-Board Computer - 102110646 | $200.00 | 237 In Stock |
| DFRobot | Unihiker M10 - Iot Python Single Board Computer With Touchscreen | $97.50 | 2 In Stock |
| DFRobot | D-robotics Rdk X3 Ros2 Ai Board (4gb, 5tops) | $100.00 | 5 In Stock |
| EDATEC | Industrial Single Board Computer Based On Raspberry Pi Cm0,without Wi-fi & Bluetooth, 512mb Ddr And Without Em… | $51.50 | 3 In Stock |
| EDATEC | Cm0 Nano Sbc Cm0100000 1ghz Quad-core Cortex-a53 512mb Ram Micro-sd Slot Hdmi Ethernet 2xusb 2.0 Wifi/bt Fpc A… | $53.56 | 2 In Stock |
| EDATEC | Cm0 Nano Sbc Cm0100008 1ghz Quad-core Cortex-a53 512mb Ram8gb Emmc Hdmi Ethernet 2xusb 2.0 Wifi/bt Fpc Antenna | $103.00 | 10 In Stock |
| IBASE | Nxp I.mx 93 Cortex-a55 Dual Processor, 1.7ghz,2gb Lpddr4 On Board, 32gb Emmc On Board (up To 256gb),2x Rj45 Gb… | $256.14 | 2 In Stock |
| Particle | Tachyon 4gb Ram / 64gb Flash (noram), [x1] | $349.95 | 22 On Order |
| Raspberry Pi | Raspberry Pi Zero 2 W With Header | $18.00 | 2581 On Order |
| Raspberry Pi | Raspberry Pi 3 Model A+ Single-Board Computer - SC0130(J) | $25.00 | 42 In Stock |
| Raspberry Pi | Raspberry Pi 3 Model B V1.2 Single-Board Computer - SC0022 | $35.00 | 2305 In Stock |
| Raspberry Pi | Raspberry Pi4/1gb | $35.00 | 3905 On Order |
| Raspberry Pi | Raspberry Pi 3 Model B+ Single-Board Computer - SC0073 | $40.00 | 1374 On Order |
| Raspberry Pi | Raspberry Pi 4 Model B 2GB - Compact Dual-Display Desktop Computer | $55.00 | 3063 On Order |
| Raspberry Pi | Raspberry Pi 4 Model B (4GB) | $112.21 | 829 In Stock |
| Raspberry Pi | Raspberry Pi 4 B (8GB) Rev 9 | $186.61 | 6603 In Stock |
| Seeed Studio | Recomputer R1125-10 - Raspberry Pi Iot Gateway & Controller, Cm4-powered, Ai Capable 4gb Ram, 32gb Emmc | $312.40 | 1 In Stock |
| Seeed Studio | reComputer J1010 Edge AI Device with Jetson Nano & M.2 Slot | $338.75 | 172 On Order |
| Seeed Studio | Seeed Studio reComputer J3010 Jetson Orin Nano 4 GB Edge AI Computer with 128 GB NVMe - 110110146 | $705.00 | 1 In Stock |
| Seeed Studio | Seeed Studio reComputer J3011 Jetson Orin Nano 8 GB Edge AI Computer with 128 GB NVMe - 110110147 | $813.75 | 1 In Stock |
| Seeed Studio | Recomputer J2022-edge Ai Device With Jetson Xavier Nx 16gb Module, 4xusb, M.2 Key E & Key M Slot, Aluminum Cas… | $826.84 | 26 In Stock |
| Seeed Studio | A203 Mini Pc With Jetson Xavier Nx 8gb Module, 128gb Ssd, 2xusb 3, Rs232, Wifi/ble, Aluminum Case, Pre-install… | $989.30 | 3 In Stock |
| Seeed Studio | A205E Mini PC with Jetson Xavier NX - High-Performance Computing in a Compact Form | $999.41 | 3 In Stock |
| Seeed Studio | Recomputer Super J4012 - Advanced Edge Ai Computer With Nvidia Jetson Orin Nx 16gb | $1276.53 | 9 On Order |
| SoMLabs | Titansbc Computer, I.mx 8m Mini Quad Core At 1.8ghz, 2gb Ram, 8gb Emmc, 0+70c | $150.11 | 2 In Stock |
| StereoLabs | Zed Box Orin Nx 16gb Orin 16gb, 256gb, Gmsl2, Gps | $2714.07 | 1 On Order |
| StereoLabs | The Zed Box Is A Powerful Ai Computer Offering Spatial Computing Capabilities For Autonomous Robotics And Smar… | $3506.69 | 1 In Stock |
| iWave Systems | I.mx 8mmini Quad Sbc With 2gb Lpddr4, 16gb Emmc, 2xeth, Wi-fi, Bt - Boot Code With Heatsink & 12v, 2a Power Ad… | $432.35 | 1 In Stock |

### B. x86_64 (needs an x86_64 cog build) (91)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| AAEON | The Up Board With X5-z8350 Cpu,2gb Ram, 16gb Emmc | $150.03 | 127 In Stock |
| AAEON | Up Squared Board With Colour Box Packing,apollo Lake. Intel N3350 (f1). 2gb Ddr4, 32gb Emmc. Rev A1. 0 | $419.73 | 175 In Stock |
| AAEON | Up Squared Pro, Upn-apl01.cpu N3350(f1).memory 2gb.emmc 32gb, 12-24v Dc-in ,rev.a1.0 | $465.89 | 2 In Stock |
| AAEON | Up Squared Board With Colour Box Packing,apollo Lake . Intel N3350 (f1). 4gb Ddr4, 32gb Emmc. Rev A1. 0 | $551.08 | 131 In Stock |
| AAEON | Up Squared Pro, Upn-apl01.cpu N3350(f1).memory 4gb.emmc 32gb, 12-24v Dc-in,rev.a1.0 | $588.43 | 9 In Stock |
| AAEON | Up4000 Board Intel Atom E3950, 4gb Ram, 64gb Emmc, Rev A1.0 | $615.14 | 42 In Stock |
| AAEON | Up4000 Board Intel Pentium N4200, 4gb Ram, 32gb Emmc, Rev A1.0 | $616.31 | 17 In Stock |
| AAEON | Up Twl.intel Processor N150.8gb Ram.64gb Emmc.a1.0 | $620.14 | 2 In Stock |
| AAEON | Up Squared Board With Colour Box Packing, Apollo Lake . Intel N4200 (f1). 4gb Ddr4, 32gb Emmc. Rev A1. 0 | $621.57 | 97 In Stock |
| AAEON | Up Twls.intel Processor N150.8gb Ram.64gb Emmc.a1.0 | $631.38 | 2 In Stock |
| AAEON | Up (up Squared) Board With Apollo Lake Intel Atom Quad Core X7-e3950 Up To 1.9ghz, On Board 4gb Ddr4, 64gb Emm… | $638.21 | 4 In Stock |
| AAEON | Up Squared 7100.intel Processor N100.8gb Ram.64gb Emmc | $677.35 | 20 On Order |
| AAEON | Up Sqaured Pro, Upn-apl01.cpu E3950(f1).memory 4gb.emmc 64gb, 12-24v Dc-in, Rev.a1.0 | $692.50 | 3 In Stock |
| AAEON | Up Squared Pro 7000.intel Processor N97.4gb Ram.32gb Emmc.a1.0 | $727.32 | 10 On Order |
| AAEON | Up4000 Board Intel Pentium N4200, 8gb Ram, 64gb Emmc, Rev A1.0 | $750.82 | 26 In Stock |
| AAEON | Up Squared Pro Twl.intel Processor N150.8gb Ram.64gb Emmc.a1.0 | $769.35 | 2 In Stock |
| AAEON | UP Squared Pro 7000 - Intel Atom x7425E, 8GB RAM, 64GB eMMC | $780.85 | 52 On Order |
| AAEON | Up Sqaured Pro, Upn-apl01.cpu N4200(f1).memory 8gb.emmc 64gb, 12-24v Dc-in,rev.a1.0 | $812.83 | 28 In Stock |
| AAEON | Up Squared Board With Colour Box Packing,apollo Lake. Intel N4200(f1). 8gb Ddr4, 128gb Emmc. Rev A1. 0 | $861.66 | 83 In Stock |
| AAEON | Up Squared Pro 7000.intel Core I3-n305.16gb Ram.64gb Emmc.a1.0 | $1091.57 | 91 On Order |
| AAEON | Up Xtreme Ptl.intel Core Ultra 7 Processor 356h.a1.0 | $1091.59 | 2 On Order |
| AAEON | Up Xtreme I14 Board With Intel Ultra 5 125h, 16gb Ram | $1274.90 | 14 In Stock |
| AAEON | Up Xtreme Ptl Edge.intel Core Ultra 7 Processor 356h.a1.0 | $1315.12 | 1 In Stock |
| AAEON | Up Xtreme Board With I7-8665ue. Onboard Lpddr4 16gb. Onboard 32gb Emmc | $1901.27 | 70 In Stock |
| ADLINK Technology | Amitx-sl-g-q170 Mini-itx For Intel 6th/7th Gen Core I7/i5/i3 Lga 1151 Desktop Processor With Q170 Chipset | $478.26 | 1 In Stock |
| ADLINK Technology | Core Module Intel Atom E3815 Bay Trail | $901.28 | 1 In Stock |
| ADLINK Technology | 4hp Cpci-6540 With Intel Xeon E-2276me And 32gbddr4-2666 Ecc Soldered Memory With 2x Gbe, 2x Usb 3.0, 1x Rj-45… | $6038.26 | 1 In Stock |
| ADLINK Technology | 4hp Cpci-6540 With Intel Core I7-9850hl And 16gb Ddr4-2666 | $8890.29 | 1 In Stock |
| Axiomtek | Intel Celeron N3060 With One Lan One Usb Vga/lvds Heat-spreader Heatsink And Cables | $333.71 | 1 In Stock |
| Axiomtek | Lga1151+h310 With Dual Gigabit Ethernet Dp/hdmi/lvds; Dual Displays; Gift Box | $434.00 | 5 On Order |
| Axiomtek | 3.5 Sbc With Amd Ryzen Apu V1605b Displayport/2 Hdmi/lvds And 2 Gigabit Lans With Fan | $623.40 | 1 In Stock |
| Axiomtek | Picmg 1.3 Full-size Cpu Card With 14th/13th/12th Lga1700 Socket, Intel R680e, Ecc, Dvi-i, 2.5gbe Lan, 6 Sata, … | $658.00 | 625 On Order |
| Axiomtek | 3.5 Sbc With Amd Ryzen Apu V1807b Displayport/2 Hdmi/lvds And 2 Gigabit Lans With Fan | $937.94 | 1 In Stock |
| Axiomtek | Pico-itx Sbc With Intel Core Ultra 5 Processor 125u, Hdmi, Lvds, Gbe Lan, 2.5gbe Lan, And Heatsink | $985.00 | 1 In Stock |
| Axiomtek | 8th Gen Intel Coretm I5-8365ueu Pico-itx Sbc With Displayport++, Hdmi, Lvds, And 2 Gbe Lan | $1042.33 | 1 In Stock |
| Axiomtek | Pico-itx Sbc With Intel Core Ultra 7 Processor 155u, Hdmi, Lvds, Gbe Lan, 2.5gbe Lan, And Heatsink | $1778.00 | 1 In Stock |
| DFRobot | Lattepanda Iota Palm-sized X86 Single Board Computer (intel N150, 8gb Ram / 64gb Emmc) | $165.01 | 1 On Order |
| DFRobot | Lattepanda Mu - A Micro X86 Compute Module (n100 Cpu,8gb Ram,64gb Emmc) | $216.25 | 74 On Order |
| DFRobot | Lattepanda Iota Palm-sized X86 Single Board Computer (intel N150, 16gb Ram / 128gb Emmc) | $260.00 | 47 On Order |
| DFRobot | Lattepanda Sigma - X86 Windows / Linux Single Board Computer Server (16gb Ram, 500gb Ssd, Wifi 6e) | $921.25 | 1 In Stock |
| GIGAIPC | Qbi-6412a Embedded Compact Board With Intel Celeron J6412 Processor, Single Channel Ddr4 Memory, 1 X Com , 1 X… | $261.83 | 1 In Stock |
| GIGAIPC | Itxl-6412a Thin Mini-itx Embedded Motherboard With Intel Celeron J6412 Processor, Dual Channel Ddr4 Memory, 4 … | $268.85 | 4 In Stock |
| GIGAIPC | Itxl-6210a Thin Mini-itx Embedded Motherboard With Intel Celeron N6210 Processor, Dual Channel Ddr4 Memory, 4 … | $289.43 | 4 In Stock |
| GIGAIPC | Qbip-6412a 3.5 Subcompact Embedded Motherboard With Intel Celeron J6412 Processor, Dual Channel Ddr4 Memory, 4… | $311.90 | 1 In Stock |
| GIGAIPC | Mitx-2748a Mini-itx Embedded Motherboard With Amd Ryzen V2748 Embedded Processor, Dual Channel Ddr4 Memory, Pc… | $984.83 | 4 In Stock |
| Hackboard | Hackboard 2 With Ubuntu Linux | $174.97 | 180 In Stock |
| Hackboard | Hackboard Single Board Computer With Psu And Windows 10 Pro | $199.98 | 121 In Stock |
| Hackboard | Hackboard 2 With Debian Linux 8gb Ram, 64gb Storage, International Psu | $226.23 | 2 In Stock |
| Hackboard | Hackboard 2 With Debian Linux 8gb Ram, 128gb Storage, International Psu | $236.23 | 6 In Stock |
| Hackboard | Hackboard 2 With Windows 10 Pro, 8gb Ram, 64gb Storage, International Psu | $249.95 | 2 In Stock |
| Hackboard | Hackboard 2 With Debian Linux 8gb Ram, 256gb Storage, International Psu | $249.99 | 2 In Stock |
| Hackboard | Hackboard 2 With Windows 10 Pro, 8gb Ram, 256gb Storage, International Psu | $275.00 | 5 In Stock |
| Hackboard | Hackboard 2 With Debian Linux 8gb Ram, 512gb Storage, International Psu | $275.04 | 8 In Stock |
| Hackboard | Hackboard 2 With Windows 10 Pro, 8gb Ram, 512gb Storage, International Psu | $349.98 | 11 In Stock |
| Hackboard | Hackboard 2 Windows 10 Pro Complete Kit, 8gb Ram, 512gb Storage, International Psu, Screen, Keyboard, Case, Ca… | $511.19 | 1 In Stock |
| IBASE | 3.5" Intel Atom X7-e3950 Qc Soc (1.6ghz/2.0ghz) Onboard, W/ I210it Gbe Lan X 2, Hdmi(1.4) , Dual Lvds (24-bit … | $399.87 | 7 In Stock |
| IBASE | Intel Atom X7-e3950 Processor 3.5 In Sbc | $403.63 | 1 In Stock |
| IBASE | Amd Ryzen Embedded R1000 3.5" Sbc | $523.06 | 1 In Stock |
| IBASE | Uatx, Lga1700 Core I7/i5/i3 & Pentium.celeron, Q670e Pch | $537.17 | 1 In Stock |
| IBASE | Uatx, Lga1700 Core I7/i5/i3, W680 Pch | $546.12 | 1 In Stock |
| IBASE | Uatx, Lga1700 Core I7/i5/i3 & Pentium/celeron, R680e Pch | $555.07 | 1 In Stock |
| IBASE | Itx, Amd Ryzen V2748 Qc Apu (2.9ghz/4.25ghz) Onboard With Cpu Cooler, W/ I211at Gbe Lanx2, 4x Displayport (1.4… | $903.85 | 1 In Stock |
| IBASE | 3.5" Intel Core-ultra 7 165h (24m Cache, Up To 5.00 Ghz) Onboard, W/ I226lm + I226v 2.5g Lan, Hdmi +dp + Edp +… | $1225.46 | 1 In Stock |
| IEI | Mini-itx Sbc Supports Lga1200 Intel 10th Generation Core I9/i7/i5/i3, Celeron And Pentium Processor, Ddr4, Dua… | $291.40 | 4 In Stock |
| IEI | Full-size Picmg 1.0 Cpu Card Supports Lga1150 Intel Core I7/i5/i3, Pentium And Celeron Cpu Per Intel H81, Ddr3… | $364.62 | 10 In Stock |
| IEI | Half-size Picmg 1.3 Cpu Card Supports Lga1200 Intel 10th/11th Gen. Core I9/i7/i5/i3/pentium/celeron Cpu With Q… | $371.94 | 1 In Stock |
| IEI | Pico-itx Sbc Supports Intel Quad-core Celeron J6412 2.0ghz On-board Soc, With 4gb Lpddr4x Memory On Board Defa… | $389.51 | 1 In Stock |
| IEI | Full-size Picmg 1.3 Cpu Card Supports Lga1700 Intel 12th/13th Gen. Core I9/i7/i5/i3/pentium /celeron Cpu With … | $434.29 | 4 In Stock |
| IEI | Pico-itx Sbc Supports Intel Atom X7433re On-board Soc, With 8gb Lpddr5 Memory On Board Default, With Hdmi, Lvd… | $580.00 | 1 In Stock |
| IEI | Pico-itx Sbc Supports Intel Alder Lake-n N97 On-board Soc, With 8gb Lpddr5 Memory On Board Default, With Hdmi,… | $727.15 | 2 In Stock |
| IEI | Pico-itx Sbc Supports Intel Atom X7835re On-board Soc, With 8gb Lpddr5 Memory On Board Default, With Hdmi, Lvd… | $859.32 | 1 In Stock |
| IEI | Mini-itx Sbc With Intel Tiger Lake-up3 Core I5-1145g7e Proccessor,ddr4 So-dimm,9 36v Dc Input,quad Display,sat… | $941.46 | 3 In Stock |
| IEI | 3.5" Sbc With Intel Meteor Lake-u Core Ultra 5 Processor 125u With Quad Displays,ddr5,double Intel I226v 2.5 G… | $979.20 | 1 In Stock |
| IEI | 3.5" Sbc With Intel Meteor Lake-u Core Ultra 7 Processor 155u With Quad Displays,ddr5,double Intel I226v 2.5 G… | $1050.00 | 1 In Stock |
| Innodisk | Mini-itx, Arrow Lake Cpu, Dc12v | $1933.70 | 1 In Stock |
| Intel | Intel Server Board S1200v3rpo | $273.33 | 4 In Stock |
| Kontron | 3.5" Sbc W/ Intel Atom X6212re, Etr (-40 To 85c) | $420.00 | 1 In Stock |
| Kontron | 3.5" Sbc W/ Intel Atom X6425re, Etr (-40 To 85c) | $485.00 | 1 In Stock |
| Kontron | 3.5"-sbc-tgl-4-i7-1185gre-xt | $1041.25 | 3 In Stock |
| Kontron | 3.5" Sbc W/ Intel I7-1185g7e | $1137.50 | 1 In Stock |
| Kontron | 3.5"-sbc-tgl-5-i5-1145gre-xt | $1297.50 | 1 In Stock |
| Nexcom | Scb 100 (safety Control Board) Is A Miniitx Board With Functional Safety Compatibility. | $1800.00 | 1 In Stock |
| SECO | Pitx - Sbc-a44-pitx W/bay Trail E3845 @1.91 Ghz Qc - Emmc 32gb - Power Jack - Serials Present - Audio Present … | $389.73 | 1 In Stock |
| SECO | Sbc - Udoo Vision X5 W/ Intel Atom X5-e3940 - 4gb Quad-channel Lpddr4 | $413.39 | 48 In Stock |
| SECO | UDOO Bolt V3 - Advanced SBC with AMD Ryzen & Radeon Graphics | $414.42 | 40 In Stock |
| SECO | Sbc - Udoo X86 Ultra W/ Intel N3710 - Ddr3l 8gb Dual Channel - Emmc 32gb | $492.19 | 33 In Stock |
| SECO | Sbc - Udoo Vision X7 W/ Intel Atom X7-e3950 - 8gb Quad-channel Lpddr4 | $497.00 | 49 In Stock |
| SECO | UDOO BOLT V8 Single-Board Computer - SC40-2000-0000-C0 | $518.34 | 45 In Stock |
| SECO | Udoo Bolt Gear - Kit Based On Udoo Bolt V8 With Metallic Case, Wifi Module, Power Supply | $639.90 | 36 In Stock |
| SECO | Sbc-c90 W/ Amd Ryzen Embedded V1807b A. 3.35ghz 45w, Horizontal Power Conn, Tpm 2.0, 2x Gbe (i210), 3-wire Fan… | $1108.73 | 2 In Stock |
| Seeed Studio | Odyssey - X86j4105800 (telec) | $205.00 | Not Available |

### C. RISC-V (needs a riscv64 build) (2)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| BeagleBoard | Beagleboard Beaglev-fire | $149.00 | 182 In Stock |
| BeagleBoard | BeagleBoard BeagleV-Ahead Open-Source RISC-V SBC - 102991698 | $151.14 | 453 In Stock |

### D. 32-bit ARMv7 only (armv7 cogs only) (14)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| BeagleBoard | Beagleboard X15 | $328.39 | 68 In Stock |
| Digi International | Connect Core 6ul Sbc I.mx6ul-2 Pico-itx | $342.21 | 5 In Stock |
| Digi International | Connectcore 6ul Sbc Pro 1g-1g-w-bt | $388.41 | 19 In Stock |
| Digi International | Connectcore Wi-i.mx6 Sbc, Quad, 1.2ghz, 4gb Emmc, 1gb Ddr3, Wi-fi 802.11ac, Bt 5.0, -20-70c | $517.88 | 3 In Stock |
| Eurotech | Cpu-351-13-30, Imx6 Sbc, Quad Core 800mhz, 4gb Ddr3, 4gb Flash, I Temp, Basic, R | $516.94 | 1 In Stock |
| Ezurio | Quad-core 1gb Ddr3 Nitrogen6x Ext Temp | $328.46 | 31 In Stock |
| MYIR | Mcimx6g2cvm05a, 256mb Ddr3, 256mb Nand, Industrial | $69.92 | 143 In Stock |
| MYIR | Zynq-7010, 1gb Ddr3, 16mb Spi Flash, Commercial | $179.57 | 1 In Stock |
| MYIR | Zynq-7020, 1gb Ddr3, 16mb Spi Flash, Commercial | $211.44 | 17 In Stock |
| Olimex Ltd. | Olimex A20-olinuxino-lime2 Open Source Hardware Embedded Arm Linux Android Computer With Allwinner Dual Core C… | $64.84 | 3 In Stock |
| Seeed Studio | Seeed Studio Beaglebone Green | $66.50 | 782 In Stock |
| Seeed Studio | Seeed Studio Beaglebone Green Wireless | $91.12 | 333 In Stock |
| VIA Technologies | Som+9x50 Module W/4gb Ram + Carrier Board | $300.72 | 3 In Stock |
| VIA Technologies | Freescale Cortex A9 Imx6 Quad Core 1ghz, Com, Can, Hdmi, Lvds, Usb, Gigabit Lan | $328.76 | 1 In Stock |

### E. ARMv6 (not compatible: no armv7) (5)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| Raspberry Pi | Raspberry Pi Zero W Wireless Single Board Computer | $15.00 | 563 On Order |
| Raspberry Pi | Raspberry Pi Zero WH - 1GHz CPU, 512MB RAM, Pre-Soldered GPIO, Wi-Fi | $16.00 | 310 On Order |
| Raspberry Pi | Raspberry Pi1 Model A+ | $20.00 | 119 In Stock |
| Raspberry Pi | Raspberry Pi1 Model B+ | $25.00 | 145 In Stock |
| SparkFun Electronics | The Sparkfun Raspberry Pi Zero W Camera Kit Provides You With A Pan/tilt Camera Controlled Via A Raspberry Pi … | $112.45 | 1 In Stock |

### F. Android-only (4)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| Penguin Edge | Penguin Edge Inforce 6560 Snapdragon 660 SBC, 3 GB RAM, 32 GB eMMC - IFC6502-00-P1 | $213.95 | 5 In Stock |
| Penguin Edge | Inforce 6560 Sbc (board Only) Snapdragon 660 Processor,;android Os, 3gb Lpddr4, 32gb Emmc Board Only. No Mipi-… | $225.00 | 1 In Stock |
| Penguin Edge | Inforce 6560 Sbc (board Only) Snapdragon 660 Processor,;android Os, 3gb Lpddr4, 32gb Emmc Board Only. No Mipi-… | $234.20 | 2 In Stock |
| Penguin Edge | Inforce 6560 Sbc (board Only) Snapdragon 845 Processor,;android Os, 3gb Lpddr4, 32gb Emmc Board Only. No Mipi-… | $307.50 | 1 In Stock |

### G. Microcontrollers and non-Linux boards (not cog hosts) (31)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| Crowd Supply | Obsidian Esp32 W/ Case | $60.00 | 4 In Stock |
| Digi International | Lp3510 | $272.54 | 40 In Stock |
| Digi International | Lp3500 | $299.80 | 27 In Stock |
| GHI Electronics | Sitcore Fez Flea Sbc | $15.00 | 106 In Stock |
| GHI Electronics | Sitcore Fez Pico Sbc | $20.00 | 73 In Stock |
| GHI Electronics | Sitcore Fez Feather Sbc | $39.95 | 5 In Stock |
| GHI Electronics | Sitcore Fez Duino Sbc | $49.94 | 83 In Stock |
| GHI Electronics | Sitcore Fez Bit Sbc | $49.94 | 11 In Stock |
| Olimex Ltd. | Risc-v Retro Like Diy Soldering Kit With Vga, Ps2 Keyboard Support | $1.33 | 34 In Stock |
| Olimex Ltd. | Dual-core Arm Cortex-m0+ With Soldered Connectors | $7.97 | 18 In Stock |
| Olimex Ltd. | Rp2040-pico-pc Motherboard For Rp2040 | $14.16 | 4 In Stock |
| Olimex Ltd. | AgonLight2 Z80 Retro SBC - BBC Basic, VGA & USB | $59.00 | 17 In Stock |
| Onion | Omega2 Pro | $57.72 | 33 In Stock |
| Onion | Omega2 Dash | $69.00 | 144 In Stock |
| Onion | Omega2 Pro Essentials Collection By Onion | $87.50 | 5 In Stock |
| Onion | Omega2 Lte North America Model | $99.00 | 23 In Stock |
| Onion | Omega2 Lte Global Model | $139.80 | 154 In Stock |
| Onion | Omega2 Pro Ulimate Collection By Onion | $186.30 | 5 In Stock |
| Raspberry Pi | Raspberry Pi Pico H - Compact Microcontroller for Creative Projects | $5.00 | 1294 In Stock |
| Raspberry Pi | Pico 2 Pairs Rp2350 With 4mb Of On-board Qspi Flash Memory | $5.00 | 2867 In Stock |
| Raspberry Pi | Raspberry Pi Pico W - Wireless Microcontroller Board with WiFi | $6.00 | 2365 In Stock |
| Raspberry Pi | Raspberry Pi Pico 2 W – Dual-Core 150MHz, Wi-Fi, Bluetooth, I/O Rich MCU | $7.00 | 1902 In Stock |
| SECO | Scheda Esp32 Sensor To Cloud, Users Button, Can Bus, Micro Sd Card - Ext.temp. | $74.44 | 3 In Stock |
| Seeed Studio | Raspberry Pi Pico Basic Kit | $8.01 | 48 In Stock |
| Seeed Studio | Raspberry Pi Pico 3 Pack | $21.21 | 61 In Stock |
| SparkFun Electronics | Rp2040 Thing Plus | $19.95 | 1 In Stock |
| SparkFun Electronics | Don T Let The Size Fool You The Bbc Micro:bit V2 Is The Perfect Device For You To Get Creative With Digital Te… | $20.63 | 4 In Stock |
| SparkFun Electronics | Micro:bit V2 Go Bundle: Compact Creative Coding Kit | $31.16 | 5 In Stock |
| Western Design Center (WDC) | 65xxcelr8r Board W/ W65c265s Mcu | $49.13 | 2 In Stock |
| microbit | Micro:bit V2 Board - Programmable SBC for Education and DIY Projects | $20.50 | 19 In Stock |
| microbit | Microbit Go Ver 2 | $22.28 | 2 In Stock |

### H. Accessories and a gateway (4)

| Supplier | Product | Price | Stock |
|---|---|---|---|
| IEI | Edp To 24 Bit Dual Channel Lvds Converter Board Converter Board (for Iei Display Module) | $35.49 | 1 In Stock |
| Kontron | D3627-a Tpm V2.0 Module | $16.00 | 20 In Stock |
| Kontron | Wlan Module Thunder Peak 9260 For Smartcase S500/s520 | $20.00 | 2 In Stock |
| Seeed Studio | Seeed Studio SenseCAP SX1302 4G LoRaWAN Indoor Gateway, EU868 - 114993080 | $159.00 | 1 In Stock |
