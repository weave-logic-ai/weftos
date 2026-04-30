# Installing WeftOS

`scripts/install.sh` is the canonical install path for the `weft`,
`weaver`, and `weftos` binaries. It is published verbatim at
[https://weftos.weavelogic.ai/install.sh](https://weftos.weavelogic.ai/install.sh)
and intended for `curl | sh` use.

## Quick start

```bash
curl -fsSL https://weftos.weavelogic.ai/install.sh | sh
```

The installer is idempotent: re-running it upgrades to the latest
release if you are not already on it, and is a no-op otherwise.

## Provenance verification

Every release archive that `cargo-dist` publishes ships with a
sigstore-rekor attestation. By default, `install.sh` runs
`gh attestation verify` against each downloaded archive before
installing it:

| Mode      | Trigger                                                       | Behaviour |
|-----------|---------------------------------------------------------------|-----------|
| `default` | (no flag)                                                     | Verify if `gh` is installed; warn and continue if `gh` is missing. |
| `force`   | `--verify` (or `WEFTOS_VERIFY=1` for parity with `--no-verify`) | `gh` MUST be installed. Verification failure aborts the install. |
| `skip`    | `--no-verify` or `WEFTOS_NO_VERIFY=1`                         | Skip verification entirely. |

Examples:

```bash
# Default behaviour (verify when gh is present, warn otherwise)
curl -fsSL https://weftos.weavelogic.ai/install.sh | sh

# Hard-require attestation verification — refuse to install on failure
curl -fsSL https://weftos.weavelogic.ai/install.sh | sh -s -- --verify

# Skip verification (for air-gapped systems without gh)
curl -fsSL https://weftos.weavelogic.ai/install.sh | sh -s -- --no-verify
```

`--verify` is the recommended setting for any production install. It
guarantees that the archive was produced by the WeftOS `Release`
workflow on a tag in this repository — not pulled from a tampered
mirror.

The same check can be run manually:

```bash
gh attestation verify weft-cli-0.7.0-x86_64-unknown-linux-musl.tar.gz \
  --repo weave-logic-ai/weftos
```

See [`release.md` "Verifying Provenance"](./release.md#verifying-provenance)
for the full release-side picture.

## Custom install location

The default install directory is `/usr/local/bin`. Override with
`WEFTOS_INSTALL_DIR`:

```bash
mkdir -p ~/.local/bin
WEFTOS_INSTALL_DIR=$HOME/.local/bin \
  curl -fsSL https://weftos.weavelogic.ai/install.sh | sh
```

The installer writes one binary per channel (`weft`, `weaver`,
`weftos`) directly into that directory; no extra wrappers are
created.

## Updating

Re-run the installer or, if `weaver` is already on `$PATH`:

```bash
weaver update
```

Both flows download the latest release, run the same verification
step, and replace the binaries in place. If `weaver kernel status`
reports a running daemon, the installer stops it before swapping the
binary and starts it again afterwards.

## Uninstall

There is no separate uninstall script. Remove the three binaries from
`$WEFTOS_INSTALL_DIR`:

```bash
sudo rm /usr/local/bin/weft /usr/local/bin/weaver /usr/local/bin/weftos
```

User-level state lives under `~/.clawft/` — see the
[Canonical Install Paths](./release.md#canonical-install-paths)
section in the release docs.
