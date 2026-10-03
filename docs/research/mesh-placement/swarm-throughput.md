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

The 0.02 s above the ideal 3.00 s / 1.50 s / 1.00 s is the signed descriptor
exchange and the last block's pacing. Work follows speed: a worker asks for its
next piece as soon as the last one lands, so a faster peer takes more pieces.
`locality_and_measured_speed_choose_the_sources` checks that: after fetching
from an 8 MiB/s and a 1 MiB/s peer, the node's measured figures differ by more
than 4x and the faster one is dialed first next time.

Reproduce:

```
NEXTEST_SUCCESS_OUTPUT=immediate scripts/build.sh test clawft-kernel \
    --test-filter three_seeders
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

## Open items

- `net.lan` and `store.artifact.*` are not in `config/capabilities.toml` yet.
  Unknown ids are accepted and matched, so nothing depends on it, but the
  vocabulary file is governed and pinned, so the entries need to go in through
  that path.
- Nothing in the daemon owns an `ArtifactExchange` yet: the swarm is a library
  with a test suite, not wired into `weaver`. Merging `ArtifactCache::advertise`
  into the facts the daemon publishes is one call once it is.
- Partial copies are not served, and there is no request window.
