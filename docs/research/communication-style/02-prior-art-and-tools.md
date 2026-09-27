# Prior art and tools for a communication-style skill set

Survey of what already exists before we build skills that (1) strip AI-slop tells
out of agent-written text and (2) let an agent write as a specific person from a
voice profile filled by an ingest pipeline. Covers local skills on this machine,
public Claude Code skills/plugins, commercial voice-cloning products, and the
open models/libraries usable in an ingest pipeline.

## Summary table

| Name | Type | What it does | License | Reuse verdict |
|---|---|---|---|---|
| `~/.claude/skills/de-slopify` | local skill | Manual anti-slop pass: emdash, "Here's why", "It's not X, it's Y", forced enthusiasm | n/a (local) | adopt as base, but thin |
| `~/.claude/skills/docs-de-slopify` | local skill | Byte-identical duplicate of de-slopify | n/a | merge/delete duplicate |
| `~/.claude/skills/readme-writing` | local skill | README structure/templates; not a tone tool | n/a | pattern-only |
| `~/.claude/skills/git-commit-craftsman` | local skill | Conventional Commits format; no voice handling | n/a | skip |
| `~/.claude/skills/skill-builder` | local skill | SKILL.md spec/scaffolding conventions | n/a | reference for packaging |
| `~/.grok/skills/deck` (symlinked as `deck`) | local skill | Presentation builder; voice only via DESIGN.md free text + "cold-reader test" | n/a | pattern-only |
| Wikipedia "Signs of AI writing" | reference doc | ~15k-word taxonomy of AI writing tells, real examples | CC BY-SA | adopt as core reference |
| blader/humanizer | Claude Code skill | 25-pattern anti-slop rewrite + optional 2-3 paragraph voice sample matching | MIT | adopt |
| jooray/humanizer | Claude Code plugin | Fork/reimplementation targeting Wikipedia's 54 signs; explicit "sample teaches style, not facts" framing | MIT | adopt |
| rlorenzo/humanize | Claude Code framework | Skill+scorer+subagent+hook+always-on rule; 34 patterns, 4 domain profiles | MIT | adopt (best-engineered) |
| rampstackco/claude-skills brand-voice | Claude Code skill | 4-layer brand voice doc (attributes, tone shifts, vocab/grammar, paired examples) + stress test | check repo (not confirmed) | adapt |
| apreshill/straight-talk | Claude Code skills | codex-voice / brand-voice / social-voice, content-agnostic voice distillation | unclear (none found) | pattern-only |
| anthropics/skills | official repo | Enterprise/creative/doc skills; no dedicated voice-clone skill found | MIT (repo default) | reference for packaging conventions |
| Jasper Brand Voice | commercial product | Upload ≤8 examples/files/URLs, generates a "voice excerpt," applies org-wide | proprietary SaaS | pattern reference |
| Copy.ai Brand Voice + Infobase | commercial product | Voice profile from existing content + separate knowledge base (Infobase), referenced via `@` tags | proprietary SaaS | pattern reference |
| Delphi.ai | commercial product | Full conversational clone from a person's whole corpus (books, podcasts, video); separate voice-audio cloning | proprietary SaaS | pattern reference (out of scope: audio) |
| Lex | commercial product | "Style Guides" train continuation-matching; explicitly preserves voice rather than generating | proprietary SaaS | pattern reference |
| Gmail "Help me write" | commercial feature | Tone/style personalization mined automatically from the user's sent mail + Drive, no explicit profile step | proprietary | pattern reference |
| LUAR (LLNL) | HF model | Contrastive authorship-representation transformer (RoBERTa base), cross-domain style embeddings | Apache-2.0 | adopt for scoring/verification |
| StyleDistance / mStyleDistance | HF model | Content-independent style embeddings trained on synthetic parallel style-controlled text, 40 style features, multilingual variant | check HF card (research release) | adopt for scoring |
| AnnaWegmann/Style-Embedding | HF model | RoBERTa + SentenceTransformer trained on conversational contrastive authorship verification (CISR) | check HF card | adopt for scoring |
| faststylometry | Python lib | Burrows' Delta authorship comparison, NLTK-based | MIT | adopt for CPU-only fallback |
| stylo (R) | R package | Long-established stylometric multivariate analysis (n-grams, PCA/cluster) | GPL-3 (CRAN default) | pattern-only (R, not our stack) |
| textstat | Python lib | Readability formulas only (Flesch-Kincaid, Gunning Fog, etc.) | MIT | adopt as one feature source |
| TextDescriptives (spaCy) | Python lib | Broader linguistic metric set as a spaCy v3 pipeline component (readability + syntactic complexity + descriptive stats) | MIT | adopt, prefer over bare textstat |
| mailbox (Python stdlib) + mbox parsers | tooling | Parse Gmail Takeout .mbox exports into per-message text | stdlib / MIT (third-party parsers) | adopt for ingest |
| RVF / AgentDB (rUv stack) | vector store | General single-file vector format with lineage/COW branching; no persona/voice-specific primitive found | see ruv licensing | adapt (as generic exemplar store, not purpose-built) |

## Local prior art

`~/.claude/skills/de-slopify/SKILL.md` and `~/.claude/skills/docs-de-slopify/SKILL.md`
are byte-for-byte identical, including their `PATTERNS.md` references. Both give
the same core prompt: read the whole text manually (explicitly "you can't do this
with regex or a script"), strip emdash overuse, "Here's why," "It's not X, it's Y,"
forced enthusiasm ("Let's dive in!"), pseudo-profound openers ("At its core..."),
and unnecessary hedges ("It's worth noting..."). They're thin: a single pattern
table with no scoring, no domain awareness, and no voice-matching step. Good
starting vocabulary, nothing to build the new skill set on top of directly —
better to fold their pattern list into a fuller reference and delete one of the
two duplicate skills.

`readme-writing`, `git-commit-craftsman`, and `skill-builder` don't touch tone at
all; they're structural/format skills (README sections, Conventional Commits
grammar, SKILL.md packaging spec). Keep them as-is; only `skill-builder`'s
packaging conventions (frontmatter limits, progressive disclosure into
`references/`) are directly reusable for how we structure the new skill.

`~/.grok/skills/deck` (the `deck` skill is a symlink into `.grok`) handles tone
only through a free-text "voice" field pulled from a repo's `DESIGN.md`, plus a
qualitative "cold-reader test" and a scoring rubric for slide effectiveness. It
has no per-person voice profile mechanism; the closest analog to what we want
would be swapping `DESIGN.md`'s voice field for a generated voice-profile file.

No skill on this machine implements per-person voice cloning from a corpus; the
gap is real and matches the team's brief.

## Public Claude Code skills and prompts

Wikipedia's [Signs of AI writing](https://en.wikipedia.org/wiki/Wikipedia:Signs_of_AI_writing)
page (CC BY-SA, ~15k words) is the most complete public taxonomy and is worth
treating as the canonical reference to build the anti-slop pattern list from.
It groups tells into content patterns (undue emphasis on "significance/legacy,"
notability padding, superficial "highlighting X" analysis, promotional
travel-guide language, vague attribution/weasel wording, the boilerplate
"Challenges"/"Future Outlook" section), language/grammar patterns (a
dated-by-era AI vocabulary list — "delve," "intricate," "tapestry," "underscore"
for 2023-mid-2024, shifting to "align with," "foster," "bolstered" by
mid-2024-2025; avoidance of plain copulas, e.g. "serves as" instead of "is";
"not just X, but Y" contrast formulas; overuse of rule-of-three lists),
structural patterns (defining a list/topic title as if it were a proper noun),
and markup/citation artifacts (leftover `oaicite`/`contentReference` tags,
broken DOIs, fabricated Wikipedia templates). It's honest about limits: human
detection accuracy sits around 57-64%, only ~90% for heavy LLM users, and
human writing is itself drifting toward these patterns, so false positives are
a real risk. This is the single best source to seed a shared `PATTERNS.md`.

Three actively maintained Claude Code skills/plugins target the same problem:

- **blader/humanizer** (MIT) — the seed project most others fork from. 25
  patterns in five groups (staging-instead-of-stating, rhythm-by-rule,
  inflation/borrowed authority, formatting-by-rule, chatbot leftovers). Its
  voice-matching feature is exactly the mechanism we want to generalize: give
  it 2-3 paragraphs and it emulates "rhythm, word choice, punctuation, and
  deliberate quirks including dashes" without importing the sample's facts.
  Ships as `SKILL.md` + `agents/` + `scripts/` + CI workflow.
- **jooray/humanizer** (MIT) — installs as a Claude Code plugin
  (`/plugin marketplace add jooray/humanizer`), targets the Wikipedia list
  directly (54 patterns), and states the ingest contract cleanly: "a sample
  teaches style only... cadence, vocabulary and habitual turns of phrase carry
  over, while the sample's facts, names, dates, examples and images do not."
  That sentence is close to the exact separation-of-concerns our voice-profile
  template needs (style layer vs. content layer).
- **rlorenzo/humanize** (MIT, forked from blader, patterns from blader) — the
  most production-shaped of the three: an always-on rule file loaded every
  session, a `/humanize` slash command, a PostToolUse hook that fires
  automatically after Claude writes prose, a `humanizer-reviewer` subagent for
  deep review, a CLI scorer (`humanize_score.py`, weighted pattern detection),
  and a separate statistical "burstiness" checker for uniform sentence-length
  patterns. It extends blader's 25 patterns to 34 (adding academic/docs/blog/
  commit-specific ones) and adds **domain profiles** — e.g. em-dashes are fine
  in scientific prose but flagged in blog posts, passive voice is correct in
  IMRaD methods sections but flagged elsewhere. This six-piece architecture
  (skill + scorer + hook + subagent + rule + domain profile) is the strongest
  model for how our own skill set should be structured, and the burstiness
  checker is a genuinely useful idea (LLM output tends toward uniform sentence
  length; a statistical check catches what pattern-matching misses).

Brand-voice-specific skills exist too. **rampstackco/claude-skills**
`brand-voice` documents voice as four layers (3-5 personality attributes each
paired with a rejection, e.g. "confident, not arrogant"; tone shifts across
contexts like onboarding vs. error messages; vocabulary/grammar preferences;
and a paired example library of off-voice vs. on-voice copy), fills the layers
from 5-10 writing samples plus 3-5 reference brands, and stress-tests new copy
by scoring it 3+ on each attribute. **apreshill/straight-talk** splits into
codex-voice (technical long-form), brand-voice (corporate), and social-voice
(compressed, anti-pattern-heavy for social platforms), explicitly billing
itself as content-agnostic: "teach Claude how to write in a particular voice,
not what to write about" — the same framing as jooray's ingest contract. No
license file was found in that repo; treat as pattern-only unless a license is
confirmed. `anthropics/skills` (the official repo, MIT) has enterprise/creative/
document skills but no dedicated voice-clone or anti-slop skill as of this
survey — nothing to reuse directly there beyond its packaging conventions.

## Products that clone a person's or brand's writing voice

**Jasper Brand Voice**: ingest is ≤8 text samples, files, or crawled URLs;
output is a generated "voice excerpt" summarizing perceived tone, vocabulary,
and sentence structure, applied automatically to every subsequent generation
in the workspace. **Copy.ai** splits the same idea into two products: Brand
Voice (style) and Infobase (facts/company knowledge), referenced separately
via `@` tags in prompts — a clean structural precedent for keeping the style
layer and the content/fact layer apart, echoing jooray's ingest framing.
**Delphi.ai** goes furthest: it ingests a person's entire public corpus (books,
podcasts, video, courses) plus a short voice-audio sample, and builds a
conversational agent, with explicit consent requirements and a promise that
answers are grounded in and cite the uploaded material rather than a generic
model. **Lex** takes the opposite philosophy from all of the above — it
explicitly refuses to "replace" a writer's voice, offering "Style Guides" that
train continuation-matching and persistent "Knowledge Bases," positioning
itself for writers who want a thinking partner, not a ghostwriter. **Gmail
Help me write** personalizes tone/style automatically by mining the user's own
sent mail and Drive files with no explicit profile-building step at all — the
zero-effort end of the spectrum, worth noting as a UX option (implicit
corpus-mining vs. explicit profile template) even though it's not directly
reusable.

Pattern across all of them: every serious product separates **style** (how)
from **content/facts** (what), and every one requires multiple real writing
samples rather than a one-line tone description — a description alone
consistently under-performs actual exemplars in reviews and vendor docs.

## Open models and libraries for the ingest pipeline

Three style-embedding models are worth evaluating for scoring how close a
draft is to a target voice, independent of subject matter:

- **LUAR** (LLNL, Apache-2.0, [github.com/llnl/LUAR](https://github.com/llnl/LUAR)) —
  contrastive authorship representation on a RoBERTa base (paraphrase-
  distilroberta), trained on ~1.07M Reddit authors (the "Million User Dataset"),
  with demonstrated zero-shot transfer across Reddit/Amazon/fanfiction domains.
  Available on HF as `rrivera1849/LUAR-MUD` and a newer multi-vector Qwen3-0.6B
  variant. Best fit for **authorship verification** (does this draft match
  author X's known style) rather than generation guidance.
- **StyleDistance / mStyleDistance** (HF org `StyleDistance`, research release —
  confirm license on the model card before shipping) — trained on synthetic
  parallel examples engineered to control 40 named style features
  independent of topic, explicitly built to reduce the content-leakage problem
  that plagues naive style embeddings. The multilingual variant covers nine
  languages. This is the more interpretable of the two for our purposes since
  the 40 features are named and could double as a checklist.
- **AnnaWegmann/Style-Embedding** (HF, RoBERTa + SentenceTransformer,
  research release) — trained via contrastive authorship verification on
  conversational data specifically (CISR: content-independent style
  representations), found to outperform domain-only or uncontrolled training
  for separating style from content. Good fit if the target corpus is mostly
  conversational (chat, email, Slack) rather than long-form essays.

For a **CPU-only fallback that needs no model download**, stylometry libraries
give a cheaper, fully interpretable baseline:

- **faststylometry** (MIT, `pip install faststylometry`,
  [github.com/fastdatascience/faststylometry](https://github.com/fastdatascience/faststylometry)) —
  implements Burrows' Delta, the classic forensic-stylometry authorship-
  attribution algorithm, on top of NLTK. Gives a same-author probability
  between a draft and a reference corpus with no embedding model at all.
- **stylo** (R, GPL-3, CRAN + [github.com/computationalstylistics/stylo](https://github.com/computationalstylistics/stylo)) —
  the long-standing academic reference implementation (n-gram + PCA/cluster
  analysis, language-independent by design). Not in our stack (R), but its
  published methodology is worth mining if faststylometry's coverage proves
  too thin.
- **textstat** (MIT, `pip install textstat`) — pure readability formulas
  (Flesch-Kincaid, Gunning Fog, SMOG, Coleman-Liau, ARI). Cheap, but narrow.
- **TextDescriptives** (MIT, spaCy v3 pipeline component,
  [github.com/HLasse/TextDescriptives](https://github.com/HLasse/TextDescriptives)) —
  strictly broader than textstat: descriptive stats (sentence/word counts),
  readability indices, and syntactic complexity, all as spaCy pipeline
  extensions, so it composes with any spaCy-based feature extraction we
  already do elsewhere in the ingest pipeline. Prefer this over bare textstat.

Corpus tooling: Gmail Takeout exports mbox; Python's stdlib `mailbox` module
handles the container format directly (no extra dependency), and several
small MIT-licensed community scripts (e.g. `Gmail-MBOX-email-parser`,
`gmail-mboxsplitter`) add label-splitting and CSV export convenience on top.
LinkedIn, X, and Slack exports are all just structured JSON/CSV dumps once
requested from each platform's own export tool; no specialized parser is
needed beyond a small per-format extraction script, since the hard part
(getting clean per-message text with a stable timestamp/author field) is
already solved by the platform's own export format. Recommend writing one
thin per-source adapter (mbox, LinkedIn JSON, X archive JSON, Slack export
JSON, plus a generic "pasted text" adapter) that all normalize to the same
`{text, timestamp, source, thread_id}` record shape before anything touches
style embeddings or stylometry.

## The rUv stack angle

Queried `search_ruvnet` for persona/voice/style storage and for RVF/AgentDB as
an exemplar-retrieval store. The first query (broad: "persona voice style
profile storage exemplar retrieval writing") got declined by the capability
router as too ambiguous — no repo named, no hit. The narrower query
("RVF AgentDB storing corpus documents for retrieval augmented generation
exemplars") routed to the `agentdb` repo and surfaced ADR-003 (RVF format
integration for AgentDB): a single-file vector format with crash safety,
progressive indexing, lineage tracking, and copy-on-write branching, plus
three working npm packages (`@ruvector/rvf`, `@ruvector/rvf-node`, and a WASM
backend). That's a solid general-purpose vector store — good enough to hold
per-person exemplar embeddings for retrieval-augmented voice matching — but
there is **no purpose-built persona/voice primitive** anywhere in the rUv
stack as far as this search reached (evidence was explicitly flagged "thin,"
so absence here isn't proof of absence in the wider ecosystem, just that nothing
surfaced under two honest attempts). Verdict: adapt RVF/AgentDB as a generic
exemplar vector store if we want retrieval-augmented style matching at scale;
don't expect or wait for a rUv-native voice-profile abstraction to already
exist.

## Recommended toolchain for the ingest pipeline

1. **Adapters** (per source, normalize to `{text, timestamp, source,
   thread_id}`): stdlib `mailbox` for Gmail Takeout mbox; small JSON adapters
   for LinkedIn/X/Slack exports; a generic "paste text" adapter for anything
   without a platform export.
2. **Feature extraction** (CPU-only, no model download — the default path):
   TextDescriptives (spaCy pipeline: readability + syntactic complexity +
   descriptive stats) plus faststylometry's Burrows' Delta for same-author
   scoring against the target corpus. This alone can populate a voice-profile
   template's quantitative fields (avg sentence length, lexical diversity,
   punctuation habits, hedging frequency) with zero external API calls or
   multi-GB downloads.
3. **Optional upgrade path** (when CPU-only isn't discriminating enough):
   AnnaWegmann/Style-Embedding for conversational corpora (email, chat, Slack)
   or StyleDistance/mStyleDistance for more general/multilingual corpora, used
   purely as a same-author verification score on draft output, not as a
   generation mechanism — generation still comes from prompting Claude with
   real exemplars plus the anti-slop pass, per the "sample teaches style, not
   facts" principle every reviewed product and skill converges on.
4. **Anti-slop layer**: adopt rlorenzo/humanize's six-piece architecture
   (always-on rule, slash command, PostToolUse hook, review subagent, weighted
   scorer, burstiness checker) as the shape for our own skill, seeded with the
   merged pattern list from Wikipedia's Signs of AI Writing plus blader's 25 +
   rlorenzo's +9 domain-specific patterns, with domain profiles for at least
   docs/commit/chat/deck since those are the four surfaces named in the brief.
5. **Exemplar retrieval** (optional, for scale): RVF/AgentDB as the vector
   store for a person's exemplar corpus, retrieved at generation time to
   ground the voice in real passages rather than a static summary — but treat
   this as an adaptation of a general primitive, not a ready-made feature.

Sources visited: [Wikipedia: Signs of AI writing](https://en.wikipedia.org/wiki/Wikipedia:Signs_of_AI_writing),
[blader/humanizer](https://github.com/blader/humanizer),
[jooray/humanizer](https://github.com/jooray/humanizer),
[rlorenzo/humanize](https://github.com/rlorenzo/humanize),
[rampstackco/claude-skills brand-voice](https://github.com/rampstackco/claude-skills/blob/main/skills/brand-voice/SKILL.md),
[apreshill/straight-talk](https://github.com/apreshill/straight-talk),
[anthropics/skills](https://github.com/anthropics/skills),
[Jasper Brand Voice](https://help.jasper.ai/hc/en-us/articles/18618693085339-Brand-Voice),
[Copy.ai Brand Voice](https://www.copy.ai/blog/brand-voice), [Copy.ai Infobase](https://www.copy.ai/features/infobase),
[Delphi customer story (AssemblyAI)](https://www.assemblyai.com/customers/delphi-customer-story),
[Lex review (Agent Finder)](https://agent-finder.co/reviews/lex),
[Gmail Help me write update](https://workspaceupdates.googleblog.com/2026/05/improvements-to-help-me-write-in-gmail.html),
[LLNL/LUAR](https://github.com/LLNL/LUAR), [LUAR license](https://github.com/LLNL/LUAR/blob/main/LICENSE),
[StyleDistance paper](https://arxiv.org/pdf/2410.12757), [StyleDistance HF](https://huggingface.co/StyleDistance/styledistance),
[mStyleDistance paper](https://arxiv.org/abs/2502.15168),
[AnnaWegmann/Style-Embedding HF](https://huggingface.co/AnnaWegmann/Style-Embedding),
[faststylometry](https://github.com/fastdatascience/faststylometry),
[stylo (R)](https://github.com/computationalstylistics/stylo),
[textstat](https://github.com/textstat/textstat), [TextDescriptives](https://github.com/HLasse/TextDescriptives).
