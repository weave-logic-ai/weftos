# Time sync papers — copper braid vs CAN vs acoustic

**Date:** 2026-09-25  
**Status:** Session research (not an ADR). Citations only; no invented papers.  
**Plan under test:** [`docs/research/sonobuoy-min-test-and-copper-tow.md`](../sonobuoy-min-test-and-copper-tow.md) §6–§7  
**Folder:** [`README.md`](./README.md)

TDoA budget in that plan: **~10 µs** (≈ 1.5 cm of sound at 1480 m/s). Nodes **on** the CAT5/7 braid get a dedicated **RS-422 PPS** pair; CAN/TWAI carries `{pps_seq, dt_us, peak}`, not the clock. Nodes **off** the braid still need acoustic two-way time (TSHL / OWTT). This note grounds those two sentences.

Paywalled items are cited from the public abstract + DOI. Full-text PDFs that are open are linked.

---

## 0. Budget the copper already gives

| Quantity | Number | Source |
|----------|--------|--------|
| Sound speed (fresh, ~10 °C) | ~1480 m/s | copper-tow §2 |
| 10 µs of sound | **1.48 cm** | 10e-6 × 1480 |
| CAT5 propagation delay | **5.30 ns/m**, VF 0.64 | TIA/EIA-568 table as summarized in [Cat 5 (Wikipedia)](https://en.wikipedia.org/wiki/Category_5_cable) |
| 30 m of CAT5 | **~159 ns** | 5.30 × 30 |
| 159 ns of sound | **0.24 mm** | negligible vs hydrophone size |
| Pair-to-pair delay skew | < 0.20 ns/m | same Cat 5 table |

Cable delay is a **bias**, not jitter. Measure it once (`delay_ns[node]`) and subtract. Recal if the hose is recut. The 10 µs budget is spent on **clock capture**, not on copper VF.

ESP32-S3 MCPWM capture latches the APB timer (typically 80 MHz → **12.5 ns**) on a GPIO edge. That is hardware capture. An Arduino `ISR` + `micros()` is not.

---

## 1. AUTOSAR Time Sync over CAN (CanTSyn) — why a CAN message is not a clock

**Spec (open PDFs):**

- AUTOSAR Classic Platform, *Specification of Time Synchronization over CAN* (CanTSyn), Document ID 674. Public copies: [R4.2.2 SWS](https://autosar.org/fileadmin/standards/R4.2.2/CP/AUTOSAR_SWS_TimeSyncOverCAN.pdf) and later [R25-11 SWS](https://www.autosar.org/fileadmin/standards/R25-11/CP/AUTOSAR_CP_SWS_TimeSyncOverCAN.pdf).
- AUTOSAR Foundation, *Time Synchronization over CAN Protocol* (PRS), Document ID 1089. [R25-11 PRS](https://www.autosar.org/fileadmin/standards/R25-11/FO/AUTOSAR_FO_PRS_TimeSyncOverCANProtocol.pdf) (the protocol was split out of the SWS).
- Requirements: [AUTOSAR_RS_TimeSync (R19-11)](https://www.autosar.org/fileadmin/standards/R19-11/FO/AUTOSAR_RS_TimeSync.pdf).

**What the protocol actually does.** CanTSyn is a master–slave reference-broadcast scheme forced to live in 8-byte CAN frames. One synchronization cycle is two frames on the **same CAN ID**:

1. **SYNC** (type 0x10 / 0x20): seconds portion of the master’s time, domain id, sequence counter. The master *requests* transmit; the bus decides *when*.
2. **FUP** (type 0x18 / 0x28): nanoseconds of the **actual egress offset** `ΔtSyncTxOffset` between the time stuffed into SYNC and the moment SYNC was put on the wire (`TSyncEgress`).

The FUP exists **because the master does not know the SYNC egress time until after CAN arbitration**. A single TIME frame whose payload is “now” is wrong by the entire wait for the bus. The PRS states this in so many words: the FUP carries the offset between `TTMGlobalTime` and `TSyncEgress`.

Hartwich (Bosch), *CAN frame time-stamping — supporting AUTOSAR time base synchronization*, iCC 2017, [PDF](https://can-cia.org/fileadmin/cia/documents/proceedings/2017_hartwich.pdf):

- Software CanTSyn timestamps SYNC **in the Tx-confirm / Rx-indicate ISR**. Accuracy “depends on the interrupt response times after the synchronization message.” `Tx_Stamp` and `Rx_Stamp` diverge by ISR latency jitter.
- Hardware capture (later CiA 603 Time Stamping Unit) is triggered by the **end of the same SYNC frame**. Nodes then see that event with a phase shift of **less than one CAN bit time** (2 µs at 500 kbit/s; 8 µs at 125 kbit/s) plus ACK delay — *if* the capture is hardware.
- CiA 603 stores 32-bit stamps from a free-running counter with steps between 1 ns and 1 µs. ESP32 TWAI does **not** implement this TSU.

**Arbitration is the reason SOF is not PPS.** Bitwise arbitration delays the *start* of a frame by zero to many full frame times of higher-priority traffic. Under load that is **milliseconds**, which is what copper-tow §7.4 forbids. Even an idle bus still has bit-stuffing jitter (up to 24 stuffed bits on a DLC-8 frame — tens of µs at 125 kbit/s) if you timestamp EOF in software. SOF of a *reference* message is deterministic **only if the bus is idle** (TTCAN assumption; not our multi-node event bus).

**Use in our stack:** CAN carries detections and `pps_seq`. It does not *be* the second.

---

## 2. Luckinger & Sauter 2022 — software CanTSyn is ~50 µs

Florian Luckinger and Thilo Sauter, “Software-Based AUTOSAR-Compliant Precision Clock Synchronization Over CAN,” *IEEE Transactions on Industrial Informatics*, vol. 18, no. 10, pp. 7341–7350, Oct. 2022.

- **DOI:** [10.1109/TII.2022.3149923](https://doi.org/10.1109/TII.2022.3149923)
- **IEEE Xplore:** <https://ieeexplore.ieee.org/document/9709093/> (paywalled)
- **Abstract (public):** pure software timestamping, **standard CAN controllers, no hardware modifications**, typical automotive RTOS. After filtering and timestamp-procedure tweaks: **“a precision better than 50 µs”** with a fully AUTOSAR-compliant software implementation.
- Precursor (open abstract): Luckinger & Sauter, “AUTOSAR-compliant Clock Synchronization over CAN using Software Timestamping,” *WFCS 2021*, DOI [10.1109/WFCS46889.2021.9483588](https://doi.org/10.1109/WFCS46889.2021.9483588). Deferred CAN event processing: **~400 µs**. Rate filtering later brings that to ~50 µs.
- Diploma thesis (open): Luckinger, TU Wien, 2021, DOI [10.34726/hss.2021.84920](https://doi.org/10.34726/hss.2021.84920).

Later work citing them (abstract only; IEEE paywalled): Musuroi & Groza, “Secure Time Synchronization With Submicrosecond Accuracy in Controller Area Networks,” *IEEE TII* 2025, DOI [10.1109/TII.2025.3541719](https://doi.org/10.1109/TII.2025.3541719). They quote Luckinger as **~50 µs** with deferred processing + EWMA, then claim **sub-µs on CAN-FD** with DMA capture and clock-rate correction. That silicon is not ESP32 TWAI; we do not take the sub-µs number as ours.

**Against our 10 µs budget:** 50 µs software CanTSyn is **5× the TDoA allowance** (7.4 cm of sound). Even the ~10 µs standard deviation Luckinger reports with aggressive scheduler settings leaves **no margin** for hydrophone SNR, capture, and cable-cal error. Verdict: **does not meet 10 µs** as the TDoA clock.

---

## 3. IEEE 1588 / 802.1AS, and PPS over RS-422 as industrial practice

Packet time on Ethernet is the thing CanTSyn is trying to imitate with 8-byte frames. We do not have a PTP PHY on the buoy.

| Standard | What it is | Accuracy when done as specified | Open / DOI |
|----------|------------|---------------------------------|------------|
| IEEE Std 1588-2019 (PTP / PTPv2.1) | Precision Time Protocol; hardware timestamp at the PHY | **Sub-microsecond**; “better than 1 ns” in a designed network | DOI [10.1109/IEEESTD.2020.9120376](https://doi.org/10.1109/IEEESTD.2020.9120376). IEC dual: IEC 61588:2021. Abstract: “System-wide synchronization accuracy and precision in the sub-microsecond range.” |
| IEEE Std 1588-2008 | Previous edition; still the one most industrial profiles cite | Same order with HW assist | IEC 61588:2009 |
| IEEE Std 802.1AS-2020 (gPTP) | TSN profile of 1588; L2, BMCA, residence-time + link delay | Annex B.3: **≤ 1 µs peak-to-peak** across ≤ 7 hops in steady state | DOI [10.1109/IEEESTD.2020.9121845](https://doi.org/10.1109/IEEESTD.2020.9121845) (paywalled standard; scope/abstract public) |
| Software-only PTP | 1588 without PHY timestamps | **10–100 µs** — same class as NTP on a LAN | Arbiter, *Precision Timing in the Power Industry* <https://arbiter.com/news/technology.php?id=4>: “to achieve timing accuracy better than 10 µs, all devices must run hardware-assisted PTP.” |
| Hardware PTP | PHC in the NIC / switch | **20–100 ns** typical | Same Arbiter note; Rockwell ENET-WP030: PTP 20–100 ns vs IRIG-B 1–10 µs vs NTP 50–100 ms |

**Why this is not our v1 bus.** ESP32-S3 has **no Ethernet MAC**. A W5500 SPI PHY has no 1588 PHC. 802.1AS on a star of W5500s would be software PTP → 10–100 µs → fails the budget the same way CanTSyn software does. Copper-tow §5 already rejected Ethernet-as-hose because Ethernet is a **star**; the 3-buoy test is a **bus**.

**What industry does when it does not have PTP PHYs: a dedicated PPS pair.**

- TIA/EIA-422-B (RS-422): differential, terminated 100–120 Ω twisted pair. Same electrical family as the green pair in copper-tow §7.1.
- Substation practice: unmodulated IRIG-B and 1PPS at ~5 V over coax **or shielded twisted pair**, converted to RS-422 for runs beyond ~15–30 m (Behrendt, “Perfect Time,” [PDF](https://wprcarchives.org/wp-content/uploads/2024/07/Ken-Behrendt_PerfectTime_KB_20050928.pdf); Arbiter note above). IEC 61850-9-2LE sampled values specify an optical 1PPS with **±2 µs** jitter allowed — two orders looser than GPS PPS, still inside our 10 µs.
- Guo, Crossley, et al., “An Assessment of the Precision Time Protocol for Substations,” *IEEE Trans. Power Delivery* (author copy [Manchester](https://pure.manchester.ac.uk/ws/files/51486209/IEEE_Trans_on_Power_Delivery_Hao_Guo_27_03_2016_v3_Final.pdf)): 1-PPS + IRIG-B vs IEEE 1588, both aiming at **±1 µs** for SV/PMU. Quality devices + engineering required; the *medium* is not the limit.
- Cable delay of a PPS pair is the same 5 ns/m physics as Ethernet. Compensate it. Do not pretend it is zero.

**Use in our stack:** 1588/802.1AS is a **shore** option if a USB-Ethernet gateway ever grows a PHC. On the braid, the industrial analogue we can actually solder is **GPS PPS → RS-422 driver → green pair → MCPWM capture**. That is how substations shipped time before every IED spoke PTP.

---

## 4. Syed & Heidemann TSHL 2006 — acoustic time, for nodes *not* on the wire

Affan A. Syed and John Heidemann, “Time Synchronization for High Latency Acoustic Networks,” *Proc. IEEE INFOCOM*, Barcelona, Apr. 2006, pp. 1–12.

- **DOI:** [10.1109/INFOCOM.2006.161](https://doi.org/10.1109/INFOCOM.2006.161)
- **Open PDF:** <https://ant.isi.edu/~johnh/PAPERS/Syed06a.pdf>
- **Landing page:** <https://ant.isi.edu/~johnh/PAPERS/Syed06a.html>
- Extended TR: ISI-TR-2005-602, <https://ant.isi.edu/~johnh/PAPERS/Syed05a.html>

**Claim, from the paper not a paraphrase:** RF protocols (RBS, FTSP, TPSN) assume near-instantaneous delivery. Sound is ~1500 m/s vs 3×10⁸. At 500 m that is **~300 ms** of propagation. TSHL splits the problem:

1. **Phase 1 — skew.** Beacon broadcasts; linear regression on receive times. Skew estimate is **independent of path delay** (delay is a constant offset on every beacon).
2. **Phase 2 — offset.** One skew-compensated two-way exchange (TPSN-style) now that clocks no longer walk during the 300 ms RTT.

**Simulation numbers in the paper** (40 ppm skew, 15 µs receive jitter, 1 µs clock granularity, 25 beacons, 1000 runs):

| Protocol | Instantaneous error vs range | Notes |
|----------|------------------------------|-------|
| RBS / FTSP | **~6 ms at 10 m**, growing to **100 ms+ at 500 m** | Assume simultaneous reception; unusable underwater |
| TPSN-like | ~5–6 µs at <100 m, **~13 µs at 500 m** (Fig. 7) | Two-way cancels delay; residual is skew *during* the exchange |
| TSHL | ~5–6 µs at <100 m, **~6.5 µs at 500 m** (~12 % growth) | “twice the accuracy at 500 m” vs TPSN-L (abstract) |

Error after sync grows much slower for TSHL (Fig. 8): still **< 50 µs after 5 s** at 400 m. RBS/FTSP are omitted from those plots because propagation error already dominates.

**When we still need it.** Copper PPS replaces TSHL **only for nodes on the braid**. Keep TSHL / OWTT (`RANGING.md`, `clawft-sonobuoy-ranging` deferred “TSHL / D-Sync clock discipline”) for:

- Class B at −1 m / −2 m if the 4-conductor drop is analog-only and has no PPS pair.
- Any free-drifting Class A that is **not** taut to the CAT7.
- Later 100 m–5 km OWTT. TSHL’s own analysis says existing protocols are “adequate at very short distances”; 15–30 m of *water* is short, but 15–30 m of *acoustic* sync is still worse than 159 ns of copper.

TSHL is simulation + an in-air Cricket attempt (detection hardware was insufficient for µs). Treat the µs figures as **order of magnitude**, not a lake calibration.

---

## 5. GPS PPS ±50 ns into a DSP / MCU capture (LCPC lineage and cousins)

The plan’s master is “one GPS PPS on shore … fan the edge down green.” That is a 1990s–2010s instrument pattern, not a WeftOS idea.

### 5.1 The pulse itself

| Source | PPS vs UTC | Notes |
|--------|------------|-------|
| gpsd *Introduction to Time Service* <https://gpsd.gitlab.io/gpsd/time-service-intro.html> | **50 ns** GPS 1PPS top-of-second | USB 1PPS is 100 µs–1 ms — **do not** bring PPS in over USB |
| NovAtel APN-015 (OEM clock steering) <https://www.gnss.ca/app_notes/APN-015_NOVATEL_OEM_SERIES_Receiver_Time_GPS_Time_Clock_Steering_and_the_1_PPS_Strobe_Application_Note.html> | **~50 ns** jitter with steering on; 250 ns SPS with SA on (historical) | Hardware-tied to receiver clock |
| Berns & Wilkes, “GPS Time Synchronization System for K2K,” *IEEE Trans. Nucl. Sci.* 47(2):340–343, Apr. 2000 | 1PPS leading edge “well within” **100 ns**; LTC 20 ns ticks | Follow-on school-network card: “about **50 ns** accuracy in UTC,” DOI [10.1109/TNS.2004.829368](https://doi.org/10.1109/TNS.2004.829368) / NSSMIC [10.1109/nssmic.2003.1351816](https://doi.org/10.1109/nssmic.2003.1351816) |
| u-blox TIMEPULSE (NEO-M8T class) | 20–30 ns RMS typical | Industrial monitor / vendor tables; QErr correction → ~10 ns (Le Cam 2023) |

**50 ns of sound is 74 µm.** The GPS edge is free relative to 10 µs. The capture path is not.

### 5.2 LCPC / Gustave Eiffel: PPS into a DSP, then an FPGA

Laboratoire Central des Ponts et Chaussées (LCPC, now Université Gustave Eiffel) wired a cheap GPS module’s PPS into a **DSP timer-capture**, then published the same architecture for acoustic SHM.

- V. Le Cam, L. Lemarchand, L.-M. Cottineau (LCPC Instrumentation). Public HAL copy of the platform note (Anubis-gated when fetched 2026-09-25; indexed snippet): GPS PPS “1 s ± **50 ns**” into a DSP; “each couple of sensors Si and Sj, ensure a time stamping such that **‖Ti − Tj‖ ≤ 4 µs**.” Absolute time-base “with a precision of **1 µs**.” HAL landing: <https://hal.science/hal-04473787>.
- V. Le Cam, A. Bouche, D. Pallier, “Wireless Sensors Synchronization: an accurate and deterministic GPS-based algorithm,” *IWSHM 2017*. HAL: <https://inria.hal.science/hal-01633693>. Abstract claims up to **10 ns UT** on some (electrical) apps; acoustic is the looser case.
- D. Pallier, V. Le Cam, S. Pillement, “Energy-efficient GPS synchronization for wireless nodes,” *IEEE Sensors Journal* 21(4):5221–5229, 15 Feb 2021. **DOI [10.1109/JSEN.2020.3031350](https://doi.org/10.1109/JSEN.2020.3031350)** (IEEE paywalled; HAL <https://hal.science/hal-02968155>). Dedicated timestamping hardware. GPS off 60–95 % of the time for **20 ns to 420 ns** mean error. This is the LCPC capture path with the receiver cycled.
- V. Le Cam, L. Lemarchand, A. Bouche, D. Pallier, F. Illien, “An Original Smart Data Sampling for Wireless Sensor: Application to Bridge Cable Monitoring,” *SHM 2023*. Open PDF: <https://www.dpi-proceedings.com/index.php/shm2023/article/download/36843/35419>. PPS “typical accuracy of **20 to 50 nanoseconds** relative to GPS time.” Two independent GPS-synced nodes on a **shared step signal**: mean **3.225 µs**, max **6.187 µs**, σ **2.06 µs** (Table II). They are timestamping an ADC sample path, not just the PPS pin — that extra few µs is the lesson for us: **capture the PPS in hardware, then timestamp detections against that counter**, or the ADC/SPI path eats the budget.

**Mapping onto ESP32-S3:** LCPC’s DSP input-capture **is** MCPWM capture. Shore GPS PPS (or a docked buoy with sky) is the LCPC GPS module. The green RS-422 pair is the “wireless” replaced by copper. We should beat their 3 µs ADC-path number because we are not going GPS→NMEA→wireless→supervisor for the edge; we are fanning one edge down a 30 m pair.

### 5.3 Do not confuse PPS with NMEA

NMEA `$GPRMC` after the pulse is tens of milliseconds late and is how USB GPS dongles lie about time. Timestamp **the edge**. Label the second from NMEA after the fact, the way LCPC and Berns–Wilkes both do.

---

## 6. Verdict table

Budget column is **meets the 10 µs TDoA clock** for nodes that must share a sample time. “Typical jitter” is the figure the cited paper or spec actually states, not a guess.

| Method | Typical jitter / error | Meets 10 µs? | Use in our stack |
|--------|------------------------|--------------|------------------|
| CAN SOF or frame timestamp as time | Arbitration: **ms under load**. Bit stuffing: tens of µs. Software ISR: **50–400 µs** (Luckinger). | **No** | **Never.** CAN carries `{pps_seq, dt_us, peak}` only. |
| AUTOSAR CanTSyn, software (Luckinger TII 2022) | **< 50 µs** after rate filter; ~400 µs deferred; σ ~10 µs in the best scheduler setting | **No** (5× budget; no margin) | Not the TDoA clock. Optional later for non-ranging event order. |
| CanTSyn + CiA 603 hardware TSU (Hartwich 2017) | < 1 CAN bit + ACK; sub-µs to few µs if the TSU is real | **Maybe** | ESP32 TWAI has **no** TSU. Skip. |
| IEEE 1588 / 802.1AS, **hardware** PHC | **20–100 ns** typical; 802.1AS ≤ 1 µs over ≤ 7 hops | **Yes** | Shore Ethernet **if** a PTP NIC appears. Not on S3 + W5500. |
| IEEE 1588, **software** | **10–100 µs** | **No** / borderline with no margin | Don’t. Same failure mode as software CanTSyn. |
| **GPS PPS → RS-422 on CAT5 green pair → MCPWM capture** | GPS **±50 ns**; cable **5.3 ns/m** (cal out); RS-422 driver tens of ns; MCPWM **12.5 ns**. End-to-end well under 1 µs if the edge is hardware-captured. | **Yes** | **This is the TDoA clock for nodes on the braid.** Replaces CSAC + acoustic TWTT for that set. |
| GPS PPS into DSP/MCU capture (LCPC / Pallier / Berns–Wilkes) | PPS ±50 ns; **1–5 µs** pair error in published SHM builds (3.2 µs mean when the ADC path is included) | **Yes**, if capture is hardware and the ADC path is not in the critical timestamp | Architecture template for the master and for any buoy that has sky. We fan one PPS instead of putting a GPS on every node. |
| Acoustic TSHL / two-way (Syed–Heidemann 2006) | Simulation **~6 µs at 500 m** given 15 µs RX jitter; RBS **ms**. Real water will be worse (multipath, SNR). | **Maybe** at 15–30 m; not the copper path | **Yes, for nodes not on the wire** (Class B analog drop, free-drifting A). Already in `RANGING.md`. |
| CSAC (chip-scale atomic) | ns-class holdover | **Yes** | Skip for v1 wired set. Keep for later subsurface / long holdover. |
| USB audio / USB-GPS as time | USB 1.1 PPS ~1 ms; USB 2.0 ~100 µs (gpsd table) | **No** | Forbidden in copper-tow §7.4. |
| LoRa-on-copper symbol time | 10–100 **ms** | **No** | Not a clock. See copper-tow §6.4. |

---

## 7. What this means for the 3-buoy braid

1. **Green pair is load-bearing.** Without it, the literature says you get Luckinger’s 50 µs or TSHL’s acoustic two-way. Both fight the 10 µs budget. With it, CAT5 physics is 159 ns at 30 m and GPS is 50 ns.
2. **Do not implement CanTSyn to “save a pair.”** The entire point of SYNC+FUP is that CAN cannot tell you when a frame started. We already know that.
3. **Hardware capture or don’t bother.** ESP32 MCPWM / GPIO capture, not `micros()`. LCPC’s 3 µs residual is what happens when the timestamp includes an ADC/SPI path; latch the PPS edge first.
4. **TSHL stays in the crate, off the v1 hose.** `clawft-sonobuoy-ranging` still owes TSHL/D-Sync for anyone not on the braid. Wired taut edges are exact `D_ij = L_cable`.
5. **One GPS, many captures.** Berns–Wilkes and LCPC put a receiver per station because they had no wire. We have a wire. Shore (or one mast) GPS PPS, RS-422 fanout, `delay_ns[i]` from a cal pulse.

Nothing here requires a new silicon PHY. SN65HVD230 (CAN) + an RS-422 driver (AM26C31-class or THVD2450) + MCPWM capture is the whole timing story for v1.

---

## References (compact)

1. AUTOSAR, *Specification of Time Synchronization over CAN* (CanTSyn), ID 674, R4.2.2 / R25-11. <https://www.autosar.org/fileadmin/standards/R25-11/CP/AUTOSAR_CP_SWS_TimeSyncOverCAN.pdf>
2. AUTOSAR, *Time Synchronization over CAN Protocol* (PRS), ID 1089, R25-11. <https://www.autosar.org/fileadmin/standards/R25-11/FO/AUTOSAR_FO_PRS_TimeSyncOverCANProtocol.pdf>
3. F. Hartwich, “CAN frame time-stamping — supporting AUTOSAR time base synchronization,” *iCC 2017*. <https://can-cia.org/fileadmin/cia/documents/proceedings/2017_hartwich.pdf>
4. F. Luckinger and T. Sauter, “Software-Based AUTOSAR-Compliant Precision Clock Synchronization Over CAN,” *IEEE TII* 18(10):7341–7350, 2022. DOI [10.1109/TII.2022.3149923](https://doi.org/10.1109/TII.2022.3149923). Abstract used; PDF paywalled.
5. F. Luckinger and T. Sauter, “AUTOSAR-compliant Clock Synchronization over CAN using Software Timestamping,” *WFCS 2021*. DOI [10.1109/WFCS46889.2021.9483588](https://doi.org/10.1109/WFCS46889.2021.9483588).
6. IEEE Std 1588-2019. DOI [10.1109/IEEESTD.2020.9120376](https://doi.org/10.1109/IEEESTD.2020.9120376). Dual IEC 61588:2021.
7. IEEE Std 802.1AS-2020. DOI [10.1109/IEEESTD.2020.9121845](https://doi.org/10.1109/IEEESTD.2020.9121845). Annex B.3: 1 µs p-p ≤ 7 hops (draft text publicly mirrored in IEEE 802.1 files).
8. Arbiter Systems, “Precision Timing in the Power Industry.” <https://arbiter.com/news/technology.php?id=4>
9. A. Syed and J. Heidemann, “Time Synchronization for High Latency Acoustic Networks,” *IEEE INFOCOM* 2006. DOI [10.1109/INFOCOM.2006.161](https://doi.org/10.1109/INFOCOM.2006.161). Open PDF: <https://ant.isi.edu/~johnh/PAPERS/Syed06a.pdf>
10. H. G. Berns and R. J. Wilkes, “GPS Time Synchronization System for K2K,” *IEEE TNS* 47(2):340–343, 2000; school-network follow-on DOI [10.1109/TNS.2004.829368](https://doi.org/10.1109/TNS.2004.829368).
11. D. Pallier, V. Le Cam, S. Pillement, “Energy-efficient GPS synchronization for wireless nodes,” *IEEE Sensors J.* 21(4):5221–5229, 2021. DOI [10.1109/JSEN.2020.3031350](https://doi.org/10.1109/JSEN.2020.3031350). HAL: <https://hal.science/hal-02968155>
12. V. Le Cam et al., “An Original Smart Data Sampling for Wireless Sensor,” *SHM 2023*. Open PDF: <https://www.dpi-proceedings.com/index.php/shm2023/article/download/36843/35419>
13. gpsd, “Introduction to Time Service.” <https://gpsd.gitlab.io/gpsd/time-service-intro.html>
14. TIA/EIA-568 Cat 5 electricals (delay 5.30 ns/m, VF 0.64) as tabulated at <https://en.wikipedia.org/wiki/Category_5_cable>
15. Espressif, MCPWM capture (80 MHz APB latch). <https://docs.espressif.com/projects/esp-idf/en/latest/esp32/api-reference/peripherals/mcpwm.html>
