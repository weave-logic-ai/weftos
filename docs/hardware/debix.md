# DEBIX — Polyhex industrial SBCs and SoMs — WeftOS/WeaveLogic hardware KB

Scope: **DEBIX** (debix.io), the industrial-board brand of **Polyhex Technology
Company Limited**. DEBIX sells finished single-board computers, system-on-modules,
box PCs built on those boards, and a small add-on set (PoE, 4G, isolated I/O,
MIPI cameras). The store is first-party. A Mouser line was not found.

The module explorer had **no published DEBIX part** on 2026-10-05. That day the
eight stage rows below were submitted as pending module contributions
(`debix-model-a`, `debix-model-b`, `debix-model-c`, `debix-r3576-01`,
`debix-t62p-01`, `debix-som-a`, `debix-som-b`, `debix-som-c`). Review still has
to accept them before they show up as published catalog rows. The only i.MX 8M Plus
already in the parts pool is the Toradex Verdin iMX8M Plus SoM. DEBIX is the
ready-to-boot board version of that same NPU class, plus cheaper i.MX 9 boards
and two non-NXP hosts (Rockchip RK3576, TI AM62P).

Public vendor claims only. Prices are configuration bands seen on debix.io on
2026-10-05 and move with RAM, eMMC, and temperature grade. Last compiled
2026-10-05.

Roles:
- **NPU sensing node** — on-die NPU, candidate for small on-device inference.
- **Real-time gateway** — application cores plus a real-time core and TSN Ethernet, no NPU.
- **Low-power leaf** — about 1 W, i.MX 9.
- **Evaluated-only** — not in use. None of these boot the Cog Seed card.

A DEBIX image is its own aarch64 Ubuntu, Yocto, or Debian build from
[debix.io/download-system-image](https://debix.io/download-system-image/). It does not
boot the armhf cog0 card. Published download pages ship a default `debix` /
`debix` account. Change it before a board joins a mesh.

---

## What belongs in the module explorer

Stage these as **board** modules. Skip the Lite / Quad Lite SKUs when the reason
to buy the board is the NPU.

| Part | SoC | NPU | RAM | Ambient | Role |
|---|---|---|---|---|---|
| **Model A Standard** | i.MX 8M Plus, 4× A53 (1.6 GHz industrial / up to 1.8 commercial) + M7 @ 800 MHz + HiFi 4 | **2.3 TOPS** | 2 GB LPDDR4, 4/8 GB optional | −20 °C to 70 °C | Finished NPU SBC. $146–$299 |
| **Model B** | same i.MX 8M Plus + M7 | **2.3 TOPS** | 4 GB LPDDR4 (1/2 GB optional) | **−40 °C to 85 °C** | Wide-temp NPU SBC. Dual GbE, one TSN. Product page $276; series card $178 |
| **Model C** | i.MX 9352, 2× A55 @ 1.7 GHz + M33 @ 250 MHz | **Ethos-U65, 0.5 TOPS** | 1 GB LPDDR4/4X, 2 GB optional | −20/70 or −40/85 | ~1 W full load. $62–$100 product page, series card $62–$87 |
| **R3576-01** | RK3576, 4× A72 @ 2.2 + 4× A53 @ 2.0 + M0 | **6 TOPS RKNN** | 2 GB LPDDR4, 4/8 optional | **0 °C to 70 °C only** | Most CPU. $165. Not the wide-temp part |
| **T62P-01** | TI AM62P, 4× A53 @ 1.4 GHz + 2× R5F | none stated | 2–8 GB LPDDR4 | −20/70, −40/85 optional | Dual TSN GbE, 40-pin header. Inquire; was out of stock |
| **SOM A** | i.MX 8M Plus + M7 + HiFi 4 class block | **2.3 TOPS** | 2 GB default; 8 GB only in the −20/70 grade | −20/70, −40/85 optional | SoM. Needs the SOM A I/O board or a carrier. $110–$195 |
| **SOM B** | i.MX 9352, 2× A55 + M33 | microNPU family (same 93 as Model C) | 1/2 GB LPDDR4X | −40/85 or −20/70 | SoM. $111 on the series card |
| **SOM C** | i.MX 9131, 1× A55 @ 1.4 GHz | none | 1 GB LPDDR4X, 2 GB optional | −40/85 default | 1.24 W max. Yocto and Zephyr. $80–$90 |
| **M8391-01** | MediaTek Genio 720 (MT8391), 2× A78 + 6× A55 | **NPU850, up to 9 TOPS** | 4 GB LPDDR4 default, 8/16 optional | commercial 0/70; industrial −20/70 or −40/85 | Announced. See the note below |

Sources: the matching `https://debix.io/product/<slug>/` page, the i.MX 8 and
i.MX 9 series index cards, and for M8391-01 only the CNX Software report of
2026-10-05. A debix.io product page for M8391-01 was not retrieved.

**Model A Lite** (Polyhex’s older pages call it SE) drops the NPU, VPU, ISP, and
HiFi 4. **Infinity** is i.MX 8M Plus **Quad Lite**, part MIMX8ML4CVNKZAB, and the
product page states **NPU: none**. Infinity does keep the M7, dual Gigabit (one
TSN), Wi-Fi/BT 5.2, USB 3.0, and PCIe, at −20/70 or −40/85. Buy it for I/O and
temperature, not for inference.

**Model D** is on the i.MX 9 series index as an i.MX 9131 board (1× A55 @ 1.4 GHz,
1/2 GB, about $50–$95). A full product-page spec was not opened, so its I/O is
not recorded here. It is the SBC sibling of SOM C.

---

## Model A, Model B, SOM A — the i.MX 8M Plus tier

This is the same NXP block Toradex sells as Verdin iMX8M Plus: four Cortex-A53
cores, a Cortex-M7 at 800 MHz, a 2.3 TOPS NPU, and on the Standard / SOM A parts
a HiFi 4 DSP. The DSP is the audio and sample-clock path. The NPU is the small
vision / keyword / anomaly engine. Neither number is a Jetson.

Model A is the board you can power and boot. SOM A is the module: dual Gigabit,
one of them TSN, plus CAN FD, and it sits on the **SOM A I/O board** ($150–$170),
which also accepts SOM B and SOM C with reduced features. That carrier adds dual
GbE with PoE, six isolated RS232/RS485 ports, two isolated CAN ports, and
isolated digital I/O.

Model A’s own ambient rating on the current product page is −20 °C to 70 °C.
Model B is the −40 °C to 85 °C board and the series card caps it at 4 GB. Older
Polyhex PDFs quote a CPU-die range (−40 °C to 105 °C). Use the board ambient
from debix.io, not the die number.

Footprint on the 2022 Model A brief is 85 × 56 mm. Expansion is a pin header
(UART, I2C, CAN, GPIO, SPI), CSI, DSI, USB 3.0, and PCIe. That is not a claim
of Raspberry Pi HAT electrical compatibility.

OS images for A, B, and Infinity are shared. The current Ubuntu 22.04 build on
the download page is 64-bit, kernel 6.1.22, dated 2026-08-13.

## Model C and the i.MX 9 leafs

Model C is the board to stage next to the 8M Plus parts when the node has to
stay near 1 W. i.MX 9352, two A55 cores, an M33, Ethos-U65 at 0.5 TOPS, 1 GB
of LPDDR4/4X (2 GB optional), microSD plus 8 MB NOR, eMMC optional. Ubuntu
22.04 Server, Yocto L6.1.36, Debian 12 Server, and the page also lists OpenWrt
and FreeRTOS. The BPC-iMX93-01 box is this motherboard in a fanless case: two
Gigabit ports, one with TSN and PoE, Wi-Fi 4, and Bluetooth 5.2.

SOM C and Model D drop to one A55 and no NPU. SOM C’s own page caps the part at
1.24 W, speaks Yocto and Zephyr, and uses four 80-pin 0.5 mm board-to-board
connectors on a 60 × 40 mm module.

## R3576-01 and T62P-01 — the other two hosts

**R3576-01** is the performance board: RK3576, four A72 cores, a 6 TOPS RKNN
NPU, Mali-G52 MC3, dual Gigabit (one PoE), Wi-Fi 6, BT 5.0, HDMI up to
4K120, 85 × 56 mm. Ambient is 0 °C to 70 °C. It boots from eMMC; recovery is
MASKROM / USB loader. Android 14, Debian 12, Ubuntu 22.04 Server (kernel
6.1.84 on the download page).

**T62P-01** is the deterministic one. AM62P application cores plus two Cortex-R5F
cores, dual TSN Gigabit (one PoE), Wi-Fi 6, BT 5.4, a 40-pin header, 4-lane
MIPI CSI, and LVDS or MIPI DSI. The product page does not claim an NPU. It was
listed as out of stock.

## M8391-01 — announced, not yet in the store pull

CNX Software, 2026-10-05, describes a credit-card industrial SBC on the
MediaTek MT8391 (Genio 720): NPU850 up to 9 TOPS, 2× Cortex-A78 (2.6 GHz
commercial, 2.4 GHz industrial) plus 6× A55 at 2.0 GHz, Mali-G57 MC2, 4 GB
LPDDR4 standard with 8/16 GB optional, 32 GB eMMC standard, M.2 2242 PCIe 2.0,
4-lane MIPI CSI, MIPI DSI and eDP, Gigabit with PoE, Wi-Fi 6, BT 5.4, five USB
ports, and a 40-pin header. Treat the 40-pin header as a physical expansion
header. Do not treat “Raspberry Pi-inspired” as Pi HAT compatibility. Confirm
the debix.io product page before staging a catalog row.

## Add-ons that are parts, not hosts

| Part | Fits | What it adds | Price seen |
|---|---|---|---|
| I/O Board | Model A, B, Infinity | RS232, RS485, CAN, GbE with PoE, USB-C debug, RTC | $20 |
| SBC PoE module | A, B, C, Infinity, R3576-01 (index also names D) | 802.3at, 5 V / 4 A out, −40 °C to 85 °C | $19 |
| 4G Board | A, B, Infinity | miniPCIe modem slot + micro-SIM, 57 × 51 mm | $26 |
| SOM A I/O Board | SOM A full; SOM B and SOM C partial | isolated serial, CAN, DIO, dual GbE | $150–$170 |
| Camera 200A / 500A / 1300A | MIPI CSI boards | GC2145 2 MP, OV5640 5 MP, AR1335 13 MP | see the camera page |

The MIPI-to-HDMI adapter is listed for A, B, C, Infinity, R3576-01, and T62P-01.

## Ingest

Enumerate SBCs from https://debix.io/products/single-board-computer/ and SoMs
from https://debix.io/products/system-on-module/. Per-board specs are the
product pages under https://debix.io/product/. debix.io returned HTTP 403 to a
plain fetch of the index; the product pages were readable through search
snippets of those same URLs. Polyhex’s older catalog is
http://www.polyhex.net/product/embedded-motherboard.html and it still lists
Model A, Model B, SOM A, plus Polyhex’s own SOM-iMX8MM and EMB-RK3568-03, which
are not on the DEBIX series cards above. Prefer debix.io when the two disagree.

Box-PC SKUs (BPC-iMX8MP, BPC-iMX93) repeat the SBC. Catalog the motherboard
once.

## Related

- Same 2.3 TOPS SoC as a SoM: [toradex.md](toradex.md) (Verdin iMX8M Plus).
- Other finished SBCs: [sbcs.md](sbcs.md).
- Why 2.3 / 0.5 / 6 / 9 TOPS are not one scale: [ai-edge.md](ai-edge.md).
