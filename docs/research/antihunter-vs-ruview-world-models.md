# AntiHunter vs RuView vs rUv world models

**Date:** 2026-09-26  
**Status:** Research comparison (not an ADR)  
**Sources:** [lukeswitz/AntiHunter](https://github.com/lukeswitz/AntiHunter) README; rUv Brain `ruview` / `worldgraph` (ADR-047 Observatory, ADR-275 RfGaussian, `wifi-densepose-worldgraph` / `wifi-densepose-worldmodel` ADR-147).  
**Siblings:** `antihunter-weftos-crosswalk.md`, `spatial-intelligence-2026/ruv-parallels-and-gaps.md`, `ruv-worldgraph-vs-weftos.md`.

They share **ESP32 CSI in a room**. They are **not** the same product.

| | **AntiHunter** | **RuView** | **rUv world models** |
|--|----------------|------------|----------------------|
| **Job** | Perimeter **detector fleet**: who/what RF device is here, attacks, drones, indoor motion | **RF sensing + observatory**: pose, vitals, signal field, persistent **RF scene memory** | **Twin + predictor**: what the room *is* (graph) and *next* (occupancy) |
| **Scale** | Node + Meshtastic mesh + GPS (site / event) | Installation / household room | Same room as RuView; OccWorld is a subprocess |
| **SoT** | SD jsonl + mesh alerts. No geometric twin | `RfGaussian` / `GaussianMap` (ADR-275) + scene graph; CSI discs in UI are **viz**, not 3DGS | WorldGraph typed petgraph (ADR-139) = **now**. OccWorld voxels = **prior**, not the twin |
| **CSI use** | WiDetect **movement** statistic (`sig`); indoor area; outdoor = tripwire; may TX a probe if starved | CSI → pose, breathing, motion_score, 20×20 signal field; Observatory Three.js room | Rasterize **PersonTrack ENU** → 200×200×16 occupancy → OccWorld → `TrajectoryPrior` into Kalman |
| **Identity** | MAC / OUI / SSID / `T-XXXX` behavioral; watchlists | Person **classes** only in scene graph; identity behind privacy gate (ADR-277 cited) | Anonymous track ids; occupancy never “who looks like” |
| **Locate** | RSSI path-loss + GPS + Kalman (experimental, metres) | Channel-gain query through Gaussians (Friis if empty map); inverse update from link residuals | ENU tracks + zone rectangles; not RSSI trilateration |
| **Predict** | None (baseline “new/gone”) | Decay/τ on Gaussians; Doppler class | **OccWorld** future occupancy + waypoints |
| **Viz** | Web UI / Command Center maps | Observatory (`observatory.html`): wireframe, mist, WiFi shells, vitals HUD | SuperSplat overlay + ProvenanceCard (WorldGraph on splat) |
| **Mesh** | Meshtastic TEXTMSG | RuView sensing server / WebSocket | Graph upserts, not LoRa |
| **Privacy** | Privacy Mode redacts UI; SD still has MACs/GPS | Occupancy not pixels; PrivacyRollup on graph | Same as WorldGraph; OccWorld gated by privacy mode |
| **MCU** | XIAO ESP32-S3/C5 **is** the product | ESP32 is a **sensor**; twin runs on host Rust + optional GPU | Host |

---

## 1. One sentence each

- **AntiHunter** is a **distributed sniffer**: promiscuous 802.11/BLE, drone RID, deauth/Sentinel, CSI as a **binary motion tripwire**, RSSI as a **rough range**.
- **RuView** is the **RF digital-twin UI + memory**: CSI becomes pose/vitals/field, then **anisotropic Gaussians** that answer “what blocked this link?”
- **rUv world models** split **symbolic now** (WorldGraph) from **learned next** (OccWorld). Neither is AntiHunter’s jsonl log.

WeftOS should treat AntiHunter as a **land Event source**, RuView/WorldGraph as the **room RF twin to compose with**, and keep **BVH** as geometry SoT.

---

## 2. CSI is not one thing

| | AntiHunter CSI | RuView CSI | OccWorld |
|--|----------------|------------|----------|
| Output | `sig` vs per-install trigger; multi-AP agreement; 12 s in 60 s window | `SensingUpdate`: persons, breathing_rate, motion_score, signal_field 20×20 | Voxel occupancy person/free |
| TX | Optional probe `02:00:00:00:00:01` if &lt;15 CSI frames/s | Sensing path (demo or live WS) | No RF TX — consumes tracks |
| Outdoor | Explicitly **not** area cover | Room observatory | Room bounds ENU |
| Geometry | None | Viz room + GaussianMap 1 m hash | 0.1 m voxels typical |

Do not pipe AntiHunter `sig` into OccWorld as occupancy. You can emit a WeftOS **Event** “motion in region X” and let Graph Views / WorldGraph decide.

---

## 3. Location honesty

AntiHunter: `distance = 10^((RSSI0 − RSSI) / (10 n))` with n=2…4.8 by environment. That is **RF ranging**, same class as SPS GPS (metres). Kalman on RSSI does not become a BVH leaf or an RfGaussian.

RuView ADR-275: empty map = **exact Friis**; absorbers learned from **link residuals**. That *is* a spatial memory of RF, still **not** a collider mesh.

WorldGraph: ENU rectangles and tracks — **beliefs**, provenance-mandatory.

---

## 4. What AntiHunter does *not* have (rUv does)

- Typed world graph with `SemanticProvenance`
- Occupancy world-model / trajectory priors
- RfGaussian fusion, channel-gain inverse update, `SceneGraph.activate`
- SuperSplat / Observatory cinematic twin
- Privacy as structural (no identity on tracks) vs UI redaction

## 5. What rUv does *not* have (AntiHunter does)

- Promiscuous **protocol** intel (probes, ghost SSIDs, deauth, karma, RID)
- Meshtastic **fleet** of cheap nodes with GPS
- Headless covert-ish mode, vibration wipe
- Randomized-MAC **T-XXXX** correlator
- Drone RID decoders on the node

Compose: AntiHunter **feeds** WorldGraph/WeftOS Events. It does not **replace** RuView memory or OccWorld.

---

## 6. WeftOS placement

```
AntiHunter  →  RF Events (devices, RID, CSI-motion bit, attacks)
                 ↓ Graph Views F3  (soft, low confidence)
RuView/WorldGraph  →  room RF twin (optional compose)
OccWorld           →  occupancy prior (optional, never SoT)
BVH / uvRTH        →  metric geometry + quilt
Acoustic copper S3 →  separate class, radio off
```

P047 = coherent RF (IQ). AntiHunter = 802.11/BLE **frames**. RuView Gaussians = **RF scene memory**. Three layers, don’t collapse.
