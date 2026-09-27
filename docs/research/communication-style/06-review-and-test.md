# Communication-style skill set: review and test

This is the reviewer's report on the voice-* build under `~/.claude/`, checked
against `04-skillset-design.md` and `05-interfaces-and-tests.md`. It covers
three passes:

1. A static review of the first drop, which implemented design revision 1.
2. A real-corpus E2E run on the revision 3 build.
3. A second, final real-corpus E2E run after the builder's "defects fixed"
   build (77/77 tests), plus the manual acceptance tests A24/A34 on fictional
   profiles.

Corpus text isn't quoted beyond a few scrubbed words, and client meetings are
described only by style. Corpus files stayed in the session scratchpad. The
profile is at `~/.claude/voice-profiles/mathew-beane/`, outside the repo.

Process note: the lead's "hold the real E2E until the builder says defects
fixed" messages reached me after pass 2 had already run on the rev 3 build
(which already had the spoken-name redaction pass). The pass-2 corpus was
deleted. The installed profile comes from pass 3 only.

## 1. Verdict

**Neutral goal (no profile): ready.**
- On real text, tier 1 fires on 0 of 2,202 human segments and on 9 of 40 LLM baseline paragraphs.
- Only three things fail a draft: tier-1 hits, house-policy hits, and agent register leaking into writing for people.
- The client house style binds and enforces as designed.

**As-a-person goal: the pipeline is sound; written fidelity for this subject is weak and mostly unmeasurable.**
- Consent, speaker-only storage, ASR invalidation, agent isolation, confidence gating, and the fixed slice selection all hold on real data.
- The subject's human evidence is speech (975 meeting turns) plus 11 own-hand commit subjects. No written medium has function-word, rhythm, or punctuation data.
- So written drafts borrow spoken lexicon and stance, and the scorer compares them against the meeting model: "calibration insufficient", no percentile.
- Every profile-voice draft longer than 150 words came out "closer to default-LLM than to person". When the writer agent leaned on the spoken evidence, it produced a speech caricature (section 6.3).

**Before anyone relies on "writes like Mathew", three things are needed:**
- an email or Slack corpus of his own;
- D-3 (the agent-commit filter) fixed;
- a writer rule against importing speech-only habits into writing (N-12).

## 2. Defects

Severity: **Critical** means wrong data in a profile or a privacy leak; **Major** means a design requirement or acceptance test fails on real data; **Minor** is everything else.

| # | Sev | Where | Defect | Status after "defects fixed" build |
|---|---|---|---|---|
| D-1 | Critical | `adapters/agent_transcripts.py` | Relayed agent messages ingested as the subject's typing (76% of rows). | **Fixed** (by reviewer; the builder kept the fix and added fixtures and suspect scoring) |
| D-2 | Major | first drop | The first drop implemented revision 1 only. | Fixed (rev 3) |
| D-3 | Major | `agent_filter.py` | Agent-drafted commits get through. | **Open.** Excluded went from 122 to 275, but 68 of 83 surviving commits were still agent-drafted. Two misses: a lowercase ticket in the scope (`feat(weft-256):`, since the ticket regex needs uppercase) and a ticket with no conventional prefix (`WEFT-558: ...`). The Delta pass didn't catch them because the trusted pool is mostly agent-written. I excluded them via `tags.csv` review. |
| D-4 | Major | features | No per-field confidence. | Fixed |
| D-5 | Major | redaction | Names spoken in meetings weren't redacted. | Fixed: 479 `[NAME]` redactions. Meeting rows contain none of the client or person terms, including names that open a sentence. |
| D-6 | Major | tier-1 regexes | False positives on ordinary sentences. | Fixed: 0 tier-1 hits on the constructed sentences, 3 true positives kept |
| D-7 | Major | neutral scoring | The rhythm gate failed real human text. | Fixed: rhythm is WARN only |
| D-8 | Minor | default-LLM baseline | 2.9k words, written by the builder. His real held-out agent prompts sit at Delta 0.64 to self vs 0.65 to baseline. | Documented as a limitation; the "closer to default-LLM" verdict is now a WARN |
| D-9 | Minor | secret/PII patterns | Missed `*_KEY=`, Bearer, IPv6, SIN, and other patterns. | Fixed, except capability URLs with plain tokens |
| D-10 | Minor | Delta | Mismatch between draft length and chunk length. | Fixed: drafts under 150 words aren't scored |
| D-11 | Minor | skill triggers | Overlap with de-slopify and humanize. | voice-base now says when to prefer each; the old skills are untouched per 04 section 9 |
| N-1 | Major | `slices.select()` | The meeting slice was never added to written targets. | **Fixed.** `show --medium email --to external` now prints `spoken: meeting/external n=975 for lexicon,discourse,stance` plus the explanation patterns and meeting exemplar ids. |
| N-2 | Major | agent-register guard | Couldn't fire when the human slice was thin. | **Fixed.** An agent-register draft checked as chat/peer now FAILs via the meeting fallback model. The real writer's first Slack draft was caught this way (6.3). |
| N-3 | Major | redaction for agent-prompt rows | Agent rows get neither the name pass nor fuzzy matching, and fuzzy matching skips terms under 6 letters. | **Open.** a client staff member's name in possessive form, reached an **installed exemplar** in pass 3. I added two variants to redact-terms and re-collected before installing, so the installed profile is clean. Fix: run the proper-noun pass and plural/possessive term variants on every medium. |
| N-4 | Minor | fuzzy terms | Over-redacted ordinary words ("speak" matched "Speaker"). | Fixed: "so to speak" survives |
| N-5 | Minor | CLI naming | consent says `meeting`, collect says `transcript`. | Open |
| N-6 | Minor | meeting adapter | `Transcript.md` (Google Meet) was skipped. | Fixed: +47 turns from one call, tagged `unknown` until reviewed; I set them to external |
| N-7 | Minor | `brief.md` | Prints base function-word ratios from 65 words, and the over/under lists are identical. | Open |
| N-8 | Minor | `tag --import` | Marked untouched rows as user-tagged. | Fixed: only 68 or 115 edits applied |
| N-11 | Minor | git rows | `meta.subject` keeps the unredacted commit subject (a client name survived in `clean.jsonl`). | Open. The file is transient and deleted at finalize, but A31 says only raw may hold it. |
| N-12 | Major | voice-writer / profile use | Nothing stops the writer from carrying speech-only habits (fillers, lowercase, em dashes) into writing when a spoken slice is the only evidence. See 6.3. | Open. Fix: when `show` reports a written slice as base-only plus spoken, state "no written-register evidence: use neutral mechanics for G1-G3 and G7" and list the spoken fillers (G9) as forbidden in writing. |
| N-13 | Minor | voice_check report | On one draft it printed "to agent-prompt slice 0.55" and "Delta to agent-prompt slice 0.64 ... inside" (two different scales), then "though closer to the person" right under "verdict: closer to default-LLM than to person". | Open. Use one agent distance and one verdict sentence. |
| N-14 | Minor | `--house NAME` without `--profile` | The name only resolves inside a profile, so neutral use needs the .toml path. | Open |
| N-15 | Minor | A24 vs consent | The consent media enum has no `harness`, so a profile can never be in scope for a status update. The writer correctly refused A24 item 1 as the person. | Open. Decide whether harness output under a person's name exists; if not, drop it from A24. |
| N-16 | Minor | voice-writer | For the fictional A34 email, the writer took the client's first name from `roster.toml` even though the caller didn't supply it. That's not an exemplar fact, but it's also not from the inventory. | Open. Add "recipient names only from the caller" to the agent's rules. |

## 3. Static review of the first drop (summary)

- 51/51 tests passed. Frontmatter and descriptions are valid ("what it does", then "Use when ...", 521-651 chars).
- No file is over 500 lines.
- **A18 dogfood:** 0 tier-1 hits across all skill files.
- **A17 license: pass.** I compared six Wikipedia-derived tells against the page. Names, fixes, and examples are original; the overlap is only short example phrases such as "serves as".
- **Delta:** a sound Burrows variant (mean |z|, SD floor 2 per 1k, one shared SD).

## 4. Final E2E on the real corpus (pass 3, 77/77 tests)

**Consent refusals behaved as designed.**
- `--sources` must name `meeting` and `agent-transcripts` explicitly.
- `transcript` without `--attest-recording-rights` exits 3.
- `agent-transcripts` without the opt-in flag exits 2.

| step | result |
|---|---|
| git | 427 trailer-less non-merge commits; 511 dropped for agent trailers |
| meetings | 1,152 subject turns (66,366 words) from 15 calls. Other speakers' turns were read but not stored; recaps, summaries, chapter files, and PDFs skipped; VTT/txt and ` (1)` duplicates collapsed. Other speakers' phrases grep to 0. Participants stored as hashed ids plus org-class counts. |
| agent prompts | 1,257 human-typed prompts; relays, tool results, and harness text dropped; 8 suspect rows excluded |
| clean | 2,603 of 2,837 kept. `[NAME]` 480, `[TERM]` 281, `[PERSON]` 133, key 34, url 22, email 7, figures 23. 275 suspect rows excluded, plus 68 commits by review. |
| finalize | `corpus/` and `brief.md` removed. Directories 0700, files 0600 (A12, A13). |

| slice | n_own | status | rhythm | function words | punctuation | discourse | openers |
|---|---|---|---|---|---|---|---|
| meeting/external | 975 | own | invalid (asr) | high | invalid (asr) | high | high |
| commit/peer | 11 | insufficient | insufficient | insufficient | insufficient | insufficient | low |
| agent-prompt/agent | 1,002 | own | high | high | high | high | high |

**Checks the lead asked for:**
- **ASR stats kept out of written stats.** Base rhythm and punctuation come from the 11 commits only, so they're `insufficient`. Meeting G2/G3 are `invalid (asr)`. Disfluencies are counted separately (like 26.8, you know 4.4, um 1.5 per 1k).
- **Redaction on the output profile.**
  - A grep for emails, `sk-`/`ghp_`/`AKIA`/JWT/Bearer tokens, and phone patterns finds nothing.
  - A grep against the client/person term list finds only the subject's own name, in `consent.json` and `profile.md`.
  - The exemplars (3 meeting turns of 80 words or less, 5 agent prompts) contain no client names after the N-3 variant fix.
  - `ingest-log.jsonl` holds counts and ids only.
- **A16.** The skills contain none of the subject's identifiers. Against the real redact-terms list, the only hits are the generic words "room" and "notetaker", which my label harvest added.
- **Agent isolation (A22/A23).**
  - Base discourse differs from the agent slice ("so" 20.9 vs 5.1 per 1k), and the openers differ too ("yeah i mean" vs "we need to").
  - Human-target `show` lists no agent exemplars.
  - The guard FAILs an agent-register draft checked as chat/peer.
- **Profile validation.** `validate` fails only on section 5 ("Never"), which the subject must supply. Drafts used a scratch copy with a TEST ONLY line.

**What this corpus can and can't support:**

| field | supported? | source |
|---|---|---|
| discourse markers, lexicon phrases, stance, explanation patterns | yes, high | meeting/external |
| speech function words | yes, high | meeting/external |
| written rhythm, punctuation, function words (any human medium) | **no** | 65 own-hand commit words |
| commit register | no | 11 subjects in two inconsistent styles |
| email, chat, slides, social, docs | no corpus | none |
| agent-prompt register | yes, high | opt-in typed prompts |
| base "voice in brief" (needs 2 human slices) | no | only one human slice |

## 5. House style (client COMMUNICATION.md) and deviations from 05

**House style.** The binding `house/client-a.toml` cuts the base block by its BEGIN/END markers. It is scoped to external email, slides, doc, chat, and meeting, with `metrics.mode = "report-only"` because the guide forbids tuning prose to a score. Rules:
- 1 policy rule: ticket ids and file paths in client copy;
- 3 default rules: assigning the reader a need, staged today/tomorrow framing, routine closers.

| test | result |
|---|---|
| client email with a ticket id and a `.rs` path | FAIL, 2 house-policy hits (with the profile, and neutral with `--house`) |
| "You need to ..." | WARN (house default) |
| same text as chat/peer, outside scope | `house none`; the rule isn't applied |
| a house rule that relaxes T1-closing-offer | `validate` exit 1 |
| rhythm under report-only | printed as `info`, never WARN |
| fictional A32/A33 (unit tests) | pass. The writer's fictional A34 draft named the house (`fixture`) and overlay (`email/external`) and dropped em dashes because a house policy forbids them. |

Gaps:
- The client guide's overlays aren't individually addressable; they're unmarked examples under one heading.
- Its core asks ("don't presume the reader's situation", "don't stage a transformation") can't be checked by regex and stay writer judgment.

**The builder's deviations from 05, with my assessment:**

| # | deviation | assessment |
|---|---|---|
| 1 | FAIL only on tier-1, house policy, and agent-register leak; tier-2, rhythm, and "closer to LLM" are WARN | **Agree.** The data supports it: the rhythm gate misfired and the baseline doesn't discriminate. The consequence to state plainly is that no automated check fails a draft for not sounding like the person. |
| 2 | Zoom chat rows count as written chat, with valid G2/G3 | Agree. It's typed text. |
| 3 | Fuzzy alias match at 0.85 by default | **Disagree as a default.** It's a privacy risk: "Mat Beane" scores 0.857 and "Mathew Bean" 0.957, so a relative or colleague with a similar name would have their turns stored as the subject's. No real label came close (the next best is 0.42), so there was no harm on this corpus. Recommend exact matching plus `roster.self.aliases` for ASR misspellings, with fuzzy matching opt-in. |
| 4 | `unknown` audience allowed; such rows feed base only | Agree. The 47 `unknown` meeting turns showed up in the tag summary with confidence < 0.6, as intended. |
| 5 | scope.sources enforced for every source; meeting and agent-transcripts must be named | Agree. Stricter than 05. |
| 6 | Participants stored as hashed ids | **Partly disagree.** `tagging.hid()` is an unsalted SHA-1 of the normalized name, and the roster holds the candidate names, so it can be re-identified by dictionary. The ids only live in the transient corpus, so this is low impact. Use a per-profile salt (HMAC) or drop the ids after `tag`. |

## 6. Fidelity drafts and scores

### 6.1 The drafts

There are two sets.
- **Mine:** written by hand following the agent procedure. The six the lead asked for, plus the client email and an agent prompt.
- **The agent's:** 10 drafts by a Sonnet subagent that ran `voice-writer.md` verbatim. The agent type isn't registered in this session because it was created after the session started. It wrote 5 in the profile voice (V) and 5 neutral (N).

All client specifics are placeholders.

Excerpts (first lines; full texts are in the scratchpad):

- **V Slack (mine):** "So I put together a set of voice skills for Claude Code, four skills and an agent under ~/.claude. voice-base is the rules, basically 60 tells in three tiers..."
- **N Slack (mine):** "I've added a communication-style skill set to Claude Code under ~/.claude: voice-base (a 60-entry catalog...)..."
- **V Slack (agent):** "hey — so the communication-style setup's actually done now, it's living under ~/.claude..."
- **V client email (mine):** "Thanks for walking through [WORKFLOW] with me today. That was good feedback on the [SCREEN], so I'm taking [FIELD] off it and keeping the rest where you'd expect it..."
- **V client email (agent):** "Good talking through [WORKFLOW] with you today... I'll show it actually working before we call it done."
- **N client email (agent):** "Thanks for the time today going through [WORKFLOW]... we'll show it working before calling it finished."
- **V agent prompt (agent):** "fix the agent-commit filter, commits with a conventional prefix plus a ticket id should get excluded but some are still slipping through. add a fixture..."
- **Commits and slides:** the V and N versions carry the same facts. V uses first person and "Not fixed here:"; N is impersonal.

### 6.2 Scores (pass-3 profile)

Each row shows the slice line the checker printed.

| draft | words | slice (confidence) | result | distance and verdict |
|---|---|---|---|---|
| V Slack, mine | 137 | chat/peer base only (all insufficient) | PASS | too short |
| N Slack, mine | 110 | none | PASS | too short |
| V Slack, agent | 201 | chat/peer base only, fallback model meeting/external | WARN | 0.75 vs LLM 0.71, agent 0.72: closer to LLM. **First draft FAILed as agent register**, then was revised. |
| N Slack, agent | 102 | none | PASS | n/a |
| V commit, mine / agent | 78 / 101 | commit/peer n=11, insufficient, base only | PASS / PASS | too short |
| N commit, mine / agent | 62 / 95 | none | PASS / PASS | n/a |
| V slides, mine | 217 | slides/peer base only, meeting fallback | WARN (tricolon 0.92/100w; CV 0.26) | 0.77 vs LLM 0.67: closer to LLM |
| N slides, mine | 175 | same | WARN (tricolon 1.14/100w) | 0.80 vs LLM 0.59: closer to LLM |
| V slides, agent | 355 | same | WARN (tricolon; inside agent-slice range) | 0.63 vs LLM 0.52: closer to LLM |
| N slides, agent | 278 | none | PASS | n/a |
| V client email, mine / agent | 110 / 89 | email/external base only; **spoken: meeting/external n=975**; house client-a | PASS / PASS | too short |
| N client email, mine / agent | 102 / 72 | house client-a | PASS / PASS | n/a |
| V agent prompt, mine r0 / r1 | 159 / 172 | agent-prompt/agent own n=1002 (high) | WARN (CV 0.38 / 0.40) | 0.78 / 0.80 = **p100**, LLM 0.80 / 0.81: closer to agent slice |
| N agent prompt, mine | 172 | same | WARN (T2-kobak "underscores", a false positive on the noun) | 0.63 = p64, LLM 0.58: closer to LLM |
| V / N agent prompt, agent | 51 / 54 | same | PASS / PASS | too short |
| **held-out real meeting turns** | 261 | meeting/external | PASS | **0.58 = p3**, LLM 0.66, agent 0.64: closer to person |
| **held-out real agent prompts** | 175 | agent-prompt/agent | PASS | 0.64 = p64, LLM 0.65: closer to agent slice (0.01 margin) |

### 6.3 Where the profile voice misses

- **The checker can't see written voice.** 8 of the 12 human-audience drafts checked against the profile are under 150 words and get no distance. The 4 that were scored all came out closer to the default-LLM than to him, measured against his *speech* model, which isn't a fair target for writing. His real held-out speech lands at p3, so the model itself is fine; it just isn't a written-voice model.
- **The agent's V Slack caricatures his speech.** It's all lowercase, opens "hey —", and piles in "actually", "basically" and "its own little bucket". None of that is written-register evidence:
  - lowercase starts come from his *agent* slice (20%), not from any human writing;
  - the fillers are G9 speech disfluency.
  
  Its first draft (comma-stacked asks) FAILed as agent register, which is the guard working, and the revision swung to speech. This is N-12.
- **My first agent prompt overshot his lowercase habit**: 100% against the real 20%. My profile text said "lowercase starts common" instead of the rate, which breaks llm-pass rule 1. I fixed the line. Both my V prompts land closer to his agent slice than to the LLM baseline, but at p100 and by 0.01-0.02. The N prompt lands closer to the LLM. The direction is right; the margin is noise (D-8).
- **V and N client emails are hard to tell apart**, mine and the agent's alike. The differences are contractions, first person, and "I'll show it working". The spoken stance traits (naming what isn't built, accepting feedback in one line) came through in both V versions, but nothing measures them. They rest on my reading of 975 turns, which is a single LLM judgment: the weakest method in 01 section 5.
- **Commit voice isn't recoverable from this repo.** The own-hand commits are February lowercase status lines; the late-September capitalized subjects may be agent-drafted.

### 6.4 A24/A34 on the fictional profiles (voice-writer via subagent)

- **A24:** 10 drafts (5 neutral, 5 as `terse`).
  - All neutral drafts PASS.
  - As `terse`: chat/peer used its own slice; email/manager fell back to email/peer (d=0.15); commit/peer fell back to chat/peer (d=0.35) with spoken meeting/peer; agent-prompt used its own slice (WARN on rhythm after 2 revisions, correctly not chased).
  - The status update as `terse` was **refused** because `harness` isn't in consent scope (N-15).
  - The agent reported no exemplar facts used and listed the exemplar phrases it declined to copy.
- **A34:** `terse` email/external printed `written: email/peer (d=0.25) + base; spoken: meeting/external n=33 for lexicon,discourse,stance` and `house fixture (overlay email/external)`. It PASSes. The note split the groups correctly: rhythm and structure from the written fallback, word choice from meetings. It took the recipient's first name from the roster (N-16).
- **A34 on the real profile:** email/external likewise printed the spoken supplement and house `client-a`, and both V emails passed.

## 7. Skill text against its own tier-1 rules (A18)

- **Skill files:** after the final build, all 26 voice-* skill, reference, NOTICE, and agent files scan at 0 tier-1 hits.
- **This file:** it scans at 0 tier-1 hits and 0 tier-2 hits.

## 8. Artifacts

- **Profile:** `~/.claude/voice-profiles/mathew-beane/`, from pass 3. It fails `validate` until the subject fills section 5 ("Never").
- **House binding:** `~/.claude/voice-profiles/mathew-beane/house/client-a.toml`.
- **Reviewer fix:** `~/.claude/skills/voice-ingest/scripts/adapters/agent_transcripts.py` (D-1).
- **Scratch only:** the pass-3 drafts, `tags*.csv`, held-out samples, and the fictional demo profiles. The pass-1 stopgap output and the pass-2 corpus were deleted.

## 9. Final re-verification (2026-09-27, 94/94 build)

This was a targeted pass on the builder's final fixes. I ran one fresh ingest from a clean root, **without** manual `tags.csv` exclusions, and used the builder's current name-variant handling in place of the variants I'd added by hand. The corpus was deleted after finalize. The profile was reinstalled because the ingest changed: meeting/external n=1003, agent-prompt n=1006, commit/peer n=11.

| item | result |
|---|---|
| D-3, git with no manual exclusions | **Improved but still open.** 316 rows were flagged (275 before). Of the 43 commits that survived, **28 are still agent-drafted**: conventional prefix, no ticket in the subject, prose body, suspect score 0.33. Before this pass the count was 93, then 68. Unreviewed, they turn commit/peer into a "pooled" slice with rhythm and punctuation at low confidence and 5 agent-written exemplars, and they seed the base layer's written stats. For the installed profile I excluded them through the designed `tags.csv` review (28 edits), which leaves commit/peer at n=11, insufficient. |
| N-3, name pass on agent-prompt rows | **Fixed for the shapes seen.** 109 of 1,106 agent rows now carry `[NAME]`. The possessive staff-name case is caught without the hand-added variant. Residue: lowercase run-together client tokens in agent rows (client product names written as one lowercase word: 6 hits in the corpus). None reached an exemplar or the installed profile, which greps clean for client and person terms, PII, and secrets. Minor. |
| N-3 side effect | The proper-noun pass now also rewrites CamelCase code identifiers in commit bodies (e.g. `[NAME].spatial`). That's harmless for privacy but distorts commit lexicon. Minor. |
| N-11 | Fixed: no `meta.subject` on git rows. |
| N-12, speech habits | **Fixed.** `show` for email/external and chat/peer prints both `register` lines ("no written-register evidence..." and "spoken habits never transfer to writing..."). The regenerated real-profile Slack and client email (voice-writer via a Sonnet subagent running the agent file verbatim) have 0 fillers and no "hey". The only lowercase sentence starts are identifiers such as `voice-base`. Both PASS with 0 revisions; the email names house `client-a`. voice_check now warns on fillers in a written draft (a test caricature got `spoken fillers in a written draft (um, you know)`). "I mean", "like," and "basically" aren't counted, which is minor. |
| per-block overlay (N-10) | **Works.** `[[overlays]] match = "slides/external"` with `label = "Example overlay for an executive presentation:"` binds only that fenced block. `show --medium slides --to external` prints `house client-a (overlay slides/external)` with that block and not the technical-review one. `validate` passes. |
| alias default (deviation 3) | **Accepted as fixed.** The default is an exact match, or the same surname with a first name within one edit. "Matthew Beane" and "Mathews Beane" match. "Mat Beane", "Matt Beane", "Mathew Bean", "Andrew Beane", "M. Beane" and "Mathew Beane Jr" don't. Fuzzy matching is opt-in. |
| salted hashes (deviation 6) | **Fixed.** `hid()` is HMAC-SHA256 under a per-profile `salt` file (0600, 65 bytes). Of 16 participant ids, none equals the unsalted SHA-1 of any real speaker label. |

**Updated verdict.**
- **Neutral goal: ship.**
- **As-a-person goal:** the pipeline now protects the subject's written voice from both known contaminants: agent register (N-2) and speech habits (N-12). It keeps client names out of the profile.
- What it still can't do is learn his *written* voice. There's no written human corpus, and the one written source (git) still needs a manual review step, because conventional-prefix agent commits without tickets score 0.33.
- Written drafts are now neutral-mechanics text carrying his spoken word choice and stance. That's honest and safe, but not yet "sounds like Mathew". Next steps: an email or Slack export, and a D-3 signal for untagged conventional bodies (or treating a repo's agent-wave days as untrusted).

**Addendum (2026-09-27, 97/97 build).** Scope: the changes since the 94-build pass above. I used git only plus existing drafts; no client corpus was reprocessed.

- **D-3: unchanged.** A git-only ingest from a throwaway root excluded 307 rows. The same **28 of 43** surviving commits are agent-drafted (conventional prefix, no ticket, scores 0.17-0.33). The case-insensitive and bare-ticket signals don't touch this shape. The ingest result is the same as §9, so I didn't reinstall the profile.
- **N-12 threshold:** a written chat draft with "you know"/"um" WARNs at 69/1k against a written base of 0/1k.
- **N-13:** one `agent` line on the slice's own scale, and one verdict.
- **N-14:** `--house client-a` resolves without `--profile`.
- **N-15:** `harness` is in the default consent media.
- **N-16:** voice-writer.md says names come only from the task, never from the roster; a missing name becomes `[NAME]`.

The §9 verdict stands. D-3's remaining shape (ticketless conventional commits with prose bodies) is the one open data-quality defect.
