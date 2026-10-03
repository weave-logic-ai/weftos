# How a project gets cogs: sources, licences and private repos

Design record: [ADR-105](../adr/adr-105-cog-sources.md). Commands: the `weaver cog` group, `weaver workload catalog --kind cog`, and `weft-cog-repo` for authoring. This page is the how-to. What is built and what is only designed is listed at the end.

A WeftOS project gets cogs from a **list of sources**. Three kinds exist:

| Kind | What it is | Installable when |
|---|---|---|
| `weftos` | the WeftOS / WeaveLogic signed registry (COG-008 format) | always; the binary must be signed by a pinned key |
| `cognitum` | Cognitum's `app-registry.json` (107 cogs on the Seed store) | the project holds a licence for that cog; the binary must match the registry sha256 |
| `private` | your own registry in the same COG-008 format, signed with your key | always; the binary must be signed by your pinned key |

Cognitum cogs are always **listable**. The licence only gates **install**.

## 1. Configure sources

Sources live in a plain file: `<project root>/.weftos/cog-sources.toml` for the project, and `~/.weftos/cog-sources.toml` for your defaults across projects. The effective list is your defaults overlaid with the project file; a project entry with the same `name` replaces the default one. The project root is the nearest directory above you that holds `.weftos/project.toml` (`weft project init` creates it).

```sh
# inside the project
weaver cog source add weftos  --kind weftos   --url https://example.org/weftos-cogs/        # a repo dir or registry.json URL
weaver cog source add cognitum --kind cognitum                                              # defaults to Cognitum's public registry
weaver cog source add acme-private --kind private --url https://cogs.acme.example/ \
        --key <64-hex public key> --priority 50

weaver cog source list
weaver cog source disable cognitum        # kept in the file, skipped by search and install
weaver cog source enable cognitum
weaver cog source remove acme-private
weaver cog source add weftos --kind weftos --url ... --user      # --user edits ~/.weftos/cog-sources.toml
```

The file is hand-editable. Every field:

```toml
[[cog_source]]
name        = "acme-private"        # namespace; [a-z0-9][a-z0-9_-]{0,31}; no ':'
kind        = "private"             # weftos | cognitum | private
url         = "https://cogs.acme.example/"   # repo dir/URL holding registry.json, or the registry.json / app-registry.json itself
pinned_keys = ["<64-hex public key>"]   # Ed25519 public keys, 64 hex. Required for private; refused on weftos
priority    = 50                    # default 0; higher wins when a bare id is in several sources
enabled     = true                  # default true
```

What each kind trusts:

- `weftos`: the pinned WeaveLogic release key (`WEAVELOGIC_PUBKEY_HEX`) and the compiled-in WeftOS package signers (`WEFTOS_PINNED_SIGNERS`, empty today). **Nothing from a config file is added**: a `weftos` source with `pinned_keys` is refused, because a project file could otherwise make its own key count as WeaveLogic. Put your own key in a `private` source. Only a cog verified by one of those two compiled-in anchors is recorded as WeaveLogic on install; anything else is recorded as a local signed install.
- `private`: **only** the keys in `pinned_keys`. The WeaveLogic key is never implicitly trusted by a private source.
- `cognitum`: no signature (Cognitum's registry binaries are unsigned), so trust is the registry sha256 alone. For that reason a `cognitum` source must use `https://` (a local path or `http://` needs the development-only `allow_insecure = true`, which is honoured only in your user file `~/.weftos/cog-sources.toml`: a project file's `allow_insecure` is ignored with a warning, and every use that is in effect is warned about), and a registry that points its binaries at a non-https location is refused (`insecure_transport`). The HTTP client also refuses a redirect from https to http. `pinned_keys` is accepted but unused today.

A project file can redefine a source you set in `~/.weftos/cog-sources.toml`. Because a cloned repository ships its own `.weftos/cog-sources.toml`, `weaver cog` prints a warning when a project entry replaces a user entry and changes its url or keys, refuses a changed `kind`, and warns on plain `http://` sources. `weaver cog install` prints the resolved `source:id` and where the source was defined before it fetches anything, and `--enable` on a bare id that resolved to a project-defined source needs the namespaced id or `--confirm-project-source`.

The file holds URLs, public keys and licence declarations. Never put a private key or a token in it.

## 2. Find and install cogs

```sh
weaver cog search                 # every cog of every enabled source
weaver cog search fall --source cognitum
weaver cog info acme-private:acme-gauge
weaver cog install weftos:fall-detect --enable --arg=--interval --arg=1
```

`install` resolves the id, fetches the binary, verifies it, and only then writes it into the cog-host root (`$WEFTOS_COG_ROOT`, else `~/.weftos/cogs`; `--root` overrides). Next to the binary it writes `provenance.json`: source, signer key id, sha256, licence account, and whether the cog is eligible for governed placement. A refusal prints a stable code (`cog_unlicensed`, `licence_expired`, `verify_failed`, `unsigned`, `ambiguous`, `unknown_source`, `source_disabled`, `cog_not_found`, `no_artifact`, `no_pinned_hash`, `fetch_failed`).

### Resolution rules

Ids are namespaced `source:cog-id`: `weftos:fall-detect`, `cognitum:fall-detect`, `acme-private:fall-detect`.

- A namespaced id goes to that source only.
- A bare id (`fall-detect`) is looked up in every enabled source. One match: that one. Several: the highest `priority` wins. A tie at the top is an `ambiguous` error that lists the namespaced ids to use; there is never a silent pick.
- Example: `fall-detect` is in `weftos`, `cognitum` and `acme-private`, all priority 0: `weaver cog install fall-detect` is refused, `weaver cog install acme-private:fall-detect` works. Give `acme-private` priority 50 and the bare id resolves to it.

## 3. Licensing Cognitum cogs

Cognitum's cogs are commercial: Cognitum takes 30% per cog under a contract. The code does **not** enforce payment (ADR-100 section 6.4). What it does is keep a declared, per-project entitlement and refuse an install the project has not declared.

```sh
weaver cog licence add --cog fall-detect --cog baby-cry --account acme --expires 2027-01-31
weaver cog licence add --all --account acme                  # every cog of the source
weaver cog licence list
weaver cog licence remove cognitum
```

```toml
[[cog_licence]]
source  = "cognitum"
cogs    = ["fall-detect", "baby-cry"]    # or  cogs = "all"
account = "acme"
expires = "2027-01-31"                    # YYYY-MM-DD, valid through that UTC day; or RFC 3339; omit for none
```

- No covering licence: `cog_unlicensed`, and **nothing is downloaded**.
- Licence expired: `licence_expired`. A valid second licence for the same cog still allows it.
- This is a declaration kept in the project, not proof of purchase. Treat it like a record your own team keeps honest. If you need proof, keep the Cognitum contract and invoices; a signed entitlement is an open question in ADR-105.
- A licensed install is `placement_eligible = false`. Cognitum's binaries are not signed by us, and ADR-100 section 6.3 says an upstream binary is used in governed placement only after an operator hashes and signs it. See the operator guide, "Pack".
- A licensed Cognitum cog is never shared over the mesh. When you pack one for placement, do not pass `--redistributable`, and pass `--release-url` or `--cognitum-record` so the package is marked Cognitum-origin. See the operator guide, "Pack" (Sharing over the mesh).

### In a mesh with a bound Seed (ADR-106)

When a Cognitum Seed is bound to the mesh (`weaver workload node bind`), the per-project record above is not what lets a cog run on a member. The Seed's `weft-licence` holds the licence, one steward node talks to it, and the mesh shares the bytes. See [weft-licence.md](weft-licence.md) for the Seed side.

```sh
weaver cog checkout <cog>@<version> --arch aarch64       # e.g. fall-detect@1.2.0; asks the steward (or its own relay)
weaver cog checkout approve fall-detect@1.2.0 --operator-key ~/.config/weftos/operator.seed   # shows the hashes and a content key
weaver cog checkout approve fall-detect@1.2.0 --operator-key ~/.config/weftos/operator.seed --confirm <content-key>
weaver cog checkout status --explain                      # grants, approvals, the run gate per artifact
weaver cog checkout list                                  # held grants, expiry, approval per artifact (read-only)
weaver cog checkout renew fall-detect@1.2.0               # on the steward: renew now (the Seed renews every checkout)
weaver cog checkout release fall-detect@1.2.0             # on the steward: end the checkout; the withdrawal floods
weaver cog checkout reset-floor                           # same as weaver workload node reset-floor
weaver cog checkout approve --reapprove-orphaned --operator-key <file> --confirm <digest>   # after a mesh_nonce change
```

- **Checkout** gets a signed grant from the Seed through the steward. The grant floods to every admitted node, and members fetch the bytes from peers. The Seed sends the bytes once.
- **A grant alone never runs anything.** Before a Cognitum-origin cog is installed or started, the node needs a valid grant and an operator hash approval whose sha256 set covers the binary. The hashes are computed from the bytes that will run. The approval is signed with the operator key on the CLI side (`weft-licence-v1/approval`), then verified by the daemon, stored, and flooded to every node.
- `approve` without `--confirm` prints the hashes it would approve and their content key, and signs nothing. Compare the hashes with the registry entry or the upstream release first, then pass that content key to `--confirm`; if the content prepared on the second run hashes differently (the grant changed in between), nothing is signed. `--sha256 <hex>` (repeatable) approves exactly those hashes instead of the held grant's set.
- `--reapprove-orphaned` takes the orphaned approvals as signed envelopes from the daemon, keeps only those signed by your operator key for an earlier mesh id, prints how many and a batch digest, and signs only with `--confirm <digest>`.
- On a user daemon outside a project, `checkout` and `approve` are allowed for an Admin caller (they are mesh-wide licence operations), and `status` and `weaver workload node status` are read-only and always allowed, so `weaver doctor` reports `licence.*` findings anywhere.
- `weaver cog checkout` needs Admin: a checkout spends the Seed's licence and transfer budget. On the steward it is charged per caller (project, token or operator; 5 a minute) and node-wide (20 a minute), and the governance gate is asked as that caller.
- **Refusals** carry a stable code: `no_grant`, `grant_lapsed`, `not_in_grant`, `no_approval`, `hash_revoked` or `binding_inactive`. `weaver workload place --explain` shows it as the attempt's reason, and the node chains it as `workload.refuse`. A permitted run is chained as `cog.run.permit` with the grant id and the approval id.
- **Which binary.** The binary that runs is the one the node's runtime admits (native: the host arch; container: its own arch order); only it needs a grant and an approval, so checking out and approving just that arch is enough. A revoked binary of any other arch still refuses the package. A placement whose variant names a different arch is refused (`arch_mismatch`). At each start the staged file is hashed again from disk.
- **Re-packed bytes.** A package that does not say Cognitum but carries a binary a held grant lists (or a revoked hash) is gated all the same, under its own id.
- **A deleted licence store** does not turn the gate off: the first accepted binding writes `licence-bound.marker` beside the store, and a node with the marker (or with approvals) but no binding refuses (`binding_inactive`) until sync brings the binding back. Deleting the whole `licence/` directory does reset it (the node reads as never bound until a peer re-syncs); whoever can do that can already tamper with the daemon.
- **Release and renew** run on the steward (they use its link to `weft-licence`); elsewhere they answer `[not_steward]` with the steward's node id. `release` makes the Seed withdraw that checkout: the withdrawal is flooded, new starts stop, and running instances keep running. `renew` does now what the 12-hourly pass does; the Seed has no per-checkout renewal, so every active checkout is renewed.
- **A lapse** (an expired or withdrawn grant, or an unbind) refuses new starts and restarts. Running instances keep running.
- **To withdraw an approval**, revoke the artifact hash: `weaver workload revoke --hash <blake3>`. That also evicts the bytes.
- Signed WeftOS and private packages never reach this gate. A node that never held a Seed binding keeps the per-project path above.

## 4. Create and publish a private repo

A private repo is a directory you sign with a key only you hold. Use `weft-cog-repo`.

```sh
weft-cog-repo init acme-cogs --name acme-private
# keep the key OUTSIDE the repo directory (the tool refuses inside it)
weft-cog-repo keygen --out ~/.config/weftos/keys/acme-private.pem --repo acme-cogs
#   prints the public key; also stored in acme-cogs/repo.toml

weft-cog-repo add acme-cogs --binary target/aarch64-unknown-linux-gnu/release/cog-acme-gauge \
        --id acme-gauge --arch arm64 --cog-toml src/cogs/acme-gauge/cog.toml
weft-cog-repo add acme-cogs --binary cog-acme-gauge-armv7 --id acme-gauge --arch arm    # a second arch of the same cog

weft-cog-repo sign   acme-cogs --key ~/.config/weftos/keys/acme-private.pem
weft-cog-repo verify acme-cogs
weft-cog-repo publish acme-cogs --to ./acme-cogs-public
```

`publish` verifies every artifact, then copies `acme-cogs/repo/` to the directory you name and prints the `[[cog_source]]` snippet for consumers. **It does not upload anything**: put that directory on static HTTPS, object storage or a file share yourself. Then, in each project that should use it:

```sh
weaver cog source add acme-private --kind private --url https://cogs.acme.example/ --key <the public key>
```

Layout of the repo directory:

```text
acme-cogs/repo.toml     name and public key (safe to commit)
acme-cogs/.gitignore    ignores *.pem, *.key, *.seed
acme-cogs/dist/<id>/    staged, unsigned: cog-<id>-arm[-arm64] + manifest.json
acme-cogs/repo/         signed output: registry.json + cogs/<arch>/...   (this is what you host)
```

`weft-cog-repo verify <dir-or-url> --pin <pubkey>` and `install ... --pin <pubkey>` check any COG-008 repo against your own key (without `--pin` they check the WeaveLogic key; for a private repo directory `verify` without `--pin` uses the key in `repo.toml`). An explicit `--pin` always wins, also for a directory with a `repo.toml`: a pin that is not the signing key fails.

## 5. Key management

There are four different keys. Do not mix them up.

| Key | Signs | Held by | Pinned where |
|---|---|---|---|
| WeaveLogic release key | binaries in the WeftOS registry (COG-008) | CI secret `WEAVELOGIC_RELEASE_KEY` | compiled in: `WEAVELOGIC_PUBKEY_HEX` |
| WeftOS package signer | `cogpkg.json` manifests (governed placement) | a secret store (see below) | compiled in: `WEFTOS_PINNED_SIGNERS` |
| Your private-repo key | binaries in your private registry | you | each project's `pinned_keys` |
| Operator package key | `cogpkg.json` manifests you pack | you | the node's `workload-trust.json` |

### Provisioning the WeftOS package signer (`WEFTOS_PINNED_SIGNERS`)

The set in `crates/clawft-kernel/src/workload_pkg/trust.rs` is empty until this is done, so today every accepted package signature is operator-pinned. To provision it (an owner action, one time):

1. Generate the key on a trusted machine: `weaver workload keygen --out weftos-package-signer.seed --key-id weftos-release-1`. It writes the 32-byte seed as 64 hex chars with mode 0600 (never overwriting) and prints the key id, the public key and a trust-file entry.
2. Put the **seed** in your secret store (the CI secret manager) under a name such as `WEFTOS_PACKAGE_SIGNING_KEY`, and delete the local file. The seed is never committed, never in a chain event and never in a log. Back it up separately from the CI secret.
3. Pin the **public** key: add `("weftos-release-1", "<64 hex public key>")` to `WEFTOS_PINNED_SIGNERS` in a reviewed commit. The key id is part of the pin: a signature that carries a different `key_id` for the same key is refused (`bad-signature`).
4. Release builds sign packages with the seed from the secret store (`weaver workload pack --key <file>`, where CI materialises the secret into a 0600 temp file). Everyone who verifies with the default anchors now trusts that signer.
5. Rotation: generate a new key and key id (`weftos-release-2`), add it next to the old one, release, re-sign what must stay installable, then remove the old pin in a later release. Revoke a compromised signer by key (`RevocationKind::SignerKey`) in addition to removing the pin.

Until step 3 ships, use an operator-pinned key: put `{"key_id": "...", "public_key": "..."}` in the node's `workload-trust.json` (schema `weftos.workload-trust.v1`).

### Your private-repo key

`weft-cog-repo keygen` writes a PKCS#8 PEM with mode 0600, never overwrites an existing file, and refuses a path inside the repo directory, including one whose parent directories do not exist yet and one that reaches the repo through `..`. Keep it in a secret store or an encrypted volume, and back it up: losing it means you cannot sign updates under the pinned key, and every project must re-pin a new key. Rotating means: generate a new key, sign the repo with it, and ship the new public key to each project (`weaver cog source remove` / `add --key`), or pin both keys during the changeover (`--key` is repeatable). A compromised key means: stop publishing, generate a new one, re-sign, re-pin everywhere, and treat binaries signed since the compromise as untrusted.

## 6. The catalog

```sh
weaver workload catalog --kind cog \
    --cog-toml-dir <cogs checkout>/src/cogs        # optional: resources, secrets, hardware (<id>/cog.toml)
```

One row per cog per source: arch coverage, hardware requirement, `[resources]`, secret config keys, run mode (`once`, `interval N`, `needs-seed`, `needs-extra`), install access (`signed`, `licensed`, `needs-licence`, `licence-expired`), whether the id is in the Seed store, which other sources list it, and the default placement policy (node tier at least `paired`, signed package required, emulation opt-in, capability requirements, v1 scope). Run mode comes from the committed conformance baseline `scripts/cogs/expectations.json` (read from `scripts/cogs/` in the current directory, or `--baseline`): **93 clean, 5 need `--interval`, 9 need seed peers or other setup**, plus one cog with no aarch64 build. `--json` prints the rows.

## What is built, and what is not

Built and tested: config parsing and the project/user merge; the three source kinds; licence check (presence, coverage, expiry; unlicensed refused before any download); private-repo `init/keygen/add/sign/verify/publish` and `--pin`; namespaced and bare resolution with explicit ambiguity; verified install into a cog-host root with `provenance.json`; the catalog; the four package-trust fixes in the kernel.

Built for Seed-bound meshes (ADR-106 phase 3): `weaver cog checkout`, `approve`, `--reapprove-orphaned`, `status`; the run gate in the workload host; the steward relay.

Designed, not built (ADR-105 open questions): moving the source list into the user-daemon project manifest and serving it from the daemon; chain events for resolve and install (the payload exists as `Provenance::chain_payload`); building a governed `cogpkg` straight from a registry artifact; signed licences; a default `weftos` registry URL; provisioning the first `WEFTOS_PINNED_SIGNERS` key; Cognitum release-record verification on the registry install path.
