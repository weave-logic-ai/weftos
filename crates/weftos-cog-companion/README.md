# weftos-cog-companion

The shared shell for sensor-cog companion apps (ADR-104). A sensor app implements `SensorApp`:
- its live view;
- its checklist steps;
- its calibration panel;
- any extra polling of its cog's export.

Everything else comes from `Companion`, natively and in the browser:

| Piece | What it does |
|---|---|
| Connection bar | One **Seed host** gives the agent API (`http://<seed>`, port 80) and the cog export (`http://<seed>:<port>`). There's an optional pairing token, plus Start, Stop and Test run. |
| Hook-up checklist | Four connection steps (agent, cog installed, cog running, export), then the app's steps. They're blocked in order, and each has a **?** that opens the guide page through `[links]` in the cog's `guide.toml`. |
| Cog settings | Generated from the cog's manifest (`GET /api/v1/apps/<id>/manifest`): integer, float, boolean, select and string fields, with ranges, units and an Advanced section. Edits are merged into the current config before `PUT`, because the agent replaces the whole config and restarts the cog. |
| Guide tab | The cog's own guide from `<export>/guide`, rendered by `weftos-sensor-guide`. |
| Native and web entry | `run_native(app, title)` and `start_web(app, canvas_id)`. |

**Polling.** `CogClient` runs ehttp polling lanes with one request in flight each and recovery from stale requests. A generation counter drops replies from a previous Seed. `poll_export(path, every_ms, ...)` lets an app add its own lanes.

**Environment (native):**
- `SEED_HOST`;
- `COGNITUM_SEED_TOKEN`;
- `COMPANION_GUIDE_DIR=<cog>/guide`, to author against a local guide folder;
- `COMPANION_TAB=guide` with `COMPANION_PAGE=<id>`, to open on a guide page;
- `COMPANION_SCREENSHOT=<file.bmp>` with `COMPANION_SCREENSHOT_AFTER=<s>`, for headless visual checks.

In the browser, `?seed=<host>` picks the Seed.

## Apps built on it

| App | Cog | Export | Build |
|---|---|---|---|
| `weft-ecg-scope` | `sen0213-ecg` (SEN0213 ECG through an ADS1115) | :8046 | `scripts/build.sh scope ecg` |
| `weft-tof-scope` | `sen0628-tof` (SEN0628 8×8 matrix ToF) | :8047 | `scripts/build.sh scope tof` |

**A new sensor app** is a crate `crates/weftos-<name>-scope` with a `SensorApp` implementation, a `run_native` wrapper and a `#[wasm_bindgen]` start function, plus `www/index.html`. Build it with `scripts/build.sh scope <name>` and `scope-web <name>`.
