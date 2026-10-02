# Seed adapter operations (cards 04de512f, d4fe33c5, 1ac2c0a4, ea705398, 2f540b88)

Date: 2026-10-02. Code: `crates/clawft-kernel/src/workload_runtime/` (`seed*.rs`, `fleet_*.rs`) and `workload_ctl/`. Nothing here was run against the real Seed, the Pi 5 or the Cognitum cloud; steps that need them are marked **owner-run**.

## 1. Certificate pinning and Seed certificate rotation (04de512f)

Since firmware 0.22.20 a Seed limits its leaf certificate to 825 days and renews it on its 6-hourly update loop, so a pin on the SHA-256 of the leaf certificate (`SeedTls::PinnedSha256`) can break with no operator action.

- New: `SeedTls::PinnedSpki`, the SHA-256 of the leaf's SubjectPublicKeyInfo, written `spki-sha256:<64 hex>`. A renewal that keeps the device key keeps the pin valid. Handshake signatures are still verified, so the peer must hold the pinned key.
- A renewal that **changes the key** is refused (the token is never sent) and the operator re-pins over a trusted path. That is a deliberate manual step; there is no silent re-trust and no automatic re-pin flow. The governed re-pin alternative was not built: it would need an authenticated channel that the key change has just invalidated.
- Record a pin: `openssl s_client -connect <seed>:8443 </dev/null 2>/dev/null | openssl x509 -pubkey -noout | openssl pkey -pubin -outform DER | openssl dgst -sha256` gives the digest to write after `spki-sha256:`. `SeedTls::spki_fingerprint(cert_der)` does the same in code. The certificate-hash pin remains for compatibility; prefer the key pin.
- Test: `a_renewed_leaf_certificate_under_the_same_key_still_connects` (same key, new serial: the old leaf-hash pin fails, the key pin connects) and `a_certificate_under_another_key_is_refused_by_the_key_pin`.

## 2. Plain-http USB transport, console pin, install ordering (d4fe33c5)

### 2a. "error sending request" over `http://169.254.42.1`

The adapter's client failed where `curl` worked. The cause was inferred from the client code and could not be confirmed against the real Seed (not touched). Two client behaviours differ from `curl` and are fixed in `HttpSeedTransport::new`:

1. **Proxies.** `reqwest` honors `HTTP_PROXY` / `ALL_PROXY` and, on macOS, the system proxy settings; `curl` honors the environment only. A proxy cannot route link-local `169.254.0.0/16` and would also see the bearer token. The client now uses `no_proxy()`: a Seed is always reached directly. Test `an_environment_proxy_is_not_used_for_a_seed` runs a child process with a dead proxy in the environment; it fails without the fix.
2. **Pooled keep-alive connections.** The Seed closes idle keep-alive connections after a few seconds (see the keep-alive quirk in the cogs repo notes); reusing the stale pooled connection fails with "error sending request". The client now keeps no idle pool (`pool_max_idle_per_host(0)`). Test `plain_http_survives_a_server_that_drops_keep_alive_connections` uses a stub that advertises keep-alive and then closes.

If the owner run below still fails, capture `curl -v` and `RUST_LOG=reqwest=trace,hyper=trace` output and attach it to the card; the remaining suspects are HTTP/1 header handling and the `Host` header.

**Owner-run (live USB, not run by the agent):** with the Seed on the USB link and fall-detect already installed,

```
COGNITUM_SEED_LIVE=1 COGNITUM_SEED_BASE=http://169.254.42.1 \
COGNITUM_SEED_TOKEN="$(cat <token file>)" \
cargo test -p clawft-kernel --features workload-runtime --lib workload_runtime::tests_live -- --nocapture
```

(`scripts/build.sh test` has no test-name filter; this is the raw cargo form for one live suite.) The run loads, starts, runs one console cycle, stops and unloads, and leaves every cog as it found it. Add `COGNITUM_SEED_LIVE_INSTALL=1` only to also cover install and uninstall. Record the outcome here.

### 2b. Version pin on the console path

`SeedApiRuntime::console` now reads the installed list and refuses unless the cog is installed at the pinned version, so a Seed upgraded since `load` never runs an unpinned console cycle. Test: `console_refuses_a_cog_whose_installed_version_is_not_the_pin`.

### 2c. Install ordering

Install takes only `{"id"}` and the Seed starts what it installs (the documented contract; the API has no install-without-start option), so "install without auto-start, verify, then start" is **not possible** and is declined for that reason. What is enforced instead:

- Before the install call, `admit` requires the store listing to show the pinned version (and sha256 if pinned); otherwise no install call is made (`a_store_version_other_than_the_pin_is_refused_before_any_install_call`).
- Immediately after the install, one read of the installed list decides: anything but the pinned version is stopped and uninstalled before any other step (`a_version_that_slips_in_at_install_time_is_stopped_and_removed_before_anything_else`; it is never started by us). Only a pinned version goes on to the load-time stop.
- Residual risk: for the one round trip between install and the read, a version the store served that differs from its own listing could be running. Closing that needs a Seed API change (pin the version in the install request); raise it upstream with the cogs project.

## 3. Native Linux ARM evidence (d4fe33c5 item d)

The owner reports that cogs run on ARM on the real Pi 5, and the 2026-09-29 run is already recorded in ADR-100 section 5: weaver v0.8.1 as a mesh member on the Pi 5 ran released aarch64 `anomaly-detect`, `fall-detect`, `baby-cry` and `sleep-apnea` ingest natively. That is the native-on-real-ARM evidence for card 09; it was not re-run for this card. ARM verification on hardware stays `scripts/build.sh test-pi` on the real Pi 5.

## 4. Controller restart, demoted peers, trust on first use (1ac2c0a4)

- **Stop and unload on a demoted peer.** The controller used to ask the gate for every state-changing verb with the target's current trust tier, so a demoted target (`discovered`) could not be stopped. `stop` and `unload` now never fail on the controller's gate: a denial is chained as `gate_denial_overridden_for_teardown` on the request event and the call proceeds. `start`, `place` and `load` on a demoted peer are still denied. Tests: `stop_and_unload_work_on_a_demoted_peer_but_start_does_not`, `stop_and_unload_work_on_a_demoted_seed_but_a_new_place_does_not`. Demoting a Seed (`set_tier`) now also updates the tier governance reads for new placements on it.
- **Seed placements survive a controller restart.** They are written to the state file (`workload-placements.json`) with the store pin they came from (no credential). After a restart the first verb on one calls `WorkloadHost::adopt` -> `SeedApiRuntime::adopt` (new `WorkloadRuntime::adopt`, default unsupported), which checks the Seed still holds the pinned cog, rebuilds the adapter's table and chains the load with `phase: readopted`. If the Seed no longer has the cog, `unload` forgets the record and the other verbs report the mismatch. Tests: `a_seed_placement_survives_a_controller_restart_and_stays_controllable`, `a_restarted_controller_refuses_a_seed_that_no_longer_has_the_pinned_cog`.
- **Trust on first use.** An `OperatorPeer` listed without a key trusts the first node key seen at its address (`plane_peers.rs`); a different key answering later is never learned under it. A listed peer with a key is pinned. **Decision (recommended, recorded here):** a `remote.api` node (a Seed, or anything reached over a routed or untrusted path) requires a pinned key or certificate pin; trust on first use is allowed only for peers on the operator's own LAN and only after an operator confirmation of the first-seen key (show the node id and key fingerprint, record the confirmation in the chain). Unpinned peers stay at the tier the operator listed and are never raised by discovery. Enforcement (refusing an unpinned `remote.api` peer in `apply_operator_peers`) is a follow-up; today this is policy and documentation.

## 5. Fleet identity binding and adapter-attested facts (ea705398)

`workload_runtime/seed_bind.rs`. A Seed has an Ed25519 device identity but no WeftOS node key, so:

- The Seed's WeftOS **node id** is derived from a per-Seed *adapter key* held by WeftOS (`seed_node_id`); the Seed cannot sign for itself.
- The operator signs `BindRecord {device_id, device_pubkey, node_id, bound_at}` (`sign_bind`). `SeedBinder::bind` checks, in order: signer is a pinned operator key; signature over the exact bytes; the record is for this adapter's node; `bound_at` is within the accepted window (24 h, 60 s skew); the Seed's `GET /api/v1/identity` returns the same device id and public key (so an untrusted record costs the Seed nothing); the record is not a replay and not older than the device's current binding; the node is not bound to another device.
- Success chains `workload.node.bind` (device id, device key, node id, bound_at, operator key, record hash). Every failure chains `workload.refuse` with `phase: node.bind` and a stable code (`bad_signature`, `untrusted_operator`, `identity_mismatch`, `replayed`, `expired`, `wrong_node`, `node_taken`, ...).
- `attest_seed_facts` signs the Seed's facts with the adapter key; every capability is forced to provenance `claimed`. `measured` only comes from WeftOS's own conformance runs.
- Tokens: the Seed bearer token is only used as a request header; it is in no record, error or chain event (`no_bearer_token_reaches_the_chain_or_an_error`).
- Limits: the replay memory is in-process; across a restart the 24 h window bounds an old record. The `device_pubkey` string is compared exactly as the Seed reports it. The key format the real firmware returns is not confirmed (**owner-run**: `GET /api/v1/identity` on the paired Seed, record the format here).
- **Owner-run (real Seed):** sign a record for the paired Seed and bind it; a chain export should show the bind and a tampered copy refused.

## 6. Cognitum cloud fleet inventory, read-only (2f540b88)

`workload_runtime/fleet_inventory.rs` and `fleet_mcp.rs`.

- Only the `fleet_status` MCP tool can be called. `ReadOnlyFleet` refuses any other tool name before it reaches the transport (`READ_TOOLS`); the test transport fails the test if a non-read tool is ever invoked. The adapter proposes candidates (`device_id`, firmware, online state, public key if reported); it never registers, places or configures anything through the cloud.
- `cross_check` compares the local pairing key (`GET /api/v1/identity`) with the cloud's key for the same device: equal is `Verified`, a different key is refused and chained (`key_mismatch`), an unlisted device is `NotListed`, a listing without a key is `KeyNotReported` (not verified).
- **Fixture status:** `fixtures/cognitum_fleet_status.json` is an **assumed** shape, not a recording: no call to the cloud was allowed. The parser accepts the key spellings the schema is expected to use (`device_id`/`deviceId`/`id`, `firmware`/`firmware_version`/`version`, `online` or `status`, `public_key`/`publicKey`) and skips an entry with no device id.
- **Owner capture of the real schema:** in a Claude session with the `cognitum` MCP server connected and signed in, call the `fleet_status` tool (read scope only), save the raw JSON output over `fixtures/cognitum_fleet_status.json` (remove the `_fixture` marker, keep one device you own, redact nothing the parser needs), and run `cargo test -p clawft-kernel --features workload-runtime --lib workload_runtime::tests_fleet_inventory`. A failing `the_fixture_parses_in_every_accepted_spelling` shows what to adjust. The cloud endpoint is `POST https://api.cognitum.one/v1/mcp` (OAuth 2.1, `mcp:read`).
- **OAuth token:** held in the operator secret store and handed to `HttpFleetMcp` through `OAuthTokens`; it is only placed in an `Authorization` header and is never logged or chained. The chain records a credential *label*, not a token or fingerprint.
- **Owner-run live verification:** point `HttpFleetMcp::new(COGNITUM_MCP_URL, ...)` at the cloud with a read-scope token, run `candidates`, then `cross_check` against the paired Seed, and keep the chain export. The MCP session handshake (`initialize`, `notifications/initialized`, `tools/call`) is implemented against a local stub only; the real server may differ.
