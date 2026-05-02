#!/usr/bin/env bash
# Build the clawft-gui-egui crate for wasm32-unknown-unknown and drop the
# wasm-bindgen bundle under `webview/wasm/` so the VSCode / Cursor
# extension's webview can load it.
#
# Artifacts are .gitignore'd; run this before packaging the extension.
#
# Prefers `wasm-pack` when available and healthy. Falls back to `cargo
# build --target wasm32-unknown-unknown` + a separately-installed
# `wasm-bindgen` CLI — covers environments where wasm-pack's bundled
# bindgen downloader is broken (observed: "invalid type: map, expected a
# string" when parsing the GitHub releases manifest).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"
out_dir="$here/../webview/wasm"

cd "$repo_root"
mkdir -p "$out_dir"

# ── Preferred path: wasm-pack ────────────────────────────────────────
if command -v wasm-pack >/dev/null; then
    echo "→ Trying wasm-pack build clawft-gui-egui → $out_dir"
    if wasm-pack build crates/clawft-gui-egui \
            --target web \
            --out-dir "$out_dir" \
            --no-typescript \
            -- --no-default-features; then
        echo "✓ Wasm bundle at $out_dir"
        ls -l "$out_dir"
        exit 0
    fi
    echo "! wasm-pack failed — falling back to cargo + wasm-bindgen"
fi

# ── Fallback: cargo build + wasm-bindgen ─────────────────────────────
command -v wasm-bindgen >/dev/null || {
    echo "Neither wasm-pack (working) nor wasm-bindgen are available."
    echo "Install either:"
    echo "  cargo install wasm-bindgen-cli"
    echo "  curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh"
    exit 1
}

echo "→ cargo build --target wasm32-unknown-unknown --release"
cargo build \
    -p clawft-gui-egui \
    --target wasm32-unknown-unknown \
    --release \
    --no-default-features

wasm_in="target/wasm32-unknown-unknown/release/clawft_gui_egui.wasm"
[ -f "$wasm_in" ] || {
    echo "wasm artifact missing at $wasm_in"
    exit 1
}

echo "→ wasm-bindgen → $out_dir"
wasm-bindgen \
    --target web \
    --out-dir "$out_dir" \
    --no-typescript \
    "$wasm_in"

# WEFT-246: post-bindgen wasm-opt -Oz pass.
#
# wasm-pack handles this through the package.metadata.wasm-pack
# profile when its preferred path fires above. In the manual
# cargo+wasm-bindgen fallback path we have to invoke wasm-opt
# ourselves, otherwise the webview loads the unoptimised bundle
# (~4.2 MB on M4-C). Fail soft: if wasm-opt is missing or rejects
# the input, ship the unoptimised bytes — the panel still works,
# just larger.
wasm_bg="$out_dir/clawft_gui_egui_bg.wasm"
if [ -f "$wasm_bg" ] && command -v wasm-opt >/dev/null; then
    pre=$(stat -c '%s' "$wasm_bg" 2>/dev/null || stat -f '%z' "$wasm_bg")
    echo "→ wasm-opt -Oz $wasm_bg (pre: ${pre} bytes)"
    # Older binaryen versions (≤ 116) refuse modern Rust output
    # without explicit feature flags. Pass the union of features
    # rustc 1.93 emits — bulk-memory + nontrapping-fptoint + sign-ext
    # + mutable-globals + multivalue + reference-types — so the
    # validator accepts the input. WEFT-246.
    if wasm-opt \
        --enable-bulk-memory \
        --enable-nontrapping-float-to-int \
        --enable-sign-ext \
        --enable-mutable-globals \
        --enable-multivalue \
        --enable-reference-types \
        -Oz \
        -o "$wasm_bg.opt" "$wasm_bg" 2>/dev/null; then
        mv "$wasm_bg.opt" "$wasm_bg"
        post=$(stat -c '%s' "$wasm_bg" 2>/dev/null || stat -f '%z' "$wasm_bg")
        delta=$((pre - post))
        echo "✓ wasm-opt succeeded (post: ${post} bytes, saved: ${delta} bytes)"
    else
        rm -f "$wasm_bg.opt"
        echo "! wasm-opt rejected the bundle; shipping unoptimised bytes."
    fi
elif [ ! -f "$wasm_bg" ]; then
    echo "! expected $wasm_bg missing — wasm-bindgen output naming changed?"
else
    echo "! wasm-opt not found in PATH — install binaryen for size-opt builds."
fi

echo "✓ Wasm bundle at $out_dir"
ls -l "$out_dir"
