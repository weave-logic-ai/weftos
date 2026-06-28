# Minimal Docker image for weft (clawft CLI)
#
# Uses pre-built musl binaries from GitHub Releases + Alpine base.
# No compilation — just downloads the correct binary for the target arch.
#
# Build:
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     --build-arg VERSION=0.4.2 -t weft .
#
# Target: ~15MB compressed image.

FROM alpine:3.21

ARG VERSION=0.6.21
ARG TARGETARCH

RUN apk add --no-cache ca-certificates tzdata \
    && adduser -D -h /home/weft weft

# Map Docker TARGETARCH to Rust target triple
# TARGETARCH is set automatically by buildx: amd64 or arm64
COPY <<'EOF' /tmp/install.sh
#!/bin/sh
set -eu
case "$TARGETARCH" in
  amd64) TRIPLE="x86_64-unknown-linux-musl" ;;
  arm64) TRIPLE="aarch64-unknown-linux-musl" ;;
  *) echo "Unsupported arch: $TARGETARCH"; exit 1 ;;
esac
ASSET="clawft-cli-${TRIPLE}.tar.gz"
URL="https://github.com/weave-logic-ai/weftos/releases/download/v${VERSION}/${ASSET}"
echo "Downloading ${URL}"
wget -qO- "$URL" | tar xz --strip-components=1 -C /usr/local/bin/
chmod +x /usr/local/bin/weft
EOF
RUN sh /tmp/install.sh && rm /tmp/install.sh

# Pre-create the data dir owned by the weft user BEFORE declaring the
# VOLUME (Docker creates an unmounted VOLUME mountpoint root-owned, so
# `weft gateway` as uid 1000 would otherwise die at "bootstrap app
# context: Permission denied" creating ~/.clawft/workspace/sessions),
# and ship a default config that turns the REST/WS API on at the exposed
# port 8080. `weft gateway` refuses to start with no channel and the API
# off; enabling the API makes the image usable out-of-the-box and lets
# the EXPOSE 8080 / HEALTHCHECK / `/api/health` probe work. The API is
# auth-gated (Bearer token via TokenStore); only /api/health and the
# token-bootstrap path are public.
RUN mkdir -p /home/weft/.clawft \
    && printf '{"gateway":{"api_enabled":true,"api_port":8080}}\n' \
       > /home/weft/.clawft/config.json \
    && chown -R weft:weft /home/weft/.clawft

USER weft
WORKDIR /home/weft

VOLUME ["/home/weft/.clawft"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/weft", "status"]

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/weft"]
CMD ["gateway"]
