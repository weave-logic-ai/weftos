# ADR-111: Sensor evidence and readings contracts

- **Status**: Accepted (owner decision, 2026-10-07)
- **Deciders**: owner
- **Origin**: the `spatial.evidence.v1` wire format first appeared in the spatial
  workspace's ADR-107 §7 and its 2026-10-03 amendment (§7.1). That workspace is not
  published, so this ADR is the canonical public home of the format.
- **Consumers**: RuView ADR-384 (external sensor ingest), the Cognitum sensor cogs, the
  spatial evidence engine.
- **Files**: `contracts/sensors/quantities.v1.json`, `contracts/sensors/vectors/`,
  `contracts/sensors/README.md`.

## Context

Producers (cogs, firmware readers, RuView's own RF export) and consumers (RuView, the
spatial engine) need two shared formats: one for spatial perception evidence and one for
readings that are not spatial, such as temperature or door state.

Two things forced a decision now:

1. RuView ADR-384 and its plan cite "WeftOS ADR-107" for the evidence format. In this
   repository ADR-107 is a different decision (the catalog links modules to cogs). The
   citation cannot be followed.
2. The sound-ranging and snapshot-sounding work needs three record types the format lacks:
   an acoustic range, a cooperative radio delay, and a clock correction row.

## Decision

1. WeftOS publishes both contracts here: `spatial.evidence.v1` (evidence) and a SenML
   profile (readings).
2. Three record types are added inside `spatial.evidence.v1`: `acoustic_range`,
   `radio_delay`, `clock_correction`.
3. The quantity vocabulary for readings is `contracts/sensors/quantities.v1.json`, owned
   by WeftOS and consumed by RuView and the cogs.
4. **Number collision.** This ADR takes number 111. ADR-107 here keeps its meaning. Citations
   of "WeftOS ADR-107 §7" for the wire format, in RuView ADR-384, its plan and elsewhere,
   should move to ADR-111. The spatial workspace's ADR-107 §7 now points here.

Everything below is normative. "MUST" and "MUST NOT" are used in the RFC 2119 sense.

## 1. Evidence: `spatial.evidence.v1`

Versioned JSONL, one record per line, at most 16 KiB per line.

### 1.1 Envelope (every record)

| Field | Type | Rule |
|-------|------|------|
| `schema` | string | Exactly `spatial.evidence.v1`. Checked before the typed parse, so a v2 line is a version mismatch |
| `type` | string | One of the types in §1.3 and §1.4 |
| `t_ns` | u64 | Nanoseconds since the Unix epoch on the producer's clock, kept as sent |
| `frame` | string | Exactly `room_enu` |
| `region` | id | Urth region id (`region/urth/meso/<room>`) |
| `source_id` | id | Device or operator that observed it, never a person |
| `uncertainty_m` | f64 | 1σ in metres, in (0, 100]. What it measures per type is in §1.3 and §1.4 |
| `provenance` | object | `receipt` (id, unique per record), `producer` (id, such as `ld2450-radar@0.1.0`), `proof` |

- **Frame `room_enu`:** room-local ENU, origin at the room's south-west floor corner, x
  east, y north, z up, metres. The Urth region carries the transform to ECEF. It is not
  repeated per line.
- **Ids** are 1 to 128 bytes of ASCII letters, digits and the characters `.` `_` `:` `/` `-` `@`.
- **Labels** are 1 to 64 characters with no control characters.
- **Proof** is `MEASURED` (direct physical measurement with a reproducer), `CODE` (derived by
  code from other evidence) or `SYNTHETIC` (simulated or generated). There is no `CLAIMED`.
- **Coordinates** are within ±1000 m. All numbers are finite; NaN, infinity and JSON
  overflow such as `1e999` are rejected.
- **Yaw** in every type is degrees counter-clockwise from room +x (0° faces east, 90° faces
  north), in [−360, 360]. **Pitch** is elevation above the horizontal, positive up, in
  [−90, 90]. **Roll** (ToF only) is clockwise looking out, in [−180, 180]. **Field of view**
  is the full angle `[horizontal, vertical]`, each in (0, 180).
- A JSONL document aborts on its first bad line and reports the line number. A corrupt file
  is never partially ingested.
- Unknown extra fields are accepted and ignored.

### 1.2 Versioning

- Adding an optional field, or adding a new record `type`, stays inside v1. No existing line
  changes meaning, and every line that parsed before parses identically.
- A reader that does not know a `type` rejects the line as an unknown type. A stream
  consumer counts it and carries on; it MUST NOT treat it as fatal to the stream and MUST
  NOT guess at its meaning.
- Removing or renaming a field, or changing what a field means, needs `spatial.evidence.v2`.
- Producers that must feed an older reader keep newer types behind an option, off by default.

### 1.3 Existing record types

Carried over from the spatial ADR-107 §7 and §7.1 without change. The reference validator
accepts exactly these rules.

| Type | Required fields | Optional | Rules |
|------|-----------------|----------|-------|
| `shell_measure` | `min`, `max` [x,y,z] m | none | Interior box, `min < max` on every axis |
| `pose` | `node_id` (id), `position` [3] | `yaw_deg` | A device pose, never a person |
| `radar_track_point` | `track` (u32), `position` [3] | `velocity` [3], `sensor` | `track` is an ephemeral per-session integer, never an identity. Velocity components within ±50 m/s |
| `rf_link_observation` | `tx`, `rx` [3], `freq_hz`, `excess_loss_db` | none | `freq_hz` in [1e8, 3e11]; loss beyond free space in [−60, 200] dB, negative is constructive multipath |
| `rf_gaussian` | `position` [3], `scale` [3], `orientation` [w,x,y,z], `occupancy`, `confidence`, `motion` | `role` | `scale` each in [1e-6, 1e4] m; orientation non-zero; `occupancy` in [0, 1e6] nepers/m; `confidence` in [0, 1]; `motion` is `static`, `slow` or `fast`; `role` is `absorber` or `reflector` |
| `uwb_echo` | `anchor` [3], `range_m` | `direction` [3] | `range_m` in (0, 1000]; `direction` non-zero |
| `imu_event` | `node_id`, `event` (label), `magnitude` | none | `magnitude` in [0, 1e4] m/s² |
| `human_confirm` | `min`, `max`, `label`, `occupied` (bool) | none | Ordered box |
| `tof_depth` | `position`, `yaw_deg`, `pitch_deg`, `fov_deg`, `grid` [columns, rows], `range_mm`, `valid` | `roll_deg` | Grid sides 1 to 16. `range_mm` (u32, ≤ 50 000) and `valid` (bool) have exactly columns·rows entries, row-major, row 0 the top row and column 0 the leftmost zone looking out. Zone (row, col) has azimuth `(col + ½)/columns · h − h/2` (positive right) and elevation `v/2 − (row + ½)/rows · v` (positive up). Invalid zones claim nothing |
| `radar_range` | `position`, `yaw_deg`, `fov_deg`, `range_m` | `pitch_deg` (default 0), `targets` | `range_m` in (0, 1000]; `targets` 0 to 64, 0 claims nothing |

`radar_track_point.sensor` (optional beam geometry) holds `position` [3], `yaw_deg`,
`elev_half_deg` in [0.1, 89] and optionally `pitch_deg` (default 0) and `az_half_deg` in
[0.1, 180]. The emitter applies pitch before writing `position`: the in-plane forward
distance `y` becomes `y·cos(pitch)` on the floor.

`uncertainty_m` is the 1σ position uncertainty of the record's geometry. For `tof_depth` it is
the 1σ of the range.

### 1.4 New record types

All three follow the envelope in §1.1. Each carries a stated source of its numbers, so a
consumer never has to guess whether a value is a measurement or a derivation.

#### `acoustic_range`

One range from an ultrasonic or audible ping (HC-SR04, JSN-SR04T, Marvelmind class, or a
peak picked from a microphone impulse response). It is not `tof_depth` (optical, many
zones) and not `uwb_echo` (radio). `uncertainty_m` is the 1σ of `range_m`.

| Field | Type | Meaning |
|-------|------|---------|
| `position` | [x,y,z] m | Transducer, room ENU |
| `yaw_deg`, `pitch_deg`? | deg | Boresight; pitch defaults to 0 |
| `beam_deg` | [h,v] deg | Full beam width, each in (0, 180) |
| `range_m` | m | In (0, 200]. Already corrected with the speed of sound below |
| `kind`? | `echo` or `direct` | Default `echo` |
| `peer`? | id | The other node. Required when `kind` is `direct`, forbidden when `echo` |
| `sound_speed_mps`? | m/s | Speed used to turn flight time into range, in [300, 400] |
| `air_temp_c`? | °C | Air temperature that speed came from, in [−50, 100] |
| `peak_rank`? | u32 | 1-based rank of the picked peak in the echo train (1 is first), 1 to 16 |

- `echo`: something hard lies at `range_m` somewhere inside the beam. Bearing within the
  beam is unknown. One echo train is not a room shell.
- `direct`: the direct arrival between this node and `peer`, so `range_m` is the baseline
  between the two devices. The peer's own `pose` places the other end.
- Proof: `MEASURED` for a real capture, `SYNTHETIC` for a simulated one. A producer that
  has no temperature correction omits `sound_speed_mps`, and the consumer widens its
  uncertainty. Humidity and drafts still move the path.

#### `radio_delay`

Cooperative radio delay between two scheduled nodes (UWB two-way ranging, a Wi-Fi preamble
snapshot, and similar). It is neither `uwb_echo` (a reflection from one anchor) nor
`radar_range`. `uncertainty_m` is the 1σ of the implied inter-node distance in metres:
`c · delay_sigma_ns / 2` for a round trip and `c · delay_sigma_ns` for a one-way delay.
It is informative; consumers use `delay_sigma_ns`.

| Field | Type | Meaning |
|-------|------|---------|
| `tx_node`, `rx_node` | id | Node scheduled to transmit (initiator of a round trip) and node scheduled to listen (responder). MUST differ |
| `mode` | `one_way` or `round_trip` | One flight, or there and back |
| `direction` | `forward`, `reverse` or `both` | `forward` is `tx_node` to `rx_node`, `reverse` is the other way, `both` is a round trip |
| `delay_ns` | ns | The delay. Round trip: the sum of both flights, with the responder's fixed turnaround removed |
| `delay_sigma_ns` | ns | 1σ of `delay_ns`, in (0, 100 000] |
| `offset_free` | bool | True when `delay_ns` holds no clock offset |
| `carrier_hz`? | Hz | In [1e8, 3e11] |
| `bandwidth_hz`? | Hz | In [1e3, 1e11]. Delay resolution is about `c / B` |
| `method`? | label | Short method name, such as `uwb_twr` |

- `mode` and `direction` MUST agree: `one_way` takes `forward` or `reverse`, `round_trip`
  takes `both`. A pair is recorded under one canonical node order, so `reverse` exists to
  say which way a one-way signal flew without swapping the nodes.
- A round trip MUST have `offset_free` true: the two clock offsets cancel in the exchange.
- A one-way delay has `offset_free` true only after a `clock_correction` has been applied.
  Otherwise it is an apparent delay (clock offset plus flight), `delay_ns` is in
  [−1e9, 1e9], and a consumer MUST NOT turn it into a distance.
- When `offset_free` is true, `delay_ns` is in (0, 100 000] (up to about 30 km of path).
- Proof: `MEASURED` on a real capture. `SYNTHETIC` when a file's truth annotation supplied
  the simulated delay. A single correlation peak over a white-noise threshold is not a
  detection in the 2.4 GHz band, so producers SHOULD check an off-air code as the noise
  reference before emitting `MEASURED`.

#### `clock_correction`

One node's clock fit. The node keeps its counter running and stamps samples with the raw
local time. The fit says how to turn that stamp into shared time:

```
t_shared = t_local − (offset + rate × (t_local − t_anchor))
```

All times in nanoseconds, `t_local` and `t_anchor` on the node's local clock, `rate`
dimensionless. `uncertainty_m` is the 1σ of the known baseline length that the direct-path
residual was solved against.

| Field | Type | Meaning |
|-------|------|---------|
| `node` | id | Node whose local clock the fit corrects |
| `anchor_t_ns` | u64 | Anchor instant, local clock |
| `offset_ns` | ns | Offset at the anchor, within ±1e13 |
| `rate` | f64 | Rate error, seconds gained per second, within ±1e-3 (1000 ppm) |
| `residual_ns` | ns | RMS residual of the fit, in [0, 1e9] |
| `fit_points` | u32 | Rows (chirps) used, 1 to 65 535 |
| `basis` | string | `direct_path`. No other value is accepted in v1 |
| `master`? | id | Node whose chirp defined the slot, if one did |

- **Direct path only.** The residual MUST come only from direct-path arrivals over a known
  distance. A wall or person reflection MUST NOT enter the fit, or the rate follows the
  person. `basis` states this and is the only accepted value.
- `fit_points` below 2 cannot show a rate, so then `rate` MUST be 0.
- `t_ns` (envelope) is when the fit was produced; `anchor_t_ns` is on the node's own clock.
  They are different clocks and are not compared.
- A new fit replaces the previous one for that node and is applied backward over the
  samples in the interval it closes. Stored records are never rewritten. The correction is a
  separate fact, so a read returns both the raw and the corrected time.
- Until a fit exists, consumers use the producer time as sent and report per-source skew.
- The solver publishes the row to every node, including the one that transmitted, so one
  device's drift does not become the room's clock.

Example lines, one per new type (the evidence vectors under
`contracts/sensors/vectors/evidence/valid/` hold these and the rest):

```jsonl
{"schema":"spatial.evidence.v1","type":"acoustic_range","t_ns":1759500005000000000,"frame":"room_enu","region":"region/urth/meso/test-room","source_id":"sonar-1","uncertainty_m":0.02,"provenance":{"receipt":"sonar-1:000311","producer":"jsn-sr04t@0.1.0","proof":"MEASURED"},"position":[0.1,1.83,1.0],"yaw_deg":0.0,"beam_deg":[60.0,60.0],"range_m":6.31,"kind":"echo","sound_speed_mps":343.2,"air_temp_c":20.0,"peak_rank":1}
{"schema":"spatial.evidence.v1","type":"radio_delay","t_ns":1759500005500000000,"frame":"room_enu","region":"region/urth/meso/test-room","source_id":"node-2","uncertainty_m":0.12,"provenance":{"receipt":"uwb-1-2:000045","producer":"uwb-twr@0.1.0","proof":"MEASURED"},"tx_node":"node-1","rx_node":"node-2","mode":"round_trip","direction":"both","delay_ns":36.7,"delay_sigma_ns":0.8,"offset_free":true,"carrier_hz":6489600000.0,"bandwidth_hz":499200000.0,"method":"uwb_twr"}
{"schema":"spatial.evidence.v1","type":"clock_correction","t_ns":1759500006000000000,"frame":"room_enu","region":"region/urth/meso/test-room","source_id":"solver-1","uncertainty_m":0.01,"provenance":{"receipt":"clk-node2:000012","producer":"chirp-sync@0.1.0","proof":"MEASURED"},"node":"node-2","anchor_t_ns":1759500005900000000,"offset_ns":125000.0,"rate":2e-5,"residual_ns":14.0,"fit_points":12,"basis":"direct_path","master":"node-1"}
```

### 1.5 Privacy

`source_id` names devices and operator consoles, never people. `track` is ephemeral. None of
the three new types identifies a person. They describe devices and clocks.

## 2. Readings: SenML profile

Readings describe the state of an object or space, not people. They use SenML (IETF
RFC 8428), JSON form, one pack (a JSON array) per line, at most 16 KiB per line and 256
records per pack. SenML base fields (`bn`, `bt`, `bu`, `bver`) carry forward to later
records in the pack as RFC 8428 defines.

### 2.1 Profile rules

| Rule | Detail |
|------|--------|
| Source id | `bn` is required. It is the source id (an id as in §1.1) followed by one `/`. The slash makes the resolved SenML name, `bn` + `n`, read `source/quantity` and match binding-table keys |
| Proof | `proof_` is required on every measurement record and is `MEASURED`, `CODE` or `SYNTHETIC`. The trailing underscore makes it must-understand under RFC 8428, so a reader that does not know it MUST reject the record. SenML does not inherit it from a base record |
| Other `_` fields | Any other field name ending in `_` MUST be rejected |
| Name | `n` is required and is a quantity name from the vocabulary. Unknown names are accepted, stored and counted. Nothing consumes them until they are added |
| Value | Exactly one of `v`, `vb`, `vs`, `vd`. A known quantity takes the one its vocabulary entry names (`v` numeric, `vb` boolean) |
| Unit | For a known numeric quantity the effective unit (`u`, else `bu`) MUST equal the vocabulary unit exactly. A boolean quantity carries no unit |
| Range | A known numeric value MUST fall in the quantity's physical `range` in the vocabulary |
| Time | `bt + t` MUST be absolute Unix seconds (at least 2^28, RFC 8428's threshold). Relative times are rejected. Wire time maps to `t_ns` as `t × 1e9` |
| `entity` | Optional. `space:<id>`, `object:<id>`, `track:<id>` or `occupant:<id>`, id as in §1.1. It names the world-model entity the reading describes. A binding table in the consumer's configuration maps `source/quantity` to an entity when `entity` is absent, and a record's `entity` overrides it |
| `sigma` | Optional. 1σ in the reading's own unit, greater than 0, numeric values only |
| `bver` | Absent or 10 |

### 2.2 Units

RFC 8428 limits `u` to primary units, and RFC 8798 lets a definition allow secondary units
through a must-understand field. This profile is identified by `proof_`, and it allows the
secondary units the vocabulary names (`ppm`, `ug/m3`). Each quantity still has exactly one
unit, so a consumer needs no conversion table beyond the vocabulary. The vocabulary records
each secondary unit's primary equivalent (scale and offset) for consumers that want it.

Unit strings were checked against the IANA SenML Units registry and its Secondary Units
subregistry on 2026-10-07. Two obvious units are not in either registry: `ppb` and
`breath/min`. The vocabulary does not invent them. TVOC is written in `ppm` (a ppb sensor
divides by 1000) and respiration rate in `1/min`, the registered per-minute unit.

### 2.3 Subjects, vitals, cues and covariates

- Every quantity has a `subject`: `space-or-object`, or `person-track-only` for heart rate
  and respiration rate. A person-track quantity MUST carry an explicit `entity` of kind
  `track:` or `occupant:`, and a `space-or-object` quantity MUST NOT. The wire format grants
  no authority to publish vitals. Publication gates belong to the consumer.
- Readings are not evidence about people by default. A reading enters perception only
  through a declaration in the vocabulary file:
  - A **cue** is a state change that implies a person acted (a door opened, a stove drew
    power, CO2 rose in a closed room). It has a trigger, an entity kind and a likelihood.
  - A **covariate** is a reading that changes how a sensor behaves (temperature and humidity
    shift Wi-Fi CSI, a metal door changes multipath). It routes to calibration or
    domain-state checks and never to the person estimate.
- Declared likelihoods start as `unmeasured` placeholders. A cue feeds fusion only after its
  likelihood has been measured, and never decides presence alone. Each declaration carries a
  test vector in the vocabulary file.

### 2.4 Versioning

The vocabulary file has its own `version` (semantic). Adding a quantity, cue or covariate is
a minor version and a one-entry change with a test vector. Changing a quantity's unit or
range, or removing one, is a major version and needs a new file name (`quantities.v2.json`).
Consumers record the version they loaded.

## 3. Conformance

`contracts/sensors/vectors/` holds valid and invalid test lines for every evidence type (all
13) and for every profile rule above, with a `manifest.json` stating what each file expects.
Evidence rejections carry a class: `version`, `unknown_type`, `shape`, `range`, `text` or
`size`. A consumer conforms when it accepts every valid vector and rejects every invalid one
for the stated reason class. The spatial reference validator (`weftos-spatial-core`) runs the
evidence vectors in its tests.

A producer conforms when its output passes a consumer's check tool. No access to the device
is needed.

## Consequences

- RuView, the cogs and the spatial engine share one citable, public format. RuView ADR-384
  and its plan should cite ADR-111, and can move its "not yet in v1" rejection of the three
  new types to acceptance in the same change.
- Two validators (RuView's and the spatial reference) can be cross-checked on the same vectors.
- The new records are accepted and validated now. The spatial engine does not yet write any of
  them to its voxel map: they have no source kind in the engine.
- The slash in `bn` is a small departure from ADR-384's wording ("`bn` is the source id").
  It is what RFC 8428 name resolution needs, and ADR-384 should state it the same way.
- Format changes ripple both ways. A new evidence type or vocabulary entry is added here
  first, and consumers add it in the same change.

## Related

- Spatial workspace ADR-107 §7 and §7.1 (origin of the evidence format).
- RuView ADR-384 (consumer), IETF RFC 8428 (SenML), RFC 8798 (additional SenML units).
- ADR-104 (sensor guides), for how sensor cogs describe themselves to users.
