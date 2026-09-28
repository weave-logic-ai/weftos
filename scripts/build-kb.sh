#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "Building knowledge base tool..."
cargo build --release --manifest-path "$ROOT/tools/build-kb/Cargo.toml"

echo "Generating knowledge base..."
mkdir -p "$ROOT/docs/src/public/kb"
# Fumadocs MDX (site pages) plus the rest of docs/*.md: ADRs, guides,
# research, brain, architecture, weftos specs. Skip plans/ and generated
# src/ trees (see corpus.rs should_skip).
"$ROOT/tools/build-kb/target/release/build-kb" \
    --repo-root "$ROOT" \
    --docs-dir "$ROOT/docs/src/content/docs" \
    --docs-dir "$ROOT/docs" \
    --output "$ROOT/docs/src/public/kb/weftos-docs.rvf"

echo ""
echo "Output: $ROOT/docs/src/public/kb/weftos-docs.rvf"
ls -lh "$ROOT/docs/src/public/kb/weftos-docs.rvf"
