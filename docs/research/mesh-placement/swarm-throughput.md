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

- **Licence-gated cogs are not redistributed.** A package with Cognitum
  provenance (a `cognitum.*` release-record attestation, or a `cognitum`
  release URL) is marked not redistributable when its manifest is authorized.
  This node may hold and run it, but never seeds it, advertises it
  (`store.artifact.*`, `model.present`) or serves it to a peer. No licence or
  same-operator check exists to relax that, so there is no exception.
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
- **Eviction cannot break a running package.** `swarm_fetch_package` pins the
  manifest and every file before fetching them, and leaves them pinned until
  `ArtifactCache::unpin_package`. Bytes being downloaded count against the
  budget. `forget` only removes blobs this exchange created and nobody else
  has stored since (store reference count 1), and does not forget a pinned
  entry. `apply_revocations` does evict pinned entries: a revoked package has
  to stop running as well as stop seeding.
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
  checked strictly. A node accepts at most a burst of 10 notices, then 2 per
  second, and a notice already on the list is not swept again.

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
