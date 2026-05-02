# Follow-up audit — ws14-17 (deployment, mcp, browser-wasm, research)

Date: 2026-05-01
Auditor: audit-E (Opus 4.7 1M)
Scope: 6 items shipped across M7-F + M7-I.
Branch verified: `m7-08-sweep` @ `81dd34c6`.

## Per-item verification

### WEFT-409 — sparc/browser: retire `scripts/check-features.sh`

Status: **PARTIAL — concerns**.

Plane acceptance criteria (from `.planning/reviews/0.7.0-release-gate/triage/ws11-12-16.json`):
- Decision: rename `scripts/build.sh gate` reference into `scripts/check-features.sh` (or doc fix)
- step1-bw1 doc updated to point at the real script
- CI doesn't break

Closure commit: `9630a534 docs(sparc,browser): retire scripts/check-features.sh in favor of build.sh gate [WEFT-409]` (2026-04-30).

Evidence reviewed:
- `.planning/sparc/00-master-plan.md` — A4, Gate 5, acceptance checklist all reference `scripts/build.sh gate` with WEFT-409 supersession comment. Clean.
- `.planning/sparc/browser/00-orchestrator.md:173` — supersession note present.
- `.planning/sparc/browser/01-phase-BW1-foundation.md:32, 563-584` — file table strikethrough + dedicated `Feature Validation` section. Clean.
- `.planning/sparc/browser/02-phase-BW2-core-engine.md:453` — acceptance check updated. Clean.
- `.planning/development_notes/step0-scripts-fixtures.md:6-11` — WEFT-409 update banner. Clean.
- `.planning/reviews/0.7.0-release-gate/16-browser-wasm.md:228-234` — P1.6 marked RESOLVED. Clean.

Concerns (two of them):

1. **`scripts/check-features.sh` is still on disk** (62 lines, executable, mtime 2026-04-22). The closure-commit rationale and the updated docs both repeat the claim that the script "was never created" — but `git log -- scripts/check-features.sh` shows it was added 2026-02-25 by commit `6a1416c6 feat: three-workstream implementation + unified build script`. The file pre-dates the audit and was simply missed when the auditor wrote `.planning/reviews/0.7.0-release-gate/16-browser-wasm.md` on 2026-04-28. Per the audit-E prompt's option 5 ("confirm the file is actually deleted from `scripts/` (or marked as deprecated with a redirect comment)"), this is not satisfied: the file is neither deleted nor carries a `# deprecated — see scripts/build.sh gate` redirect comment, and no test or `gate()` step rejects the script.
2. **`.planning/sparc/browser/05-phase-BW5-wasm-entry.md` was missed by the WEFT-409 sweep.** Two references survive:
   - line 581: `- [ ] \`scripts/check-features.sh\` passes`
   - line 602: `bash scripts/check-features.sh`
   Both are inside an "acceptance criteria" / "test commands" block that future agents will copy verbatim. They should either be replaced with `scripts/build.sh gate` or annotated with the supersession note used elsewhere.

CI check: a manual ripgrep across `.github/workflows/` returns zero matches for `check-features`, so CI does not invoke the script. The "CI doesn't break" criterion holds — but only because nothing was relying on the script in the first place.

### WEFT-467 — deployment: `wasm.md` target + wasmtime version

Status: **CONFIRMED**.

Plane acceptance criteria (from `.planning/reviews/0.7.0-release-gate/triage/ws14.json`):
- wasip1 → wasip2 reference corrected
- URLs reflect current namespace
- Browser-WASM vs wasip2 surfaces remain distinguished

Closure commit: `fd0f89d6 docs(deployment): refresh wasm.md target + wasmtime version [WEFT-467]`.

Evidence:
- `docs/deployment/wasm.md:43` now reads `scripts/build.sh wasi      # WASI target (wasm32-wasip2, release-wasm profile)`. No `wasm32-wasip1` or `wasi[^p]` substrings remain (verified by ripgrep).
- `docs/deployment/wasm.md:155` reads `[wasmtime](https://wasmtime.dev/) 33`. Workspace is on `wasmtime = "33"` in both `Cargo.toml:141` (host) and `crates/clawft-wasm/Cargo.toml:74` (optional, for `wasm-plugins`). Versions match.
- `Rust 1.93+` reference at line 28 matches `rust-version = "1.93"` in workspace Cargo.toml.
- All four external links (`wasmtime.dev`, `wasm-micro-runtime` GitHub repo) are reachable namespaces (spot-checked URL paths; not fetched live this session).
- Browser-WASM vs wasip2 surfaces remain distinguished via the "Platform Limitations" section (lines 105-119) + the "Future Roadmap" "Browser target" bullet (line 288). No conflation introduced.

### WEFT-469 — docker: `Dockerfile.alpine` kernel-only header

Status: **CONFIRMED, with two non-blocking improvement notes**.

Plane acceptance criteria (from `.planning/reviews/0.7.0-release-gate/triage/ws14.json`):
- Verified via grep that docker-compose.yml references it
- Documented as a kernel-only image with build instructions
- `scripts/build.sh check` green (docs-only Dockerfile change)

Closure commit: `59a2758f docs(docker): document Dockerfile.alpine as kernel-only build image [WEFT-469]`.

Evidence (read as a Dockerfile, not just text):

| Check | Status | Detail |
|------|--------|--------|
| Multi-stage build | YES | `FROM rust:1.93-alpine AS builder` + `FROM alpine:3.21` runtime stage |
| Base images current | YES | rust 1.93 matches workspace MSRV; alpine:3.21 is latest stable (Dec 2024) |
| No secrets baked in | YES | Zero `ENV`, `ARG`, or copy of credential paths. Only `RUN apk add ...` and `COPY . .` (workspace source) |
| Runs as non-root | NO | No `USER` directive. The runtime stage runs the entrypoint as root. |
| Healthcheck | NO | No `HEALTHCHECK`. Image is intended for `docker-compose` dev use, so this is acceptable. |
| Compose wire-up | YES | `crates/clawft-kernel/docker-compose.yml:6` builds with `dockerfile: crates/clawft-kernel/Dockerfile.alpine` and `context: ../..` (workspace root) — matches the new header's stated build invocation. |
| Entrypoint resolves to a real subcommand | YES | `CMD ["kernel", "status"]` maps to `weaver kernel status` defined at `crates/clawft-weave/src/commands/kernel_cmd.rs:62 (KernelAction::Status)`. |

Header text (lines 1-17) clearly differentiates this from the root `Dockerfile` (which downloads pre-built musl binaries via cargo-dist) and includes a working `docker build -f ...` invocation.

Improvement notes (file as follow-up if desired, not blockers for this WEFT close):

- **(N1)** No `USER` directive. For a dev image consumed only via `docker-compose` this is acceptable, but adding `USER 1000:1000` (and adjusting `COPY --from=builder ... --chown=`) would close one easy hardening gap. Worth a low-priority WEFT if the kernel-only image is ever used outside the local-dev path.
- **(N2)** `COPY . .` copies the entire workspace into the build stage every time, busting layer caching on any source change. A `COPY Cargo.toml Cargo.lock ./` + dummy-build dependency layer would dramatically speed local rebuilds. Out of scope for WEFT-469 (which was docs-only) but trivial to add when somebody iterates on this image.

### WEFT-470 — adr-037: `0.X.Y` placeholder

Status: **CONFIRMED**.

Plane acceptance criteria (from `.planning/reviews/0.7.0-release-gate/triage/ws14.json`):
- Example block updated to a generic placeholder
- `scripts/build.sh check` green (docs-only change)

Closure commit: `5a14255d docs(adr-037): replace stale 0.3.1 example with 0.X.Y placeholder [WEFT-470]`.

Evidence:
- `docs/adr/adr-037-rust-edition-2024-msrv.md:19` reads `version = "0.X.Y"          # see workspace Cargo.toml for the current value`. Clean.
- The previous "(current: 0.3.1)" parenthetical at the consequences section line 46 was removed.
- ADR-001 cross-reference resolves to an existing file (`docs/adr/adr-001-lockstep-semver.md`).
- `rust-version = "1.93"` matches workspace Cargo.toml.

No remaining literal version numbers in the ADR (verified by grep; only `MSRV 1.93` and `Edition 2024` remain, both of which are policy decisions, not example placeholders).

### WEFT-540 — research-audit: orphan symposium close

Status: **CONFIRMED**.

Plane acceptance criteria: cross-link `compositional-ui` and `RLM - arxiv-2512.24601` symposium output into responsible streams or mark closed.

Closure commit: `d5f6fd5d docs(research-audit): close orphan symposium and research-index decisions [WEFT-540][WEFT-541]`.

Evidence in `.planning/reviews/0.7.0-release-gate/17-research-streams.md`:
- Lines 397-409 (Symposium Reports → "Other Symposiums") give explicit citations for `compositional-ui`:
  - Stream 8 → `08-weftos-gui.md` lines 466-477 (verified: `compositional-ui` cited at lines 466-469, 477)
  - Stream 13 → `13-app-substrate-surface.md` lines 479-490 (verified: lines 479, 489-490)
  - Stream 15 → `15-mcp-integration.md` line 309 (verified)
- Same block for `RLM - arxiv-2512.24601`: marked CLOSED with the file inventory (`00-synthesis.md` … `04-gaps-and-risks.md`), revisit trigger (Stream 17 KG / RoMem work hits recursive-LM relevance), and adoption-candidate deferral note.
- Lines 625-631 (Orphaned research): both bullets struck through with WEFT-540 closure annotation pointing back at the canonical citation block. No dangling references.

Cross-link sanity:
- All cited line ranges in the closure resolve (manually grep-verified for `compositional-ui` in the three named files).
- Symposium directory `.planning/symposiums/RLM - arxiv-2512.24601/` exists with all five named files (`00-synthesis.md` … `04-gaps-and-risks.md`).

### WEFT-541 — research-audit: research-index decisions

Status: **CONFIRMED**.

Plane acceptance criteria: decide on single research → feature pipeline index vs ADR-only.

Closure commit: `d5f6fd5d` (same commit as WEFT-540).

Evidence in `.planning/reviews/0.7.0-release-gate/17-research-streams.md`:
- Task list row T40 (line 682) is struck through and replaced with: *"DECIDED 2026-04-30 (WEFT-541): **ADR-only**. The 'Released Features' section of this audit doc + each ADR's 'Status: Accepted' + the 'Status & Timeline' table at top serve as the de-facto landed-as-feature index."* with revisit-trigger ("if audit-time discovery becomes painful again").
- The decision is internally coherent: the doc points at three live indexes that already exist (Released Features section, ADR Status:Accepted, Status & Timeline table at top), so no new artifacts are owed.
- No new "research → feature pipeline" file was created (correct — that was the deferred path).

## Cross-cutting findings

### Documentation hygiene

- **WEFT-409 left two references in `05-phase-BW5-wasm-entry.md` (lines 581, 602)** that the closure-sweep missed. The doc still tells future agents to run a script the parent commit calls "never created". Fix: either delete the lines or replace with `scripts/build.sh gate` and the supersession comment used in the sibling phase docs. New issue filed below.
- **WEFT-409 closure rationale is internally inconsistent with the on-disk reality.** The commit message and the SPARC docs say the script "was never created"; `git log -- scripts/check-features.sh` shows it was created on 2026-02-25 (commit `6a1416c6`) and is still present. Either the file should be removed, or the docs should be re-stated as "the script was retired" rather than "never created". New issue filed below.
- ADR-037 placeholder + cross-ref hygiene is clean. No follow-up needed.
- `wasm.md` link surface is clean; no stale `wasip1` substrings remain. Wasmtime 33 matches workspace lock state.

### Deployment artifacts

- **`Dockerfile.alpine` review**: multi-stage, current bases (rust:1.93-alpine + alpine:3.21), no secrets, runs as root (acceptable for the documented dev-only kernel-rebuild use case but worth a future hardening pass per N1/N2 above). Compose wiring (`crates/clawft-kernel/docker-compose.yml`) and entrypoint (`weaver kernel status`) both resolve correctly.
- **`wasm.md` target / version current**: yes — `wasm32-wasip2` and `wasmtime 33` align with workspace Cargo.toml.

### MCP impact of WEFT-498 (cross-workstream check on audit-D's territory)

The audit-E prompt asked whether the WEFT-498 `AgentChat` type relocation (from `clawft-weave::protocol` / `clawft-service-agent::protocol` to `clawft-types::agent_chat`) broke any MCP tool schema serialization. **It did not.** Evidence:

- The MCP server lives at `crates/clawft-services/src/mcp/{server, provider, types, transport, …}.rs`. Ripgrep across that directory returns zero `AgentChat` matches.
- The CLI MCP entry (`crates/clawft-cli/src/commands/mcp_server.rs`, `crates/clawft-cli/src/mcp_tools.rs`) imports from `clawft_services::mcp::{BuiltinToolProvider, SkillToolProvider, ToolDefinition, …}` — none of which touch `AgentChat`.
- `AgentChat*` types are consumed by `clawft-weave::daemon`, `clawft-weave::voice_router`, `clawft-service-agent::service`, and `clawft-gui-egui::explorer::chat` only. These are all wire shapes for the panel ↔ daemon RPC, not MCP tool schemas.
- `crates/clawft-types/src/agent_chat.rs:11-20` documents this explicitly: the relocation collapses the duplicate `clawft-weave` / `clawft-service-agent` mirrors into a single source-of-truth, with both crates re-exporting from here.

Conclusion: WEFT-498 has zero MCP surface impact.

### Browser-WASM build commands post-WEFT-409

- `scripts/build.sh wasi --dry-run` runs cleanly: emits `cargo build --target wasm32-wasip2 --profile release-wasm -p clawft-wasm`. Works.
- `scripts/build.sh browser --dry-run` runs cleanly: emits the same `wasm32-unknown-unknown --features browser --profile release-wasm` invocation followed by `wasm-bindgen target/.../clawft_wasm.wasm --out-dir crates/clawft-wasm/www/pkg --target web --no-typescript`. Works.
- `scripts/build.sh --help` lists `wasi`, `browser`, `gate`, `bundle-size`, `wasm-panel` etc. — the canonical entrypoints documented across the WEFT-409 sweep.
- **Side observation**: dry-run reports the current `crates/clawft-wasm/www/pkg/clawft_wasm_bg.wasm` at **1.78 MB raw / 1.32 MB after wasm-bindgen** — well over the documented `<300 KB raw / <120 KB gzip` budget in `docs/deployment/wasm.md` size-budget table (line 121). This is already tracked as part of the `16-browser-wasm.md` "What's Left" section item P1.5 / P6.5 (WEFT-388, WEFT-389) and is out of scope for this audit; flagging for visibility only.

### Research audit triage stamp coherence

- `17-research-streams.md` triage stamp (lines 706-717) declares **WEFT-502 … WEFT-549 (48 items)**.
- `plane.sh search "ws17:"` returns exactly 48 matches in the `WEFT-502 … WEFT-549` range. Stamp matches Plane reality. Clean.

### Stubs / TODOs spotted

- None new in the six items audited.
- Pre-existing TODOs in WEFT-409 territory (`crates/clawft-wasm/src/lib.rs:78-99` Phase 3A placeholders, `set_env` no-op at line 353) are documented in `16-browser-wasm.md` and tracked under WEFT-388..409 — out of scope here.
- `Dockerfile.alpine` non-root + cache-layer items (N1, N2) are observations from this audit, not pre-existing stubs.

### Recommendations / new issues

Two findings filed as Plane work items (cycle 0.8.x — 0.7.x is closed, `CYCLE_COMPLETED` rejection on `add_to_cycle`):

1. **WEFT-563 — ws16: sparc(BW5) — retire scripts/check-features.sh references missed by WEFT-409 sweep.** Fix `.planning/sparc/browser/05-phase-BW5-wasm-entry.md:581` and `:602` to point at `scripts/build.sh gate` (or strike through with the WEFT-409 supersession note used in the sibling phase docs). Priority: low. Labels: ws16-browser-wasm, audit-finding, audit-0.7.0, docs.
2. **WEFT-564 — ws16: scripts — actually retire or annotate scripts/check-features.sh (still on disk).** The file exists on disk (62 lines, executable, since commit `6a1416c6` 2026-02-25) but every doc reference now says it "was never created" or has been retired. Either delete the file, or prepend a `# DEPRECATED 2026-04-30 (WEFT-409) — use scripts/build.sh gate` redirect that `exec`s into `build.sh gate`. Plus: rephrase the SPARC doc language from "was never created" to "was retired" so docs match git history. Priority: low. Labels: ws16-browser-wasm, audit-finding, audit-0.7.0, tooling.

## Summary

- Items confirmed shipped: **5/6** (WEFT-467, WEFT-469, WEFT-470, WEFT-540, WEFT-541)
- Items with concerns / partial: **1** (WEFT-409 — docs-only sweep was incomplete in two places, and the script it claims to retire still exists on disk)
- New issues filed: **2** (WEFT-563, WEFT-564 — both in cycle 0.8.x, low priority)
- Cross-workstream MCP regression check (WEFT-498 → MCP serialization): **clean — zero `AgentChat` references in `clawft-services::mcp` or the CLI MCP entry**.
- `scripts/build.sh wasi` / `scripts/build.sh browser` post-retirement: **both work in dry-run**.
- Research-stream triage stamp WEFT-N range (WEFT-502 … WEFT-549, 48 items): **matches Plane**.
