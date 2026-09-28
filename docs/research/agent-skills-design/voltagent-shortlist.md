# VoltAgent awesome-agent-skills: shortlist for WeftOS

Source: [VoltAgent/awesome-agent-skills](https://github.com/VoltAgent/awesome-agent-skills) (README
fetched 2026-09-28 via `gh api repos/VoltAgent/awesome-agent-skills/readme`). Full parsed catalog:
[voltagent-catalog.tsv](voltagent-catalog.tsv) — 1109 entries across 71 categories (17 official-team
sections + a "Community Skills" umbrella with 7 sub-categories: Vector Databases, Marketing,
Productivity and Collaboration, Development and Testing, Context Engineering, Specialized Domains,
n8n Automation). The README's own badge claims "1497+"; 1109 is the count of parseable bullet
entries — the gap is likely stale badge text or entries folded into multi-skill repos counted once
here.

License/date checks below use `gh api repos/OWNER/REPO --jq '.license.spdx_id, .pushed_at'` run
2026-09-28. Several "official" entries route through **officialskills.sh** (a VoltAgent-run
directory — see Registries, below) rather than linking GitHub directly; where that's true the real
source repo is resolved and noted.

Existing overlap checked against `~/.claude/skills/` (164 skills), `/Users/mathewbeane/weftos/.claude/skills/`
(35 skills), and plugins under `~/.claude/plugins/marketplaces/` (`claude-plugins-official`,
`cloudflare`, `media-pipeline-marketplace`).

Legend: **ADOPT** = use close to as-is · **ADAPT** = worth building our own version informed by it ·
**PATTERN** = no import, just a useful idea/structure · **SKIP** = not worth pursuing, with reason.

## (a) Spatial / 3D / geospatial / CV / image & video analysis

| Skill | Repo | License | Pushed | Verdict — reason |
|---|---|---|---|---|
| fal-3d, fal-vision, fal-realtime, fal-train | [fal-ai-community/skills](https://github.com/fal-ai-community/skills) | none detected | 2026-05-13 | PATTERN — fal.ai API wrapper shape (3D-from-image, segment/detect/OCR/VQA) is a clean reference for a CV-analysis skill, but no repo license means don't vendor code |
| hugging-face-vision-trainer | [huggingface/skills](https://github.com/huggingface/skills) | Apache-2.0 | 2026-09-25 | ADAPT — vision-model fine-tuning workflow on HF infra; useful structure for a Skill-3D/image-analysis training skill, cloud-infra-specific so don't adopt verbatim |
| threejs-skills | [CloudAI-X/threejs-skills](https://github.com/CloudAI-X/threejs-skills) | none detected | 2026-07-09 | SKIP — no license, single small repo; Three.js patterns are easy to reproduce directly if the Urth/dashboard frontend ever needs client-side 3D |
| azure-ai-vision-imageanalysis-py | [microsoft/skills](https://github.com/microsoft/skills) | MIT | 2026-09-24 | PATTERN — captioning/OCR/object-detection skill shape; cloud-locked to Azure so not directly reusable |

No true geospatial/point-cloud/photogrammetry/SLAM skills surfaced in the catalog — this is a real
gap relative to Skill-3D, the Urth twin, and MentraOS capture work, not a shortlisting miss.

## (b) Science / STEM

| Skill | Repo | License | Pushed | Verdict — reason |
|---|---|---|---|---|
| scientific-agent-skills | [K-Dense-AI/scientific-agent-skills](https://github.com/K-Dense-AI/scientific-agent-skills) | MIT | 2026-09-21 | SKIP (duplicate) — already reviewed at `docs/research/episteme/inventory.md`; per task scope, not re-reviewed here |
| materials-simulation-skills | [HeshamFS/materials-simulation-skills](https://github.com/HeshamFS/materials-simulation-skills) | Apache-2.0 | 2026-06-25 | PATTERN — numerical-stability/mesh-gen/validation skill structure; no current Episteme workstream needs computational materials science |
| jupyter-notebook | [openai/skills](https://github.com/openai/skills) | none detected | 2026-09-08 | PATTERN — reproducible-notebook conventions; already have `openai/skills` cataloged generally in skill-sources.md |

## (c) Software-engineering lanes (review, testing, docs, release, board/ticket, memory, security)

| Skill | Repo | License | Pushed | Verdict — reason |
|---|---|---|---|---|
| code-review | [coderabbitai/skills](https://github.com/coderabbitai/skills) | MIT | 2026-09-28 | ADAPT — CLI-driven AI review skill; compare technique against our own `github-code-review`, don't replace it (ours is swarm-integrated) |
| review, qa, document-release | [garrytan/gstack](https://github.com/garrytan/gstack) | MIT | 2026-09-28 | ADOPT-candidate — staff-eng review / QA-fix-with-atomic-commits / doc-sync-on-ship are exactly our SE-lane shape; read closely against `github-code-review`, `github-release-management` before merging ideas in |
| semgrep-rule-creator, semgrep-rule-variant-creator, property-based-testing, testing-handbook-skills | [trailofbits/skills](https://github.com/trailofbits/skills) | CC-BY-SA-4.0 | 2026-09-25 | ADAPT — strong security-testing patterns from a top-tier auditing firm; CC-BY-SA is share-alike, so treat as reference to reimplement, not verbatim import, if any derivative work is redistributed |
| Anthropic-Cybersecurity-Skills (753 skills, MITRE ATT&CK mapped) | [mukul975/Anthropic-Cybersecurity-Skills](https://github.com/mukul975/Anthropic-Cybersecurity-Skills) | Apache-2.0 | 2026-08-31 | ADAPT — huge surface area for a `security-auditor` lane; cherry-pick individual skills after review rather than bulk-import (quality across 753 skills is unverified) |
| skillreaper | [thousandflowers/skillreaper](https://github.com/thousandflowers/skillreaper) | MIT | 2026-08-30 | **ADOPT** — prunes unused skills/MCP servers/subagents from transcript evidence; directly addresses our own sprawl (164 skills in `~/.claude/skills/`) |
| skill-optimizer | [hqhq1025/skill-optimizer](https://github.com/hqhq1025/skill-optimizer) | MIT | 2026-05-14 | ADAPT — diagnoses/optimizes SKILL.md files from real session data; complements `skill-builder` |
| skills-library (guided-discovery interview) | [lindblomstefan/skills-library](https://github.com/lindblomstefan/skills-library) | MIT | 2026-09-06 | PATTERN — interview-to-recommend UX over a skill catalog; useful idea for surfacing our own 164+ skills, not a needed import |
| finding-unknowns-skills | [Neeeophytee/finding-unknowns-skills](https://github.com/Neeeophytee/finding-unknowns-skills) | MIT | 2026-09-28 | ADAPT — blindspot pass / interview / reference hunt meta-skills complement `idea-wizard` and `planning-workflow` |
| github-image-upload | [drogers0/gh-image](https://github.com/drogers0/gh-image) | MIT | 2026-09-09 | **ADOPT** — small, scoped utility: attach screenshots/PDFs/logs/videos to GitHub PRs/issues via canonical `user-attachments` URLs (no public GitHub API for this); low risk, immediately useful for PR workflows |
| claude-memory-skill | [hanfang/claude-memory-skill](https://github.com/hanfang/claude-memory-skill) | MIT | 2026-02-07 | PATTERN only — filesystem-based hierarchical memory with background agents; we already have deeper AgentDB-backed memory (`agentdb-memory-patterns`, `reasoningbank-agentdb`), so this is a downgrade, not an upgrade |
| skills (Node/Fastify/TS/Git/skill-optimizer bundle) | [mcollina/skills](https://github.com/mcollina/skills) | MIT | 2026-08-17 | PATTERN — well-known maintainer's bundle; narrow relevance given WeftOS is Rust-first, but the skill-optimizer sub-skill is worth a look |
| release-notes | [phuryn/pm-skills](https://github.com/phuryn/pm-skills) | MIT | 2026-09-14 | PATTERN — ticket/changelog → release notes generator; overlaps `changelog-md-workmanship`, no clear upgrade |
| write-concisely, review (PR multi-agent) | [NeoLabHQ/context-engineering-kit](https://github.com/NeoLabHQ/context-engineering-kit) | **GPL-3.0** | 2026-08-26 | SKIP for adoption — copyleft license unsuitable for vendoring into a closed skill set; read-only for pattern ideas if ever revisited |
| eskill (meta-skill builder w/ eval loop) | [hedralab/eskill](https://github.com/hedralab/eskill) | NOASSERTION (no license) | 2026-09-26 | SKIP — no license means no legal basis to reuse; the eval-loop + validator idea is worth re-deriving independently for `skill-builder` |
| task-observer / DAGx control kernel | [rebelytics/one-skill-to-rule-them-all](https://github.com/rebelytics/one-skill-to-rule-them-all) | CC-BY-4.0 | 2026-09-27 | ADAPT — hard-stop-on-irreversible-actions + evidence-gated completion + drift governance maps onto our metaharness flywheel; CC-BY only requires attribution, so reuse is workable |

## (d) Rust / embedded / ESP32 / WASM

| Skill | Repo | License | Pushed | Verdict — reason |
|---|---|---|---|---|
| rust-best-practices | [apollographql/skills](https://github.com/apollographql/skills) | MIT | 2026-09-26 | ADAPT — general Rust guidelines from Apollo's internal handbook; supplementary reference, not workspace-specific (no `scripts/build.sh` awareness) |
| makepad-skills | [ZhangHanDong/makepad-skills](https://github.com/ZhangHanDong/makepad-skills) | none detected | 2026-04-07 | SKIP — no license; Makepad (Rust GUI) isn't in our stack today |

No ESP32/embedded-Rust/no_std/Embassy/WASM-specific skills appeared anywhere in the catalog. This
is a genuine gap — our embedded-rust-expert cluster and `crates/clawft-wasm` browser work have no
counterpart here to adopt from.

## (e) Agent-ops and meta (skill creation/eval, MCP servers, CLI tooling)

| Skill | Repo | License | Pushed | Verdict — reason |
|---|---|---|---|---|
| mcp-builder | [anthropics/skills](https://github.com/anthropics/skills) | none detected at repo root (per-skill licensing; docx/pdf/pptx/xlsx are Anthropic-licensed) | 2026-09-24 | Already known — listed in `skill-sources.md` "Official collections"; no new action |
| mcp-builder | [microsoft/skills](https://github.com/microsoft/skills) | MIT | 2026-09-24 | PATTERN — second MCP-server-authoring reference; compare against our own `mcp-server-design` skill |
| skill-creator | [apollographql/skills](https://github.com/apollographql/skills) | MIT | 2026-09-26 | PATTERN — Apollo-flavored skill scaffolding; we already have `skill-builder` |
| zero (paid-tool discovery for agents) | [officialzeroxyz/zero-plugins](https://github.com/officialzeroxyz/zero-plugins) | none detected | 2026-09-08 | SKIP — no license, and "agent autonomously signs up for/pays for external tools" conflicts with our controlled-tool-use posture |

## (f) Vercel / Next.js / Supabase / Cloudflare (dashboard stack)

| Skill | Repo | License | Pushed | Verdict — reason |
|---|---|---|---|---|
| next-best-practices, next-cache-components, next-upgrade | [vercel-labs/next-skills](https://github.com/vercel-labs/next-skills) | none detected | 2026-09-16 | ADAPT — official Vercel team patterns, directly on our stack; no repo license so read for guidance and reimplement rather than vendor files, fold into our `vercel`/`nextjs` skills |
| postgres-best-practices | [supabase/agent-skills](https://github.com/supabase/agent-skills) | MIT | 2026-09-24 | **ADOPT** — official, MIT, directly matches our `supabase` skill's Drizzle+pooler/RLS focus; merge in |
| security-audit-skill | [cloudflare/security-audit-skill](https://github.com/cloudflare/security-audit-skill) | MIT | 2026-09-14 | **ADOPT** — multi-phase security audits with independently verified, machine-readable findings; genuinely new capability, no equivalent in our current Cloudflare skill set |
| sandbox-sdk | [cloudflare/skills](https://github.com/cloudflare/skills) | Apache-2.0 | 2026-09-26 | ADAPT — sandboxed code execution on Workers; overlaps our existing `cloudflare` mega-skill, fold in as a section rather than adopting standalone |
| sentry-nextjs-sdk, sentry-cloudflare-sdk | [getsentry/sentry-for-ai](https://github.com/getsentry/sentry-for-ai) | MIT | 2026-09-24 | SKIP for now — Sentry isn't a current WeftOS dependency; note as available if that changes |

## (g) Media (image/video generation and analysis)

| Skill | Repo | License | Pushed | Verdict — reason |
|---|---|---|---|---|
| venice-image-generate, venice-video | [veniceai/skills](https://github.com/veniceai/skills) | MIT | 2026-09-28 | ADAPT — privacy-positioned image/video-gen + transcription API; viable alternate provider skill alongside `media-pipeline-marketplace` |
| fal-3d, fal-vision, fal-realtime, fal-train | [fal-ai-community/skills](https://github.com/fal-ai-community/skills) | none detected | 2026-05-13 | PATTERN — see (a); same no-license caveat applies to the media-gen side |
| superCMO-skills (marketing video/image production, AI actors) | [SupercmoHQ/superCMO-skills](https://github.com/SupercmoHQ/superCMO-skills) | Apache-2.0 | 2026-08-28 | PATTERN — well-licensed but marketing-campaign-skewed (UGC/ad video), not core WeftOS media-analysis need |

## Top 15 overall picks

1. **cloudflare/security-audit-skill** (MIT) — new capability, directly on our Cloudflare stack.
2. **thousandflowers/skillreaper** (MIT) — solves our own 164-skill sprawl problem.
3. **drogers0/gh-image** (MIT) — small, scoped, immediately useful GitHub-attachment utility.
4. **garrytan/gstack** `review`/`qa`/`document-release` (MIT) — strong SE-lane reference to diff against our existing review/release skills.
5. **coderabbitai/skills** `code-review` (MIT) — second opinion on our `github-code-review` approach.
6. **supabase/agent-skills** `postgres-best-practices` (MIT) — official, direct fit for our `supabase` skill.
7. **veniceai/skills** image/video (MIT) — alternate media-gen provider pattern.
8. **trailofbits/skills** `semgrep-rule-creator` + `property-based-testing` (CC-BY-SA-4.0) — security-testing patterns from a top auditing firm.
9. **mukul975/Anthropic-Cybersecurity-Skills** (Apache-2.0) — large, cherry-pickable security corpus.
10. **rebelytics/one-skill-to-rule-them-all** (CC-BY-4.0) — evidence-gated, drift-governed control-kernel pattern for the metaharness flywheel.
11. **hqhq1025/skill-optimizer** (MIT) — SKILL.md diagnostics, complements `skill-builder`.
12. **lindblomstefan/skills-library** (MIT) — guided-discovery UX pattern for our own skill catalog.
13. **apollographql/skills** `rust-best-practices` (MIT) — supplementary Rust guidelines.
14. **huggingface/skills** `hugging-face-vision-trainer` (Apache-2.0) — vision-training skill shape for image-analysis work.
15. **Neeeophytee/finding-unknowns-skills** (MIT) — blindspot/unknowns meta-skills, complements `idea-wizard`/`planning-workflow`.

## Registries and directories not yet in skill-sources.md

- **[officialskills.sh](https://officialskills.sh)** — "Official Agent Skills Directory," run by
  VoltAgent (same org as this catalog). Aggregates official-team skills (Anthropic, Cloudflare,
  Stripe, Microsoft, OpenAI, Supabase, Vercel, and more) with a per-skill page that links the real
  source GitHub repo. Checked 2026-09-28, HTTP 200. Appended to the registries table below.

No other new registries surfaced — `modem.dev/go/awesome-agent-skills` is a redirect link to this
same VoltAgent list, not an independent directory, so it wasn't added.

Rows appended to both `~/.claude/skills/skill-builder/references/skill-sources.md` and
`/Users/mathewbeane/weftos/.claude/skills/skill-builder/references/skill-sources.md`:

| Source | What it is | How to search |
|---|---|---|
| [officialskills.sh](https://officialskills.sh) | VoltAgent's directory of official developer-team skills, with per-skill pages linking the real source repo | Browse by team, or resolve `officialskills.sh/<org>/skills/<name>` to its GitHub source with a page fetch |
