# LoRa-on-copper vs CAN + PPS (WeftOS buoys)

**Date:** 2026-09-25  
**Status:** Hardware research note (not an ADR)  
**Question:** Can LoRa (CSS) over a CAT5/7 pair replace CAN + a dedicated PPS strobe for the 15–30 m 3-buoy test?  
**Companions:** [`docs/research/sonobuoy-min-test-and-copper-tow.md`](../sonobuoy-min-test-and-copper-tow.md) (§6.4, §7), [`docs/research/pzsdr-p047-and-fiber-towline.md`](../pzsdr-p047-and-fiber-towline.md)

---

## Verdict

**No.** LoRa-like chirp spread spectrum (CSS) on a copper pair is a **PLC curiosity for kilometre-class, seconds-scale telemetry**. It is **not a PPS**, and it is **not a 16 kHz PCM bus**.

Keep the stack already pinned in the copper-tow note:

| Pair (T568B) | Job |
|--------------|-----|
| orange | **CAN_H / CAN_L** — ESP32-S3 TWAI + SN65HVD230, 125–500 kbit/s |
| blue | **+VIN / GND** — 12–24 V, buck at each node |
| green | **PPS / strobe** — RS-422 (or open-drain + local pull-ups) |
| brown | analog / spare |

LoRa (SX1262 / SX1276) stays an **optional later radio** (over-water, km drop, leaky-feeder experiment). It does not replace the copper bus or the sync pair.

---

## 1. What the buoy bus actually has to do

The 3-buoy lake test is 15–30 m of CAT5/7, not a rural MV feeder. Acoustic TDoA wants **~10 µs** common time (~1.5 cm of sound at \(c \approx 1480\) m/s). Events (`peak`, `t_us`, `snr`) are a few CAN frames. Optional 16 kHz PCM is a different problem:

| Stream | Raw rate | What can carry it |
|--------|----------|-------------------|
| 16 kHz × 16-bit, one phone | **256 kbit/s** | 500 kbit/s–1 Mbit/s CAN *if* you poll slices and accept ~50% frame overhead; Ethernet; analog pair |
| Same, three buoys continuous | **768 kbit/s** | **not** classic CAN 2.0; **not** LoRa |
| Detection events, 3 nodes | tens of bytes / chirp | CAN at 125 kbit/s is plenty |

Wired time is general; wired *position* is 1-D along a taut hose. Slack CAT7 is not a survey. That geometry argument is in the copper-tow note §7 — this note is only the **PHY**.

---

## 2. The IEEE paper: LoRa-like CSS as PLC

The paper that people mean when they say “LoRa on the power line, −40 dB SNR, seconds-scale” is:

> S. Robson and M. Haddad, “A Chirp Spread Spectrum Modulation Scheme for Robust Power Line Communication,” *IEEE Transactions on Power Delivery*, vol. 37, no. 6, pp. 5299–5309, Dec. 2022.  
> DOI: [10.1109/TPWRD.2022.3175830](https://doi.org/10.1109/TPWRD.2022.3175830)  
> Open preprint: [arXiv:2106.13965](https://arxiv.org/abs/2106.13965) · [https://doi.org/10.48550/arXiv.2106.13965](https://doi.org/10.48550/arXiv.2106.13965)  
> Cardiff OA PDF: [https://orca.cardiff.ac.uk/id/eprint/149788/](https://orca.cardiff.ac.uk/id/eprint/149788/)

It is **not** “plug an SX1262 into CAT5 and get a fieldbus.” It is a **modified** LoRa PHY for the LV→MV power-line channel:

1. **LoRa-Mod** — after dechirp + FFT, group the \(2^{SF}\) bins into “superbins” so RMS delay spread (tens–hundreds of µs on MV) stays inside one symbol. Bits per symbol drop.
2. **LoRa-Mod-Enhanced** — average the same superbin over \(Q\) consecutive symbols. Variance falls as \(1/Q\). They show error-free demodulation at **SNR = −40 dB** with large \(Q\).

That robustness is paid for in **time**:

- Stated application: LV feeder *load data* back to an MV primary substation on **“timescales of several seconds or minutes.”**
- ATP-EMTP case study (Table I): **13 s time-on-air** per transmitter at SF = 13.
- FPGA prototype (SF = 10, BW = 50 kHz, \(Q = 64\)): **≈ 1.3 s per running average** at SNR = −19 dB.

A predecessor conference paper is explicit that this is an experiment on whether the LoRa *physical layer* belongs on the *wired power-line channel*, not a CAN replacement:

> S. Robson and A. M. Haddad, “On the use of LoRa for Power Line Communication,” *2019 54th International Universities Power Engineering Conference (UPEC)*, 2019.  
> DOI: [10.1109/UPEC.2019.8893538](https://doi.org/10.1109/UPEC.2019.8893538)

**Takeaway for WeftOS:** the −40 dB number is real, and it is the wrong figure of merit. On 15–30 m of CAT7 the SNR is enormous. The thing we lack is **symbol rate and a hardware edge**, not processing gain.

---

## 3. Why LoRa-on-a-pair “works” (leaky feeder)

You *can* couple an SX1262 / SX1276 RF port into a twisted pair with a **4:1 balun** (50 Ω → 100 Ω CAT5) instead of an antenna. The cable then behaves like a crude **bifilar leaky feeder**: guided RF along the pair, some radiation, huge SNR on a short run.

That is a known confined-space radio technique, not a new PHY:

- ITU-R Rec. **M.1075** (1994), *Leaky feeder systems in the land mobile service* — coaxial radiating cable *and* bifilar lines as distributed antennas in mines and tunnels.  
  [https://www.itu.int/rec/R-REC-M.1075](https://www.itu.int/rec/R-REC-M.1075-0-199409-I)
- NIOSH survey of North-American mine leaky-feeder plants: J. C. Cawley, “An Assessment of Leaky Feeder Radio Systems in Underground Mines,” NIOSH.  
  [https://www.cdc.gov/niosh/mining/works/coversheet1804.html](https://www.cdc.gov/niosh/mining/works/coversheet1804.html) (also OneTunnel reprint)
- Low-cost “unintentional leak” coax as a rescue feeder: M. D. Bedford, *Mining Technology* 129(4), 2020. DOI: [10.1080/25726668.2020.1838110](https://doi.org/10.1080/25726668.2020.1838110)

Mines run **voice and slow telemetry** over kilometres of radiating cable with line amplifiers every 350–500 m. They do not timestamp hydrophones off the LoRa chirp.

On 15–30 m of CAT7 the link budget is the opposite problem: **too much power**.

| Quantity | Number | Source |
|----------|--------|--------|
| SX1262 \(P_{TX}\) | up to **+22 dBm** | [Semtech SX1262](https://www.semtech.com/products/wireless-rf/lora-connect/sx1262) |
| SX1262 LoRa bitrate | **0.018–62.5 kbit/s** (SF12/BW 7.8 kHz → SF5/BW 500 kHz) | SX1261/2 datasheet |
| Typical LoRaWAN | **~0.3–5 kbit/s** | SF12→SF7, 125 kHz; Semtech AN1200.13 |
| Module RF input, operating / abs. max | **0 dBm / +10 dBm** | e.g. Seeed Wio-SX1262 module DS |
| CAT5e IL @ 100 MHz | ~22 dB / 100 m → **~7 dB / 30 m** | TIA-568 (cable is **not** specified at 868 MHz) |

Even if 868 MHz on UTP is several times lossier than 100 MHz, a +22 dBm PA into 30 m of pair still lands near or above the next node’s **0 dBm operating max**. TX **must be padded / attenuated** (20–30 dB class) or the neighbour saturates. That is a radio-on-a-wire, not a bus transceiver.

---

## 4. LoRa is not a PPS

LoRa symbol duration is \(T_{sym} = 2^{SF}/BW\):

| SF | \(T_{sym}\) @ 125 kHz | 12-byte ToA (CR 4/5) | Nominal rate @ 125 kHz |
|----|------------------------|----------------------|------------------------|
| 7 | 1.024 ms | 41 ms | ~2.3 kbit/s |
| 10 | 8.192 ms | 289 ms | ~0.33 kbit/s |
| 12 | **32.768 ms** | **1.16 s** | **~83 bit/s** |

Formula and tables: Semtech **AN1200.13** (*SX1272/3/6/7/8 LoRa Modem Design Guide*) and the SX1261/2 datasheet §6.1.4. Fastest SX1262 LoRa corner (SF5, 500 kHz) is still **\(T_{sym} = 64~\mu s\)**, and a *packet* is a preamble plus many symbols.

TDoA budget is **10 µs**. A CSS symbol is **6–3000× too long** to *be* the strobe. Unslotted **ALOHA** (LoRaWAN Class A) adds collision backoff on top. You cannot recover a 1 Hz GPS-quality edge from “the chirp arrived sometime this symbol.”

A separate, FPGA-grade CSS *timing* paper exists. It does **not** rescue SX1262-on-CAT5:

> S. Robson and M. A. Haddad, “A sub-μs accuracy GPS alternative using electrical transmission grids as precision timing networks,” *Scientific Reports* 14, 8696 (2024).  
> DOI: [10.1038/s41598-024-56296-8](https://doi.org/10.1038/s41598-024-56296-8) · PMC: [PMC11018771](https://www.ncbi.nlm.nih.gov/pmc/articles/PMC11018771/)

That work uses a **GNSS-aligned downchirp**, a fine interpolator past the LoRa bin, FPGA correlators, and **TOF calibration while GNSS is up**. Experimental floor is sub-µs on 700 m of coax; averaging is required down to −20 dB (600 ns class at the noisy end). It is a **substation PTN**, not an ESP32 SPI radio, and it still is not a dedicated PPS pair you GPIO-capture.

**Do not** timestamp off CAN start-of-frame either. SOF hardware stamps are fine for *which frame won the bus*. They are not a 10 µs epoch:

- A classic 8-byte frame is ~110–130 bits → **~220–260 µs** on the wire at 500 kbit/s, longer with stuffing.
- A node that loses arbitration **retries after the winner**. Under load that wait is **milliseconds** (priority inversion, error frames).
- AUTOSAR “Time Sync over CAN” exists precisely because “put UTC in a frame” is wrong; it still needs ingress/egress HW stamps and is specified in the tens-of-µs class, not a GPIO edge.  
  AUTOSAR FO *Time Synchronization over CAN Protocol*: [https://www.autosar.org/fileadmin/standards/R22-11/FO/AUTOSAR_PRS_TimeSyncOverCAN.pdf](https://www.autosar.org/fileadmin/standards/R22-11/FO/AUTOSAR_PRS_TimeSyncOverCAN.pdf)

The green pair is the clock: **RS-422 PPS / TX_ACTIVE**, ESP32 **MCPWM / GPIO capture** (not `micros()` in an Arduino ISR). CAT5 delay is ~**5 ns/m** (VF ≈ 0.66); 30 m is ~150 ns ≈ 0.2 mm of sound — below the hydrophone. Measure `delay_ns[node]` once.

---

## 5. LoRa is not 16 kHz PCM

| PHY | Payload class | 16 kHz × 16-bit PCM |
|-----|---------------|---------------------|
| LoRa CSS (SX1262) | 0.018–62.5 kbit/s; typical 0.3–5 kbit/s | **No.** One channel is 256 kbit/s. Even the 62.5 kbit/s corner is 4× too slow, ALOHA, half-duplex. |
| CSS-PLC (Robson 2022) | seconds–minutes per reading | **No.** |
| Classic CAN 2.0 | 125 kbit/s–1 Mbit/s, **8-byte** frames | Events yes. Continuous 3-buoy PCM **no**. One buoy, 256-sample slices, maybe, at 500 kbit/s–1 Mbit/s. |
| RS-485 UART | 115.2 kbit/s typical; 1–10 Mbit/s short | Same story as CAN for PCM: possible only if you dedicate the pair and drop events. |
| 100BASE-TX (W5500) | 100 Mbit/s, star | PCM yes; **not** a one-hose bus. |

Do not put three continuous PCM streams on 125 kbit CAN. Poll events; if you need waveform, take **one node at a time** or keep analog on brown / USB audio.

---

## 6. ESP32-S3 TWAI / CAN — the actual copper PHY

The S2/S3 have **no Ethernet MAC**. They do have on-chip **TWAI** (Two-Wire Automotive Interface) = ISO 11898-1 **CAN 2.0**.

| Limit | Value | Source |
|-------|-------|--------|
| Controllers on ESP32-S3 | **1** | [ESP-IDF TWAI (ESP32-S3)](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s3/api-reference/peripherals/twai.html) |
| Frame format | 11-bit and 29-bit IDs, 0–8 byte payload | same; TRM ch. 30 |
| Bit rate | **1 kbit/s – 1 Mbit/s** | ESP32-S3 TRM |
| CAN FD | **Not supported** — FD frames are errors | ESP-IDF note above |
| External PHY | required; **SN65HVD230** (3.3 V, ISO 11898-2, 1 Mbit/s, ~$1) | [TI SN65HVD230](https://www.ti.com/product/SN65HVD230) · DS [SLOS346](https://www.ti.com/lit/ds/slos346o/slos346o.pdf) |
| ISO 11898-2 length | **~40 m @ 1 Mbit/s**, ~100 m @ 500 kbit/s, ~500 m @ 125 kbit/s | ISO 11898-2; CiA length/bitrate tables |
| Termination | 120 Ω at **both ends** of the braid | ISO 11898-2 |
| Arbitration | non-destructive, lowest ID wins | ISO 11898-1 |

The 15–30 m triangle is **inside** the 1 Mbit/s length budget. Run **500 kbit/s** unless the hose grows; 125 kbit/s is the wet-tolerant conservative default. 29-bit IDs map cleanly to `buoy_id` + message class. Hardware ACK + error confinement beat a UART.

TWAI **frame timestamps** (ESP-IDF `timestamp_resolution_hz`) inherit chip time from power-on. They are useful for *ordering on one node*. They are **not** a fleet PPS.

---

## 7. RS-485 vs CAN for this sensor bus

Both are differential two-wire PHYs. They are not interchangeable at the protocol layer.

| | **CAN (ISO 11898-1/2)** | **RS-485 (TIA/EIA-485-A)** |
|--|-------------------------|----------------------------|
| What the standard is | data link **+** PHY | **PHY only** |
| Collision | wired-AND **arbitration**; winner’s frame is intact | two drivers fight; data is garbage; modern parts thermally protect |
| Access | multi-master | software: **master poll**, token, or DE/RE half-duplex discipline |
| Integrity | CRC, ACK, error frames, bus-off | none in the PHY; Modbus CRC is software |
| Payload | 8 bytes classic (TWAI) | UART framing, any length |
| Rate × distance | 1 Mbit/s @ ~40 m | 10 Mbit/s short; **~1200 m** at low baud |
| Silicon here | SN65HVD230 + on-chip TWAI | MAX3485 + UART (~$0.50) |
| Direction pin | none (open-collector dominant) | **DE / ~RE** must be sequenced |

Primary sources:

- TIA/EIA-485-A (1998, reaffirmed 2012) — electrical only. Wikipedia/overview: [https://en.wikipedia.org/wiki/RS-485](https://en.wikipedia.org/wiki/RS-485)
- Analog Devices **AN-1123**, *CAN Implementation Guide* — side-by-side dominant/recessive vs RS-485 A−B levels.  
  [https://www.analog.com/en/resources/app-notes/an-1123.html](https://www.analog.com/en/resources/app-notes/an-1123.html)
- TI **SBOA442** — CAN open-collector vs RS-485 push-pull contention.  
  [https://www.ti.com/lit/pdf/sboa442](https://www.ti.com/lit/pdf/sboa442)
- TI **SLLA067** — industrial interface cookbook (CAN vs 485 length/rate).  
  [https://www.ti.com/lit/an/slla067/slla067.pdf](https://www.ti.com/lit/an/slla067/slla067.pdf)

**Pick CAN** for the 3-buoy hose: detections are asynchronous, IDs are hardware, and a collision must not destroy the high-priority peak. **RS-485 is the afternoon fallback** if TWAI misbehaves in the bucket test (poll from shore, 115200 8N1, USB-485 into the laptop). I²C stays on the 1–3 m Class B pigtail only.

---

## 8. Coupling, EMI, and why a radio on the pair is still a radio

If someone still wants a LoRa-on-pair *experiment* (km drop, not the 3-buoy bus):

1. **Attenuate TX.** Aim for −20 to 0 dBm at the far node, not +22 dBm.
2. **Do not** share the orange CAN pair. A CSS chirp in the 150–500 kHz NB-PLC band, or an 868 MHz leak, is hostile to a 500 kbit/s CAN eye. Keep LoRa on brown or a fifth conductor.
3. Single-point braid ground at the **dry** end (CAT7 S/FTP). Seawater must not close a loop.
4. Common-mode chokes on the RF tap so the balun does not dump PA current into CAN_GND.
5. Shore side is a USB LoRa dongle — still no Wi-Fi, still the wrong bitrate for PCM.

That experiment does not move the 15–30 m stack.

---

## 9. Decision table

| Need | LoRa / CSS-on-pair | CAN 2.0 + TWAI | Dedicated PPS (RS-422) |
|------|--------------------|----------------|------------------------|
| 15–30 m 3-buoy events | works, slow ALOHA | **yes** | n/a |
| km / transformer-hostile PLC | **yes** (Robson 2022) | not the point | n/a |
| 10 µs TDoA epoch | **no** (symbol 0.06–33 ms; PLC paper is seconds) | **no** (arbitration / SOF jitter) | **yes** |
| 16 kHz PCM × 3 | **no** | no (poll one node) | n/a |
| Multi-master detections | ALOHA collisions | **hardware ID arbitration** | n/a |
| ESP32-S3 native | SPI to extra radio | **on-chip TWAI** | MCPWM capture |
| TX on short copper | must pad | ISO 11898-2 levels | logic / RS-422 |

**CAN + dedicated strobe remains the stack.** LoRa-on-copper is optional later radio, not the bus.

---

## References

1. S. Robson, M. Haddad, “A Chirp Spread Spectrum Modulation Scheme for Robust Power Line Communication,” *IEEE Trans. Power Delivery* 37(6):5299–5309, 2022. DOI: [10.1109/TPWRD.2022.3175830](https://doi.org/10.1109/TPWRD.2022.3175830). arXiv: [2106.13965](https://arxiv.org/abs/2106.13965).
2. S. Robson, A. M. Haddad, “On the use of LoRa for Power Line Communication,” *UPEC* 2019. DOI: [10.1109/UPEC.2019.8893538](https://doi.org/10.1109/UPEC.2019.8893538).
3. S. Robson, M. A. Haddad, “A sub-μs accuracy GPS alternative using electrical transmission grids as precision timing networks,” *Sci. Rep.* 14:8696, 2024. DOI: [10.1038/s41598-024-56296-8](https://doi.org/10.1038/s41598-024-56296-8).
4. Semtech, *SX1261/2 datasheet* (LoRa bitrate 0.018–62.5 kb/s, \(P_{TX}\) +22 dBm). [Product page](https://www.semtech.com/products/wireless-rf/lora-connect/sx1262).
5. Semtech, *SX1272/3/6/7/8 LoRa Modem Design Guide* (AN1200.13) — \(T_{sym}=2^{SF}/BW\), time-on-air.
6. ITU-R Rec. M.1075, “Leaky feeder systems in the land mobile service,” 1994. [https://www.itu.int/rec/R-REC-M.1075](https://www.itu.int/rec/R-REC-M.1075-0-199409-I).
7. ISO 11898-1 (CAN data link) and ISO 11898-2 (high-speed PMA, ~1 Mbit/s @ 40 m).
8. Espressif, “Two-Wire Automotive Interface (TWAI)” — ESP32-S3, classic CAN only. [https://docs.espressif.com/projects/esp-idf/en/latest/esp32s3/api-reference/peripherals/twai.html](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s3/api-reference/peripherals/twai.html).
9. Texas Instruments, *SN65HVD23x 3.3-V CAN Bus Transceivers*, SLOS346. [https://www.ti.com/product/SN65HVD230](https://www.ti.com/product/SN65HVD230).
10. TIA/EIA-485-A (RS-485 PHY). Analog Devices AN-1123; TI SBOA442, SLLA067.
11. AUTOSAR, *Time Synchronization over CAN Protocol*. [AUTOSAR FO PRS](https://www.autosar.org/fileadmin/standards/R22-11/FO/AUTOSAR_PRS_TimeSyncOverCAN.pdf).
12. In-tree: [`docs/research/sonobuoy-min-test-and-copper-tow.md`](../sonobuoy-min-test-and-copper-tow.md) §6.2–§7.4 (pair map, “do not use CAN SOF as PPS”).
