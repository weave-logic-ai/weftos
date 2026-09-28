# Array telemetry papers — copper tow + 3-buoy grounding

**Date:** 2026-09-25  
**Status:** Session research (not an ADR)  
**Plan under test:** [`docs/research/sonobuoy-min-test-and-copper-tow.md`](../sonobuoy-min-test-and-copper-tow.md)  
**Companions:** [`README.md`](./README.md) · [`pzsdr-p047-and-fiber-towline.md`](../pzsdr-p047-and-fiber-towline.md) · `.planning/sonobuoy/RANGING.md`

**Constraint:** no Wi‑Fi. CAT5/7 braid carries power, packets, and a common clock. Analog hydrophones **or** addressable pods. USB‑CAN gateway into WeftOS. First tests: 3 Class A buoys at 15–30 m (lake) **or** 4-phone copper tow at ~0.4 m spacing.

Each source is mapped **steal / already matches / do not copy**. The copper-only plan is orthodox array engineering. WeftOS-specific bits are ESP32‑S3 + TWAI, USB‑CAN, substrate identity, and `clawft-sonobuoy-ranging` treating taut `L_cable` as exact `D_ij`.

---

## 1. What we are grounding

| WeftOS choice | Why it is in the plan |
|---------------|------------------------|
| One waterproof CAT5/7 braid | Four pairs: CAN, 12–24 V, RS‑422 PPS, analog/spare. CAT7 S/FTP, braid grounded **dry-end only**. |
| Common clock on a **wire** | TDoA budget ~10 µs. Do not timestamp CAN SOF / frames (arbitration jitter). |
| Power on the same hose | Buck + TVS + polyfuse per node. Not PoE injectors for the bus. |
| USB‑CAN (or USB‑ETH / USB‑485) gateway | WeftOS never talks to an AP. Radio off (it also jittered the hydrophone ADC). |
| Analog phones **or** addressable pods | v1 analog on brown; pods = S3 + SN65HVD230, address 1…N. |
| 3 Class A, 15–30 m lake | First geometry `D(t)` can use. Taut CAT7 = tape-measure baselines. |
| 4-phone tow, 0.40–0.45 m | λ/2 at 1.8 kHz. Two phones is only an interferometer. Separate Kevlar/Dyneema strain. |

Fiber / RFSoC / CSAC stay later ([`pzsdr-p047-and-fiber-towline.md`](../pzsdr-p047-and-fiber-towline.md)).

---

## 2. US4464739 — leading-edge simultaneous sample (1984)

**Cite:** Moorcroft, A.L. (U.S. Navy). *Sampled towed array telemetry.* U.S. Patent 4,464,739, 7 Aug 1984. [Google Patents](https://patents.google.com/patent/US4464739A/en).

Navy STA (Sampled Towed Array). Identical modules along a line. **Two small-diameter coax** only: a **control** cable and a **data** cable. Power is the potential difference between the two shields. A clock pulse’s **leading edge travels the full array without delay** and **sample-and-holds every hydrophone at once**. The **trailing edge is delayed at each module** so PAM pedestals go out in sequence. Receiver counts pedestal leading edges for channel ID. Module delay passing the control rise is **&lt; 1 µs**. Patent explicitly allows **twisted pair instead of coax**, plus a **strength member** in an extruded jacket. Simultaneous hold is so beamformers do not need per-channel delay corrections.

| | |
|--|--|
| **Steal** | Dedicated sample/strobe pair. Leading-edge **common hold**, telemetry on a different edge or a different pair. Identical interchangeable pods. Measure (do not guess) hose delay; 30 m of CAT5 is ~150 ns, still calibrate. |
| **Already matches** | Green pair = PPS/strobe (RS‑422). Orange = CAN events. Analog v1 on brown is the cheap cousin of “track until hold.” |
| **Do not copy** | PAM current-pedestal analog telemetry. Two-coax-only (we have four pairs; spend them). Unique-per-channel address modules. High-speed sample clock as the **data** PHY. |

For the 4-phone tow, a single RS‑422 edge that latches all four ADCs (or one USB-audio interface’s common clock) **is** this patent. For 3 buoys, the same edge is the TDoA clock; detections publish `{t_pps, dt_us, peak}` on CAN.

---

## 3. US5583824 — twisted-pair digital bus + tension members (1996)

**Cite:** Fletcher, D.D. (Whitehall Corp.). *Telemetry data transmission circuit having selectable clock source.* U.S. Patent 5,583,824, 10 Dec 1996. [Google Patents](https://patents.google.com/patent/US5583824A/en).

Seismic streamer, miles long, modular waterproof sections. **Digital bus is twisted-pair**, partitioned into auxiliary / **power** / downlink / **timing** / acoustic+nonacoustic **data**. **Tensioning wires** take the tow load; electrical cables, jacket, and foam do not. Open-cell foam + fill fluid for neutral buoyancy; “birds” for depth. Hydrophone **spacing is the array** — phase dies if elements slide.

Clock: **1.024 MHz master** generated in the **aftmost** module, forwarded; each combiner/repeater (CRU) **slaves via XPLL**. Frame-sync + end-of-data ride a timing bus. Each module **adds local samples and retransmits**. If the aft clock/data/frame-sync dies, a timeout flips that module to its **local XO** and it becomes master for everything forward (foreshortened streamer). Power bus is **hundreds of volts**; a zener drops **~6.8 V per module**. Half-clock-cycle delay per hop for frame handoff.

| | |
|--|--|
| **Steal** | **Kevlar/Dyneema separate from copper** (already in the min-test note). Partition **timing ≠ data ≠ power**. Pods that add-and-forward are the towline variant of our CAN bus. Local-clock fallback is nice-to-have on a 15–30 m braid, not v1. Non-acoustic sensors (heading/T/P/depth) belong on the hose if you ever beamform a slack tow. |
| **Already matches** | CAT5 pair map: orange CAN, blue VIN/GND, green PPS, brown spare. 12–24 V + per-node buck is the low-voltage version of “power bus + drop per module.” |
| **Do not copy** | 9-wide parallel data bus. Hundreds of volts in a lake test. Miles of oil-filled streamer, birds, paravanes. Xilinx CRU + 1.024 MHz telemetry clock as PPS. Half-cycle-per-hop as a **timebase** (that is frame-handoff, not TDoA). |

US5583824 is the digital-bus ancestor. Our CAN + PPS split is the 2026 cheap reading of “timing bus 304 vs data bus 305.”

---

## 4. Large coherent array — Cat6 + independent frame-sync + PTP/GPS ~1 µs

**Cite (IEEE):** Schinault, M., Zhu, C., Makris, N.C., Ratilal, P. *Development of a large-aperture 160-element coherent hydrophone array system for instantaneous wide area ocean acoustic sensing.* OCEANS 2022 Hampton Roads. IEEE. doi:[10.1109/OCEANS47191.2022.9977226](https://doi.org/10.1109/OCEANS47191.2022.9977226). Northeastern University + MIT (Makris).

**Cite (thesis):** Schinault, M. *A Large-Aperture 160-Element Coherent Hydrophone Array for Real-Time Wide-Area Ocean Acoustic Sensing.* PhD thesis, ECE, Northeastern University, Apr 2025 (advisor Purnima Ratilal Makris). §2.17 “Array Telemetry and Timing.”

160 elements, **192 m** nested aperture (four 64-phone sub-apertures, λ/2 at 250 / 500 / 1000 / 2000 Hz). Phones 10 Hz–50 kHz with integrated preamps; 24-bit, 32-ch ADCs up to 100 kHz/ch. **UDP Ethernet on Cat 6**, **independent frame-synchronization** to the ADCs. PTP timestamps on every Ethernet frame; GPS time on the shipboard acquisition NIC, **within ~1 µs of UTC**. Frame-sync pulse error **down to nanoseconds** (beamforming positional error between adjacent ADCs negligible). Two GbE backbones (acoustic vs non-acoustic). Converted to **single-mode fiber** for **600 m** faired tow. Power + Ethernet copper in each section (eight 16 AWG + nine 22 AWG). Forward/aft **NAS**: depth, heading, pitch, roll, temperature. Modular Cat 8.2 interconnects, oil-fill, pressure-tolerant switches. Built **without** a Navy hose plant.

This is the academic analog of “Cat6 in the wet end, clock not in the packet.” It is **not** WHOI’s OWTT-iUSBL AUV (Rypkema 2021 uses CSAC + pyramid array — different problem). WHOI shows up here as the MIT/WHOI coherent-array research line (Makris), not as the copper PHY.

| | |
|--|--|
| **Steal** | **Independent frame-sync + PTP/GPS as belt-and-suspenders.** NAS (heading/T/P) on the hose if the tow is long enough to curve. Modular interconnects / oil-fill later. 1 µs UTC is **finer** than our 10 µs TDoA budget — proves copper Ethernet + a sync channel is enough. |
| **Already matches** | Cat6/7 as the wet-end backbone. Power on copper in the same hose. USB/GbE into a shore laptop is our gateway, scaled down. λ/2 nesting is why 4 phones at 0.40–0.45 m exist. |
| **Do not copy** | 160 channels, 192 m, 600 m fiber tow, 24-bit 100 kHz, pressure-tolerant GbE switches, 300-class VDC distribution. PTP **instead of** a PPS pair on 15–30 m (PTP is for when you already have Ethernet PHYs and switches; ESP32‑S3 has neither). Star Ethernet homeruns for the 3-buoy bus (use CAN). |

For v1: **frame-sync = green RS‑422**; **PTP/GPS = one GPS PPS on the master**, not IEEE 1588 on W5500. If a later pod generation has a real Ethernet MAC, then copy Schinault’s “UDP + independent sync + PTP timestamp,” not his channel count.

---

## 5. iPEN / iPON — common clock coax + TDM sensor net + power (2011)

**Cite:** Jamieson, J., Murray, J., Johnson, G., Caplan, S.I. (3 Phoenix, Inc.; Navy contract N00244-04-P-1737). *Inverted passive optical network / inverted passive electrical network (iPON/iPEN) based data fusion and synchronization system.* U.S. Patent 8,018,954, 13 Sep 2011. [Google Patents](https://patents.google.com/patent/US8018954B2/en). Also CA2599365C.

**Inverted** PON: bulk data flows **upstream** from sensors; downlink is timing + control. iPEN is the copper/coax reading. Dual-coax backbone for towed arrays (~40 Mbit/s, ~5 km, 8–12 MHz QPSK/QAM, DOCSIS-like, no in-line amps). **Constant-current** power on the backbone; local groups get **constant voltage** on Control + Data coaxes.

Local **sensor net**: short-reach, low-power, **common clock coax** + **TDM data coax**. Acoustic Sensor Node (ASN) is a 2-channel piezo front-end. Failsafe receivers **tap** the clock coax; failsafe tri-state drivers TDM onto the data coax through a bi-impedance network so a dead node does not kill the bus. Sample-frame + synchronous clock come from the Network Gateway. ~36 gateways, ASNs in groups of ≥3 (6+ channels). Superframe alignment of ADC sample pulses; claimed ~10 ns class at Gb/s iPON rates. Engineering Sensor Nodes for non-acoustic suites.

| | |
|--|--|
| **Steal** | **Clock pair and data pair are different wires.** Fail-safe tap (one dead ESP must not short CAN_H/CAN_L or the PPS pair). Gateway generates sample/frame; pods lock to it. Power + data + clock in one hose. TDM/slot the uplink so PCM cannot hog — same rule as “do not put 16 kHz PCM from three buoys on 125 kbit CAN.” |
| **Already matches** | Dual-coax “control + data” ≈ our green PPS + orange CAN. Addressable pods along the hose. Shore/ship receiver is the Network Controller analog of USB‑CAN. |
| **Do not copy** | DOCSIS QAM at 8–12 MHz, dual-coax RF circulators, 36 gateways, 2.5 Gbit iPON optics, constant-current 300 V backbone. Coax as the PHY (we already bought CAT7). Full iPON TC-layer / T-CONT machinery. |

iPEN is the closest **architecture** paper to “common clock on a wire + TDM sensor net + power.” Scale it to three nodes and CAT7.

---

## 6. US9501926 — two-wire sonar telemetry (power + data, same pair)

**Cite:** Meninno, J.T., Obara, M.J. (U.S. Navy). *Two wire sonar telemetry.* U.S. Patent 9,501,926, 22 Nov 2016. [Google Patents](https://patents.google.com/patent/US9501926B1/en). Expired-fee 2020.

**One wire** carries DC + data (bias tee: inductor for power, capacitor for AC). **Second wire is ground.** Shared bus, half-duplex **time-slotted** TDM. Header = 16-bit sync + 16-bit frame counter + 16-bit CRC + 8-bit status. Frame period = 1 / sample rate.

Worked examples: (1) 20 nodes × 2 ch, 1 kHz 16-bit → 852 kbit/s, λ/2 ≈ 6 ft, design freq ~400 Hz. (2) 250 nodes × 2 ch, 250 Hz 12-bit → 1.96 Mbit/s, λ/2 ≈ 12 ft, ~100 Hz. They **do not** provide a dedicated sample-clock pair. Patent itself flags the shared-bus penalty: nodes in parallel → parasitic C → high-frequency degradation. Tow-cable R in the example is ~17 Ω.

| | |
|--|--|
| **Steal** | Bias-tee trick **if** a pair must do two jobs (e.g. brown analog + phantom power). Frame counter + CRC on event frames. Guard-band between node slots. Throughput arithmetic before you promise PCM. |
| **Already matches** | Shared bus of nodes on one hose. Bidirectional command/data. Power always on, independent of data slots. |
| **Do not copy** | **Power + data on the same pair as the only PHY.** We have **four** pairs; spending one on PPS is cheaper than a bias tee and a 10 µs TDoA argument. Half-duplex TDM as the clock. 500-channel / 2 Mbit analog of a lake triangle. |

**Contrast with CAT5:** two-wire is a reliability/weight play for a Navy streamer that is already too fat. CAT5’s extra pairs exist so we **do not** multiplex time onto the data PHY. That is the whole point of §7 in the min-test note.

---

## 7. Shape — Wikipedia / GPS / strain; taut vs slack

**Cite:** [Towed array sonar — Wikipedia](https://en.wikipedia.org/wiki/Towed_array_sonar) (accessed 2026-09-25). Beamforming needs **known relative positions**. That is free only when the cable is a **straight line**, or when a **self-sensing** system (strain gauges), **GPS**, or other embedded reporters correct for **curvature**. Turns and speed changes disturb the array; modern systems measure element-to-element and correct in the beamformer. Military arrays are often ballasted to **sink**; seismic streamers are **neutrally buoyant** ~10 m. First few hundred metres aft of the propeller are usually empty (flow / cavitation). Drag ∝ v²; a minimum speed may be required so a sinking array does not hit the bottom.

**Cite (GPS-only shape):** Gerstoft, P., Hodgkiss, W.S., Kuperman, W.A., Song, H., Siderius, M. *Adaptive beamforming of a towed array during a turn.* IEEE J. Oceanic Eng. **28**(1), 44–54 (2003). doi:[10.1109/JOE.2002.808211](https://doi.org/10.1109/JOE.2002.808211). Ship GPS + **water-pulley** delay + **parabola** fit; no in-array instrumentation. Works for a slow turn when the array follows the ship’s track.

**Cite (strain-to-shape):** Navy FBG curvature patents (e.g. Bragg pairs on opposite sides of the hose → Frenet integration). Fiber later, not v1.

| | |
|--|--|
| **Steal** | Treat **shape as a sensor**, not a hope. For the **4-phone, ~1.6–3.2 m** acoustic section, “assume straight + heading of the kayak/USV” is enough **if** copper is not the strain member. For a longer tow, add heading/T/P or GPS-pulley. Wikipedia’s “straight or sense curvature” is the doctrine. |
| **Already matches** | Min-test §7.2: **wired time is general; wired position is 1-D along the cable or a taut truss.** Taut 3-buoy CAT7 triangle = tape-measure `D_ij = L_cable`. Slack floating backbone = time + power + bus **only**. One GPS on the master georeferences the figure. |
| **Do not copy** | Claim slack CAT7 locates a drifting field. km-class streamer birds / paravanes. Water-pulley GPS shape on a 10 m dock hose (overkill; just pull it taut). Strain-gauge FBG plant for four phones. |

**Taut vs slack, restated:**

| Geometry | Copper gives | Copper does not give |
|----------|----------------|----------------------|
| 4-phone tow, taut, known spacing | Element stations for beamform | USV (x, y) in the lake |
| 3 buoys, **taut** known-length CAT7 | Exact baselines; flip ambiguity with depth or a 4th point | Absolute lat/lon without one GPS |
| 3 buoys, **slack** braid | Time, power, CAN | A survey |
| Free-drifting, radio off, no taut links | Nothing geometric | Need GPS and/or acoustic TWTT |

---

## 8. JWCN 2013 — offshore linear array, sub-µs clock cascade, ~18 m nodes

**Cite:** Chen, J., Duan, F., Jiang, J., Li, Y., Hua, X. *Offshore towed hydrophone linear array: principle, application, and data acquisition results.* EURASIP Journal on Wireless Communications and Networking **2013**, 35 (2013). doi:[10.1186/1687-1499-2013-35](https://doi.org/10.1186/1687-1499-2013-35). Tianjin University.

**Follow-on (same group, more PHY detail):** CN106411418A (Duan et al., Tianjin Univ., 2016). *Accurate data acquisition clock synchronization method for hydrophone linear array.* [Google Patents](https://patents.google.com/patent/CN106411418A/en). Differential clock on **unshielded twisted pair**, RS‑422/485 format, **no Ethernet protocol**. Master–slave absolute error **nanoseconds**, **linear in pair length**.

Head-end **TCXO 16.384 MHz** (`f_H`), divide-by-N to a slow sync `f_L` (~**4 kHz** sample/output ticks in the 2013 paper). Nodes **cascaded ~18 m**. Each slave **PLL frequency-doubles** the received clock; the PLL comparator locks the node’s **data-output tick** (`f_1d`…`f_md`) to the received edge so **phase error of the output ticks** — not merely the local XO — is driven to zero. Local crystal is **only a VCO in the PLL**, not the sample timebase. Topology is **physical cascade, logical star** (wet-end interface is the centre). Seawater kills GPS at the phones, so the clock **must ride the hose**. They explicitly reject CSMA Ethernet as the array data PHY (fixed distances, continuous multi-day recording).

README delay model (same family):  
`t_ns = t_d + 2 t_c + l_n t_p + t_nm`  
(driver + 2× conversion + length × propagation + node). Store `delay_ns[node]`. Recal if you recut.

| | |
|--|--|
| **Steal** | **Clock on twisted pair, RS‑422, PLL-lock the capture/output edge, compensate `l_n t_p`.** 18 m node spacing is the same order as our 15–30 m lake triangle. Sub-µs (they claim ns-class absolute) is an order better than 10 µs TDoA. Separate sync channel from the data uplink. |
| **Already matches** | Green pair + MCPWM/GPIO capture. `delay_ns[node]` after a cal pulse. “Do not use Ethernet as the time PHY.” Cascade of identical nodes. |
| **Do not copy** | 16.384 MHz TCXO as a **sample clock** on 15–30 m (a PPS edge + local PLL/MCPWM is enough at 1.8 kHz acoustics). FPGA N-multiply recovery of a fast ADC clock for 4 phones. Multi-day seismic recording stack. UTP without the CAT7 braid (we want the foil). |

This is the paper that says our `delay_ns[node]` equation is not invented. 18 m cascade ≈ one side of the lake triangle.

---

## 9. Steal / match / don’t — one table

| Source | Steal | Already matches | Do not copy |
|--------|-------|-----------------|-------------|
| **US4464739** | Leading-edge common hold; telemetry on another edge/pair; identical pods | PPS strobe; analog hold on brown | PAM analog bus; two-coax-only |
| **US5583824** | Tension members ≠ copper; timing bus ≠ data bus; add-and-forward pods | Pair map; per-node power drop | kV power; 9-wide bus; birds / miles of oil |
| **Schinault 160-el (NU/MIT)** | Independent frame-sync; PTP/GPS ≤1 µs; NAS on long hose | Cat6/7 + copper power; λ/2 spacing | 160 ch, fiber 600 m, PTP-on-W5500, star Ethernet as the 3-buoy bus |
| **iPEN/iPON US8018954** | Clock coax ≠ data coax; failsafe taps; gateway-generated sample frame; TDM so PCM cannot hog | Dual-pair control+data; shore gateway | DOCSIS QAM, 36 NGs, 300 V constant-current, optics |
| **US9501926** | Bias tee if a pair must double; slot/CRC/frame-counter | Shared bus; power always on | **Power+data as the only pair**; TDM as the clock |
| **Wikipedia + Gerstoft** | Straight **or** sense curvature; GPS-pulley for long turns | Taut `L_cable` = `D_ij`; slack ≠ survey | Slack-CAT7 localization; FBG plant for 4 phones |
| **JWCN 2013 / CN106411418** | RS‑422 clock on UTP; PLL-lock output ticks; `delay_ns = f(length)` | Green pair; 15–30 m scale; no Ethernet-as-time | 16 MHz sample clock on the wire; seismic-length cascade |

---

## 10. What this does to the two v1 geometries

**3 Class A, 15–30 m lake.** Treat the braid as iPEN’s local sensor net + JWCN’s 18 m cascade, not as Schinault’s 192 m Ethernet aperture. One GPS PPS (or a docked master timer) fans down green. CAN carries `{pps_seq, dt_us, peak}`. If the triangle is **taut**, `D_ij = L_cable` into `clawft-sonobuoy-ranging`; if **slack**, copper is time+power only and ranging stays acoustic. Wi‑Fi stays off.

**4-phone copper tow, 0.40–0.45 m.** US4464739 is the whole story: one strobe, four simultaneous samples, sequential or USB-audio readout. Strain on Kevlar, not CAT6 (US5583824). At ~2 m acoustic length, assume straight; do not build a shape sonar. Addressable pods (S3 per phone) are optional v1.1 — analog on brown is enough. Independent frame-sync from Schinault is the same strobe.

**Neither geometry** should: put time in CAN; put power+data+clock on one pair (US9501926); use LoRa-on-copper as PPS; claim slack hose position; start at 100 m–5 km.

---

## Sources (accessed 2026-09-25)

1. US 4,464,739 (Moorcroft / Navy, 1984) — sampled towed array, leading-edge simultaneous hold.  
2. US 5,583,824 (Fletcher / Whitehall, 1996) — twisted-pair digital streamer, tension members, selectable master clock.  
3. Schinault et al., OCEANS 2022 Hampton Roads, IEEE, doi:10.1109/OCEANS47191.2022.9977226 — 160-el coherent array, Cat6 + frame-sync + PTP/GPS.  
4. Schinault, PhD thesis, Northeastern ECE, 2025, §2.17.  
5. US 8,018,954 (Jamieson et al. / 3 Phoenix, 2011) — iPON/iPEN, common clock coax + TDM + power.  
6. US 9,501,926 (Meninno & Obara / Navy, 2016) — two-wire power+data telemetry.  
7. Wikipedia, *Towed array sonar*.  
8. Gerstoft et al., IEEE JOE 28(1):44–54 (2003) — GPS water-pulley array shape.  
9. Chen et al., EURASIP JWCN 2013:35, doi:10.1186/1687-1499-2013-35 — offshore linear array, ~18 m, sub-µs clock cascade.  
10. CN106411418A (Duan / Tianjin Univ., 2016) — RS‑422 differential clock on UTP, ns-class, linear in length.  
11. In-tree: `docs/research/sonobuoy-min-test-and-copper-tow.md` §§5–7; `docs/research/pzsdr-p047-and-fiber-towline.md`.
