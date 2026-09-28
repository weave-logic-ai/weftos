# AntiHunter × WeftOS / rUv — RF perimeter node

**Date:** 2026-09-26  
**Status:** Research capture (not an ADR, not Plane-filed)  
**Upstream:** [lukeswitz/AntiHunter](https://github.com/lukeswitz/AntiHunter) (DIGI node firmware, main)  
**C2:** [AntiHunter-Command-Control-PRO](https://github.com/TheRealSirHaXalot/AntiHunter-Command-Control-PRO)  
**Parents:** `copper-bus-timing-grounding/`, `spatial-intelligence-2026/ruv-parallels-and-gaps.md`, `pzsdr-p047-and-fiber-towline.md`

AntiHunter is **distributed Wi‑Fi + BLE intelligence and attack detection** on a Seeed XIAO **ESP32-S3** (C5 beta for 5 GHz), GPS, SD, vibration, RTC, **Meshtastic LoRa**. Same MCU class as the sonobuoy S3. **Different radio job.**

WeftOS acoustic plan: **Wi‑Fi radio off**, copper CAN+PPS. AntiHunter: **Wi‑Fi radio on** (promiscuous / CSI). Do **not** flash AntiHunter onto the hydrophone node.

Legal: their own disclaimer — passive scan of broadcast frames ≠ interception; rules vary. WeftOS research treats this as a **defensive RF sensor class**, not an attack toolkit. Do not port Sentinel’s “how they’re caught” into offensive firmware.

---

## 1. What it is (grounded in their README)

| Piece | AntiHunter | WeftOS rhyme |
|-------|------------|--------------|
| MCU | XIAO ESP32-S3 8 MB (C5 2.4+5 GHz beta) | Class A S2 TX / S3 RX |
| Backhaul | Meshtastic UART TEXTMSG 115200, or SoftAP `192.168.4.1` | **Copper CAN + USB gateway** (acoustic). LoRa = *optional land mesh later* |
| Time | GPS NMEA + DS3231 RTC | **RS-422 PPS** on CAT5; GPS georef only |
| Identity | Node id + mesh radio id; MAC watchlists; `T-XXXX` behavioral IDs | Substrate `n-6hex`, ed25519; HNSW `VectorRef` for embeddings |
| C2 | Command Center PRO | `weftos-sensor-pipeline` / shore host |
| Motion | **CSI** indoor (WiDetect IMWUT 2019); **RadarNode** 24 GHz outdoor (experimental) | rUv CSI occupancy = **feature**, not BVH. Graph Views F3 |
| Locate | Multi-node **RSSI + GPS** trilateration + Kalman (experimental) | SPS GPS 2–5 m; RSSI path-loss **not** geometry SoT |
| Air | Drone RID (ODID / ASTM F3411 / BLE 0xFFFA) | Event leaves on uvRTH / Urth L4 |
| Provenance | Sentinel mesh-command **audit trail** (sender logged, not guessed) | Chain / ADR-069 panopticon rhyme |

**Headless** firmware (serial + mesh, no AP) is the only AntiHunter mode that is OPSEC-adjacent to our “don’t beacon.” Full firmware’s default AP `Antihunter` / `antihunt3r123` is published — treat unchanged nodes as open.

---

## 2. Steal / compose / do not

| Item | Verdict | Why |
|------|---------|-----|
| CSI indoor motion (beta), per-room `sig` trigger, multi-AP agreement, listen-only | **Compose** as Graph View **feature** on L4 indoor quilt | Same honesty as rUv: occupancy ≠ AABB. Outdoor CSI is a **tripwire**, not area cover (their note). |
| WiDetect method (ACM IMWUT 3(3) 2019) | **Cite** | They already named the paper. |
| RadarNode 24 GHz outdoor | **Watch** | Fills the CSI outdoor gap; not our P047 / not sonar. |
| Meshtastic TEXTMSG UART to Heltec | **Steal pattern** for **land** nodes only | Matches “LoRa later as radio, not copper PHY.” |
| Drone RID → Event + GPS | **Compose** | uvRTH Event leaves; VectorRef if we embed RID blobs. |
| Baseline anomaly (new/gone/RSSI change) | **Compose** as temporal View | Same job as “school persists” clustering, RF domain. |
| Randomized MAC `T-XXXX` signatures | **Compose** into HNSW identity namespace | Feature index, not BVH. |
| RSSI trilateration + path-loss table (n, RSSI0 by environment) | **Feature only** | Open-sky n=2 still metres of error. Do not mint BVH Object leaves from RSSI. |
| Sentinel attack detectors | **Do not port as weapons** | Detection of deauth/karma/evil-twin is defensive intel; keep it a **View of RF events**. |
| SoftAP / promiscuous on the **acoustic** S3 | **Do not** | Kills ADC timing; user: copper replaces Wi‑Fi. |
| Vibration wipe / self-destruct | **Optional land OPSEC** | Not a buoy requirement. |
| Default AP creds | **Never copy** | |

---

**vs RuView / OccWorld / WorldGraph (full table):** [`antihunter-vs-ruview-world-models.md`](./antihunter-vs-ruview-world-models.md). Short: AntiHunter is a **sniffer fleet**; RuView is **RF scene memory + observatory**; rUv world models are **graph now + occupancy next**. CSI `sig` ≠ OccWorld voxels ≠ RfGaussian.

## 3. Crosswalk to rUv WorldGraph / CSI

rUv room twin is **CSI + occupancy grid + RF-Gaussians**. AntiHunter is **commodity ESP32 CSI motion + device graph**.

- AntiHunter CSI: **movement**, not presence; indoor multipath is the aperture; may TX a probe if starved (`02:00:00:00:00:01`).
- rUv OccWorld: occupancy **prior** into Kalman, not the twin.
- WeftOS: both are **F3-bound features**. Geometry stays BVH. A CSI “someone moved in room-12” is an **Event** with `vector: None` unless we embed a snippet.

Triangulation: AntiHunter `distance = 10^((RSSI0-RSSI)/(10 n))` + GPS + Kalman. That is **RF ranging**, same honesty class as our “SPS GPS is not coherent BF.” Fuse as a **soft edge** on Graph Views (low confidence), never as taut `L_cable`.

---

## 4. Fleet shape (don’t mix radios on one S3)

```
Land / dock / USV deck     AntiHunter node(s)  WiFi+BLE sniff + GPS
        │ Meshtastic (optional) or USB/Ethernet
        ▼
   WeftOS shore            mesh.sensor.v1  RF events + drone RID
        ▲
        │ USB-CAN + PPS
Water                      acoustic S3s   radio OFF, copper bus
```

Three Class A hydrophones stay copper-only. AntiHunter is a **fourth class: RF perimeter**, same Meshtastic channel *if* we want land coverage, separate MCU.

P047 (1 MHz–6 GHz) remains the **coherent RF** head; AntiHunter is the **802.11/BLE protocol** head. Don’t collapse.

---

## 5. Sources

- https://github.com/lukeswitz/AntiHunter (README, docs, `Antihunter/src`)
- CSI: WiDetect, ACM IMWUT 3(3) 2019, https://cswu.me/papers/ubicomp19_widetect_paper.md
- Drone RID: ASTM F3411 / ODID
- WeftOS: ADR-056/078/079, copper-bus plan, rUv parallels (CSI ≠ 3DGS ≠ BVH)
