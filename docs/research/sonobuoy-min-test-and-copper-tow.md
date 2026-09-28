# Minimum buoy test + copper-first towline

**Date:** 2026-09-25  
**Status:** Planning note (not an ADR)  
**Companions:** `.planning/sonobuoy/build/{roadmap,lake-test-protocol,build-fleet-density,build-tethered-subsurface}.md`, `RANGING.md`, `docs/research/pzsdr-p047-and-fiber-towline.md`, **literature ground:** `docs/research/copper-bus-timing-grounding/`

Fiber can wait. First good test is **three surface buoys** and a **short analog copper line**.

---

## 1. Minimum buoys for a *good* test

| Test | Buoys | What it proves | What it cannot prove |
|------|-------|----------------|----------------------|
| Bench air loop | 0 (two discs, ~30 cm) | TX→RX→demod→chain | Water, geometry |
| Phase 1 pool | **1** Class A | Seal, self-noise, T60, chain in water | Ranging, TDoA |
| Two-buoy presence | **2** Class A | Acoustic + WiFi link, one range | 2D fix (ambiguous) |
| **First good mesh** | **3** Class A | Every node hears the other two; 2D triangle | Overdetermined residuals, density scaling |
| First *good localization* | 3 Class A + **2 Class B on one buoy** (−1 m / −2 m) | Horizontal triangle + one vertical TDoA | Full 3-class lake protocol |
| Phase 2 lake (later) | 3 A + 6 B + 5–10 C | Joint solver vs diver truth | — |

**Do not start at 9–30 nodes.** Fleet-density (Phase 1c) is a *scaling* experiment after 3-buoy presence works.

**Why 3:** two nodes give a distance, not a plane. Three non-collinear nodes are the first geometry the spatial branch and `clawft-sonobuoy-ranging` `D(t)` matrix can actually use. Four is nicer (overdetermined) but not required for a good first test. Otero-style 4-buoy GNSS ranging is v2.

**Skip for v1 test:** CSAC, JANUS full stack, P079 imaging, 20× Class C.

---

## 2. Distances

Sound speed ~1480 m/s. Mesh chirp in the build is **1.8 kHz** (λ ≈ **0.82 m**).

| Venue | Baseline | Why |
|-------|----------|-----|
| Bench (air) | 0.3 m | Roadmap Phase 0 |
| Pool, 3 buoys | **5–12 m** triangle, ≥2 m from walls | Reference pool 25×12×2 m; T60 200–500 ms at 1.8 kHz; stay out of the worst reverb corners |
| **Lake, first good** | **15–30 m** equilateral, water **5–15 m** deep | Lake protocol allows 10–50 m; 15–30 m is long enough that GPS 2–5 m error is visible *and* short enough that 1.8 kHz still has SNR without a projector upgrade |
| Do **not** yet | 100 m–5 km | That is `RANGING.md` production OWTT. Needs measured source level + absorption before you stretch |

Guard time: `≥ 1.5 × measured T60`, minimum 200 ms (pool). Lake T60 is shorter; still measure.

If the 1.8 kHz discs cannot close 30 m in the lake, drop to **10–15 m** and log SNR vs range — that *is* the test, not a failure.

---

## 3. Copper-first towline (cheaper, faster)

Do **not** wait on FBG / DAS / RFSoC. Analog copper is how every lab array starts. Fiber (or fiber *plus* copper) layers into the same hose later.

### Minimum copper line that is worth building

| Item | v1 pick |
|------|---------|
| Elements | **4** JFET hydrophones (8 if the first 4 are quiet) |
| Spacing | **0.40–0.45 m** (≈ λ/2 at 1.8 kHz) |
| Acoustic section | ~1.6–3.2 m |
| Lead-in / tow | **8–15 m** of slack + strain relief |
| Cable | 4× twisted pair: **CAT6 outdoor** or 8-conductor marine (already in Class B thinking) |
| Strength | **Separate** Dyneema/Kevlar/paracord; copper must not take tow load |
| Jacket | Cheap PU garden hose or lay-flat; oil-fill optional later |
| Preamps | JFET at each element (same as oil sidecar). At **&lt;20 m** a pair per channel is enough |
| ADC | USB audio (Focusrite-class, 4–8 in) **or** one ESP32-S3 per 2 ch — **not** P047 |
| Head | Same shore laptop as the buoy WiFi host |

4 elements at λ/2 is the first line that can **beamform** (left/right ambiguity until you add a second line or a heading sensor). 2 elements is only a phase interferometer; skip it.

### Why copper is enough now

- Class B already specified **4-conductor marine cable, 1–3 m**. This is that idea, longer and multi-drop.
- No FORJ, no laser, no interrogator, no $8.7k RFSoC.
- EMI and capacitance show up as *measurable* noise vs length — useful data before you pay for fiber.
- Channel count of 4–8 matches a cheap audio interface. Hundreds of channels is why fiber exists; you do not have that problem yet.

### Limits (accept, then layer)

| Copper limit | When it actually bites |
|--------------|------------------------|
| Capacitance / HF roll-off | Well past 20 m without per-element constant-current (IEPE) |
| Channel count | One pair per hydrophone; 16+ pairs is a fat hose |
| EMI / galvanic | Near engines, long seawater runs |
| Weight / diameter | USV winch, km-class |

**Layer later:** pull 2–4 SM fibers in the same braid beside the copper (dark fiber until an interrogator exists). Strength member stays Kevlar. Do not rip out the analog line.

### Geometry vs the buoy field

```
Lake triangle:  3 Class A at 15–30 m
One copper tow: 4 phones, 0.4 m spacing, 10 m lead-in
                (kayak / dock / USV — not on a buoy)
```

The tow is a **linear aperture**. The three buoys are a **sparse volume**. First good test can be **buoys only**. Add the copper line in the same lake day if the 3-buoy mesh is already green.

---

## 4. Suggested sequence

1. Bench air loop (existing Phase 0).  
2. **1 buoy** pool.  
3. **3 buoys**, pool, **~8 m** triangle.  
4. Same 3, lake, **15–30 m**.  
5. Optional: 2 Class B under *one* buoy.  
6. Parallel, not blocking: **4-phone copper hose**, tank or dock, 10 m lead-in.  
7. Fiber / P047 only after (4) and (6) have numbers.

Cost ballpark (already in build docs): 3× Class A oil path is the Phase 1b fleet, on the order of a few hundred dollars, not thousands. Copper tow BOM is cable + 4 JFETs + cheap ADC — same order as one extra buoy.

---

## 5. ESP32 over CAT5/6/7 (packet network, not analog)

**No Wi‑Fi.** The 2.4 GHz radio stays **off** (it also jittered the RX ADC). Copper is the only backhaul: power, packets, and PPS on the same braid. WeftOS sits on the laptop and talks **USB‑CAN / USB‑Ethernet / USB‑485** to a shore gateway — never to an AP.

The S2/S3 **have no Ethernet MAC**. Pick a PHY below, or skip IP and use the §6 bus.

### Pick one PHY

| Path | Chip | Cable use | Length (practical) | When |
|------|------|-----------|--------------------|------|
| **A — 100BASE-TX (recommended)** | **W5500** SPI (~$3–8 module, magnetics on the RJ45) | 2 pairs data | ~30–80 m wet; 100 m spec dry | Same TCP/UDP publish as Wi‑Fi |
| B — native EMAC | **ESP32 (not S3)** + LAN8720 RMII | 2 pairs | same | Only if you switch MCU |
| C — single-pair Ethernet | ADIN1110 10BASE-T1L SPI | 1 pair | up to ~1 km | Later; extra cost |
| D — no IP | MAX3485 **RS‑485** on UART | 1 pair | hundreds of m | Simplest; need a USB‑485 dongle on the laptop |

Stay on **S3 + W5500** so firmware stays one family. Do not put an RJ45 in the water: gland the cable into the dry chamber and terminate on a jack **inside**.

### Pair map (T568B)

CAT5/6/7 = four twisted pairs. 100 Mbit Ethernet only needs two.

| Pair | Color | v1 job |
|------|-------|--------|
| 1 | orange | 100BASE-TX TX |
| 2 | green | 100BASE-TX RX |
| 3 | blue | **PoE** (or 12 V) to the buoy |
| 4 | brown | spare: analog hydrophone **or** RS‑485 debug |

CAT7 S/FTP (foil + braid) is the right jacket for a motor/USV. Bond the braid to chassis ground **at the dry end only** (single-point) so you do not loop seawater as an antenna.

**Power:** 802.3af injector on shore, cheap splitter at the buoy (48 V → 5 V). Or ignore PoE and keep the 18650; copper is data-only.

**Topology:** Ethernet is a **star** (one homerun per buoy to a dock switch). For **one hose, N nodes**, use the §6 bus instead — not three Ethernet runs.

### Firmware (esp-idf)

W5500 is a second `netif`. After DHCP (or static `192.168.4.x`), the existing substrate client uses that socket instead of `WIFI_STA`. Sketch:

```c
// esp-idf examples/ethernet/basic + SPI W5500
eth_mac_config_t mac_cfg = ETH_MAC_DEFAULT_CONFIG();
eth_phy_config_t phy_cfg = ETH_PHY_DEFAULT_CONFIG();
spi_bus_config_t bus = { .mosi_io_num = 11, .miso_io_num = 13, .sclk_io_num = 12 };
spi_device_interface_config_t dev = { .mode = 0, .clock_speed_hz = 20 * 1000 * 1000, .spics_io_num = 10 };
// eth_w5500 install → esp_netif_attach → DHCP
// then UDP/TCP publish of the same AcousticEvent bytes (no WIFI_STA)
```

embassy-net + `embassy-net-wiznet` is the Rust equivalent if the buoy stays embassy-rs.

WeftOS side: USB‑Ethernet into the laptop (or a $10 switch). Daemon unchanged. **No AP.**

### Bench before the lake

1. 10 m dry CAT6, W5500, ping the S3.  
2. Same cable in a bucket, glands only, ping still works.  
3. Then publish `pcm_chunk` / `acoustic.event` over that IP.

If Ethernet is flaky in water, fall back to **path D** (RS‑485) on the brown pair the same afternoon — UART at 115200, MAX3485 both ends, USB‑485 into the host, a 20-line serial-to-substrate shim. That is still “copper as network”; it is not Ethernet.

---

## 6. Cheap packetizer, or a custom power+bus

Two cheaper ideas than a W5500 TCP stack on every S3.

### 6.1 UART → packets (the ESP only prints a stream)

The S3 already emits a byte stream (framed `AcousticEvent` / `pcm_chunk`). A **$2–4 UART-to-Ethernet** chip wraps that in UDP/TCP. The MCU never runs lwIP.

| Part | Role | ~$ |
|------|------|----|
| **CH9121** (or CH9120) | UART in, 100BASE-TX UDP client out | $2–4 |
| USR-C216 / similar | same, more config UI | $5–8 |
| ENC28J60 | cheapest SPI Ethernet; painful software — skip | $1–2 |

Config once: UDP to `shore:port`, 115200 8N1. ESP `uart_write` of COBS frames. Shore `socat`/tiny shim → substrate. CAT5 pair map from §5 still applies.

### 6.2 Custom jacket: power + common bus (best fit for “each one”)

Ethernet is a **star** (one homerun per buoy). A **bus** is one CAT5/7 braid, many nodes, power on the same hose. The S3 already has the interesting bit: **TWAI (CAN 2.0)** on-chip. Transceiver is ~$1.

```
shore 12–24 V ── CAT5 braid ──┬── buoy 1  ESP32-S3 + SN65HVD230 + buck 5 V
                              ├── buoy 2  same
                              └── buoy 3  same
pairs:  orange  CAN_H / CAN_L     (data, 125–500 kbit/s)
        blue    +VIN / GND        (power; fuse per node)
        green   optional PPS / TX_ACTIVE sync
        brown   spare analog or second RS-485
```

| Bus | Silicon | Why |
|-----|---------|-----|
| **CAN / TWAI (pick this)** | SN65HVD230 (~$1) | Arbitration, 29-bit IDs = `buoy_id`, 1 Mbps class, wet-tolerant differential |
| RS-485 poll | MAX3485 (~$0.50) | Even cheaper; **master must poll** or they collide |
| I²C | — | **No** — Class B 1–3 m only |

**Framing (same for CAN or 485):** COBS or 8-byte CAN frames. Event path is a few frames (peak, t_us, snr). PCM is optional, polled in 256-sample slices so one node cannot hog the bus. Node 0 on shore is the gateway: USB-CAN (`slcan` / candleLight ~$10) or a spare S3 with TWAI → **USB serial/ETH into WeftOS**. No radio.

**Do not** put 16 kHz continuous PCM from three buoys on 125 kbit CAN. Events yes; waveform no (or one buoy at a time).

**Power:** 12–24 V on blue pair, buck to 3.3/5 V at each node, TVS + polyfuse, common GND. Isolated DC-DC later if galvanic noise shows up. This replaces PoE injectors.

**Towline variant:** same bus, nodes = hydrophone pods along the hose (address 1…N), head ESP32 is the gateway. Analog v1 can stay on brown; bus is how the pods *talk* if you put an S3 at each phone.

### 6.3 What to build first

1. Three S3s on a **dry** CAT6, TWAI + 12 V, poll `AcousticEvent`.  
2. Same in a bucket, glands only.  
3. Lake: **one braid is the whole network** (power + CAN + PPS). Wi‑Fi never on.

CH9121 is the lazy IP path (still copper-only). **Power + CAN + PPS on CAT7** is the custom interface — cheapest per node, one hose, WeftOS only talks to the USB gateway.

### 6.4 LoRa on a pair — possible, usually the wrong PHY

You can couple an **SX1262/SX1276 RF port into a twisted pair** (4:1 balun, 50 Ω → 100 Ω CAT5) instead of an antenna. Mines do this as leaky feeder. On 15–30 m of CAT7 it will “work”: huge SNR, long cable, ESP still talks SPI to the LoRa chip.

It is a **slow ALOHA modem**, not a bus:

| | LoRa-on-copper | CAN + PPS |
|--|----------------|-----------|
| Rate | ~0.3–5 kbit/s | 125–1000 kbit/s |
| PCM | no | polled slices only |
| Addressing | preamble / sync word | hardware IDs + arbitration |
| Timing | symbol is 10–100 ms — **cannot be PPS** | green pair still required |
| TX power | must **pad/attenuate** or you saturate the next node | n/a |

Use LoRa-on-wire only if you already have modules and a km-class drop. For the 3-buoy / short-tow test, **do not**. Keep LoRa in the lake protocol as an *optional long-range radio later*; it does not replace the copper bus or the sync pair.

Shore would be a USB LoRa dongle — still no Wi‑Fi, still the wrong bitrate.

---

## 7. Wired time (and what position the wires actually give)

The architecture note called clock sync the hard problem: TDoA wants **~10 µs** (~1.5 cm in water) or you fall back to acoustic TWTT (no shared clock). **A dedicated sync pair on the same CAT5/7 braid is that clock.** Do not timestamp off CAN frames — arbitration jitter is milliseconds under load.

### 7.1 How time rides the hose

| Pair | Job |
|------|-----|
| green | **PPS / strobe** — RS-422 (two wires, differential) or open-drain + local pull-ups |
| orange | CAN: `pps_seq`, detections, delay-cal reports |
| blue | power |
| brown | analog / spare |

**Master:** one GPS PPS on shore (or a docked buoy with sky), or an S3 hardware timer if you only need *relative* time. Fan the edge down green.

**Slaves:** ESP32 **MCPWM / GPIO capture** on that edge (not an Arduino `ISR` + `micros()`). Latch `local_us` + `pps_seq`. Acoustic detections then publish `{t_pps, dt_us, peak}`. Shore subtracts the known cable delay.

Propagation on CAT5 is ~**5 ns/m** (VF ≈ 0.66). Thirty metres is **~150 ns** ≈ 0.2 mm of sound travel — below the hydrophone. Still **measure** it once: master fires a cal pulse, each node reports capture time, store `delay_ns[node]` (or TDR). Recal if you recut the hose.

That **replaces CSAC, acoustic TWTT, and Wi‑Fi** for the wired set. Detections go out on **CAN** with `{t_pps, dt_us, peak}`. Acoustic in water is for *listening* (and for any node not on the braid), not for the backhaul.

### 7.2 Position — yes, but only the geometry the wires actually constrain

| Setup | What the copper gives | What it does not |
|-------|----------------------|------------------|
| **Towline**, nodes along one hose | 1-D station: `s = v_prop · τ/2` or simply *cut length / index × spacing*. Confirms array geometry for beamforming. | 3-D lake position of the USV |
| **Taut known-length links** (triangle of CAT7 between 3 buoys, or each buoy taut to a dock) | **Baselines** — same role as a tape measure. Flip ambiguity until a depth or a fourth point. | Slack line (only an upper bound) |
| **Slack floating backbone** | Time + power + bus only | Not a survey |
| **Electrical TWTT on a star of homeruns** | Length of each drop | Buoy (x, y) in the pond |

So: **wired time is general; wired position is 1-D along the cable or a taut truss.** Drifting free buoys still need GPS and/or acoustic ranging. A **towed copper array** gets *both* time and element position from the hose. A **3-buoy taut triangle** gets relative geometry without OWTT; one GPS on the master georeferences the triangle.

### 7.3 Solver picture (WeftOS shore)

```
pps_seq + dt_us + delay_ns[i]  →  common time
cable length or τ_copper       →  array station / taut baseline
acoustic TDoA in that frame    →  bearing / source (x,y) in the triangle
one GPS on master              →  lat/lon of the whole figure
```

`clawft-sonobuoy-ranging` still consumes a `D(t)` matrix. Wired taut edges are **exact** `D_ij = L_cable` (with sag model if you care). Acoustic fills the rest.

### 7.4 Do not

- Use CAN SOF as PPS.
- Assume 16 kHz PCM timestamps from USB jitter.
- Claim slack CAT7 locates a drifting field.
- Skip delay cal after you change the reel.
