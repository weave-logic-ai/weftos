# Operating cogs: pack, sign, pair, place, revoke

For the person who takes a cog from a source to a running, governed workload on a real node. Design: [ADR-099](../adr/adr-099-governed-workload-placement.md) (placement, trust) and [ADR-100](../adr/adr-100-cog-workload-kind.md) (the cog kind). Where cogs come from: [cog-sources.md](cog-sources.md). Hardware lane: [test-pi.md](test-pi.md).

Two paths exist and they are different:

- **Appliance path**: `weaver cog install` puts a verified binary into a cog-host root (`weft-cog-host` supervises it). Verified, recorded, not a governed placement.
- **Governed path**: pack the cog into a signed `cogpkg`, then `weaver workload place` it onto a paired node. Every step is gated and chained.

Pick the catalog first: `weaver workload catalog --kind cog` shows run mode, resources, secrets, source and the default placement policy for each cog. The v1 scope is the 93 cogs in the clean `--once` group; the five persistent-listener health cogs need `--mode interval`; the nine `needs-*` cogs need seed peers, assets or other setup and are not v1.

## Pack

```sh
weaver workload pack --cog-dir <cogs checkout>/src/cogs/anomaly-detect \
    --bin aarch64=cog-anomaly-detect-aarch64 --bin armv7=cog-anomaly-detect-armv7 \
    --source-commit 8970f99 --out ./pkg/anomaly-detect
```

The package directory holds the unmodified `cog.toml`, one binary per arch, optional attestations, and `cogpkg.json` pinning each file by BLAKE3 and size. `--cognitum-record <file>` carries a Cognitum release record as an attestation; `--release-url` records where upstream binaries came from (provenance, never trust). An upstream binary is usable only after you hash and sign it here.

**Sharing over the mesh.** A package is seeded, advertised and served to other nodes only if it was packed with `--redistributable` (a signed manifest field, default off). Pass it for your own cogs that you want shared. Never pass it for a binary whose `provenance.json` says `trust = "cognitum-sha256"` (a licensed Cognitum install); pack such a binary with `--release-url <the Cognitum URL>` or `--cognitum-record <file>`, which marks the package Cognitum-origin so the swarm never redistributes it. `pack` does not read `provenance.json` yet, so this is on you. Nodes built before this change reject manifests packed with `--redistributable`; upgrade every node before using it. The full rules: [swarm throughput, "Safety rules the swarm enforces"](../research/mesh-placement/swarm-throughput.md#safety-rules-the-swarm-enforces).

## Sign

```sh
weaver workload keygen --out ./operator.seed --key-id acme-operator   # 64-hex seed, mode 0600, never overwrites
weaver workload pack ... --key ./operator.seed --out ./pkg/anomaly-detect
weaver workload verify ./pkg/anomaly-detect --trust ./workload-trust.json
```

`workload-trust.json` (`weftos.workload-trust.v1`) lists the operator-pinned signer keys the node accepts, plus optional pinned Cognitum release keys. Without it only the compiled-in WeftOS signer set is trusted. Verify exits with a distinct status per failure: missing signature, untrusted signer, bad signature, tampered file, bad manifest. Details and the signer provisioning steps: the key management section of [cog-sources.md](cog-sources.md).

Verifying a package trusted only through a Cognitum record (`--cognitum-release`) needs more than the record: the record's `sourceCommit` must equal the package's source commit and you must pin the `cog.toml` hash you reviewed (`--cog-toml-pin <blake3>`), because a record does not cover `cog.toml`. Sign the package yourself and none of that is needed. With `--cognitum-release` on, an attached record is checked even when your signature is present.

## Pair

A node that can run workloads is described to the controller in the daemon's runtime directory (ADR-099 section 8, `workload_place_policy.rs`):

- `workload-peers.json`: `[{"addr": "host:port", "tier": "paired", "key": "<64 hex node key>"}]`. A named peer is only `discovered` until you give it a tier. With a `key` the tier belongs to that node key; without one it goes to the key first seen at the address, and a different key answering later never inherits it.
- `workload-permits.json`: the permit rules for gated actions (default deny: missing file means no permits).
- `workload-trust.json`: the package signers this node accepts.
- On the target node, `workload-host.json` makes it serve `workload-host` (Noise XX to the named controller).

Tiers gate what may run: `discovered` nothing by default, `paired` operator-signed workloads (native cog placement needs this), `pinned` workloads that carry secrets. A Cognitum Seed is addressed as an operator-assigned node id in `workload-seeds.json`, with pinned store cogs (`id`, `version`) and its token under `secrets/workload.seed/<node_id>.token`. Cogs from a Cognitum source reach a Seed only through that store path with the cog pinned; a Seed's store does not list `anomaly-detect`.

## Place

```sh
weaver workload explain ./pkg/anomaly-detect            # candidates, scores, failed constraints; dispatches nothing
weaver workload place   ./pkg/anomaly-detect --mode interval --interval 10
weaver workload place   ./pkg/anomaly-detect --pin <node-id>       # that node or fail naming the constraint
weaver workload place-seed seed-kitchen fall-detect@1.0.0 --sha256 <hex>
weaver workload status [<instance>]
weaver workload logs <instance>
weaver workload stop <instance>
weaver workload unload <instance>
```

Placement picks real ARM hardware over a Mac container over emulation, prefers the node on the sensor's LAN, and refuses emulation unless you pass `--allow-emulated`. The target re-checks the package at admission (a package whose `aarch64` binary is really x86-64 is refused there and retried on the next candidate). An unsigned package is refused. Each place, load, start, stop, unload and refusal is a chain event.

## Revoke

Revocation is by package id, signer key or artifact hash (`RevocationKind::{Package, SignerKey, ArtifactHash}`), persisted in `revoked_subjects.json` beside the host ban list and recorded as a `workload.revoke` chain event by `revoke_and_record`. A revoked package, or a package signed by a revoked key, is refused at verify and place time. Revoking a signer key is how you respond to a leaked package key; also remove its pin from `workload-trust.json` (or the compiled set in a release). For a private-repo key, revoking is on the consumer side: remove or replace the key in each project's source (`weaver cog source remove` then `add --key <new>`).

**Built:** signed mesh-wide revocation notices. The daemon runs `RevocationExchange`, so a notice signed by a pinned operator or WeftOS key spreads to every node, which stops serving the revoked artifacts and removes their bytes. **Not built:** an operator CLI verb or RPC to issue a notice (there is no revoke verb under the workload group; today a revocation is a kernel-side call), and forced unload of running instances. A running instance keeps going until you stop it with `weaver workload stop` / `unload` or it restarts.
