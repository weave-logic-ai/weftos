# AI & edge-inference compute — WeftOS/WeaveLogic hardware KB

Scope: hardware candidates for the **AI / edge-inference compute tier** that WeftOS
governs — local LLM / vision inference plus NPU/GPU/TPU workload placement across a
mesh of nodes (Seed/Pi class, an x86-64 box, and dedicated AI accelerators). Each
entry is PUBLIC-facts-only with a source URL. Newly-announced or shifting numbers are
flagged. Last compiled 2026-10-02.

Two broad categories live in this tier:

1. **Self-hosting SoC/module compute** — Jetson Orin, Radxa/RK3588, DGX Spark. These
   run a full OS and can host LLM / vision models directly. Candidate mesh nodes.
2. **Bolt-on inference accelerators** — Coral Edge TPU, Hailo-8/8L. No OS of their
   own; they attach to a host (often a Pi or x86 box) over USB/PCIe/M.2 and offload
   quantized vision inference. Candidate accelerators *attached to* a mesh node.

---

## NVIDIA Jetson Orin family (Orin Nano / Orin NX / AGX Orin)

The mainstream CUDA-capable edge-AI module line. All are Ampere-architecture GPU +
Arm Cortex CPU SoM (system-on-module) sold as a module that drops onto a carrier, with
NVIDIA reference developer kits available. TOPS figures below are INT8 (sparse unless
noted); this is NVIDIA's headline unit.

- **Role for us**: Self-hosting mesh node. The realistic "run a real model locally at
  the edge" tier below DGX Spark — small/quantized LLMs (7B-class quantized on 16GB+),
  and strong multi-stream vision. First-class CUDA/TensorRT ecosystem is the main draw.
- **Compute / memory**:
  - **Orin Nano 4GB** — ~20 TOPS, 4GB LPDDR5. (Entry; vision, light inference.)
  - **Orin Nano 8GB** — originally 40 TOPS; the **"Super" software/dev-kit update
    (Dec 2024)** raises it to **67 sparse-INT8 TOPS**, 8GB LPDDR5, 102 GB/s bandwidth.
    1024 CUDA cores, 32 Tensor cores.
  - **Orin NX 8GB** — ~70 TOPS, 8GB LPDDR5, 1024 CUDA cores.
  - **Orin NX 16GB** — ~100 TOPS (157 TOPS headline on newer "Super" positioning for
    concurrent pipelines — see note), 16GB LPDDR5, 1024 CUDA cores, 102 GB/s.
  - **AGX Orin 32GB** — ~200 TOPS (INT8), 32GB LPDDR5, 1792 CUDA cores, 204 GB/s.
  - **AGX Orin 64GB** — up to **275 TOPS**, 64GB LPDDR5, 2048 CUDA cores, 64 Tensor
    cores, 204 GB/s. Server-class edge; best Jetson for larger local models.
- **Interface / form factor**: SoM over a board-to-board connector onto a carrier
  (260-pin SODIMM-style for Nano/NX; larger connector for AGX). NVIDIA devkits provide
  M.2, USB, GbE/multi-GbE, CSI camera lanes, PCIe. AGX Orin devkit is a self-contained
  box.
- **Power**: Orin Nano 7–25 W; Orin NX 10–25 W; AGX Orin 15–60 W (configurable nvpmodel
  power modes). Fanless possible at the low end of each range.
- **Fit / notes**: The CUDA + TensorRT + JetPack stack is the big differentiator vs the
  int8-only TPUs below — you can actually run LLM/transformer workloads, not just
  pre-compiled vision graphs. Memory is **not** unified-huge like DGX Spark, so model
  size is capped by the 8/16/32/64GB RAM. Good "governed accelerator node" in the mesh:
  WeftOS can place a vision or small-LLM job here. Watch: TOPS "Super" re-labelling
  means marketing numbers shifted in late 2024 — pin the exact module + JetPack when
  quoting. Source:
  <https://developer.nvidia.com/embedded/jetson-modules> ,
  Orin Nano Super: <https://developer.nvidia.com/blog/nvidia-jetson-orin-nano-developer-kit-gets-a-super-boost/>

---

## NVIDIA DGX Spark (GB10 Grace-Blackwell "Spark", formerly Project DIGITS)

Desktop "personal AI supercomputer." Announced as Project DIGITS (CES Jan 2025),
shipped as **DGX Spark** — **on sale 2025-10-15**. This is recent; treat pricing as
volatile.

- **Role for us**: The heavyweight local-inference node. Its reason to exist is
  **large unified memory for LLMs** — fine-tune / inference reasoning models that won't
  fit on a Jetson. In our mesh it's the "send the big model job here" tier, sitting
  above Jetson and below a datacenter GPU.
- **Compute / memory**: GB10 Grace-Blackwell Superchip — Blackwell GPU (5th-gen Tensor
  Cores) + 20-core Arm CPU (10× Cortex-X925 + 10× Cortex-A725), NVLink-C2C coherent
  CPU↔GPU link. **128GB coherent unified LPDDR5x memory**. Headline **up to 1,000 AI
  TOPS / 1 PFLOP of FP4** (sparse FP4 — NVIDIA's chosen headline unit; not directly
  comparable to the INT8 TOPS above). NVIDIA states models **up to ~200B params** for
  inference (and ~70B-class for fine-tune / two units linked for ~405B).
- **Interface / form factor**: Self-contained desktop unit (not a module). Up to 4TB
  NVMe, ConnectX-7 smart NIC (lets two Spark units be linked), Wi-Fi 7. Runs NVIDIA's
  DGX OS (Ubuntu-based) with the full CUDA stack.
- **Power**: Desktop appliance, wall-powered (~240 W class — treat as **unconfirmed**
  exact figure; NVIDIA positions it as a standard desk device on a normal outlet).
- **Fit / notes**: **Unified 128GB** is the standout — it's what lets a single node hold
  a large model without multi-GPU sharding, directly serving the "heavier local
  inference" slice of our placement direction. FP4 headline TOPS is marketing-favorable;
  for apples-to-apples with Jetson/Hailo use real model throughput, not the 1-PFLOP
  number. **Pricing moved post-launch**: Founders Edition launched **$3,999** (Oct 2025)
  and rose to **~$4,699** on the NVIDIA marketplace amid LPDDR5x shortages — confirm
  current price before quoting. Sold via NVIDIA, PNY, Micro Center, Amazon, Best Buy,
  etc. Source:
  <https://marketplace.nvidia.com/en-us/developer/dgx-spark/> ,
  <https://www.nvidia.com/en-us/products/workstations/dgx-spark/>

---

## Google Coral (Edge TPU) — USB Accelerator + Dev Board

Google's Edge TPU ASIC. Mature, cheap, narrow. INT8-only by design.

- **Role for us**: Pure **vision-inference offload accelerator**, bolted onto a host
  (Pi/Seed/x86). NOT an LLM device. Good for a sensor/camera cog that needs fast,
  low-power object detection/classification without a GPU. In the mesh it's an
  accelerator attached to a node, not a node itself (USB stick) — except the Dev Board,
  which is a small SBC.
- **Compute / memory**: Edge TPU — **4 TOPS peak (INT8)**, **~2 TOPS/W**. Runs
  TensorFlow Lite models **compiled for Edge TPU** (8-bit quantized only). The USB
  stick itself has only a tiny Cortex-M0+ (32 MHz) housekeeping MCU + 16KB flash / 2KB
  RAM — all real memory/compute is on the host. Dev Board adds an NXP i.MX 8M SoC
  (quad Cortex-A53) + 1–4GB LPDDR4 + the same Edge TPU.
- **Interface / form factor**: USB Accelerator = USB-C 3.0 dongle (~65×30×8 mm). Also
  sold as M.2/mini-PCIe modules and the standalone Dev Board SBC.
- **Power**: Very low — USB-bus-powered; ~2 TOPS/W means roughly a couple of watts
  under load.
- **Fit / notes**: **INT8-only** is the hard limit — no transformer/LLM story, and the
  toolchain (Edge TPU compiler, aging TF-Lite path) is comparatively stale. Strength is
  $/perf/W for classic CNN vision. Treat as a cheap vision cog accelerator, not a
  general inference tier. Source:
  <https://coral.ai/products/accelerator/> ,
  datasheet: <https://www.coral.ai/static/files/Coral-USB-Accelerator-datasheet.pdf>

---

## Hailo-8 / Hailo-8L (M.2 / mPCIe AI accelerators — Raspberry Pi AI Kit / AI HAT+)

Dedicated deep-learning inference ASICs, widely known via the **Raspberry Pi AI Kit**
(Hailo-8L) and **AI HAT+** (8L or 8). Strong perf/W vision accelerators over PCIe.

- **Role for us**: **Vision-inference offload for a Pi/Seed-class node** (and x86 via
  M.2/mPCIe). Like Coral but markedly higher TOPS and a more modern toolchain (Hailo
  Dataflow Compiler, ONNX ingest). Still a vision/CNN accelerator, not an LLM host
  (Hailo has separate "Hailo-10" silicon aimed at genAI — out of this entry's scope;
  mark that as a separate line to track). In the mesh: the default AI accelerator
  attached to our Pi/Seed vision cogs.
- **Compute / memory**:
  - **Hailo-8L** — **13 TOPS (INT8)**.
  - **Hailo-8** — **26 TOPS (INT8)**.
  - No large on-board DRAM; works against host memory + its own dataflow fabric.
- **Interface / form factor**: M.2 (2242/2230) key M module, or mini-PCIe. On a Pi 5 it
  rides the **PCIe Gen3** lane via the M.2 HAT+ / AI HAT+. AI Kit = M.2 HAT+ + Hailo-8L
  (M.2 2242); AI HAT+ sold in 13-TOPS (8L) and 26-TOPS (8) variants.
- **Power**: Low single-digit watts typical (fits inside the Pi 5 power/thermal budget
  with the active cooler); exact draw workload-dependent.
- **Fit / notes**: Best **vision TOPS-per-dollar-per-watt** in the Pi ecosystem, and the
  26-TOPS Hailo-8 doubles the 8L for multi-stream / larger nets. PCIe (not USB) means
  lower latency / higher bandwidth than Coral. Still INT8 quantized vision — not a path
  to local transformers. Track Hailo-10H separately if genAI-on-accelerator becomes a
  requirement. Source:
  <https://www.raspberrypi.com/products/ai-hat/> ,
  product brief: <https://datasheets.raspberrypi.com/ai-hat-plus/raspberry-pi-ai-hat-plus-product-brief.pdf> ,
  Hailo: <https://hailo.ai/products/ai-accelerators/hailo-8-ai-accelerator/>

---

## Radxa Rock 5B / 5B+ (RK3588) and peers (Orion O6, Khadas VIM, BeagleY-AI)

Rockchip **RK3588**-class SBCs — a capable Arm application CPU with a modest built-in
NPU. The "general-purpose edge node that also has some AI" category.

- **Role for us**: A **self-hosting mesh node** in the Pi-plus class — runs a full Linux
  OS, hosts cogs, and has a small NPU for light vision/quantized inference. More of a
  compute/placement node than a dedicated accelerator. Its NPU is weak next to Jetson/
  Hailo, so heavy inference would still be offloaded or placed elsewhere.
- **Compute / memory**:
  - **Rock 5B / 5B+** — RK3588 octa-core (4× Cortex-A76 + 4× Cortex-A55), Mali-G610
    GPU, **6 TOPS NPU (INT8)**. 5B up to 32GB LPDDR4x; **5B+ up to 32GB LPDDR5**
    (4/8/16/24/32GB options). M.2 NVMe (PCIe 3.0), eMMC, microSD, 8K HDMI.
  - **Radxa Orion O6** — Nano-ITX, **CIX P1** SoC (8× L720 + 4× A520), Immortalis
    G720-MC10 GPU, **~30 TOPS NPU**, LPDDR5 **up to 64GB**. A notably stronger NPU +
    much larger RAM ceiling than the RK3588 boards — the more interesting "big Arm node"
    candidate; still newer/less proven, verify software maturity.
- **Interface / form factor**: Full SBC (Rock 5B ~Pi-sized; Orion O6 Nano-ITX). Onboard
  GbE/2.5GbE, USB, M.2, HDMI — no host required.
- **Power**: ~5–15 W class typical for RK3588 boards under load (board + workload
  dependent); Orion O6 higher given the bigger SoC.
- **Fit / notes**: The RK3588 6-TOPS NPU and fragmented NPU toolchain (RKNN) mean it's a
  **node that can host, not an inference powerhouse** — good for placement/orchestration
  and light local vision. **Orion O6's 30 TOPS + 64GB** is the standout if we want a
  bigger non-NVIDIA Arm node, at some software-maturity risk.
  - **Peers (1-liners):** **Khadas VIM3/VIM4** — Amlogic SoC SBCs with a ~5 TOPS NPU,
    similar "SBC + modest NPU" class. **BeagleY-AI** — TI AM67A (J722S) SBC, ~4 TOPS
    vision/AI accelerator, Pi-form-factor open-hardware option.
  Source: <https://wiki.radxa.com/Rock5/hardware/5b> ,
  Orion O6: <https://radxa.com/products/orion/o6/>

---

## Comparison table

TOPS are vendor-headline figures and **not directly comparable across precisions** —
NVIDIA DGX Spark is **FP4** (1,000 TOPS ≈ 1 PFLOP), Jetson is **sparse INT8**, Coral/
Hailo/Radxa are **INT8**. Use real model throughput for final decisions.

| Device | AI compute (TOPS) | Memory | Power | Best-for | Our fit |
|---|---|---|---|---|---|
| Jetson Orin Nano 8GB (Super) | ~67 (INT8) | 8GB LPDDR5 | 7–25 W | Small/quantized LLM + vision | Accelerator node; CUDA entry tier |
| Jetson Orin NX 16GB | ~100 (157 "Super") INT8 | 16GB LPDDR5 | 10–25 W | Multi-stream vision, 7B-class quantized | Solid mid mesh node |
| Jetson AGX Orin 64GB | up to 275 (INT8) | 64GB LPDDR5 | 15–60 W | Larger local models, robotics | Top Jetson node |
| **DGX Spark (GB10)** | up to 1,000 (FP4) | **128GB unified** LPDDR5x | ~desktop (≈240 W, unconfirmed) | **Large local LLM infer/fine-tune (~200B)** | Heavy-inference tier of the mesh |
| Coral USB Accelerator | 4 (INT8 only) | host memory (tiny on-stick) | ~2 W (2 TOPS/W) | Cheap CNN vision offload | Vision cog accelerator; no LLM |
| Hailo-8L (Pi AI Kit) | 13 (INT8) | host memory | low single-W | Vision on Pi 5 (PCIe) | Default Pi/Seed vision accel |
| Hailo-8 (AI HAT+ 26T) | 26 (INT8) | host memory | low single-W | Multi-stream / larger vision nets | Higher-end Pi vision accel |
| Radxa Rock 5B+ (RK3588) | 6 (INT8 NPU) | up to 32GB LPDDR5 | ~5–15 W | General Arm node + light vision | Host/placement node, weak NPU |
| Radxa Orion O6 (CIX P1) | ~30 (NPU) | up to 64GB LPDDR5 | higher | Big non-NVIDIA Arm node | Candidate large Arm node (newer) |
| Khadas VIM3/4 (peer) | ~5 (NPU) | up to 8GB | low | SBC + modest NPU | Alt host node |
| BeagleY-AI (peer) | ~4 (TI AI) | 4GB | low | Open-HW Pi-form node | Alt host node |

### Takeaways for WeftOS placement

- **Accelerators vs nodes**: Coral and Hailo are *attached* INT8 vision accelerators —
  WeftOS should model them as a capability of their host node, not as independent nodes.
- **CUDA is the ecosystem moat**: Jetson + DGX Spark share the CUDA/TensorRT stack; a
  model/placement decision favoring them keeps one inference toolchain across the tier.
- **Unified memory is the LLM lever**: DGX Spark's 128GB unified memory (and Orion O6's
  64GB) is what decides whether a *large* model can be placed on a node at all — this is
  the single most important axis for the "heavier local inference" slice of our mesh.
- **Honesty flags**: DGX Spark specs/pricing are post-Oct-2025 and still moving (memory
  shortage price bump); Jetson TOPS were re-labelled under the 2024 "Super" campaign;
  Orion O6 is newer with less-proven software. Pin exact SKU + date when quoting.
