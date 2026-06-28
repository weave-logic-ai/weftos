# Sonar Buoy — Phase 1c Fleet-Density Experiment

**Status**: Planning. Drafted 2026-05-11 by the sonobuoy symposium
follow-up turn.
**Phase**: 1c (extension of Phase 1b, pre-Phase-2 lake test).
**Companion ADRs**: ADR-082 (three-class architecture), ADR-083
(shore-side calibration service consuming the experiment's chain
streams), ADR-084 §"Implication for the deployment plan" lines
374–381 (the experiment this doc operationalizes).
**Companion build docs**: `build-mininode.md` (Class C — the
incremental node), `build-tethered-subsurface.md` (Class B —
already present), `build-hydrophone-oil.md` (Class A — already
present), `lake-test-protocol.md` (Phase 2 test methodology — the
downstream consumer of the scaling law this experiment produces).

## What this is

A **pool-deployed cheap fleet-density sweep** that measures how
the WeftAcousticTSF (ADR-084) acoustic mesh degrades as node
count `N` grows from 9 → 14 → 19 → ~30. The experiment establishes
empirical limits on:

- **Slot-collision rate vs N** — how often two emitters land in
  the same chirp slot under the per-band TDM schedule.
- **Optimal carrier-slot allocation across the fleet** — how many
  slots per band per second the schedule should reserve for a
  given fleet size.
- **Mesh refresh rate degradation with N** — how the per-node
  effective ranging update rate falls as the fleet grows.
- **Multi-anchor coverage overlap** — how the fleet's effective
  joint-coverage footprint scales when multiple Tier-1 (or Tier-2)
  anchors share the band.

The output is a **scaling-law dataset** that the Phase 2 lake test
(`lake-test-protocol.md`) uses to choose its fleet composition and
that ADR-084 / ADR-086 use to lock the carrier-priority and
TDM-slot-budget contracts at production fleet sizes.

ADR-084 §"Implication for the deployment plan" lines 374–381
explicitly call out Phase 1c as a ~$250 experiment producing
"the scaling-law data points the whole project's thesis depends
on" — this build doc operationalizes that paragraph.

## Why this design

Five forces converged on this experiment (ADR-084 §"Compounding
with N" + P3 §10.5 + P4 §4.1.3 + P4 §4.2.4):

1. **Mesh value grows super-linearly with N**, but only if
   per-slot collision rate stays below threshold. P4 §4.1.3
   pegs `acoustic.timing` aggregate rate at 6–2,340 ev/s across
   Class A/B/C × pool/lake/open-water.
2. **Pool is the cheap test bed**. Phase 1b fleet (3 Class A +
   6 Class B) is already on-site; adding 5–20 Class C mini-nodes
   (~$25–$31 each, `build-mininode.md`) stays under $600.
3. **Pre-Phase-2 risk reduction**. Discovery in pool is hours;
   discovery in lake with diver is days.
4. **Joint solver (ADR-083) is data-hungry** — needs scaling-law
   to weight redundant vs unique-geometry observations.
5. **Class C is the cheapest acoustic-active fleet increment** —
   no GPS, no LoRa, just a diver-drop at known depth per
   `build-mininode.md`.

## Experiment description

The experiment is a **5-step N-sweep** over the per-band TDM
schedule of WeftAcousticTSF (ADR-084 §1.6):

| Step | Fleet composition | N total | Aggregate emit rate target |
|------|-------------------|---------|------------------------------|
| 1 | 3 Class A + 6 Class B + 0 Class C | 9 | ~10 ev/s |
| 2 | 3 Class A + 6 Class B + 5 Class C | 14 | ~25 ev/s |
| 3 | 3 Class A + 6 Class B + 10 Class C | 19 | ~50 ev/s |
| 4 | 3 Class A + 6 Class B + 15 Class C | 24 | ~80 ev/s |
| 5 | 3 Class A + 6 Class B + 20 Class C | 29 | ~120 ev/s |

For each step, the fleet runs the per-band TDM cycle for **5
minutes** of nominal chirp-and-log activity, with the shore
service (ADR-083) consuming and counting:

- **Successful `acoustic.timing` event rate per receiver per
  emitter pair** — the "good slot" rate.
- **Slot-collision detection rate** — overlapping chirps in the
  same slot identified by matched-filter peak-mass-vs-expected-
  one-emitter and by the joint solver flagging unrecoverable
  cross-talk.
- **Per-receiver missed-emission rate** — slots where the
  receiver should have heard an emission and didn't (per the
  TDM schedule).
- **Joint-solver convergence time** — how long does ADR-083
  take to produce a position fix on every node, at each N.

Across the 5 steps × 5 minutes × ~6 acoustic bands (LF song /
1.8 kHz mesh / 35 kHz mesh-comms / 50 kHz imaging / 200 kHz
imaging / 235 kHz imaging) the experiment runs roughly **2.5
hours** of pool time end-to-end (including 10 min reconfigure
between steps).

## Required materials

Beyond the existing Phase 1b pool fleet (which is assumed to be
on-hand from the prior milestone), the incremental BOM is:

| Qty | Part | Per-unit | Subtotal |
|-----|------|----------|----------|
| 20 | Class C mini-node (`build-mininode.md` C-active variant) | $25 | $500 |
| 10 | 1-3 m dive line + clip + 200-500 g weight (for C suspension at known pool depth) | $5 | $50 |
| 1 | Pool depth gauge / tape measure (already in lab kit, $0 incremental) | — | $0 |
| 1 | Underwater slate + grease pencil (for diver-logged drop positions, ~$15 if not on hand) | $15 | $15 |
| 1 | Spare battery pack for shore laptop (2-3 hour experiment) | $30 | $30 |
| 1 | Extension cord + GFI for shore lab power | $20 | $20 |
| 1 | Notebook + tally sheet for manual collision-event logs | $5 | $5 |
| | **Incremental BOM** | | **~$620** |

(ADR-084 line 377 quoted "~$250" — that figure assumed the 5-10
mini-node lower end of the sweep. The $620 figure here covers
the full 20-node sweep including spares; running just steps 1-3
at ~$300 total is acceptable for a smaller-budget pass.)

## Test procedure

### Pre-experiment (shore-based, ~2 hours)

1. **Build the Class C fleet**. Per `build-mininode.md` §"Assembly
   steps", build 10-20 C-active variants. Bench-test each in air
   (WiFi join + self-test + bench piezo tap). Charge to full.
2. **Pre-configure TDM slots**. The fleet master assigns
   per-step slot allocations: 9-node schedule, 14-node schedule,
   19-node schedule, 24-node schedule, 29-node schedule. ADR-084
   §1.6 chirp-payload format encodes the slot ID; the shore
   service knows the schedule per step.
3. **Configure the shore-service experiment harness**. The
   `clawft-sonobuoy-calibration` service runs in a special
   *experiment-trace mode* that exports per-step CSV files of
   slot-success / slot-collision / missed-emission tallies plus
   the joint-solver convergence trace.
4. **Brief the operator**. Two operators: one shore (running
   the harness, recording step transitions); one pool-side
   (dropping mini-nodes at known positions, logging deployment
   on the underwater slate).

### Per-step pool sequence (~30 min per step)

For each of the 5 steps:

5. **Place the incremental Class C nodes**. The pool-side
   operator drops each new mini-node at a known position (tape-
   measured against pool corner) at known depth (anchor line
   length). Records position on the underwater slate. Joins
   each node to the fleet via pre-deploy WiFi sync per
   `build-mininode.md` §"Pre-deploy".
6. **Apply the step's TDM schedule**. Shore harness pushes the
   new schedule to all nodes via a signed-command chain entry.
   All nodes ack via `health/sensor/acoustic` health event
   within 10 s.
7. **Run the 5-minute acoustic exercise**. The fleet emits
   chirps per the schedule across all bands. The harness records
   every `acoustic.timing` event and tags each with: (a) step ID,
   (b) emitter, (c) receiver, (d) slot ID, (e) band, (f) success
   vs collision vs miss.
8. **Run the joint-solver convergence trace**. At minute 4 of
   the 5-minute exercise, the shore harness triggers a full
   batch solve and logs the wall-clock time to convergence
   (defined as `||position_residual|| < 5 cm` for all nodes).
   Captures intermediate residual norms for the convergence
   curve.
9. **Reconfigure for next step (~5 min)**. Pool-side operator
   drops the next batch of mini-nodes; shore operator pushes
   the next TDM schedule.

### Post-experiment (shore-based, ~30 min)

10. **Recover all mini-nodes**. Pool-side operator pulls each
    anchor line; mini-nodes surface; WiFi reconnect triggers
    recovery dump per `build-mininode.md` §"Post-deploy". Logs
    flush to fleet within ~2 min per unit.
11. **Run the full-fleet batch solve**. With all 5 steps' raw
    chain data on the shore host, the harness re-runs the joint
    solver in batch mode (ADR-083 §"Modes") to produce the
    end-to-end position trajectory for every node across the
    full experiment window.
12. **Export the scaling-law dataset**. CSV files exported per
    step + a summary CSV across steps. Schema below.

## Success metrics

The experiment is "successful" if all four scaling-law data points
are measured (the experiment is exploratory by design — there is no
pass/fail criterion on the physics itself, only on the data-collection
rigor):

1. **Slot-collision rate vs N is measured** at the 5 sweep points.
   Threshold for "concerning": > 5% collision rate at N = 19.
   Above 5%, ADR-086 carrier-priority dispatcher needs revision.
2. **Joint-solver convergence time vs N is measured**. Threshold
   for "concerning": > 30 s batch solve at N = 19 with Gauss-Newton
   per P4 §4.8.3. Above 30 s, the lake test plan should pre-test
   the solver at intermediate N before committing to a 5-10
   Class C drop.
3. **Per-band schedule capacity is measured**. The experiment
   should produce a per-band saturation curve — at what N does
   the 1.8 kHz band saturate? At what N does the 35 kHz mesh-comms
   band saturate? This informs ADR-084 §1.6 schedule design at
   production fleet sizes.
4. **Recommended max-buoys-per-cluster figure**. Per the four
   metrics above, the experiment produces a single recommended
   number — "for a single-cluster pool deployment, do not exceed
   N = X without a second band-disjoint cluster." That number
   feeds the lake-test protocol's fleet-composition decision.

## Expected output

### Primary dataset: per-step scaling-law CSV

```text
step_id, n_total, n_class_a, n_class_b, n_class_c, aggregate_emit_rate_hz,
  slot_success_rate, slot_collision_rate, missed_emission_rate,
  joint_solver_converge_s, residual_at_30s_cm, band_saturation_flags
```

One row per step × per acoustic band × per receiver-class. Roughly
5 steps × 6 bands × 3 RX classes = 90 rows.

### Secondary dataset: per-event raw chain-stream replay

The full set of `acoustic.timing` chain events from all 5 steps
is left on the chain (it is the WeftOS substrate's job to retain
this). The experiment harness exports a one-time **replay-bundle
manifest** (chain stream segment references + content hashes per
ADR-088 / ADR-089 pattern) so that a future re-analysis can re-run
any solver variant against the same raw data.

### Tertiary dataset: hand-recorded operator log

The underwater-slate deployment log + the shore-operator step-
transition tally sheet are scanned and committed to
`.planning/sonobuoy/build/experiments/phase-1c-fleet-density/`
alongside the CSV files. These are the human-readable ground truth
that lets a future operator reproduce the experiment.

### Recommendation deliverable

A short markdown summary (~1 KB) at
`.planning/sonobuoy/build/experiments/phase-1c-fleet-density/RESULTS.md`
written by the operator running the experiment, containing:

- The single recommended max-buoys-per-cluster figure.
- The recommended per-band carrier-slot allocation for the lake test.
- Any tensions surfaced (e.g., 35 kHz mesh-comms saturates earlier
  than 1.8 kHz mesh — this would feed back into ADR-086).
- The joint-solver convergence curve as a chart (CSV + PNG).

## Phase-by-phase context

- **Phase 1b** (preceding): 3 Class A + 6 Class B in pool, TWTT
  trilateration validated. Already done per `phase-economics.md` §2.
- **Phase 1c** (this experiment): 9 → 29 node sweep, scaling-law
  measured. ~$620 incremental BOM + 1 day operator time.
- **Phase 2** (downstream): lake test per `lake-test-protocol.md`
  with N chosen from Phase 1c recommendation; diver-drop scheme
  inherits the TDM schedule.
- **Phase 4+** (long horizon): scaling-law extrapolates to open-
  water 25–50 buoy fleets per `phase-economics.md` §5.2.

## Risk register

- **Pool reverb dominates at N > 15** — T60 of 200–500 ms
  (`architecture.md`) may produce pseudo-collisions where late
  multi-path overlaps the next slot. **Mitigation**: log
  band-specific multi-path energy; mark suspect rows; cross-
  reference against Phase 2 lake test where reverb is lower.
- **Class C node loss during reconfigure** — anchor knot slip,
  drift to deep end. **Mitigation**: $25 per loss is inside
  `phase-economics.md` §1 disposability framing; plan for 20
  units, tolerate 18.
- **WiFi sync flakiness** when fleet master pushes new TDM
  schedules to 14 → 24 nodes in 10 s. **Mitigation**: budget
  20 s ack timeout; hold step and re-push on missed ack.
- **Joint-solver divergence at N > 19** — Gauss-Newton (P4
  §4.8.3) has empirical convergence to ~50 nodes but pool's
  tight baselines reduce conditioning. **Mitigation**: capture
  divergence as data (it *is* the scaling-law signal); shore
  harness falls back to Levenberg-Marquardt damping if needed.
- **TDM schedule infeasible at N = 29** — 35 kHz mesh-comms may
  have < 29 slots/s. **Mitigation**: that *is* the answer —
  saturation point is what this experiment exists to measure.

## Cross-references

- ADR-082 (Class A/B/C architectural roles — fleet-composition
  source).
- ADR-083 (shore-side calibration service — experiment-trace mode
  is this build doc's harness).
- ADR-084 §1.6 + lines 374-381 (WeftAcousticTSF TDM schedule this
  experiment sweeps; original Phase-1c call-out this doc
  operationalizes).
- ADR-086 (carrier prioritization — production policy this
  experiment's scaling-law feeds into).
- ADR-089, ADR-091 (replay-bundle manifest + per-deployment cal
  pattern referenced).
- `build/build-mininode.md` (Class C — incremental unit).
- `build/build-tethered-subsurface.md` (Class B — fixed across sweep).
- `build/lake-test-protocol.md` (Phase 2 — downstream consumer of
  max-buoys-per-cluster figure).
- `build/phase-economics.md` §1 (disposability) + §2 (Class C BOM).
- `panels/P3-firmware-power.md` §10.5 (multi-band coexistence open
  question this closes).
- `panels/P4-data-plane.md` §4.1.3 (`acoustic.timing` rate range)
  + §4.2.4 (latency target) + §4.8.3 (Gauss-Newton solver).
