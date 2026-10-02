# How a project gets cogs: sources, licences and private repos

Design record: [ADR-105](../adr/adr-105-cog-sources.md). Commands: `weaver cog ...`, `weaver workload catalog --kind cog`, and `weft-cog-repo` for authoring. This page is the how-to. What is built and what is only designed is listed at the end.

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
pinned_keys = ["<64-hex public key>"]   # Ed25519 public keys, 64 hex. Required for private
priority    = 50                    # default 0; higher wins when a bare id is in several sources
enabled     = true                  # default true
```

What each kind trusts:

- `weftos`: the pinned WeaveLogic release key (`WEAVELOGIC_PUBKEY_HEX`), the compiled-in WeftOS package signers (`WEFTOS_PINNED_SIGNERS`, empty today), and any `pinned_keys` you add.
- `private`: **only** the keys in `pinned_keys`. The WeaveLogic key is never implicitly trusted by a private source.
- `cognitum`: no signature (Cognitum's registry binaries are unsigned); `pinned_keys` would pin release-record keys for the optional verifier.

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

`weft-cog-repo verify <dir-or-url> --pin <pubkey>` and `install ... --pin <pubkey>` check any COG-008 repo against your own key (without `--pin` they check the WeaveLogic key).

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

`weft-cog-repo keygen` writes a PKCS#8 PEM with mode 0600, never overwrites an existing file, and refuses a path inside the repo directory. Keep it in a secret store or an encrypted volume, and back it up: losing it means you cannot sign updates under the pinned key, and every project must re-pin a new key. Rotating means: generate a new key, sign the repo with it, and ship the new public key to each project (`weaver cog source remove` / `add --key`), or pin both keys during the changeover (`--key` is repeatable). A compromised key means: stop publishing, generate a new one, re-sign, re-pin everywhere, and treat binaries signed since the compromise as untrusted.

## 6. The catalog

```sh
weaver workload catalog --kind cog \
    --cog-toml-dir <cogs checkout>/src/cogs        # optional: resources, secrets, hardware (<id>/cog.toml)
```

One row per cog per source: arch coverage, hardware requirement, `[resources]`, secret config keys, run mode (`once`, `interval N`, `needs-seed`, `needs-extra`), install access (`signed`, `licensed`, `needs-licence`, `licence-expired`), whether the id is in the Seed store, which other sources list it, and the default placement policy (node tier at least `paired`, signed package required, emulation opt-in, capability requirements, v1 scope). Run mode comes from the committed conformance baseline `scripts/cogs/expectations.json` (read from `scripts/cogs/` in the current directory, or `--baseline`): **93 clean, 5 need `--interval`, 9 need seed peers or other setup**, plus one cog with no aarch64 build. `--json` prints the rows.

## What is built, and what is not

Built and tested: config parsing and the project/user merge; the three source kinds; licence check (presence, coverage, expiry; unlicensed refused before any download); private-repo `init/keygen/add/sign/verify/publish` and `--pin`; namespaced and bare resolution with explicit ambiguity; verified install into a cog-host root with `provenance.json`; the catalog; the four package-trust fixes in the kernel.

Designed, not built (ADR-105 open questions): moving the source list into the user-daemon project manifest and serving it from the daemon; chain events for resolve and install (the payload exists as `Provenance::chain_payload`); building a governed `cogpkg` straight from a registry artifact; signed licences; a default `weftos` registry URL; provisioning the first `WEFTOS_PINNED_SIGNERS` key; Cognitum release-record verification on the registry install path.
