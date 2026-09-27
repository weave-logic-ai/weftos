# Communication-style skill set: build spec

Builds on 01 (academic), 02 (prior art), 03 (tells by surface). The builder implements this; the
reviewer tests against section 11. Two goals share one toolchain but stay separate: **neutral**
(agent prose on every surface stops reading as generated; no consent question) and **as a person**
(writing in someone's voice from a profile; consent required whenever the subject isn't the
operator).

Revision 2 (2026-09-27): a voice is **conditioned on context**. Every corpus item is tagged with
`medium` and `audience`; the profile is a base layer plus register slices keyed by (medium,
audience); weight falls off with distance between contexts; agent-directed text is its own opt-in
slice and never shapes human-audience writing. Section 4 is the core of this revision.

Revision 3 (2026-09-27): adds spoken meetings as a medium (4.6), a feature-validity matrix so ASR
artifacts never shape written voice (4.6), a confidentiality regime for third-party speech (7), and
an optional house-style layer between voice-base and the person (4.7). Data formats, CLIs, and
acceptance tests moved to `05-interfaces-and-tests.md` to keep this file under 500 lines.

## 1. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | Four skills + one agent: `voice-base`, `voice-profile`, `voice-ingest`, `voice-check`, agent `voice-writer`, all user-level under `~/.claude/`. | Neutral writers need scoring without touching ingest, so check is its own skill. |
| D2 | One stdlib Python library, `voicelib`, lives in `voice-check/scripts/`; `voice-ingest` imports it by path. | Profile stats and draft scores must come from the same code to be comparable. |
| D3 | Tells live in `voice-base/references/tells.json` with stable IDs (`T1-*`, `T2-*`, `T3-*`); `tells.md` is the human version. | Profiles override by ID; the scorer and the writer read one source. |
| D4 | Profile frontmatter is TOML between `+++` fences, read with stdlib `tomllib` (Python >= 3.11). | Slices and overrides are nested data; stdlib has no YAML parser; TOML stays hand-editable. |
| D5 | Scripts never call an LLM or the network. The "LLM pass" is Claude reading a generated `brief.md` and writing `profile.md`. | Deterministic, CPU-only, zero-download; the corpus goes nowhere new. |
| D6 | Least retention: `raw.jsonl` deleted after `clean`; `clean.jsonl`, `brief.md` deleted at `finalize` unless `--keep-corpus`. | 01 section 6: the profile is already a fingerprint. |
| D7 | Tier 1 can't be overridden. Tiers 2/3 can, per rule ID, per slice or globally, with evidence. | "Great question!" carries no information; em-dashes and tricolons are personal. |
| D8 | Kobak focal words are tier 2 (high weight, density escalation); `lexicon.keep` whitelists individual words. | Corpus-level evidence; an individual can genuinely say "crucial". |
| D9 | Fidelity = tell scan + burstiness vs the slice's range + self-calibrated Burrows' Delta percentile + Delta to a bundled default-LLM baseline + optional embedding. LLM judgment is only the final checklist. | 01 findings 9-10. |
| D10 | Voice = base layer + (medium, audience) slices, with distance-weighted pooling and per-field confidence. `relationship` is stored per item but is not a slice key. | Register shifts with medium and recipient; relationship as a third key would split thin corpora into nothing. It's used to rank exemplars instead. |
| D11 | Agent-directed text (`medium = agent-prompt`, `audience = agent`) is opt-in and weighs 0 on the base layer and every human slice. | What people type to agents is imperative shorthand, not their voice to humans. |
| D12 | Fields below their minimum sample size are marked `insufficient` and left empty, never estimated. | Guessed traits are worse than a declared fallback. |
| D13 | `meeting` is a medium. Transcript punctuation and sentence splits are machine-made, so meeting items feed only valid feature groups (lexicon, discourse markers, stance, explanation patterns) outside spoken slices. | Meetings are the richest client-facing evidence, but ASR rhythm and punctuation would corrupt written stats. |
| D14 | Optional house-style layer: voice-base -> house style -> person base -> person slice -> task instructions, with hard rules above all of them. | Client and org guidance exists as prose prompts; it has to compose with a person's voice, not replace it. |
| D15 | English only in v1; non-English segments are detected and skipped. | Function-word lists are language-specific. |
| D16 | Supersede, don't delete, `de-slopify` / `docs-de-slopify` (section 9). | Their blanket em-dash ban contradicts the evidence. |

## 2. File tree

```
~/.claude/skills/
  voice-base/
    SKILL.md                    # <= 200 lines: tiers, revision procedure, medium router
    NOTICE.md                   # blader, rlorenzo, jooray (MIT); Wikipedia (CC BY-SA, paraphrased+linked); Kobak; Juzek & Ward
    references/
      tells.json  tells.md      # catalog (05 section 1); human version, one before/after per tell
      word-lists.md             # Kobak 21 + secondary cluster; hedges; discourse markers
      rhythm.md  register.md    # burstiness; one fact calibrated across media/audiences (no real names)
      checklist.md  always-on-rule.md   # final self-review (<= 20 items); <= 25-line paste-able rule
      surfaces/{harness-output,markdown-docs,commits-prs,long-form,slides,email-chat,social,agent-prompt}.md
                                # each: tells, structural tells, replacement principle, before/after, "not a tell here"
  voice-profile/
    SKILL.md                    # create, fill (manual or assisted), validate, update, retire
    template/ profile.md  consent.json  roster.toml  house-style.toml  exemplars/README.md
    examples/fictional-terse-dev/   # fully filled FICTIONAL profile with 3 slices + 1 thin slice
    scripts/voice_profile.py    # init | validate | show | slices | overrides
  voice-ingest/
    SKILL.md                    # pipeline incl. consent, tagging review, LLM pass
    references/ adapters.md  tagging.md  llm-pass.md  privacy.md
    scripts/
      voice_ingest.py  select_exemplars.py   # CLI (05 section 2)
      adapters/ mbox.py slack.py linkedin.py x_archive.py git_log.py text_dir.py
                agent_transcripts.py meeting_transcript.py   # meeting: VTT, m:ss-speaker txt, Zoom chat, docx
      tagging.py                # medium/audience/relationship inference + roster + CSV round-trip
      clean.py                  # quotes, signatures, code, PII, dedupe, language, suspect-agent filter
  voice-check/
    SKILL.md                    # when/how to score; reading the report; Goodhart warning
    references/
      baseline-llm.json  baseline-llm-corpus.jsonl   # ~40 builder-written unstyled assistant paragraphs + stats
      function-words-en.txt  disfluencies-en.txt  metrics.md               # ~150-300 function words; metric definitions and calibration
      context-distance.toml  thresholds.toml          # distance tables (4.3); minimum samples per field (4.4)
    scripts/
      voice_check.py            # CLI (05 section 2)
      voicelib/ __init__.py textproc.py features.py tells.py delta.py
                slices.py       # weights, pooling, confidence, slice selection + fallback
                profile_io.py   # paths, perms, TOML, consent, git-worktree guard
                embed.py        # optional, lazy import
    tests/ fixtures/  test_*.py # fictional corpora only; stdlib unittest
~/.claude/agents/voice-writer.md
```

Per-person data, outside the skills:

```
~/.claude/voice-profiles/            # 0700
  config.toml                        # default = "<slug>" (optional)
  <slug>/                            # 0700; files 0600
    profile.md  consent.json  roster.toml  stats.json  calibration.json  redact-terms.txt
    house/<name>.toml                # house-style bindings; the prose file stays where the org keeps it
    exemplars/<medium>--<audience>.md
    corpus/raw.jsonl  corpus/clean.jsonl  tags.csv  brief.md   # transient (D6)
    ingest-log.jsonl                 # counts only, never text
```

Every file stays under 500 lines. No skill file ships a real person's name, email, or text.

## 3. Skill specs

Frontmatter: `name`, `description` (<= 1024 chars, what it does then "Use when ..."),
`user-invocable: true` for `voice-ingest` and `voice-check`, `argument-hint` where there are args.
SKILL.md routes; references hold detail.

**voice-base** (load before writing any words a person reads). Contents: the three tiers in five
lines, with "a tell is something that appears regardless of what the content needs" (03); a router
from medium to `surfaces/*.md`; inline harness-output rules (outcome and number first, verification
in the same breath, no preamble, no closing offers on finished work, no emoji as pass/fail, headers
only for real sections); and the **revision procedure**, which `voice-writer` also follows:
1. Name the medium, the recipient (audience + relationship), what they already know, and what
   they're owed (proof or headline).
2. Content inventory: list the facts and claims to convey. Nothing gets added later that isn't on
   it.
3. Draft to the surface guide (and the selected slice, when a profile is loaded).
4. Tier-1 pass removes every T1 hit; tier-2 pass fixes unless the profile overrides.
5. Rhythm pass: if sentence-length CV is below target, merge or split for meaning, never pad.
6. Claims pass: every "fixed/verified/works" says how it was checked; say what wasn't done.
7. Over 80 words: run `voice-check` if available, else `checklist.md`.

`tells.json` seed: 03's per-surface tells, local `de-slopify` PATTERNS.md, blader's 25, rlorenzo's
domain carve-outs, Wikipedia's categories paraphrased with a link. 40-60 tells, each naming its
source. `agent-prompt.md` covers writing prompts and instructions for agents: be specific, state
constraints and done-conditions, no politeness padding.

**voice-profile.** `voice_profile.py init <slug>` (copies template, sets perms, refuses inside a git
work tree); manual filling (base layer first, then one slice per real context; mark any field you
can't back with samples `insufficient`); Claude-assisted filling (hand off to `voice-ingest`);
validate; bind a house style (copy `template/house-style.toml` to `house/<name>.toml`, point
`path` at the org's file, set scope and overlays); update (Changelog + `updated`); retire
(`voice-ingest purge`). States "a sample teaches
style, not facts" in our own words, credited to jooray/humanizer.

**voice-ingest.** Ordered pipeline, one CLI call per step: init -> consent -> collect (per source)
-> clean -> **tag** (infer, then the user reviews `tags.csv`) -> features -> exemplars -> brief ->
**LLM pass** -> validate -> user review -> finalize. Between steps, show the user counts,
redactions, suspect-agent exclusions, date ranges, and the slice table (n and confidence per slice)
before the LLM pass. `llm-pass.md` rules: describe habits with rates from `stats.json`; every trait
cites >= 2 segment IDs from >= 2 threads in the same slice (else `insufficient`); topic words aren't
voice; base-layer traits must hold in >= 2 human slices; do/don't pairs are rewrites of real
segments with facts replaced by placeholders; typos are not habits; section "Never" comes only from
the subject's own answers.

**voice-check.** When to run (any draft > 80 words; always under a person's name), how to read the
report, and the Goodhart warning: never insert function words or punctuation to move a metric;
revise for meaning and re-check at most twice.

## 4. The context model

### 4.1 Tags on every corpus item
- `medium`: `email, chat, commit, pr, doc, slides, social, meeting, agent-prompt`. (`harness-output` is a
  writing surface for the agent's own words, not a corpus medium.)
- `audience`: `peer, report, manager, external, public, self, agent`.
- `relationship`: `close, working, distant, unknown`.
- `tag_source`: `inferred | roster | user`, plus `tag_confidence` 0-1.

Inference (in `tagging.py`; the user's `roster.toml` maps identifiers such as emails, domains, and
Slack IDs to audience and relationship, and always wins):

| source | medium | audience rule |
|---|---|---|
| mbox | email | roster match; else recipient domain == sender's -> `peer` (0.5), other domain -> `external` (0.7); >= 15 recipients or list headers -> `public` (0.5) |
| slack | chat | DM -> roster or `peer` (0.5); channel -> `peer` (0.6); shared/Connect channel -> `external` (0.8) |
| linkedin | social / chat | shares and comments -> `public` (0.9); messages -> `external` (0.6) |
| x | social | `public` (0.9) |
| git | commit | `peer` (0.7); `--public-repo` -> `public` |
| text | from `--medium` | from `--audience` (required) |
| transcript (meeting) | meeting | from participants: any roster-external org present -> `external` (0.8); all internal -> `peer`, or `manager`/`report` if the roster says so for a 1:1 (0.7); unknown participants -> `unknown` (0.3) until the user tags it |
| transcript (Zoom chat file) | chat | same participant rule as its meeting |
| agent-transcripts | agent-prompt | `agent` (1.0); opt-in only |

Relationship defaults to `unknown` unless the roster says otherwise. `voice_ingest.py tag --export`
writes `tags.csv` (`id, medium, audience, relationship, exclude, first_60_chars`); the user edits
it; `tag --import` applies it with `tag_source = user`. Summary before import: counts per (medium,
audience) and how many rows are below 0.6 confidence.

### 4.2 Base layer and slices
- **Base layer**: traits that hold across human contexts (function-word habits, punctuation, core
  lexicon, stance). Computed from all items with `audience != agent`, each weighted 1 (agent items
  weight `agent_base_weight`, default 0).
- **Slice** `(medium, audience)`: own stats from its items only (`n_own`), plus pooled stats for
  drafting (`n_eff`), where every other item counts with weight `w = max(0, 1 - d/0.6)` and `d =
  0.5*d_medium + 0.5*d_audience`. Hard rule: if exactly one side is `agent`, `w = 0`.
- A slice exists in the profile once `n_own >= 10` segments; below that its items only feed
  neighbors and the base layer.

### 4.3 Distance tables (`context-distance.toml`, defaults, tunable)
Medium: same 0; email-chat 0.3; commit-pr 0.2; pr-doc 0.3; doc-slides 0.4; social-doc 0.5;
social-chat 0.5; meeting-chat 0.5; meeting-email 0.6; anything-agent-prompt 1.0; other pairs 0.7. Audience: same 0; peer-report 0.2;
peer-manager 0.3; report-manager 0.4; peer-external 0.5; manager-external 0.4; external-public 0.3;
self-peer 0.4; self-other humans 0.6; agent-any human 1.0.

### 4.4 Minimum samples and confidence (`thresholds.toml`)
Per field, per slice (on `n_own` for "own" confidence, `n_eff` for pooled):

| field | none below | low | medium | high |
|---|---|---|---|---|
| rhythm (sentence-length distribution, CV) | 60 prose sentences | 60 | 150 from >= 20 segments | 500 |
| function-word profile / Delta | 2,000 words | 2,000 | 5,000 | 15,000 |
| punctuation rates | 1,000 words | 1,000 | 2,000 | 8,000 |
| openers / closers | 10 segments | 10 | 20 | 50 |
| lexicon phrase (keep) | seen in < 3 threads | n/a | >= 3 threads | >= 6 threads |
| calibration held-out | 8 segments | 8 | 20 | 50 |
| exemplars | 10 candidates | pick 2 | pick 3-5 | pick 5 |

`none` means the field is written as `insufficient` and nothing is guessed.

### 4.5 Slice selection (drafting and checking)
Given a target `(medium, audience)`:
1. Exact slice with `own` confidence >= medium for rhythm and function words: use it.
2. Else rank existing slices by `d` (excluding any slice across the agent boundary); take the
   nearest with `d < 0.6` and blend its traits with the base layer, using the target's pooled
   (`n_eff`) stats.
3. Else base layer only.
4. Separately, for the spoken-valid groups (lexicon, discourse, stance, explanation), a `meeting`
   slice with the same audience may be added when it is stronger than the written choice.
The chosen path is always printed, e.g. `slice email/external: own n=4 -> written: email/peer
(d=0.25) + base; spoken: meeting/external n=612 for lexicon,discourse,stance; confidence
rhythm=medium fw=low`.

### 4.6 Spoken media and feature validity
Feature groups: **G1** function words, **G2** punctuation, **G3** rhythm (sentence length, CV,
paragraphs, fragments), **G4** lexicon and phrases, **G5** discourse markers, **G6** stance and
explanation patterns (LLM-described: how they explain, frame, reassure, push back, use analogies),
**G7** openers/closers, **G8** markdown/format, **G9** disfluency (spoken-only).

| item medium | feeds its own slice | feeds base layer | feeds written slices |
|---|---|---|---|
| written (email, chat, commit, pr, doc, slides, social) | G1-G8 | G1-G8 | G1-G8, by weight |
| meeting | G1 (after disfluency strip), G4-G7, G9 | G4, G5, G6 | G4, G5, G6, by weight |
| agent-prompt | all | none | none |

G2 and G3 from meetings are **invalid** (ASR punctuation and segmentation) and are never computed
for meeting slices; the profile shows them as `invalid (asr)`, not `insufficient`. Before G1, strip
`disfluencies-en.txt` fillers (um, uh, er, ah, hmm, mm-hmm, uh-huh), collapse ASR stutters ("the
the"), and remove discourse fillers (you know, I mean, like, kind of, sort of) into G9; written items
keep those words. The validity matrix lives in `context-distance.toml` under `[validity]` and is
applied as `w_group = w_context * valid(group, item_medium, target_medium)`. Meeting thresholds use
words for G4-G6 (lexicon phrase >= 3 meetings; stance trait >= 2 meetings) and turns for G7.

### 4.7 House-style layer
A house style is an org's or client's writing guidance (often a prompt with a base and audience
overlays). It sits between voice-base and the person. The profile binds it by path in
`house/<name>.toml` (template: `template/house-style.toml`):
`name`, `path` (the prose file, left in place), optional `begin_marker`/`end_marker` for the base
block, `scope` (media/audience keys where it applies), `[[overlays]]` (`match` = audience or
format, `begin_marker`/`end_marker`), and machine-readable `[[rules]]`: tighten or relax a tell by
ID, or add house tells `H-<name>-*` (same schema as tells.json).

Precedence, highest first:
1. Hard rules: tier-1 tells, truthfulness and no fabrication, confidentiality and disclosure rules,
   consent scope, the profile's "Never", required output schemas.
2. House-style **policy**: what may be disclosed, claim discipline, forbidden patterns, and any rule
   marked `kind = "policy"`. Neither a profile nor a task instruction can relax these.
3. Explicit instructions for this piece (a task overlay): tone, format, depth, within their scope.
4. Person slice, then person base layer: voice mechanics (G1-G7).
5. House-style **defaults**: tone, format defaults, rule `kind = "default"`.
6. voice-base tier 2/3 defaults.
Conflicts: a house `forbid` beats a person `allow` unless the house rule is `kind = "default"`; a
person habit beats a house default only with profile evidence (medium+ confidence); tools a house
style names (for example a humanize pass) are used when present and don't replace voice-check. The
writer and checker print `house: <name> (<n> rules, overlay <match>)`. Nothing from a house-style
file is copied into the skills; the template only holds the path. A house style also works without
a person profile (neutral writing for that org): `--house` on the checker and the writer.

## 5. Profile template (`template/profile.md`)

```
+++
schema = "voice-profile/v2"
slug = "{{slug}}"
display_name = "{{name}}"
subject_relation = "self"          # self | other
consent_ref = "consent.json"
languages = ["en-US"]
spelling = "US"
created = {{YYYY-MM-DD}}
updated = {{YYYY-MM-DD}}
disclosure = "ask"                 # none | footer | ask
agent_base_weight = 0.0
[[sources]]                        # filled by voice-ingest; manual profiles may omit
kind = "mbox"                      # mbox|slack|linkedin|x|git|text|transcript|agent-transcripts|manual
segments = 0
words = 0
date_range = ["", ""]
note = ""                          # agent-transcripts: "agent-directed register, not voice evidence"
[[house_style]]                    # optional; repeatable
ref = "house/{{house_name}}.toml"
scope = ["email/external", "meeting/external"]
[base]
confidence = { rhythm = "insufficient", function_words = "insufficient", punctuation = "insufficient" }
[lexicon]
keep = []
avoid = []
[[slice]]
medium = "email"
audience = "peer"
n_segments = 0
n_words = 0
n_sentences = 0
n_eff = 0.0
status = "own"                     # own | pooled | insufficient
spoken = false                     # true for meeting slices: G2/G3 are "invalid (asr)"
confidence = { rhythm = "insufficient", function_words = "insufficient", punctuation = "insufficient", openers = "insufficient" }
exemplars = "exemplars/email--peer.md"
[[overrides]]                      # tier 2/3 only
rule = "T3-emdash"
action = "allow"                   # allow | require | forbid
scope = ["base"]                   # "base" or "medium/audience" keys
evidence = "base punct.emdash_per_1k = 6.2 over 41k words"
+++

# Voice profile: {{name}}
## 1. Identity and audiences
Role and domains (context only, not voice). Who they write to, by audience class; the roster lives in roster.toml.
## 2. Base layer
### 2.1 Voice in brief — 5-7 traits as "X, not Y", each true in >= 2 human slices.
### 2.2 Function words and discourse markers — over/under-used vs general English with ratios; habitual connectors; hedging rate.
### 2.3 Punctuation and typography — per 1k words; quote style; capitalization; emoji; numbers; Oxford comma.
### 2.4 Core lexicon — habitual phrases (>= 3 threads), words avoided, jargon used correctly; typos are not habits.
### 2.5 Stance — directness, certainty, humor, how they disagree, apologize, say no, give credit.
## 3. Slices
One subsection per [[slice]], header "### <medium> → <audience> (status, n, confidence)":
- register: formality 1-5, typical length, opener, closer, markdown, emoji
- rhythm: sentence median/p10/p90, CV, fragment and question rates (or `insufficient`)
- how this slice differs from the base layer (only differences)
- do/don't: >= 2 pairs rewritten from this slice's segments, facts as placeholders
- meeting slices only: explanation patterns (explain, frame, reassure, push back, analogies), each
  citing >= 2 meetings; G2/G3 shown as `invalid (asr)`
- exemplar ids, and relationship mix of the exemplars
## 4. Overrides of voice-base — mirror of [[overrides]] with reasons.
## 5. Never — commitments not to make (money, legal, hiring, dates), topics, claims, people not to speak for. Subject-supplied only.
## 6. Changelog
```

`voice_profile.py validate` fails on: bad TOML; leftover `{{...}}`; tier-1 overrides; unknown rule
IDs or scope keys; missing/invalid consent; a `[[slice]]` without a matching body subsection; any
numeric rhythm or punctuation claim in a slice or base section whose confidence is `insufficient`;
empty 2.1 or 5; an `agent` slice feeding base (weight > 0.1); G2/G3 numbers in a spoken slice; a
`[[house_style]]` ref that doesn't resolve or a house rule relaxing tier 1. Warns on: every slice below medium,
profile older than 365 days.

## 6. Data formats and script interfaces

Specified in `05-interfaces-and-tests.md`: tells.json, corpus row, stats.json, calibration.json
(section 1 there), the three CLIs with adapter rules and report format (section 2), and the
acceptance tests (section 3).

## 7. Consent, confidentiality, and runtime

`consent.json`: `{"schema":"voice-consent/v1","subject","operator",
"relation":"self|other","granted","expires","form":"self|written|recorded-verbal",
"evidence","scope":{"media":[...],"audiences":[...],"sources":["mbox","meeting",...],"uses":["draft-for-subject-review","send-as-subject"]}}`.
Enforced in `profile_io.py`: `collect` exits 3 without valid consent; `other` needs `form !=
"self"`, evidence, and unexpired `expires`; the writer refuses a medium, audience, or use outside
`scope` (default use `draft-for-subject-review`); profile roots inside a git work tree or a sync
folder (Dropbox, iCloud, Google Drive, OneDrive) are refused; dirs 0700, files 0600. `clean` redacts
emails, phones, query-string URLs, IPs, card/SSN-like numbers, key-like tokens (`sk-`, `ghp_`,
`AKIA`, >= 20-char mixed), addresses, and names harvested from export metadata and the roster
(`[PERSON]`). Meeting and external-audience sources get more: `redact-terms.txt` (client, company,
project, product names -> `[ORG]`/`[TERM]`), currency, percentages, large numbers and dates ->
`[AMOUNT]`/`[NUMBER]`/`[DATE]`. The meeting adapter persists **only the subject's turns**; other
speakers' text is read in memory to find participants and is never written. Participants are stored
as counts by org class (`{"internal":3,"external":2}`), not names. Collecting `transcript` requires
`consent.scope.sources` to include `meeting` and `--attest-recording-rights` (the operator confirms
they may process these recordings). `roster.toml` and `redact-terms.txt` are personal and client
data and get the same perms. The log records counts, never text. Client content never enters the
skills: fixtures, examples, and tests use invented companies and people only (test A16 greps the
skills against `redact-terms.txt`).

Python >= 3.11 stdlib on the default path; optional `textdescriptives`, `faststylometry`,
`sentence-transformers`, lazily imported with "skipped: not installed". Exit codes: 0 ok, 1 check
failed, 2 usage, 3 consent/privacy refusal, 4 input not found.

## 8. Agent: `voice-writer`

`~/.claude/agents/voice-writer.md`; `tools: Read, Write, Edit, Bash, Glob, Grep`; `model: sonnet`
(escalate only for long-form). Description: drafts or rewrites any text so it doesn't read as
generated, optionally in a person's voice from `~/.claude/voice-profiles`. Inputs: the content or
request, `medium`, `to` (audience) and relationship if known, optional profile slug.
1. Resolve profile: explicit slug; `config.toml` default only if the caller said "as me"; else
   neutral. `validate`; on exit 3 stop and say why.
2. Resolve medium and recipient; ask once if the audience is unclear rather than guessing.
3. Load `voice-base`, then any house style bound for this scope (base block plus the overlay
   matching the recipient; `--house` overrides), then the surface guide. With a profile, run
   `voice_profile.py show <slug> --medium M --to A` and load 2-4 exemplars from the selected slice, preferring the same
   relationship. Never load agent-slice exemplars for a human recipient, or human-slice exemplars
   for an agent recipient.
   When a spoken slice supplies lexicon, discourse, or stance for written work, use it for those
   groups only (never its rhythm or punctuation) and say so in the note.
4. Content inventory. Exemplars supply zero facts, names, numbers, or claims; meeting exemplars
   never supply client facts even in redacted form.
5. Draft; run `voice_check.py` with the same medium/to; revise at most twice.
6. "Never" is absolute. Commitments on the subject's behalf (money, dates, legal, hiring) become
   `[CONFIRM]` markers.
7. Return the draft, then after `---` a short note: slice used and its confidence (including any
   fallback and any spoken evidence), house style and overlay used, check result, disclosure. Never claim the person wrote it.

Refusals: no third-party profile without valid consent; no public figure or anyone without a profile
on disk; nothing outside consent scope.

## 9. Existing skills

Retire `docs-de-slopify` (a renamed copy of `de-slopify`) and turn `de-slopify` into a 10-line
pointer to voice-base's revision procedure; its blanket em-dash ban is wrong per 01 #7 and 03. Point
`git-commit-craftsman`, `readme-writing`, and `deck` at the matching surface guide; `deck` could
accept `voice_profile = "<slug>"` in DESIGN.md. Recommendations only; don't edit them in this build.

## 10. Adoption (recommend, don't install)

A `~/.claude/CLAUDE.md` line: "Before writing prose a person will read, follow the voice-base skill;
to write as someone, use the voice-writer agent with the medium and recipient." Or paste
`always-on-rule.md`. Optional non-blocking hooks (PostToolUse on `*.md`, git `commit-msg`) run
`voice_check.py --tells-only --quiet` and print tier-1 hits only. Mirror the rule to `.grok/rules/`
if Grok should follow it.

## 11. Acceptance tests

In `05-interfaces-and-tests.md` section 3 (A1-A34, fixtures and pass conditions).

## 12. Build order

1. `voicelib` (textproc, features, tells, delta, slices, profile_io) + `tells.json` +
   `voice_check.py`; A1-A4, A15.
2. `voice-base` SKILL.md and references. The neutral goal works after 1-2.
3. `voice-profile` template, fictional example, `voice_profile.py`; A14, A20, A21.
4. `voice-ingest`: git, text, mbox, slack, transcript (meetings), linkedin, x, then
   agent-transcripts (opt-in); clean, tag, features, exemplars, brief, finalize; A5, A7-A13,
   A19, A22, A23, A25-A31.
5. House-style binding in `voice_profile.py`, `voice_check.py --house`, and the writer; A32-A33.
6. `voice-writer`, `baseline-llm.json`; A6, A18, A24, A34.

Out of scope for v1: non-English, embedding-conditioned generation (TinyStyler), RVF/AgentDB
exemplar retrieval (v2), installing any hook, relationship as a slice key.
