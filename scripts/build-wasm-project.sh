#!/usr/bin/env bash
# Explicit compiler job for the integration lead. Never creates a new target
# cache or installs targets. Run detached as required by repository policy.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${CARGO_TARGET_DIR:?Set the existing shared Cargo cache explicitly}"
[[ -d "$CARGO_TARGET_DIR" ]] || { echo 'Shared cache does not exist' >&2; exit 1; }
case "${1:---help}" in
    --guest)
        # Core module ABI + WASI Preview 1. NOT the existing wasip2 component.
        cargo build --locked --offline -p clawft-wasm --no-default-features \
            --features project-kernel --bin weftos-project-guest --target wasm32-wasip1
        ;;
    --runner)
        cargo build --locked --offline -p clawft-wasm-host --features project-kernel \
            --bin weftos-wasm-project-runner
        ;;
    *) echo 'Usage: CARGO_TARGET_DIR=<existing-cache> scripts/build-wasm-project.sh --guest|--runner' ;;
esac
