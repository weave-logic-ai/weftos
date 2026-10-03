# ADR-106: The Cognitum Seed is the licence proxy for its WeftOS mesh

- **Status**: Proposed
- **Date**: 2026-10-03
- **Updated**: 2026-10-03. Review round 1 (SOUND-WITH-CHANGES) folded in:
  - B1: the signer is a separate process under its own uid.
  - B2: ADR-100 6.3 is not superseded before phase 4, and members re-check the registry sha256 themselves.
  - H1: `mesh_id` includes the cluster-owner key, and open membership is refused.
  - H2: only mesh-admitted connections are served.
  - H3: the production transport is phase 1 work.
  - H4: catch-up sync.
  - H5: the steward can delay a withdrawal, and any operator-Admin node can unbind.
  - M1 to M7 and L1 to L6 are folded in below.
- **Deciders**: Owner / platform
- **Depends-On**:
  - ADR-099 (placement; section 6 swarm, section 8 trust)
  - ADR-100 (cog kind; section 5 Seed, section 6 trust)
  - ADR-103 (mesh service, admission A10, K1; machine and user tenants)
  - ADR-105 (cog sources; section 3 licensing)
  - mesh-facts (`RedistributionPolicy` with `Audience`, `GrantOrigin`, `ServePeer`), now in `integrate/cog-repo`
- **Relates-To**: COG-009 (`weft-cog-host` on the appliance) and COG-011 (per-node Ed25519 request signing, clock floor), both in the private cogs repo
- **Implementation**: none yet. Phase plan below.

## Owner decisions this ADR implements (not reopened here)

1. The Seed works as a proxy for the one WeftOS mesh it is attached to. Licensing works and the Seed device is not overloaded.
2. The Seed fetches and maintains the licence, the Cognitum credential, the cogs and anything else needed. When a node needs to check out a cog, the Seed handles it.
3. One Seed licence covers the whole attached mesh. **Pending C3**: Cognitum must agree to this commercially.
4. Once a cog is checked out, no other node needs permission. A checkout is per mesh, not per node, and any node in that mesh may fetch, run and share the cog with no further Seed round trip. This also keeps load off the Seed: it serves the bytes once per version, and mesh peers share them through the swarm.

## Context

What exists, from local sources:

- **Licence is a per-project declaration.** ADR-105 section 3 gates a `cognitum` install on an unsigned `[[cog_licence]]` record (`crates/weftos-cog-sources/src/licence.rs`). The check is presence, coverage and expiry. Each installing node needs its own record. Cognitum installs record `placement_eligible = false`, because ADR-100 6.3 requires an operator to hash and sign an upstream binary before governed placement.
- **The swarm refuses Cognitum bytes.**
  - The policy point is `RedistributionPolicy::allows(hash, grants, &Audience)` in `mesh_swarm_state.rs`. The audience is `Serve(&ServePeer)` (a node id plus a verified flag), `Advertise` or `Seed`.
  - Each grant carries a `GrantOrigin`: `Cognitum {cog_id, version}`, `OptIn` or `NotFlagged`.
  - The default `ManifestPolicy` allows a hash only when every grant is `OptIn`. That is finding F3.
  - Non-`OptIn` content is never put in broadcast facts. Peers find it only through `who_has`, where the serving side sees who is asking.
  - The policy is the `ExchangeConfig.redistribution` field, set once at construction.
  - `CogPackageBody` is `deny_unknown_fields`.
- **No production peer-to-peer artifact transport.**
  - `PeerDialer` is test-only.
  - The only production serve is `workload_ctl` `session.rs` (`serve_frame`), and it serves an *unverified* peer.
- **Admission.**
  - A peer counts as verified only when it is admitted: its hello verified, the mesh is in `enforce` mode, and the verdict is a permit (ADR-103 A10, K1).
  - The hello signs a `genesis_hash`. That is an operator-chosen label, not a credential.
  - `kernel.mesh.admission_open_membership` admits any valid hello with that label (`boot.rs`, the `bind_open` branch).
  - `TenantRouter::deliver_local` builds `PeerCtx {node_verified: true}` for every local tenant (`router.rs`), including another OS user's tenant on the same machine.
- **Seed binding is a library.**
  - `workload_runtime/seed_bind.rs` verifies an operator-signed `BindRecord` against the Seed's `GET /api/v1/identity`. It accepts a record only within a 600 s age window and persists replay memory.
  - `workload.node.bind` is a reserved method in the daemon's `NOT_YET` list (`clawft-weave/src/workload_rpc.rs`).
  - Seed links must be pinned (https with an SPKI pin), unless the runtime dir sets a per-Seed lab opt-in.
- **Revocation floods without catch-up.** `mesh_swarm_revoke.rs` signs notices with `Operator`/`Weftos` keys, and any operator node can `issue` one. Each node forwards a notice once. There is no anti-entropy, so a late joiner never receives an earlier notice.
- **`weft-cog-host` on the Seed** (`crates/weftos-cog-host`):
  - It spawns every cog as its own uid (`supervise.rs`).
  - `POST /install` accepts an unsigned binary whose sha the requester supplies.
  - It binds `0.0.0.0` over plain HTTP. Its bearer token is a shared secret on the LAN.
- **The real Seed** (memory notes, firmware 0.24.2, not re-checked here):
  - The agent serves plain HTTP on :80 and a self-signed :8443.
  - `apps/install` also starts the cog. There is no fetch-only call.
  - Active cogs are capped at 3, and the cap is compiled in.
  - The userland is armhf, and there is no RTC.
  - The account sign-in is held by the agent.
- **COG-011 clock handling.** While the clock is below a floor date, the bridge refuses to verify signed requests (`clock_not_set`) and the relay does not send.

**Gap.** Licensing is per project and declarative, and the swarm refuses Cognitum bytes, so every node downloads from Cognitum under its own declared licence. That is the opposite of decisions 3 and 4.

## Decision

**What this ADR is.** It scopes WeftOS redistribution of Cognitum cogs to one admitted mesh. It is not copy protection, and it is not payment enforcement.

### 1. Roles

| Role | Holds | Never holds |
|---|---|---|
| **`weft-licence`** (on the Seed) | the licence, the grant key, the cache of checked-out binaries, the mesh binding, the persisted `seq` and clock floor | mesh membership. It does not seed the swarm and runs no cogs. |
| **Steward node** (one admitted WeftOS node) | the link to `weft-licence`, the steward key, the first copy of each artifact | the grant key, the licence, Cognitum tokens |
| **Member nodes** | the binding and grants (received by flood and sync), and copies fetched from peers | Cognitum tokens, any link to the Seed |

Mesh nodes never see a Cognitum token. `weft-licence` does not need one either, as long as the registry stays public (C2).

### 2. Where the proxy logic runs: `weft-licence`, a separate process on the Seed

The licence check, the Cognitum fetch, the hash checks and the grant signing run on the Seed in **`weft-licence`**. It is a WeftOS-built binary installed signed-only (COG-008) beside `weft-cog-host`, but it is **not part of `weft-cog-host`**:

- It runs as its own system user (`weft-licence`) under its own service unit.
- Its key file is mode 0600 in a 0700 directory that this user owns.
- It shares nothing with `weft-cog-host`: not the uid, not the bearer token, not the API.

`weft-cog-host` runs every cog as its own uid and accepts unsigned installs from anyone holding the LAN-wide token. A grant key inside that process would be readable by any cog it runs and by any token holder. The `weft-licence` unit has no endpoint that runs code. It does not import the cog-host request policy. Its listener is separate from cog-host's `0.0.0.0` (section 7).

Why on the Seed: the Seed is the licence holder (decision 2). Signing belongs on the box that holds the licence and the account. Why not as an agent cog: it would take one of the 3 slots, a config `PUT` restarts it, and the agent's sandbox is unknown.

What the firmware already offers:
- identity (`GET /api/v1/identity`),
- the store listing with sha256,
- the account sign-in.

What it does not offer:
- fetch without install,
- a device-key signing call usable by a local process (the `custody` tools are a hint only; **assumption**),
- an entitlement API.

So `weft-licence` fetches from the registry the way the `cognitum` source kind does (`weftos-cog-sources`: https, registry sha256, https-to-http redirect refused). It signs with its own grant key.

Rejected options:
- A WeftOS node drives the store API. The key and the licence would sit on the node, and the bytes would only come out through install, which also starts the cog, or through SSH.
- A node acts for the Seed. Same objection.
- A module inside `weft-cog-host`. Its uid is shared with the cogs it runs (B1).

### 3. Mesh binding: one Seed, one mesh

**Mesh id (W1, decided).**

```
mesh_id = sha256("weft-licence-v1/mesh-id\n" || genesis_pin || cluster_owner_user_pubkey)
```

- `genesis_pin` is the 32-byte genesis pin that admission compares.
- `cluster_owner_user_pubkey` is the Ed25519 user key of the designated cluster owner whose daemon answers admission verdicts (ADR-103 Phase 3).
- Two meshes that copied a label but have different owners get different ids.

**Admission requirement.** A node accepts a binding, and turns on the checkout part of its policy, only when all of these hold:
- admission is `enforce`,
- a governance verdict source is bound,
- `admission_open_membership` is **off**.

Otherwise it chains `binding_refused: open_membership` and stays on the `ManifestPolicy` behaviour.

**Binding record.** It extends `BindRecord` to v2:

```
{ v: 2, device_id, device_pubkey, node_id, mesh_id, grant_pubkey,
  steward_node_id, steward_pubkey, state: bound|unbound, seq, bound_at }
```

- It is signed by a pinned operator key under the domain tag `weft-licence-v1/binding`.
- Operator approval is the signature. There is no automatic binding.

**Where `grant_pubkey` comes from.**
1. The operator runs `weft-licence init --operator-key <hex>` over the USB link (physical presence).
2. That generates the grant key and prints the grant key's fingerprint.
3. The operator compares the fingerprint with what `weaver` shows before signing the binding.
4. The key is never learned over the network, so it cannot be swapped in transit.

**Two verification profiles (M1).**
- *Steward, full bind.* The full `SeedBinder` checks, including the 600 s age window and the live `GET /api/v1/identity` match. In addition, the grant key's fingerprint must match the one the operator confirmed, `mesh_id` must equal the local mesh id, and `seq` must exceed the last accepted `seq`. It is reached through the reserved method `workload.node.bind`, which this ADR moves out of `NOT_YET` (Admin; state in `dir.join(BIND_STATE_FILE)`).
- *Member, record only.* Members check the operator signature, that `mesh_id` equals the local id, that `seq` exceeds the last stored `seq`, and the `state`. There is no age check and no Seed contact, because members have no Seed link. A different record at an equal `seq` is refused and chained (`binding_conflict`).

**Distribution.** On the control topic `mesh.cog.binding`, using the revocation flood rules:
- cheap checks first (size, pinned key, already applied),
- a per-connection budget before signature verification,
- each record forwarded once,
- plus the sync step in section 5.

**Rebind and unbind.**
- *New steward.* Same Seed, higher `seq`. Grants continue, because they do not name the steward.
- *Unbind.* An operator-signed `state: unbound` with a higher `seq`. **Any operator-Admin node may issue it** and flood it, the same way `RevocationExchange::issue` works, so a steward cannot block it.
- *Move the Seed to another mesh.* Unbind first. `weft-licence` then deletes its grant key, and `init` runs again over USB.
- *No operator available.* A USB local reset wipes the binding and the key. Old grants lapse at expiry, and A's operator can revoke the old grant key with a `SignerKey` notice.

**One mesh only.** `weft-licence` stores one binding and refuses a bind for another `mesh_id` (`seed_bound_elsewhere`). Phases 1 to 3 allow one Seed per mesh.

**Licence scope inside a member machine (ADR-103).** A Seed licence covers the **admitted machines** of the mesh. The project kernels and user daemons on an admitted machine are covered as part of that machine. They obtain Cognitum cogs through that machine's own node, under the owning user daemon's governance gate. They are never served as a swarm peer through `deliver_local`. Another OS user's tenant on the steward's machine is not a swarm peer, and its checkout request is not accepted (section 4). Whether per-machine scope fits Cognitum's terms is part of C3.

### 4. Checkout protocol

1. **Local first (decision 4).** A node that wants `cognitum:<cog>` version V for its arch first looks for a valid grant for this mesh that covers (cog, V, arch). If it finds one, it fetches from peers via `who_has`, with no request to anyone.
2. **Request.** Otherwise it sends `CheckoutRequest {request_id, cog_id, version | "latest", arch}` to the steward on the `mesh.cog.checkout` request topic.
   - The steward accepts it only on a connection the **mesh listener admitted**: `PeerCtx.node_verified`, class `node`, and not a `deliver_local` delivery.
   - It also accepts requests from its own kernel.
   - It asks its governance gate for `cog.checkout` (Write).
   - Concurrent requests for the same (cog, version, arch) are merged.
3. **Relay.** The steward calls `POST /licence/v1/checkout` on `weft-licence`. Each request is signed with the steward key, using the COG-011 fields: method, path, node, timestamp, nonce and body hash. `weft-licence` accepts only the bound `steward_pubkey`.
4. **Seed decision.**
   - If no licence covers the cog, or the licence has expired: `cog_unlicensed` or `licence_expired`.
   - If a valid grant already exists, the existing grant is returned. Bytes are sent only on request.
   - Otherwise `weft-licence` fetches that arch from the registry and checks its size and registry sha256. It then computes BLAKE3 (the swarm content hash), caches the binary, **writes the new `seq` durably**, and only then releases the grant.
5. **Steward.** It verifies:
   - the signature, the binding and `mesh_id`,
   - the sha256 and BLAKE3 of the bytes it received,
   - that `issued_at` is within 5 min of its own clock (otherwise `seed_clock_skew`).

   It then stores the bytes, registers the grant and chains `cog.checkout.granted`, and floods the grant on `mesh.cog.grant`.
6. **Use.** Other nodes fetch from peers. The steward is the first seeder.
7. **Before running (B2).** A node installs or runs the bytes only after it has itself checked the grant's sha256 against the Cognitum registry entry for (cog, V, arch). It fetches that entry over https from its `cognitum` source; the entry is small and needs no licence. A grant alone never makes bytes runnable.

**Grant format.** Canonical JSON, signed Ed25519 by the grant key, domain tag `weft-licence-v1/grant`:

```
{ v: 1, grant_id,                        # sha256 of the signed payload
  mesh_id, seed_device_id, grant_key_id, # "ed25519:" + 16 hex
  source: "cognitum", registry, cog_id, version,
  artifacts: [{ arch, size, sha256, blake3 }],
  manifest_sha256,                       # registry entry used
  licence: { licence_id, account, expires },
  seq, issued_at, expires_at }
```

**Replacement rules (M6).**
- For one (mesh, cog, version), the highest `seq` wins and a lower `seq` is ignored.
- A newer grant carries the **union** of every arch checked out so far for that version, so adding an arch never drops another.
- Two different payloads with the same `seq` under one key are a fault: both are refused, `grant_conflict` is chained, and the newest earlier grant is kept. `weft-licence` makes this impossible by persisting `seq` before release.
- A **withdrawal** is a renewal with `expires_at <= issued_at`. Seed keys never sign revocation notices.

**Renewal.**
- Default TTL is 72 h, with a maximum of 7 days.
- The steward pulls `GET /licence/v1/grants?since=<seq>` every 12 h and on demand. The Seed never connects into the mesh.
- Renewal batches hold at most 256 grants.
- A checkout stays active until it is released (`weaver cog checkout release`) or the licence stops covering it.

**Clock (M2).**
- `weft-licence` refuses to sign until its clock passes a build-time floor and its persisted last `issued_at` (`clock_not_set`).
- Each verifier keeps, per grant key, `floor = max(highest accepted issued_at, persisted local-now high-water mark)`. It persists the floor and uses `max(now, floor)` for expiry, so a reset clock cannot revive a grant.
- A grant more than 5 min ahead of `max(now, floor)` is deferred as not yet valid and retried at the next sync.
- A floor more than 30 days ahead of a clock that the steward's sync confirms is treated as a far-future poisoning. The verifier chains it and clamps the floor to the newest accepted `issued_at`. An operator `weaver cog checkout reset-floor` (Admin, chained) is the manual reset.

### 5. Swarm, transport and sync

1. **Policy (M5).** The daemon always builds `ExchangeConfig.redistribution` as `MeshCheckoutPolicy`, in a new `mesh_checkout_policy.rs`. It wraps `ManifestPolicy` and holds `Arc<CheckoutGrantStore>` plus the local `mesh_id`.
   - With no accepted binding, it returns exactly what `ManifestPolicy` returns (tested).
   - A binding that arrives at runtime takes effect with no restart.
   - The rule: allow if `ManifestPolicy` allows. Otherwise allow only when all of these hold:
     - every non-`OptIn` grant for the hash is `Cognitum {cog_id, version}`;
     - the store holds a valid grant for this `mesh_id`, under the current binding's key, that covers the hash with the same cog and version;
     - the audience is `Serve(peer)` with `peer.verified`, or `Seed`.
   - `Advertise` is always denied, so peers look the content up via `who_has`.
   - Validity is checked on every call. A sweep drops expired grants and evicts bytes that no running instance pins.
2. **Grant source.** `ArtifactExchange::grant_checkout(&VerifiedCheckoutGrant)` calls `grant_with(blake3, "checkout:<cog>@<version>", [grant_pubkey_hex], GrantOrigin::Cognitum {..})`. Revocation of the key, the hash or the package then applies through `is_revoked_subject`. This makes the bytes *shareable* inside the mesh. It does not make them runnable (section 4, step 7) or placement-eligible (section 10).
3. **Who is a peer (H2).** A `ServePeer` is built only on connections the mesh listener admitted (`PeerCtx.node_verified`, class `node`). It is never built from `TenantRouter::deliver_local` contexts.
4. **Transport (H3, phase 1c).** This is new production work:
   - a production artifact dialer over admitted mesh connections, replacing the test-only `PeerDialer`;
   - an inbound artifact serve handler on the mesh listener that passes the admitted `PeerCtx` to `serve_as` / `serve_frame_as`;
   - changing the existing `workload_ctl` `session.rs` serve to take admission's verified flag instead of `ServePeer::unverified`.

   Until this exists, Cognitum bytes move only between the steward and its own kernel.
5. **Catch-up sync (H4).**
   - On each admitted connect, and every 30 min, a node asks the peer (preferably the steward) for `{binding, highest-seq grant per (cog, version)}`.
   - It verifies everything locally under its own profile.
   - It defers verification while `clock_not_set` and retries at the next sync.
   - Sync answers are budgeted like the flood topics.
   - This brings late joiners, partitions and transient refusals to the same state.
6. **No manifest change.** `CogPackageBody` is `deny_unknown_fields`, so an older verifier rejects an unknown field. The grant is a separate object and is never embedded in a package. New fields go only in `provenance.json`.
7. **Fail closed.** Under `off`, `observe` or open membership, no checkout policy is active and no Cognitum bytes move between nodes. `Legacy` and `Leaf` peers are never served.

### 6. Load on the Seed

- Bytes go from the Seed to the steward once per (cog, version, arch). After that, transfer is peer to peer, and the Seed never seeds.
- The steady state is one signature per active checkout per renewal, plus one small exchange every 12 h.

`weft-licence` defaults, all configurable:

| Limit | Default |
|---|---|
| Checkouts in flight | 1 |
| Requests per minute | 10 |
| Largest artifact | 64 MiB |
| Transfer rate to the steward | 4 MiB/s |
| Cache | 256 MiB LRU (entries under an active grant are kept) |
| Renewal batch | 256 grants |
| Byte transfers per (cog, version, arch) | 3 per 24 h, **per steward key** (L1); an operator-signed override can raise it, for example after a steward rebuild |

Steward defaults: merged duplicate requests, and 5 checkout requests per minute per member.

### 7. Link between steward and `weft-licence`

`weft-licence` serves plain HTTP on **its own listener**. That listener binds only the USB link-local interface and the tailnet interface (COG-002), never `0.0.0.0`.

What protects the link:
- Signed requests and signed grants give integrity and authenticity.
- Bytes are checked against the signed hashes.
- `grant_pubkey` comes only from the USB `init` with fingerprint confirmation (section 3), so swapping it in transit gains nothing.
- Confidentiality of the licensed bytes comes only from the link itself: WireGuard on the tailnet, or the physical USB cable.

A LAN path is plain text and needs the explicit per-Seed lab opt-in (`allow_unpinned_lab_link`, read only from the runtime dir). Without TLS, a normal deployment uses the tailnet or USB. In practice every Seed-bound mesh that wants a LAN-only link needs the opt-in until TLS with a pinned SPKI is added to `weft-licence` (W4, decided in phase 2).

### 8. Lapse, unbinding and revocation

| Event | Effect on sharing | Effect on running instances |
|---|---|---|
| Licence lapses | No renewals. Grants lapse within one TTL, and a withdrawal can stop them sooner. | Keep running. A start or restart is refused (phase 3). |
| Seed offline or steward down | Grants lapse at expiry. The offline grace is one TTL. | Same. |
| Unbind (any operator-Admin node) | Every grant under the key stops when the record arrives. | Same. |
| Revocation of the key, hash or package | Immediate, and the bytes are evicted. | Same (existing semantics). |

- **Latencies (L2).**
  - A withdrawal reaches the mesh within the next steward pull (at most 12 h), then flood or sync (30 min).
  - A TTL of T means a lapse can go unnoticed for up to T. The 7-day maximum therefore means up to a 7-day lapse.
  - The steward can delay a withdrawal for up to one TTL (section 9), which is why the default is 72 h.
- **Soft stop recommended.** Sharing and new starts stop, and running instances finish. This matches current revocation semantics. It does not kill safety-relevant cogs over a clock or network fault, and the overrun is bounded by the instance's next restart.
- **Hard stop, if C5 demands it.** A `hard_stop: true` withdrawal would make placement stop those instances through the teardown path that `check_teardown` already allows.

### 9. Trust and threats

- **Grant key isolation (B1).** The key lives only in `weft-licence`'s own uid and directory. Cogs, `weft-cog-host`, its token holders and the agent cannot read it. A compromised `weft-cog-host` or cog does not yield the key; what remains is root on the Seed, which is the stolen-Seed case.
- **A compromised grant key (B2)** lets its holder make member nodes *share* bytes inside the mesh under a forged grant. It does **not** let them run code:
  - every install or run re-checks the sha256 against the Cognitum registry independently (section 4, step 7);
  - governed placement still needs an operator signature until phase 4 (section 10).

  Response: unbind from any operator-Admin node, then a `SignerKey` revocation.
- **A malicious member forging grants.** Refused: grants verify only under the bound key (`grant_untrusted`). Replays are stopped by `seq`, expiry and the floor.
- **Honest limit.** An admitted member that holds the bytes can copy them out of the mesh. This is a redistribution scope, not DRM.
- **A malicious steward (H5)** cannot mint grants. However, it can:
  - withhold requests and renewals, which is an availability attack;
  - **suppress a withdrawal for up to one TTL**, and suppress an unbind that it is asked to relay.

  Mitigations:
  - unbind can be issued and flooded by any operator-Admin node;
  - sync from any peer spreads the record;
  - the TTL bounds the damage;
  - the steward can be replaced by a rebind.
- **A stolen Seed** can sign grants for its `mesh_id`, which only admitted members act on, and only for sharing. Response: unbind plus key revocation. There is no hardware key store (**assumption**).
- **Clocks.** `weft-licence` refuses to sign before its floor. The steward refuses grants skewed by more than 5 min. Verifiers keep a persisted floor with a clamp.
- **Local tenants (H2).** They are not swarm peers, and their checkout requests are refused (section 3, licence scope).

### 10. Reconciliation with ADR-105, ADR-100 and F3

- **ADR-105 section 3.** In a Seed-bound mesh, the `[[cog_licence]]` record is no longer the install gate on member nodes; a valid grant plus the member's own registry sha256 check is. The licence record moves to `weft-licence`: operator-signed in phase 2, Cognitum-signed in phase 4. Meshes without a Seed are unchanged.
- **ADR-105 section 2 and ADR-100 6.3: not superseded before phase 4 (B2).** Governed placement of a Cognitum cog still needs an operator hash and signature. The grant is shareability evidence, not a placement signer. In phase 4, once members can verify a statement Cognitum signed (C1), a grant that carries one may become placement-eligible inside its mesh.
- **ADR-105 open question 4.** The grant is a signed statement of an operator-declared licence. It is not a Cognitum proof until phase 4.
- **Provenance.** Adds `trust = "mesh-checkout-grant"`, `grant_id`, `mesh_id`, `grant_key_id` and `registry_sha256_checked`.
- **ADR-099 8.4 and ADR-100 6.4.** Commercial terms stay contractual.
- **ADR-100 section 5.** The reserved method `workload.node.bind` is implemented in phase 1d. The store-pin rule for cogs the agent runs on the Seed is unchanged.
- **F3.** The default stays: Cognitum content is never redistributable. `MeshCheckoutPolicy` is the only exception, and it applies to admitted members of the grant's mesh while the grant is valid.

## Phases

All of phase 1 runs in process, with no hardware and no network, under `scripts/build.sh test` and `clippy`.

**1a. Types, store and policy (pure unit tests).** `mesh_checkout.rs` (grant, binding v2, canonical JSON, domain tags, both verification profiles, seq rules, arch union, conflicts, the persisted floor and clamp), `CheckoutGrantStore`, `MeshCheckoutPolicy` and `grant_checkout`. Tests:
- without a binding the policy equals `ManifestPolicy`;
- a binding that arrives at runtime takes effect with no restart;
- a grant signed by a node key, signed by an unbound key, or for another `mesh_id` is refused;
- expiry stops serving and leaves a running instance untouched;
- an unbind stops serving at once;
- a lower-seq grant is ignored, and a withdrawal stops serving;
- a newer grant keeps the union of arches;
- a same-seq conflict is refused;
- a clock set back cannot revive a grant, the floor and the store survive a restart, and the far-future clamp works;
- a `SignerKey` revocation of the grant key ends every checkout grant, while a hash that another unrevoked package grant still lists keeps only that grant's own policy result (L6);
- open membership, `observe` and `off` refuse the binding.

**1b. Floods and sync.** The `mesh.cog.binding` and `mesh.cog.grant` topics (budgets, forward-once) and the sync step. Tests:
- a late joiner reaches the binding and the highest grants through sync;
- a partition heals;
- a grant deferred under `clock_not_set` is accepted later;
- an unbind issued by a non-steward Admin node reaches every node.

**1c. Transport and steward relay.** The production dialer, the inbound serve with `PeerCtx`, the admission flag in `session.rs`, the `mesh.cog.checkout` handler, and the steward client against a stub `weft-licence` that implements the HTTP contract in section 4. Tests:
- in a 3-node `enforce` mesh, A checks out, and B and C install from peers; the stub records exactly 1 checkout and 1 byte transfer;
- a `deliver_local` tenant is neither served nor accepted for checkout;
- an unverified peer, a `Legacy` peer and a `Leaf` peer are refused;
- a run is refused when the registry sha256 does not match the grant.

**1d. Bind RPC.** Implement the reserved method `workload.node.bind` (Admin), the full steward profile, and the fingerprint confirmation. Test the end-to-end bind and refusal paths with a stub Seed identity.

**Phase 2. `weft-licence` on the Seed (armhf).** Depends on C2 (registry access) and C7 (arches).
- `init` over USB, the own uid and service unit, the key, an operator-signed licence record (the ADR-105 fields plus `mesh_id`);
- the checkout, renew and identity endpoints, registry fetch, durable `seq`, the limits and override, the clock floor;
- a listener on USB and the tailnet only;
- a W4 decision on TLS.

`weft-licence`'s own tests cover `seed_bound_elsewhere` (this replaces the old test 9, which only exercised a stub). Acceptance:
- the armhf build size is recorded;
- `weftos-cog-sources` is confirmed to build for `armv7-unknown-linux-gnueabihf`;
- a cog's uid cannot read the key (tested on Linux);
- **owner-run on the real Seed**: check out `fall-detect` for aarch64, a second node runs it from peers, the `weft-licence` log shows one byte transfer, and switching the Seed off for longer than the TTL stops sharing.

**Phase 3. Lifecycle.**
- the start-time grant check in `weft-cog-host` and the workload host;
- `weaver cog checkout list|release|renew|reset-floor`;
- the chain events `cog.checkout.request|granted|renewed|refused|lapsed`;
- `weaver doctor` checks for binding health and the expiry horizon.

Placement still uses operator re-signing. Acceptance: after a lapse, a restart is refused.

**Phase 4. Needs Cognitum.** A Cognitum-signed entitlement and release statement that members can verify. Then grant-backed placement eligibility (replacing operator re-signing inside the mesh), licensed download, a device-key cross-certificate of the grant key, a Cognitum withdrawal feed, and multi-Seed meshes if wanted.

## Open questions

**For Cognitum (C).** Anything not verifiable locally is marked as an assumption above.

- **C1.** Is there an entitlement or release-statement API, a signed artifact and an offline grace? (None today; ADR-105 open question 4.)
- **C2.** Will registry binaries and entries stay public? If they move behind authentication, how does a local process on the Seed fetch without holding the token? This affects section 4 step 7 as well.
- **C3.** Does one Seed licence per mesh (scoped to admitted machines) fit Cognitum's terms? Are there size limits, and redistribution terms for the binaries?
- **C4.** Is there an HTTP device-key signing call for local processes? What is the key format of `/api/v1/identity`?
- **C5.** Must a lapse hard-stop running instances?
- **C6.** Will Cognitum provide a withdrawn-version feed?
- **C7.** Does the registry carry aarch64 and x86_64 builds for every cog, beside armhf?
- **C8.** Can the agent install without starting the cog?

**For us (W).**

- **W1.** Decided in section 3: the genesis pin combined with the cluster-owner key.
- **W2.** Should there be a standby steward?
- **W3.** Should unused checkouts be released automatically?
- **W4.** TLS in `weft-licence`, or the tailnet only (decided in phase 2)?
