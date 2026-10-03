# Swarm artifact distribution: throughput measurement

Card mesh-placement-25 (ADR-099 section 6). This page records what the
four-node in-process swarm suite measures, and what it does not.

## What was measured

`mesh_swarm_tests::three_seeders_serve_one_fetch_faster_than_one` fetches a
12 MiB signed-package binary (twelve 1 MiB pieces, 256 KiB blocks) from 1, 2,
3 and 4 seeders, each of which holds the whole package. Every seeder's upload
link is limited to **4 MiB/s** by an injected per-peer rate limit on the
serving side (`piece` frames are paced; nothing else is). The fetching node
has no cap of its own. The test runs under paused tokio time, so the elapsed
times below are exact and repeat on every run.

| Seeders | Elapsed | Aggregate rate | Speedup vs one |
|---|---|---|---|
| 1 | 3.02 s | 4.0 MiB/s | 1.0x |
| 2 | 1.51 s | 7.9 MiB/s | 2.0x |
| 3 | 1.01 s | 11.9 MiB/s | 3.0x |
| 4 | 0.76 s | 15.9 MiB/s | 4.0x |

The 0.02 s above the ideal 3.00 s / 1.50 s / 1.00 s is the descriptor
exchange and the last block's pacing. Work follows speed: a worker asks for its
next piece as soon as the last one lands, so a faster peer takes more pieces.
`locality_and_measured_speed_choose_the_sources` checks that: after fetching
from an 8 MiB/s and a 1 MiB/s peer, the node's measured figures differ by more
than 4x and the faster one is dialed first next time.

Reproduce:

```
NEXTEST_SUCCESS_OUTPUT=immediate scripts/build.sh test clawft-kernel \
    --filter three_seeders
```

## What this does not show

- **It is a model, not a network.** The link speeds are injected. Real links
  add latency, loss, congestion and CPU limits (a Pi 5 verifying BLAKE3 at
  line rate is slower than the 4 MiB/s modelled here would suggest on a LAN
  and faster than it on Wi-Fi). The numbers show that the scheduler uses
  several sources in parallel without waste, not what a deployment will get.
- **Seeders hold the whole artifact.** Partially downloaded copies are not
  served yet (only verified artifacts are), so a piece is never rarer than
  "all seeders have it" in these runs. Rarest-first and the in-flight rules
  are covered by the picker's unit tests with partial holdings.
- **One request in flight per peer.** Throughput per peer is bounded by one
  piece round trip at a time. On a high-latency link, a request window would
  help; the protocol allows it (`request` takes several indexes).

## How the pieces fit

- **Fetch** (`mesh_swarm_fetch`): `ArtifactExchange::swarm_fetch` dials up to
  `max_sources` peers ranked by `net.lan` locality, then measured speed; one
  worker per peer takes the rarest free piece that peer holds
  (`mesh_swarm_picker`). A lost peer's pieces go back to the pool and the peer
  is replaced; a corrupt piece bans its sender (`artifact.piece_rejected`,
  `artifact.peer_ban`) and is fetched elsewhere.
- **Seeding** (`mesh_swarm_governance`): an artifact is served once it is
  verified here and a verified signed manifest lists it; the first time that
  holds, `artifact.seed` is chained. A finished fetch is therefore a seeder
  with no extra step.
- **Who has** (`mesh_swarm_lookup`): peers advertise held artifacts in their
  signed facts (`store.artifact.<16 hex>`, `model.present`); `who_has` asks
  peers directly with `meta_request`. No tracker.
- **Cache** (`mesh_swarm_cache`): byte budget, LRU eviction of unpinned
  entries, pins that the policy never evicts (`pin_content` pins before an
  artifact arrives). Eviction chains `artifact.evict`.
- **Bandwidth** (`mesh_swarm_rate`): per-node upload and download pacing
  (`ExchangeConfig::upload_bytes_per_sec`, `download_bytes_per_sec`).
- **Revocation** (`mesh_swarm_governance`, `mesh_swarm_revoke`): a revoked
  package id, signer key or artifact hash stops seeding at once, and
  `apply_revocations` drops the grant and the bytes (`artifact.revoke`,
  `artifact.evict`). A signed notice from a pinned operator or WeftOS key
  carries it across the mesh on `mesh.artifact.revoke`, once per node.

## Safety rules the swarm enforces

- **Sharing is opt-in and fails closed.** A package is redistributable only
  when its signed manifest says `redistributable = true` (a field of the cog
  manifest body, default false, part of the signed statement;
  `weaver workload pack --redistributable` sets it). Even then a package with
  Cognitum provenance (a `cognitum.*` attestation or a `cognitum` release URL)
  is not. A package that is not redistributable is held and run by this node
  but never seeded, advertised (`store.artifact.*`, `model.present`) or
  served; an operator re-pack of a Cognitum binary that drops the attestation
  is still not shared unless its signer opts in. One non-redistributable
  package listing a content hash vetoes serving that hash for every package.
  **Our own weftos cogs therefore need `redistributable = true` in their
  manifests to be seeded.** Hashes recorded in a cog-sources `provenance.json`
  with a Cognitum trust are not consulted yet, by the swarm or by
  `weaver workload pack`: follow-up.
- **Sizes are checked before any piece is requested.** A descriptor with a
  piece size over `max_piece_size` (default 16 MiB), a total over
  `max_artifact_bytes` (default 64 GiB), or one that differs from the exact
  size in the signed manifest is refused. At most one piece is buffered per
  source, so memory is bounded by `max_sources x max_piece_size`. At most 64
  partial downloads are held. The manifest pins sizes and content hashes, not
  the piece-list root, so the root cannot be required for package files.
  `DEFAULT_PIECE_SIZE` is now 16 MiB, matching the cap (it was 64 MiB).
- **A liar cannot block a content-hash fetch.** When pieces that a peer's
  descriptor describes do not assemble to the content hash, the descriptor and
  the pieces that fetch wrote are discarded, the peers that vouched for it
  are banned and the fetch continues with the other candidates (three
  attempts). Candidates rank by trust tier first; a `net.lan` id is
  self-asserted and only orders peers of the same tier.
- **One policy point, fixed at construction.** Seeding, advertising and
  serving all ask `RedistributionPolicy::allows(hash, grants, audience)`, where
  the audience is `Serve(peer)` (the requesting peer: node id and whether
  admission verified it), `Advertise` or `Seed`. The policy is an
  `ExchangeConfig::redistribution` field (default `ManifestPolicy`); there is
  no setter, and the daemon sets `ManifestPolicy` explicitly. Each grant
  records an origin (`Cognitum { cog_id, version }`, `OptIn`, `NotFlagged`).
  `ManifestPolicy` allows a hash only when every grant is `OptIn`, for every
  audience. Whatever a policy says, content that is not `OptIn` is never put
  in broadcast facts (`held_capabilities`): it can be found only with
  `who_has`, where the serving side sees who asks. `serve` and `serve_frame`
  take a claimed id (an unverified peer); `serve_as` and `serve_frame_as`
  take a `ServePeer` whose verified flag the caller got from admission. A
  licence-proxy policy (for example a Seed-signed, mesh-scoped grant) plugs in
  there.
- **Eviction cannot break a running package.** `swarm_fetch_package` pins the
  manifest and every file before fetching them, and leaves them pinned until
  `ArtifactCache::unpin_package`. Bytes being downloaded count against the
  budget. `forget` only removes blobs this exchange created and nobody else
  has stored since (store reference count 1), and does not forget a pinned
  entry. The daemon opens its store with `ArtifactStore::open_file`, so blobs
  left by installed workloads from earlier runs are indexed and the exchange
  never takes ownership of them (with `new_file` it would have re-stored the
  same bytes as its own and could evict them). `apply_revocations` does evict
  pinned entries: the bytes are removed and serving is blocked. The sweep
  itself does not touch a workload that is already running; the host does,
  right after (see "Revocation: scope and limits", forced unload).
- **Unverified facts cannot crowd the cache.** One connection holds at most 4
  `Discovered` entries and the cache at most 512 in all; past that the
  connection holding the most loses its oldest entry first, and `Paired` and
  `Pinned` entries are never touched. Frames are budgeted on their whole size
  before anything is parsed. Limitation: a connection that reconnects gets a
  fresh id and quota (no remote address is available to charge), so a patient
  attacker can still churn the `Discovered` class; it cannot displace verified
  peers or an unverified peer that holds fewer entries than the attacker's
  connections.
- **Grants are per package.** Two packages listing the same content each keep
  a grant; revoking one leaves the content allowed by the other.

## Revocation: scope and limits

- One pinned operator or WeftOS key can revoke any package id, signer key or
  artifact hash on every node that pins it, including the WeftOS signer
  itself. Pin few operator keys.
- Revocation is monotonic mesh-wide. Nothing removes an entry, and a notice is
  re-flooded to every peer, so a local `unrevoke_subject` is undone by the next
  re-flood. To lift a revocation, clear the list on every node or ship the
  package under a new key.
- A signer key that is itself revoked cannot issue notices. Signatures are
  checked strictly. Cheap checks come first and cost nothing (size, pinned
  operator/WeftOS key, revoked signer, notice already applied); a notice that
  passes spends a token from its own connection's bucket (burst 10, then 2 per
  second) before the signature verify, so junk on one connection cannot starve
  notices on another, and the operator's own `issue` is exempt. A known notice
  is not swept again.
- **Forced unload.** Every applied revocation (the operator's
  `weaver workload revoke`, a signed notice from a peer) is followed by
  `WorkloadHostService::enforce_revocations` on that node: each hosted
  instance whose package id, signer key or artifact hashes are now revoked is
  stopped and unloaded, its ingest token is dropped, and the controller's
  record of it is forgotten. The daemon also runs it once at start-up and
  every 60 s, so a teardown that failed is retried. A `place` that a revocation
  races (it lands after the gate looked, before the instance is listed) is
  caught and torn down, and a start refused because of a revocation is rolled
  back by the revocation itself. The sweep locks the instance map only to pick
  targets. The teardown is not gated: the applied
  revocation is the authority, so it needs no stop or unload permit. Every
  step is chained (`workload.stop`, `workload.unload` with
  `forced_by_revocation` naming the subject; a step that fails is chained as
  `workload.refuse` and the instance is retried by the next sweep).
- **Chained, always.** The list itself chains every revocation
  (`workload.revoke`: subject, reason, `revoked_by` = `operator` or
  `mesh:<signer prefix>`) and every lifted one (`workload.unrevoke`),
  whichever caller applied it, and does so before the disk write is
  reported: a failed write leaves the entry in force in memory and the event
  on the chain with `persisted: false`.
- **No bypass.** The workload gate checks the list on every place, load and
  start and for install and migrate; a request that names no package, signer
  key or artifact hash is refused rather than trusted on its `package_trust`
  claim. Stopping or unloading is never blocked by a revocation (a revoked
  package must stay stoppable). The list is a required argument of
  `WorkloadGate::new` / `with_rules`; the one way round it is
  `WorkloadGate::exempt(.., why)`, and
  `crates/clawft-weave/tests/revocation_population.rs` fails a build that
  uses it outside its short allowlist (the project supervisor). `workload.revoke`
  is served before the placement guards and works on the kernel's list alone
  when the plane cannot be built or governance has changed.
- Not covered: instances on a Cognitum Seed (the device's own store; a
  revoked store cog is refused at the next place or start, not stopped), and
  a remote `workload-host` that missed the notice (it stops its own instances
  when it receives one; the operator's node can only flood to the peers it is
  connected to). A notice is issued only when this node's key is a pinned
  operator key in `workload-trust.json`; otherwise the verb says so and the
  revocation holds on this node alone.

## Live facts

The 60 s tick refreshes free memory only. Capability busy/free states are not
produced by any probe; a delta carries them only when the workload host marks
one.

## Daemon wiring

The daemon's placement exchange (`workload_place_rpc::build`) now gets the
kernel's `RevocationList` and starts a `RevocationExchange` on the mesh
runtime (without a mesh it still gets the list), so a revoked package stops
seeding there and signed notices from peers are applied. It does not own an
`ArtifactCache` yet, so nothing is advertised in the daemon's facts and no
byte budget applies; `ArtifactCache::advertise` is one call once it does.

## Open items

- `net.lan` and `store.artifact.*` are not in `config/capabilities.toml` yet.
  Unknown ids are accepted and matched, so nothing depends on it, but the
  vocabulary file is governed and pinned, so the entries need to go in through
  that path.
- Partial copies are not served, and there is no request window.
- No licence field exists in the manifest on this branch; "Cognitum
  provenance" is the only signal for non-redistributable packages.
