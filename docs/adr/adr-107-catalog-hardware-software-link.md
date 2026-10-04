# ADR-107: The hardware catalog links modules to cogs

- **Status**: Accepted (2026-10-03, owner feedback: "I cannot see how to install this sensor, or if we have a cog installed, or if one is available")
- **Date**: 2026-10-03
- **Deciders**: Owner / platform
- **Depends-On**: ADR-104 (sensor guides ship with sensor cogs), ADR-105 (cog sources), COG-009 (appliance console)
- **First implementation**: `crates/weftos-cog-market` (`hw.rs`: `Module::cogs`, `firmware`, `docs`, `seen_in`); `crates/weftos-cog-manager` (`sensor_link.rs`, `sensor_detail.rs`)

## Context

The appliance console had three tabs that never met. The Catalog tab described a module (the HLK-LD2450 card showed a summary, price and spec rows), the Cogs tab managed cogs, and the Sensors tab rendered a running cog's guide. Nothing said that the `ld2450-radar` cog drives the LD2450, whether it was available, installed or running, how to wire the sensor, or where its firmware and docs were.

## Decision

1. **One source of truth, on the hardware side.** Each catalog `Module` carries `cogs: [<cog id>]`, the ids of the cogs that drive it (the cog's own `[cog].id`). A supporting board lists every cog that needs it (`ads1115-board` lists `sen0213-ecg` and `sound-detect`). The reverse lookup, `HwCatalog::modules_for_cog`, is computed, never stored, so the two directions cannot drift. An empty list means no cog yet.
2. **Optional module facts live beside it.**
   - `firmware`: `version`, `read_with`, `read_config_key`, `update`, `url`, `notes`. Empty strings mean unknown, never a guess. `read_with` says how a cog reads the version off the module (ld2450: `query_firmware`); the update field only points at the vendor route. WeftOS does not update module firmware.
   - `docs`: `[{label, url}]`, where `url` is an `http(s)` link or a repo-relative / `repo: path` reference shown as text.
   - `seen_in`: the source documents the entry was drawn from (already in the JSON, now typed).
3. **Availability and state are never stored in the catalog.** They come from the marketplace catalog the Cogs tab already loads (WeaveLogic signed, Cognitum) and the host's `/status` (installed, running, stopped, refused by the licence gate). The join is `sensor_link::cog_view`.
4. **The module card is the detail panel.** Expanding it shows Software, Setup, Firmware, Docs and, for running cogs, Stats.
   - Software: the cog, the sources offering it, install state, and Install / Start / Stop / Configure / Open guide.
   - Setup: catalog pins, bus enablement steps, power, and the cog's own guide wiring when it is being served.
   - Firmware: the facts above and whether the cog can read the version.
   - Docs: datasheet, the Sensors-tab guide (deep link), curated links, buy link, catalog source.
   - Stats: host supervision facts plus the cog's own `/status` output line on its export port.
   The actions call the same client methods the Cogs tab uses (`install`, `lifecycle`). The host has no config-write endpoint, so Configure opens the cog's guide at its config-keys page rather than writing anything.
5. **Cross-links both ways.** The Sensors tab and the Cogs tab (running rows and marketplace rows) link to the hardware a cog needs, and open that module's card in the Catalog. The Catalog list badges each module `cog available`, `installed` or `running`.
6. **Works with no host and in WASM.** With no host the panel shows availability only and Install is disabled with a reason. All logic is in egui-independent functions; the cog's export `/status` is fetched with `ehttp` like the guide.
7. **Mesh-wide, not one device.** The panel and badges show where a cog is on the mesh: per node, the version, state, restarts, uptime, last output and output-log size. The console never polls peers. It asks the connected host for `GET /mesh/cogs`, and the host reads its own supervisor and asks each online tailnet peer's `/status` (literal tailnet IPs only, bounded, cached 5 s; a peer that is not a cog host is listed as unreachable with the reason). With no answer, or a host that predates the route, the view is the connected host alone and says `this host only (<why>)`. The daemon's placement layer (`workload.list`) is not consulted yet: `weftos-cog-host` has no daemon link, and the daemon RPC is not browser-reachable. **Queue depth does not exist**: cogs post straight to the store. The only backlog a host holds is each cog's output log, so `/status` gains `log_bytes` and `log_age_s` (from `host.log`) and the panel labels them as the output log, not a queue.
8. **No cog yet** is stated plainly, with how to request one (the dashboard board) and how to build one (the `sensor-cog` skill).

## Consequences

- Adding a cog for a module is a one-line catalog edit plus the cog; the console picks it up. `scripts/consolidate-catalog.py` unions `cogs` when it merges duplicate modules.
- `HwCatalog::validate` rejects malformed or duplicated cog ids, empty doc links and a firmware `read_config_key` with no `read_with`. It cannot check that a cog id exists, because the cogs live in other repos; an unlisted cog shows as `cog (not published)`.
- The cog id to export port map (`sensor_link::default_export_port`) is still hand-kept until the host reports the port in `/status`.
- Cogs that are not hardware-bound (`bridge`, `catalog`, `sensor-ota-push`) have no module and no panel.
