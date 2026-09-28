# Episteme (working name) — Security review of scientific-agent-skills

Scope and method: this is a **read-only** review of `k-dense-ai/scientific-agent-skills`
(MIT, K-Dense Inc.), shallow-cloned at `~/dev/scientific-agent-skills`. Nothing in the
clone was modified, no skill was executed, no package was installed, and no external
database or API was queried. `scan_skills.py` was **not run** (see below — it makes
outbound LLM API calls, so it does not qualify as the "read files only" bar for running
it in this review); instead this review reads the repo's own pre-generated
`docs/security-report.md` / `docs/security-report.json` (674 findings, run 2026-09-21 by
the upstream maintainers) and `docs/security-triage.md` (the maintainers' own human
review of those findings), and independently verifies a sample of the claims against the
actual skill files.

## 1. The skill format

Each skill is a directory under `skills/<name>/` conforming to the open
[Agent Skills specification](https://agentskills.io/specification) (`AGENTS.md` is the
repo's own contribution guide, written for humans/agents *editing* the repo — it is not
itself loaded by a consuming agent host).

**`SKILL.md` frontmatter** — six allowed top-level keys, closed set, validated by
`strictyaml` via the `skills-ref` CLI:

| Field | Required | Notes |
|---|---|---|
| `name` | yes | must equal the directory name |
| `description` | yes | third person, states what + when to trigger |
| `license` | no | free text or a pointer to a bundled `LICENSE.txt` |
| `compatibility` | no | environment/network/credential requirements, ≤500 chars |
| `allowed-tools` | no | space-separated tool names (not a YAML list) |
| `metadata` | no | string→string map; `metadata.version` is the one field the repo's own CI requires when present; `metadata.openclaw` and `metadata.hermes` are documented exceptions allowed to be nested mappings (host-specific credential-gating blocks) |

Anything else at the top level is a hard validation failure, and because `strictyaml`
rejects the whole document on one bad key, a stray field can silently take `name` and
`description` down with it (AGENTS.md calls this out explicitly as a real failure mode
maintainers have hit).

**`references/`** — long-form documentation loaded by the agent only when needed (keeps
`SKILL.md` itself under the repo's 500-line CI-enforced soft cap). This is where
per-database API docs, worked examples, and citation/license notes for wrapped data
sources live.

**`scripts/`** — executable helper code, almost always Python. 106 of 166 skills ship
`scripts/`. The repo's own CI structural contract (`tests/_meta`, `skill_contract`)
statically parses every script for `eval`/`exec`/`os.system` calls, stdlib-name shadowing,
and hardcoded local paths, and fails a PR that adds `scripts/` without a matching suite
under `tests/<name>/`.

**`assets/`** — static templates/resources (used sparingly; most skills have none).

`plugin.json` makes the whole checkout loadable as one
[Agent Plugins](https://agent-plugins.org/) package — `name`, `version` (kept in lockstep
with `pyproject.toml`), `license`, `keywords`. It carries no executable content itself; it
is a manifest pointing at `skills/`. `CLAUDE.md` at the repo root is one line, deferring to
`AGENTS.md`. `AGENTS.md` is the maintainer-facing spec/contribution guide (skill layout,
frontmatter rules, testing, the CI gates below) — useful to WeftOS as a reference for how
this specific corpus was built, not as something a consuming host loads at runtime.

## 2. What the repo's own tooling checks

**`scan_skills.py`** (615 lines) wraps the third-party
[Cisco AI Defense Skill Scanner](https://github.com/cisco-ai-defense/skill-scanner)
(`cisco-ai-skill-scanner`, pinned in `pyproject.toml`) across three analyzers —
`BehavioralAnalyzer` (static), `TriggerAnalyzer` (static), and `LLMAnalyzer`, which sends
skill content to an LLM (`claude-opus-5` by default) over the network using
`SKILL_SCANNER_LLM_API_KEY`. **This means the script is not a pure static/read-only
tool** — running it would call an external LLM service with the content of every skill,
which is out of scope for a read-only review with no external calls. This review reads
its already-published output instead of re-running it. It also does concurrent scanning,
caches results by content hash, and writes `docs/security-report.md` +
`docs/security-report.json` together (never `SECURITY.md`, which GitHub reserves for the
hand-authored policy). A weekly GitHub Actions workflow (Mondays 09:00 UTC) reruns it.

**`scan_pr_skills.py`** (271 lines) is a thin wrapper around the same scanner, scoped to
the skill directories changed in a pull request, formatted as a sticky PR comment, and
gated on `SKILL_SCANNER_LLM_API_KEY` being present (it fails open with an explanatory
comment on fork PRs, which never receive repo secrets). Also **not static-only**, for the
same reason.

**`tests/_meta`** (via `tests/_contract`, imported as `skill_contract`) is the actual
static, stdlib-only, no-network gate: frontmatter conformance, the 500-line limit, no
`eval`/`exec`/`os.system`, no stdlib-name shadowing, no hardcoded local paths, local link
resolution, shell-script validity, and — the load-bearing rule — every skill that ships
`scripts/` must have a suite under `tests/<name>/` or an entry in
`tests/skill-requirements.toml`. I did not run this either (it is read-only in principle,
but running any of the repo's `uv`/`pytest` tooling would touch the network to resolve
dependencies and was out of scope), but I did read enough of `tests/_contract` and
`tests/skill-requirements.toml` to use the latter as ground truth for per-skill runtime
dependencies in `inventory.md`.

## 3. Findings: code execution, network egress, secrets handling

- **106/166 skills execute code** when used (ship `scripts/`, almost all Python). 21 of
  those ship stdlib-only scripts with no third-party package (documentation/planning CLIs
  — e.g. `genomic-coordinates`, `analytical-method-validation`, `clinical-decision-support`
  are deliberately dependency-free and network-free by design, per their own
  `compatibility` fields).
- **28/166 skills declare network access explicitly** in `compatibility`; a further set
  touch a named external host without using the literal phrase "network access" (e.g. a
  documented REST endpoint). `inventory.md`'s Network/DB column reflects the literal
  declared text, not an exhaustive crawl of every URL a script could reach at runtime.
- **47/166 name a credential** (API key, OAuth, account, access token/password) somewhere
  in `compatibility` or a declared env var — most of these are for the skill's *core*
  function (e.g. `benchling-integration`, `dnanexus-integration`, `exa-search`,
  `labarchive-integration`), a smaller number are optional advanced paths (e.g.
  `pymatgen`'s `MP_API_KEY` for live Materials Project queries vs. its bundled local data;
  `pufferlib`'s optional Neptune/W&B experiment-tracking tokens; `cirq`'s optional IonQ
  hardware backend key).
- **26/166 skills carry a structured `metadata.openclaw` block** (`primaryEnv` +
  `envVars`) — a machine-readable credential-gating declaration consumed by OpenClaw-family
  hosts. This is a genuinely good pattern (declared, typed, gateable) but it is **not**
  read by Claude Code, Grok CLI, or Codex, so it does nothing in a WeftOS delivery unless
  something is built to consume it (see `adoption-notes.md`).
- **5/166 skills explicitly name paid/gated access**: `bgpt-paper-search` (BGPT paid
  tier), `deepspot-m` (gated Hugging Face weights, CC-BY-NC-SA-4.0), `generate-image`
  (OpenRouter, bills per request), `transformers` (Hugging Face gated-model downloads),
  `waypoint-bio` (Outpost.bio commercial API).
- **Secrets handling is consistently disciplined in the prose**: every credentialed skill
  I sampled instructs reading from an environment variable, never hardcoding a key, and
  several (`neuropixels-analysis/references/AI_CURATION.md`,
  `datalad/SKILL.md`) contain an explicit "never hardcode API keys" callout. This is a
  documentation convention, not a runtime guarantee — nothing prevents an agent from
  ignoring the instruction, and the scanner's `BEHAVIOR_ENV_VAR_HARVESTING` false-positive
  pattern (below) exists precisely because "read your own env var and call your own
  service" looks identical to credential exfiltration to a naive static rule.

## 4. Supply chain: unpinned installs

- `pip install`/`uv pip install` appears **454 times** across `SKILL.md` and
  `references/*.md` files; only **13** of those pin an exact version with `==`. The
  repo's own `tests/skill-requirements.toml` (which builds one throwaway `uv` environment
  per skill for CI) also mostly lists bare package names, not pins — it exists to keep
  mutually-incompatible upstream pins from colliding across skills in one interpreter, not
  to lock a supply chain.
- Two skills install directly from a Git remote rather than PyPI: `esm` pins to a **full
  40-character commit SHA** (`esm@git+https://github.com/Biohub/esm.git@<sha>` — good
  practice), and `adaptyv` installs
  `git+https://github.com/adaptyvbio/adaptyv-sdk.git` with **no ref pin at all** — an
  agent following that instruction gets whatever is at the tip of that branch at the time
  it runs, from a smaller, less-audited GitHub org rather than a PyPI release.
- Net effect: an agent that follows a skill's own installation instructions verbatim will
  usually get *whatever is current on PyPI*, not the exact version the skill's
  `compatibility` field says it was verified against (e.g. "Examples target RDKit
  2026.03.3"). `compatibility` text is a documentation claim, not an enforced pin. This is
  a normal trade-off for a documentation-style skill library (pinning every install
  command would fight the point of "always install the current release"), but it means
  version drift between "what was tested" and "what an agent installs in the field" is
  architecturally possible for essentially every non-stdlib skill, not a defect specific
  to one skill.
- The historical fix log in `docs/security-triage.md` (below) shows the maintainers have
  already caught and fixed one real unpinned/uncontrolled-install issue
  (`imaging-data-commons` shelling out to `pip3 install --break-system-packages`
  unattended) and one real predictable-temp-file `LD_PRELOAD` issue
  (`docx`/`pptx`/`xlsx` sharing an office-automation shim), which is a reasonable signal
  that supply-chain and local-file issues get taken seriously when found, not that none
  remain.

## 5. Prompt-injection risk in `SKILL.md` and reference files

Two distinct patterns are worth separating:

**(a) The self-citation directive — widespread, low-severity, and already flagged
upstream.** **136 of 166 skills** end `SKILL.md` with a "Citing Scientific Agent Skills"
section instructing the agent to add the paper (arXiv:2609.00065, authored by K-Dense, the
skill publisher) to a user's manuscript/report/presentation/code release "if it materially
contributed," tell the user it did so, and optionally fetch `arxiv.org` to get the
citation text. This is an **operational directive that mutates the user's own deliverable
for the publisher's benefit**, embedded in every skill regardless of relevance, which is
the textbook shape of a low-grade prompt-injection / self-promotional-content pattern —
though it is disclosed in plain prose in every instance, not hidden or obfuscated, and it
is conditioned on the skill having "materially contributed." The scanner independently
flagged this as `LLM_SOCIAL_ENGINEERING` (verified directly against the `adaptyv` skill,
one of its 155 "safe" skills, which still carries this exact finding at LOW severity — see
`docs/security-report.json`). **Recommendation for WeftOS: strip or neutralize this
section on ingest** (make it passive/informational, or delete it) rather than accept it as
shipped — see `adoption-notes.md`.

**(b) Everything else the scanner and triage doc found.** Per the upstream
`docs/security-report.md` (generated 2026-09-21, scanner `cisco-ai-skill-scanner` 2.1.0,
model `claude-opus-5`, 166 skills, 674 findings): **28 CRITICAL**, 4 HIGH, 188 MEDIUM, 453
LOW, 1 INFO; **155/166 skills "safe"** (scanner's per-skill classification, not an
independent certification). The 28 CRITICAL findings are `BEHAVIOR_*EXFILTRATION*`
patterns across eight skills — `autoskill`, `citation-management`, `infographics`,
`latex-posters`, `literature-review`, `research-lookup`, `scientific-schematics`,
`scientific-slides` — which `docs/security-triage.md` (the maintainers' own review)
characterizes as "the service-authentication pattern" (a skill reads its own declared
credential and calls its own declared service), verified per-skill against named
credential reads and destinations, with one specific call-out that **`autoskill`'s
configurable foundry endpoint still requires trust and authorization; HTTPS validation
alone does not establish either** — i.e. the maintainers' own triage does not wave this
away, it names the one skill (`autoskill`) where a user-configured destination is a real
residual risk. The 3 HIGH `MDBLOCK_PYTHON_EVAL_EXEC` findings (`histolab`, `modal`,
`waypoint-bio`) are verified false positives (`cv2.CV_64F` constant, `.eval()` model-mode
calls, not `eval()`/`exec()`).

The 1 HIGH `CROSS_SKILL_DATA_RELAY` finding deserves its own note: reading the raw JSON
(`docs/security-report.json` → `cross_skill_findings`), this single finding's "collector"
list names **well over a hundred** of the repo's 166 skills as a single undifferentiated
group, which is a strong signal it is an **overlap heuristic** (any skill that reads a
file plus any skill that can reach the network, unioned) rather than a demonstrated
attack chain — no two named skills actually invoke each other or share a file handle in
this repository. The maintainers' triage agrees: "an overlap heuristic... not evidence of
a confirmed attack." Treat cross-skill findings as a composition-review prompt, not a
verified vulnerability, and re-derive them yourself before trusting the count if
WeftOS ever runs the scanner against a curated subset.

I independently spot-checked the `adaptyv` "safe" (0 deterministic findings, 1 LLM LOW
finding) skill's raw JSON entry directly (shown in §5a) and it matches the published
summary — the report is internally consistent on the one skill I checked in full.

**What I did not verify**: I did not re-run the scanner (see §2), did not read all 674
findings individually (I read the CRITICAL/HIGH rows plus the triage doc's own analysis
of them, and spot-checked one LOW finding end-to-end), and did not audit every skill's
`scripts/` by hand for logic bugs — this review is a security *posture* read of a
166-skill corpus via its own published scan plus targeted verification, not a
line-by-line audit of 106 scripts.

## 6. License and attribution of bundled data / third-party content

- **Repo license**: MIT (K-Dense Inc., 2025) for the code and prose in this repository.
- **Not everything under `skills/` is actually MIT**, despite the repo-wide license badge:
  - `docx`, `pdf`, `pptx`, `xlsx` carry a bundled `LICENSE.txt`: **"© 2025 Anthropic, PBC.
    All rights reserved... governed by your agreement with Anthropic"** (Consumer or
    Commercial Terms of Service). These are Anthropic's own official document-processing
    Skills, vendored into this repo. `license:` in their frontmatter reads "Proprietary.
    LICENSE.txt has complete terms." **This matters directly for a WeftOS delivery to
    Claude Code specifically**: Claude Code likely already ships or can already reach
    these same official Anthropic skills through its own channel; vendoring K-Dense's copy
    into an "Episteme" pack would duplicate Anthropic-licensed content under a third
    party's redistribution rather than use Anthropic's own distribution.
  - `rowan` — `license: Proprietary (API key required)` — a thin wrapper around the Rowan
    quantum-chemistry SaaS; using it means agreeing to Rowan's own terms, not this repo's
    MIT license.
- **License field is honest about gaps**: `primekg`'s own frontmatter says `license:
  Unknown` — the maintainers did not paper over an unresolved upstream license, they
  surfaced it. Treat `primekg` as needing a manual license check before any
  redistribution-adjacent use.
- **Bundled-data licenses that differ from the wrapping skill's own MIT license**:
  - `depmap` — skill is MIT, but its frontmatter separately states the DepMap data itself
    is `CC-BY-4.0`.
  - `imaging-data-commons` — skill is MIT; frontmatter explicitly warns "IDC data itself
    has individual licensing (mostly CC-BY, some CC-NC) that must be respected when using
    the data" — i.e. per-collection, not repo-wide.
  - `alphagenome` — skill is MIT; the AlphaGenome API key is explicitly
    "free non-commercial" (Google DeepMind's own terms), so any commercial use of
    AlphaGenome outputs needs a separate agreement outside what this skill grants.
  - `deepspot-m` — skill is MIT; the actual model weights on Hugging Face
    (`ratschlab/DeepSpotM`) are gated and licensed **CC-BY-NC-SA-4.0**.
  - `waypoint-bio` — skill is MIT; references a third party's own published citations
    page (outpost.bio/citations) for the data it wraps.
- **`database-lookup` is the widest surface for this** (78 reference files, one per
  external database) and is also the most careful about it: it proactively steers away
  from a paid/restrictive source in its own guidance — e.g. its DrugBank entry reads
  *"Paid API license required → Use ChEMBL + PubChem + OpenFDA instead"* — rather than
  silently wrapping it. It does not, however, carry a consolidated ToS/attribution table
  across all 78 databases; each reference file documents its own source's terms
  individually, so a full attribution audit of `database-lookup` means reading up to 78
  files, not one.
- **Recommendation**: before any redistribution of an "Episteme" pack beyond WeftOS's own
  internal use, carry `LICENSE.md` (MIT) plus a manifest of the exceptions above
  (`docx`/`pdf`/`pptx`/`xlsx`/`rowan` proprietary, `primekg` unknown, and the
  data-license notes on `depmap`/`imaging-data-commons`/`alphagenome`/`deepspot-m`), rather
  than a blanket "MIT" claim over the whole pack.

## 7. Update cadence

Via `gh api repos/k-dense-ai/scientific-agent-skills` (2026-09-28): created 2025-10-19,
**46,912 stargazers**, **4,233 forks**, **30 contributors**, **30 releases** (current
`plugin.json`/`pyproject.toml` version is 2.69.0 — roughly one release every 3–4 days
since creation), **15 open PRs**, last push 2026-09-21T09:27:50Z (one week before this
review). A 200-commit sample spans 2026-06-12 to 2026-09-21 (~101 days) at roughly
**2 commits/day** during active periods, arriving in bursts (many same-day commits,
consistent with batched/reviewed PR merges) rather than a steady trickle. The weekly
security-scan workflow (Mondays 09:00 UTC) and the PR-scan/spec-validation/skill-tests
workflows run on every PR touching `skills/`, so a new or changed skill gets both a
structural check and a scanner pass before merge — this is an actively, frequently
maintained repository, not a stale drop.
