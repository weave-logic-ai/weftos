# Step 0 (A4): Feature Validation Script & Test Fixture Update

Date: 2026-02-24
Branch: feature/three-workstream-implementation

> **Update 2026-04-30 (WEFT-409):** `scripts/check-features.sh` was
> never landed. The canonical feature-flag validation entrypoint is
> now `scripts/build.sh gate`, which covers the same five gates plus
> clippy, bundle-size, audit, and docs-regen checks (12 total). The
> intent of the original A4 task is preserved; the implementation is
> consolidated into `build.sh` rather than a parallel script.

## What was created (historical / superseded)

### scripts/check-features.sh (PLANNED, NEVER CREATED — see WEFT-409)
Feature validation script that runs five compilation gates before push:

1. **Gate 1** - Native workspace compilation (`cargo check --workspace`)
2. **Gate 2** - Native test compilation (`cargo test --workspace --no-run`)
3. **Gate 3** - Native CLI binary build (`cargo build --bin weft`)
4. **Gate 4** - WASI WASM compilation (`cargo check --target wasm32-wasip2 -p clawft-wasm`)
5. **Gate 5** - Browser WASM compilation (`cargo check --target wasm32-unknown-unknown -p clawft-wasm --no-default-features --features browser`)

Gates 4 and 5 gracefully skip if the required rustup targets are not installed.
Gate 5 also skips (rather than failing) if the browser feature is not yet implemented,
since it depends on the BW1 workstream completing first.

The script uses `set -euo pipefail` for strict error handling and color-coded
output (PASS/FAIL/SKIP) for readability.

## What was changed

### tests/fixtures/config.json (updated)
Added fields from all three workstreams while preserving all existing content:

- **Gateway additions** (Workstream BW - Browser WASM):
  - `gateway.apiPort`: 18789
  - `gateway.corsOrigins`: ["http://localhost:5173"]
  - `gateway.apiEnabled`: false

- **Provider additions** (Workstream BW - Browser WASM):
  - `providers.anthropic.browserDirect`: true

- **Voice section** (Workstream V - Voice pipeline, new top-level key):
  - `voice.enabled`: false
  - `voice.audio.sampleRate`: 16000
  - `voice.audio.chunkSize`: 512
  - `voice.audio.channels`: 1
  - `voice.vad.threshold`: 0.5
  - `voice.vad.silenceTimeoutMs`: 1500

All existing fields remain unchanged. The fixture remains valid JSON.
