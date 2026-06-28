# Sonar Buoy — Lake Test Protocol (Phase 2, Diver-Assisted)

**Status**: Planning. Drafted 2026-05-11 by the sonobuoy symposium.
**Phase**: 2 (the localization milestone, evolved from pool to lake
per user direction 2026-05-11).
**Companion ADRs**: ADR-081 (sensor-framework adoption), ADR-082
(three-class architecture), ADR-083 (shore-side calibration service),
ADR-084 (acoustic time-sequencing).
**Companion build docs**: `build-hydrophone-oil.md` (Class A),
`build-tethered-subsurface.md` (Class B), `build-mininode.md`
(Class C).

## What this is

The methodology for **Phase 2 lake-deployment validation** of the
three-class sonobuoy fleet, using a free diver to place known-position
reference targets at known depths. Replaces the original Phase 2
"pool tape-measure" methodology, which the user upgraded by offering
access to a lake and a diver.

## Goals

1. **Validate the three-class fleet end-to-end** in real water
   (not pool, not bench).
2. **Validate the shore-side joint-inference service** (ADR-083)
   against diver-logged ground truth.
3. **Validate the acoustic time-sequencing protocol** (ADR-084)
   at lake-scale baselines (10-100 m).
4. **Bootstrap the fleet's self-calibration** — first deployment
   produces the first per-RX sensitivity, per-RX clock offset,
   and lake-volume SSP coefficients on the chain.
5. **Build operator confidence** — first time the fleet is operated
   end-to-end without an engineer's hand on every parameter.

## Fleet configuration

The minimum deployable fleet for Phase 2:

- **3 × Class A surface buoys** (`build-hydrophone-oil.md` build):
  anchored at the three corners of a ~10-50 m equilateral triangle
  in the lake. Each carries:
  - 1× JFET hydrophone (parent's primary RX)
  - 2× Class B subsurface RX at -1 m and -2 m
  - 1× WiFi antenna mast above water
  - 1× 18650 battery
- **2-3 × Class B subsurface tethered to each Class A**
  (`build-tethered-subsurface.md` build): 1 m and 2 m below
  parent. Total 6-9 Class B units.
- **5-10 × Class C mini-nodes** (`build-mininode.md` build):
  C-full variant with depth + IMU. Diver-placed at varying
  depths during the test.
- **5-10 × Class C-passive PVC tubes** (no electronics): used
  as known-TS reference targets at known depths.

Total fleet: ~14-19 receive elements + 5-10 active TX + 5-10
passive targets. Cost: 3×$60 + 6×$15 + 10×$31 + 10×$5 = ~$610.

## Diver toolkit (BOM additions for the test)

| Qty | Part | ~Price |
|-----|------|--------|
| 10 | Latex weather balloons (30 cm) | $5 |
| OR 10 | Mylar/foil balloons (30 cm, more rigid) | $15 |
| OR 10 | Sealed 2" PVC tube targets (Class C-passive) | $50 |
| 1 | Dive line, 50 m | $20 |
| 1 | Stainless quick-clips for line termination | $10 |
| 1 | 1-3 kg dive weights (5 ea.) | $50 |
| 1 | Diver-side IMU + depth log (e.g., Garmin Descent G1) | (diver-provided) |
| 1 | Underwater slate + grease pencil (for recording positions) | $15 |
| 1 | Surface marker buoy (high-vis) with attached GPS unit (optional) | $50 |
| 1 | Boat / kayak for fleet deployment + recovery | (user-provided) |
| 1 | Pre-deployed shore base (laptop running WeftOS) | (user-provided) |
| 1 | LoRa gateway or extended WiFi (optional, for shore comms during deployment) | $30 |
|     | **Total new BOM for the test** | **~$100-200** |

The lake itself (depth, water quality, accessibility) is the
user's resource; this doc assumes a freshwater lake 5-30 m deep
with clear-enough water for the diver to navigate.

## Day-of-deployment sequence

### Pre-deployment (shore-based, ~2 hours)

1. **Charge all units to full**. Class A, Class B, and Class C
   batteries verified ≥ 95%.
2. **Pre-flight checks**:
   - Every unit powers on, joins the shore WiFi, passes self-test.
   - Sensor reads sanity-checked (depth = 0 ± 5 cm in air;
     temperature ≈ ambient; IMU gravity vector vertical).
   - TDMA slot assignments distributed by the master Class A.
   - Chirp generator + matched filter self-test on each TX-RX pair.
3. **Bench-test the joint-inference service**: load a synthetic
   recording from a previous pool test, verify the service
   produces consistent position estimates.
4. **Brief the diver**:
   - **Acoustic safety**: pulser drive locked at 50 Vpp default,
     peak SPL at 1 m ≈ 156 dB re 1 µPa. Well below NMFS 180 dB
     diver-exposure limit. Confirm by reviewing P2 §2.8 + this
     section.
   - **Test sequence**: target positions to be placed, depths,
     order.
   - **Abort criteria**: visibility < 1 m, current > 0.5 m/s,
     temperature stratification (thermocline) shallower than
     2 m and stronger than 3 °C/m, weather change, diver
     discomfort.
   - **Recovery sequence**: which targets to recover when, what
     to do if a Class C mini-node fails to surface.

### Deployment (boat-based, ~30 min)

5. **Anchor the 3 Class A buoys** in the planned triangle
   geometry, ~10-50 m baseline. Record GPS coordinates of each
   anchor.
6. **Drop the Class A units** with their attached Class B tethered
   subsurface chain. Verify each one floats correctly and the
   surface marker is visible.
7. **Verify the fleet self-organizes**:
   - All 3 Class A units come up on WiFi from the shore base.
   - All 6 Class B units report through their parents.
   - The acoustic mesh broadcasts a `chirp_kind = BEACON_TIMING`
     from the master Class A and is received by all RXs.
   - `acoustic.timing` chain stream begins flowing.

### Diver descent + reference deployment (in-water, ~60 min)

8. **Diver descends with 5-10 Class C-passive PVC-tube targets**
   on dive lines:
   - Diver places each target at a known depth (logged via the
     diver's dive computer with depth + bearing-to-surface-
     marker noted on the slate).
   - Targets are anchored to weights at the bottom.
   - Surface marker for each target (small float on the dive line)
     to enable subsequent diver recovery and to GPS-tag the target
     position from a boat-mounted GPS during the test.
   - Recommended target distribution: 2 at 2 m, 3 at 5 m, 3 at
     10 m, 2 at 15 m (sampling the water column).

9. **Diver places 5-10 Class C-full mini-nodes** at varying depths:
   - These are active. Diver clips each mini-node to a dive line
     at a known depth (1-15 m range).
   - The mini-node begins emitting chirps on its TDMA slot.
   - Diver records the mini-node serial + planned depth on the
     slate; fleet auto-logs the mini-node entering the deployment
     epoch (the master receives the mini-node's first chirp and
     timestamps it).

10. **Capture window** (15-30 minutes):
    - Class A buoys broadcast timing beacons at 1 Hz.
    - Class C mini-nodes broadcast at their assigned TDMA slots.
    - All Class A + Class B RXs log every detected chirp →
      `acoustic.timing` chain stream.
    - Diver may move between targets, occasionally repositioning
      one as a "moving-target" test.
    - Diver vocalizes / kicks fins for passive PAM signature
      diversity (the fleet's PAM capability tests via diver-
      generated noise).

11. **Diver ascends and recovers** the Class C-full mini-nodes
    (Class C-passive targets stay; they're cheap enough to leave
    if necessary). Diver surfaces with each mini-node; the mini-
    node WiFi-link automatically engages and dumps logs to the
    fleet within 2 minutes.

### Post-deployment (shore-based, ~1 hour)

12. **Run the joint-inference service** (ADR-083) over the
    captured data:
    - Inputs: fleet `acoustic.timing` + recovered Class C logs +
      diver-logged ground truth (depths + slate notes).
    - Outputs: per-node position trajectories, per-node clock
      offsets, per-node sensitivity, lake-volume SSP.
13. **Compare** the service's position estimates to the diver-
    logged ground truth.
14. **Visualize** in the WeftOS surface (the chart-style PPI from
    `commercial-comparison.md` §5.1 or the 3D-with-AR surface
    from §5.11).
15. **Recover the fleet**: pull anchors, retrieve Class A buoys
    + their Class B chains.

## Acceptance criteria

### Hard pass / fail

- [ ] All 3 Class A buoys complete the test without firmware
      crash or battery failure.
- [ ] At least 80% of deployed Class C mini-nodes complete the
      test and dump logs successfully.
- [ ] Shore service produces position estimates for all surfaced
      Class C mini-nodes.
- [ ] Position estimate error vs diver-logged ground truth:
      **≤ 50 cm at 95% confidence** for targets at known
      depths within 30 m of the fleet center.
- [ ] Clock-offset estimates converge to within ±100 µs for all
      nodes by deployment end.
- [ ] No safety incident (diver discomfort, equipment damage,
      lost / unrecoverable hardware beyond the disposability
      budget).

### Soft / informational

- Per-node sensitivity calibration values (`acoustic.calibration`
  stream) recorded for posterity.
- Lake SSP coefficients (3-5 EOF terms) recorded; compared
  against a Mackenzie 1981 prediction from the surface
  temperature reading.
- Passive PAM detection of the diver's noise signature recorded.
- Comparison: fleet-detected position of a Class C-passive PVC
  tube (passive echo only) vs Class C-full (active TX). Active
  TX should give 20-40 dB better SNR — confirm in the data.

## Failure-mode handbook

| Failure | Detection | Response |
|---------|-----------|----------|
| Class A buoy doesn't WiFi-link at shore | shore base reports missing | Re-flash firmware via USB; otherwise replace with spare |
| Class A buoy GPS doesn't lock | reported in pre-flight self-test | Move to better sky-view position; verify GPS antenna |
| Class A acoustic mesh silent (no chirps received) | acoustic.timing stream empty | Check pulser drive level; verify TDMA slot config; replace pulser MOSFET if needed |
| Class B tether disconnects | parent reports missing child | Recover and re-mate; check M8 connector for water ingress |
| Class C mini-node doesn't surface | not seen on surface after expected duration | Diver retrieves anchor line; mini-node is buoyancy-positive once anchor is removed (should bob up) |
| Class C mini-node surfaces but WiFi link doesn't engage | no log dump after 5 min surface time | Manual recovery + USB-C extraction of internal flash |
| Class C battery dead before recovery | timestamps end mid-deployment | Partial log is still useful; data ends at battery cutoff. Mini-node still recoverable. |
| Joint-inference service produces position with covariance > 5 m | suspicious estimate | Check SSP convergence; check for missing Class A anchor in input; check for chirp-payload decode errors |
| Diver acoustic discomfort | diver signals abort | Reduce pulser drive to 25 Vpp via OTA fleet config; resume |
| Visibility drops below 1 m | diver visual | Abort test; recover fleet at surface; reschedule |

## Lake selection criteria

Not every lake works. Required:

- **Depth ≥ 3 m** at the fleet center (to fit Class B at 1m and
  2m plus headroom).
- **Visibility ≥ 2 m** at deployment time (for diver navigation).
- **Wind / chop < 0.5 m wave height** (Class A buoys are stable
  in modest chop but more is operator-unfriendly).
- **No active fishing / boating traffic** during the test
  window (acoustic interference; also a safety concern).
- **Permission to deploy** (private lake or local-authority
  consent).
- **Accessible from the shore base** for the WiFi mesh (≤ 100 m
  for ESP-NOW; LoRa gateway extends this).
- **Thermocline within reasonable bounds**: ideally absent in
  shallow lakes < 5 m deep, or known and characterizable in
  deeper lakes. The fleet should ideally span only one thermal
  layer to keep SSP estimation simple in the first deployment.

## Iteration plan

The Phase 2 test is **not one-shot**. Plan for 3-5 iterations:

1. **Iteration 1 — fleet works at all**: minimum 3 Class A + 1
   Class C placed by diver. Just confirm chirps cross the lake
   and the chain ingests events. Acceptance: any non-zero
   meaningful position estimate at all.
2. **Iteration 2 — bearing/range accuracy**: 3 Class A + 2
   Class C at known depths. Verify position estimate accuracy.
3. **Iteration 3 — full Class B integration**: add Class B
   tethered subsurface array. Verify in-buoy TDoA bearing per
   Class A.
4. **Iteration 4 — calibration array**: full 3+6+10 fleet.
   Verify per-RX sensitivity calibration and SSP recovery.
5. **Iteration 5 — performance push**: extend to longer
   baselines, deeper Class C placements, additional Class C-
   imaging variants (Phase 5+ overlap).

Each iteration produces lessons that flow back into the build
docs and firmware. Plan one weekend per iteration if diver
availability permits.

## Data products from each deployment

Per ADR-083, each deployment produces:

- **`acoustic.position`** — back-filled position trajectories for
  every node.
- **`acoustic.calibration`** — per-RX sensitivity, clock offsets,
  SSP coefficients, deployment epoch metadata.
- **`acoustic.timing`** — raw chirp-detection events (kept on
  chain for replay and re-analysis).
- **`acoustic.contact`** — residual-after-subtraction contacts
  (passive PAM + active reflections from PVC tube targets).
- **`acoustic.deployment_epoch`** — start/end markers with fleet
  composition, weather, diver session ID.

The chain becomes a **growing per-lake dataset**: every revisit
to the same lake compares against past deployments, enabling the
temporal-change detection described in `phase-economics.md` §5.6
once Phase 5d imaging-tier integration lands.

## Acoustic safety briefing (for the diver)

The fleet's TX sources are:

- **Class A pulser at 50 Vpp into the 1.8 kHz piezo**: peak SPL at
  the diver's location depends on distance. At 1 m: ~156 dB re
  1 µPa (computed from P1 §1.1 sonar-equation budget). At 10 m:
  ~136 dB. At 50 m: ~122 dB.
- **NMFS marine-mammal-exposure guideline** (also used by US
  Navy for diver safety): 180 dB SPL re 1 µPa peak / 195 dB
  SPL re 1 µPa² · s cumulative. The fleet's 50 Vpp pulser at any
  realistic dive distance (≥ 1 m) is **well below** this limit.
- **Class C mini-nodes at 6.6 Vpp** are even quieter: ~130 dB at
  1 m, ~110 dB at 10 m. Inaudible to the diver as a discrete
  event; perceived as ambient ticks at best.
- **The 50 Vpp default is hard-capped in firmware** per P2 §2.8.
  Enhanced 80 Vpp mode requires explicit operator activation and
  is paired with a cavitation-noise interlock.

**Diver may signal abort at any time**. Abort response: shore
operator reduces TX power to 25 Vpp via OTA fleet config; fleet
re-acks within 5 seconds. Diver surfaces at their own pace
afterward.

## References

- ADR-082 (three-class fleet composition).
- ADR-083 (joint-inference service consuming the test data).
- ADR-084 (acoustic time-sequencing exercised by the test).
- `build/build-hydrophone-oil.md` (Class A build).
- `build/build-tethered-subsurface.md` (Class B build).
- `build/build-mininode.md` (Class C build).
- P1 §1.1 (sonar-equation budget for SPL calculations).
- P2 §2.8 (pulser drive safety lock).
- `commercial-comparison.md` §5.7 (forensic / replay surface) and
  §5.11 (3D + AR fly-through) — the eventual UI consumers of
  the deployment data.
- `phase-economics.md` Phase 2 row — cost line for the lake-test
  BOM additions.
- NMFS Marine Mammal Acoustic Exposure Guidelines (2018, NOAA
  Technical Memorandum NMFS-OPR-59) — diver-exposure safety basis.
