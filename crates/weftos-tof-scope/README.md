# weft-tof-scope

The companion app for the `sen0628-tof` cog: a DFRobot SEN0628 8×8 matrix time-of-flight sensor (VL53L7CX + RP2040) on a Cognitum Seed's I2C header. It's built on `weftos-cog-companion`, which provides the connection, checklist, manifest-driven settings and the cog's own guide, and it runs natively and in the browser.

```bash
scripts/build.sh scope tof && SEED_HOST=169.254.42.1 target/release/weft-tof-scope
scripts/build.sh scope-web tof && python3 -m http.server 8092 -d crates/weftos-tof-scope/www   # open /?seed=169.254.42.1
```

## What it shows

- **Depth zones.** A live heatmap of the 8×8 or 4×4 frame, near warm and far cool, with the distance per zone and an adjustable colour scale.
  - The nearest zone is outlined in white.
  - Zones closer than the cog's learned background by more than `presence_mm` are outlined in orange.
  - Zones with no target are grey.
  - Both outlines are computed from the displayed frame.
- **Timeline.** Nearest distance and frame-to-frame motion over the last 30 s.
- **Checklist,** after the connection steps:
  1. SEN0628 found on I2C
  2. Frames arriving
  3. Zones see a target
  4. Background learned
  5. Mounting level (flat-wall check)
  6. Zone noise
- **Readings:** nearest distance and zone, presence, mode, frame rate, valid share, median, motion, zone noise, left/middle/right sector distances, read errors.
- **Calibration.**
  - **Check level** compares the top and bottom rows and the left and right columns against a flat wall, as a percentage of the mean; ±5 % counts as level.
  - The noisiest zone over the last 3 s is shown.
- **Record:** the last 30 s of frames as CSV (`t_ms,side,z0..zN`).

It reads the cog's export at `http://<seed>:8047`: `/frames`, `/status` and `/guide`. The cog source, guide and ADR-159 are in the private cogs repo (`src/cogs/sen0628-tof`).
