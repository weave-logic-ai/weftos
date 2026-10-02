# probe-rs + Arm binutils for the NUCLEO-N657X0-Q (STM32N657, Cortex-M55).
# macOS USB stalls the board's STLINK-V3EC, so scripts/n6.sh passes the probe
# through OrbStack (`orb usb attach`) and runs these tools in linux/arm64.
FROM debian:trixie-slim

ARG PROBE_RS_VERSION=0.32.0
ARG PROBE_RS_SHA256=7c818cfd77808e806bf8f4d108c9137910b4fb28e0fe5c464d39782dbbc8af31

RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      ca-certificates curl xz-utils libudev1 binutils-arm-none-eabi gdb-multiarch \
 && rm -rf /var/lib/apt/lists/*

RUN set -eu; \
    f=probe-rs-tools-aarch64-unknown-linux-gnu.tar.xz; \
    curl -sSfLO "https://github.com/probe-rs/probe-rs/releases/download/v${PROBE_RS_VERSION}/$f"; \
    echo "${PROBE_RS_SHA256}  $f" | sha256sum -c -; \
    tar xf "$f"; \
    find . -maxdepth 2 -type f -name probe-rs -exec install -m 0755 {} /usr/local/bin/probe-rs \; ; \
    rm -rf "$f" probe-rs-tools-*; \
    probe-rs --version

WORKDIR /work
