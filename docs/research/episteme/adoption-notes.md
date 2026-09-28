# Episteme (working name) — adoption notes for WeftOS

How `k-dense-ai/scientific-agent-skills` (166 skills, MIT, reviewed read-only at
`~/dev/scientific-agent-skills`) could land in WeftOS as a STEM pack for Claude Code,
Grok, and Codex. `inventory.md` has per-skill data; `security-review.md` has what to strip/flag.

## 1. Format compatibility per host

### Claude Code
Direct fit. Claude Code's `Skill` tool loads any directory with a conformant `SKILL.md`
(`name`, `description`, optional `license`/`compatibility`/`allowed-tools`/`metadata`) —
exactly what this repo ships, since it was built against the same open Agent Skills spec
Claude Code implements. `references/` and `scripts/` map onto Claude Code's own
progressive-disclosure convention (load `SKILL.md` first, pull `references/*.md` only
when needed, run `scripts/*.py` via Bash). The repo's `plugin.json` also makes the whole
tree loadable as **one plugin** — this project's `Skill` tool listing already shows the
`plugin:skill` namespacing pattern in use (`cloudflare:wrangler`,
`ruvnet-brain:brain-score`) — so `scientific-agent-skills` could land as a single plugin
(`science:rdkit`, `science:scanpy`, ...) rather than 166 individually-copied directories,
keeping upstream provenance visible and avoiding name collisions.

**Two fields Claude Code does not act on**: `metadata.openclaw` (26 skills) and
`metadata.hermes` (0 skills present, but documented) are credential-gating blocks for a
different host family. Claude Code will just see them as inert metadata — harmless, but
it means the one place these skills declare "this specific env var is required before you
can use this skill" in a structured, host-checkable way goes unused. If WeftOS wants that
gating behavior, it has to be built (e.g. a session-start check reading
`metadata.openclaw.envVars` before a credentialed skill is offered), not inherited for
free.

### Codex
Partial fit, translation required. Codex's primary extensibility surface in this project
is `AGENTS.md` (read, and per its documented behavior, read nested as context narrows into
subdirectories) plus MCP servers (`.codex/config.toml`, `.codex/agents/*.toml`). Codex
lacks Claude Code's/Grok's `SKILL.md`-per-directory, frontmatter-triggered auto-discovery
as a first-class primitive here. The repo's Codex-compatibility claim holds at the content
level (a `SKILL.md` is readable markdown) but not at *discovery* — Codex won't notice a
new skill in a `skills/` tree unaided. Two paths, in order of drift created: (1) **a root
or per-domain `AGENTS.md` index** listing skill directories and when to open each
`SKILL.md` — low effort, mirrors what Codex already reads; (2) **per-skill `AGENTS.md`**
(generated from `SKILL.md`) if Codex is confirmed to load nested `AGENTS.md` on demand in
the version WeftOS runs — needs verifying against the actual Codex CLI, unconfirmed from
this repo alone. Either way `allowed-tools`/`metadata` have no Codex equivalent and would
be restated as prose in the index — §6 below gives Codex a real per-skill manifest
(`agents/openai.yaml`) as a third option, borrowed from Higgsfield.

### Grok
Same underlying format, thinner frontmatter, no packaging convention.
`/Users/mathewbeane/weftos/.grok/skills/*/SKILL.md` uses only `name` + `description` —
confirmed by reading `agent-teams-grok/SKILL.md` — no `license`, `compatibility`,
`allowed-tools`, or `metadata`. Grok skills are also flatter: no repo-wide `references/`
vs `scripts/` split is enforced, and there's no Grok equivalent of `plugin.json` bundling.
So a scientific-agent-skills `SKILL.md` is *readable* by Grok as-is (extra frontmatter
keys are presumably just ignored, matching how Claude Code ignores `metadata.openclaw`),
but Grok gets none of the `allowed-tools` tool-scoping or `metadata.version` change
tracking, and there's currently nothing in `.grok/skills/` from any external pack to use
as precedent for "how does a vendored third-party skill collection look on the Grok
side" — `.grok/skills/` today holds only WeftOS/Ruflo-authored skills
(`agent-teams-grok`, `handoff`, `metaharness-tasks`, `plane-dag`).

The `grok-claude-sync` skill (`~/.claude/skills/grok-claude-sync/SKILL.md`) is directly
relevant here even though it wasn't built for this: it already documents that
`SKILL.md`/command `.md` files "use the same format on both sides — no tool-specific
frontmatter to translate," and ships a mechanical, non-destructive mirror
(`~/.claude/helpers/grok-claude-sync.cjs apply`) that copies missing skills from `.grok/`
to `.claude/` or vice versa without overwriting anything that exists on both sides. That
tool is oriented at WeftOS-authored skills drifting between the two trees, not at
importing an *external* 166-skill pack, but the mechanical-copy half of it (skills/commands
are format-identical, safe to copy) is exactly the operation needed to mirror an ingested
`skills/<name>/` tree from `.claude/skills/` into `.grok/skills/` once it's landed on the
Claude side, provided `allowed-tools`/`metadata` being inert-but-harmless on the Grok side
is accepted (recommended — don't strip them, just don't expect them to do anything there).

### Summary table

| Field | Claude Code | Grok | Codex |
|---|---|---|---|
| `name`/`description` (discovery) | native | native (thinner, but reads fine) | not auto-discovered; needs an `AGENTS.md` pointer |
| `license`/`compatibility` | read, informs allowed-tools gating conventions this project already uses | ignored, harmless | prose only, via index |
| `allowed-tools` | native (Claude Code convention) | ignored | no equivalent |
| `metadata.version` | informational | informational | informational |
| `metadata.openclaw`/`hermes` | inert (not an OpenClaw host) | inert | inert |
| `references/`, `scripts/` | native progressive disclosure | works as plain files, no enforced split | works as plain files |

## 2. Proposed tiering

**Core** (bring in now, low risk, clear WeftOS fit):
- `database-lookup` (78 curated database references, ToS-aware, MIT, no scripts to
  execute beyond documented REST calls) — broadly useful reference material regardless of
  domain.
- Pure-analysis/stdlib-only skills with `explicit_no_network=true` and no credential need
  (per `inventory.md`'s Flags column, roughly the 20+ skills with no `N`/`$` flag and
  `stdlib only` deps) — e.g. `analytical-method-validation`, `genomic-coordinates`,
  `clinical-decision-support`, `statistical-power`, `experimental-design`. Lowest attack
  surface in the whole corpus: no network, no credentials, no third-party package to
  drift.
- `geomaster`/`geopandas` — direct overlap with Urth's geospatial/remote-sensing world
  (see §3) and both are MIT with no paid/gated dependency.
- `docx`/`pdf`/`pptx`/`xlsx` are **excluded**, not Core — see the binding ingest rules
  (§6): Anthropic-licensed, not MIT, and redundant with Claude Code's own first-party
  copies (`security-review.md` §6).

**Optional** (bring in on demand, per-domain, credentialed or heavier):
- Any skill flagged `needs_api_key`/`needs_paid` in `inventory.md` (47 and 5 skills
  respectively) — real utility (`benchling-integration`, `exa-search`,
  `dnanexus-integration`, etc.) but each one adds a credential WeftOS has to provision and
  a vendor relationship it doesn't currently have. Pull in per-project as a specific need
  arises, not as a blanket import.
- Domain packages with heavy/conflicting native dependencies (per
  `tests/skill-requirements.toml`'s own documented conflicts: `opentrons` needs
  `numpy<2`, `esm` caps `transformers`, `geniml`/`spikeinterface` pin `zarr<3` against
  `zarr-python`'s 3.x, `pytdc`/`molfeat`/`deepchem`/`histolab`/`vaex`/`ete3` each need an
  older interpreter). These are fine individually but should not be assumed
  co-installable in one WeftOS Python environment — the upstream repo itself solves this
  with one throwaway `uv` env per skill; WeftOS would need the same discipline if it ever
  actually executes these scripts rather than just reading them as agent guidance.

**Excluded**: `autoskill` (upstream's own triage still flags residual risk — a
user-configured foundry endpoint, HTTPS-only isn't the same as trusted — leave out until
there's a concrete WeftOS use); `primekg` (`license: Unknown` in its own frontmatter,
exclude until resolved); and, by default, anything requiring a credential WeftOS has no
intent to provision. The self-citation directive is stripped from every tier on ingest
(§6 ingest rules) — it's a mechanical edit, not a reason to exclude the skill.

## 3. Overlap with existing WeftOS domains

- **Sonobuoy / underwater acoustics** (`.planning/sonobuoy/`) — **no overlap**. A
  targeted grep for acoustic/underwater/hydrophone/sonar terms across all 166 `SKILL.md`
  files returned no dedicated skill; the two incidental hits (`ginkgo-cloud-lab`,
  `infographics`) are unrelated mentions, not acoustics content. This pack does not
  reduce or duplicate the acoustic-DSP/marine-acoustics expert work already in this
  project's own agent roster.
- **Geospatial / Urth** — **real overlap**, worth reconciling rather than ignoring.
  `geomaster` (MIT, no scripts, stdlib-friendly guidance) explicitly covers remote
  sensing, satellite imagery (Sentinel/Landsat/MODIS/SAR/hyperspectral), STAC/COG/
  Planetary Computer cloud-native workflows, terrain analysis, and "marine spatial
  analysis" — directly adjacent to Urth's basemap-ingest and open-geospatial-feed work.
  `geopandas` covers the vector-data/GeoDataFrame layer underneath that. Recommendation:
  when Urth work touches satellite/STAC ingestion, have the `world-builder` agent (or
  whoever owns basemap ingest) treat `geomaster`'s reference material as a documentation
  source to consult, not a runtime dependency to wire in blind — it's a curated pointer
  library (500+ code examples across 8 languages) more than a single opinionated SDK, so
  its main value to Urth is as a discovery/reference skill, not a code path.

## 4. Vendoring vs. pulling upstream updates

Three options, in order of how much drift-control effort each buys:

1. **Git submodule pinned to a tag/commit.** Cleanest provenance, easiest to bump
   (`git submodule update --remote`), matches upstream `SECURITY.md`'s own stated model.
   Downside: pulls in the *entire* 166-skill tree plus repo tooling WeftOS doesn't need,
   and no other WeftOS subsystem currently uses a submodule.
2. **Generated selective sync** (analogous to `grok-claude-sync.cjs`'s mechanical-copy
   half): pull upstream into a scratch location, copy only the tiered subset into
   `.claude/skills/science/<name>/`, apply the ingest rules (§6) during the copy, and
   record source commit + local edits per skill in a manifest
   (`docs/research/episteme/sync-manifest.json`). Best fit for WeftOS's actual need — a
   curated subset with deliberate content edits applied on ingest — at the cost of writing
   and maintaining the sync script.
3. **One-time selective copy, no re-sync mechanism.** Fastest to start, but drifts
   silently — not recommended given upstream ships real security fixes (the
   `docx`/`pptx`/`xlsx` `LD_PRELOAD` fix, `imaging-data-commons`'s unattended-install fix)
   on a ~3–4-day cadence (`security-review.md` §7); a one-time copy never receives them.

**Recommendation: option 2** (generated selective sync with a manifest). It's the only
one of the three that both (a) keeps the core/optional/excluded tiering enforceable going
forward rather than a one-time decision, and (b) gives WeftOS a place to apply and
preserve the one deliberate content change (stripping the citation directive) across
re-syncs without hand-editing 136 files every time upstream moves. A submodule is worth
revisiting later if WeftOS decides it wants the *entire* pack rather than a curated
subset.

## 5. Name: Episteme vs. STEM

- **Episteme** — a real Greek epistemology term (knowledge-as-justified-true-belief,
  contrasted with *techne*/craft-skill and *doxa*/opinion), distinctive enough to
  namespace under (`episteme:rdkit`) without colliding with the generic word "science."
  One-line rationale: **it's a name, not a category label, so it won't collide with every
  other "science"-flavored thing in the project or in search results.**
- **STEM** — instantly legible regardless of etymology, matches this pack's own README
  language, but a four-letter acronym already overloaded in education/policy contexts
  with no natural namespace prefix (`stem:rdkit` reads oddly next to `cloudflare:wrangler`).
  One-line rationale: **maximally clear to a human, at the cost of being a generic,
  crowded acronym with a weaker plugin-prefix feel.**

**Recommendation: Episteme.** This project already uses distinctive proper names for
major subsystems (Urth, Forge, WeftOS itself) rather than generic category labels, and a
plugin-style prefix (`episteme:scanpy`, `episteme:rdkit`) reads more consistently with
that pattern than `stem:scanpy` would.

## 6. Episteme agent and package format

Modeled on two read-only-reviewed references (full detail in
`docs/research/agent-skills-design/`): `higgsfield-ai/skills` (8-skill MIT repo,
reverse-engineered format standard) for **routing shape**, and the installed
`media-pipeline` plugin (`guinacio/claude-image-gen`, MIT, `~/.claude/plugins/marketplaces/
media-pipeline-marketplace/`) for **proactive-trigger/CLI-or-MCP packaging shape**. Neither
reference's name, logo, or brand assets are reusable — MIT covers structure, not identity.

### The router agent

An `episteme` skill (not 166 separate top-level entries) that routes a STEM request to
the right Tier-1/Tier-2 sub-skill, shaped like `higgsfield-generate` (322 lines, the one
skill that runs over the repo's own 300-line rule because a router needs more inline
decision logic — everything enumerable still pushed to `references/`):
- **Auto-pick vs named-only routing**, per request-intent class (sequence analysis,
  cheminformatics, stats/ML, geospatial, lab-protocol lookup, ...): a short, priority-
  ordered `condition → skill` list evaluated top-to-bottom ("higher entry wins"), ending
  in an explicit default rather than silence — exactly `model-catalog.md`'s shape, applied
  to `inventory.md`'s domain table instead of a model list.
- **Route by task intent, not surface keyword or output type** — the single highest-value
  rule from Virality Predictor's misrouting warning ("don't file a text-out analysis task
  as chat just because the output is text"). Applies directly: "explain this equation"
  is a derivation task for a symbolic-math skill, not a chat answer.
- **Catalog is a mapping, never the database** — the router's `references/` points at
  `inventory.md`/a generated skill-list command, never a hand-copied snapshot that drifts
  as Episteme's tier composition changes.
- **Delivery shape** for any evaluative sub-skill (unit-consistency check, statistical-
  power calculation, method-validation report): score/verdict → key metric →
  one plain-language interpretation sentence → link to full detail, Virality Predictor's
  four-line report shape, not a raw dump.
- **Proactive trigger, mirrored from `media-pipeline`**: Episteme's own `description`
  should read like image-generation's ("ALWAYS invoke... IMMEDIATELY when you detect
  a STEM computation/lookup/analysis need"), not a passive "use when asked."
- **CLI-or-MCP duality**: offer both a direct-script path (a sub-skill's own
  `scripts/*.py`, dependency-light) and, where a sub-skill already wraps a live service
  (`database-lookup`, `exa-search`), an MCP-tool alternative — mirrors
  media-pipeline's "skill runs the CLI directly, disable the MCP server to cut startup
  overhead" framing.
- **Cost guardrail, mirrored from `image-to-3d`**: any paid/credentialed sub-skill (the 5
  flagged `needs_paid` in `inventory.md`) must dry-run first — print the plan, the cost,
  the account/credit balance — and require an explicit `--yes`/confirmation before
  spending, never submit-on-first-call.
- **The contrast case: no router at all.** `paper-matimageagent.md` (deep read of
  MatImageAgent, Jiang et al. 2026) has an LLM write its own Python per task instead of
  routing to a fixed tool — 54% task-completion over 10 reruns, the failure mode Episteme's
  routing rules above exist to avoid. Its **DebugAgent self-repair loop** (0%→100% on one
  task) is worth adopting inside the router's own error handling as a **bounded** retry —
  re-attempt with the error fed back, capped (e.g. 3 tries), then surface the failure
  rather than loop silently.
- **Measurement provenance rule**: any Episteme sub-skill that derives a physical
  quantity from an image (pixel-to-µm scale, intensity calibration) must read that scale
  from verified metadata or a detected scale bar, never a hand-typed constant in the
  skill or prompt text, and must report the calibration's provenance alongside the
  result — MatImageAgent's hand-typed, unlogged scale constants are the negative example.

### Per-host agent definition

- **Claude Code**: one `episteme` plugin (`.claude-plugin/plugin.json` +
  `marketplace.json` listing every shipped Tier-1/Tier-2 skill folder, CI-checked for
  completeness per Higgsfield's job 3) — matches §1's existing recommendation.
- **Codex**: `agents/openai.yaml` per sub-skill that deserves its own one-click entry
  point (3-field `interface:` block — `display_name`, `short_description`,
  `default_prompt`), plus the package-level `.codex-plugin/plugin.json` `interface` block
  for Episteme itself. This is the concrete answer to "an agent definition per host" —
  Codex is the one host in scope with a real per-skill manifest primitive; Claude Code and
  Grok don't have an equivalent, they read `SKILL.md` directly.
- **Grok**: no translation layer needed for the skill body (frontmatter/body round-trips
  unchanged into `.grok/skills/<name>/SKILL.md`, confirmed by `grok-claude-sync`); no
  manifest layer exists to fill. Treat Grok compatibility as a self-containment smoke test,
  not a fourth packaging track.

### Repackaging the chosen tier

Apply the Higgsfield template skeleton (§13 of `higgsfield-format-standard.md`) to each
Core/Optional skill from §2's tiering: add `version`/`Use when`/`Chain with`/`NOT for`
clauses to the existing `description`; push oversized `SKILL.md` content (flag tables,
catalogs) into `references/`; add the two-tier `## Errors` +
`references/troubleshooting.md` split where missing. Mechanical, applied during the
sync (§4), not a manual rewrite.

### Image-analysis dedupe outcome

`omero-integration`, `pathml`, and `pydicom` are **ADOPT as-is**, sourced from
`K-Dense-AI/scientific-agent-skills` only (confirmed byte-identical to the renamed
`claude-scientific-skills` tag — same repo, no separate upstream to track). A **separate**
copy of `omero-integration`/`pathml` exists in `davila7/claude-code-templates`
(`cli-tool/components/skills/scientific/`) — **SKIP**: that copy ships with zero
`scripts/`, and its `pathml` variant drops K-Dense's entire five-point PHI/consent
safety-boundary section entirely. **Ingest rule: pin the source repo explicitly
(`K-Dense-AI/scientific-agent-skills`) for these three skills in the sync manifest**, so
no future auto-import step can silently substitute the unguarded `claude-code-templates`
copy.

### Ingest rules (binding, apply during the sync in §4)

1. **Strip the self-citation directive** from all 136 affected skills (the "Citing
   Scientific Agent Skills" section, per `security-review.md` §5a) — mechanical
   find-and-remove during sync, not a per-skill judgment call.
2. **Exclude `docx`, `pdf`, `pptx`, `xlsx`** — Anthropic-licensed (`LICENSE.txt`, not
   MIT despite the repo badge), redundant if Claude Code already carries Anthropic's own
   first-party copies (`security-review.md` §6).
3. **Exclude or quarantine `autoskill`** — the one skill upstream's own triage still
   flags with residual risk (user-configured foundry endpoint) even after their fix; keep
   out of Core/Optional until there's a concrete, reviewed WeftOS use for it.
4. **Pin every `pip install`** the sync touches — upstream ships 454 install commands, only
   13 version-pinned (`security-review.md` §4); pin each to the version named in that
   skill's `compatibility` field (or the exact commit for `adaptyv`'s unpinned
   `git+https` install), never carry the unpinned command forward verbatim.
