# Higgsfield Skills — Reverse-Engineered Format Standard

Source: `higgsfield-ai/skills` (MIT, v0.13.0), cloned read-only at `~/dev/higgsfield-skills`, commit `f83af0bc` (2026-09-26). Eight skills, one repo, four host targets (Claude Code, Codex, Cursor, `npx skills`/`gh skill` generic). This document reverse-engineers their conventions into rules we can adopt for WeftOS skill packages (Agent Directory, Episteme, Skill-3D host wrappers). Companion doc: `higgsfield-video-and-generate.md` (deep dive on two specific skills). License/trademark notes at the end — read before reusing anything visual.

## 1. Repo layout

```
skills/
├── README.md                  # public description + skills table + quick-reference table
├── INSTALL.md                 # human-facing install, 4 options
├── INSTALL_FOR_AGENTS.md      # agent-driven install runbook (imperative, numbered)
├── CONTRIBUTING.md            # PR workflow, branch naming, PR checklist, "adding a skill"
├── CLAUDE.md                  # host-facing dev doc: repo map, API conventions, the 300-line rule
├── COOKBOOK.md                # end-to-end recipes, cross-skill chains
├── VERSION                    # single source of truth, bare semver string
├── LICENSE                    # MIT
├── setup                      # bash, idempotent, host-detecting installer
├── scripts/update-check.sh    # opt-in version-check, cache+snooze state machine
├── .claude-plugin/{plugin.json, marketplace.json}
├── .codex-plugin/plugin.json
├── .cursor-plugin/plugin.json
├── .github/{workflows/validate-skills.yml, ISSUE_TEMPLATE/, pull_request_template.md, CODEOWNERS}
├── evals/{README.md, scenarios.md}   # dev-only, not shipped
├── assets/{icon.svg, logo.png, README.md}
└── higgsfield-<skill>/
    ├── SKILL.md
    ├── references/*.md        # optional, on-demand
    ├── scripts/*.py           # optional, deterministic local tooling
    └── agents/openai.yaml     # optional, Codex-only interface metadata
```

Every `higgsfield-*/` folder is a **standalone, installable unit** — this is the load-bearing design decision the whole repo is built around (see §3).

## 2. Per-skill layout

| Path | Required | Purpose |
|---|---|---|
| `SKILL.md` | yes | Frontmatter + body. Loaded into agent context on every trigger. |
| `references/*.md` | no | On-demand detail: flag tables, prompt galleries, troubleshooting trees, model catalogs. Never auto-loaded. |
| `scripts/*.py` | no | Deterministic local tooling the skill shells out to (e.g. `higgsfield-brandkit/scripts/brandkit.py` for state, preview rendering, PPTX/PDF export). Keeps non-LLM work out of the model's hands. |
| `agents/openai.yaml` | no | Codex-only. A tiny `interface:` block (display name, one-line description, default prompt). Only 2 of 8 skills have it (`brandkit`, `youtube-thumbnail`) — it's opt-in polish, not mandatory. |

No skill folder contains a `.claude-plugin`-style manifest of its own — all plugin/marketplace metadata lives at the repo root and references the skill folders by relative path. A skill is just a folder; the repo root is what turns folders into an installable plugin.

## 3. SKILL.md frontmatter — exact schema

```yaml
---
version: 0.13.0                 # must equal repo-root VERSION (CI-enforced)
name: higgsfield-<skill>         # must equal directory name exactly (CI-enforced)
description: |                   # ≤1024 chars (CI-enforced), four required parts:
  <one paragraph: what it does and which API surface it wraps>
  Use when: "<trigger phrase>", "<trigger phrase>", ...
  Chain with: <other skill> when <condition>.
  NOT for: <case A> (use <skill A>), <case B> (use <skill B>).
argument-hint: "[primary-arg] [--flag <value>]"
allowed-tools: Bash
---
```

CI validates (`validate-skills.yml`, job "Validate frontmatter and name"): `name` == directory name; `version` present; `description` present, ≤1024 chars, and contains the literal substrings `"Use when"` and `"NOT for"`. `Chain with` is conventional (documented, not CI-checked). This is the single most portable piece of the whole standard — it's plain YAML frontmatter, no host-specific syntax, so it round-trips to Claude Code, Cursor, Codex, and (per §6) Grok unchanged.

## 4. Section order, tone, length

Every `SKILL.md` follows the same skeleton, in this order:

1. `# Title` — one line, no subtitle.
2. One-sentence "what this is a wrapper around."
3. **Bootstrap / Step 0** — CLI install check, auth check, live-schema inspection commands to run before any paid call.
4. **UX Rules** — numbered, imperative, terse (e.g. "Be concise. No raw IDs, no JSON dumps in chat," "Don't batch-ask," "Detect the user's language...").
5. **Workflow** — numbered steps, often with an embedded decision tree for model/mode selection expressed as a flat bulleted list (`condition → pick`, see §7).
6. Domain-specific sections (Marketing Studio, Virality Predictor, Phase pipelines — whatever the skill needs).
7. **Errors** — short list of `error string → fix`, deferring to `references/troubleshooting.md` for the long tail.
8. **Reference docs** — a flat bullet list, one line per `references/*.md` file, with a one-clause description of when to load it.

Tone: second person imperative, present tense, no marketing language, no hedging. Sentences are short. Numbers and enum values are backtick-quoted. Every claim about a command is written as something the agent actually runs, not paraphrased.

**The 300-line rule** (from `CLAUDE.md`, verbatim test): *"If removing a section from `SKILL.md` would NOT break the agent's ability to decide what to do next, it belongs in `references/`. If it WOULD break decision-making, it stays."* Measured: `higgsfield-generate/SKILL.md` is 322 lines (the biggest, because Marketing Studio is a sub-domain with its own concepts), `higgsfield-soul-id/SKILL.md` is 85 (simplest skill). Everything else falls between 88 and 281 lines. None of the eight exceed ~320.

## 5. Progressive disclosure — what's inline vs in `references/`

| Stays in `SKILL.md` | Moves to `references/` |
|---|---|
| Frontmatter | — |
| Stage/phase flow overview | Full flag tables per model/mode |
| Decision trees (model pick, mode pick, target detection) | Asset classification tables, routing matrices |
| UX rules that apply on every turn | Prompt galleries, style-preset libraries |
| Short pointers (`See references/X.md for details`) | Error-handling trees, troubleshooting detail |
| One or two runnable command examples | Anything only needed once, after the path is decided |

Mechanically, a reference is triggered two ways: (a) an inline pointer sentence at the point of decision ("See `references/model-catalog.md` for the full table"), and (b) the closing **Reference docs** bullet list, which is the canonical index — every reference file must appear there (CI-enforced, §9). There is no metadata-driven trigger system (no keywords-to-file map) — the pointer sentence *is* the trigger, written in prose at the exact decision point the agent would need it.

## 6. Tables, checklists, decision trees, examples

- **Decision trees** are never drawn as a tree — they're written as priority-ordered numbered lists: *"Match by intent, not surface keyword. When two could apply, the higher entry wins"* (`model-catalog.md` §Picking flow). Each rule is `condition → model/mode`, one line. Two explicit tiers recur everywhere: **auto-pick** (chosen without asking) vs **only when asked/named** (everything else). This tiering is the core routing idiom of the whole repo.
- **Tables** are the default format for anything enumerable: model catalogs (columns: Model / Provider / What it's for), media-role tables (Model / Accepted roles / Notes), flag tables (Flag / Purpose / Models that accept it), error tables. Tables are preferred over prose because they're grep/scan-friendly for an agent mid-task.
- **Worked examples** are runnable shell commands, not pseudocode — every `higgsfield ...` line in a doc is asserted (CONTRIBUTING §7: *"Every `higgsfield …` example in your SKILL.md or references must be a real, current command. Run it locally before merging."*).
- **Checklists** appear in CONTRIBUTING/PR template only, not in skill bodies — skill bodies use imperative rule lists instead of checkboxes.

## 7. Guardrails and "truthful" rules

A recurring pattern: **never invent, always verify against a live source.** Concretely:
- "Source of truth: never invent model or workflow names. Run `higgsfield model list` for the live model catalog... Reference catalogs... are mappings (intent → command), not the database." (`CLAUDE.md`)
- Scenario 10 in `evals/scenarios.md` is dedicated entirely to this: user names a fake model, agent must verify via `model list`, report it doesn't exist, suggest alternatives — never fabricate a fallback.
- Presets/voices/animation actions that are "live server-managed data" are explicitly forbidden from being embedded in a skill file — they must be listed live every time (`video-explainer` preset catalog, animation actions).
- UX guardrails are stated as negative rules with the reason attached: "Don't batch-ask across skills," "Don't pre-estimate cost... unless asked," "No raw IDs, no JSON dumps in chat."
- `higgsfield-video-explainer` adds research-integrity guardrails specific to generated content: "Research real topics before scripting. Do not invent quotes, dates, numbers, or events," with a dedicated Phase R (research) gate before scripting begins, and a required Sources list on delivery.

## 8. Error / troubleshooting sections

Two-tier pattern, consistent across every skill that has both:
1. A short `## Errors` section inline in `SKILL.md` — the 3-5 most common failure strings verbatim, each mapped to a one-line fix, ending with a pointer to the full reference.
2. `references/troubleshooting.md` — the same errors organized by category (Authentication / Validation / Job lifecycle / Rate limits / Infra-specific / Cost), each with the literal error string as the sub-heading so an agent can pattern-match output directly.

Two skills maintain **separate copies** of `troubleshooting.md` rather than sharing one (`higgsfield-generate` and `higgsfield-soul-id`) — this is intentional (§9, self-containment) and the repo explicitly accepts the resulting drift as a tradeoff for standalone installability.

## 9. Self-containment rule (the repo's core invariant)

*"Each skill folder is independent. No `../` parent-directory references. Every file in `references/` is reachable from that skill's `SKILL.md`. This lets each skill install standalone... even though we ship them together."* (`CLAUDE.md`). CI enforces three things per skill (`validate-skills.yml`): (a) every `references/X.md` linked from `SKILL.md` exists; (b) every file physically present in `references/` is linked from `SKILL.md` (no orphans); (c) no `../` or `../../` appears anywhere in `SKILL.md` or `references/`.

## 10. Per-host packaging

### `.claude-plugin/plugin.json` (Claude Code plugin identity)

Fields: `name`, `version`, `description`, `author.{name,url}`, `homepage`, `repository`, `license`, `keywords[]`. No `interface`/`skills` field here — that lives in `marketplace.json`.

### `.claude-plugin/marketplace.json` (Claude Code marketplace registration)

```json
{
  "name": "<marketplace-name>",
  "owner": {"name": "...", "url": "..."},
  "plugins": [{
    "name": "<plugin-name>",
    "source": "./",
    "description": "...",
    "version": "0.13.0",
    "skills": ["./higgsfield-generate", "./higgsfield-soul-id", "..."]
  }]
}
```
`plugins[0].skills` is the authoritative list of installable skill folders — CI verifies every `higgsfield-*/` folder appears here (job "Validate marketplace.json lists every skill folder").

### `.codex-plugin/plugin.json` (Codex)

Superset of the Claude manifest plus an `interface` block that's Codex's richer presentation layer:

```json
{
  "name": "...", "version": "...", "description": "...",
  "author": {"name": "...", "email": "...", "url": "..."},
  "homepage": "...", "repository": "...", "license": "...", "keywords": [...],
  "skills": "./",
  "interface": {
    "displayName": "Higgsfield AI",
    "shortDescription": "one line, ≤~60 chars",
    "longDescription": "one paragraph, plain prose, no markdown",
    "developerName": "...",
    "category": "Design",
    "capabilities": ["Read", "Write"],
    "websiteURL": "...",
    "defaultPrompt": ["<example prompt 1>", "<example prompt 2>", "..."],
    "brandColor": "#RRGGBB",
    "composerIcon": "./assets/icon.svg",
    "logo": "./assets/logo.png"
  }
}
```
`defaultPrompt` is a list of ready-to-click example prompts, one per major capability the plugin exposes — this is the closest thing in the repo to a "quickstart demo script."

### `.cursor-plugin/plugin.json` (Cursor)

Same shape as the Claude manifest plus `$schema`, `displayName`, `publisher`, `category` (flat string, e.g. `"ai-content"`, not Codex's `interface.category`). No `interface`/`skills`/`defaultPrompt` block — Cursor's manifest is the thinnest of the three.

### `agents/openai.yaml` (per-skill, Codex-only, opt-in)

```yaml
interface:
  display_name: "Higgsfield Brandkit"
  short_description: "Create complete visual brand systems"
  default_prompt: "Use $higgsfield-brandkit to create a complete visual identity and brand asset system for my business."
```
Three fields only. This is a per-skill override/supplement to the plugin-level `interface` block — used when a specific skill deserves its own one-click entry point distinct from the plugin's generic default prompt.

### `CLAUDE.md`

Not user documentation — it's the **contributor/agent-facing dev manual**: repo map, API conventions ("route through one binary," "never call the API directly"), the 300-line rule, version-sync table, skill-chaining rules, and "Adding a new skill" pointer. Read by an agent working *on* the repo, not by an agent *using* the skills.

### `INSTALL.md` vs `INSTALL_FOR_AGENTS.md`

| | `INSTALL.md` | `INSTALL_FOR_AGENTS.md` |
|---|---|---|
| Audience | Human reading GitHub | An agent, told to paste this file's contents into itself and execute |
| Voice | "Pick one. Each method handles..." (descriptive, offers choices) | "You are an AI coding agent... Follow this exactly." (imperative, single path) |
| Structure | 4 numbered *options*, verify, update-matrix | 5 numbered *steps*, each with a "Verify: expect \<output\>" line |
| Ends with | Update-command table per install method | Explicit "do NOT explain internals... just confirm install + give starter prompts" instruction |

This split — one doc a human reads to choose, one doc an agent executes verbatim — is a reusable idiom worth adopting directly.

## 11. Quality machinery

### `evals/README.md` + `evals/scenarios.md`

Dev-only (not shipped). No automated runner — this is a **manual eval protocol**, not a test suite. Format per scenario: `User request` (verbatim prompt) → `Expected behavior` (bulleted, what the agent should do and *not* do) → `Score` (Pass / Partial / Fail, each with a concrete example of what qualifies). 14 scenarios across the 8 skills. `README.md` defines the methodology: a **Round** is a scored run of the full scenario set against one commit; regression threshold is ">15% score drop or >2× time = revert and investigate." Findings that survive 3+ stable rounds get promoted into `CLAUDE.md`'s "Key Decisions (Do Not Revisit Without Data)" section — an explicit empirical-only-changes-defaults discipline: *"Adding here means the next contributor cannot revert without showing other numbers."*

### `.github/workflows/validate-skills.yml`

Five CI jobs, all on PRs touching `higgsfield-*/**`, `.claude-plugin/**`, `.codex-plugin/**`, `.cursor-plugin/**`, or `VERSION`:
1. Frontmatter validity (name/version/description/Use-when/NOT-for, Python + PyYAML).
2. Version sync across `VERSION`, every `SKILL.md`, and all 4 manifest files (`marketplace.json` is nested at `plugins[0].version`).
3. `marketplace.json` lists every skill folder.
4. Every `references/X.md` link resolves; no orphan files.
5. No `../` parent-dir references anywhere.

### `scripts/update-check.sh` + `VERSION`

Bare semver string is the single source of truth (§version sync, §CI job 2). `update-check.sh` is a defensive opt-in polling script: caches remote `VERSION` with a TTL (60min if up-to-date, 720min if an upgrade is pending — "nag less" once the user knows), snoozes at three escalating levels (24h/48h/7d) reset by a new remote version, and emits one of three states (`JUST_UPGRADED`, `UPGRADE_AVAILABLE`, silent). No auto-update — it only surfaces the signal.

### `CONTRIBUTING.md` + PR/issue templates

`CONTRIBUTING.md`: git workflow (branch-prefix convention `feat/fix/refactor/docs`, Conventional Commits for future `release-please` automation), a 9-point PR checklist, and an "Adding a new skill" section with a literal `SKILL.md` skeleton to copy. PR template mirrors the checklist as tickable boxes plus What/Why/Changes/Type/Testing/Breaking-changes sections. Issue templates: `bug_report.md` (which-skill checkboxes, repro steps, environment block with CLI version + skills VERSION), `feature_request.md` (which-skill, what-would-change, why, a literal "what you'd say to the agent" quoted block, acceptance checklist), `new_skill_request.md` (proposed name, use-when triggers, chain rules, **a mandatory "why this is a separate skill, not an addition to an existing one" question** — the repo's bias toward fewer, broader skills over many narrow ones — plus a `SKILL.md` sketch). `CODEOWNERS` maps every top-level path (per-skill, manifests, CI, top-level docs) to one team.

## 12. Grok support

**None exists in the Higgsfield repo** — no `.grok-plugin/`, no Grok mentions anywhere in the 8 skills, manifests, or docs. Grok is not one of their four targets.

Mapping a Grok variant, using what's already true in this codebase (`.grok/skills/` and the `grok-claude-sync` skill at `~/.claude/skills/grok-claude-sync`):

- **No translation needed for the skill body.** `grok-claude-sync` states plainly: *"`SKILL.md` and command-`.md` files use the same format on both sides — no tool-specific frontmatter to translate."* A Higgsfield-standard `SKILL.md` (frontmatter + body as in §3-§4) would work unmodified if copied into `.grok/skills/<name>/SKILL.md`.
- **No manifest layer exists or is needed.** Grok has no plugin-marketplace concept analogous to `.claude-plugin/marketplace.json` or a per-plugin `interface` block like Codex's. It reads `.grok/skills/<name>/SKILL.md` directly from the project tree — closer to a filesystem convention than an installable package.
- **What *would* need translation is agent/comms language, not skill format** — per `grok-claude-sync` §2, Claude-side agent bodies reference `Task`/`SendMessage`; Grok-side agent bodies reference `spawn_subagent`/ "Ruflo team bus." This matters if a Higgsfield-standard skill's body invokes host-specific coordination primitives (none of the 8 Higgsfield skills do — they're single-agent, CLI-driving skills, which is exactly why they'd port cleanly).
- **Versioning/CI would need a project-local equivalent.** Higgsfield's `validate-skills.yml` is GitHub-Actions-specific; a WeftOS Grok/Claude dual target would want the same five checks run by whatever CI this repo already uses (`scripts/build.sh gate` is the closest analog) rather than inventing a second workflow file.
- Recommendation: build WeftOS skill packages Claude-first per §13 below, and treat "does it work unmodified in `.grok/skills/`" as a `references/` self-containment smoke test rather than a separate packaging track — this is the cheapest way to get de facto Grok support for free.

## 13. WeftOS skill package standard v0

Rules, adopted from the above, for Agent Directory / Episteme / Skill-3D host wrappers:

1. One skill = one folder = one topic. Prefer fewer, broader skills; require a written "why is this not an addition to an existing skill" justification before creating a new folder (mirrors the Higgsfield `new_skill_request` template's mandatory question).
2. `SKILL.md` frontmatter: `version`, `name` (== folder name), `description` with `Use when:` / `Chain with:` / `NOT for:` clauses, `argument-hint`, `allowed-tools`. Enforce with a CI/lint step mirroring `validate-skills.yml` job 1 (name match, required substrings, length cap).
3. Apply the 300-line test verbatim: if cutting a section doesn't break decision-making, it moves to `references/`.
4. Every skill is self-contained: no `../` references, every `references/` file is both linked-from and reachable-from `SKILL.md`. Lint both directions in CI.
5. Decision trees are priority-ordered bullet lists with an explicit **auto-pick vs named-only** tier split, not free prose.
6. Every runnable example must be a real command, verified before merge (no pseudocode in skill bodies).
7. Two-tier errors: a short inline `## Errors` list plus a `references/troubleshooting.md` organized by category with literal error strings as headings.
8. Guardrail discipline: never let a skill hardcode a catalog that has a live source of truth (model lists, presets, voices) — always point at the live discovery command instead of embedding a snapshot.
9. Package for two audiences: a human-facing `INSTALL.md` (choices) and an agent-facing `INSTALL_FOR_AGENTS.md` (imperative steps with verify lines), if the package has any install/auth step beyond "the skill file is present."
10. Version sync: one root `VERSION` file; every `SKILL.md` and manifest echoes it; CI fails on drift. Don't hand-bump on feature branches.
11. Maintain a manual `evals/scenarios.md` per skill package from day one — request → expected behavior → pass/partial/fail — even before there's automation to run it. Promote stable findings into a "Key Decisions" block in the package's dev doc.
12. Treat `.grok/skills/<name>/SKILL.md` compatibility as a free byproduct of self-containment (§12), not a separate deliverable.

### Template skeleton

```
weftos-<skill>/
├── SKILL.md
├── references/
│   ├── <catalog-or-routing-table>.md
│   ├── prompt-or-usage-patterns.md
│   └── troubleshooting.md
├── scripts/                      # optional: deterministic local tooling
└── agents/
    └── openai.yaml                # optional, Codex-only polish
```

```yaml
---
version: 0.1.0
name: weftos-<skill>
description: |
  <one paragraph: what it does, what API/CLI/surface it wraps>
  Use when: "<trigger phrase>", "<trigger phrase>", ...
  Chain with: <other skill> when <condition>.
  NOT for: <case A> (use <skill A>), <case B> (use <skill B>).
argument-hint: "[primary-arg] [--flag <value>]"
allowed-tools: Bash
---

# <Title>

<One sentence: what this wraps.>

## Bootstrap
1. <install/auth checks>

## UX Rules
1. Be concise...
2. Don't batch-ask...

## Workflow
1. <decision tree, auto-pick vs named-only>
2. <submit/validate/deliver>

## Errors
- <error string> → <fix>

## Reference docs
- `references/<file>.md` — <when to load it>
```

## 14. License and trademark constraints

- The Higgsfield skills repo itself is MIT (`LICENSE`, copyright Higgsfield AI 2026) — the *format* (frontmatter schema, directory layout, CI checks, doc structure) is freely reusable; that's what this document extracts.
- **Do not reuse Higgsfield's name, logo, or brand assets.** `assets/` (`icon.svg`, `logo.png`, brand color `#D1FE17`) and the `interface.logo`/ `composerIcon` references in the plugin manifests are Higgsfield's brand identity, sourced per `assets/README.md` from "internal Higgsfield brand assets (Figma / brand kit)" — proprietary and outside the MIT grant on the code. Any WeftOS package built from this standard needs its own name, icon, and brand color; do not copy `assets/logo.png`, `assets/icon.svg`, or the `#D1FE17` brand color, and do not use "Higgsfield" in a WeftOS skill's `displayName`/`developerName`/`author` fields.
- The repo's `interface.developerName`/`author` fields for a WeftOS-derived package should read "WeftOS" (or the appropriate sub-project), not "Higgsfield AI" — those fields are per-package identity, not shared infrastructure.


