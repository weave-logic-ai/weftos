# SDR sensing capabilities (mined from sdrtop)

- **Status**: Research / backlog seed — 2026-10-04
- **Source**: [musithang/sdrtop](https://github.com/musithang/sdrtop) (v0.6.4), a terminal
  bench instrument for HackRF One / RTL-SDR / tinySA / SoapySDR radios (~140k LOC Rust).
- **Why**: sdrtop surfaces a set of host-side SDR sensing capabilities that map cleanly onto
  WeftOS cogs and onto RuView's unified RF world model. This doc captures what is worth taking,
  what is **not** takeable, and the capability cards seeded on the WeftOS board from it.
- **Related**: `docs/research/` (RF sensing), the cog contract (ADR-091 cog-sensor-sources,
  ADR-104 guides), COG-012 (publishing pipeline), RuView `docs/notes/` SDR note.

## The gating constraint: license

sdrtop is **GPL-3.0-or-later**, every file SPDX-tagged, contributions required under the same
license. Consequence:

- **Do not vendor or link any sdrtop code** into the MIT `weftos-cogs`, the commercial Cognitum
  cogs, or a proprietary RuView path. Linking GPL-3 makes the whole linked work GPL-3.
- **Ideas and techniques are free** — algorithms and architecture are not copyrightable. Everything
  below is a **clean-room** target: reimplement from the idea and the public protocol, never from
  their source.

If we ever want their code directly, the only clean route is a separately-distributed GPL-3 tool
that talks to our stack over a process/IPC boundary — not a linked dependency.

## Architecture patterns worth adopting (clean-room) in the cog framework

sdrtop's code is unusually rigorous; these patterns are the highest-value, lowest-risk takeaways
and generalise to any high-rate sensor cog (ECG, radar, audio), not just SDR.

1. **Two-thread `arrive` / `digest` split** (`hardware/process.rs`). The driver/callback thread does
   only the minimum at block arrival — timestamp, read capture settings, hand off — and a second
   thread does all heavy work. They measured a HackRF **losing ~1/3 of its samples at 20 Msps**
   because the callback folded the previous block while the radio's buffer overflowed. Our fast
   samplers should never do work on the acquisition thread.
2. **Lossy-by-design telemetry.** `try_send` on a *bounded* channel (drop under load — correct
   load-shedding for a display/ingest feed) **plus a sequence number** so stateful consumers detect
   the gap rather than silently corrupt across it. Good model for cog → store ingest under pressure.
3. **"Count what arrived, measure what was in the band."** Losses are a first-class *reported*
   number, not an inference. Honest telemetry principle for every cog.
4. **Capability-descriptor dispatch.** One RX→FFT→UI pipeline keys off a `DeviceCapabilities`
   descriptor, never `match device_type`. Maps directly onto `cog-sensor-sources`' multi-source
   trait and keeps a cog backend-agnostic.

## Proposed capabilities → cogs (clean-room)

| Capability | What it senses | Hardware | Store-vector sketch | Priority |
|---|---|---|---|---|
| **RF device census** | distinct 2.4 GHz devices present (BLE + classic BT), airtime occupancy, churn | RTL-SDR / HackRF on a Seed (armv7 Linux) | `[device_count/N, airtime_frac, new_devices, left_devices, median_snr, band_occupancy, …]` | **High** — direct occupancy/presence signal for Cognitum |
| **tinySA spectrum occupancy** | calibrated dBm band occupancy / spectral activity | tinySA Ultra over USB serial (~$100) | `[occupied_frac, peak_dbm, noise_floor, n_peaks, …]` | Medium — cheap, concrete hardware |
| **ADS-B presence** | aircraft overhead (count, nearest, activity) | RTL-SDR @ 1090 MHz | `[ac_count, nearest_km, …]` | Low — niche, clean "sensing as data" |
| **AIS presence** | ships in range (count, activity) | RTL-SDR @ 161–162 MHz | `[vessel_count, …]` | Low — niche |

The flagship is **RF device census**: a BLE/BT (and later Wi-Fi) device-population count is an
occupancy/presence sensor, and an RTL-SDR dongle on a Seed makes it physically plausible today.
sdrtop's `signal/net/census.rs` ("a device is a device whichever protocol found it") is the design
to reimplement; its honesty about SNR-vs-RSSI (no calibrated RSSI on BLE) is a trap to copy on
purpose.

**Every cog must still honour the WeftOS cog contract** (ADR-091/104): stdout JSON per window +
8-float store vector, `/status /raw /guide /healthz`, `--once/--interval/--simulate/--help`, and an
ADR-104 `guide/`. `--simulate` matters doubly here — SDR hardware is not always attached, and
sdrtop's "testable with no radio anywhere" architecture is exactly the `--simulate` philosophy.

## RuView connection

RuView is **not** ESP32-bound: its CSI ingest already spans ESP32 serial, Intel 5300,
Atheros/Nexmon, and Intel AX200/AX210 via FeitCSI (RuView ADR-292, Accepted), and it documents a
unified RF world model fusing WiFi CSI + radar + UWB + cellular (ADR-144 UWB, ADR-063 mmWave,
ADR-287 wideband RF tomography, ADR-311 real sensor fusion). Host-side SDR (RTL-SDR/HackRF/SoapySDR)
is a natural **additional RF-sensing adapter** for that model. See the note added in the RuView repo
(`docs/notes/`). These RuView ADRs are documented *design-intent*; confirm implementation state
before asserting any is shipped.

## Related idea: a "projects" tab on the Sensor Explorer

The Explorer (parts / cogs / firmware tabs, D1 + seed scripts) could gain a **projects** tab
indexing external open-source projects that pair with WeftOS sensing (sdrtop, RuView, SoapySDR, …) —
each row: name, repo, license, paired hardware, related cogs/sensors (reusing the `maps_to` link
pattern). It turns the Explorer from "parts + our cogs" into "the sensing ecosystem around WeftOS."
Small, well-fitted addition (migration + seed + one tab). Tracked as its own board card.
