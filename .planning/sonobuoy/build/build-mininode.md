# Sonar Buoy — Class C Mini-Node Build (Acoustic-Only Drop-Deployable)

**Status**: Planning. Drafted 2026-05-11 by the sonobuoy symposium.
**Class**: C — Acoustic-Only (per ADR-082).
**Build cost**: ~$25 (passive-target variant) to ~$31 (full active TX +
depth + IMU variant).
**Build time**: ~1.5 hours per unit once practiced.
**Companion ADRs**: ADR-081 (sensor-framework adoption), ADR-082
(three-class architecture), ADR-083 (calibration service), ADR-084
(acoustic time-sequencing).

## What this is

A **sealed 2" PVC tube** containing an ESP32-S2 mini, a piezo
pressed against the inner wall, a depth + IMU sensor pair, and a
LiPo battery. **No surface section**, no antenna mast, no GPS, no
LoRa. The mini-node lives entirely underwater for the duration of
its deployment. Its only modes are:

- **Surface sync** (pre-deploy, above water): joins the surface
  fleet's WiFi/ESP-NOW group, syncs initial clock, receives
  configuration.
- **In-water chirp+log** (the deployment mode): TX chirps on a
  schedule per ADR-084, log every received chirp with local
  timestamp, log depth + IMU at 10 Hz.
- **Recovery dump** (post-deployment, above water): dump all
  logged data back to the fleet over WiFi.

The mini-node is **deliberately stupid**: no real-time positioning,
no real-time clock-sync correction, no mesh-routing logic. The
shore-side `clawft-sonobuoy-calibration` service (ADR-083) does
the heavy lifting post-hoc using the recovered logs joined with
the fleet's `acoustic.timing` chain stream.

## Why this design

Five forces converged on this design (user direction 2026-05-11):

1. **Diver-deployable**: the diver clips it to a line, lowers it
   to known depth, and lets go. No surface tether, no surface
   marker (the surface marker is the diver).
2. **Calibration target**: a sealed 2" PVC tube has a known TS
   (`-28 dB broadside at 1.8 kHz to -4 dB at 235 kHz` per P1
   §1.7 + P2 §2.4). The mini-node's electronics live INSIDE the
   target — one part, three functions (target + housing + active
   TX).
3. **Multi-depth capability**: place mini-nodes at multiple
   depths in the water column to provide ToF anchors for SSP
   measurement (Munk-Wunsch ocean-acoustic-tomography at hobby
   scale).
4. **Fleet density on the cheap**: at ~$25-31 BOM, deploying 5-10
   mini-nodes alongside 3 surface buoys is < $300 total — within
   the disposability framing of `build/phase-economics.md` §1.
5. **Post-hoc inference** (ADR-083): the chirp-and-log firmware
   spec is ~500 lines of Rust. Tiny binary, simple to verify,
   long battery life.

## Variants (in increasing capability and cost)

### Variant C-passive — passive PVC tube target ($5 BOM, no electronics)

Just a sealed PVC tube full of air. No electronics inside. A
calibration TARGET only; not a node. Used in early Phase 2 lake
tests when the fleet is verifying its ability to detect known-TS
objects before active-TX mini-nodes are built.

- 2" Sch 40 PVC, 25 cm long: $1.50
- 2 × 2" PVC end caps (cemented permanently): $4
- PVC primer + cement: $0.50 amortized
- D-rings (2× stainless eye bolts through caps, sealed with
  silicone): $2
- Total: ~$5

### Variant C-active — active TX with chirp+log ($25 BOM)

The standard Class C node per ADR-082. Active acoustic TX +
on-board memory for chirp logging.

- Same chassis as C-passive: ~$5
- ESP32-S2 mini: $3
- DRV8837 H-bridge: $2
- 35 mm 1.8 kHz piezo (existing parts, pressed against PVC inner
  wall): $0
- 250 mAh LiPo + TP4056 charger + AP2112 LDO: $7
- 3D-printed PETG mounting cradle (P1S, 100% infill): $1
- M3 brass heat-set inserts (4×): $0.20
- Wiring + protoboard: $2
- Conformal coating: $1
- Total: ~$25

### Variant C-full — active TX + depth + IMU ($31 BOM)

The full-capability variant. Adds two I²C sensors for depth and
orientation logging — required for the joint-inference solver
(ADR-083) to recover the mini-node's position trajectory.

- Same as C-active: ~$25
- MS5837-02BA depth/pressure sensor (I²C, 30 m / 100 m variants
  both $10): $10
- LSM6DSM IMU (I²C): $5
- Total: ~$31 (slightly less if MS5837 is bought in bulk)

The **recommended** variant for Phase 2 lake test is **C-full**.
Variant C-active is acceptable when the diver provides ground-
truth depth at deployment (less convenient but works).
Variant C-passive is the cheap calibration-target option.

## Mechanical: sealed PVC tube housing

```text
                 D-ring on top cap (M5 SS eye-bolt, sealed)
                 ↑
   ╔══════════════════════════════════════╗
   ║ 2" PVC pipe, 25 cm length, Sch 40    ║
   ║                                       ║
   ║  ┌─ Top cap: solvent-cemented        ║
   ║  │  permanently (no service access)   ║
   ║  │  Eye-bolt through cap, sealed     ║
   ║  │  with silicone caulk + backing nut ║
   ║  ▼                                    ║
   ║  ╔═══════════════════════════════╗   ║
   ║  ║ Foam cradle (3D-printed PETG) ║   ║
   ║  ║                                ║   ║
   ║  ║  ┌─ ESP32-S2 mini              ║   ║
   ║  ║  ├─ DRV8837 H-bridge           ║   ║
   ║  ║  ├─ MS5837 depth (I²C)         ║   ║
   ║  ║  ├─ LSM6DSM IMU (I²C)          ║   ║
   ║  ║  ├─ 250 mAh LiPo + TP4056      ║   ║
   ║  ║  ├─ AP2112 3.3V LDO            ║   ║
   ║  ║  └─ Conformal-coated protoboard║   ║
   ║  ║                                ║   ║
   ║  ║   ┌──────────┐                ║   ║
   ║  ║   │ 35 mm    │ ◄── Pressed     ║   ║
   ║  ║   │ piezo    │     against PVC ║   ║
   ║  ║   │ disc     │     wall with   ║   ║
   ║  ║   └──────────┘     thermal-    ║   ║
   ║  ║                    grease bond ║   ║
   ║  ╚═══════════════════════════════╝   ║
   ║                                       ║
   ║  ┌─ Bottom cap: solvent-cemented      ║
   ║  │  permanently. Eye-bolt for         ║
   ║  │  anchor weight on a 1 m line.      ║
   ║  ▼                                    ║
   ╚══════════════════════════════════════╝
                 ↓
                 D-ring → anchor weight (~200-500 g) on 0.5-3 m line
```

### Buoyancy and tuning

A 25 cm × 2" PVC tube full of air displaces 506 mL of water →
buoyancy ~5 N (positive). Total mass of contents (ESP32, battery,
sensors, cradle) ≈ 50-80 g → ~0.5-0.8 N downward. Net positive
buoyancy ≈ 4.2-4.5 N (≈ 425-460 g of float).

The anchor weight on the bottom D-ring sets deployment depth:
- 200 g weight at 1 m line → mini-node sits at 1 m depth
  (floating against the anchor line)
- 500 g weight at 3 m line → mini-node sits at 3 m depth
- For neutral-buoyancy free-floating (Phase 6+ aspirational), add
  a small weight pocket inside the cradle and tune empirically.

**Foam packing inside the cradle** is optional — at this size the
buoyancy is dominated by the air-filled tube volume, not the
electronics-displaced volume.

## Materials and 3D-printed parts

Per the consolidated 3D-print matrix in `.planning/symposiums/
sonobuoy/panels/P2-build-mechanical.md` §2.4.

| Part | Material | Print settings | Coating |
|------|----------|----------------|---------|
| Internal mounting cradle | **PETG** | 100% infill, 4 walls | none (dry-side) |
| Optional internal foam-cradle stiffener | PLA + Plasti-Dip | 25% infill, 3 walls | Plasti-Dip 2 coats |
| Heat-set M3 brass inserts | — | — | — (factory) |
| Eye-bolt back-cap reinforcement washer | PETG | 100% infill, 4 walls | none |

Total filament cost: ~$1 per mini-node.

## Electrical: chirp + log + sensor logging

### Signal chain

The signal chain is **trimmed-down from the Class A oil-sidecar
build** (`build-hydrophone-oil.md`):

```
ESP32-S2 GPIO ──► DRV8837 H-bridge ──► piezo (1.8 kHz)
                  6.6 Vpp differential
                  (50 Vpp pulser NOT in this build — passive TX
                   only; the imaging-tier upgrade is a separate
                   "C-imaging" sub-variant in Phase 5+ if needed)

piezo (RX side) ──► JFET source-follower (inside the cradle) ──►
   MCP6022 BPF chain ──► ESP32-S2 SAR ADC
```

The S2 is single-core but adequate: it runs the chirp generator
(low-rate), the matched-filter detector for incoming chirps, the
sensor I²C bus, and the chain-log writer. ESP-DSP for the FFT-
domain matched filter on the S2 is performant enough at 1.8 kHz
sample-rate budgets (16 kS/s).

### Why ESP32-S2 (and not S3 like the full buoys)

- **Cost**: S2 mini is ~$3; S3 mini N8R8 is ~$10.
- **Simplicity**: no WiFi-ISR-vs-acoustic-ISR conflicts because
  WiFi is OFF the entire deployment. The single-core S2 dedicates
  100% to acoustic.
- **Battery**: S2 in light-sleep between chirps draws <1 mA;
  S3 has higher idle.
- **Tradeoff**: no PSRAM (S2 doesn't have it). Mitigated by the
  smaller buffer requirement (1.8 kHz at 16 kS/s × N seconds
  fits in internal SRAM).

### Power budget

| Mode | Current draw (S2 + sensors) | Duty |
|------|------------------------------|------|
| Surface sync (WiFi on, 240 MHz) | ~150 mA | seconds, pre-deploy |
| Chirp+log idle (modem-sleep, RX active) | ~30 mA | ~95% of deployment |
| Chirp TX (DRV8837 driving) | ~80 mA peak | ~1% (50 ms TX every 5-10 s) |
| Chirp matched-filter | ~50 mA | ~2% (active during chirp arrival) |
| I²C sensor read (depth + IMU at 10 Hz) | adds ~3 mA | continuous |
| **Average** | **~30-35 mA** | — |

A 250 mAh battery → ~7-8 hours active. A 1000 mAh battery →
~28-32 hours. For a single dive (1-2 hours typical), 250 mAh is
plenty; for multi-day moored deployment, 1000+ mAh is correct.

### Local storage budget

`acoustic.timing` events generated locally during deployment:
- Per chirp received: ~50 bytes
- Chirp rate from fleet at 1 Hz, listening to ~5-15 emitters:
  ~10 events/s
- Per hour: ~36k events ≈ 1.8 MB

Depth + IMU log at 10 Hz × 24 bytes/record = ~240 bytes/s → ~860
KB/hour.

S2 has 320 KB SRAM + 4 MB flash. A 1-hour deployment is borderline
on flash. A 4 MB / 8 MB external SPI flash chip (W25Q32 ~$0.50)
adds ~24 MB capacity → ~12 hours of full-logging deployment.

For multi-day deployments, recommend adding an SD card adapter
(SPI, ~$2) for arbitrary-capacity local storage.

## Firmware: chirp+log mode

```
Class C firmware loop (chirp+log mode, pseudocode):

let local_clock = MonotonicClock::start();
let sequence_no = 0u24;
let log = LogWriter::new();

// Background tasks (embassy-rs)
spawn task: sensor_logger() {
    every 100 ms:
        depth = ms5837.read_depth();
        imu = lsm6dsm.read_6dof();
        log.write(SensorRecord { time: local_clock.now(), depth, imu });
}

spawn task: rx_chirp_detector() {
    let adc = I2sAdc::start(16_000); // sample at 16 kS/s
    loop {
        let window = adc.read_window(1024); // ~64 ms
        if let Some(detection) = matched_filter.detect(window) {
            log.write(AcousticTiming {
                receiver_path: NODE_PATH,
                emitter_path: detection.emitter_id,
                emitter_seq: detection.seq_no,
                local_rx_time_us: local_clock.now_us(),
                peak_correlation: detection.peak,
                snr_db: detection.snr,
                chirp_kind: detection.kind,
                band_khz: 1800,
            });
        }
    }
}

spawn task: tx_chirp_emitter() {
    every 10 s: // tdma slot determined at pre-deploy sync
        let tx_time = local_clock.now_us();
        let payload = ChirpPayload {
            emitter_path: NODE_PATH,
            seq_no: sequence_no,
            tx_time_local_us: tx_time,
            chirp_kind: ChirpKind::Beacon,
            band_khz: 1800,
        };
        chirp_generator.emit_with_payload(payload).await;
        sequence_no += 1;
        log.write(LocalTx { time: tx_time, payload });
}

// Main task waits for surface-sync triggers
loop {
    if wifi_link_up() {  // surfaced
        let sync_data = wifi_sync_with_fleet();
        local_clock.adjust(sync_data.master_time);
        log.flush_to_fleet().await;  // recovery dump
    }
    embassy_time::Timer::after(Duration::from_secs(60)).await;
}
```

This is the entire firmware. ~500 lines of Rust including
matched-filter setup, chirp generator, ChirpPayload encoder/
decoder, I²C drivers, log writer, and WiFi sync. The simplicity
is the design goal.

## Acoustic time-sequencing (ADR-084 inheritance)

The mini-node speaks the full **WeftAcousticTSF protocol** per
ADR-084:

- Every emitted chirp carries `(emitter_path, seq_no,
  tx_time_local_us, kind, band)`.
- Every detected chirp is logged as an `acoustic.timing` event
  with `(receiver_path, emitter_path, emitter_seq,
  local_rx_time_us, peak, snr, kind, band)`.
- The mini-node makes **no attempt** to correct its local clock
  during deployment. The shore-side `clawft-sonobuoy-calibration`
  service (ADR-083) recovers the clock offset and drift jointly
  with everything else.

This is the user-directed model: "**we can be broadcasting that
to some degree and building it into the active sonar components
— so that we can essentially build an array to calibrate against**"
(user direction 2026-05-11).

## Deployment workflow

### Pre-deploy

1. Charge mini-node to full (USB-C via TP4056 charge port on the
   top cap).
2. Power on; mini-node enters Surface Sync mode and joins the
   fleet's ESP-NOW group.
3. Fleet master assigns: TDMA slot, node serial confirmation,
   logging duration, target depth.
4. Mini-node reports back: battery %, sensor self-test pass.
5. Mini-node enters chirp+log mode (begins logging immediately).
6. Diver attaches anchor + dive line to bottom D-ring.

### During deployment

7. Diver descends to target depth, releases mini-node + anchor.
8. Mini-node sits at depth, executes chirp+log loop autonomously.
9. Surface fleet sees the mini-node's chirps as `acoustic.event`
   detections (carrying the emitter_path of the mini-node).
10. Shore service tags this as the mini-node entering Phase 2
    deployment epoch.

### Post-deploy

11. Diver descends to anchor position, recovers mini-node by
    pulling anchor line.
12. Mini-node surfaces with the anchor.
13. WiFi link comes up automatically; firmware enters Recovery
    Dump mode.
14. Logs are pushed to the fleet over WiFi (~2 minutes for a 1-
    hour deployment at full data rate).
15. Shore service consumes the recovered logs alongside the
    fleet's chain stream and runs the joint inference (ADR-083),
    emitting `acoustic.position` records for the mini-node's
    trajectory.
16. Mini-node powers off automatically when WiFi flush completes.

## Assembly steps

**1. Prep the PVC tube** (~10 min)
- Cut 25 cm of 2" Sch 40 PVC. Sand both ends.
- Drill 5 mm hole in side wall near the top, ~3 cm from the cap
  line, for the USB-C charge port plug.
- Drill 5 mm hole in top cap and bottom cap for eye-bolts.

**2. Bond bottom cap permanently** (~30 min including cure)
- PVC primer + cement on the bottom cap and lower pipe end.
- Insert M5 SS eye-bolt through the drilled hole; seal with
  silicone caulk + backing nut.
- Cure 24 hours before water exposure.

**3. Print the internal mounting cradle** (~30 min print time)
- PETG, 100% infill, 4 walls, 0.16 mm layers.
- Cradle holds: ESP32-S2, DRV8837 board, MS5837 board, LSM6DSM
  board, 250 mAh LiPo, TP4056 board.
- Heat-set M3 brass inserts into 4 mounting holes.

**4. Build the electronics subassembly** (~45 min)
- Solder ESP32-S2 to a 5×7 cm perfboard with the supporting
  components per the wiring diagram.
- Wire the piezo to the H-bridge output (TX) and to a JFET-
  source-follower preamp at the ADC input (RX). The JFET
  subassembly is the same J201 + 10 MΩ + 10 kΩ circuit as
  `build-hydrophone-oil.md`.
- Wire I²C bus to MS5837 and LSM6DSM.
- Conformal coat the populated board (MG Chemicals 422, 2 thin
  coats).

**5. Bond the piezo to the PVC inner wall** (~15 min)
- Thermal-grease (silicone) the brass side of the piezo.
- Press flat against the inside of the PVC pipe wall at the
  midpoint of the tube length.
- Foam-wedge in place from the opposite side until grease cures
  (~30 minutes).

**6. Install the cradle + electronics** (~20 min)
- Insert the cradle into the tube via the open top.
- Connect the piezo's leads to the cradle-mounted board.
- Connect the battery.
- Bench-test: power on, verify WiFi link, verify sensor reads,
  tap the piezo and verify ADC trace shows the response.

**7. Seal the top cap** (~30 min including cure)
- Solvent-cement the top cap permanently.
- Seal the USB-C charge port hole with a silicone-gasketed
  plug-and-screw (cheap waterproof Switch-style: ~$2; see
  Amazon hobby parts).
- Eye-bolt + silicone seal as on bottom cap.
- Cure 24 hours.

**8. Pressure test** (~1 hour)
- Submerge in a deep sink (>20 cm) for 2 hours.
- Recover, inspect for water ingress.
- If water present: identify failure (USB-C plug? cement joint?)
  and rebuild.

**9. Power on and verify firmware** (~10 min)
- Should join WiFi within 30 s of power-on.
- Should pass self-test.
- Set TDMA slot via fleet master.
- Ready for deployment.

Total build time per unit: ~3.5 hours including cures. Bulk-build
of 5-10 units: ~6-8 hours actual labor (cures overlap).

## Phase-by-phase deployment

- **Phase 1c** (proposed extension of Phase 1b): build 2-3 C-active
  variants alongside the 3 Class A buoys. Use them as drop-anchor
  ToF anchors in the pool. Verify that the joint-inference solver
  recovers their positions to ±10 cm.
- **Phase 2** (lake): build 5-10 C-full variants. Diver places them
  at varying depths. Verify ±50 cm position recovery vs diver-
  logged ground truth.
- **Phase 4** (open water): same hardware; longer deployments. Add
  1000 mAh battery and SD card option.
- **Phase 6** (fleet scale): 25-50 mini-nodes per deployment.

## Variants for later phases

- **C-imaging** (Phase 5+): add a 50 / 200 / 235 kHz transducer +
  pulser daughterboard (per `build-buoy-p79.md`). Mini-node
  becomes a mobile imaging-band illuminator. Combined with the
  Phase 5d gimbal architecture on the surface Class A, you get a
  mobile-illuminator multistatic SAS geometry — exactly Kiang
  2022 (ADR-065).
- **C-mooring** (Phase 6+): replace the LiPo with a primary D-cell
  alkaline pack. Multi-month deployment.
- **C-tracking** (Phase 7+): add a Doppler-aware crystal (TCXO)
  for finer drift. Borderline overkill for the joint-inference
  approach but useful for short-window coherent operations.

## Risk register

- **WiFi sync fails at deployment**: mini-node ships with a
  pre-configured fallback TDMA slot and a default chirp signature
  per serial number. Worst case: it chirps blindly with its hard-
  coded ID; the fleet still picks it up.
- **Recovery dump fails (WiFi link not coming up)**: long-press
  reset button (via the USB-C port plug position with a
  paperclip) puts the mini-node in mass-storage mode, exposing
  the SD card as USB drive.
- **Battery dies mid-deployment**: mini-node logs cease at battery
  end. Diver recovers the dead unit; partial log is still useful.
  Future variant: add an under-voltage cutoff that preserves a
  hash of the final log state in flash.
- **Lost mini-node** (rope breaks, diver loses sight): batteries
  drain in ~8-24 hours; the unit becomes a inert PVC tube. If it
  surfaces (positive buoyancy), it's recoverable by sight. If it
  sinks (anchor failure), it's lost. **The disposability framing
  (~$25/unit) accepts this loss rate**.
- **Diver acoustic discomfort**: pulser drive locked at 50 Vpp
  default per P2 §2.8. Peak SPL at 1 m ≈ 156 dB re 1 µPa — well
  below the NMFS 180 dB SPL limit for diver exposure. Document
  in the lake-test protocol.

## References

- ADR-082 (Class C architectural role).
- ADR-083 (shore-side calibration service that consumes the
  recovered logs).
- ADR-084 (acoustic time-sequencing protocol the mini-node speaks).
- `.planning/sensors/JOURNALED-NODE-ESP32.md` (substrate path
  identity).
- `.planning/sensors/JOURNALED-SENSOR-MIC.md` (hydrophone class
  inheritance).
- `build/build-hydrophone-oil.md` (parent build doc for the
  oil-sidecar circuit reused here).
- `build/build-hydrophone-simple.md` (parent build doc for the
  pressed-piezo-against-PVC technique reused here).
- `build/lake-test-protocol.md` (deployment methodology).
- P1 §1.6 (PVC acoustic impedance), §1.7 (near/far-field), §1.8
  (sparse-aperture math), §1.9 (cavitation thresholds).
- P2 §2.8 (pulser drive safety), §2.4 (consolidated 3D-print
  matrix).
