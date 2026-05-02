---
title: "Deployment & Release Engineering"
slug: deployment-release
workstream_id: "14"
release_gate: "0.7.0"
audit_scope: "comprehensive"
status: "in-review"
last_updated: 2026-04-28
audited_by: "release-gate-audit"
---

# Deployment & Release Engineering

## General Description

Workstream 14 covers the entire path from "commit on `master`" to "users
running the binary". Concretely:

- **Workspace versioning** — single `[workspace.package] version` in
  `Cargo.toml`, every crate inherits via `version.workspace = true`
  (ADR-001, lockstep semver).
- **Release artifacts** — `cargo-dist v0.31.0` drives `release.yml`,
  producing 7 platform binaries (linux-gnu/musl × x86_64/aarch64, macOS
  Intel/AS, Windows x86_64), shell + powershell installers, and a
  Homebrew formula auto-pushed to `weave-logic-ai/homebrew-tap`
  (ADR-002).
- **WASM artifacts** — two parallel side-channels:
  `release-wasi.yml` builds `wasm32-wasip2` and attaches to the GitHub
  Release; `wasm-browser.yml` builds `wasm32-unknown-unknown` +
  `wasm-bindgen` glue and uploads to a rolling `cdn-assets` release
  (ADR-044 — note ADR title is "wasip1" but ADR text already records
  the migration to wasip2 and the build script + workflows are
  on wasip2).
- **Knowledge-base artifact** — `release-kb.yml` builds
  `weftos-docs.rvf` from MDX content via `tools/build-kb` and attaches
  it to each release tag.
- **Crates.io publish** — `publish-crates.yml` runs
  `cargo workspaces publish --from-git --allow-dirty --no-git-commit`
  on every `v*` tag (was the source of the recent 586b081c fix —
  cargo-workspaces 0.3+ rejects `--no-git-push` / `--no-git-tag`
  when `--no-git-commit` is set).
- **Container** — `release-docker.yml` waits on `Release` via
  `workflow_run`, then `docker buildx` produces a multi-arch
  (linux/amd64 + linux/arm64) Alpine image. The Dockerfile downloads
  the prebuilt musl tarball from the GitHub Release rather than
  compiling — ~2 min builds, ~15 MB image (ADR not numbered, recorded
  in CHANGELOG 0.4.3).
- **CDN assets** — `docs-assets.yml` runs after browser-WASM
  succeeds on `master`, builds the RVF KB, and uploads everything to
  the rolling `cdn-assets` GH Release that the docs site fetches at
  runtime via `NEXT_PUBLIC_CDN_URL`.
- **Release gate** — `release-gate.yml` watches `Publish Crates` and
  `Release (Docker)`, marking the GitHub Release as `--prerelease` if
  either downstream fails (visual flagging of broken releases).
- **PR gates** — `pr-gates.yml` (clippy, test, WASM size, binary
  size, browser WASM check, voice feature check, UI lint+type-check,
  `cargo check`, `weft assess`, smoke test). `wasm-build.yml` runs the
  size gate independently. `benchmarks.yml` runs `scripts/bench/run-all.sh`
  and posts a regression comment if any metric moves >10%.
- **Docs site** — `docs/src/` is a Next.js 16 + Fumadocs 16.7 app,
  deployed to Vercel at `weftos.weavelogic.ai`. Three-property strategy
  per ADR-015: marketing (`weavelogic.ai`) / docs (`weftos.weavelogic.ai`)
  / assessment app (`assess.weavelogic.ai`). Token-based theming per
  ADR-016 maps Fumadocs ocean preset → other render targets.
- **Local release scripts** — `scripts/release/{package,package-all,
  generate-changelog}.sh`, `scripts/build/{cross-compile,docker-build,
  size-check,wasm-opt}.sh`, `scripts/deploy/{vps-deploy.sh,
  docker-compose.yml}`.
- **Phase gates** — `scripts/build.sh gate` (11 checks),
  `scripts/09-gate.sh` (Sprint 09 WeftOS gaps), `scripts/k6-gate.sh`
  (K3-K6 phases).
- **Universal installer** — `scripts/install.sh` (curl|sh entry point,
  detects platform, downloads from latest GH Release).

## Status & Timeline

| Capability | First shipped | Last touched | State |
|---|---|---|---|
| `cargo-dist`-driven `release.yml` | 0.1.0 (Sprint 11, expanded Sprint 14 to 10 targets) | 2026-04-22 | OK |
| `pr-gates.yml` (8 jobs) | 0.4.1 (Sprint 14) | 2026-04-22 | OK |
| `release-docker.yml` (workflow_run) | 0.6.11 (eliminated tag-push race) | 2026-04-22 | OK |
| `release-wasi.yml` (cargo-dist gap) | 0.5.x | 2026-04-22 | OK |
| `release-kb.yml` | 0.4.x | 2026-04-22 | OK |
| `wasm-browser.yml` + `docs-assets.yml` | 0.4.0 (Sprint 14 / cdn-assets) | 2026-04-22 | OK |
| `release-gate.yml` (prerelease flagging) | 0.6.11 | 2026-04-22 | OK |
| `publish-crates.yml` (cargo-workspaces) | 0.6.x | 2026-04-22 (586b081c) | **fixed last week** |
| Fumadocs site at `weftos.weavelogic.ai` | 0.4.0 | active | OK (Next 16.2, Fumadocs 16.7, React 19.2) |
| Universal `install.sh` | 0.6.0 | 2026-04-22 | OK |
| `scripts/build.sh gate` (11 checks) | 0.5.x | 2026-04-22 | OK |
| `scripts/09-gate.sh` (1200+ kernel tests gate) | 0.5.x | not retouched | drifting (gate counts) |
| Last successful tag | v0.6.19 (2026-04-22) | per CHANGELOG | OK |
| Workspace version | 0.6.19 | `Cargo.toml:50` | shipping |
| Internal-crate dep versions | 0.6.6 | `Cargo.toml:182-216` | **DRIFT** — see open questions |

Recent CI-related commits (oldest → newest on the branch):

- `cd779585` — fix musl builds (vendored-openssl), WASI target install, defer Win ARM (0.5.x)
- `0230ad86` — increase WASI release wait to 30 min (cargo-dist takes ~15 min)
- `e213f7c3` — ensure `wasm32-unknown-unknown` target after cache restore
- `13b422d1` — handle "already exists" error in crates.io publish
- `24ea4c69` — add `clawft-services` to publish pipeline
- `928ed2cc` — add `clawft-plugin-treesitter` to publish pipeline
- `6cbe5196` — Alpine + pre-built binaries for Docker (~30min → ~2min)
- `349d13c0` — add `eml-core` to publish pipeline, fix Docker race, add release gate
- `5d948edd` — tiered parallel crate publishing (~90s vs ~390s)
- `a4b9d615` — replace manual publish with cargo-workspaces
- `cf374681` — add explicit `rustup target add wasm32-wasip2` in PR gate
- `586b081c` — drop mutually-exclusive flags from publish-crates workflow

## Released Features

The release pipeline as it stands today, by stage:

### Source-of-truth layer
- **Lockstep workspace version** — one bump per release (`Cargo.toml:50`),
  every crate inherits.
- **`[workspace.metadata.dist]`** in `Cargo.toml:228-263` — 7 build
  targets, shell + powershell + homebrew installers, GitHub Attestations
  enabled, profile `dist` inherits from `release`, `release-wasm` profile
  with `opt-level = "z"`.

### Tag-triggered fan-out
- `Release` (cargo-dist) — multi-target builds + GitHub Release create + Homebrew tap push.
- `Publish Crates` — `cargo-workspaces publish --from-git`, dependency-topological order, idempotent on re-publish.
- `Release WASI` — independent `wasm32-wasip2` build, polls release for ≤30 min, `gh release upload --clobber`.
- `Release (Knowledge Base)` — `tools/build-kb` → `weftos-docs-{tag}.rvf`, polls release, uploads.
- `Release (Docker)` — `workflow_run` on Release success, `docker buildx` multi-arch to `ghcr.io/weave-logic-ai/weftos:{tag}` and `:latest`, post-publish smoke test (gateway health probe — currently a `sleep 5` + `docker ps` check, not an HTTP probe).
- `Release Gate` — marks any GH Release as prerelease when `Publish Crates` or `Release (Docker)` fails.

### PR + master CI
- `PR Gates` — clippy, test, WASM size (300 KB raw / 120 KB gzip), binary size (10 MB), browser WASM check (`continue-on-warning` until BW1), voice feature check (`continue-on-warning` until VS1.1), UI lint/type-check/test (only when `ui/` exists), `cargo check`, `weft assess --scope ci --format github-annotations`, integration smoke test.
- `WASM Build & Size Gate` — wasm32-wasip2 build, `wasm-opt`, twiggy profiling, raw + gzip + optimized size summary.
- `Browser WASM` — wasm32-unknown-unknown + wasm-bindgen 0.2.108, uploads `browser-wasm-pkg` artifact, attaches `clawft-browser-wasm-{tag}.tar.gz` to release on tag push.
- `Docs Assets Publish` — runs after browser WASM succeeds on master, refreshes the rolling `cdn-assets` release with WASM bundle + KB.
- `Benchmarks` — runs benchmark suite, regression-checks against `scripts/bench/baseline.json` (10% threshold), comments on PR if regressed.

### Distribution surfaces
- GitHub Releases — binaries, installers, KB, browser-WASM bundle, WASI bundle, Docker image manifest reference.
- crates.io — every workspace crate marked `publish = true` (most are).
- ghcr.io/weave-logic-ai/weftos — multi-arch Alpine image.
- weave-logic-ai/homebrew-tap — Homebrew formula via cargo-dist.
- `cdn-assets` rolling GH Release — browser WASM + KB consumed by docs site at runtime.
- `https://weftos.weavelogic.ai/install.sh` (and the master raw URL) — universal installer.

### Documentation site
- Fumadocs 16.7.6 + Next.js 16.2.1 + React 19.2 (`docs/src/package.json`).
- 56+ MDX pages under `docs/src/content/docs/` — root, `/clawft/`, `/weftos/` (35 top-level + sub-trees).
- Vercel deployment for `weftos.weavelogic.ai`.
- API route `/api/cdn/[...path]` proxies the `cdn-assets` release (s-maxage 604800 + SWR 86400).
- `/clawft` route hosts the WASM playground that loads the browser WASM + KB from the CDN.

## What's Left — Total Depth

### Open TODOs / FIXMEs in code & workflows

- **No live `TODO`/`FIXME` markers in `.github/workflows` or top-level `scripts/`**
  (`grep TODO|FIXME|XXX|HACK` returns only `scripts/bench/run-all.sh:110` which is a `mktemp` template, not a real TODO). The dirtiness lives elsewhere — in untouched planning docs and in stale doc references.
- **`pr-gates.yml:171-201`** — `Browser WASM check` deliberately swallows failures and posts a `::warning::` "skipped (feature not yet implemented)". This was waiting on BW1 (browser feature flags); the work has since landed (`wasm-browser.yml` is a real build), so the soft check is now stale and should be hard-gated.
- **`pr-gates.yml:209-236`** — same pattern for `Voice feature check` waiting on `VS1.1` (voice deps). Voice has not landed; gate is correctly soft, but the comment is the only tracker.
- **`pr-gates.yml:346-386`** — `Integration smoke test` "verifies the container starts and stays running for 5 seconds" but the comment explicitly says "the gateway may not have a /health endpoint yet". Gateway HTTP `/health` was added in 0.3.0 (CHANGELOG), but the smoke test still uses `sleep 5 + docker ps`. Replace with `curl -fsSL localhost:8080/health` once we confirm port-binding behaviour in the smoke harness.
- **`release-docker.yml:107-141`** — same `sleep 5` + `docker ps` shape in the post-publish smoke test. Same fix.
- **`scripts/deploy/docker-compose.yml:11`** — image is `ghcr.io/clawft/clawft:${WEFT_VERSION:-latest}`. The repo moved from `clawft/clawft` to `weave-logic-ai/weftos` (CHANGELOG 0.4.1: "Wrong URLs"). Compose, `vps-deploy.sh:23`, and `docs/deployment/docker.md` (15+ refs) all still point at the old `ghcr.io/clawft/clawft` path. The Dockerfile and release-docker workflow already use the new path.
- **`scripts/deploy/vps-deploy.sh:23,26,30`** — same wrong image, also `--config DIR (default: ~/.clawft)`. The current install path is `~/.clawft/` but the runtime moved to `~/.weftos/` for some subsystems (per `weave-init.sh` + CHANGELOG note about `cluster_peers.json`). Verify which is canonical and fix the script.
- **`scripts/build/docker-build.sh:91`** — only builds `x86_64-unknown-linux-musl`. There is no aarch64 path; the `release-docker.yml` workflow handles cross-arch via `docker buildx`, but this local helper drifts.
- **`scripts/build/docker-build.sh:75`** — image is `clawft:${IMAGE_TAG}`, not `weftos:` — name drift.
- **`scripts/install.sh:14`** — repo correctly set to `weave-logic-ai/weftos`. Asset name `${asset_prefix}-${TRIPLE}.tar.gz` matches what cargo-dist emits *for the binary `clawft-cli`/`clawft-weave`/`weftos`*; verify those crate-name asset names actually exist for every release post-cargo-dist (cargo-dist by default uses workspace crate names, but the asset prefix logic (`set -- clawft-cli clawft-weave weftos`) needs to track binary naming convention).
- **`scripts/release/package*.sh`** — these are the *pre*-cargo-dist hand-rolled packagers. `Cargo.toml:228-263` is now the source of truth. Either delete these scripts (ADR-002 said "the existing hand-rolled `release.yml` workflow will be replaced") or relabel them as "local rehearsal" tools. Right now they appear to be dead code that still references `BINARY_NAME="weft"` only — wouldn't package `weaver` or `weftos`.
- **`scripts/09-gate.sh:101-103`** — gate asserts ">=1200 kernel tests pass" by parsing `cargo test` output. CHANGELOG suggests we are well above this floor (handoff cites 1218 in clawft-core alone). Treat as a stale floor — either bump to a meaningful threshold or delete.
- **`scripts/09-gate.sh:67-71`** — references `.planning/sparc/weftos/0.1/09b-decision-resolution.md` and `phase-K0/decisions.md` etc. Confirm these still exist after the planning reorg (they may have moved under `phase4/`).
- **`docs/deployment/docker.md:15-18,21-27,55,84`** — repository URL on line 27 is `github.com/weave-logic-ai/clawft.git` (WRONG; should be `weftos`). Image lines 15, 84 say `ghcr.io/clawft/clawft:latest`. Doc body still describes the old `FROM scratch` ~5 MB image; current Dockerfile is `FROM alpine:3.21` ~15 MB. Plus a contradictory "K2 multi-stage build with cargo-chef debian:bookworm-slim" appendix (lines 171-279) that describes a Dockerfile we no longer ship.
- **`docs/deployment/release.md:43,55,93,119`** — wrong URLs (`github.com/weave-logic-ai/clawft`, `ghcr.io/clawft/clawft`), references to "FROM scratch ~5 MB image" that no longer exists, asset names `weft-linux-x86_64` (cargo-dist emits `weft-{version}-{target}.tar.gz`).
- **`docs/deployment/wasm.md`** — not opened in this audit, but worth checking for the wasip1/wasip2 confusion (ADR-044 still titled `wasm-wasip1-target`).
- **`docs/adr/adr-044-wasm-wasip1-target.md`** — title says "wasm32-wasip1" but file body acknowledges the migration to wasip2 happened. Either retitle (preferred) or supersede with a new ADR. Build script + every release workflow now emits `wasm32-wasip2`.
- **`docs/adr/adr-037-rust-edition-2024-msrv.md:18-22`** — example block still shows `version = "0.3.1"`. Cosmetic; flag in a doc-sweep pass.

### Deferred / orphaned items

- **Browser-WASM hard gate** — `pr-gates.yml` continues to soft-check
  while `wasm-browser.yml` builds a real artifact every push to master.
  Convert PR-gate browser check to fail-on-error.
- **Smoke test as HTTP probe** — both `pr-gates.yml` and
  `release-docker.yml` need a real `/health` probe (gateway has the
  endpoint since 0.3.0).
- **Migrate or retire `scripts/release/package*.sh`** — superseded by
  cargo-dist (ADR-002). Currently dual-maintained.
- **`scripts/deploy/` URL fixup** — stale `clawft/clawft` GHCR path in
  `docker-compose.yml` and `vps-deploy.sh`. Straight find/replace.
- **`docs/deployment/docker.md` rewrite** — at minimum: URL fixup,
  drop `FROM scratch` references, drop the "cargo-chef multi-stage
  debian:bookworm-slim" appendix that no longer matches the Dockerfile,
  add a section on the binary-prefetch flow.
- **`docs/deployment/release.md` rewrite** — describe cargo-dist asset
  names, add the WASI / KB / browser-WASM sub-releases, document the
  `release-gate` prerelease-flagging behaviour.
- **`scripts/build.sh gate` test 1** — runs `cargo test --workspace`
  via `>/dev/null 2>&1`; one hanging test (CHANGELOG 0.6.19 "Known
  Issues") would silently look like a fail. Consider a
  per-crate variant, or add a test-matrix skip-list.
- **`scripts/build.sh` does NOT have a `release` subcommand** — every
  ADR/skill assumes `scripts/build.sh` covers all release ops. The
  release flow today is `git tag vX.Y.Z; git push --tags` (cargo-dist
  takes over from there), with no local `scripts/build.sh release`
  rehearsal mode. Add a `release-dry-run` that reproduces the cargo-dist
  matrix locally for at least the host triple.
- **`tools/build-kb`** — used by `release-kb.yml` and `docs-assets.yml`.
  Not in the workspace `Cargo.toml`. Should be — at minimum, document
  why it's a side workspace and ensure `Cargo.lock` for it is
  committed.
- **CDN-assets retention** — rolling `cdn-assets` release uses
  `--clobber`. There is no audit trail for what version of WASM the
  docs site is serving on any given day. Consider tagging each upload
  with the source commit SHA in the asset filename, or add a
  `cdn-assets-history` companion release with versioned snapshots.
- **No release-attestation verification step** — cargo-dist enables
  GitHub Attestations (`Cargo.toml:254`) but there's no client-side
  verification path documented (no `cosign verify-attestation` mention
  in install.sh, no docs page describing how to verify).
- **`scripts/09-gate.sh`** — references kernel test count; stale floor
  (1200, current is ≫ 1200). Either bump to a meaningful number or
  delete the file.
- **`scripts/k6-gate.sh`** — runs phase-K3..K6 gates. Not invoked from
  CI (phase-gate is `scripts/build.sh gate`). Either wire it into a
  workflow or mark "developer-rehearsal only".
- **`scripts/clawft-wake.service` + `scripts/com.clawft.wake.plist`**
  — systemd + launchd unit files for the wake service. Are these
  actually packaged into anything? They're not in `[workspace.metadata.dist]
  include`, not in any release tarball. Probably dead.
- **`build_vp_deck.py`, `dev_server.py`** — Python helpers in
  `scripts/`. Not part of any workflow or build-gate. Consider moving
  to `scripts/dev/` or similar.
- **`scripts/weave-init.sh`** — bootstraps a project with `.weftos/`
  layout. Compare against `weaver init` (Rust binary) — these may
  duplicate. CHANGELOG 0.6.14: "weaver init rewritten — works in any
  directory, generates weave.toml". The shell script may now be dead.
- **Fumadocs site link drift** — `docs/deployment/docker.md` and
  `release.md` are not in `docs/src/content/docs/`, so they're invisible
  to the public docs site. Either move/link them in or delete; ADR-014
  was explicit that Fumadocs is the single source of truth.
- **`docs/src/content/docs/weftos/vision/releases.mdx`** — last release
  documented is **v0.6.13** (file head shows that as the most recent
  entry); CHANGELOG.md is current to v0.6.19. The site's "Release Notes"
  page is six releases behind. Either auto-generate from CHANGELOG.md
  during the docs build or have the release workflow PR an update.
- **CHANGELOG.md links section** — `[0.6.6]` through `[0.1.0]` have
  compare-link footnotes (lines 956-975). 0.6.7 through **0.6.19** do
  not. Compare links missing for the last 13 releases.
- **No `Unreleased` section in CHANGELOG.md** — Keep-a-Changelog
  convention recommends a `## [Unreleased]` heading where staged work
  lands before tagging. The repo does not maintain one; release notes
  are written into the next version's section directly. Either add the
  convention or document explicitly that we deviate.
- **No `cargo-dist` regenerate cadence** — `cargo-dist-version =
  "0.31.0"` is pinned in `Cargo.toml:230`. cargo-dist 1.0 has shipped;
  the `dist` binary is hardcoded in `release.yml:67`. Bumping
  cargo-dist requires regenerating `release.yml` (it is autogenerated,
  per the file header). No process documented.
- **Closure-sdk integration** (`.planning/development_notes/closure-sdk-integration.md`)
  — recommendation is "defer / conceptual-only" because of AGPL-3.0.
  No release-engineering implication today, but worth re-checking
  whenever `weftos-closure-bridge` work is proposed (would force a
  separate AGPL crate behind an IPC boundary, which means a *separate*
  release artifact lifecycle).
- **`development_notes/sprint-16/wasmtime-upgrade.md`** — wasmtime v33
  upgrade landed; document closes 10 Dependabot alerts. No follow-up
  beyond keeping wasmtime current. Action: add a quarterly
  "dependency sweep" cadence or rely on Dependabot to keep us honest.
- **No SBOM / cargo-audit gate in CI** — security review side-channel,
  but missing from this workstream's pipeline. cargo-dist supports
  CycloneDX SBOM generation (not currently enabled in
  `[workspace.metadata.dist]`).
- **No `release-plz`** — ADR-002 said "Complement with release-plz".
  Not adopted. Manual version bumps + tag + push is the current flow.
- **No `git-cliff`** — also called out by ADR-002. Not adopted.
  `scripts/release/generate-changelog.sh` is the home-rolled
  conventional-commit grouper instead.

### Open questions

- **Workspace internal-dep version drift** — `Cargo.toml:50` is
  `version = "0.6.19"`, but every internal `clawft-*` dependency at
  `Cargo.toml:182-216` is pinned to `version = "0.6.6"`. Is this
  intentional (publishing each crate at its individually-bumped
  version, lockstep only for the workspace tag)? If so, ADR-001's
  "single source of truth" framing is misleading. If not, every release
  since 0.6.6 has shipped with stale path-dep version declarations.
  This needs a decision before 0.7.0 cuts.
- **What is the canonical install path?** — `install.sh` and
  `vps-deploy.sh` use `~/.clawft/`. The CHANGELOG, kernel docs, and
  several runtime artifacts use `~/.weftos/` or `.weftos/runtime/`.
  Pick one, document, sweep all install/deploy code.
- **Should `release.yml` also build `wasm32-wasip2` and run-on-pull-request?**
  Currently `release-wasi.yml` only fires on tag push. Adds 30-min
  wait. Could be folded into cargo-dist if/when wasip2 is a supported
  target there (currently HP-16 deferred per `release-wasi.yml`
  header).
- **Should we publish a `weft-cli` Homebrew bottle (compiled binary)
  vs. the current source-build formula?** — affects macOS install UX
  (formula currently rebuilds from source via cargo).
- **Is `assess.weavelogic.ai` actually deployed?** — ADR-015 names it
  as one of three properties. No CI workflow targets it from this
  repo. Likely lives in a sibling repo. Confirm and link.
- **Universal installer integrity** — `install.sh` does not verify
  checksums or attestations on the downloaded binaries. cargo-dist
  enables GitHub Attestations (`Cargo.toml:254`); add a
  `cosign`/`gh attestation verify` step before the `cp $bin
  $INSTALL_DIR`.
- **Browser-WASM artifact size budget** — `wasm-browser.yml` does
  not gate on size, only `wasm-build.yml` (wasip2) does. Should
  browser WASM also have a budget? It's the user-facing playground.
- **VPS deploy has no rollback path** — `vps-deploy.sh` only stops +
  removes the existing container, then `docker run`s the new one. If
  the new container exits, the old one is gone. Add health-probe →
  rollback.
- **No `cdn-assets` purge / lifecycle policy** — every WASM build on
  master overwrites the rolling release. If we ever ship an MDX
  change that expects a newer browser-WASM API, the docs site can
  break for users with cached old WASM. Need cache-bust strategy.
- **Where does the docs site actually deploy from?** — `docs/src/`
  has its own `package.json`. There is no GitHub Action in this repo
  that runs `vercel deploy`. Likely Vercel Git integration on
  `docs/src/`. Confirm and document the trigger surface (which paths
  trigger a redeploy vs. which need manual).
- **Can a bad MDX change land on master and break the docs site
  silently?** — there is no `npm run build` step in CI for `docs/src`.
  Vercel will catch it post-merge, but the contributor doesn't see it
  in the PR. Add a `docs-build` job to `pr-gates.yml`.
- **Per-target test matrix** — CI runs `cargo test --workspace` only on
  ubuntu-latest. macOS and Windows targets only get a `cargo build` via
  cargo-dist on tag push. Catch platform-specific test failures pre-tag.
- **cargo-dist v1 / v0.32+ migration plan** — pinned at v0.31.0. New
  versions add wasip2 support, SBOM, etc. Schedule the bump.

### Orphaned work

- `.planning/development_notes/10-deployment-community/phase-K-{docker,security,community}/notes.md|decisions.md|blockers.md|difficult-tasks.md`
  are all empty stubs (`_No notes recorded yet._`). Either populate
  retroactively from CHANGELOG/handoff notes or delete the empty
  scaffolding.
- `.planning/sparc/phase4/10-deployment-community/04-element-10-tracker.md`
  has Element 10 marked **COMPLETE** as of K2-K5. The tracker references
  ClawHub features (`weft skills publish/install`, Ed25519 signing,
  vector search). These are tangentially deployment-related but mostly
  belong to the security/community workstream. Confirm whose lap they
  sit in and de-duplicate.
- `scripts/clawft-wake.service` + `scripts/com.clawft.wake.plist` —
  see "deferred items"; likely orphaned.
- `tools/build-kb` — outside the workspace; orphaned in the sense of
  being a build dep for two workflows but not visible from `Cargo.toml`.
- `crates/clawft-kernel/Dockerfile.alpine` — second Dockerfile beside
  the root one. Verify whether it is built by anything.

## Task List

The following items are concrete, scoped TODOs derived from the audit
above. Numbering is for cross-reference only; ordering is by deployment
risk (highest first).

1. **[P0/blocker] Resolve workspace internal-dep version drift.**
   `Cargo.toml:50` says `0.6.19`, every internal dep says `0.6.6`. If
   intentional, document in ADR-001 and the release runbook. If not,
   bump every internal `version = "0.6.6"` to `version = "0.6.19"`
   and add a `scripts/build.sh check-versions` lint that fails when
   they diverge.
2. **[P0] Update stale `ghcr.io/clawft/clawft` paths.** Sweep
   `scripts/deploy/docker-compose.yml`, `scripts/deploy/vps-deploy.sh`,
   `docs/deployment/docker.md`, `docs/deployment/release.md`. Replace
   with `ghcr.io/weave-logic-ai/weftos`.
3. **[P0] Decide canonical install path** (`~/.clawft/` vs `~/.weftos/`).
   Sweep install.sh, vps-deploy.sh, docker-compose.yml, docs.
4. **[P1] Convert PR-gate browser-WASM check to hard gate.** Remove the
   `2>/dev/null` swallow in `pr-gates.yml:171-201`; the artifact build
   is real now.
5. **[P1] Replace smoke-test `sleep 5 + docker ps`** with an HTTP probe
   to `/health` in `pr-gates.yml:346-386` and `release-docker.yml:107-141`.
6. **[P1] Add `docs-build` job to `pr-gates.yml`** that runs
   `cd docs/src && npm install && npm run build`. Stops bad MDX from
   merging.
7. **[P1] Decide fate of `scripts/release/package*.sh`.** Either
   delete (cargo-dist is the source of truth per ADR-002) or relabel
   as local-rehearsal-only and remove the implicit "weft-only" pattern
   (it doesn't package weaver or weftos binaries).
8. **[P1] Auto-generate `docs/src/content/docs/weftos/vision/releases.mdx`
   from CHANGELOG.md** during the docs build, OR add a release-tag
   workflow that PRs an update. Currently 6 releases behind.
9. **[P1] Backfill CHANGELOG.md compare links** for 0.6.7 through 0.6.19
   (lines 956-975 only cover 0.1.0 → 0.6.6).
10. **[P1] Add `## [Unreleased]` heading to CHANGELOG.md** OR document
    explicitly that we don't maintain one (release-engineering runbook).
11. **[P2] Re-title ADR-044** from `wasm-wasip1-target` to
    `wasm-wasip2-target`, since the ADR body acknowledges the migration
    and the current code targets wasip2 everywhere.
12. **[P2] Add `cargo-audit` / `cargo-deny` gate to `pr-gates.yml`.**
    Adjacent workstream (security) but lives here in CI.
13. **[P2] Verify cargo-dist attestations in `install.sh`.** Pull
    `gh attestation verify` (or `cosign`) into the install flow before
    `cp $bin $INSTALL_DIR`.
14. **[P2] Snapshot every `cdn-assets` upload by commit SHA** —
    add `clawft_wasm-{sha}.wasm` alongside `clawft_wasm.wasm` so the
    docs site has a roll-back path.
15. **[P2] Add browser-WASM size budget** to `wasm-browser.yml`,
    matching the wasip2 budget.
16. **[P2] Roll back path in `vps-deploy.sh`** — keep the prior image
    around, only `docker stop` after the new one passes a health probe.
17. **[P2] Add macOS / Windows test job to `pr-gates.yml`** (currently
    only ubuntu-latest runs `cargo test`).
18. **[P3] Schedule cargo-dist bump** to v1+ (or current latest).
    Regenerate `release.yml`. Document in `docs/deployment/release.md`.
19. **[P3] Adopt `release-plz` and/or `git-cliff`** per ADR-002, OR
    explicitly amend the ADR to record that we use
    `scripts/release/generate-changelog.sh` instead.
20. **[P3] Move dead scripts.** Audit `scripts/clawft-wake.service`,
    `scripts/com.clawft.wake.plist`, `scripts/build_vp_deck.py`,
    `scripts/dev_server.py`, `scripts/weave-init.sh`. Either wire into
    a workflow, move to `scripts/dev/`, or delete.
21. **[P3] Bump or delete `scripts/09-gate.sh`** test-count floor (1200
    is now low). Same for path references to `.planning/sparc/weftos/0.1/`
    (likely moved under `phase4/`).
22. **[P3] Wire `scripts/k6-gate.sh` into a workflow** OR mark it
    "developer rehearsal only".
23. **[P3] Populate (or delete) the empty stubs** under
    `.planning/development_notes/10-deployment-community/phase-K-*/`.
24. **[P3] Move `tools/build-kb` into the workspace** OR document why
    it sits outside.
25. **[P3] Add a `docs/deployment/wasm.md` audit pass.** Not opened by
    this audit; likely has stale URLs / wasip1 references.
26. **[P3] Add an SBOM job** (CycloneDX via cargo-dist or
    cargo-cyclonedx) and attach to releases.
27. **[P3] `scripts/build.sh release-dry-run`** subcommand that
    exercises the cargo-dist matrix locally for the host triple.

## Sources

- `/home/aepod/dev/clawft/Cargo.toml` (workspace, version 0.6.19, cargo-dist metadata at lines 228-263)
- `/home/aepod/dev/clawft/CHANGELOG.md` (v0.6.19 → v0.1.0; missing compare links 0.6.7-0.6.19; no Unreleased section)
- `/home/aepod/dev/clawft/.github/workflows/release.yml` (cargo-dist autogenerated)
- `/home/aepod/dev/clawft/.github/workflows/release-docker.yml` (workflow_run trigger)
- `/home/aepod/dev/clawft/.github/workflows/release-wasi.yml`
- `/home/aepod/dev/clawft/.github/workflows/release-kb.yml`
- `/home/aepod/dev/clawft/.github/workflows/release-gate.yml`
- `/home/aepod/dev/clawft/.github/workflows/publish-crates.yml` (586b081c fix)
- `/home/aepod/dev/clawft/.github/workflows/pr-gates.yml` (8 jobs)
- `/home/aepod/dev/clawft/.github/workflows/wasm-browser.yml`
- `/home/aepod/dev/clawft/.github/workflows/wasm-build.yml`
- `/home/aepod/dev/clawft/.github/workflows/docs-assets.yml` (cdn-assets release)
- `/home/aepod/dev/clawft/.github/workflows/benchmarks.yml`
- `/home/aepod/dev/clawft/scripts/build.sh` (gate, native, wasi, browser, ui, all)
- `/home/aepod/dev/clawft/scripts/install.sh` (universal curl|sh installer)
- `/home/aepod/dev/clawft/scripts/09-gate.sh`
- `/home/aepod/dev/clawft/scripts/k6-gate.sh`
- `/home/aepod/dev/clawft/scripts/release/package.sh`
- `/home/aepod/dev/clawft/scripts/release/package-all.sh`
- `/home/aepod/dev/clawft/scripts/release/generate-changelog.sh`
- `/home/aepod/dev/clawft/scripts/build/cross-compile.sh`
- `/home/aepod/dev/clawft/scripts/build/docker-build.sh`
- `/home/aepod/dev/clawft/scripts/build/wasm-opt.sh`, `size-check.sh`
- `/home/aepod/dev/clawft/scripts/deploy/docker-compose.yml`
- `/home/aepod/dev/clawft/scripts/deploy/vps-deploy.sh`
- `/home/aepod/dev/clawft/scripts/bench/run-all.sh` (and 13 other bench scripts)
- `/home/aepod/dev/clawft/Dockerfile` (Alpine 3.21, prebuilt-binary download)
- `/home/aepod/dev/clawft/crates/clawft-kernel/Dockerfile.alpine` (second Dockerfile)
- `/home/aepod/dev/clawft/docs/deployment/docker.md` (stale URLs)
- `/home/aepod/dev/clawft/docs/deployment/release.md` (stale URLs)
- `/home/aepod/dev/clawft/docs/deployment/wasm.md`
- `/home/aepod/dev/clawft/docs/src/package.json` (Fumadocs 16.7.6, Next 16.2.1, React 19.2.4)
- `/home/aepod/dev/clawft/docs/src/content/docs/` (56+ MDX pages)
- `/home/aepod/dev/clawft/docs/src/content/docs/weftos/vision/releases.mdx` (last entry v0.6.13)
- `/home/aepod/dev/clawft/docs/adr/adr-001-lockstep-semver.md`
- `/home/aepod/dev/clawft/docs/adr/adr-002-cargo-dist.md`
- `/home/aepod/dev/clawft/docs/adr/adr-008-weftos-cloud-side.md`
- `/home/aepod/dev/clawft/docs/adr/adr-014-fumadocs.md`
- `/home/aepod/dev/clawft/docs/adr/adr-015-three-property-web.md`
- `/home/aepod/dev/clawft/docs/adr/adr-016-multi-target-theming.md`
- `/home/aepod/dev/clawft/docs/adr/adr-037-rust-edition-2024-msrv.md` (stale 0.3.1 example)
- `/home/aepod/dev/clawft/docs/adr/adr-044-wasm-wasip1-target.md` (mistitled — wasip2 now)
- `/home/aepod/dev/clawft/docs/handoff.md` (agent-core-v1 ships, 1549 tests)
- `/home/aepod/dev/clawft/.planning/development_notes/sprint-16/wasmtime-upgrade.md`
- `/home/aepod/dev/clawft/.planning/development_notes/closure-sdk-integration.md`
- `/home/aepod/dev/clawft/.planning/development_notes/10-deployment-community/README.md`
- `/home/aepod/dev/clawft/.planning/development_notes/10-deployment-community/phase-K-docker/{notes,decisions,blockers,difficult-tasks}.md` (empty stubs)
- `/home/aepod/dev/clawft/.planning/development_notes/10-deployment-community/phase-K-security/*.md` (empty stubs)
- `/home/aepod/dev/clawft/.planning/development_notes/10-deployment-community/phase-K-community/*.md` (empty stubs)
- `/home/aepod/dev/clawft/.planning/sparc/phase4/10-deployment-community/00-orchestrator.md`
- `/home/aepod/dev/clawft/.planning/sparc/phase4/10-deployment-community/04-element-10-tracker.md` (Element 10 marked COMPLETE)
- `/home/aepod/dev/clawft/.planning/weftos.weavelogic.ai/05-sparc-completion.md` (Sprint-by-sprint exec checklist for site)
- git history: commits cd779585 → 586b081c (CI/release iteration log), 8c08ce0a (current HEAD on `development-0.7.0`)

<!-- TRIAGED-STAMP:BEGIN -->
## Triaged into Plane — 2026-04-28

All open items in this audit have been filed as Plane work items in the WeftOS workspace under the `ws14-deployment` label.

- **Range**: WEFT-441 … WEFT-550 (38 items)
- **Per cycle**: 0.7.x: 13, 0.8.x: 22, 0.9.x: 3
- **Triage spec**: `.planning/reviews/0.7.0-release-gate/triage/`
- **WEFT-N → name map**: `.planning/reviews/0.7.0-release-gate/triage/weft-mapping.json`

Per the project rule (CLAUDE.md → "Plane is the authoritative work tracker"): future updates to these items happen in Plane, not in this audit doc. This doc remains the source-of-truth for the original survey.
<!-- TRIAGED-STAMP:END -->
