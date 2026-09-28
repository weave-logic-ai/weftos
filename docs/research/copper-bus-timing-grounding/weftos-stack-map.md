# Copper bus → WeftOS stack map

**Date:** 2026-09-25  
**Status:** Planning note (not an ADR; **not a crate**)  
**Companions:** [`../sonobuoy-min-test-and-copper-tow.md`](../sonobuoy-min-test-and-copper-tow.md), [`.planning/sonobuoy/build/architecture.md`](../../../.planning/sonobuoy/build/architecture.md), [`.planning/sonobuoy/RANGING.md`](../../../.planning/sonobuoy/RANGING.md) §0 + §2.4, [`JOURNALED-NODE-ESP32.md`](../../../.planning/sensors/JOURNALED-NODE-ESP32.md), [`../sonobuoy-ranging-scaffold.md`](../sonobuoy-ranging-scaffold.md), [`../pzsdr-p047-and-fiber-towline.md`](../pzsdr-p047-and-fiber-towline.md)

**ADR-087** (K-STEMIT dual-branch) is **Proposed only**. This note does not treat it as shipped and does not rewrite it.

This is a **plan ground**: how the copper-only 3-buoy / taut-triangle / CAN+PPS hose maps onto crates and contracts that already exist. **Buoy firmware is not shipped.** There is no TWAI PHY in `clawft-rpc`, no USB-CAN gateway crate, and no embassy sonobuoy image. Do not read this file as a delivery receipt.

---

## 0. What already exists vs what this plan needs

| Piece | In-tree today | Copper-v1 needs |
|-------|---------------|-----------------|
| `weftos-sensor-pipeline` + `-wire` | Collect → aggregate → encode → Ed25519 `mesh.sensor.v1.*` (WEFT-523/526). Host-side, tokio. | USB-CAN (or USB-485) **gateway Node** feeds `RawSample`s; shore signs `mesh.sensor.v1.encoded`. |
| `clawft-kernel` `mesh_sensor.rs` | Registers `mesh.sensor.v1.{encoded,consensus,control}` on the mesh wire. | Unchanged. Gateway is just another publisher on the local UDS. |
| `clawft-rpc` | Unix domain socket (`kernel.sock`) / Windows named pipe. **No copper PHY.** | Shore shim: slcan / candleLight / USB-485 → UDS. Never AP/STA. |
| `clawft-sonobuoy-ranging` | WEFT-535 scaffold: `BuoyId`, `RangeObservation`, `DistanceMatrix`, `owtt_range_m`. No mesh deps. | Taut `D_ij = L_cable` via `DistanceMatrix::set`. Acoustic TDoA fills *source* geometry, not the taut edges. |
| JOURNALED-NODE-ESP32 | Draft: ed25519 Node ID, `n-<6hex>`, write-gate `substrate/<publisher-node-id>/`. Hardware profile still lists 2.4 GHz Wi‑Fi + BLE. | **Radio OFF.** Identity and write-gate **stay**. Transport becomes CAN (or 485) through the gateway. |
| JOURNALED-SENSOR-MIC | `summary` + `pcm_chunk` sibling split; hydrophone is the acoustic special case (architecture.md ADR-081 draft). | Events on the bus; PCM polled or USB-audio on the analog tow. Not 16 kHz × 3 on 125 kbit CAN. |
| `clawft-edge-bench` / `clawft-edge-pad` | ESP32-S3 embassy **bench** and LCD pad. Hard-coded `KERNEL_HOST` / Wi‑Fi DHCP. | **Not** sonobuoy firmware. Do not pretend they publish `AcousticEvent`. |
| ADR-087 | Proposed (Candidate). Dual-branch GNN over array graphs. | Later consumer of `D(t)` + detections. Not in the copper critical path. |

RANGING.md §10 proposed an “ADR-078” for OWTT ranging. Production **ADR-078 is splat → world model**, already Accepted. Ranging remains the WEFT-535 scaffold + `.planning/sonobuoy/RANGING.md`. This note does not mint a new ADR.

---

## 1. Layer picture (copper-only)

```
3 Class A (ESP32-S3) ── CAT5/7 braid ── shore gateway ── laptop WeftOS
   TWAI + SN65HVD230        orange  CAN_H/CAN_L
   MCPWM capture            green   PPS / RS-422 strobe
   buck 5 V                 blue    +VIN / GND
   radio OFF                brown   analog / spare

   embassy-rs / esp-hal     USB-CAN (slcan / candleLight)
   (NOT SHIPPED)            or USB-485 shim
                                   │
                                   ▼
                            clawft-rpc  →  ~/.clawft/kernel.sock
                                   │
              ┌────────────────────┼────────────────────┐
              ▼                    ▼                    ▼
   substrate/<n-esp>/     mesh.sensor.v1.*      clawft-sonobuoy-ranging
   sensor/acoustic/…      weftos-sensor-pipeline         D(t)
   (ESP32 ed25519)        (gateway Node signs)      taut L_cable + TDoA
```

WeftOS still does **not** run on the buoy (`architecture.md` “Why WeftOS does not run on the buoy”). Tokio stays on the laptop. The MCU speaks a compact framed stream; the host is the trusted compute surface.

Two copper jobs, do not merge them:

| Job | PHY | WeftOS seam |
|-----|-----|-------------|
| **3-buoy bus** | CAN + PPS + 12–24 V on one braid | Gateway Node + per-buoy signed paths |
| **4-phone analog tow** | JFET → USB audio (or one S3 ADC), separate Dyneema | `RawSample` `sensor_class = "audio"` / hydrophone; linear `D` from cut length |

Ethernet-star (W5500) and CH9121 UART-to-UDP are optional copper IP paths in the min-test note. The **preferred** 3-node hose is the §6.2 bus. LoRa-on-copper is the wrong PHY (slow ALOHA, cannot be PPS).

---

## 2. JOURNALED-NODE-ESP32 — radio OFF, identity unchanged

The contract that survives copper is **path identity**, not the radio.

- Each ESP32 still gets an **ed25519** keypair at provision (NVS-plain for first units).
- Node ID is still `n-<6hex>` = truncated BLAKE3 of the pubkey.
- Write-gate still: every substrate publish hits `substrate/<publisher-node-id>/` and is signed by **that** node’s key. Unsigned or wrong-key writes are rejected.

### 2.1 Two Nodes, not one mashed publisher

| Node | Who signs | What it may write |
|------|-----------|-------------------|
| **Each buoy ESP32** | Buoy key | `substrate/<n-buoy>/sensor/acoustic/{event,summary}`, optional polled `pcm_chunk`, `health`, `meta` |
| **USB gateway** | Gateway / daemon key | `substrate/<n-gw>/health` (bus stats, VIN, PPS lock, CAN error counters), `meta`. Also the **LeWM** `mesh.sensor.v1.*` frames (numeric `node_id: u64` in the wire crate). |

The gateway is a **transport Node**. It **forwards** buoy-signed envelopes; it does **not** re-sign them as itself. Re-signing would collapse three hydrophones into one path and break the write-gate.

`weftos-sensor-pipeline`’s `PipelineConfig.node_id: u64` is the **mesh.sensor.v1** integer, not `n-6hex`. Canonical journal identity remains the substrate path (architecture.md ADR-081 draft: `buoy_id: u8` is transit-time only). Map:

```
CAN 29-bit ID / buoy_id:u8  →  provisioned n-<6hex>  →  mesh u64 (gateway table)
```

Keep that table on the gateway Node (`meta` or `cluster/nodes/<n-id>`), not inside the ESP32 frame.

### 2.2 What the Wi‑Fi-shaped fields become

JOURNALED-NODE-ESP32 currently assumes STA + SNTP + `rssi_dbm`. Copper-v1 profile:

| Field / capability | Wi‑Fi journal | Copper-v1 |
|--------------------|---------------|-----------|
| `capabilities.radios` | `["wifi-2g4", "ble5"]` | `["can-twai"]` (BLE unused; 2.4 GHz **off**) |
| `health.rssi_dbm` | STA RSSI | omit or NaN; replace with `can_tx_err`, `can_rx_err`, `vin_mv`, `pps_locked` |
| `last_publish_ts` | SNTP wall | `pps_seq` + host GPS time at the gateway |
| `status` degraded | Wi‑Fi down | PPS unlock, bus-off, VIN brownout, delay-cal stale |

Friendly label (`kitchen-esp32` → `buoy-a`) stays a mutable `meta.label`. Identity does not move to CAN hardware IDs.

### 2.3 Firmware honesty

The journal’s “ESP32 firmware” pointer is `crates/clawft-edge-bench` (Wi‑Fi, flat paths). **There is no sonobuoy embassy image** that: loads NVS keys, captures MCPWM on the green pair, speaks TWAI COBS/`AcousticEvent`, and keeps `WIFI_STA` down.

Until that firmware exists, this section is a **binding for whoever writes it**, not a description of running code.

---

## 3. `weftos-sensor-pipeline` / `mesh.sensor.v1`

Shore host, not the S3.

```
CAN frames {t_pps, dt_us, peak, snr, self_tx}
        │  gateway unwraps, verifies ed25519
        ▼
RawSample { sensor_class: "acoustic", bytes, timestamp_ms, quality }
        │  SensorPipeline::collect  (quality = SNR gate)
        ▼
aggregate → encode (HashEncoder until a real latent exists)
        ▼
SignedSensorFrame  topic mesh.sensor.v1.encoded.<cluster>
        │  signed by gateway/daemon key
        ▼
clawft-kernel mesh_sensor  →  chain event mesh.sensor.v1.frame
```

### 3.1 What the pipeline is for

LeWM / world-model **observations** (≤ 2 KB encoded latent). It is **not** the TDoA clock and **not** the `D(t)` matrix.

| Stream | Publisher | Clock |
|--------|-----------|-------|
| `substrate/<n-buoy>/sensor/acoustic/event` | buoy key | `{t_pps, dt_us}` minus `delay_ns[i]` |
| `mesh.sensor.v1.encoded` | gateway Node | host `timestamp_ms` (millisecond, good enough for LeWM, **not** for ranging) |
| `substrate/_derived/acoustic/position/<source>` | shore Actor / daemon Node | solver epoch |

Do not timestamp TDoA off `mesh.sensor.v1` or USB PCM. Architecture already forbids trusting 16 kHz USB jitter; copper-tow §7.4 repeats it.

### 3.2 PCM vs events (CAN budget)

JOURNALED-SENSOR-MIC wants `pcm_chunk` beside `summary`. Copper-tow §6.2: **do not** put 16 kHz continuous PCM from three buoys on 125 kbit CAN.

| Payload | On the bus | Shore |
|---------|------------|-------|
| Detection / `AcousticEvent` | yes, few CAN frames | collect → pipeline + ranging |
| `pps_seq`, delay-cal | yes | gateway table `delay_ns[node]` |
| `pcm_chunk` | polled 256-sample slices, one node at a time — or never | analog tow uses USB audio instead |
| Health 5 s | yes | `health/sensor/acoustic` sibling |

Analog 4-phone tow skips CAN for samples: Focusrite-class USB audio → `RawSample` `audio` on the **same** laptop pipeline, different `sensor_class`. Element stations come from hose cut length, not from CAN IDs.

### 3.3 Gateway as the WeftOS-facing Node

`clawft-rpc` only speaks UDS/named pipe. The USB-CAN dongle is therefore a **host process** (20-line slcan shim, or a spare S3 that bridges TWAI → USB serial). That process:

1. Is provisioned as a Node (`n-gw`).
2. Publishes its own health (link up, bus-off, PPS present).
3. Forwards buoy-signed substrate writes without changing the path prefix.
4. Owns one `SensorPipeline` instance whose `node_id` is the gateway’s mesh integer — LeWM sees **one** encoded stream per cluster, with buoy identity inside the payload / substrate paths, not as three separate tokio pipelines on the S3.

---

## 4. `clawft-sonobuoy-ranging` — taut `L_cable` as exact edges

Scaffold (`crates/clawft-sonobuoy-ranging/src/lib.rs`):

- `DistanceMatrix` is dense `R^{N×N}` metres, diagonal 0, missing pairs `None`.
- `set(from, to, range_m, symmetric)` already accepts **any** metre value.
- `RangeObservation` / `owtt_range_m` are `r = c · τ` helpers for **acoustic** travel time.

Copper-v1 does **not** need a new type to start. Taut CAT7 triangle:

```text
D_ij = L_cable_ij          # tape / cut length, sag model optional
σ_ij ≈ tape error + sag    # RangeObservation.sigma_m; matrix cells are still Option<f64>
```

Call `DistanceMatrix::set(&a, &b, l_cable_m, true)` at deploy (and after a recut). Do **not** run those metres through `owtt_range_m` — that helper is water, not copper VF.

| Edge kind | How it enters `D(t)` | What it locates |
|-----------|----------------------|-----------------|
| **Taut known-length CAT7** | exact `L_cable` | inter-buoy baselines (tape-measure). Flip until depth / 4th point. |
| **Towline stations** | `index × spacing` or `v_prop · τ/2` | 1-D element position along the hose |
| **Slack backbone** | **omit** (`None`) | time + power only; not a survey |
| **Acoustic TDoA** (common PPS frame) | not a `D_ij` of the buoys | **source** `(x,y)` inside the triangle |
| **Acoustic TWTT/OWTT** | `owtt_range_m` | free-drifting or off-braid nodes (later) |
| **One GPS on master** | not an edge | georeferences the whole figure |

Acoustic fill: detections `{t_pps, dt_us, peak}` from each RX, corrected by `delay_ns[i]`, give TDoA of a source (diver, projector, Class B at −1/−2 m). That is the first *good* localization the spatial branch can use. It is **not** how the three Class A locate *each other* when the hose is taut.

Scaffold gap (do not silently paper over): cells have no per-pair σ or edge-kind. `RangeObservation.sigma_m` exists but `ingest` drops it. A later ranging pass can tag `exact-cable` vs `owtt` vs `tdoa`; copper-v1 can keep a sidecar table on the gateway.

`synthetic_ring_field` remains a unit-test fixture. Lake geometry is an **equilateral 15–30 m taut triangle**, not a ring.

---

## 5. What BREAKS in `architecture.md` — and the replacement

The build architecture is a **Wi‑Fi-above-water + acoustic-below** system. Copper-only **keeps** embassy-on-buoy / WeftOS-on-shore, the S2/S3 acoustic chain, 1.8 kHz discs, and leading-edge matched-filter. It **breaks** every place the data plane assumed 2.4 GHz.

| Architecture.md assumption | Why it breaks | Copper-v1 replacement |
|----------------------------|---------------|------------------------|
| **Wi‑Fi gossip of TWTT timestamps** (`WiFi: report b_rx, b_tx`) | Radio is **off** (ADC jitter + no AP). No STA, no NTP, no ESP-NOW. | CAN frames `{t_pps, dt_us, peak}` after PPS capture. Shore already has both ends. |
| **`acoustic.twtt` as v1 ranging** | TWTT exists to avoid a shared clock. Wired PPS **is** the shared clock (~10 µs class after delay cal). | Drop TWTT for the **wired set**. Keep the stream schema only for off-braid / later OWTT. |
| **v2 TDMA schedule maintained over Wi‑Fi** | No Wi‑Fi control plane. | 3 nodes: ALOHA in water is still fine. Bus schedule = CAN arbitration + shore poll. TDMA in water, if any, is a table on the gateway, not `mesh_kad` gossip. |
| **SNTP `timestamp_wallclock_ms` on the MCU** | No Wi‑Fi, no SNTP. `micros()` ISRs are the wrong clock anyway. | MCPWM/GPIO capture of green-pair PPS. Publish `{t_pps, dt_us}`. Host binds `pps_seq` to GPS UTC. |
| **Antennas above the waterline as the backhaul** | True of 2.4 GHz in water — and now irrelevant, because the backhaul is the braid. | Gland CAT7 into the dry chamber. No mast AP. |
| **Wi‑Fi ISR as the reason to split TX S2 / RX S3** | Radio off removes that jitter source. Split still useful for self-TX isolation and TX_ACTIVE GPIO, not required to kill Wi‑Fi noise. | Keep the split if you already built it; a single S3 is acceptable for the 3-buoy copper test. TX_ACTIVE can ride the green pair as a strobe. |
| **Build README: “acoustic + Wi‑Fi nodes that gossip on WeftOS”** | Gossip was Wi‑Fi / ESP-NOW / mesh_kad. | Gossip is **substrate publish after the gateway**. WeftOS mesh is the laptop daemon, not the pond. |
| **Health `rssi_dbm` / radios `wifi-2g4`** | Lies if the radio is down. | Bus / PPS / VIN fields (§2.2). |
| **RANGING.md `mesh_kad.rs` TDMA gossip + LoRa cargo** | Production OWTT field protocol, not the 15–30 m test. | Out of scope for copper-v1. LoRa stays optional *later radio*, not a pair on the hose. |

### 5.1 Time sync: the architecture “hard problem” is solved *for the wired set*

Architecture § Time synchronization: TDoA wants ~10 µs (~1.5 cm in water) **or** TWTT without a shared clock.

| Clock plan | Where it still applies |
|------------|------------------------|
| **Green-pair PPS + `delay_ns[i]`** | All nodes on the braid (3 Class A, tow pods, Class B if cabled). Replaces CSAC, acoustic TWTT, and Wi‑Fi for that set. |
| **v1 TWTT over water + Wi‑Fi report** | **Broken** for copper-v1. Do not implement the Wi‑Fi half. |
| **RANGING.md OWTT + JANUS + CSAC + TSHL/D-Sync** | Free-drifting / km-class / GPS-denied. **Not** the 15–30 m taut test. |
| **One GPS PPS on shore (or docked master)** | Disciplines the green pair; georeferences the triangle. Pool: skip GPS, relative PPS from an S3 timer is enough. |

Do **not** use CAN SOF or frame timestamps as PPS (arbitration jitter is milliseconds under load). Propagation ~5 ns/m is below the hydrophone; still measure delay cal once per reel.

### 5.2 What does *not* break

- Embassy-rs / `esp-hal` on the buoy; WeftOS on shore.
- Compact acoustic frame (`BEACON` / detections); no JSON on the wet side.
- Leading-edge matched filter; pool T60 guard ≥ 1.5× measured (min 200 ms).
- Derived positions under `substrate/_derived/…`, not written by the ESP32.
- Hydrophone as JOURNALED-SENSOR-MIC special case (`summary` + optional `pcm_chunk`).
- `clawft-sonobuoy-ranging` as the `D(t)` consumer — only the **source of the metres** changes.

---

## 6. ADR-087 (Proposed) — later consumer, not this crate

ADR-087 candidates a spatial GNN branch + temporal branch + α fusion for sonobuoy detect → bearing → species-ID. Status: **Proposed (Candidate)**. Implementation is not scheduled.

Copper-v1 gives that ADR the **graph it would want**, if it ever ships:

- Spatial edges: taut `D_ij = L_cable` (high-confidence) + TDoA source hypotheses.
- Temporal features: `pps_seq` detections, SNR, self-TX flags.
- Physics priors: Mackenzie `c(T)` from the thermistor; not OAT-SSP yet.

Do **not** grow a dual-branch crate to prove the hose. Hash-encoded `mesh.sensor.v1` + a filled `DistanceMatrix` is the seam. Graphify stays code/forensic KG (ADR-082).

---

## 7. Solver picture (shore only)

```
delay cal (once per reel)     →  delay_ns[i]
green PPS + CAN {t_pps,dt_us} →  common time
taut L_cable                  →  D_ij exact  (DistanceMatrix::set)
acoustic TDoA in that frame   →  source (x,y) in the triangle
one GPS on master             →  lat/lon of the figure
SensorPipeline                →  mesh.sensor.v1.encoded (LeWM, not geometry)
```

Flip ambiguity of a 2-D triangle: one Class B depth, a fourth point, or a known dock bearing. Slack CAT7 must not appear as a filled `D_ij`.

---

## 8. Non-goals / do not

- Ship this document as a crate or an ADR.
- Run WeftOS / tokio on the S3.
- Leave 2.4 GHz on “just for gossip.”
- Re-sign buoy paths as the gateway.
- Feed USB PCM or `mesh.sensor.v1.timestamp_ms` into TDoA.
- Use CAN SOF as PPS.
- Claim slack copper locates a drifting field.
- Put continuous 16 kHz × N on 125 kbit CAN.
- Treat LoRa-on-copper or W5500-per-node as the 3-buoy hose.
- Wait on P047 / fiber / CSAC / JANUS for the first good test.
- Rewrite ADR-087 or production ADR-078.

**Not shipped (repeat):** embassy sonobuoy firmware, TWAI in `clawft-rpc`, USB-CAN gateway, delay-cal table, taut-edge kind on `DistanceMatrix`. The map above is what those pieces must obey when they exist.
