# Docker Deployment

WeftOS ships a multi-arch Docker image based on `alpine:3.21`. The image
contains a pre-built static musl `weft` binary downloaded from the matching
GitHub Release at build time -- no compilation runs inside the image.
Compressed size is ~15 MB; supported architectures are `linux/amd64` and
`linux/arm64`.

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) 20.10 or later (with
  `buildx` for multi-arch builds).

## Quick Start

Pull the published image and run the gateway:

```bash
docker pull ghcr.io/weave-logic-ai/weftos:latest
docker run --rm -it ghcr.io/weave-logic-ai/weftos:latest --version
```

By default the entrypoint is `weft` and the default command is `gateway`,
so a bare `docker run ... weftos:latest` starts the gateway on port 8080.

## Image Tags

| Tag         | Contents                                            |
|-------------|-----------------------------------------------------|
| `latest`    | Most recent release (rolls forward on every release)|
| `vX.Y.Z`    | Pinned release (e.g. `v0.6.19`, `v0.7.0`)           |

Pin to a version in production. The `latest` tag is safe for local
exploration but moves under you on every release.

## Configuration

The container runs as the unprivileged `weft` user with `$HOME=/home/weft`.
The canonical config + state directory inside the container is
`/home/weft/.clawft` -- mount your host config there.

### Mounting Config

```bash
docker run --rm -it \
  -v "$HOME/.clawft:/home/weft/.clawft:ro" \
  ghcr.io/weave-logic-ai/weftos:latest gateway
```

### Environment Variables

Pass API keys and runtime settings via environment variables. Common ones:

```bash
docker run --rm -it \
  -e OPENAI_API_KEY="sk-..." \
  -e CLAWFT_CONFIG="/home/weft/.clawft/config.json" \
  -e RUST_LOG=info \
  -v "$HOME/.clawft:/home/weft/.clawft:ro" \
  ghcr.io/weave-logic-ai/weftos:latest gateway
```

### Persisting Workspace State

To persist sessions, memory, and skills across restarts, mount a writable
volume on the same path:

```bash
docker run --rm -it \
  -v weftos-data:/home/weft/.clawft \
  ghcr.io/weave-logic-ai/weftos:latest gateway
```

## Docker Compose

A working compose definition ships under `scripts/deploy/docker-compose.yml`.
Run it directly:

```bash
docker compose -f scripts/deploy/docker-compose.yml up -d
docker compose -f scripts/deploy/docker-compose.yml logs -f
```

The shipped file uses a named volume (`weft-config`) for state and exposes
port 8080. Override the version with the `WEFT_VERSION` environment
variable:

```bash
WEFT_VERSION=0.6.19 docker compose -f scripts/deploy/docker-compose.yml up -d
```

Inline equivalent:

```yaml
services:
  weft:
    image: ghcr.io/weave-logic-ai/weftos:${WEFT_VERSION:-latest}
    command: ["gateway"]
    restart: unless-stopped
    ports:
      - "8080:8080"
    volumes:
      - weft-config:/home/weft/.clawft
    environment:
      - RUST_LOG=info
      - OPENAI_API_KEY=${OPENAI_API_KEY}
    healthcheck:
      test: ["CMD", "weft", "status"]
      interval: 30s
      timeout: 5s
      start_period: 10s
      retries: 3

volumes:
  weft-config:
```

## VPS One-Click Deploy

`scripts/deploy/vps-deploy.sh` wraps `docker run` with sensible defaults
for a single-host VPS install:

```bash
./scripts/deploy/vps-deploy.sh --pull
```

Flags: `--image`, `--port`, `--name`, `--config`, `--restart`, `--pull`.
The script idempotently stops any existing container with the same name
before starting the new one.

## Health Checks

The image declares a `HEALTHCHECK` that runs `weft status` every 30
seconds. The same probe is shown in the compose example above. To check
health from the host:

```bash
docker inspect --format '{{.State.Health.Status}}' weft
docker exec weft weft status --detailed
```

## Building Locally

The shipped `Dockerfile` downloads the published binary from the matching
GitHub Release. To build a local image at a specific version:

```bash
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  --build-arg VERSION=0.6.19 \
  -t weftos:local .
```

To build against a release that hasn't been cut yet (development image),
you would need to swap `Dockerfile` for one that compiles from source --
see "Appendix: legacy build approaches" below.

## CI / Release Pipeline

The image is built and published by `.github/workflows/release-docker.yml`,
triggered automatically when the `Release` workflow (cargo-dist) succeeds
on a tag push. The workflow:

1. Waits for the upstream `Release` workflow to finish (`workflow_run`
   completed + success gate).
2. Sets up QEMU and Buildx for multi-arch.
3. Logs into GHCR using the workflow's `GITHUB_TOKEN`.
4. Tags the resulting manifest with both `latest` and `vX.Y.Z`.
5. Pushes a single multi-arch manifest covering `linux/amd64` and
   `linux/arm64`.

See `docs/deployment/release.md` for the full release flow and how Docker
fits into the cargo-dist sub-release graph.

## Security Considerations

The image is small but not minimal -- it includes Alpine + musl libc +
`ca-certificates` + `tzdata`. Hardening recommendations:

- **Run as the built-in unprivileged user.** The image already declares
  `USER weft`. Don't override with `--user 0:0` unless you know why.
- **Mount config read-only.** Use `:ro` on the config bind mount; only
  the workspace volume needs to be writable.
- **Restrict egress.** For network-isolated deployments use a dedicated
  Docker network or `--network=none` for offline workflows.
- **Never bake secrets into the image.** Pass keys via environment
  variables, Docker secrets, or a sidecar secrets manager.
- **Pin tags in production.** `latest` is convenient for local dev; pin
  to `vX.Y.Z` in any deployment you don't want to silently roll forward.

## Troubleshooting

**Container exits immediately.** Inspect with:

```bash
docker run --rm -it \
  -v "$HOME/.clawft:/home/weft/.clawft:ro" \
  ghcr.io/weave-logic-ai/weftos:latest status --detailed
```

Common causes: missing config, invalid JSON, an LLM provider env var
referenced by config but not set in the container's environment.

**Permission denied on config file.** The container runs as a non-root
`weft` user. Make sure the host config is readable by anyone:

```bash
chmod 644 ~/.clawft/config.json
```

**Cannot reach LLM API.** Verify DNS and outbound network access from
inside the container:

```bash
docker run --rm ghcr.io/weave-logic-ai/weftos:latest agent -m "ping"
```

If running behind a corporate proxy, propagate the proxy env vars:

```bash
docker run --rm \
  -e HTTPS_PROXY="http://proxy:8080" \
  -e HTTP_PROXY="http://proxy:8080" \
  -e NO_PROXY="localhost,127.0.0.1" \
  ghcr.io/weave-logic-ai/weftos:latest agent -m "ping"
```

---

## Appendix: Legacy Build Approaches (Historical)

Earlier releases experimented with two different image layouts. They are
preserved here only so operators upgrading from old documentation can map
the new approach onto what they remember.

### `FROM scratch` (pre-0.4.2, deprecated)

Sprint 11 / 12 images were built `FROM scratch`, copying a single
statically-linked musl binary into the empty filesystem (~5 MB compressed).
The approach was dropped because:

- No `ca-certificates` -- TLS to LLM providers required either baking
  certs in or a sidecar.
- No `tzdata` -- log timestamps and any `chrono::Local` calls fell back
  to UTC silently.
- No shell -- `docker exec ... bash` was impossible; only `weft`
  subcommands worked, which made operational debugging painful.
- Health checks had to call the `weft` binary itself; standard
  busybox-style probes were unavailable.

The current Alpine 3.21 base solves all four at the cost of ~10 MB.

### `cargo-chef` multi-stage build (proposed, never shipped)

A K2-era proposal documented in earlier revisions of this guide
described a multi-stage `debian:bookworm-slim` image using `cargo-chef`
for dependency caching:

1. Chef stage installs `cargo-chef`.
2. Planner emits a recipe from `Cargo.toml`.
3. Builder cooks dependencies (cached layer), then builds the app.
4. Runtime is `debian:bookworm-slim` plus the stripped binary,
   `ca-certificates`, `libssl3`, `libgcc-s1`.

This was never adopted as the shipping image. Reasons:

- Build time on GitHub Actions runners exceeded 30 minutes for cold
  caches, vs. ~2 minutes for "download the published musl binary"
  (since CI already produces the musl binary as a cargo-dist artifact).
- Debian-base image landed at ~50 MB compressed, ~3x larger than the
  current Alpine layout.
- Multi-arch via QEMU doubled build time again on `linux/arm64`.

The current Dockerfile is the post-0.4.3 simplification: download the
musl tarball produced by cargo-dist, install into `/usr/local/bin/`,
declare the runtime user. It's reproducible (the binary matches the
release artifact byte-for-byte), fast to build, and small enough to pull
on a slow connection.

If you need a fully-from-source image (e.g. for an unreleased commit),
the cargo-chef recipe in git history is a reasonable starting point --
but most use cases are better served by `cargo install --path crates/clawft-cli`
on the host plus a thin runtime image, or by tagging a pre-release and
letting the standard CI flow build the matching image.
