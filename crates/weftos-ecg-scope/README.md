# weft-ecg-scope

A small WeftOS egui app for setting up the `sen0213-ecg` cog on a Cognitum Seed. The cog reads a DFRobot SEN0213 (AD8232) ECG through an ADS1115 on the Seed's I2C header. The app walks you through the wiring, shows the live signal, and helps calibrate it. It runs natively and in the browser (WASM).

## Run

```bash
scripts/build.sh ecg-scope                         # native
SEED_HOST=169.254.42.1 target/release/weft-ecg-scope

scripts/build.sh ecg-scope-web                     # browser: wasm + wasm-bindgen into www/pkg
python3 -m http.server 8091 -d crates/weftos-ecg-scope/www
# open http://127.0.0.1:8091/?seed=169.254.42.1
```

`SEED_HOST` (or `?seed=` in the browser) is the Seed's address: `169.254.42.1` over USB, or its LAN or tailnet IP. Everything else is derived from it:
- the agent API at `http://<seed>`;
- the cog's signal export at `http://<seed>:8046`.

There's no SSH and no TLS. The agent serves plain HTTP on port 80 over USB, LAN and the tailnet, and both it and the cog export send `Access-Control-Allow-Origin: *`, so the browser build calls them directly. `COGNITUM_SEED_TOKEN` sets a pairing token if the agent refuses writes.

## What it does

- **Hook-up checklist.** Steps are checked in wiring order from live data. Each failing step names the fix, and later steps wait until it passes:
  1. Seed agent reachable
  2. Cog installed
  3. Cog running
  4. Signal export reachable
  5. ADS1115 found on I2C (shows the cog's I2C error plus the pin list)
  6. Sensor powered (baseline near 1.65 V, not flat)
  7. Electrodes attached (not at the rails)
  8. Heartbeats detected (quality ≥ 0.8)
  9. Lead polarity (R-waves upright, or swap RA/LA)
  10. Mains notch (the hum frequency matches the configured notch)
- **Wiring table** and electrode placement, always visible.
- **Live graph:** the filtered ECG in mV with R-peak markers, an optional raw-volts trace, a 2-30 s window, and Freeze.
- **Calibration** over the last 4 s:
  - baseline volts and the fraction of samples at the rails;
  - 50 and 60 Hz hum (Goertzel on the raw signal);
  - median R amplitude and polarity;
  - noise floor (first-difference RMS away from QRS) and SNR;
  - SDNN and RMSSD;
  - sampling health (late samples, I2C errors).
- **Cog settings,** written through the agent's config API: mains notch, ADS1115 address, ADC input, sample rate, simulate. Applying restarts the cog. **Apply recommended notch** sets the notch to the measured hum frequency. Edits are merged into the full config, because the agent's PUT replaces it.
- **Pulse cross-check:** tap along with the wrist pulse (button or Space) and compare with the cog's heart rate.
- **Record:**
  - the last 60 s as CSV (`t_ms,raw_v,filtered_mv,r_peak`);
  - a RuView ADR-293 reference series (`timestamp_ms,value`: beat-to-beat bpm). That series is ground truth for grading `wifi-densepose-vitals`.

  Natively both are written to files; in the browser they're copied to the clipboard.
- **Guide tab (ADR-104):** the cog's own sensor guide, fetched from `http://<seed>:8046/guide`. It's a searchable mini-wiki covering start, parts, wiring, electrodes, setup, calibration, troubleshooting, API reference, safety and glossary, with painted header, wiring, placement and flow diagrams, rendered by `weftos-sensor-guide`. Each checklist step has a "?" that opens its page. Set `COMPANION_GUIDE_DIR=<cog>/guide` (native) to author against a local folder.
- **Start, stop and test-run** the cog (`--once --simulate`, which needs no hardware).

Not a medical device. Power the Seed from a battery while pads are on a person.

## Layout

It's built on `weftos-cog-companion`, which provides the connection bar, the agent API, the four connection checklist steps, the cog settings generated from the manifest, the Guide tab, and the native and web entry points. The environment variables (`SEED_HOST`, `COGNITUM_SEED_TOKEN`, `COMPANION_*`) are listed in that crate's README.

| File | Role |
|---|---|
| `src/app.rs` | `EcgApp`: polls `/raw`, draws the ECG plot (R markers snapped to the waveform), the readouts, calibration, recommended notch, tap-along check and recording |
| `src/checklist.rs` | ECG sensor steps (ADS1115 found, power, electrodes, beats, polarity, mains notch) |
| `src/model.rs` | merged 60 s signal buffer, CSV and RuView reference export |
| `src/analysis.rs` | calibration maths and tap tempo |

The cog lives in the private cogs repo (`src/cogs/sen0213-ecg`, ADR-158).
