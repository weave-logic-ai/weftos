# Handoff — WeftOS — 2026-09-28

WeftOS is a Rust agent OS (kernel, chain, BVH/HNSW spatial, Urth twin) with a WeftOS dashboard
(`~/dev/weftos-dashboard`, Next.js on Vercel + Supabase) as its work board and portfolio view. This
session shipped **v0.8.1** with an agent directory: agent packages authored in `agents/`, rendered
into Claude, Grok and Codex by `weftos init`, and shown in the dashboard's AI org tab. It also
answered review feedback on four upstream ruflo PRs. Everything is committed and pushed; nothing is
running.

Topic handoffs: [`handoff-urth-spatial.md`](handoff-urth-spatial.md) (sonobuoy/copper research,
dead ends), `handoff-voice-talk.md`, `handoff-oil-rig.md` (confidential, local only),
`handoff-tracker-ci-memory.md`, `handoff-local-llm-config.md`.

## Current state

- WeftOS: branch `0.8-metaharness` @ `a9ddcf90`, clean, one worktree, equal to `origin`. This branch
  stays open; all current work lands here and is pushed at checkpoints (see memory).
- **v0.8.1 is released** (tag on `2cd752e1`, 87 assets, Release/KB/SBOM/WASM green). `a9ddcf90` is
  the first 0.8.2 commit (CHANGELOG `[Unreleased]`).
- Gate: `scripts/build.sh gate` **19/19 green** on `a9ddcf90`'s content (run 2026-09-28 ~17:24).
  `scripts/build.sh clippy` **fails** on pre-existing debt the gate does not run (ticket `8dfd7ed5`).
- Dashboard: `~/dev/weftos-dashboard` `main`, clean; production auto-deploys from `main`.
- Nothing is running. `~/dev/ruflo-pr-teambus` is a kept, built ruflo checkout for testing (below).

## What's working (verified)

| Thing | State | Verified how |
|---|---|---|
| `weftos init --claude --grok --codex` | renders the `weftos-core` team: 61 files, re-run is a no-op | released arm64 binary: `--plan` in a scratch repo showed `source: embedded (weftos 0.8.1 @ 2cd752e1)` |
| Hosts load rendered agents | Claude, Grok and Codex all load them (project must be trusted in Codex/Grok) | agent ran `claude -p`, `grok inspect`, codex with decoy names |
| Agent package gate | `agents-validate`, `agents-catalog --check`, `agents-leak-check` = gate checks 17–19 | gate run; 9 seeded validator failures and 4 leak cases fail correctly |
| Dashboard AI org tab | live, reads `agents/catalog.json` from GitHub (15 agents, 1 team) | `curl https://weftos-dashboard.vercel.app/api/agents/catalog` → `source: weftos` |
| Dashboard agent-directory harness routes | **not working yet** | migration `202609280001_agent_directory.sql` not applied to live Supabase |
| Ruflo team bus via WeftOS shim | works against the reviewed ruflo build | scratch project: create → spawn (`reviewer:reviewer@demo`) → per-team mailbox → shutdown |
| BakeOS → dashboard publishing | Vercel Cron `*/5` runs a Workflow; host loop stopped | Vercel logs: cron 202 + 5 steps 200; dashboard POST 200 at :15 and :20 |
| FlipsOS | auto-deploys production from `dev`; daily sync cron on the new deploy | PR #2 merge deployed; Vercel project crons |

## Done this session

- **v0.8.1**: Ruflo agent-team host (ADR-402), `weft agent -m --session <id>` with failure exit
  codes, ObservationPack (tool-result archive + `obs_recall`), build-kb from repo docs, Urth docs
  pages, cargo + npm audit clean, and the agent directory below.
- **Agent directory (AD-1…AD-7)**: design + ADR (Accepted, D1–D6) in
  `docs/research/agent-directory/`. `agents/` holds the base team (steward, doc-gardener, liber, mo,
  lead-doctrine, 5 lanes, 3 templates) and eikon/eikon-specialist; versioned by weftos release (D3);
  merged PR = approval (D4). AD-1/2/4/5/6 done; AD-7 in review (Shasta draft PR #4); AD-3 partial.
- **Ruflo PRs** (Dragan's review, all addressed, replies posted): #3521 `3574f10ca`, #3512
  `19c1afe17`, #3513 `040e0f1b0` (#3512 + one commit), new #3526 (memory tag filter + provider
  precedence, split out of #3512). #3513 was *ported* onto #3512's single `.mjs` store.
- **WeftOS team bus synced** to the reviewed store (`a9ddcf90`): no API keys in `passEnv`, stale
  stop hook replaced, interop test on the reviewed protocol.
- **Research** (committed, `docs/research/`): Skill-3D Rust plan + 100-reference analysis
  (`skill-3d/`), Episteme STEM pack review (`episteme/`), agent-skill design research and skill
  registries (`agent-skills-design/`).
- **Other repos**: dashboard split into components, migrations for companies/agent directory, AI org
  tab; BakeOS goal export + Vercel Workflow publisher (PRs #99/#126/#127/#128 merged); FlipsOS goals
  + Git auto-deploy; Shasta base-team delivery PR #4 (draft).

## Measurements & calibration

| Quantity | Value | Source |
|---|---|---|
| Workspace tests | 9,350 run / 9,350 pass / 24 skipped | `scripts/build.sh test` |
| Gate | 19/19 (16 before agent checks) | `scripts/build.sh gate` |
| v0.8.1 release | 87 assets (same as v0.8.0) | `gh release view v0.8.1` |
| `weftos init` weftos-core | 61 files + project-context seed | `--plan` output |
| Pi3 reconstruction | ~21 s per 7 frames vs ~1 s for other experts | Skill-3D paper App. B (shared GPU) |
| `bin/depth` (DA3METRIC-LARGE) | canonical depth for f=300 px @ 504×504; meters = canonical × f_504/300 | ~/llm session |

## Dead ends — do not retry

- **Publishing a local-DB snapshot to the live dashboard** — BakeOS's local script used the live
  `wfs_` token and replaced 30 real tickets with a 5+2 seed. It now refuses the production dashboard
  without `--i-am-publishing-production-source`.
- **`git checkout Cargo.toml Cargo.lock` to back out one change** — it reverts every change in those
  files; back out the specific edit instead.
- **zsh word-splitting** — `$VAR` holding `node script` or `set -- $r` does not split in zsh; use a
  function, `${=VAR}`, or `bash -c`.
- **`gh pr create` from the weave-logic-ai/ruflo fork** — failed with "Head sha can't be blank"; the
  GraphQL `createPullRequest` mutation with explicit repository ids worked.
- **`~/dev/ruflo-wt-team-bus-hosts` (feat/team-bus-codex-weftos, e4d732e42)** — the PRE-review team
  bus (v0 format, schemaVersion). Test against `~/dev/ruflo-pr-teambus` (#3513 head) instead.
- **Absolute local paths in tracked files** (e.g. `.grok/config.toml` MCP) — the repo is public and
  WEFT-684/669 forbid it; local overrides go in gitignored `.claude-flow/ruflo-cli-path`.
- **Tagging while fixes are still landing** — v0.8.1 was tagged and cancelled twice; confirm the
  tree and every worktree is settled before tagging.
- **wasmtime 46 for RUSTSEC-2026-0269** — needs Rust 1.94 (toolchain pinned 1.93); ignored with
  rationale (no FS preopens) pending ticket `8f1794da`.
- **`npm ci` at repo root** — fails on the committed lockfile too (npm 12, empty-version optional
  bindings); no CI job runs it. Use `npm install --allow-remote=all`.
- **Grounding DINO in Eikon / own SAM stack in WeftOS** — SAM 3.1 is served by `~/llm` (`bin/sam`);
  DINO stays an unbuilt catalog fallback.

## Open threads

1. **Apply the dashboard migration** `supabase/migrations/202609280001_agent_directory.sql` to live
   Supabase project `tjmgczbialndijqwewws` (SQL editor or a DB connection; this repo has no linked CLI).
   Done = `/api/harness/agents/*` stop erroring.
2. **Review/merge Shasta PR #4** (base team). Done = AD-7 closed.
3. **Test WeftOS 0.8.1 against ruflo** using `~/dev/ruflo-pr-teambus` (see Resume). Trust command
   hosts once: `ruflo team trust-host weft`.
4. **AD-3 evals**: run each base-team member's `evals/scenarios.md` on Claude, Grok, Codex; record
   score/completion/tokens per host. **Eikon** (`7f2ea05d`): one test image per host.
5. **Ruflo PRs** #3512/#3513/#3521/#3526 await maintainer review and fork-CI approval. After #3512
   lands, stack `split/3512-memory-provider`'s Grok bench/README commit on it.
6. **0.8.2**: clippy debt (`8dfd7ed5`), Rust 1.94 + wasmtime 46 (`8f1794da`), then cut 0.8.2.
7. **Skill-3D Rust rewrite**: start at R0.2 (Python reference fixtures, ticket `dc295594`); training
   is on hold (`65f8a922` collects training needs). **Episteme**: EP-1 sync, EP-2 router.
8. **User actions**: trust projects in Codex/Grok; top up or unset the empty `ANTHROPIC_API_KEY`.

## Resume here

```bash
cd /Users/mathewbeane/weftos
git branch --show-current            # 0.8-metaharness
git status --short                   # expect clean
node scripts/dashboard-board.mjs ready
scripts/build.sh agents-validate && scripts/build.sh agents-catalog --check
# render agents into a project (plan first, then --apply; --global targets ~/.claude, ~/.grok, ~/.codex)
./target/release/weftos init . --claude --grok --codex --plan    # or the released weftos binary
# ruflo testing build (reviewed PR head); WeftOS's gitignored .claude-flow/ruflo-cli-path points here
node ~/dev/ruflo-pr-teambus/v3/@claude-flow/cli/bin/cli.js team --help
```

## Key paths

- `agents/` — agent packages; `agents/teams/weftos-core/team.yaml`; generated `agents/catalog.json`
- `crates/weftos/src/init/` — the host renderer; `crates/weftos/build.rs` embeds `agents/`
- `scripts/agents-{validate,catalog}.mjs`, `scripts/agents-leak-check.sh` — gate checks 17–19
- `docs/research/agent-directory/` — design + Accepted ADR; `private/` is local-only (gitignored)
- `docs/research/skill-3d/README.md` — Skill-3D index and conclusions
- `~/dev/weftos-dashboard` — board, AI org tab, `supabase/migrations/`
- `~/llm` — owns local models (eikon, `bin/sam`, `bin/depth`, `bin/vlm`)
- `~/.claude/skills/skill-builder/references/skill-sources.md` — skill registries to search first

## Gotchas

- **The repo is public.** Run `scripts/agents-leak-check.sh`; confidential oil-rig/Deck Twin files are
  excluded via `.git/info/exclude` (local), not `.gitignore`.
- **Upstream ruflo PRs are one squashed commit on upstream main** (`git commit-tree`), force-pushed with
  `--force-with-lease=refs/heads/<branch>:<old>`; RuFlo attribution comes from ruflo's own settings.
- **`~/dev/ruflo` main checkout has ~145 uncommitted files** on `feat/grok-host` — not ours; never touch
  it. Use separate worktrees.
- **Agents renamed/new in this session appear as subagent types** (steward, doc-gardener, liber, mo,
  developer) because the base team is installed globally.
- **Shasta's main checkout has uncommitted work on `feat/wave`**; deliver via a temporary worktree.
- Memory: `~/.claude/projects/-Users-mathewbeane-weftos/memory/` (release state, branch workflow,
  search-skill-registries-first).
