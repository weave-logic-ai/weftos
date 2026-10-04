# Network tab as Fleet Manager: research and design brief

Date: 2026-10-04. Read-only research; no code was changed. Paths are relative to `/Users/mathewbeane/weftos` unless noted. "not found" means I searched and did not find it. Labels: **[V]** read in source, **[U]** unverified (live system not reachable or card not readable).

## 0. Summary

- The Network tab is three flat tables fed by ONE endpoint on ONE host: `GET /network` on the `weft-cog-host` the console is pointed at. It shows name, IP, OS and online for tailnet peers, a raw JSON dump of Cognitum-overlay peers, and a heartbeat roster of edge nodes (kind, sensor, rssi, battery, age). No row is clickable. There is no detail view, no action, no load, no location, no trust or licence state.
- The console never talks to the WeftOS daemon. `weftos-cog-host` has no `clawft-rpc` dependency (`crates/weftos-cog-host/Cargo.toml`, grep for `DaemonClient` in `crates/weftos-cog-host/src` finds nothing). All the rich fleet data (signed node facts, trust tier, placement, instances, leases, licence binding, infer adverts, admission class, revocation) lives behind daemon RPCs (`cluster.*`, `workload.*`, `infer.*`, `mesh.*`) and the machine mesh service. **That missing bridge is the central gap.** Everything else is a field-level gap.
- What the system can already report is much richer than the tab shows, for full nodes (signed facts with cores, model, OS, kernel, memory, storage, accelerators, trust tier, live `mem_free`) and for placed cogs (state, restarts, rss, uptime, licence refusal). For ESP32 and smaller the data is thin by design: the heartbeat carries only id, kind, sensor, rssi, battery, fw, ip (`crates/weftos-cog-host/src/fleet.rs:29-44`).
- Not found anywhere: CPU load average or per-node CPU percent, per-peer RTT surfaced over any RPC (the `PeerMetrics.rtt_ms` struct exists, `crates/clawft-kernel/src/mesh_heartbeat.rs:392-396`, but nothing in `clawft-weave` or `clawft-mesh-service` reads it), physical location or site label, an "identify/blink" action, a logs RPC for a node (only per-instance `workload.logs`), a drain/cordon verb.
- Safest design: a daemon-side read-only "fleet snapshot" aggregator that the console reads through the existing cog-host (or gateway) path, with no new trust path and no write verbs in phase 1.

## 1. Current state of the Network tab

| What | Where |
|---|---|
| Tab list: Cogs, Sensors, Catalog, Network, Apps, System | `crates/weftos-cog-manager/src/lib.rs:30-38` (enum), nav at `:306` |
| Dispatch `Section::Network => self.network_view(ui)` | `lib.rs:831` |
| `network_view`: header "the fleet this OS is part of", reads `client.snapshot().net`, three states (ok / error "Can't reach the host's /network" / "Querying the mesh…") | `lib.rs:543-555` |
| `fleet_tables`: "this node: <hostname>" | `lib.rs:557-560` |
| Tailnet table (4 cols: node, tailnet IP, os, state), self first, then online, then by name; "N nodes, M online" | `lib.rs:562-592` |
| Cognitum mesh overlay: only `count`; if count 0 prints "cog0 is alone on the Cognitum mesh"; otherwise each peer as raw `{}` JSON text | `lib.rs:594-612` |
| "Fleet nodes (edge / ESP32)" table (5 cols: node+ip, kind, sensor, signal/batt, seen) | `lib.rs:614-643` |
| Empty-state text: "none checked in. Edge nodes POST /fleet/heartbeat to appear here (COG-010); firmware is the next step." | `lib.rs:617` |
| Types: `NetPeer` (name, ip, os, online, self), `Tailscale`, `FleetNode` (id, kind, sensor, ip, rssi, battery, fw, age_s, online), `Net` | `crates/weftos-cog-manager/src/client.rs:85-143` |
| Poll: `GET {host}/network` every 4 s, single in-flight, stale guard | `client.rs:421-439` |
| Note: `FleetNode.fw` is parsed (`client.rs:123`) but never drawn | `lib.rs:626-641` |

Host side (what `/network` actually returns):

- `GET /network` = `network::snapshot()` plus `fleet` roster: `crates/weftos-cog-host/src/http.rs:211-215`. Unauthenticated, CORS `*` (`http.rs:84-87`, test at `:456`).
- `network::snapshot()` = `{node: hostname, tailscale, cognitum_mesh}` (`crates/weftos-cog-host/src/network.rs:11-17`). Tailscale = `tailscale status --json` reduced to name/ip/os/online/self (`network.rs:29-59`); Cognitum mesh = raw `GET 127.0.0.1:80/api/v1/peers` (`network.rs:61-67`), i.e. only meaningful on a Seed.
- Fleet roster is soft state, 256-node cap, online if seen < 60 s, dropped after 1 h, **display-only by design** (`fleet.rs:1-8,15-22`). Heartbeats are unauthenticated (`http.rs:216-222`, auth exemption `auth.rs:5-7`).
- The roster is a rebuild of the cog-host's own memory: it is empty after the host restarts. A separate static inventory exists, `crates/weftos-cog-host/fleet/nodes.json` (id, chip, mac, flash_mb, role, firmware, status, enrolled, seen), written by `scripts/enroll-node.py`; nothing reads it into the console [V: grep finds only the script and the file].

Honest assessment: the tab answers "who is on my tailnet and did some ESP32 phone home". It cannot answer what a node is, how busy it is, where it is, or let you do anything to it.

## 2. Data inventory by device class

Source key: **CH** = weft-cog-host HTTP (the only thing the console reaches today); **D** = weaver daemon RPC (Unix socket / `clawft-rpc`); **MS** = machine mesh service local protocol (`clawft-mesh-local`); **PEER** = the device's own API.

RPC permission classes come from `crates/clawft-weave/src/capability.rs:62-110,195-220`: unlisted methods are Read; `workload.*` unlisted is Write; the Admin list is explicit.

### 2.1 Fields every class can have (cross-cutting)

| Field | Source and exact location |
|---|---|
| Node identity (id) | D `cluster.nodes` -> `ClusterNodeInfo {node_id,name,platform,state,address,last_seen}` `crates/clawft-weave/src/daemon.rs:6298-6322`. Node id = Ed25519-derived (`node_id_from_pubkey`, ADR-025: `docs/adr/adr-025-ed25519-node-identity.md`) |
| Name, platform (`cloud-native`/`edge`/`browser`/`wasi`/`custom`), state (`joining/active/suspect/unreachable/leaving/left/unverified`), address | same; types `crates/clawft-kernel/src/cluster.rs:29-93,99-127` |
| Last seen | **Gap in the RPC**: `last_seen` is hard-coded `String::new()` at `daemon.rs:6318`, though `PeerNode.last_heartbeat` exists (`cluster.rs:119`) |
| Health summary | D `cluster.health` -> `{node_id, healthy, state}` `daemon.rs:6373-6388`; D `cluster.status` (counts only, `total_shards: 0`) `daemon.rs:6283-6297` |
| Signed node facts (probe results) | D `cluster.facts` (params: `node_id`, `refresh`) -> `FactsEntry {node_id, local, trust_tier, tier_source, received_at, expires_at, delta_seq, facts, signed}` `crates/clawft-weave/src/node_facts_rpc.rs:427-450,467-527`. Facts type `NodeFacts {version,node_id,issued_at,ttl_secs,seq,capabilities[],notes[]}` `crates/clawft-types/src/placement/node_facts.rs:136-151` |
| Trust tier (receiver-assigned, never self-asserted) | `TrustTier` = `discovered` / `paired` / `pinned` `node_facts.rs:71-90`; `tier_source` in `FactsEntry`. Note: mesh-"paired" means "node id verified at admission", not operator-paired (comment `node_facts_rpc.rs:434-437`) |
| Load summary | `NodeLoad {busy, total, mem_free}` `node_facts.rs:124-131`. Only `mem_free` is refreshed live; "no probe sets a capability's busy/free state" (`node_facts_rpc.rs` header comment) |
| Admission class of the live connection | `PeerClass` = `Node` / `Leaf` / `Legacy` (no verifiable hello, "old build, ESP32 firmware") `crates/clawft-kernel/src/mesh_admit.rs:317-324`; held in `PeerConnection.class`, `.verified`, `.connected_at` `crates/clawft-kernel/src/mesh_runtime.rs:80-100`. Exposed only as `peer_verified` / `peer_licensed` booleans (`mesh_runtime.rs:929-941`) and `peer_ids()` (`:943`); **no RPC returns class or connected_at per peer** |
| Mesh peer list (service mode) | MS `peers.list` (needs registered daemon or admin) `crates/clawft-mesh-local/src/proto.rs:407-409`, handler `crates/clawft-mesh-service/src/local_server.rs:398-406`; `peers` in the status JSON is **a list of node-id strings only** `crates/clawft-mesh-service/src/state.rs:302`. Count also on loopback `GET /health` `crates/clawft-mesh-service/src/health.rs:31` |
| Mesh service identity / policy / journal | MS `status` -> `status_json` `state.rs:250-316`: node_id, machine_pubkey, proto range, build_sha, started_at, listen, admission, bind_policy, cluster_owner_uid, registrations (admin), journal head, router counters. CLI `weaver mesh status` renders it `crates/clawft-weave/src/commands/mesh_cmd.rs:341-397` |
| Signed machine facts (service mode) | MS `facts.get` `proto.rs:410`; built in `crates/clawft-mesh-service/src/facts.rs` |
| Revocation state | D `mesh.revoked` (read) `daemon.rs:8593`; `mesh.revoke` / `mesh.unrevoke` `daemon.rs:8540,8567`; MS `peer.revoke` / `peer.unrevoke` admin `proto.rs` (`PeerRevoke`, `PeerUnrevoke`), gated admin in `crates/clawft-mesh-service/src/admin.rs:32,197` |
| Tailnet presence | CH `/network` `tailscale.peers` (name, ip, os, online) `network.rs:29-59`. Tailscale JSON also has last-seen and relay info that `peer_row` drops (`network.rs:51-59` keeps 5 fields) |

### 2.2 Servers and workstations (Mac dev box, photo-gallery x86-64)

| Field | Source |
|---|---|
| Hardware class and board | `cpu.arch.<arch>` capability with attrs `cores`, `model` (board from `/proc/device-tree/model` else cpuinfo) `crates/clawft-kernel/src/node_facts/linux.rs:95-115`; Mac: `crates/clawft-kernel/src/node_facts/macos.rs:56-66` |
| OS / kernel / distro | `os.linux` attrs `distro`, `kernel` (`linux.rs:117-130`); `os.macos` (`macos.rs:66`) |
| Memory | `mem.system` / `mem.unified` with total; live `mem_free` (`linux.rs:134`, `macos.rs:77-92`, refresh `probe.rs:334`) |
| Storage | `store.tier.internal` / `store.tier.external` (`linux.rs:160,180`; `macos.rs:110,133`) |
| Accelerators | `accel.gpu.metal`, `accel.npu.ane` (`macos.rs:147-214`); nvidia, hailo, coral in `node_facts/accel.rs` |
| Runtimes | native, Docker/OrbStack, Podman, Apple container, llama-server, mlx_lm, ollama (`node_facts/runtimes.rs`) |
| Node class | `node.class.<class>` (`linux.rs:152`), `node.class.dev-mac` claimed (`macos.rs:103`) |
| Measured perf | `perf.*` from `perf.measured.json` (`node_facts/measured.rs`, loaded by `node_facts_rpc.rs`) |
| Cogs and workloads placed here | D `workload.status` (no params) -> `{controller, targets, instances[], unsettled, workload_host, served_on}`; each instance row `{placement, status, lifecycle}` `crates/clawft-weave/src/workload_place_rpc.rs:726-737`. `PlacementRecord {instance_id,node_id,kind,workload,variant,decision_id,manifest_hash,project_id,...}` `crates/clawft-kernel/src/workload_ctl/plane.rs:142-170`; `TargetInfo {node_id,addr,tier,public_key,reachable,learned_ms}` `plane.rs:123-138` |
| Per-instance health | host list rows: `lifecycle`, `restarts` (count), `ingest`, `lease_stopped` `crates/clawft-kernel/src/workload_ctl/host_instances.rs:65-70`; lifecycle states Requested..Running, Unhealthy... `workload_ctl/lifecycle.rs:15-29` |
| Workload host advertisement | `ServiceAdvertisement` with routes, addr, ingest state `workload_ctl/host_service.rs:336-357` |
| Local inference | D `infer.status` (Read) -> roles, memory budget/used/resident, qualifying mesh peers, roster skipped `crates/clawft-weave/src/infer_rpc.rs:123-143`; adverts via the infer hub `crates/clawft-kernel/src/infer_proxy/hub.rs` |
| Daemon health | D `kernel.status` (state, uptime, process/service counts, build sha/version, handshake, shared services) `daemon.rs:5886-5914`; `kernel.ps` (daemon RSS substituted for root) `daemon.rs:5915-`; `kernel.services` `daemon.rs:6036` |
| Topology role (ADR-103: machine mesh service / user daemon / project kernel) | `docs/adr/adr-103-weave-topology-roles-and-instances.md:24-29`; project status D `project.status` `crates/clawft-weave/src/project_lifecycle_rpc.rs:116`; handshake `crates/clawft-weave/src/handshake_rpc.rs` |
| Licence | D `workload.node.binding` (Read: mesh id, held binding, orphaned) `crates/clawft-weave/src/licence_rpc.rs:115`; D `workload.cog.checkout.status` / `.list` (Read) `licence_checkout_rpc.rs:134-135`; CLI `weaver workload node status` is exactly this (mesh id + binding), NOT a fleet view `crates/clawft-weave/src/commands/workload_node_cmd.rs:44-50` |
| Chain / audit | D `chain.status`, `chain.tail`, `chain.verify` `daemon.rs:6393-6559` |
| Physical location | **not found** for any class (no site/room/lat-lon field in facts, `PeerNode.labels` exists `cluster.rs:126-127` but nothing populates it that I found) |

### 2.3 Pi / Seed appliances (cog0 Pi Zero 2 W, Pi 5)

Reachable through CH on the appliance itself (the console target, e.g. cog0 `http://100.109.125.18:9480`, per memory) and through the Seed's own API.

| Field | Source |
|---|---|
| Host's cog list: id, version, source, enabled, running, pid, restarts, rss_kb, uptime_s, last_exit, signed, licence_refusal, licence_grant | CH `GET /status` / `/cogs` `crates/weftos-cog-host/src/http.rs:223-232`, `CogStatus` `crates/weftos-cog-host/src/supervise.rs:143-161` (console mirror `client.rs:49-83`; rss/uptime computed `supervise.rs:411-412`) |
| Cog start / stop / install / reload | CH `POST /cogs/<id>/start|stop`, `/install`, `/reload` `http.rs:233-247` (POSTs need token + JSON content type, `auth.rs`) |
| Host-local licence status | CH `/licence` (token-gated) `http.rs:186-190`; `HostLicence::status` `licence.rs:304` |
| USB devices attached to the appliance | CH `GET /hw/usb` (token), `POST /hw/usb/scan|baseline|identify` `crates/weftos-cog-host/src/hw_http.rs:58-64`; console `hw_identify.rs`. This identifies USB parts on the host, it is not a "blink this node" action |
| Cognitum overlay peers | CH `/network` `cognitum_mesh` (raw agent `GET :80/api/v1/peers`) `network.rs:61-67` |
| Seed device state | PEER `GET /api/v1/status` (includes `writes_gated`), `/api/v1/identity` (device_id, public_key, firmware_version), `/api/v1/apps`, `/api/v1/apps/<id>/config`, `/api/v1/witness/chain`, `/api/v1/upgrade/check` `crates/clawft-kernel/src/workload_runtime/seed_ops.rs:165-345`. Other Seed read APIs exist as MCP tools (`seed_device_status`, `seed_thermal_state`, `seed_firmware_status`, `seed_swarm_peers_detail`, `seed_cogs_list`, `seed_wifi_status`, `seed_framework_mesh_status`); I could not call them: the Seed was down on the USB link (`seed unreachable at http://169.254.42.1/mcp: Host is down`) **[U: response shapes not verified]** |
| Binding to the mesh, adapter-attested facts (always `claimed` provenance) | `docs/research/mesh-placement/seed-adapter-operations.md` section 5; `seed_bind.rs`; D `workload.node.bind` (Admin) |
| Cloud-side inventory (id, firmware, online) | `ReadOnlyFleet`, only `fleet_status` callable `crates/clawft-kernel/src/workload_runtime/fleet_inventory.rs:31,142-153`; fixture is an **assumed** shape, not a recording (`seed-adapter-operations.md` section 6). The `cognitum` MCP `fleet_status` and `cognitum-fleet` MCP both failed to connect in this session (`invalid_target`), so the real shape is **[U]**. Board card 9b1224e8 ("Real Cognitum fleet_status fixture") was not readable (board token needed); presumably the owner capture described in that doc |
| Cert-pin state | `SeedTls::PinnedSpki` etc. (`seed-adapter-operations.md` section 1) |
| Hardware inventory of SBCs | `docs/research/mesh-placement/sbc-inventory-2026-10.md` (research doc, not wired to the UI) |

### 2.4 ESP32 sensing nodes (S3 / C6)

Two distinct populations:

1. **Heartbeat roster nodes** (COG-010): `POST /fleet/heartbeat {id, kind, sensor, rssi, battery, fw, ip}` to a cog-host. Stored and shown as `{id, kind, sensor, ip, rssi, battery, fw, age_s, online}` `fleet.rs:29-44,99-128`. `rssi` is bounded -127..20, `battery` 0..100 (`fleet.rs:86-91`; the UI formats battery as volts `lib.rs:637`, a unit mismatch to resolve). No CPU, memory, uptime, heap, channel, sample rate, reset reason.
2. **RuView CSI nodes** (`/Users/mathewbeane/dev/ruview/RuView/firmware/esp32-csi-node`): their own HTTP server on :8032. `GET /ota/status` returns `{version, date, time, running_partition, next_partition, max_size}` (`main/ota_update.c:83-102`). `POST /ota` uploads firmware, PSK bearer auth (ADR-050, `ota_update.c:107-118`). Also `/wasm/upload|list|start|stop` on the same server (`main/wasm_upload.c:372-406`). Re-provisioning is USB-serial only (NVS), per `/Users/mathewbeane/dev/ruview/RuView/docs/handoff.md` ("Dead ends"). A reachable node was seen at 192.168.1.83 on 2026-09-28 per that handoff [U: not rechecked]. The RuView firmware also has thermal, power-management, swarm bridge (`swarm_bridge.c`), `node_log.c` (a log ring; exposure over HTTP not found) and C6 time sync / ESP-NOW (`c6_sync_espnow.c`). None of these feed the WeftOS roster today; `crates/weftos-cog-host/fleet/nodes.json` is the enrolment inventory (chip, mac, flash_mb, role, firmware, status) for owned C6 boards.

Other ESP32-class WeftOS firmware that joins the mesh as a **leaf** (admission class `Leaf` or `Legacy`): `crates/clawft-edge-pad` (no_std), `crates/clawft-edge-pad-idf` (std). The leaf announce is `LeafServices {node_pubkey, hostname, firmware_version, audio_sink, display_sink, compute}` published on `mesh.leaf.<pubkey>.announce` (`crates/weftos-leaf-types/src/lib.rs:212-219`; topics in `docs/leaf-push-protocol.md` section 2). Nothing in the daemon aggregates announces into an RPC [V: not found; `cluster.nodes` only lists `ClusterMembership` peers]. Leaf control today is push-only via `weaver leaf push|scene` (`crates/clawft-weave/src/commands/leaf_cmd.rs`).

### 2.5 MCU leaves (STM32N6) and smaller

`crates/weftos-n6-leaf/src/main.rs` and `crates/weftos-n6-node/src/{main,ptp}.rs` exist; I did not find any status/heartbeat/announce for them beyond the same `LeafServices` shape, and no card "444c7964" text in the repo (board, not readable). Smaller parts (bare sensors, I2C/UART modules) are not nodes; they are the Catalog tab's hardware items (the in-progress sensor-detail view covers those) plus, for a Seed or Pi, USB parts via `/hw/usb`.

For both MCU and sub-MCU classes the honest data set is: identity (pubkey), firmware_version, advertised sinks, last announce time (not recorded anywhere), and transport RSSI if a gateway proxies it. Everything else must be reported by a gateway node on its behalf and flagged `claimed`/`proxied`.

### 2.6 Mesh-wide / cross-cutting sources worth wiring

| Source | Why |
|---|---|
| D `cluster.facts` + `workload.status` + `infer.status` + `kernel.status` + `mesh.revoked` + `workload.node.binding` + `workload.cog.checkout.list` | The complete "what a daemon already knows about the fleet" set, all Read (default or listed Read) |
| Cog swarm: `crates/clawft-weave/src/cog_swarm.rs` (ADR-106 cog mesh, artifact tunnel, `mesh.cog.*` topics) | Who has which artifact; only wiring/state, no RPC found for listing it |
| Licence: `PeerDirectory` / `PeerSnapshot {connected, licensed, reserved_holder, reserved_holder_uid}` `crates/clawft-kernel/src/licence/service_links.rs:64-82` | Which connected peers are licensed (class `node`) |
| CLI views that already exist | `weaver mesh status|bindings`, `weaver workload list|status`, `weaver workload node status`, `weaver doctor` (`commands/doctor_cmd.rs`, `mesh_doctor.rs`, `licence_doctor.rs`) |

## 3. Gaps and the smallest fill (no new trust paths)

Constraint recap: ADR-103 splits roles; the machine mesh service owns the mesh listener, admission and revocation, and the user daemon owns tokens and placement. The console must not talk a mesh protocol, mint tokens, or reach a device with credentials the operator did not give it. Admin RPCs need an authenticated token with admin scope (`capability.rs`). The existing HTTP surfaces (cog-host, gateway) are the transports the console already uses.

| # | Gap | Smallest fill |
|---|---|---|
| G1 | **No bridge from console to the daemon** (CH has no daemon client) | Add ONE read-only daemon RPC `fleet.snapshot` (Capability::Read) that composes `cluster.nodes` + `cluster.facts` + `workload.status` + `infer.status` + `workload.node.binding` + the revoked list into one JSON document. Expose it via the gateway/MCP path that ADR-102 already puts behind daemon-issued tokens (`docs/adr/adr-102-gateway-health-and-api-playground.md` D1/D3), or via a read-only `GET /fleet` proxy in cog-host that calls the local daemon socket. No new credentials: the cog-host and daemon are on the same box, the socket is already the trust boundary |
| G2 | `last_seen` always empty in `cluster.nodes` | Fill from `PeerNode.last_heartbeat` at `daemon.rs:6318` (one-line change) |
| G3 | Admission class, connected_at, verified, per-peer RTT not exposed | Add `peer_detail()` on `MeshRuntime` returning `{node_id, class, verified, connected_at, rtt_ms, error_rate}` from `PeerConnection` (`mesh_runtime.rs:80`) and `PeerMetrics` (`mesh_heartbeat.rs:392`), surface it in `fleet.snapshot`. In service mode the service's `peers.list` returns ids only (`state.rs:302`); extend it (admin or registered-daemon only, as today) rather than giving the console a path to the service |
| G4 | No CPU load / queue depth / sample rate on any node | Add a cheap live probe beside `mem_free` (`node_facts_rpc.rs` live refresh): load average (Linux `/proc/loadavg`, macOS `sysctl vm.loadavg`), cpu count already known. Queue depth = placed instance counts from `workload.status`. Sample rate only exists per cog (`CogOutput` in sensor-detail `sensor_link.rs:230-262`), so report it from the cog's own `/status` as the sensor view already does |
| G5 | Location | Add an operator-set `labels` map (site, room, rack, coordinates) stored in the daemon (`PeerNode.labels` already exists, `cluster.rs:126`) and in `nodes.json` for edge boards; show as "claimed by operator". Mesh location = address + route (tailnet / LAN / overlay / USB) which we already know. Do not infer physical location from IP |
| G6 | Edge heartbeat too thin; roster lost on restart; unauthenticated | Extend `Heartbeat` with optional `uptime_s`, `free_heap`, `reset_reason`, `chip`, `mac`, `channel`, `sample_hz`; persist roster to disk; merge with `fleet/nodes.json` so reserved/enrolled-but-silent boards show as "never seen". Keep it display-only and unauthenticated as designed (`fleet.rs:5-8`); mark every roster field `self-reported`. Show a per-node warning that nothing here is trusted for placement |
| G7 | Leaf announces not aggregated | Daemon subscribes to `mesh.leaf.*.announce` (already routed per `leaf-push-protocol.md` s2) and keeps `{pubkey, hostname, firmware_version, sinks, last_announce}` in a bounded table; include in `fleet.snapshot` with `class: leaf` and tier from `PeerClass` |
| G8 | Seed live state only reachable by the controller, not the console | Include in `fleet.snapshot` from the placement adapter's existing reads (`seed_ops.rs` status/identity/apps); never hand the Seed token to the console. Cloud `fleet_status` stays read-only through `ReadOnlyFleet` and is `claimed` |
| G9 | No node log access | Instances: `workload.logs` exists (Write class by default, `workload_place_rpc.rs:739`, `capability.rs:219`) and cog logs on a Seed via its API. Host: expose a bounded tail of the cog-host's own supervisor log. Nodes: tail of `kernel.logs` `daemon.rs:6161`. ESP32: only if firmware exposes it (RuView `node_log.c` is not served over HTTP that I found); otherwise "not available" |
| G10 | `FleetNode.fw` parsed but not shown; battery unit | Show firmware; decide volts vs percent (host clamps 0..100, UI prints "V") |
| G11 | Tailscale row drops useful fields (last seen, relay/direct, exit node, tags) | Extend `peer_row` in `network.rs:51-59` with `LastSeen`, `CurAddr`/`Relay`, `Tags`; zero trust impact |
| G12 | Cognitum overlay peers printed as raw JSON | Typed rows (`/api/v1/peers` shape is on the Seed; verify shape live, **[U]**) |
| G13 | Class is not computed anywhere | Derive `class` in the aggregator: `server/workstation` (cluster platform cloud-native or `node.class.*`), `appliance` (Seed: has `/api/v1/identity`, or board `Raspberry Pi`), `esp32` (roster or Legacy/Leaf with chip esp32*), `mcu` (leaf with no OS), `browser` (platform browser). Label the evidence |

Governance notes: `fleet.snapshot` returns only what the caller's capability already allows (all Read-class sources). Facts remain signed; the UI shows `provenance` (`probed` / `measured` / `claimed`) on every capability (`capability.rs:219`-`258` area, `Provenance` enum) so Seed `claimed` facts are visibly weaker than a node's own probes.

## 4. Interaction tools per class

"Exists" = an RPC/endpoint is in the tree today. "Perm" = what the daemon requires (`capability.rs`).

| Tool | Servers / workstations | Pi / Seed | ESP32 | MCU / leaf | Exists? | Perm / governance |
|---|---|---|---|---|---|---|
| Details (all facts, raw JSON) | yes | yes | roster fields + `/ota/status` | announce fields | Partly: `cluster.facts` (Read), CH `/status`, PEER status | Read |
| Workloads / cogs list | yes | yes (CH `/status`) | n/a (RuView `/wasm/list`) | n/a | `workload.status`, CH `/status`, `workload.list` | Read |
| Logs | daemon `kernel.logs`; instance `workload.logs` | per-cog via Seed API; host log | firmware dependent, not found | not found | Partly | `workload.logs` is Write (unlisted `workload.*`), `kernel.logs` Read |
| Restart cog / start / stop | yes (placed instances: `workload.stop` etc.) | CH `POST /cogs/<id>/start|stop` | RuView `/wasm/start|stop` | no | Yes | CH POST needs token + JSON (`auth.rs`); `workload.*` Write and gated + chained on both nodes (`workload_place_cmd.rs` help text); licence run gate can refuse start |
| Install / unload cog | via `workload.place` / `workload.unload` | CH `/install` (signed-only) | RuView `/wasm/upload` | no | Yes | Write, governed placement; signed packages only (COG-008) |
| OTA status | n/a | `GET /api/v1/upgrade/check` (`seed_ops.rs:339`) | `GET :8032/ota/status` | not found | Yes (read) | Read; PEER-side none for ESP32 status |
| OTA update | `weaver update` (`commands/update_cmd.rs`) on that node | Seed firmware upgrade API | `POST :8032/ota`, PSK bearer | not found | Yes, per device | **High risk.** Needs operator secret held outside the console (PSK / Seed token); do not store in the console. Phase late, behind an explicit confirm and a chained audit entry |
| Identify (blink) | no | no | **not found** in RuView or edge-pad | **not found** | **No** | Would need new firmware verb. Do not call `hw_identify` "identify": that is USB part identification |
| Drain / cordon | no verb found; closest is `infer.expose false` and `workload.stop` | no | no | no | **No** | New verb; must be Admin and chained. Placement already has `migratable`/pin and a `--avoid` hint (`workload_place_cmd.rs:106-115`), so a cordon = "avoid" set on the controller, no new trust path |
| Re-admit / revoke peer | `weaver mesh peer revoke|unrevoke` -> MS `peer.revoke` / `peer.unrevoke` (admin); D `mesh.revoke|unrevoke` | same | same (node id) | same | Yes | **Admin**; chained; force-revoke persists when the journal is read-only (`admin.rs:9`). Never expose from a read-only console |
| Revoke package / signer | `workload.revoke` | same | - | - | Yes | Admin, incident verb (`workload_place_rpc.rs:~780`) |
| Bind / unbind Seed | `workload.node.bind|unbind` | yes | - | - | Yes | Admin and operator-signed on the CLI; the daemon never sees the operator key (`workload_node_cmd.rs:1-8`). Console must not do this; show the CLI command |
| Re-probe facts | `cluster.facts {refresh:true}` | adapter | - | - | Yes | Read RPC but rate-floored and chained (`node_facts_rpc.rs:483-505`) |
| Open guide | sensor cog guide at `/guide` (ADR-104) | yes | - | - | Yes (Sensors tab) | none |
| SSH hint | show `ssh <user>@<ip>` using tailnet/LAN address | yes (Pi5 `genesis@`, per memory) | no | no | Display only | none; never run ssh from the console |
| Copy node id / pubkey / address | all | all | all | all | n/a | none |

Rule of thumb for the UI: Read-class actions are buttons; Write-class show a confirm; Admin-class are not buttons at all in phase 1, they show the exact `weaver ...` command to copy (the admin key and the daemon token stay on the operator's CLI).

## 5. Proposed UI

### 5.1 Fleet list (the Network tab)

Group by class, one row per node, key columns: status dot, name, class, address and route, OS/firmware, load, cogs, trust, seen. Sort: self, online, then name. Top bar: counts per class, filter box, class filter chips, "Source: daemon snapshot | host only" indicator. Rows expand into a detail page (`style::card_focus` already supports deep-link open and scroll).

```
 Network  ·  fleet manager                        source: daemon (cog0)   [Refresh] [filter: ______] [all|servers|appliances|esp32|mcu]
 ─ 9 nodes · 7 online · 1 degraded · 1 unverified ───────────────────────────────────────────────────────────────────────────
 SERVERS & WORKSTATIONS (2)
  ● mathews-mac     self   macOS 15 · arm64 · 12c · 32 GB (free 11)   LAN 192.168.1.249  load 1.9   3 cogs  pinned    now
  ● photo-gallery          Ubuntu 24.04 · x86-64 · 8c · 16 GB (free 6) tailnet 100.x       load 0.4   0 cogs  paired    4s
 APPLIANCES (2)
  ● cog0            Pi Zero 2 W · fw 0.10.11 · 512 MB (free 90)       tailnet 100.109.125.18 temp 58C 5 cogs  claimed*  6s   [host :9480]
  ○ pi5             Pi 5 · aarch64 · 4c · 8 GB                          tailnet 100.x          -     -        paired    3d
 ESP32 SENSING (3)
  ● c6-01  esp32-c6 · fw v0.4.3.1 · ota_0   192.168.1.83  rssi -61  batt 92%   wifi-csi        self-reported   12s
  ◐ s3-02  esp32-s3 · fw v0.4.3.1           192.168.1.91  rssi -78  -          wifi-csi        self-reported   71s (stale)
  ○ c6-03  reserved slot (never seen)                                                           enrolled 2026-10-02
 MCU LEAVES (1)
  ● n6-leaf-01  stm32n6 · fw 0.1 · sinks: display            via mathews-mac      leaf           verified 2s
 * claimed = adapter-attested facts, not probed by the node.            [ ● online  ◐ stale  ○ offline/never ]
```

### 5.2 Node detail page

Tabs: Overview, Workloads/Cogs, Health/Stats, Network, Trust/Licence, Firmware, Logs, Raw ("ALL DETAILS" JSON). Each class hides tabs it has no data for and says why ("ESP32 roster nodes report no workloads").

```
 < Fleet   ● cog0   appliance · Pi Zero 2 W        node 5e1c…a9f0     [Copy id] [ssh hint] [Open guide] [Restart cog ▾]
 [Overview][Workloads/Cogs][Health/Stats][Network][Trust/Licence][Firmware][Logs][Raw]
 ─ Overview ───────────────────────────────────────────────────────────────
  What      Cognitum Seed (Pi Zero 2 W) running weft-cog-host      Class evidence: /api/v1/identity, board model
  Where     tailnet 100.109.125.18 · LAN 192.168.1.235 · USB 169.254.42.1     site/room: (not set)  [edit label]
  Busy      load 0.62 · mem 90/512 MB · 5 cogs running · queue 0 · restarts 2       temp 58C (seed thermal)
  Trust     tier: claimed (Seed binding) · admission: n/a (not a mesh peer) · licence: bound, 1 grant
  Seen      6 s ago · up 3d 4h
 ─ Workloads/Cogs  (same CogView/actions as sensor detail) ─────────────────
  bridge       0.4.1 weavelogic signed   ● running  pid 812  rss 4.1 MB  up 3h  restarts 0   [Stop] [Logs]
  fall-detect  1.2.0 cognitum            ○ stopped  last_exit: signal 9   licence_refusal: no_grant
 ─ Raw ───────────────────────────────────────────────────────────────────
  { "snapshot": {...}, "facts": {...signed envelope...}, "placement": {...}, "host_status": {...}, "peer_detail": {...} }
  provenance per field · [Copy JSON] [Verify facts signature]
```

Tab to data mapping:
- Overview: facts capabilities (`cpu.arch.*`, `os.*`, `mem.*`, `node.class.*`), cluster node row, labels.
- Workloads/Cogs: `workload.status` instances for this node plus CH `/status` for a cog-host. Reuse the sensor-detail `CogView`, `cog_state`, `actions` and `stat_rows` (worktree `.claude/worktrees/sensor-detail/crates/weftos-cog-manager/src/sensor_link.rs:40-110,176-216,274-312`).
- Health/Stats: `NodeLoad`, `mem_free`, load, temp (Seed thermal, via snapshot), instance restarts, `kernel.status` uptime, sparkline history kept in the console (ring buffer, per node).
- Network: address list and route type, admission class, verified, RTT and error rate, connected_at, tailnet row, cognitum overlay peer detail, infer adverts.
- Trust/Licence: trust tier and `tier_source`, node pubkey fingerprint, revocation state, `workload.node.binding`, checkout grants, run gate refusals. Admin actions shown as copyable `weaver` commands.
- Firmware: facts version, `/ota/status` (ESP32), `upgrade/check` (Seed), roster `fw`, partition info; OTA button late and gated.
- Logs: bounded tail through the aggregator; "not available for this class" otherwise.
- Raw: the full merged JSON with per-source provenance and a "Verify signature" action on the signed facts envelope (the console can re-verify with the public key in `signed`; the type is built for that, `node_facts_rpc.rs:448-449`).

### 5.3 Aggregation, not polling devices

- One poll of `fleet.snapshot` (4 s, same cadence and in-flight guard as `poll_network`, `client.rs:421-439`); the daemon does fan-out, caching and signature checks. The UI never polls ESP32 `:8032` or Seeds directly.
- Per-node detail (OTA status, thermal, `/status` of a cog) is fetched on demand when a detail tab is open, through the aggregator (for devices the daemon already has credentials for) or, for read-only unauthenticated device endpoints on the LAN, directly as the sensor view already does for cog exports (`client.rs ensure_cog_output`, sensor-detail worktree). No device credential is ever stored in the console.
- The snapshot carries `source`, `fetched_at`, `ttl`, and a `degraded[]` list (for example "mesh service link down") so the UI can say which part is stale.

### 5.4 Offline / no-daemon fallback

Layered, same page, with a banner that names the active layer:
1. **Daemon snapshot** (full).
2. **Host only**: `/network` + `/status` (today's data, now with typed rows, `fw` shown, tailnet extras, roster merged with `nodes.json`). Banner: "No daemon on this host: showing tailnet and check-ins only. Trust, placement and licence need the daemon."
3. **Cached last snapshot** (read-only, greyed, with age) if the host is unreachable; persist in browser storage / native config dir.
4. **Unreachable**: show the host address, last good time, and the ssh hint; never an empty page.

## 6. Phased build plan

Overlap column names components shared with the in-progress sensor detail view (worktree `.claude/worktrees/sensor-detail`, uncommitted at the time of writing: `sensor_detail.rs`, `sensor_link.rs`, `style::card_focus`, `CogOutFetch` polling in `client.rs`).

| Phase | Scope | Acceptance criteria | Shared with sensor detail |
|---|---|---|---|
| **P0 tidy (host only, console only)** | Typed overlay-peer rows; show `fw`; fix battery unit; tailnet extras (last seen, relay, tags); merge `fleet/nodes.json` so reserved/enrolled boards appear; rows expand; "Raw JSON" per row | Network tab shows every field `/network` returns, no `{}` debug text; reserved C6 slots listed as "never seen"; unit tests on parsers with the real fixture shape; wasm + native build | `style::card_focus`, `pill`, `truncated`, time formatter `fmt_dur` (`sensor_link.rs:262`) |
| **P1 daemon bridge, read-only** | `fleet.snapshot` RPC (Read) composing `cluster.nodes/facts`, `workload.status`, `infer.status`, binding, revoked; fill `last_seen` (G2); expose via the gateway token path or a read-only cog-host proxy; console `Fleet` client + fallback layers 1-4 | With a daemon: list shows class, OS, cores, memory, trust tier, cogs count, last seen for this node and any peers; without: banner and host-only data; no new write path, no credential in the console; RPC covered by tests including an unsigned/expired-facts case; works against `weaver` daemon on Mac and the Pi | `Client` poll/in-flight pattern; `CogView` for the cogs column |
| **P2 node detail page** | Tabs Overview, Workloads/Cogs, Health/Stats, Raw; Start/Stop via existing CH POSTs with the same token and `Event` pattern as sensor detail; copy id/ssh hint; "Verify facts signature" | Clicking a node opens the detail; Raw shows merged JSON with per-field provenance; start/stop work on a cog-host and refuse with the licence code when gated; no Admin verb offered | `PanelCtx`/`Event` pattern, `actions()`, `stat_rows`, `cog_state`, `software_line` (`sensor_link.rs`), `ensure_cog_output` |
| **P3 telemetry** | Load average probe; `peer_detail()` (class, connected_at, RTT, error rate); history sparklines; edge heartbeat v2 fields + persisted roster; leaf announce table; Seed thermal/identity through the adapter | Each class shows at least identity, firmware, load or "not reported by this class", last seen; RTT shown for connected mesh peers; stale nodes visibly aged; leaf nodes listed with `class: leaf` and sinks | `stat_rows`, spark/stat widgets |
| **P4 trust, licence, firmware** | Trust/Licence tab (tier, source, revocation, binding, grants, refusals) with copyable admin commands; Firmware tab (`/ota/status`, Seed `upgrade/check`); operator labels for location | Admin actions are never executed from the console; displayed commands match `weaver mesh peer revoke` / `workload node bind`; ESP32 firmware shows version and partition; labels persist and show as operator-claimed | Firmware read model `firmware_read`/`FwRead` (`sensor_link.rs:407`), docs links |
| **P5 gated actions** (only if wanted) | Logs tail; cordon as "avoid" on the controller; OTA update for ESP32/Seed with explicit confirm; identify (needs a new firmware verb) | Each action requires an authenticated daemon token with the right scope, is chained, and has a test that a Read token is refused; OTA never stores the PSK; drain proven not to bypass admission | Action button/confirm pattern |

Open questions for team-lead:
1. Transport for G1: gateway/MCP with ADR-102 tokens, or a read-only `/fleet` proxy in cog-host talking to the local daemon socket? The proxy is smaller; the gateway is closer to ADR-102 intent. On a Seed with no daemon only the host layer applies.
2. ESP32 heartbeat: allow the v2 fields and keep it unauthenticated display-only, or sign heartbeats with the node key (leaf provisioning is a topology phase, `adr-103` "Leaf" row)?
3. Is a physical location label wanted at node or site level? Nothing exists today.
4. Real `fleet_status` and Seed API shapes are unverified: both Cognitum MCP servers failed to connect (`invalid_target`) and the Seed was down on USB in this session. Owner capture is needed before typing the parser (`docs/research/mesh-placement/seed-adapter-operations.md` section 6).

## Decisions (user, 2026-10-04)

1. **Transport:** go through the ADR-102 gateway with its scoped tokens, not a cog-host proxy.
2. **ESP32 heartbeats:** add the richer v2 fields, keep them unauthenticated and display-only, and label them "self-reported" in the UI. Signing waits for leaf key provisioning.
3. **Physical location:** site + room labels per node, set by an operator command and recorded on the chain.
4. Real `fleet_status` / Seed API shapes remain unverified until an owner capture (open).
