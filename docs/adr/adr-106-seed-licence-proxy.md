# ADR-106: The Cognitum Seed is the licence proxy for its WeftOS mesh

- **Status**: Proposed
- **Date**: 2026-10-03
- **Deciders**: Owner / platform
- **Depends-On**: ADR-099 (placement; section 6 swarm, section 8 trust), ADR-100 (cog kind; section 5 Seed, section 6 trust), ADR-103 (mesh service, admission A10, K1), ADR-105 (cog sources; section 3 licensing), the mesh-facts branch (`RedistributionPolicy` with `Audience`, `GrantOrigin`, `ServePeer`; commit `4221bb0ab`, not yet merged into `integrate/cog-repo`)
- **Relates-To**: COG-009 (`weft-cog-host` on the appliance), COG-011 (per-node Ed25519 request signing, no-RTC clock floor) in the private cogs repo
- **Implementation**: none yet. Phase plan below.

## Owner decisions this ADR implements (not reopened here)

1. The Seed works as a proxy for the one WeftOS mesh it is attached to, so licensing works without overloading the Seed device.
2. The Seed fetches and maintains the licence, the Cognitum credential, the cogs and anything else needed. When a node needs to check out a cog, the Seed handles it.
3. One Seed licence covers the whole attached mesh.
4. Once a cog is checked out, no other node needs permission. A checkout is per mesh, not per node. Any node in that mesh may fetch, run and share the cog with no further Seed round trip. This is also what keeps load off the Seed: it serves the bytes once per version, and mesh peers share them through the swarm.

## Context

What exists today, from local sources:

- **Licence is a declaration per project.** ADR-105 section 3 gates a `cognitum` install on a `[[cog_licence]]` record in `.weftos/cog-sources.toml`: presence, coverage, expiry (`crates/weftos-cog-sources/src/licence.rs`). The record is not signed and proves nothing. Every node that installs needs its own copy. A Cognitum install records `placement_eligible = false` (`install.rs`), because ADR-100 6.3 needs an operator to hash and sign an upstream binary before governed placement.
- **The swarm does not share Cognitum bytes.** On the mesh-facts branch, `crates/clawft-kernel/src/mesh_swarm_state.rs` has one policy point, `RedistributionPolicy::allows(content_hash, grants, audience)`. `Audience` is `Serve(&ServePeer)`, `Advertise` or `Seed`. Grants carry a `GrantOrigin`: `Cognitum {cog_id, version}`, `OptIn` or `NotFlagged`. The default `ManifestPolicy` allows a hash only when every grant is `OptIn`, so Cognitum-origin content is never seeded, advertised or served (finding F3). Non-`OptIn` content never reaches broadcast facts and is found only through `who_has`. The policy is fixed when the exchange is built (`ExchangeConfig`) and has no setter. `serve_as(stream, &ServePeer)` threads the verified peer. Plain `serve(stream, &str)` treats the peer as unverified.
- **Seed binding is a library.** `workload_runtime/seed_bind.rs` verifies an operator-signed `BindRecord {device_id, device_pubkey, node_id, bound_at}` against the Seed's `GET /api/v1/identity`, persists replay memory in `workload-seed-binds.json`, and chains `workload.node.bind`. No daemon or RPC path reaches it (ADR-100 section 5, `seed-adapter-operations.md` section 5).
- **Seed links must be pinned.** `SeedBinder::bind` and `add_seed` refuse a link that is not https with an SPKI or certificate pin, unless the runtime dir's `workload-seeds.json` sets the per-Seed `allow_unpinned_lab_link` opt-in.
- **Revocation floods.** `mesh_swarm_revoke.rs` carries signed notices (package id, signer key, BLAKE3 hash) on `mesh.artifact.revoke`. Only `Operator` or `Weftos` keys may sign them, with budgets checked before signature verification, and each notice is forwarded once. Revocation evicts bytes. A running instance continues until it restarts.
- **Admission.** A peer counts as verified only when admitted: its hello verified, the mesh in `enforce` mode, and a verdict permit (ADR-103 A10, K1 limits). Under `off` or `observe` no peer is verified. The hello signs a `genesis_hash`, which `CryptoGate` compares to its pin. That hash is a cluster label, not a credential.
- **The real Seed** (memory notes, firmware 0.24.2, not re-checked here):
  - The agent API is plain HTTP on :80, with a self-signed :8443.
  - `apps/install {"id"}` installs a cog *and starts it*. There is no fetch-only call.
  - The agent caps active cogs at 3. The cap is compiled in.
  - `PUT config` replaces the whole config and restarts the cog.
  - Store ingest is open on localhost.
  - The userland is armhf.
  - The account sign-in is held by the agent (`/api/v1/oauth/status`).
  - `weft-cog-host` (COG-009, `crates/weftos-cog-host`) already runs on the Seed, outside the agent's slot cap. It is plain HTTP with a bearer token, binds all interfaces, and has CSRF, CORS and Host-header guards.
- **No clock.** The Seed has no RTC. The COG-011 bridge refuses to sign until the clock passes a floor date (`clock_not_set`).

The gap: licensing is per project and declarative. The swarm refuses Cognitum bytes outright, so every node that wants a Cognitum cog downloads it from Cognitum itself under its own declared licence. That is the opposite of decisions 3 and 4.

## Decision

### 1. Roles

| Role | Holds | Never holds |
|---|---|---|
| **Seed licence proxy** (on the Seed) | the licence; the Cognitum credential (through the agent); a **grant key** generated on the Seed; the cache of Cognitum binaries it has checked out; the mesh binding | mesh membership. It does not seed the swarm. |
| **Steward node** (one admitted WeftOS node per mesh) | the Seed adapter config and the pinned link; the steward key that signs requests to the Seed; the first copy of each checked-out artifact | Cognitum tokens, the licence, the grant key |
| **Member nodes** | the verified grants and the binding record they received by flood; copies fetched from peers | Cognitum tokens, any link to the Seed |

Mesh nodes never see a Cognitum token. In the recommended design the proxy process on the Seed never sees one either, as long as fetching can stay with the agent or the public registry (open question C2).

### 2. Where the proxy logic runs: a `licence` service in `weft-cog-host` on the Seed

Recommended: the licence check, the Cognitum fetch, the hash checks and the grant signing run on the Seed. They run as a `licence` module of `weft-cog-host`, the WeaveLogic-signed process COG-009 already puts on the appliance. The steward is a WeftOS node that only relays requests and holds no authority.

Why this option:

- **The Seed is the licence holder** (decision 2). The grant key and the decision to sign belong on the same box as the licence and the account. If they sit on a WeftOS node, the Seed is just a downloader.
- **`weft-cog-host` is already there.**
  - It runs outside the agent's compiled-in 3-cog cap.
  - It is installed signed-only (COG-008).
  - It already has a request-policy layer (`auth.rs`).
  - A new agent cog would instead take one of the 3 slots, be restarted by every config `PUT`, and live under the agent's sandbox. Whether that sandbox can write a key file is not verifiable here.
- **The firmware offers part of what is needed.**
  - Already there: the device identity (`GET /api/v1/identity`), the store listing with sha256, the account sign-in, and the cog binaries the agent has downloaded (`/var/lib/cognitum/apps/<id>/`).
  - Not offered: fetch without install (install also starts the cog), a signing call a local process can use (the `custody` MCP tools suggest one exists; **assumption**, not verified), and any entitlement API.
  - So the proxy fetches from the registry the way the `cognitum` source kind already does (`weftos-cog-sources`: https only, registry sha256, https-to-http redirect refused). It signs with its own grant key.

Rejected:

- **(b) The WeftOS Seed adapter drives the store API from a node.** The signing key and the licence would live on the node. Getting bytes off the Seed would need the install call, which starts the cog, or SSH.
- **(c) A WeftOS node acts for the Seed.** Same problem, plus the Seed adds nothing to licensing.
- **An agent cog.** It takes a slot, and config `PUT` and sandbox limits apply (see above).

Built new:

- the `licence` module in `weft-cog-host`: an armhf build, a grant key file, a checkout and renew API, and a cache;
- the steward relay in the WeftOS daemon;
- the grant and binding types, the flood topics and the mesh policy in `clawft-kernel`;
- wiring `SeedBinder` to an RPC.

### 3. Mesh binding: a Seed is bound to exactly one mesh

- **Mesh id.** `mesh_id` = the admission genesis pin of the mesh (`CryptoGate`, 64 hex). It is a label that names the admission domain. Authority comes from admission, never from the label. A node belongs to one mesh: its machine mesh service has one listener and one pin.
- **Binding record v2.** This extends `BindRecord`: `{v: 2, device_id, device_pubkey, node_id (the Seed's adapter node id), mesh_id, grant_pubkey, steward_node_id, steward_pubkey, state: bound|unbound, seq, bound_at}`.
  - It is signed by a pinned operator key with the domain `weftos.cog.mesh_binding.v1\0`.
  - The operator approves by signing. There is no automatic binding.
- **Verification.** `SeedBinder` runs today's checks. It also requires:
  - the proxy's `GET /licence/v1/identity` returns the same `device_id` and `grant_pubkey`;
  - `mesh_id` equals the local pin;
  - `seq` is greater than the last seq accepted for that mesh.
- **Wiring.** It is reached through the existing Admin RPC `workload.node.bind`, with state in `dir.join(BIND_STATE_FILE)` (runtime dir).
- **Distribution.** The steward floods the signed record on a new control topic, `mesh.cog.binding`. It uses the revocation rules: size, pinned key and already-applied checks first, then a per-connection budget before signature verification, forwarded once. Every node keeps the highest `seq` per `mesh_id`. A different record at the same `seq` is refused and chained as `binding_conflict`.
- **One mesh only.** The proxy stores one binding. It refuses a bind for another `mesh_id` while bound (`seed_bound_elsewhere`). One Seed per mesh in phases 1 to 3.
- **Rebinding.**
  - *New steward:* the same Seed, a new steward, a higher `seq`. Grants continue, because grants do not name the steward.
  - *Move to another mesh:* the operator of mesh A signs `state: unbound`. The proxy deletes its grant key and generates a new one. Then it can be bound to mesh B. On the unbind record, A's nodes drop every grant under the old key at once (section 6).
  - *No operator available* (Seed resold, keys lost): a local reset over the USB link (physical presence) wipes the binding and the key. A's grants then lapse at expiry. A's operator can also revoke the old grant key with an existing `SignerKey` revocation notice.

### 4. Checkout protocol

1. **Local check first** (decision 4). A node that wants `cognitum:<cog>` version V for its arch looks in its grant store. If a valid grant for this mesh covers (cog, V, arch), it fetches from peers through `who_has`, with no request to anyone.
2. **Request.** Otherwise it sends `CheckoutRequest {request_id, cog_id, version | "latest", arch}` to the steward. The request goes on the mesh request path, topic `mesh.cog.checkout`.
   - The steward accepts it only from an admitted peer (`PeerCtx.node_verified`, class `node`).
   - It asks its governance gate for `cog.checkout` (Write).
   - It merges concurrent requests for the same (cog, version, arch) into one.
3. **Relay.** The steward calls `POST /licence/v1/checkout` on the proxy, over the link in section 7. The request is signed per request with the steward key, using the COG-011 scheme: method, path, node, timestamp, nonce, body hash. The proxy accepts only the `steward_pubkey` of its binding.
4. **Seed decision.**
   - A licence must cover the cog and be unexpired. If not, the proxy answers `cog_unlicensed` or `licence_expired`.
   - If a valid grant already exists for (mesh, cog, V), the proxy returns it, with bytes only when the steward asks for them (section 5).
   - Otherwise it fetches the binary for that arch from the registry and checks size and registry sha256. It computes BLAKE3, the swarm's content hash (`mesh_artifact.rs`), keeps the binary in its cache and signs a grant.
5. **Grant.** The proxy returns `SignedCheckoutGrant` and the bytes. The steward:
   - verifies the signature, the binding, the `mesh_id`, the sha256 and the BLAKE3 of what it received;
   - checks that `issued_at` is within 5 minutes of its own clock, or refuses with `seed_clock_skew`;
   - stores the bytes and registers the grant in its `ArtifactExchange`;
   - chains `cog.checkout.granted`;
   - floods the grant on `mesh.cog.grant`, with the same rules as the binding topic.
6. **Use.** The requester, and later any other node, fetches from peers. The steward is the first seeder. No node asks the Seed again for that version.

**Mesh Checkout Grant** (canonical JSON, Ed25519 by the grant key, domain `weftos.cog.mesh_checkout_grant.v1\0`):

```
{ v: 1, grant_id,                       # sha256 of the signed payload, hex
  mesh_id, seed_device_id, grant_key_id,        # "ed25519:" + 16 hex
  source: "cognitum", registry, cog_id, version,
  artifacts: [{ arch, size, sha256, blake3 }],   # every content hash covered
  manifest_sha256,                               # the registry entry the Seed used
  licence: { licence_id, account, expires },     # what allowed it
  seq,                                           # per-Seed, strictly increasing
  issued_at, expires_at }
```

**Replacement.** For one (mesh, cog, version) the highest `seq` wins, and a lower `seq` is ignored. A **withdrawal** is a renewal with `expires_at <= issued_at`. It stops sharing at once and needs no new revocation authority: Seed keys cannot sign revocation notices, and that stays so.

**Renewal.**
- Default TTL is 72 h, configurable up to 7 days. The steward pulls `GET /licence/v1/grants?since=<seq>` every 12 h and on demand, so the Seed never opens a connection into the mesh.
- The proxy renews every active checkout whose licence still covers it. A renewal is a new grant with a higher `seq` and costs one signature. Renewals come in batches of at most 256 grants.
- A checkout stays active until the operator releases it (`weaver cog checkout release`) or the licence stops covering it.

**Clock handling** (no RTC on the Seed, possibly none on members):
- The proxy refuses to sign until its clock passes a build-time floor and its own persisted last `issued_at` (`clock_not_set`, as COG-011).
- Every verifier keeps a floor: the highest `issued_at` it has accepted from that grant key. It judges expiry at `max(now, floor)`. A node whose clock is set back cannot bring an expired grant back to life.
- A grant whose `issued_at` is more than 5 minutes past the verifier's `max(now, floor)` is refused as not yet valid.

### 5. Plugging into the swarm

The mesh-facts trait already has the needed shape (`allows(hash, grants, &Audience)`, with `Serve(&ServePeer)`). The changes:

1. **`ServePeer` must mean "admitted to this mesh".**
   - Build it only from `PeerCtx` with `node_verified && class == Node`.
   - The mesh serve loop must call `serve_as` / `serve_frame_as` with it. The `&str` entry points stay unverified.
   - No new field: under A10, `node_verified` implies an admitted, `enforce`, permit connection to the local mesh, and a node is in one mesh.
   - If that invariant ever loosens, add `mesh_id` to `ServePeer` (`mesh_swarm_state.rs`).
2. **A grant source for checkouts.**
   - New `ArtifactExchange::grant_checkout(&VerifiedCheckoutGrant)` (`mesh_artifact.rs`). It calls `grant_with(blake3, "checkout:<cog>@<version>", vec![grant_pubkey_hex], GrantOrigin::Cognitum {cog_id, version})` for each artifact.
   - Revocation of the grant key, the hash or the package id then applies through the existing `is_revoked_subject`.
3. **`MeshCheckoutPolicy`**, in a new `mesh_checkout_policy.rs`, wraps `ManifestPolicy`. It holds `Arc<CheckoutGrantStore>` and the local `mesh_id`. It is set in `ExchangeConfig::redistribution` by the daemon when the node has an accepted binding. Otherwise the daemon keeps `ManifestPolicy`. Its rule:
   - If `ManifestPolicy` allows, allow (no change for `OptIn` content).
   - Otherwise allow only when all of these hold:
     - every non-`OptIn` grant for the hash is `GrantOrigin::Cognitum`, and the store holds a valid checkout grant for this `mesh_id` that covers that hash with the same `cog_id` and `version`;
     - the grant key matches the current binding;
     - the grant is not expired at `max(now, floor)`;
     - the audience is `Serve(peer)` with `peer.verified`, or `Seed`.
   - `Advertise` is never allowed for Cognitum-origin content (the branch rule stays).
   - Validity is checked on each call, so expiry needs no sweep to take effect. A periodic sweep drops expired grants and evicts bytes that no running instance pins.
4. **Fail-closed edges.**
   - Under `off` or `observe` no peer is verified, so no Cognitum bytes move between nodes. A Seed-bound mesh needs `enforce`.
   - A `Legacy` or `Leaf` peer is never served Cognitum content.

### 6. Load on the Seed

- Bytes go from the Seed to the steward once per (cog, version, arch). Everything after that is peer to peer. The Seed is not a mesh member and never seeds.
- After the first checkout the Seed's steady-state work is one signature per active checkout per renewal, plus one small HTTP exchange every 12 h.
- Proxy limits (defaults, all configurable):
  - 1 checkout in flight;
  - 10 requests per minute;
  - largest artifact 64 MiB;
  - transfers to the steward capped at 4 MiB/s;
  - each (cog, version, arch) served at most 3 times per 24 h, which covers steward retries and a steward replacement;
  - cache capped at 256 MiB, LRU, with entries still under an active grant kept;
  - renew batches of at most 256 grants.
- Steward limits: merge duplicate requests, and allow 5 checkout requests per minute per member node.

### 7. Link between steward and Seed

`weft-cog-host` is plain HTTP. Signed grants and signed requests give integrity and authenticity in both directions, and bytes are checked against the signed hashes. Confidentiality of licensed bytes still needs an encrypted link. The licence API is therefore reached only over:
- the tailnet (COG-002), or
- the USB link-local cable,

and the `licence` listener binds those interfaces only. A plain LAN path needs the same explicit per-Seed lab opt-in as the adapter (`allow_unpinned_lab_link`, read only from the runtime dir). This matches the ADR-100 pinned-link rule. Adding TLS with a pinned SPKI to `weft-cog-host` is the later alternative (phase 2 decides).

### 8. Lapse, unbinding and revocation

| Event | Effect on sharing | Effect on running instances |
|---|---|---|
| Licence lapses on the Seed | no renewals; grants lapse within one TTL (72 h by default). The proxy may issue a withdrawal at once | keep running until they stop or restart; a start or restart is refused (phase 3) |
| Seed offline or steward down | renewals stop; grants lapse at expiry. That is the offline grace: up to one TTL | same |
| Operator unbinds | flooded unbind record: every grant under that key stops at once | same |
| Grant key, hash or package revoked (existing notice) | immediate, bytes evicted | same (existing revocation semantics) |

**Recommendation: lapse is a soft stop.** Sharing and new starts stop, and running instances finish. Reasons:
- It matches today's revocation semantics.
- A lapse caused by a clock or network fault should not kill safety-relevant cogs mid-run, such as fall detection.
- The overrun is bounded by the instance's next restart.

If Cognitum requires a hard stop (open question C5), add `hard_stop: true` to a withdrawal. Placement would then `stop` instances of that hash through the existing teardown path, which `check_teardown` already allows.

### 9. Trust and threats

- **Pinning.**
  - Nodes trust a grant key only through an operator-signed binding for their own mesh.
  - The steward reaches the Seed only over the link in section 7, and the adapter link keeps its SPKI pin.
  - Phase 4 adds a device-key cross-certificate of the grant key, if the Seed exposes device signing (C4).
- **A malicious member forges a grant.** It cannot: grants verify only under the bound grant key. A node-signed or unbound-key grant is refused and chained (`grant_untrusted`).
- **A member replays an old grant.** The `seq` replacement rule, expiry and the clock floor defeat it.
- **Honest limit:** an admitted member that holds the bytes can copy them out of the mesh. The grant governs WeftOS redistribution, not DRM.
- **A stolen Seed** can sign grants for its mesh id, but only admitted members of that mesh act on them. The response is to unbind and revoke the grant key (`SignerKey` notice). The key file is mode 0600, created with `create_new`, on the SD card. Hardware-backed storage is not available (assumption: no secure element is exposed).
- **Seed clock wrong.** The steward refuses grants skewed more than 5 minutes. Verifiers use the floor rule. The proxy refuses to sign before its clock floor.
- **A malicious steward** cannot mint grants. It can withhold renewals or requests, which is availability only. The operator rebinds to another steward.

### 10. Reconciliation with ADR-105, ADR-100 and finding F3

- **ADR-105 section 3 (`[[cog_licence]]`).** In a Seed-bound mesh it is no longer the install gate on member nodes. A valid grant is. The licence record moves to the Seed: operator-signed in phase 2, signed by Cognitum in phase 4. Projects outside a Seed-bound mesh keep section 3 unchanged.
- **ADR-105 section 2 (`placement_eligible = false` for `cognitum`).** Unchanged for direct installs. A grant-backed install becomes placement-eligible inside the grant's mesh while the grant is valid (phase 3). This **supersedes, for Seed-bound meshes only**, the ADR-100 6.3 requirement that an operator hash and sign upstream binaries. The grant names the content hashes, and `manifest_sha256` binds the registry entry, which serves as the manifest (the analogue of the `cog.toml` pin in ADR-105 6.2).
- **ADR-105 open question 4 (signed licences).** Partly answered: the grant is a signed statement, but until phase 4 it attests an operator-declared licence, not a Cognitum proof.
- **Provenance** gains `trust = "mesh-checkout-grant"`, `grant_id`, `mesh_id` and `grant_key_id`.
- **The `cognitum` source on member nodes.** In a Seed-bound mesh it resolves through checkout instead of downloading directly.
- **ADR-099 8.4 and ADR-100 6.4.** Commercial terms stay contractual. This ADR enforces *redistribution scope*, not payment.
- **ADR-100 section 5.** "Seed bind is a library" becomes wired in phase 1. The store-pin rule for cogs run *on* the Seed by the agent is unchanged.
- **F3 (mesh-facts).** The default stays: Cognitum-origin content is never redistributable. `MeshCheckoutPolicy` is the only exception, scoped to admitted members of the grant's mesh while the grant is valid.

## Phases

**Phase 1: kernel and daemon side, tested in process (no hardware, no network).**
- `mesh_checkout.rs`: grant and binding types, canonical JSON, domain tags, sign and verify, the clock floor, and `CheckoutGrantStore` with seq replacement and expiry.
- The `mesh.cog.binding` and `mesh.cog.grant` control topics.
- `MeshCheckoutPolicy`, `grant_checkout`, and `serve_as` wiring from `PeerCtx`.
- The steward relay and the `cog.checkout` RPC.
- `SeedBinder` wired to `workload.node.bind` with BindRecord v2.
- A stub proxy in the test kit implementing the section 4 HTTP contract.

Acceptance, as tests under `scripts/build.sh test`, with `clippy` clean:
1. A 3-node `enforce` mesh with the stub. A checks out. B and C install from peers, and the stub records exactly 1 checkout and 1 byte transfer.
2. A peer under `observe`, an unverified peer, or a `Legacy` peer gets nothing.
3. A grant signed by a node key, by an unbound key, or for another `mesh_id` is refused and chained.
4. After expiry nothing is served or seeded, and a running instance is untouched.
5. An unbind record stops serving at once.
6. A lower-seq grant is ignored, and a withdrawal stops serving.
7. A node with its clock set back cannot revive an expired grant.
8. A `SignerKey` revocation of the grant key ends every grant.
9. A bind for a second mesh is refused (`seed_bound_elsewhere`).

**Phase 2: the Seed side, and the steward over the real link.**
- The `licence` module in `weft-cog-host` (armhf):
  - `licence init --operator-key` over USB;
  - grant key generation;
  - an operator-signed licence record (the ADR-105 fields plus `mesh_id`);
  - the checkout, renew and identity endpoints;
  - registry fetch through `weftos-cog-sources`;
  - the limits in section 6 and the clock floor;
  - a listener bound to the tailnet and USB only.

Acceptance:
- Tests against a stub registry.
- The armhf build size is recorded, and `weftos-cog-sources` is confirmed to build for `armv7-unknown-linux-gnueabihf`.
- **Owner-run on the real Seed:** check out `fall-detect` for aarch64; a second node runs it from peers; the proxy log shows one byte transfer; powering the Seed off for longer than the TTL stops sharing.

**Phase 3: placement and lifecycle.**
- The grant as a `VerifyPolicy` trust path for governed placement.
- A grant check at start time in `weft-cog-host` and the workload host.
- `weaver cog checkout list|release|renew`.
- Chain events: `cog.checkout.request|granted|renewed|refused|lapsed`.
- `weaver doctor` checks for binding health and the grant-expiry horizon.

Acceptance: a Cognitum cog is placed on a member node with no operator re-sign, and refused after lapse on restart.

**Phase 4: needs Cognitum.**
- A Cognitum-signed entitlement replaces the operator-signed licence.
- Licensed download with the Seed's account.
- A device-key cross-certificate of the grant key.
- A Cognitum revocation feed relayed as withdrawals.
- Multi-Seed meshes, if wanted.

## Open questions for Cognitum (C) and for us (W)

Anything not verifiable from local sources is marked as an assumption above.

- **C1. Entitlement API.** Does a Seed account have per-cog entitlements? Is there a signed artifact, and how does the Seed fetch it? What offline grace does Cognitum allow? (Today none exists: ADR-105 open question 4.)
- **C2. Licensed download.** Will registry binaries move behind authentication? If so, how does a local process on the Seed get a download, without holding the token: a fetch-only agent call, or a short-lived URL?
- **C3. Commercial scope.** Does Cognitum accept one Seed licence covering a whole mesh (decision 3)? Is there a node or size limit? Do the cog licence terms permit redistribution between nodes of one operator?
- **C4. Device signing.** Is there an HTTP call by which a local process can have the device key sign a statement (the `custody` tools)? What is the key format of `/api/v1/identity`?
- **C5. Hard stop.** Must a lapse stop running instances, or is the soft stop in section 8 acceptable?
- **C6. Security pulls.** Will Cognitum publish a feed of withdrawn cog versions?
- **C7. Arches.** Does the registry carry aarch64 and x86_64 binaries for every cog, beside armhf?
- **C8. Install without start**, so the Seed could reuse the agent's own download.
- **W1. `mesh_id`.** Is the genesis pin enough, or should the mesh carry a signed mesh certificate (cluster owner key)? It is enough while admission is the authority.
- **W2. Steward availability.** Allow one standby steward in the binding?
- **W3. Checkout lifetime.** Should checkouts no node has used for N days be released automatically to shrink renewal work?
- **W4. TLS in `weft-cog-host`**, or tailnet only (section 7)?
