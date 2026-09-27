# Communication-style skill set: interfaces and acceptance tests

Companion to `04-skillset-design.md` (the design: context model, profile template, precedence,
privacy, agent). This file holds the exact formats, CLIs, and the tests the reviewer runs. Section
numbers like "4.5" refer to 04.

## 1. Data formats

**tells.json**:
```json
{"schema":"voice-tells/v1","tells":[
 {"id":"T1-closing-offer","tier":1,"name":"Closing offer on finished work",
  "media":["all"],"audiences":["all"],
  "detect":{"type":"regex","pattern":"(?im)^.*\\blet me know if you('d| would) like\\b.*$"},
  "weight":3,"fix":"Delete it, or name the one real follow-up.",
  "source":"03-tells-by-surface a; blader #22"}]}
```
`detect.type`: `regex`, `wordlist` (+ `min_per_1k`), `structure` (named check in `tells.py`:
`bold-label-bullets`, `headers-on-short-text`, `tricolon-density`, `uniform-sections`,
`restates-question`, `summary-restates`), or `manual` (checklist only).

**Corpus row** (`raw.jsonl` / `clean.jsonl`):
```json
{"id":"mbox:3f2a9c1e","source":"mbox","medium":"email","audience":"external",
 "relationship":"working","tag_source":"inferred","tag_confidence":0.7,
 "text":"...","timestamp":"2025-03-04T15:22:00Z","thread_id":"<root-msgid>",
 "is_reply":true,"lang":"en","words":142,"sentences":9,
 "redactions":{"email":1,"person":2},"suspect_agent":0.1,"exclude":false,"meta":{}}
```
`id` = `<source>:<sha1(normalized text)[:8]>`.

**stats.json**:
`{"schema":"voice-stats/v2","base":{...},"slices":{"email/peer":{"own":{...},"pooled":{...},"n_own":..,"n_eff":..}}}`.
Each block: `segments, words, sentences, sent_len{mean,median,p10,p90,sd,cv}, para_sents{median},
fragment_rate, question_rate, exclaim_rate, contraction_rate, first_person{i,we}, sent_initial{...},
punct_per_1k{emdash,endash,dashdash,semicolon,colon,paren,ellipsis}, emoji_per_1k,
lowercase_start_rate, mattr_50, mean_word_len, fw_freq{w:per_1k}, fw_seg{w:{mean,sd}},
discourse{m:per_1k}, kobak_per_1k, hedge_per_1k, md{bullets,headers,bold}, openers{top},
closers{top}, confidence{field:level}`.

**calibration.json**: held-out 20% split **by thread**, per slice with >= 8 held-out segments (else
`"insufficient": true`): `self_delta{median,p90}`, `baseline_delta{median}`, `cv_range[p10,p90]`,
`n_heldout`, and `cross_slice_delta{other_slice: median}` (used by test A22).

## 2. Script interfaces

### 2.1 voice_profile.py
```
voice_profile.py init <slug> [--root DIR] [--relation self|other]
voice_profile.py validate <slug|path> [--json]
voice_profile.py show <slug> --medium M --to AUDIENCE [--relationship R]   # selected slice + base + overrides + Never + exemplar ids, with the selection line
voice_profile.py slices <slug> [--json]                                    # table: slice, n_own, n_eff, status, confidence per field
voice_profile.py overrides <slug> [--json]
```

### 2.2 voice_ingest.py
```
voice_ingest.py consent <slug> --relation self|other --subject NAME [--form F --evidence T] [--media a,b] [--audiences a,b] [--uses a,b] [--expires D]
voice_ingest.py collect <slug> --source mbox --path Takeout.mbox --from me@x.com[,alt@y.com]
voice_ingest.py collect <slug> --source slack --path export_dir --user-id U123
voice_ingest.py collect <slug> --source linkedin|x --path export_dir
voice_ingest.py collect <slug> --source git --repo PATH --author EMAIL [--author E2] [--public-repo] [--include-suspect]
voice_ingest.py collect <slug> --source text --path DIR --medium M --audience A [--relationship R]
voice_ingest.py collect <slug> --source transcript --path DIR --alias "Full Name" [--alias A2] --attest-recording-rights [--include-docx] [--scan-zips]
voice_ingest.py collect <slug> --source agent-transcripts --opt-in-agent-register [--projects-dir ~/.claude/projects] [--project GLOB]
          common: [--since D] [--until D] [--limit N] [--dry-run]
voice_ingest.py clean <slug> [--no-redact-names] [--redact-terms FILE] [--redact-figures auto|on|off] [--min-words 4] [--keep-raw]
voice_ingest.py tag <slug> --infer | --export tags.csv | --import tags.csv | --summary
voice_ingest.py features <slug> [--heldout 0.2] [--seed 7]
voice_ingest.py exemplars <slug> [--per-slice 5]
voice_ingest.py brief <slug> [--per-slice 12]
voice_ingest.py finalize <slug> [--keep-corpus]
voice_ingest.py status <slug> | purge <slug> [--corpus-only] --yes
```
Adapter rules:
- **git**: `--no-merges`; drop commits with a `Co-Authored-By:` naming Claude, claude-flow, Copilot,
  Cursor, Codex, Grok, or an `@anthropic.com`/bot/noreply address, or a "Generated with" line, or a
  claude.ai/code session link. Then score trailer-less commits for `suspect_agent` (0-1): markdown
  headers in the body, "Summary"/"Test plan"/"Changes" sections, >= 3 bold-label bullets, emoji or
  checkmarks, "This commit/PR", body > 25 lines, T1/T2 tell density above the slice median, and a
  second pass that flags commits whose Delta against the author's other trusted commits is above
  p95. Default: exclude >= 0.5 and list them for review; `--include-suspect` keeps them.
- **transcript** (meetings): walks DIR. Formats: WebVTT cues whose text is `Speaker: text` (skip
  `*.chapter.vtt`); Otter/Fathom txt with `m:ss - Speaker` (or `h:mm:ss`) header lines followed by
  text; `MM:SS Xx — Speaker` header lines (1-3 letter initials, an em dash, the full name; match on
  the full name, not the initials) followed by text; Zoom chat files (`*Chat*.txt`, `HH:MM:SS<TAB>Speaker:<TAB>text`) -> medium `chat`; `.docx`
  (stdlib zipfile, `word/document.xml`) only with `--include-docx` and only when paragraphs match a
  speaker pattern. Skip: Word lock files (`~$*`), AI summaries (`*Recap*`, `*Summary*`, `Notes by
  Gemini`, `*.pdf`), media, and speakers that are notetaker bots (`/notetaker|otter|fathom|read\.ai|bot/i`).
  Keep only turns whose speaker label matches an `--alias` (case-insensitive, exact after
  whitespace normalization); drop `Unidentified Speaker` and unknown labels. Merge consecutive turns
  by the same speaker (VTT gap < 5 s); drop merged turns under `--min-words`. Dedupe meetings: strip
  ` (1)`/` (2)` from names, then treat files whose subject-turn 5-word shingles overlap by Jaccard
  >= 0.6 as one meeting (VTT and txt of the same call), keeping the copy with more subject words.
  `thread_id` = meeting id (folder or date+title).
- **agent-transcripts**: exit 2 without `--opt-in-agent-register`. Keep only `type == "user"` records with
  human-typed string/`text` content; drop `tool_result`, `isMeta`, `<command-*>`,
  `<local-command-*>`, `<system-reminder>`, `<pasted_content>`, `<teammate-message>`, and any
  harness-injected block. Tag `agent-prompt`/`agent`, source note "agent-directed register, not
  voice evidence".
- **all**: strip code, stack traces, logs, quoted replies (`>`, "On ... wrote:", "-----Original
  Message-----", "From:/Sent:"), signatures (`-- ` plus trailing blocks repeated in >= 20% of
  messages), auto-mail. Dedupe by hash, then 5-word-shingle Jaccard >= 0.8. The suspect-agent score
  is also computed for email/pr/doc and reported; excluded only for git by default.

`brief.md`: the slice table, base and per-slice stats, over/under-used function words vs
`baseline-llm.json`, then a stratified sample per slice (by year, short/long) with IDs. `exemplars`:
per slice, 40-250-word segments (chat: sets of 5 short messages) nearest the slice centroid,
diversified by maximal marginal relevance and relationship, excluding held-out segments and segments
with > 2 redactions (meeting exemplars: <= 80 words, <= 1 redaction token per 20 words, at most 3
per slice, used only to show explanation patterns). Format: `## <id> · <words>w · <relationship> · <year>` then the text.

### 2.3 voice_check.py
```
voice_check.py [FILE|-] --medium M [--to AUDIENCE] [--relationship R] [--profile SLUG|PATH]
               [--house NAME|PATH] [--json] [--tells-only] [--embed styledistance|wegmann|luar] [--quiet]
```
`--medium` is inferred when omitted (`COMMIT_EDITMSG` -> commit, `*.md` -> doc, else chat) and
`--to` defaults to `peer`; both defaults are printed as "assumed". Without a profile, scoring is
neutral (sentence CV >= 0.45 for >= 6 sentences). With a profile, the slice is chosen by 04 section 4.5. Fenced
code blocks are skipped.
```
voice-check  medium=email to=manager  profile=fictional-terse-dev  words=184
slice    email/manager: own n=0 -> fallback email/peer (d=0.15) + base   confidence rhythm=medium fw=low
house    none
FAIL     tier1: 2   tier2: 3 (1 suppressed by profile)   tier3: info only
  L3  T1-opener-hope-well    "I hope this finds you well"
  L6  T2-kobak               "crucial", "pivotal" (2 in 184w)
rhythm   sentence CV 0.31 (slice p10-p90: 0.52-0.88)  LOW
distance Delta 1.42 = p94 of slice held-out (p90 1.30) | to LLM baseline 0.96 | to agent-prompt slice 0.88
verdict  closer to default-LLM than to person
embed    skipped: sentence-transformers not installed
```
FAIL when any tier-1 hit remains, tier-2 weighted score > 4 per 100 words, or the verdict is "closer
to baseline" (or closer to the agent slice for a human audience). Delta: z-score function-word rates
with the slice's `fw_seg` mean/sd, mean |z| over the top 150 words, compared to that slice's
calibration. With `fw` confidence below low, Delta is reported as "not scored (insufficient)" rather
than computed.


## 3. Acceptance tests (reviewer runs these)

Fixtures are fictional and builder-written. Two authors: `terse-dev` (short sentences, lowercase
chat, em-dashes, no hedges) and `warm-manager` (long sentences, semicolons, hedges, greetings), >=
3,000 words each over email and chat to several audiences, commits, and docs. Shipped as an mbox, a
Slack export (DMs, channels, one shared channel), a git repo the test builds in a temp dir, and a
transcript `.jsonl` for `terse-dev` in a deliberately different agent register (imperative, "pls",
"don't", numbered steps, marker token `zqx`). Also a roster, `slop.md`, `clean.md`, and a thin slice
(email/manager, 30 sentences). Meeting fixtures (fictional company names only): 6 calls for
`terse-dev` as VTT, Otter-style txt, `MM:SS Xx — Name` txt, a Zoom chat file, one docx, a `.chapter.vtt`, a `*Recap.txt`,
a `" (1)"` duplicate and a VTT/txt pair of the same call; other speakers include a notetaker bot and
`Unidentified Speaker`; a canary token `qlmv` appears only in other speakers' turns; ASR-style text
with heavy commas, fillers ("um", "you know"), and 3 external-client calls plus 3 internal ones. A
fictional house style (`house-fixture.md` + `house/fixture.toml`) with one policy rule, one default
rule, and two overlays.

| # | Test | Pass condition |
|---|---|---|
| A1 | `python3 -m unittest discover ~/.claude/skills/voice-check/tests`, no extras, no network | all pass |
| A2 | `voice_check.py slop.md` / `clean.md` | exit 1 with tier-1 hits and line numbers / exit 0 |
| A3 | em-dash-heavy text, neutral | not a failure (tier 3 info only) |
| A4 | tier-2 override in fixture profile; a tier-1 override | suppressed and reported / `validate` exit 1 |
| A5 | ingest both authors | held-out text of each scores a lower Delta percentile against its own profile than the other's (4/4) |
| A6 | `slop.md` against `terse-dev` | verdict "closer to default-LLM" |
| A7 | git: human, co-authored, "Generated with", session-link, merge, and 3 trailer-less agent-style commits | only human non-merge commits kept; >= 2 of 3 trailer-less ones flagged `suspect_agent` and listed |
| A8 | agent-transcripts without the opt-in flag / with it | exit 2 / only human-typed text, tagged agent-prompt/agent |
| A9 | mbox quotes, signatures, forwards | stripped; redaction tokens present; no seeded secret in any output |
| A10 | `collect` without consent; `other` without evidence; expired | exit 3 each |
| A11 | `init --root` inside a git repo | exit 3 |
| A12 | perms after ingest (incl. roster.toml) | dirs 0700, files 0600 |
| A13 | `finalize` without `--keep-corpus` | no corpus/, tags.csv, or brief.md; stats, calibration, exemplars remain |
| A14 | `validate` on a fresh template / on the fictional example | exit 1 listing placeholders / exit 0 |
| A15 | skill lint | frontmatter valid; referenced files exist; no file > 500 lines; tell IDs unique; override IDs and scopes resolve |
| A16 | leak scan | `grep -riE 'aepod|mathew|beane|@gmail' ~/.claude/skills/voice-* ~/.claude/agents/voice-writer.md` finds nothing; `grep -rif <profile>/redact-terms.txt` over the same paths finds nothing (no client names, projects, or figures in skills) |
| A17 | license | NOTICE.md present; reviewer spot-checks 5 tells against Wikipedia: paraphrased, not copied |
| A18 | dogfood | voice-* SKILL.md and references: 0 tier-1 hits (bad examples sit in fenced blocks, which are skipped) |
| A19 | tagging | mbox same-domain -> peer, other domain -> external; Slack DM/channel/shared -> as 4.1; roster overrides inference; CSV import round-trips with `tag_source = user` |
| A20 | slice selection | `show --medium chat --to peer` selects own slice; `--medium email --to manager` (thin) prints a fallback line naming the neighbor slice, `d`, and confidence per field |
| A21 | thin-slice honesty | email/manager rhythm confidence is `none` and written `insufficient`; a hand-inserted rhythm number there makes `validate` fail |
| A22 | register separation | marker `zqx` and agent-register rates are ~0 in base and every human slice's stats; `show` for chat/peer and commit/peer lists no agent-slice exemplars; an agent-register Slack draft checked as `--medium chat --to peer` FAILS as closer to the agent slice; the same text checked as `--medium agent-prompt --to agent` passes the distance check |
| A23 | weighting | base-layer stats are identical with and without the transcript source ingested (agent_base_weight 0) |
| A24 | manual: `voice-writer` on a status update, a Slack reply to a peer, an email to a manager, a commit message, and an agent prompt, neutral and as `fictional-terse-dev` | all pass voice_check; each prints the slice used; no fact traceable only to an exemplar; the agent prompt reads like the agent slice and the Slack reply doesn't |
| A25 | meeting parsing | VTT, Otter txt, `MM:SS Xx — Name` txt, docx (with `--include-docx`) yield the subject's turns; Zoom chat rows tagged `chat`; `.chapter.vtt`, `*Recap*`, `~$*`, pdf skipped |
| A26 | speaker filtering | only `--alias` turns persisted; bot and `Unidentified Speaker` turns absent; canary `qlmv` appears in no file under the profile dir, including transient ones before finalize |
| A27 | merge and dedupe | consecutive same-speaker turns merged; the `(1)` copy and the VTT/txt pair each count once; kept copy is the one with more subject words |
| A28 | meeting audience | calls with a roster-external participant -> `external`; internal-only -> `peer`; participant names stored only as org-class counts |
| A29 | feature validity | base-layer G2/G3 stats (punctuation, rhythm) identical with and without the meeting source; meeting slices show G2/G3 as `invalid (asr)`; base G4-G6 do change |
| A30 | disfluency | "um", "uh", "you know" absent from meeting G1 counts and present in G9; the same words in written fixtures still count in G1 |
| A31 | confidentiality | fixture client names, amounts, percentages, dates in subject turns come out as `[ORG]`/`[AMOUNT]`/`[NUMBER]`/`[DATE]`; meeting exemplars <= 80 words, <= 3 per slice; `transcript` without `--attest-recording-rights` or without `meeting` in consent scope exits 3 |
| A32 | house precedence | house policy `forbid` beats a person `allow` (hit reported as house rule); person habit with medium+ confidence beats a house `default`; tier-1 hit fails even if a house rule relaxes it, and `validate` rejects that rule |
| A33 | house binding | `show --medium email --to external` names the house style and the matching overlay; `--to peer` outside the scope names none; `voice_check.py --house` prints the house line and applies `H-*` tells |
| A34 | manual: writer, email to an external client as `terse-dev` with only a thin written external slice | note says lexicon/discourse/stance came from `meeting/external` and rhythm/punctuation from the written fallback; no client fact from any meeting appears; house overlay named |
