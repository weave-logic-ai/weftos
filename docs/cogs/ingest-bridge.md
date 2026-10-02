# Cog ingest bridge

Code: `crates/clawft-kernel/src/cog_ingest/`. Card: mesh-placement-10.
Decision: ADR-100, "Decision 5 resolved (2026-10-02)".

A placed cog posts feature vectors to `POST /api/v1/store/ingest` with its
`COGNITUM_COG_TOKEN`. The released cog binaries always post to
`127.0.0.1:80`; the bridge is what answers there (or what a container relay
forwards to). The bridge validates the request, forwards the batch to the
store owner as a signed mesh message, and answers the cog.

```
ESP32 --UDP--> cog (native or container) --HTTP + token--> bridge (node A)
                                                              |
                          signed MeshIpcEnvelope (Ed25519)    v
                                                   store owner (node A or B)
                                                   project store (or controller's)
```

## Where the vectors go

Ingested vectors belong to the project that placed the cog. When the cog is
placed the node registers an `InstanceBinding { instance_id, project_id,
controller_node }` together with the instance's token
(`TokenRegistry::register`), and revokes it at unload (`revoke`).

`StoreRouter` picks the owner from the binding:

| Binding | Store that takes the batch |
|---|---|
| `project_id = Some(p)` | the store owned by project `p`'s kernel; if `p` has no route the batch is **refused (502)**, never redirected to the controller |
| `project_id = None` | the placing controller's store (`controller_node`) |

The owner may be this node (`LocalForwarder`) or another (`MeshForwarder`).
`PlacementRecord` does not carry a project id yet, so the caller that holds
the project id passes it to `InstanceBinding::from_record`.

## Request contract

`POST /api/v1/store/ingest`, `Content-Length` body, JSON:

```json
{"vectors": [[1, [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]]], "dedup": true}
```

| Rule | Limit | Status |
|---|---|---|
| Body size | 64 KiB (checked from `Content-Length` before reading) | 413 |
| Vectors per batch | 1 to 256 | 400 / 413 |
| Vector | `[id, [8 numbers]]`, id a non-negative integer | 400 |
| Values | finite f32 (NaN, infinity, overflow rejected) | 400 |
| Keys | only `vectors` and `dedup` | 400 |
| Token | `Authorization: Bearer <token>` or `X-API-Key: <token>` | 401 unknown / revoked, 403 other instance's token on an instance-scoped listener |
| Rate | per instance, default 20 requests and 2048 vectors per second | 429 with `Retry-After` |
| Transport | one request per connection, no chunked bodies, 8 KiB head, 5 s read timeout, 64 connections per listener | 400 / 408 |

Nothing is read from the body of an unauthenticated request. Owner-side
failure detail is logged, not returned.

`dedup: true` skips a vector when the store already holds the same id or a
bit-identical value. `dedup: false` upserts by id. The store applies it, so
dedup holds across bridges and nodes.

## Forwarding to a remote owner

`MeshForwarder` sends a `ForwardRequest` signed by the bridge node's key
(domain `weftos.cog_ingest.forward.v1`) in a `MeshIpcEnvelope` addressed to
`ServiceMethod { cog-store, store.ingest }`. The owner's `StoreOwnerService`
checks size, signature, structure, that `requester` is the signing key's
node, addressee, time window (30 s skew, 5 min maximum lifetime), the
`ForwardPolicy` (which keys may forward, optionally per project), nonce
freshness, then writes. The answer is signed by the owner and bound to the
request nonce; the forwarder pins the owner's key and rejects any other
signer. A replayed request is refused, never written twice.

Transport is the same as `workload.ctl`: in-process streams for tests
(`OwnerConnector::register_local`), mesh TCP with optional Noise XX
otherwise (`serve_listener`).

## stdout is evidence, never data

Whatever a cog prints is captured by the runtime adapter as `RunEvidence`
for logs and the chain. It is never parsed into a store write and never
used as control input. The only path from a cog to a store is this bridge.

## Network policy per runtime

The contract (ADR-100 section 4) is: sensor feed in, bridge out, nothing
else. What is enforced today and what is not:

| Runtime | Enforced by this card | Operator configuration | Deferred |
|---|---|---|---|
| native | The bridge binds loopback only (`bind` refuses anything else for the shared scope). The token is per instance and is checked on every request. The adapter declares `NetworkPolicy::Egress` to the gate, so a permit rule is needed to place a native cog. | Run the cog as an unprivileged user. | Blocking the cog's other egress (nftables, landlock net rules). A native cog can still open other sockets. |
| container (docker, podman, apple) | The in-container relay (`container_relay`) carries the cog's `127.0.0.1:80` to the bridge. Use one instance-scoped listener per container (`BridgeScope::Instance`), bound to the address the container sees (the VM or bridge gateway) with `BridgeConfig::allow_non_loopback`; the token still applies, and another instance's token on it is 403. `network=none` is refused when an upstream is set. | Put the container on a dedicated network that routes only to the bridge address (for docker, an `--internal` network with the gateway forwarded to the bridge). The adapter cannot verify this and declares `egress` to the gate for any network but `none`. | Verifying the network from the adapter. |
| Seed (`remote.api`) | Not this bridge: the Seed ingests into its own store. | | |

A cog that opens a non-bridge socket is therefore stopped by the container
network the operator configured, not by this code. That gap is the reason
the gate sees `egress`.

## Optional UDP forwarder

`UdpForwarder` relays the ESP32 feed from the node's LAN to a container's
feed port, datagram for datagram: at most 512 bytes, empty datagrams and
datagrams from any source other than the configured sensor address are
dropped and counted. Nothing is parsed.

## Wiring a node

```rust
let registry = Arc::new(TokenRegistry::new());
let router = StaticRouter::new()
    .with_project(project_id, owner_forwarder)       // MeshForwarder or LocalForwarder
    .with_controller(controller_node, controller_forwarder);
let bridge = IngestBridge::new(registry.clone(), Arc::new(router),
                               RateBudget::default(), BridgeConfig::default());
let contract = HostContract::default_feed();          // fresh token
registry.register(InstanceBinding::new(instance_id, Some(project_id), controller_node), &contract)?;
let listener = bridge.bind("127.0.0.1:80".parse()?, BridgeScope::Any).await?;
// ... place the cog with `contract`; on unload: registry.revoke(instance_id)
```

The owner node runs a `StoreOwnerService` over a `StoreDirectory` (its
project stores and the controller fallback) with a `KeyPolicy` naming the
bridge keys it accepts, and serves it with `serve_listener`. `IngestStore`
is implemented by `MemoryIngestStore` and, with the `ecc` feature, by
`VectorBackendStore` over the kernel's HNSW / DiskANN backends.

## Acceptance runs use a replayed feed

Acceptance uses a replayed ESP32 feed (recorded or synthetic packets:
ADR-069 magic `0xC5110003`, 48 bytes, 8 little-endian f32 at offset 16).
`scripts/cogs/harness.py` produces the reference feed. A live ESP32 feed is
optional: point the sensor at the node (or the UDP forwarder) and the path
is the same. Document the live run next to its chain export when one is
made.

## Tests

`cargo test -p clawft-kernel --lib cog_ingest`:

- request validation (shape, dimensions, non-finite, batch and body caps);
- fake feed, cog stub, bridge, store: vectors land and dedup holds;
- unknown, revoked and other-instance tokens, malformed and oversize
  requests, per-instance rate and vector budgets, bind discipline;
- project routing: project store, controller fallback, no route refused;
- two nodes in process: a cog's bridge on node A, vectors queried on node
  B's store; forged, replayed, expired, mis-addressed, unauthorised and
  tampered forwards refused; a wrong owner key rejected;
- UDP forwarder; HNSW-backed store.
