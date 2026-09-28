# PZSDR P047 RFSoC + fiber-entwined towline — sensor fit

**Date:** 2026-09-25  
**Status:** Hardware research note (not an ADR; not Plane-filed)  
**Product page:** [Crowd Supply — PZSDR P047](https://www.crowdsupply.com/puzhi/pzsdr-p047-rf-adc-and-rf-dac)  
**Related:** `.planning/sonobuoy/` (buoy fleet, OWTT/JANUS, Class B copper tethers), `crates/clawft-sonobuoy-ranging`, ADR-087 (K-STEMIT dual-branch, Proposed), `docs/research/ruv-parallels-and-gaps.md` (RF-Gaussian / WorldGraph)

This class of board is **excellent for the shore / USV head** of a fiber towed array, and for RF/MIMO sensing that Graph Views can bind. It is **not** a replacement for the ESP32 + JFET hydrophone path on the drifting buoys.

---

## 1. What P047 actually is

AMD **Zynq UltraScale+ RFSoC XCZU47DR** on a 175×180 mm board (~$8,749, pre-order, listed ship **2026-11-06**). Direct-RF converters on-die — no separate AD9361.

| Spec | P047 | Why it matters here |
|------|------|---------------------|
| RX | 8 ch, **5 GSPS**, 14-bit | Optical-hydrophone interrogator / DAS demod, RF MIMO |
| TX | 8 ch, **9.85 GSPS** | Active ping / HF / radar / optical modulator drive |
| Band | **1 MHz – 6 GHz** | RF world-sensing; **not** 10 Hz–100 kHz sonar baseband |
| Clock | **±5 ppb** | Two orders better than typical SDR ppm; ranging-adjacent |
| I/O | GbE, USB 3, **QSFP28 100G**, MiniDP | Raw IQ off-board into `weftos-sensor-pipeline` |
| Sync | Shared ref clock + SYSREF + trigger (demo: two boards, ILA, no observed drift) | Coherent aperture > 8 ch |
| Env | −40…+85 °C, enclosed | USV deck / lab rack, not a buoy payload |
| SW | PetaLinux, Vitis, **PYNQ**, RFSoC-PYNQ book examples | Fast path to a capture daemon |

Open: schematic + firmware promised; user manual and DXF already public. Oscillator precision is the sleeper spec: ±5 ppb vs LimeSDR ±1–4 ppm, HackRF ±20 ppm.

---

## 2. Where it fits WeftOS (and where it does not)

### Do not use it as

- **Per-buoy ADC.** Architecture already says WeftOS does not run on the buoy (embassy-rs / ESP32-S2 TX + S3 RX). $8.7k + 12 V 3 A + enclosure is a host, not a $15 Class B sidecar.
- **Direct hydrophone sampler.** RF-ADC floor is **1 MHz**. JANUS (~9–14 kHz), Wenz ambient, and OWTT chirps live well below that. The existing MCP6022 / ESP32 ADC path stays for piezo elements.
- **JANUS modem.** Underwater acoustic PHY is kHz, not GHz.

### Do use it as

| Role | WeftOS seam |
|------|-------------|
| **Fiber-array interrogator** | High-speed ADC + FPGA demod for FBG / DAS / Michelson mandrel hydrophones. This is the classic dry-end box. |
| **Coherent RF sensor head** | 8T8R MIMO / radar / HF / CSI-class energy → Graph Views F3 bind; occupancy as **features**, not BVH geometry (same rule as rUv RfGaussian). |
| **Fleet clock / trigger** | ±5 ppb ref to discipline acoustic nodes (GPS antenna is in the kit). Complements CSAC-on-buoy in `RANGING.md`; does not replace it for a drifting field. |
| **Shore ingest** | QSFP28 → `weftos-sensor-pipeline` / `mesh.sensor.v1` events → LeWM optional, ECC authority (ADR-090). |
| **Multi-board aperture** | Phased array / distributed RF when 8 ch is not enough. |

K-STEMIT (ADR-087) still wants **spatial graph + temporal branch** over array geometry. P047 is the **sample clock and channel count** for a linear towed array; the GNN still runs on the host.

---

## 3. Fiber-entwined towline

“Fiber entwined in the towline” is the right wet-end architecture for a **towed** array. It is a different product from the **drifting buoy field**. Keep both: buoys for sparse volume, tow for a long coherent aperture.

### What “entwined” should mean

Not glass as the sole tensile member. Production arrays **braid / weave**:

1. **Aramid strength** (Kevlar / Vectran), **contrahelical** so the hose does not corkscrew under tow (geometry is the array).
2. **Optical fibers** in the same braid or a telemetry harness (Litton-style fabric: fibers + strength yarns + cut marks for splices).
3. Optional **thin copper** only if piezo preamps still need volts. All-optical wet ends (TB-33 class) drop copper through the VIM.

Navy TB-33 / AOTA line: low-reflectivity **fiber Bragg gratings** form Fabry–Perot hydrophones; hundreds of channels on **four fibers** via time + wavelength mux. Mandrels + foam + extruded hose. Interferometric mandrel coils (Michelson, ~19 nm/Pa in tank demos) are the lab-scale cousin.

Recent / commercial analogs:

- Large DAS towed arrays (192 units, lake trial; ~−127 dB re rad/µPa, 20–1000 Hz).
- Optics11 **OptiArray** (2026): 100 m tow + 40 m / 64-element optical section, autonomous USV launch/recovery to Sea State 5.
- Thin FOTA demos: ~25 mm OD, nested HF/MF/LF, **no electrical power in the acoustic section**.

### Why fiber vs the Class B copper pigtail

| | Class B (current) | Fiber-entwined tow |
|--|-------------------|--------------------|
| Length | 1–3 m under a buoy | 10 m–km class |
| Channels | 1 hydrophone per tether | tens–hundreds on few fibers |
| EMI / galvanic | Copper in seawater | Optical wet end, no magnetic armour |
| Geometry | Known short baseline | Needs heading/T/P along hose; torque-balance or the line twists and beamforming dies |
| Host | Parent ESP32 | RFSoC / uDAS interrogator on USV |
| Cost / unit | ~$15–20 | Interrogator dominates; hose is a program |

Keep Class B for vertical TDoA on a buoy. Use fiber tow for **horizontal aperture**.

### Mechanical constraints (do not skip)

- **Neutral buoyancy** on the acoustic section; vibration-isolation modules (VIM) between tow cable and sensors.
- **Kevlar, not steel**, if magnetic signature or weight matters; steel armour also self-rotates.
- **FORJ** (fiber-optic rotary joint) on the winch if you spool under way.
- Non-acoustic sensors (heading, pressure, temperature) in the hose — required for shape estimation and SSP. Same job as buoy GPS + thermistor, different kinematics.
- Flow-noise and acceleration response: damping modules; this is why thin-line FOTA papers spend pages on anti-acceleration.

---

## 4. Recommended stack (compose, don’t merge)

```
USV / shore rack
  PZSDR P047 (RFSoC)     — optical demod + RF MIMO + 5 ppb clock
       │ QSFP28 / GbE
       ▼
  weftos-sensor-pipeline — mesh.sensor.v1 events, chain
       │
       ├─ Graph Views F3  (bind array + RF)
       ├─ clawft-sonobuoy-ranging  (OWTT D-matrix still for the *buoy* field)
       └─ BVH promote F9 only for objects with AABB + evidence

towline (Kevlar contrahelix + SM fiber, optional copper)
  VIM → acoustic section (FBG / mandrel / DAS) + heading/T/P

separate: Class A/B buoy fleet (ESP32, piezo, JANUS, copper 1–3 m)
```

**Clock story:** P047 GPS-disciplined ±5 ppb on the USV; buoys keep CSAC / GNSS-disciplined OWTT. Do not pretend one RFSoC timestamps a drifting field without an acoustic or optical time transfer.

**RF story:** P047’s 1 MHz–6 GHz path is the WeftOS analog of rUv’s CSI/UWB **coherent** head. Occupancy / channel residuals are View **features**. Geometry SoT stays BVH. **Protocol-level** 802.11/BLE (promiscuous, drone RID, CSI motion) is a different node: [AntiHunter](https://github.com/lukeswitz/AntiHunter) — see `docs/research/antihunter-weftos-crosswalk.md`. Do not put that firmware on the hydrophone S3.

---

## 5. Risks

- Campaign is **4 backers / $39k**. Real silicon (RFSoC) but thin field community vs Ettus / Lime. Treat as a lab instrument, not a fleet SKU.
- 0 dBm TX: fine for lab/radar IF; not a sonar projector.
- 5 GSPS × 8 × 14-bit is a firehose; without FPGA DDC you will not land it in Rust. Plan DDC/NCO in PL, packets out QSFP.
- Fiber tow is a **winch + hose + interrogator** program, not a weekend PVC build. First physical step is a 5–15 element tank array, not a km streamer.
- Export / ITAR-ish optics + RFSoC: check before any “array at sea” narrative.

---

## 6. Next (not filed)

1. **Lab:** one P047 (or cheaper RFSoC eval) as PYNQ capture → `weftos-sensor-pipeline` IQ/event adapter. Prove clock + 8 ch + GbE before fiber.
2. **Tank:** 5-element fiber mandrel or off-the-shelf DAS patch on a Kevlar-reinforced fiber line; interrogate from the same host.
3. **Doctrine:** add a `sonobuoy-tow` profile next to `sonobuoy-tactical` / `sonobuoy-pam` — linear coherent aperture, USV-hosted, fiber wet end.
4. **Do not** put RFSoC on the buoy or replace oil-sidecar piezos for Phase 1.

Sources: Crowd Supply P047 page + 2026-08-26 multi-board sync update; TB-33 / AOTA (NRL); Optics11 OptiArray DAGA 2026; Acta Optica Sinica 2024 DAS tow; Opt. Express 2023 CTWT tow cable; `.planning/sonobuoy/build/{architecture,build-tethered-subsurface,build-hydrophone}.md`.
