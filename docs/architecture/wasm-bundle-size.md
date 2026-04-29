# Browser WASM Bundle Size — Budget & Audit (M5-A / WEFT-389)

## tl;dr

| Metric           | Current | Budget  | Headroom |
|------------------|---------|---------|----------|
| Raw `_bg.wasm`   | 1.34 MB | 1.6 MB  | ~260 KB  |
| Gzipped (-9)     | 470 KB  | 600 KB  | ~130 KB  |

The CI gate measures `crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm`
(post-`wasm-bindgen`) and trips when either threshold is exceeded.
Gzipped size is the load-bearing metric — it's what the browser
actually downloads — but raw size is reported for context. Run it
locally with `scripts/build.sh bundle-size`.

## How we got here

The original W-BROWSER planning doc set a target of **<300 KB raw /
<120 KB gzipped**. That number came from the BW1 design phase before
we knew what the agent loop would weigh; it predated the BW2/BW3 work
that pulled the full pipeline (`AgentLoop`, `BrowserPlatform`,
`BrowserLlmClient`), the per-provider builtin registry, the tool
registry (file, web-fetch, web-search, memory), `chrono`,
`async-trait`, `serde_json` + `serde_yaml`, `reqwest` (browser flavor),
and `wasm-bindgen` glue into the bundle.

WEFT-246 (M4-C) landed `wasm-opt -Oz` post-bindgen, dropping the bundle
from ~6.9 MB to 6.46 MB on the GUI-egui artifact. For the
`clawft-wasm` artifact specifically, the same optimization run takes
the bindgen output from ~1.79 MB raw to ~1.24 MB raw / ~488 KB
gzipped, but the size that the VSCode panel and `weftos.weavelogic.ai`
playground actually load is the **post-bindgen / pre-opt** bytes
(1.34 MB raw / 470 KB gzipped). That's the number this gate measures.

The original 300 KB target is unreachable without ripping out the
agent loop, the LLM provider registry, the tool stack, and the
playground features — none of which are scope-cuttable for 0.7.0. The
revised budget below is calibrated against the shipped feature set
with realistic headroom for the 0.8.x channel + skill work.

## Per-crate breakdown (twiggy on unstripped 3.92 MB pre-bindgen)

Aggregated by crate prefix; debug sections account for ~46% of the raw
unstripped wasm and are stripped by `wasm-bindgen` and `wasm-opt -Oz`.

| Bytes (KB) | Functions | Crate prefix              | Notes |
|------------|-----------|---------------------------|-------|
| 115 KB     | 97        | `clawft_core`             | Pipeline + agent loop |
| 115 KB     | 285       | `core`                    | Rust std core (formatting, panic, fmt) |
| 47 KB      | 183       | `alloc`                   | Vec, String, Box machinery |
| 43 KB      | 134       | `hashbrown`               | HashMap impl behind std |
| 41 KB      | 42        | `unsafe_libyaml`          | Pulled in by `serde_yaml` (config parse) |
| 37 KB      | 118       | `serde_json`              | Pipeline + entry-point JSON |
| 30 KB      | 10        | `clawft_wasm`             | Browser entry surface |
| 25 KB      | 72        | `clawft_types`            | Shared types |
| 24 KB      | 29        | `http`                    | Pulled by `reqwest` |
| 22 KB      | 32        | `url`                     | Pulled by `reqwest` |
| 20 KB      | 46        | `chrono`                  | Timestamps in InboundMessage |
| 18 KB      | 44        | `serde_yaml`              | Config parse |
| 17 KB      | 51        | `std`                     | Std machinery |
| 16 KB      | 63        | `serde_core`              | serde derive runtime |
| 12 KB      | 20        | `dlmalloc`                | Allocator (target dep) |
| 12 KB      | 10        | `idna`                    | Pulled by `url`/`reqwest` |
| 7-8 KB     | 13-21     | `clawft_tools`, `clawft_llm`, `wasm_bindgen_futures` | |

## Reduction opportunities (deferred to 0.8.x)

These each warrant a dedicated tracker once the 0.7.0 release ships
and the bundle baseline is locked. None are in scope for M5-A.

1. **`serde_yaml` + `unsafe_libyaml` (~60 KB)**  
   The browser entry takes the runtime config as **JSON**. The yaml
   crates leak into the bundle through `clawft-types` deserializing
   `ProviderConfig` etc. Gating `serde_yaml` behind a `yaml-config`
   feature in `clawft-types` and toggling it off for the `browser`
   feature would drop both. Estimated saving: ~60 KB raw.
2. **`reqwest` browser machinery (~50 KB across http/url/idna/wasm-streams)**  
   The browser path uses `reqwest` only for the wasm `Fetch` adapter
   in `clawft-llm`'s `BrowserLlmClient`. Replacing it with a thin
   `web-sys::Fetch` wrapper (already partially present in
   `clawft-platform/src/browser/http.rs`) would let us drop the
   `reqwest` dep on the browser path entirely.
3. **`chrono` (~20 KB)**  
   Only used for the `InboundMessage::timestamp` field. Replacing
   with `js_sys::Date::now()` formatted via `format!` saves the
   crate. Requires touching the cross-crate `InboundMessage` type.
4. **Debug-only code paths**  
   The `analyze_files` and `boot_info` entry points add ~10 KB
   combined; the playground use case is not load-bearing for the
   VSCode panel. Splitting them behind a `playground` feature flag
   would let production embeds drop ~10 KB.
5. **`tracing` runtime**  
   The agent pipeline emits `tracing::debug!` everywhere; the
   subscriber is never installed in the browser, but the macro
   expansion still emits the call sites. Building with `RUSTFLAGS=
   "--cfg tracing_unstable"` + `tracing/release_max_level_warn` cuts
   another ~5-15 KB.

Estimated upper bound from all five: ~150 KB raw / ~60-80 KB gzipped.
That would put us at ~1.2 MB raw / ~390 KB gzipped — still nowhere
near the original 300 KB / 120 KB target, but a meaningful win without
amputating features.

## Why not the original 300 KB target

The original number was based on the `clawft-wasm` "shim-only"
ambition (a few hundred lines of glue calling `clawft-types`
serializers). Once W-BROWSER expanded the goal to "real
`AgentLoop<BrowserPlatform>` running the full 6-stage pipeline" (audit
2026-04-28, `.planning/reviews/0.7.0-release-gate/16-browser-wasm.md`),
the bundle now legitimately includes:

- `AgentLoop` + `MessageBus` + `SessionManager`.
- All six pipeline stages (classifier, router, assembler, transport, scorer, learner).
- Builtin LLM provider registry (Anthropic, OpenAI, OpenRouter, DeepSeek, Groq, Gemini, xAI).
- Tool registry (file, web-fetch, web-search, memory) with `UrlPolicy` SSRF guards.
- `BrowserPlatform`: `Fetch` HTTP, in-memory FS, in-memory env.

The 1.34 MB raw / 470 KB gzipped number reflects that scope. Any
0.7.0 budget tighter than ~1.6 MB raw would just trip on routine
feature work; the 0.8.x reductions above can be re-litigated as a
proper budget tightening story once they land.

## How to run the gate

Local:
```bash
scripts/build.sh browser       # produces www/pkg/clawft_wasm_bg.wasm
scripts/build.sh bundle-size   # measures + gates
```

Or directly:
```bash
scripts/bench/check-bundle-size.sh \
  crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm \
  1600 600   # max-raw-kb max-gz-kb
```

CI: the gate runs in `.github/workflows/pr-gates.yml` under the
`browser-wasm-bundle-size` job (added with WEFT-389). The job builds
the bundle with `scripts/build.sh browser`, runs
`scripts/bench/check-bundle-size.sh`, posts a Markdown summary to the
GitHub Actions step summary, and fails the PR if either threshold
trips.

## Adjusting the budget

Both numbers live in `scripts/bench/check-bundle-size.sh` as
`DEFAULT_MAX_RAW_KB` and `DEFAULT_MAX_GZ_KB`. When you legitimately
ship a bundle-growing feature:

1. Confirm the growth is justified (run `twiggy top` to verify the
   contributor — it should be a known feature crate, not stale debug
   code).
2. Update both constants in the script.
3. Update the table at the top of this file.
4. Note the change in `CHANGELOG.md` under the relevant version.

## Tooling references

- **`twiggy top`** — per-symbol size breakdown (works on stripped + unstripped).
- **`cargo bloat --crates`** — per-crate aggregation (only works against bin/dylib targets, not cdylib).
- **`scripts/bench/wasm-twiggy.sh`** — wrapper for the WASI-target wasm twiggy run.
- **`scripts/build/wasm-opt.sh`** — wasm-opt -Oz post-pass (used by VSCode panel build).
