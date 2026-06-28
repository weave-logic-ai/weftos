#!/usr/bin/env bash
# Unified build script for ClawFT workspace.
# Wraps cargo, wasm, and UI builds behind simple subcommands.
set -euo pipefail

# ── Colors ───────────────────────────────────────────────────────────
RED=$'\033[0;31m'
GREEN=$'\033[0;32m'
YELLOW=$'\033[1;33m'
CYAN=$'\033[0;36m'
BOLD=$'\033[1m'
NC=$'\033[0m'

# ── Resolve workspace root ──────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

# ── Defaults ─────────────────────────────────────────────────────────
PROFILE=""
FEATURES=""
VERBOSE=false
DRY_RUN=false
FORCE=false
SERVE_PORT=""
WASM_PANEL_MAX_RAW_KB=""
WASM_PANEL_MAX_GZ_KB=""
COMMAND=""

# ── Reporting helpers ────────────────────────────────────────────────
pass()  { printf "  ${GREEN}PASS${NC}  %s\n" "$*"; }
fail()  { printf "  ${RED}FAIL${NC}  %s\n" "$*"; }
skip()  { printf "  ${YELLOW}SKIP${NC}  %s\n" "$*"; }
info()  { printf "  ${CYAN}INFO${NC}  %s\n" "$*"; }
header(){ printf "\n${BOLD}── %s${NC}\n" "$*"; }

# ── Timer ────────────────────────────────────────────────────────────
TIMER_START=0
timer_start() { TIMER_START=$(date +%s); }
timer_end() {
    local elapsed=$(( $(date +%s) - TIMER_START ))
    local min=$((elapsed / 60))
    local sec=$((elapsed % 60))
    if [ "$min" -gt 0 ]; then
        printf "  ${CYAN}TIME${NC}  %dm %ds\n" "$min" "$sec"
    else
        printf "  ${CYAN}TIME${NC}  %ds\n" "$sec"
    fi
}

# ── Force-clean a package before rebuild ───────────────────────────
force_clean_pkg() {
    local pkg="$1"
    if [ "$FORCE" = true ]; then
        info "Forcing rebuild (cleaning $pkg)"
        cargo clean -p "$pkg" 2>/dev/null || true
    fi
}

# ── Run a command (respects --verbose and --dry-run) ─────────────────
run_cmd() {
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   %s\n" "$*"
        return 0
    fi
    if [ "$VERBOSE" = true ]; then
        "$@"
    else
        "$@" 2>&1 | tail -5
    fi
}

# ── Target check ────────────────────────────────────────────────────
check_target_installed() {
    local target="$1"
    if ! rustup target list --installed 2>/dev/null | grep -q "$target"; then
        printf "  ${YELLOW}WARN${NC}  Target %s not installed. Run: rustup target add %s\n" "$target" "$target"
        return 1
    fi
    return 0
}

# ── Size reporting ──────────────────────────────────────────────────
report_binary_size() {
    local file="$1" label="${2:-Binary}"
    if [ -f "$file" ]; then
        local bytes
        bytes=$(wc -c < "$file")
        local kb=$((bytes / 1024))
        if [ "$kb" -ge 1024 ]; then
            local mb
            mb=$(echo "scale=2; $bytes / 1048576" | bc 2>/dev/null || echo "$((kb / 1024))")
            printf "  ${CYAN}SIZE${NC}  %s: %s MB (%s bytes)\n" "$label" "$mb" "$bytes"
        elif [ "$kb" -gt 0 ]; then
            printf "  ${CYAN}SIZE${NC}  %s: %s KB (%s bytes)\n" "$label" "$kb" "$bytes"
        else
            printf "  ${CYAN}SIZE${NC}  %s: %s bytes\n" "$label" "$bytes"
        fi
    fi
}

# ── Feature flag builder ────────────────────────────────────────────
cargo_features_args() {
    if [ -n "$FEATURES" ]; then
        echo "--features $FEATURES"
    fi
}

# ── Subcommands ─────────────────────────────────────────────────────

cmd_native() {
    local profile="${PROFILE:-release}"
    header "Building native CLI binary (profile: $profile)"
    force_clean_pkg clawft-cli
    timer_start
    local args=(cargo build --bin weft --bin weaver)
    if [ "$profile" = "release" ] || [ "$profile" = "release-wasm" ]; then
        args+=(--profile "$profile")
    fi
    [ -n "$FEATURES" ] && args+=(--features "$FEATURES")
    run_cmd "${args[@]}"
    timer_end
    if [ "$profile" = "release" ]; then
        report_binary_size "target/release/weft" "Native binary (weft)"
        report_binary_size "target/release/weaver" "Native binary (weaver)"
    elif [ "$profile" = "release-wasm" ]; then
        report_binary_size "target/release-wasm/weft" "Native binary (weft)"
        report_binary_size "target/release-wasm/weaver" "Native binary (weaver)"
    else
        report_binary_size "target/debug/weft" "Native binary (weft)"
        report_binary_size "target/debug/weaver" "Native binary (weaver)"
    fi
}

cmd_native_debug() {
    header "Building native CLI binary (debug)"
    force_clean_pkg clawft-cli
    timer_start
    local args=(cargo build --bin weft --bin weaver)
    [ -n "$FEATURES" ] && args+=(--features "$FEATURES")
    run_cmd "${args[@]}"
    timer_end
    report_binary_size "target/debug/weft" "Native binary (weft, debug)"
    report_binary_size "target/debug/weaver" "Native binary (weave, debug)"
}

# Build the native egui GUI binary (`weft-gui-egui`). Lives in
# `crates/clawft-gui-egui/` and gates the native main loop behind the
# `native` feature so the wasm target excludes eframe's window code
# (see Cargo.toml `[[bin]]` `required-features = ["native"]`). The
# wasm bundle (used by the VSCode panel) is built via `cmd_browser`.
cmd_gui_egui() {
    local profile="${PROFILE:-release}"
    header "Building native egui GUI binary weft-gui-egui (profile: $profile)"
    force_clean_pkg clawft-gui-egui
    timer_start
    local feat="native"
    if [ -n "$FEATURES" ]; then
        feat="native,$FEATURES"
    fi
    local args=(cargo build -p clawft-gui-egui --bin weft-gui-egui --features "$feat")
    if [ "$profile" = "release" ] || [ "$profile" = "release-wasm" ]; then
        args+=(--profile "$profile")
    fi
    run_cmd "${args[@]}"
    timer_end
    if [ "$profile" = "release" ]; then
        report_binary_size "target/release/weft-gui-egui" "weft-gui-egui (release)"
    elif [ "$profile" = "release-wasm" ]; then
        report_binary_size "target/release-wasm/weft-gui-egui" "weft-gui-egui (release-wasm)"
    else
        report_binary_size "target/debug/weft-gui-egui" "weft-gui-egui (debug)"
    fi
}

cmd_wasi() {
    local profile="${PROFILE:-release-wasm}"
    header "Building WASM for WASI (wasm32-wasip2, profile: $profile)"
    if ! check_target_installed wasm32-wasip2; then return 1; fi
    force_clean_pkg clawft-wasm
    timer_start
    local args=(cargo build --target wasm32-wasip2 --profile "$profile" -p clawft-wasm)
    [ -n "$FEATURES" ] && args+=(--features "$FEATURES")
    run_cmd "${args[@]}"
    timer_end
    report_binary_size "target/wasm32-wasip2/${profile}/clawft_wasm.wasm" "WASI WASM"
}

cmd_browser() {
    local profile="${PROFILE:-release-wasm}"
    header "Building WASM for browser (wasm32-unknown-unknown, profile: $profile)"
    if ! check_target_installed wasm32-unknown-unknown; then return 1; fi
    force_clean_pkg clawft-wasm
    timer_start
    local args=(cargo build --target wasm32-unknown-unknown -p clawft-wasm --no-default-features --features browser)
    args+=(--profile "$profile")
    # Append extra features if provided (comma-separated with browser)
    if [ -n "$FEATURES" ]; then
        # browser is already set; append user features
        args[-1]="browser,$FEATURES"
    fi
    run_cmd "${args[@]}"
    timer_end

    local wasm_file="target/wasm32-unknown-unknown/${profile}/clawft_wasm.wasm"
    report_binary_size "$wasm_file" "Browser WASM (raw)"

    # Run wasm-bindgen to generate JS glue into www/pkg/ so the test
    # harness can be served directly from www/ at the root URL.
    local pkg_dir="$ROOT/crates/clawft-wasm/www/pkg"
    if command -v wasm-bindgen >/dev/null 2>&1; then
        info "Running wasm-bindgen → $pkg_dir"
        run_cmd wasm-bindgen "$wasm_file" \
            --out-dir "$pkg_dir" \
            --target web \
            --no-typescript
        report_binary_size "$pkg_dir/clawft_wasm_bg.wasm" "Browser WASM (bindgen)"
        pass "pkg/ ready — run: scripts/build.sh serve"
    else
        skip "wasm-bindgen CLI not found — pkg/ not generated"
        info "Install with: cargo install wasm-bindgen-cli"
    fi
}

cmd_ui() {
    header "Building React frontend (tsc + vite)"
    if [ ! -d "$ROOT/clawft-ui" ] || [ ! -f "$ROOT/clawft-ui/package.json" ]; then
        skip "clawft-ui/ directory not found — skipping"
        return 0
    fi
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   cd clawft-ui && npm run build\n"
    else
        (cd "$ROOT/clawft-ui" && npm run build)
    fi
    timer_end
    if [ -d "$ROOT/clawft-ui/dist" ]; then
        local size
        size=$(du -sh "$ROOT/clawft-ui/dist" 2>/dev/null | cut -f1)
        printf "  ${CYAN}SIZE${NC}  UI bundle: %s\n" "$size"
    fi
}

cmd_ui_docker() {
    header "Building clawft-ui Docker image (multi-stage)"
    if [ ! -f "$ROOT/clawft-ui/Dockerfile" ]; then
        skip "clawft-ui/Dockerfile not found — skipping"
        return 0
    fi
    if ! command -v docker >/dev/null 2>&1; then
        fail "docker not installed; install Docker Engine to build the UI image"
        return 1
    fi
    local tag="${CLAWFT_UI_DOCKER_TAG:-clawft-ui:dev}"
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   docker build -f clawft-ui/Dockerfile -t %s .\n" "$tag"
    else
        (cd "$ROOT" && docker build -f clawft-ui/Dockerfile -t "$tag" .)
        local size
        size=$(docker image inspect "$tag" --format='{{.Size}}' 2>/dev/null)
        if [ -n "$size" ]; then
            local mb=$((size / 1024 / 1024))
            printf "  ${CYAN}SIZE${NC}  %s image: %d MB\n" "$tag" "$mb"
        fi
    fi
    timer_end
}

cmd_ui_e2e() {
    header "Running clawft-ui Playwright E2E suite"
    if [ ! -d "$ROOT/clawft-ui/tests" ]; then
        skip "clawft-ui/tests/ not found — Playwright suite not scaffolded"
        return 0
    fi
    if [ ! -d "$ROOT/clawft-ui/node_modules" ]; then
        printf "  ${CYAN}INFO${NC}  installing clawft-ui dependencies\n"
        if [ "$DRY_RUN" = true ]; then
            printf "  ${YELLOW}DRY${NC}   cd clawft-ui && npm ci\n"
        else
            (cd "$ROOT/clawft-ui" && npm ci --no-audit --no-fund)
        fi
    fi
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   cd clawft-ui && npx playwright install --with-deps chromium && npx playwright test\n"
    else
        (cd "$ROOT/clawft-ui" \
            && npx playwright install --with-deps chromium \
            && npx playwright test)
    fi
    timer_end
}

cmd_releases_mdx() {
    header "Regenerating docs releases.mdx from CHANGELOG.md"
    if [ ! -x "$ROOT/scripts/build-releases-mdx.sh" ]; then
        fail "scripts/build-releases-mdx.sh not found or not executable"
        return 1
    fi
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   scripts/build-releases-mdx.sh\n"
    else
        "$ROOT/scripts/build-releases-mdx.sh"
    fi
    timer_end
}

cmd_all() {
    header "Building everything"
    local failed=0
    cmd_native  || failed=$((failed + 1))
    cmd_wasi    || failed=$((failed + 1))
    cmd_browser || failed=$((failed + 1))
    cmd_ui      || failed=$((failed + 1))
    echo ""
    if [ "$failed" -gt 0 ]; then
        fail "$failed build(s) failed"
        return 1
    else
        pass "All builds succeeded"
    fi
}

# Run the workspace test suite. Prefers cargo-nextest: it runs each test in
# its own process, which eliminates the parallel-test-isolation flake class
# (tests sharing process-global env vars, statics, or the global tracing
# subscriber) and is faster than libtest across the whole workspace. nextest
# does not run doctests, so those get a separate `cargo test --doc` pass.
# Falls back to plain `cargo test` when cargo-nextest isn't installed
# (install: `curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C ~/.cargo/bin`).
workspace_test() {
    if command -v cargo-nextest >/dev/null 2>&1; then
        cargo nextest run --workspace && cargo test --workspace --doc
    else
        cargo test --workspace
    fi
}

cmd_test() {
    if command -v cargo-nextest >/dev/null 2>&1; then
        header "Running cargo nextest run --workspace (+ doctests)"
    else
        header "Running cargo test --workspace (cargo-nextest not installed)"
    fi
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   workspace test (nextest if available, else cargo test)\n"
    else
        # Always show full output — tail -5 hides test results
        workspace_test 2>&1
    fi
    timer_end
}

# Browser regression suite (WEFT-388 / M5-A).
#
# Builds + runs `crates/clawft-wasm/tests/browser_pipeline.rs` under
# headless Chrome via `wasm-pack test`. Requires:
#   - wasm-pack             (rustup component or cargo-installed)
#   - chromedriver matching the installed Chrome
#   - Chrome / Chromium     (linux: google-chrome; macOS: /Applications/.../Google Chrome)
# CI installs all three via the `wasm-browser-test` job in
# `.github/workflows/pr-gates.yml`.
#
# Override the browser via `--features` if you want firefox: this
# script defaults to chrome.
cmd_test_browser() {
    header "Running browser WASM regression suite (wasm-pack --headless --chrome)"
    if ! command -v wasm-pack >/dev/null 2>&1; then
        fail "wasm-pack not found — install via: cargo install wasm-pack"
        return 1
    fi
    if ! check_target_installed wasm32-unknown-unknown; then return 1; fi
    timer_start
    local args=(wasm-pack test --headless --chrome crates/clawft-wasm
                --no-default-features --features browser
                --test browser_pipeline)
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   %s\n" "${args[*]}"
        timer_end
        return 0
    fi
    # Always show full output — tail -5 hides per-test results from the runner.
    "${args[@]}" 2>&1
    local rc=$?
    timer_end
    return "$rc"
}

# Browser WASM bundle-size gate (WEFT-389 / M5-A).
#
# Runs `scripts/bench/check-bundle-size.sh` against the post-bindgen
# bundle. Default thresholds (raw 1600 KB / gz 600 KB) live in the
# script and are documented in `docs/architecture/wasm-bundle-size.md`.
# Override via `--features` style if you ever need to override here:
#   scripts/build.sh bundle-size 1500 550
cmd_bundle_size() {
    header "Browser WASM bundle-size gate"
    local pkg="$ROOT/crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm"
    if [ ! -f "$pkg" ]; then
        info "pkg/ not found — running scripts/build.sh browser first"
        cmd_browser || return 1
    fi
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   scripts/bench/check-bundle-size.sh\n"
        timer_end
        return 0
    fi
    bash "$ROOT/scripts/bench/check-bundle-size.sh" "$pkg" "$@"
    local rc=$?
    timer_end
    return "$rc"
}

# VSCode dev-panel wasm bundle (WEFT-484 / M6-B).
#
# Promotes `extensions/vscode-weft-panel/scripts/build-wasm.sh` into a
# first-class scripts/build.sh subcommand so the panel build pipeline
# is reachable from the same place as every other build target. The
# inner script handles:
#   - wasm-pack (preferred) or cargo + wasm-bindgen fallback
#   - wasm-opt -Oz (WEFT-246) with the rustc 1.93 feature flag union
#   - emission to extensions/vscode-weft-panel/webview/wasm/
#
# This wrapper additionally re-uses scripts/bench/check-bundle-size.sh
# (the same gate WEFT-389 uses for the clawft-wasm browser bundle) to
# enforce the documented panel budget. The clawft_gui_egui bundle is
# distinct from the clawft-wasm bundle and rides a separate budget;
# defaults are wider here because the panel ships eframe + egui_extras.
#
#   raw budget:  7600 KB  (current ~7300 KB after wasm-opt -Oz)
#   gz budget:   3500 KB  (current ~3400 KB after gzip -9)
#
# The budget was raised from the original WEFT-484 ceiling (4500/1500
# KB) after the M7+M7b feature wave landed: chat-panel markdown
# (egui_commonmark), terminal scrollback + glyph styling, canon
# Field::Date (jiff) + Field::Code, Workshop Grid/Tabs layouts, three
# new viewers (HealthViewer / SensorViewer / sparkline), tree filters,
# breadcrumb navigation, and Object Type registrations for Mesh /
# Sensor / Node. Trimming back toward 4500 / 1500 is tracked as a
# separate optimisation pass (twiggy + cargo bloat investigation,
# optional-dep audits, possible bundle splitting). Until that lands,
# the Cursor webview ships at ~7.3 MB raw / ~3.4 MB gz.
#
# Override by passing positional args: scripts/build.sh wasm-panel 7600 3500
cmd_wasm_panel() {
    header "Building VSCode dev-panel wasm bundle"
    local inner="$ROOT/extensions/vscode-weft-panel/scripts/build-wasm.sh"
    if [ ! -x "$inner" ]; then
        fail "inner build script missing or not executable: $inner"
        return 1
    fi
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   %s\n" "$inner"
        printf "  ${YELLOW}DRY${NC}   scripts/bench/check-bundle-size.sh <panel-bundle>\n"
        timer_end
        return 0
    fi
    if ! "$inner"; then
        fail "panel wasm build failed"
        timer_end
        return 1
    fi
    timer_end

    local bundle="$ROOT/extensions/vscode-weft-panel/webview/wasm/clawft_gui_egui_bg.wasm"
    if [ ! -f "$bundle" ]; then
        fail "expected bundle missing: $bundle"
        return 1
    fi
    report_binary_size "$bundle" "Panel WASM (post-opt)"

    # Size gate. Re-uses the clawft-wasm bundle gate against panel-specific
    # thresholds. Override via positional args:
    #   scripts/build.sh wasm-panel <max-raw-kb> <max-gz-kb>
    local max_raw_kb="${1:-}"
    local max_gz_kb="${2:-}"
    [ -z "$max_raw_kb" ] && max_raw_kb=7600
    [ -z "$max_gz_kb" ] && max_gz_kb=3500
    info "Panel size gate: raw≤${max_raw_kb}KB, gz≤${max_gz_kb}KB"
    if ! bash "$ROOT/scripts/bench/check-bundle-size.sh" \
            "$bundle" "$max_raw_kb" "$max_gz_kb"; then
        fail "panel bundle exceeds size budget"
        return 1
    fi
    pass "Panel wasm bundle ready at extensions/vscode-weft-panel/webview/wasm/"
}

cmd_check() {
    header "Running cargo check --workspace"
    timer_start
    run_cmd cargo check --workspace
    timer_end
}

cmd_clippy() {
    header "Running clippy (warnings as errors)"
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   cargo clippy --workspace -- -D warnings\n"
    else
        # Always show full output — tail -5 hides warnings
        cargo clippy --workspace -- -D warnings 2>&1
    fi
    timer_end
}

cmd_clean() {
    header "Cleaning build artifacts"
    run_cmd cargo clean
    if [ -d "$ROOT/clawft-ui/dist" ]; then
        info "Removing clawft-ui/dist"
        rm -rf "$ROOT/clawft-ui/dist"
    fi
    if [ -d "$ROOT/crates/clawft-wasm/www/pkg" ]; then
        info "Removing crates/clawft-wasm/www/pkg"
        rm -rf "$ROOT/crates/clawft-wasm/www/pkg"
    fi
    pass "Clean complete"
}

cmd_serve() {
    local port="${1:-8080}"
    local www_dir="$ROOT/crates/clawft-wasm/www"
    header "Serving browser test harness on http://localhost:$port"
    if [ ! -d "$www_dir/pkg" ]; then
        fail "www/pkg/ not found — run 'scripts/build.sh browser' first"
        return 1
    fi

    # Generate .env-keys.json from detected environment variables.
    # Keys are served locally only — never committed (gitignored).
    local keys_file="$www_dir/.env-keys.json"
    local found=0
    printf '{' > "$keys_file"
    local first=true
    for pair in \
        "OPENROUTER_API_KEY:openrouter" \
        "ANTHROPIC_API_KEY:anthropic" \
        "OPENAI_API_KEY:openai" \
        "DEEPSEEK_API_KEY:deepseek" \
        "GROQ_API_KEY:groq" \
        "GOOGLE_GEMINI_API_KEY:gemini" \
        "XAI_API_KEY:xai"; do
        local env_var="${pair%%:*}"
        local provider="${pair##*:}"
        local val="${!env_var:-}"
        if [ -n "$val" ]; then
            if [ "$first" = true ]; then first=false; else printf ',' >> "$keys_file"; fi
            printf '"%s":"%s"' "$provider" "$val" >> "$keys_file"
            info "Detected $env_var → providers.$provider"
            found=$((found + 1))
        fi
    done
    printf '}' >> "$keys_file"

    if [ "$found" -gt 0 ]; then
        pass "$found API key(s) injected into .env-keys.json (local only)"
    else
        info "No API keys detected in environment — textarea defaults will be used"
    fi

    # Clean up keys file on exit (Ctrl+C or normal stop).
    trap 'rm -f "$keys_file" 2>/dev/null; exit 0' INT TERM

    info "Open http://localhost:$port in your browser"
    info "API requests proxied via /proxy/ (avoids CORS)"
    python3 "$SCRIPT_DIR/dev_server.py" "$port" "$www_dir"
    rm -f "$keys_file" 2>/dev/null
}

# ── cargo-audit ignore list ─────────────────────────────────────────
# Advisories that are known and tracked as 0.8.x followups. Each
# `--ignore` carries the WEFT-N tracker so the next reviewer can map
# the ID back to the followup. See:
#   .planning/reviews/0.7.0-release-gate/audit-findings/cargo-audit-cold-run-2026-04-28.md
#
# When a followup lands, drop the matching IDs from this array.
#
# WEFT-551 — wasmtime 33 → 43 (15 advisories)
# WEFT-552 — rustls-webpki bump (3 advisories)
# WEFT-553 — unmaintained + unsound (6 advisories)
CARGO_AUDIT_IGNORES=(
    # WEFT-551 wasmtime 33.0.2
    --ignore RUSTSEC-2025-0118
    --ignore RUSTSEC-2026-0006
    --ignore RUSTSEC-2026-0020
    --ignore RUSTSEC-2026-0021
    --ignore RUSTSEC-2026-0085
    --ignore RUSTSEC-2026-0086
    --ignore RUSTSEC-2026-0087
    --ignore RUSTSEC-2026-0088
    --ignore RUSTSEC-2026-0089
    --ignore RUSTSEC-2026-0091
    --ignore RUSTSEC-2026-0092
    --ignore RUSTSEC-2026-0093
    --ignore RUSTSEC-2026-0094
    --ignore RUSTSEC-2026-0095
    --ignore RUSTSEC-2026-0096
    # WEFT-551 (cont.) wasmtime 33.0.2 — newly disclosed 2026-06; a real fix
    # needs the wasmtime 34+ major bump (out of in-range update). Deferred
    # with the rest of the wasmtime advisories. RUSTSEC-2026-0149 is HIGH
    # (WASI path_open TRUNCATE bypasses FilePerms::WRITE) — prioritize.
    --ignore RUSTSEC-2026-0114
    --ignore RUSTSEC-2026-0149
    --ignore RUSTSEC-2026-0182
    # WEFT-552 rustls-webpki
    --ignore RUSTSEC-2026-0098
    --ignore RUSTSEC-2026-0099
    --ignore RUSTSEC-2026-0104
    # WEFT-553 unmaintained + unsound
    --ignore RUSTSEC-2017-0008
    --ignore RUSTSEC-2024-0384
    --ignore RUSTSEC-2024-0436
    --ignore RUSTSEC-2025-0134
    --ignore RUSTSEC-2025-0141
    --ignore RUSTSEC-2026-0097
)

cmd_audit() {
    header "Running cargo audit (with 0.7.0 ignore-list)"
    if ! command -v cargo-audit >/dev/null 2>&1; then
        fail "cargo-audit not installed — run: cargo install --locked cargo-audit"
        return 1
    fi
    timer_start
    if [ "$DRY_RUN" = true ]; then
        printf "  ${YELLOW}DRY${NC}   cargo audit %s\n" "${CARGO_AUDIT_IGNORES[*]}"
    else
        # Fail on any new vulnerability or warning that is NOT in the ignore-list.
        cargo audit --deny warnings "${CARGO_AUDIT_IGNORES[@]}" 2>&1
    fi
    timer_end
}

# ── Gate: full phase-gate checks ────────────────────────────────────
cmd_gate() {
    header "Phase Gate — 12 checks"
    local total=12 passed=0 failed=0 skipped=0

    run_gate_check() {
        local num="$1" label="$2"
        shift 2
        printf "\n${BOLD}[%2d/%d]${NC} %s\n" "$num" "$total" "$label"
        timer_start
        if [ "$DRY_RUN" = true ]; then
            printf "  ${YELLOW}DRY${NC}   %s\n" "$*"
            passed=$((passed + 1))
        elif "$@" >/dev/null 2>&1; then
            pass "$label"
            passed=$((passed + 1))
        else
            fail "$label"
            failed=$((failed + 1))
        fi
        timer_end
    }

    run_gate_check_soft() {
        local num="$1" label="$2"
        shift 2
        printf "\n${BOLD}[%2d/%d]${NC} %s\n" "$num" "$total" "$label"
        timer_start
        if [ "$DRY_RUN" = true ]; then
            printf "  ${YELLOW}DRY${NC}   %s\n" "$*"
            passed=$((passed + 1))
        elif "$@" >/dev/null 2>&1; then
            pass "$label"
            passed=$((passed + 1))
        else
            skip "$label (not yet available)"
            skipped=$((skipped + 1))
        fi
        timer_end
    }

    # 1. Workspace tests — nextest (per-test process isolation, kills the
    #    parallel-isolation flake class) + doctests when available, else cargo test
    run_gate_check 1 "workspace tests (nextest + doctests)" \
        workspace_test

    # 2. Release binaries (weft + weave)
    run_gate_check 2 "cargo build --release --bin weft --bin weaver" \
        cargo build --release --bin weft --bin weaver

    # 3. WASI WASM
    if check_target_installed wasm32-wasip2; then
        run_gate_check 3 "WASI WASM (wasm32-wasip2)" \
            cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm
    else
        printf "\n${BOLD}[%2d/%d]${NC} %s\n" 3 "$total" "WASI WASM (wasm32-wasip2)"
        skip "wasm32-wasip2 target not installed"
        skipped=$((skipped + 1))
    fi

    # 4–9. Browser WASM checks per crate
    local browser_crates=(clawft-types clawft-platform clawft-core clawft-llm clawft-tools clawft-wasm)
    local gate_num=4
    if check_target_installed wasm32-unknown-unknown; then
        for crate in "${browser_crates[@]}"; do
            run_gate_check_soft "$gate_num" "Browser WASM: $crate" \
                cargo check --target wasm32-unknown-unknown -p "$crate" --no-default-features --features browser
            gate_num=$((gate_num + 1))
        done
    else
        for crate in "${browser_crates[@]}"; do
            printf "\n${BOLD}[%2d/%d]${NC} %s\n" "$gate_num" "$total" "Browser WASM: $crate"
            skip "wasm32-unknown-unknown target not installed"
            skipped=$((skipped + 1))
            gate_num=$((gate_num + 1))
        done
    fi

    # 10. UI build
    if [ -d "$ROOT/clawft-ui" ] && [ -f "$ROOT/clawft-ui/package.json" ]; then
        printf "\n${BOLD}[%2d/%d]${NC} %s\n" 10 "$total" "UI build (tsc + vite)"
        timer_start
        if [ "$DRY_RUN" = true ]; then
            printf "  ${YELLOW}DRY${NC}   cd clawft-ui && npm run build\n"
            passed=$((passed + 1))
        elif (cd "$ROOT/clawft-ui" && npm run build) >/dev/null 2>&1; then
            pass "UI build"
            passed=$((passed + 1))
        else
            fail "UI build"
            failed=$((failed + 1))
        fi
        timer_end
    else
        printf "\n${BOLD}[%2d/%d]${NC} %s\n" 10 "$total" "UI build"
        skip "clawft-ui/ directory not found"
        skipped=$((skipped + 1))
    fi

    # 11. Voice feature
    run_gate_check_soft 11 "Voice feature (clawft-plugin)" \
        cargo check --features voice -p clawft-plugin

    # 12. cargo audit (deny warnings, with 0.7.0 ignore-list).
    # See CARGO_AUDIT_IGNORES + cmd_audit above. Soft check: if
    # cargo-audit isn't installed locally, skip rather than fail; CI
    # always installs it (see .github/workflows/pr-gates.yml). When
    # WEFT-551/552/553 land, drop the matching IDs from
    # CARGO_AUDIT_IGNORES so this check tightens.
    if command -v cargo-audit >/dev/null 2>&1; then
        run_gate_check 12 "cargo audit (deny warnings, 0.7.0 ignores)" \
            cargo audit --deny warnings "${CARGO_AUDIT_IGNORES[@]}"
    else
        printf "\n${BOLD}[%2d/%d]${NC} %s\n" 12 "$total" "cargo audit (deny warnings)"
        skip "cargo-audit not installed — run: cargo install --locked cargo-audit"
        skipped=$((skipped + 1))
    fi

    # Summary
    echo ""
    printf "${BOLD}═══════════════════════════════════════${NC}\n"
    printf "  ${GREEN}PASSED${NC}: %d  " "$passed"
    if [ "$failed" -gt 0 ]; then
        printf "${RED}FAILED${NC}: %d  " "$failed"
    else
        printf "FAILED: %d  " "$failed"
    fi
    if [ "$skipped" -gt 0 ]; then
        printf "${YELLOW}SKIPPED${NC}: %d" "$skipped"
    else
        printf "SKIPPED: %d" "$skipped"
    fi
    printf "  (total: %d)\n" "$total"
    printf "${BOLD}═══════════════════════════════════════${NC}\n"

    if [ "$failed" -gt 0 ]; then
        return 1
    fi
}

# ── Usage ────────────────────────────────────────────────────────────
usage() {
    cat <<EOF
${BOLD}Usage:${NC} scripts/build.sh <command> [options]

${BOLD}Commands:${NC}
  native          Build native CLI binary (release)
  native-debug    Build native CLI binary (debug, fast)
  gui-egui        Build native egui GUI binary (weft-gui-egui, requires --features native)
  wasi            Build WASM for WASI (wasm32-wasip2)
  browser         Build WASM for browser (wasm32-unknown-unknown)
  ui              Build React frontend (tsc + vite)
  ui-docker       Build the clawft-ui multi-stage Docker image (WEFT-317).
                  Override tag with CLAWFT_UI_DOCKER_TAG=...
  ui-e2e          Run the clawft-ui Playwright E2E suite (WEFT-314).
                  Installs npm deps + chromium on first run.
  releases-mdx    Regenerate docs/src/content/docs/weftos/vision/releases.mdx
                  from CHANGELOG.md (also runs as --check before commits)
  all             Build everything (native + wasi + browser + ui)
  test            Run cargo test --workspace
  test-browser    Run browser WASM regression suite under headless Chrome
                  (WEFT-388 / M5-A). Requires wasm-pack + chromedriver.
  bundle-size     Gate browser WASM bundle (raw + gzip) against the
                  documented budget (WEFT-389 / M5-A).
                  See docs/architecture/wasm-bundle-size.md
  wasm-panel      Build the VSCode dev-panel wasm bundle (clawft-gui-egui)
                  via wasm-pack / cargo + wasm-bindgen + wasm-opt -Oz, then
                  gate against the panel size budget. (WEFT-484 / M6-B)
                  Override budget: scripts/build.sh wasm-panel <max-raw-kb> <max-gz-kb>
  check           Run cargo check --workspace (fast compile check)
  clippy          Run clippy with warnings-as-errors
  audit           Run cargo audit with 0.7.0 ignore-list (deny warnings).
                  Requires: cargo install --locked cargo-audit
                  Followups: WEFT-551 (wasmtime), WEFT-552 (rustls-webpki),
                  WEFT-553 (unmaintained + unsound rand).
  gate            Run full phase gate (12 checks, includes cargo audit)
  serve [port]    Serve browser test harness (default: 8080)
  clean           Clean all build artifacts

${BOLD}Options:${NC}
  --features <f>  Extra features to enable (e.g. --features voice,channels)
  --profile <p>   Cargo profile: debug, release, release-wasm (default varies)
  --force, -f     Force rebuild even if artifacts are up-to-date
  --verbose       Show full cargo output
  --dry-run       Print commands without executing
  --help          Show this help

${BOLD}Examples:${NC}
  scripts/build.sh native                          # Release CLI binary
  scripts/build.sh native --features voice          # CLI with voice
  scripts/build.sh gui-egui                         # Native egui GUI (release)
  scripts/build.sh gui-egui --profile debug         # Native egui GUI (debug)
  scripts/build.sh browser                          # Browser WASM
  scripts/build.sh gate                             # Full phase gate
  scripts/build.sh native --dry-run                 # Preview commands
  scripts/build.sh wasi --force                      # Force WASI rebuild
  scripts/build.sh browser && scripts/build.sh serve # Build + serve test harness
EOF
}

# ── Argument parsing ─────────────────────────────────────────────────
parse_args() {
    if [ $# -eq 0 ]; then
        usage
        exit 0
    fi

    COMMAND="$1"
    shift

    # Capture positional arg for serve command (port number)
    if [ "$COMMAND" = "serve" ] && [ $# -gt 0 ] && [[ "$1" =~ ^[0-9]+$ ]]; then
        SERVE_PORT="$1"
        shift
    fi

    # Capture optional positional budget overrides for wasm-panel:
    #   scripts/build.sh wasm-panel [<max-raw-kb> [<max-gz-kb>]]
    if [ "$COMMAND" = "wasm-panel" ]; then
        if [ $# -gt 0 ] && [[ "$1" =~ ^[0-9]+$ ]]; then
            WASM_PANEL_MAX_RAW_KB="$1"
            shift
            if [ $# -gt 0 ] && [[ "$1" =~ ^[0-9]+$ ]]; then
                WASM_PANEL_MAX_GZ_KB="$1"
                shift
            fi
        fi
    fi

    while [ $# -gt 0 ]; do
        case "$1" in
            --features)
                FEATURES="${2:?'--features requires a value'}"
                shift 2
                ;;
            --profile)
                PROFILE="${2:?'--profile requires a value'}"
                shift 2
                ;;
            --force|-f)
                FORCE=true
                shift
                ;;
            --verbose)
                VERBOSE=true
                shift
                ;;
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            --help|-h)
                usage
                exit 0
                ;;
            *)
                printf "${RED}Unknown option: %s${NC}\n" "$1"
                usage
                exit 1
                ;;
        esac
    done
}

# ── Main ─────────────────────────────────────────────────────────────
main() {
    parse_args "$@"

    case "$COMMAND" in
        native)       cmd_native ;;
        native-debug) cmd_native_debug ;;
        gui-egui)     cmd_gui_egui ;;
        wasi)         cmd_wasi ;;
        browser)      cmd_browser ;;
        ui)           cmd_ui ;;
        ui-docker)    cmd_ui_docker ;;
        ui-e2e)       cmd_ui_e2e ;;
        releases-mdx) cmd_releases_mdx ;;
        all)          cmd_all ;;
        test)         cmd_test ;;
        test-browser) cmd_test_browser ;;
        bundle-size)  cmd_bundle_size ;;
        wasm-panel)   cmd_wasm_panel "${WASM_PANEL_MAX_RAW_KB:-}" "${WASM_PANEL_MAX_GZ_KB:-}" ;;
        check)        cmd_check ;;
        clippy)       cmd_clippy ;;
        audit)        cmd_audit ;;
        gate)         cmd_gate ;;
        serve)        cmd_serve "$SERVE_PORT" ;;
        clean)        cmd_clean ;;
        --help|-h)    usage ;;
        *)
            printf "${RED}Unknown command: %s${NC}\n" "$COMMAND"
            usage
            exit 1
            ;;
    esac
}

main "$@"
