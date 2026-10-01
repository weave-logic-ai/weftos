# LoRa + SDR RF sensing, and fusing it with the other sensors

Date: 2026-09-30. Status: research, nothing built. Tags: [V] read in the cited source; [I] inference.

The questions this answers:

1. Can a LoRa transmitter plus the RTL-SDR we already own sense people (presence, motion, breathing)?
2. How should that fit with the other sensors on the shopping list (WiFi CSI, mmWave, ToF camera, GNSS)?

Hardware in scope: the DFRobot FireBeetle LoRa cover (TEL0122, SX127x, 915 MHz) as the transmitter, and an RTL-SDR as the receiver. Related notes: [espargos/espsdr-and-linux-csi-nodes.md](espargos/espsdr-and-linux-csi-nodes.md) (Wi-Fi I/Q and CSI nodes), [graph-views.md](graph-views.md) (how fusion works operationally), [../weftos/splat-multimodal-sensing.md](../weftos/splat-multimodal-sensing.md) (modality catalog).

## 1. Why use a sub-GHz radio for sensing

- **The published result.** Zhang et al., "Exploring LoRa for Long-Range Through-Wall Sensing" (IMWUT 4(3), 2020; UbiComp 2021 outstanding paper) sensed human breathing at **25 m** and at **15 m through a wall**. They tracked walking out to 30 m. They used two receive antennas with a noise-reduction step, plus "chirp concentration", which uses the LoRa chirp structure to raise signal power. ([HAL](https://hal.archives-ouvertes.fr/hal-03363358), [PKU news](https://www.sei.pku.edu.cn/info/1036/1394.htm)) [V]
- **Follow-ups.** The same group extended this to multi-target breathing with beamforming ([HAL](https://hal.archives-ouvertes.fr/hal-03363386)). WideSee (SenSys 2019) used one LoRa transceiver pair on a drone to detect and locate people over a wide area, and tested it in a high-rise building ([White Rose](https://eprints.whiterose.ac.uk/149652)). [V]
- **What the wavelength does.** At 915 MHz, λ ≈ 32.8 cm, against 12.5 cm at 2.4 GHz and 1.25 cm at 24 GHz. Longer waves go through walls and give a wider Fresnel zone, so a person anywhere in a large region changes the signal. They are also less sensitive to small displacements. [I, arithmetic below]

| | 915 MHz | 2.4 GHz | 24 GHz | 60 GHz |
|---|---|---|---|---|
| Wavelength | 32.8 cm | 12.5 cm | 1.25 cm | 0.5 cm |
| Phase change, 5 mm chest (breathing), 4πd/λ | 0.19 rad (11°) | 0.50 rad (29°) | 5.0 rad (wraps) | 12.6 rad (wraps) |
| Phase change, 0.3 mm (heartbeat) | 0.011 rad (0.7°) | 0.03 rad | 0.30 rad | 0.75 rad |
| First Fresnel radius, middle of a 10 m link | 0.91 m | 0.56 m | 0.18 m | 0.11 m |

The 4πd/λ figure is the monostatic upper bound. For a bistatic link it depends on where the person stands relative to the link. [I]

**What to expect at 915 MHz:**

- **Presence and motion:** strong. The zone of influence is wide, the signal gets through walls, and it covers tens of metres. [I, consistent with the papers]
- **Breathing:** yes, but the signal is small (about 11° of phase for a 5 mm chest movement). It needs a clean, stable receiver. [I]
- **Heart rate:** treat it as impractical at this frequency. Under 1° of phase is buried in 8-bit SDR noise. Leave heart rate to the 60 GHz radar. [I]
- **Resolution:** coarse. A 125 kHz LoRa channel has no usable range resolution. Location comes from link geometry (which Fresnel zones light up), not from time of flight. [I]

### Band choice: 433, 868 or 915 MHz

**Use 915 MHz (902-928) in North America.** It's the only one of the three we can legally run as an unlicensed, continuously transmitting illuminator. It also has the most bandwidth, and the sensitivity trade-off favours it.

| | 433 MHz | 868 MHz | 915 MHz |
|---|---|---|---|
| Wavelength | 69 cm | 34.5 cm | 32.8 cm |
| Phase change, 5 mm chest | 0.09 rad (5°) | 0.18 rad | 0.19 rad (11°) |
| First Fresnel radius, 10 m link | 1.3 m | 0.93 m | 0.91 m |
| Walls | best | good | good |
| Quarter-wave antenna | ~17 cm | ~8.6 cm | ~8.2 cm |
| Unlicensed use in US/Canada | 433.05-434.79 is the 70 cm **amateur** band. Unlicensed use is limited to low-power, intermittent Part 15.231 / RSS-210-type devices, so a continuous illuminator isn't allowed. With an amateur licence it is usable, with station-ID rules. [I] | **Not an ISM band here.** It's the EU SRD band; don't transmit on it in North America. [I] | **ISM band**, Part 15.247 / RSS-247, with power and dwell limits. [I] |
| Contiguous span for stepped-frequency sweeps (§3.2) | ~1.7 MHz | ~7 MHz (EU) | **26 MHz** |
| Hardware we have | none | none | TEL0122 cover |

- **433 MHz** goes through walls best and lights up the widest zone. That is useful for "is anyone in the house". But breathing sensitivity halves, antennas double in size, and the legal route needs a ham licence. Worth it only for through-several-walls presence with a licensed operator. [I]
- **433 MHz at a low duty cycle.** FCC 15.231(e) does allow any kind of transmission, but each burst is capped at **1 s**, followed by a silent period of **at least 30× the burst length and never less than 10 s**. The limit is 4,383 µV/m average at 3 m, about −22 dBm EIRP (43,833 µV/m peak). ([eCFR via Cornell](https://www.law.cornell.edu/cfr/text/47/15.231)) [V for the limits; EIRP is arithmetic]
  - At most that allows a 1 s look every 31 s, or a 0.33 s look every 10 s. A breathing cycle lasts 2-10 s, so it can't be measured this way. Presence and motion snapshots ("is this 1 s window steadier or busier than baseline?") are possible.
  - Canada's RSS-210 has a similar provision; check its current text.
  - Unlicensed 433 therefore means a slow presence probe only. [I]
- **868 MHz** is physically almost the same as 915. It's the right answer in Europe and the wrong one here. [I]
- **All three** are within the RTL-SDR's range, so the receiver doesn't decide it. [V, datasheet coverage]
- **Check the regulations before a long unattended run.** The legal column is inference from the band plans, not a reading of the current FCC/ISED text.

## 2. Why this needs an SDR

- **The SX127x only reports summary numbers.** Per packet you get RSSI and SNR, and in FSK mode you can poll a live RSSI register. There is no channel phase and no raw I/Q. That is enough for presence and motion (RSSI jumps as people move) and not enough for reliable breathing. The SX1262 has the same limit. [I]
- **So the split is:** the LoRa board transmits (the "illuminator"), and the RTL-SDR records raw I/Q. That is also how the research systems were built: commodity LoRa transmitters, with the signal captured raw on the receive side. [I]

### The RTL-SDR as receiver

| Property | Value | Source |
|---|---|---|
| Tuner / ADC | R828D (V4) or R820T2 (V3) / RTL2832U, **8-bit** | [RTL-SDR V4 datasheet](https://www.rtl-sdr.com/wp-content/uploads/2024/12/RTLSDR_V4_Datasheet_V_1_0.pdf) [V] |
| Coverage | 500 kHz to 1.766 GHz (V4), so 902-928 MHz is covered | datasheet [V] |
| Bandwidth | 2.56 MHz stable, up to 3.2 MHz with drops | datasheet [V] |
| Clock | 1 ppm TCXO, about ±915 Hz at 915 MHz | datasheet [V] |
| Current | 250-270 mA | datasheet [V] |
| LoRa decoding | `gr-lora` / `gr-lora_sdr` decode LoRa from an RTL-SDR (GNU Radio) and correct about ±15 kHz of offset | [gr-lora wiki](https://github.com/rpp0/gr-lora/wiki/Capturing-LoRa-signals-using-an-RTL-SDR-device), [CNX](https://www.cnx-software.com/2023/08/23/gr-lora_sdr-a-gnu-radio-sdr-implementation-of-a-lora-transceiver/) [V] |

Which RTL-SDR version we have is still to confirm. Run `rtl_test` on the Pi 5, which prints the tuner. Everything below works on V3 or V4.

### Signal chain (proposed) [I]

```
FireBeetle + TEL0122 (SX127x)          RTL-SDR on Pi 5
  TX: CW carrier, or back-to-back      RX: 915 MHz, 1.0 MS/s cu8
  SF7/BW125 chirps, lowest power       │
          ~~~ 915 MHz ~~~>             ├─ dechirp: multiply by conj(upchirp), FFT
                                       ├─ per chirp: complex value at the peak bin
                                       │    (~977 samples/s at SF7/BW125)
                                       ├─ CFO removal (estimate from preamble / tone)
                                       ├─ |H| stream  (CFO-invariant)
                                       ├─ band-pass: motion 0.5-3 Hz, breathing 0.1-0.5 Hz
                                       └─ events: presence / motion / breathing-rate + confidence
```

- **Sample rate.** At 1.0 MS/s one SF7/125 kHz symbol (1.024 ms) is exactly 1,024 samples, and 1 MS/s is inside the RTL's valid ranges. The raw stream is 2 MB/s, about 7 GB/hour, so dechirp on the Pi in real time and keep only the per-chirp complex values (about 8 KB/s). Keep short raw captures for debugging only.
- **Carrier-frequency offset (CFO) is the main problem.** The transmitter's crystal and the RTL's TCXO are independent. The whole received signal rotates at the offset, typically a few kHz, and each packet starts at a random phase. Three ways around it:
  1. **Amplitude only, `|H|`.** The received signal is a static part (direct path and walls) plus a dynamic part (the body). Its magnitude changes as the body moves, and the offset does not affect magnitude. This works with one RTL-SDR, today. The catch is blind spots: when the static and dynamic parts are near-orthogonal, sensitivity drops. The papers deal with this. **Start here.**
  2. **Two coherent receive channels, `H1·conj(H2)`.** Both channels see the same offset and phase noise, so the conjugate product cancels them and leaves the phase difference. This is the dual-antenna approach in Zhang et al. It needs a shared clock: two RTL-SDRs with a clock-sharing mod, or a KrakenSDR (a coherent multi-channel RTL-based receiver). **This is the upgrade path** if amplitude-only breathing is too weak.
  3. **Continuous transmission.** A continuous carrier or back-to-back chirps keeps the phase continuous within a session. The residual offset then shows up as a slow ramp that can be detrended. It helps option 1 and does not replace option 2.
- **Transmitter mode.** Start with a plain continuous-wave carrier, which is the easiest to process. Move to LoRa chirps when range matters: dechirping gives processing gain, and chirp concentration is where the papers' range came from.
- **Gain and clipping.** The direct path is strong and the ADC is 8-bit (about 45 dB usable dynamic range). Set gain manually, and place the antennas so the direct path does not saturate the ADC. Otherwise the small body-reflected part is lost to quantization.

### Hardware and regulatory notes [I]

- **Which FireBeetle the TEL0122 fits.** The DFRobot wiki page for the TEL0122 returned 404 (2026-09-30), so the pin mapping is unverified. The cover was made for the original FireBeetle ESP32 header. Check the header on whichever mainboard it goes on; the FireBeetle 2 ESP32-C5 may not match. Any SX127x/SX1262 breakout on SPI with RadioLib works as the transmitter.
- **Transmit rules.** In the US and Canada, 902-928 MHz falls under FCC Part 15 and ISED RSS-247/RSS-210. A continuous carrier or long single-channel chirp trains at full power can break dwell-time and power limits. Use the lowest transmit power that works (the link is only metres long), keep sessions short, and check the rules before any long unattended run.
- **Nothing here goes underwater.** 915 MHz does not propagate in water. On the sonobuoy the same board is only the surface data link.

## 3. Fusion with the rest of the sensor set

### 3.1 What each sensor contributes

| Modality | Hardware | Senses | Honest range | Walls | Rate | Fusion role |
|---|---|---|---|---|---|---|
| LoRa bistatic | TEL0122 TX + RTL-SDR RX | presence, motion, breathing (coarse); no heart rate | 10-25 m (papers); lower expected with 8-bit single channel [I] | yes | ~1 kHz chirp samples | **Wide-area tripwire**: always on, long range, through walls |
| WiFi CSI | RuView ESP32-S3 nodes | presence, activity, pose (trained), breathing ±2-3 BPM | room scale | 1 wall, weak | 20-100 Hz | Room-scale activity and pose |
| 24 GHz FMCW | Seeed MR24BSD1 (discontinued) | breathing, sleep state, movement | 2.75 m, line of sight | no | ~1 Hz reports | Bedside ground truth |
| 60 GHz FMCW | Seeed MR60BHA2 (RuView's reference) | breathing ±0.5 BPM, heart rate ±1-2 BPM | 1-3 m cone | drywall only | ~1 Hz | **Vitals ground truth** |
| RGB-D ToF | DFRobot SEN0583 | depth 320×240 + RGB, 30 fps | 0.2-2 m indoors | no | 30 Hz | Position and occupancy ground truth for calibration; splat input |
| GNSS | DFR1103 / TEL0132 | node position ±2-2.5 m, UTC | outdoors | n/a | 1 Hz | Link geometry for outdoor links |

The WiFi CSI and 60 GHz accuracy figures are RuView's design-intent claims (RuView ADR-063 table), not our measurements. [V as a claim]

### 3.2 Four ways to fuse, in the order to try them

**1. Geometry: each link is a sensing volume.** [I]
Each TX→RX link sees motion inside a Fresnel ellipsoid, a volume we can compute from the two antenna positions (tape measure indoors, GNSS outdoors). That makes a LoRa detection a spatial fact: something is moving inside ellipsoid E. Add a few transmitters on different channels or time slots to one RTL-SDR receiver, and the overlapping ellipsoids give a coarse location without any time-of-flight measurement. This fits WeftOS's spatial model directly: an ellipsoid is a BVH volume (ADR-056/078), so link detections can be joined against rooms, objects and other sensors' detections spatially.

**2. Decision-level fusion inside a Graph View.** [I]
Following the operational model in [graph-views.md](graph-views.md) §4b:

- F1: create a View for the purpose (for example, a house-occupancy View).
- F2: bind the room and region geometry, including the link ellipsoids.
- F3: attach live sources, each emitting `{t, volume, kind, value, confidence}` events: LoRa link events, CSI presence/activity, mmWave vitals, ToF occupancy.
- F6: hot fusion adds co-location edges when detections from different sensors overlap in space and time.
- F9: stable occupancy and identity components get promoted into the world model.

Each sensor stays independent, and a missing sensor degrades the View rather than breaking it. Do this first.

**3. Cross-calibration: precise sensors label the wide one.** [I]
This is RuView ADR-063's "ground-truth calibration" pattern, applied to LoRa. When the 60 GHz or 24 GHz radar reports a breathing rate, or the ToF camera sees a person at a known spot, log the LoRa features at the same moment as labelled data. Use it to tune the band-pass filters, detection thresholds and blind-spot handling. The precise sensors cover a small cone, the LoRa link covers the house, and the calibration carries the precision outward.

**4. Signal-level: the RuView unified encoder (later, maybe).** [V for the contract; I for the fit]
RuView ADR-274 (Accepted, P1 implemented) normalizes every RF modality into one `RfTensor`: complex links × 56 bins × 8 snapshots, with per-link geometry, `clock_quality` and `uncertainty`. Adapters live in a **fail-closed** `AdapterRegistry`; the reference adapters are `esp32s3-csi`, `mr60bha2`, `dw3000` and `oai-srs-xapp`. A LoRa link is narrowband, one useful bin, and stretching one bin to 56 would be fake structure. Two honest options:

- **Frequency hopping.** Retune the transmitter and the RTL across 902-928 MHz channels to build a stepped-frequency sweep. That fills real bins, and a 26 MHz span gives about 38 ns (11.5 m path-length) delay resolution. RTL retune time limits the sweep rate. Research idea, unproven.
- **Keep LoRa out of the encoder** and fuse at decision level (option 2). This is the default until a LoRa adapter is justified.

### 3.3 Time alignment

- **Fusion** works on breathing and motion timescales (0.1-3 Hz), so aligning events to within about 10-50 ms is plenty. The mesh heartbeat sync designed in `.planning/development_notes/mesh-time-sync.md` (about 100 µs on a LAN) is more than enough. Timestamp each chirp sample from its sample index, anchored to host monotonic time at stream start, and watch for RTL sample drops. Drops are rare at 1 MS/s and common near 3.2 MS/s.
- **Sonar** ranging is a different problem that needs microseconds (GNSS 1PPS). Don't let the fusion clock design stand in for it.

### 3.4 Always-on tripwire, then wake the others (the sentry use case) [I]

The LoRa link is the cheapest always-on, whole-house sensor: one receiver on the Pi 5 plus a couple of transmitter boards, working through walls. When a link fires, wake the higher-cost sensors in that volume: the CSI node's tier-2 processing, the mmWave radar, the ToF camera. The RTL-SDR's 250-270 mA means this runs on the Pi's mains power; it is not a battery-node pattern.

### 3.5 How nodes would advertise it (ADR-099)

ADR-099 lets nodes advertise experimental capability ids with an `x.` prefix. The vocabulary file doesn't gate them. Proposed ids:

- `x.radio.iq.rtl-sdr`: attrs `tuner`, `freq_mhz[]`, `max_msps`, `tcxo_ppm`, `coherent_channels`
- `x.radio.tx.lora`: attrs `chip` (sx127x | sx126x), `band_mhz`, `modes[]` (lora | fsk | cw)
- `x.sense.rf.bistatic-link`: a derived capability, needing one of each plus measured antenna positions

No code needed until a capture workload asks for them.

## 4. Experiment plan

| Step | What | Pass condition |
|---|---|---|
| E0 | `rtl_test` on the Pi 5 (identify V3/V4 and tuner); choose the FireBeetle mainboard for TEL0122 and confirm pins; flash RadioLib CW/LoRa transmit at lowest power | tuner identified; carrier visible in a spectrogram (`rtl_power` or gqrx) |
| E1 | 60 s capture at 915 MHz, 1.0 MS/s; measure CFO; check for clipping | CFO estimated and stable over 60 s; ADC not saturated |
| E2 | Empty room, 10 min, amplitude-only stream | noise floor recorded: `|H|` std per 1 s window |
| E3 | Walk across the link at 3, 5 and 10 m from the receiver | motion detected on each crossing; false alarms logged against E2 |
| E4 | Seated person 1, 3 and 5 m from the link, 2 min each; breathing from the 0.1-0.5 Hz peak | within ±1 BPM of reference (MR24BSD1/MR60BHA2, or counted breaths) in ≥80% of 30 s windows at 3 m |
| E5 | E4 again with one interior wall in the path | result recorded; this is the point of sub-GHz |
| E6 | Run beside one RuView CSI node, both host-timestamped, into one log | agreement matrix for presence/motion between LoRa and CSI |
| E7 (if E4/E5 are weak) | Two coherent channels (clock-shared RTLs or KrakenSDR), `H1·conj(H2)` | measurable breathing improvement over amplitude-only |
| E7' (alternative) | AD9361 B210-class board: transmit and receive on **one clock** (no CFO at all), 2 coherent RX, 12-bit ADC, 26 MHz of the 915 band in one capture | coherent phase breathing; delay-resolved links |

**B210-class upgrade (AD9361 clones, typically $150-350, not $20).** [I]

| | RTL-SDR V3/V4 | AD9361 B210 clone |
|---|---|---|
| ADC | 8-bit, about 45 dB usable | 12-bit, about 70 dB |
| Bandwidth | 2.56 MHz | up to 56 MHz |
| Frequency | 0.5 MHz-1.766 GHz | 70 MHz-6 GHz (covers 433/915 and both Wi-Fi bands) |
| Coherent RX channels | 1 | 2 (2×2 MIMO) |
| Transmit | no | yes, full duplex, same clock as RX |

- **The transmit side is the big win.** Transmit and receive share one oscillator, so the carrier offset from §2 disappears. You get real coherent phase, and it can run as a small FMCW/chirp radar.
- **Clone risks to check before buying:**
  - an AD9363 relabelled as an AD9361 (the AD9363 covers only 325 MHz-3.8 GHz with 20 MHz bandwidth)
  - Kintex-7 (XC7K325T) variants need the vendor's FPGA image and UHD build, not stock Ettus images
- **USB 3 vs the Pi 5.** 56 MS/s on two channels is beyond what USB 3 and the Pi 5 can carry. Sensing needs only 2-10 MS/s, which is fine.

Put the capture and dechirp code in `scripts/` for the first pass. Move it to a Rust crate or cog only after E4 passes.

## 5. Open questions

1. Which RTL-SDR is on the bench (V3/V4)? It changes the tuner, not the plan.
2. Which FireBeetle mainboard will carry the TEL0122, and do its pins match?
3. Is a second coherent receive channel worth buying before E4 results are in? Recommendation: no, measure amplitude-only first.
4. What transmit power and duty cycle keep a long unattended sentry run inside FCC/ISED rules?
5. Is LoRa worth an adapter in RuView's registry (ADR-274), or is decision-level fusion enough? Decide after E6.

## Sources

- Zhang et al., "Exploring LoRa for Long-Range Through-Wall Sensing", IMWUT 2020: [HAL](https://hal.archives-ouvertes.fr/hal-03363358), [PKU news](https://www.sei.pku.edu.cn/info/1036/1394.htm)
- "Unlocking the beamforming potential of LoRa for long-range multi-target respiration sensing": [HAL](https://hal.archives-ouvertes.fr/hal-03363386)
- WideSee, SenSys 2019: [White Rose](https://eprints.whiterose.ac.uk/149652)
- RTL-SDR Blog V4 datasheet: [rtl-sdr.com](https://www.rtl-sdr.com/wp-content/uploads/2024/12/RTLSDR_V4_Datasheet_V_1_0.pdf)
- gr-lora on RTL-SDR: [wiki](https://github.com/rpp0/gr-lora/wiki/Capturing-LoRa-signals-using-an-RTL-SDR-device); gr-lora_sdr: [CNX](https://www.cnx-software.com/2023/08/23/gr-lora_sdr-a-gnu-radio-sdr-implementation-of-a-lora-transceiver/)
- RuView ADR-063 (mmWave + CSI fusion, Proposed) and ADR-274 (RF encoder and adapter registry, Accepted, P1 implemented), read through the RuvNet brain corpus 2026-09-30
- WeftOS: [graph-views.md](graph-views.md), ADR-099, ADR-078, `.planning/development_notes/mesh-time-sync.md`
