# Academic and technical research on writing style, authorship, and LLM idiolect

Scope: 2023-2026 work on stylometry, neural style representations, LLM
personalization, AI-text detection markers, and evaluation of voice fidelity,
plus canonical pre-LLM foundations. Collected to ground the design of a
"communication style" skill set: (1) a checklist for de-machining agent
prose, (2) a template for writing as a specific person from a corpus.

Every citation below was returned by a live search in September 2026 and
checked against at least the abstract or the paper itself. Where I could not
verify a claim beyond a secondary summary, it's marked "unverified" or
"secondary source only." Arxiv IDs with dates in 2026 (e.g. 2608.xxxx,
2609.xxxx) are recent preprints, not typos — arxiv's ID scheme prefixes by
year and month.

## Executive summary

1. **Function words, not content words, carry authorial identity.**
   Burrows' Delta and its descendants attribute authorship using frequency
   profiles of the ~50-500 most common words (articles, prepositions,
   pronouns, conjunctions) because they're used subconsciously and are
   topic-independent. This is the single most load-bearing fact for
   building a style profile: don't ask a corpus what it talks about, ask it
   how it glues sentences together.

2. **Style and content are entangled by default in embeddings, and papers
   spend real effort disentangling them.** Wegmann et al. (2022) showed
   that authorship-verification-trained embeddings pick up *topic*, not
   just *style*, because people write about recurring topics. Their fix —
   contrastive triplets across same-author/different-topic text — is the
   ancestor of every "content-independent style embedding" since.

3. **The best current open style embeddings are LUAR (2021) and
   StyleDistance (2024).** LUAR (Rivera-Soto et al., EMNLP 2021) is a
   contrastive transformer trained on Reddit for authorship verification;
   StyleDistance (Patel et al., NAACL 2025) improves content-independence
   by training on LLM-synthesized paraphrase pairs that vary exactly one of
   40 named style features at a time. Both have open weights on Hugging
   Face and are usable today as an automatic "does this draft sound like
   author X" scorer.

4. **Persona descriptions beat few-shot exemplars for style/writing tasks;
   the reverse holds for calibrated numeric tasks.** A 2026 negative-result
   paper found retrieval-augmented few-shot prompting dominates on rating
   prediction, but persona summaries are comparable or better for
   categorization and writing generation. Practical reading for a style
   skill: a filled-in descriptive profile (voice traits, habits, don'ts) is
   a reasonable default; a handful of verbatim exemplars still helps and a
   hybrid (profile + 2-4 exemplars) is worth trying.

5. **LLM text has its own idiolect, sometimes called a "chatolect," and it
   is model-specific enough to fingerprint.** "Idiosyncrasies in Large
   Language Models" (Sun et al., ICML 2025) classified which of five
   frontier LLMs wrote a text at 97.1% accuracy using nothing but
   word-level distribution features, and the signal survives translation,
   summarization, and rewriting by another model — meaning the tell is
   deep, not surface-level punctuation.

6. **The most reproducible, corpus-scale marker of LLM authorship is a
   specific list of ~21 overused words**, not em-dashes. Kobak, Gonzalez-
   Marquez & Horvát (2024/2025) quantified a step-change in PubMed abstract
   vocabulary after ChatGPT's release: "delve" +28x, "showcasing" +10x,
   "underscores" +14x, plus intricate, meticulous, tapestry, realm,
   pivotal, testament, and others, dominated by verbs and adjectives (66%
   + 14%). This is a directly actionable "don't use these words" list, far
   more evidence-backed than folk claims about em-dashes.

7. **The em-dash claim is popular but weakly evidenced and confounded.**
   Multiple 2025-2026 sources (including a paper on autistic writing being
   misclassified as AI-generated) argue punctuation-based tells are noisy
   and the "single biggest AI tell" that survives prompt rewrites is
   actually **uniform sentence-length cadence** (low burstiness), not
   em-dash frequency. Treat em-dash rate as a weak secondary signal, sentence-
   length variance (burstiness) as a stronger one.

8. **Burstiness and perplexity are real, measurable, and don't fully close
   even when style transfer "succeeds."** A 2026 imitation study found
   human essays average perplexity 29.5 vs. 15.2 for LLM outputs matched
   for topic and even style-conditioned — i.e., an LLM can adopt someone's
   phrasing and still be more locally predictable than that person, an
   important caveat for any fidelity metric that stops at surface style
   match.

9. **Authorship-embedding similarity (e.g., LUAR) is a stronger fidelity
   check than it might seem, and current LLM personalization fails it
   badly.** PersonalBench (2026) found LLM-personalized generations sit at
   LUAR similarity 0.48-0.51 against the target author, well below the
   human-to-human cross-author floor of 0.626 — the generating model's own
   fingerprint dominates over the persona it's asked to emulate. This is
   the empirical case for treating "sounds human" and "sounds like this
   specific person" as two different, both-necessary checks, and for not
   trusting an LLM-as-judge alone (see #10).

10. **LLM-as-judge is unreliable for exactly the kind of subtle,
    self-referential judgment this skill needs.** Survey and benchmark work
    from 2024-2026 documents position bias, verbosity bias, and
    self-preference bias (judges favor their own family's outputs) at rates
    that push frontier-model error past 50% on adversarial bias tests.
    Anything scoring "does this sound like person X" should triangulate
    LLM-judge output against an embedding-similarity score and, where it
    matters, a human rater — never rely on LLM-judge alone.

11. **Exemplar-conditioned small models can already beat prompted GPT-4 at
    style transfer.** TinyStyler (Horvitz et al., EMNLP Findings 2024) — an
    800M-parameter model conditioned on an authorship embedding — outperforms
    GPT-4 at authorship style transfer while being far cheaper, evidence
    that a compact style representation plus a small generator can be a
    stronger primitive than "put examples in the prompt and hope."

12. **Deanonymization risk from stylometry is real at surprisingly small
    text volumes,** and it cuts both ways: it's the same technology needed
    to verify "did this draft actually sound like me" and to *unmask*
    someone who didn't consent to be identified. Any per-person voice
    profile built from a real corpus is a re-identification liability if
    leaked or misused, and the skill's design needs explicit consent
    handling, not just a technical capability.

## 1. Stylometry and authorship attribution

Classical stylometry attributes or verifies authorship using
statistical/handcrafted features rather than learned representations, and it
predates neural methods by decades. It remains the reference point for
"what actually identifies a writer," because it's interpretable in a way
embeddings aren't.

**Burrows' Delta.** Burrows (2002) proposed measuring stylistic distance as
the mean absolute z-score difference between a candidate text's and a
reference set's frequency profile over the most frequent words in a corpus
(function words dominate this list: articles, prepositions, pronouns,
conjunctions). A text is attributed to whichever candidate author minimizes
this distance. Its durability — still being re-derived and re-validated
against machine translation, medieval Chinese poetry, and inductive
authorship studies in 2024-2026 — comes from the same property that makes it
useful for a writing skill: function words are frequent enough to get stable
statistics from short samples, and they're chosen subconsciously, so they
resist deliberate mimicry better than content words do.
(Rank-Turbulence Delta, arXiv:2604.19499, is a 2026 reformulation aimed at
better interpretability of *which* words drive the distance — useful if the
skill ever needs to explain *why* two texts are judged similar/dissimilar,
not just output a score.)

**Writeprints (Abbasi & Chen, 2008).** A widely used 220-feature set spanning
lexical (word/character counts, digit and case percentages), syntactic
(function-word frequencies, POS n-grams), structural (sentence-length
variability), and idiosyncratic (misspelling rate) features, computed per
author rather than per author-group for scalability. This is close to a
ready-made feature checklist for a style-profile template: it's essentially
the pre-neural version of what a "voice profile" should capture, minus
discourse-level and register features that later work added.

**Survey grounding.** "On the State of the Art in Authorship Attribution and
Authorship Verification" (arXiv:2209.06869) is a useful, if slightly dated
(2022), map of the field's methods and open problems before the LLM-writing
era complicated things further; treat as a foundations reference, not
current SOTA.

**Function words as the core signal — repeated finding.** Every source
above converges on the same claim: function words and punctuation habits
are stronger, more stable authorship signals than topical vocabulary,
because they're used below conscious awareness and are decoupled from what
the person happens to be writing about. This is the single design principle
most worth carrying into a voice-profile template: profile fields should
bias toward function-word tendencies, sentence-length distribution, and
punctuation/discourse-marker habits over "topics this person writes about."

## 2. Neural style representations

**LUAR — Learning Universal Authorship Representations** (Rivera-Soto,
Miano, Ordonez, Chen, Khan, Bishop & Andrews; EMNLP 2021,
[aclanthology.org/2021.emnlp-main.70](https://aclanthology.org/2021.emnlp-main.70/)).
A transformer trained via contrastive learning on millions of Reddit posts
to produce author-discriminative embeddings; cosine similarity between two
texts' LUAR embeddings estimates same-authorship likelihood. The paper's own
contribution beyond the embedding is a cross-domain transfer study (Amazon
reviews, fanfiction, Reddit) showing transfer is uneven across domains.
Code: [github.com/LLNL/LUAR](https://github.com/llnl/luar). This is the
most-cited "measure whether a draft sounds like author X" tool in the 2024-
2026 personalization literature (used as ground truth in PersonalBench,
among others) — it is the closest thing the field has to a standard
authorship-similarity metric, and it's open-weight.

**StyleDistance** (Patel, Zhu, Qiu, Horvitz, Apidianaki, McKeown &
Callison-Burch; NAACL 2025, arXiv:2410.12757,
[huggingface.co/StyleDistance/styledistance](https://huggingface.co/StyleDistance/styledistance)).
Trains content-independent style embeddings from LLM-synthesized parallel
paraphrase pairs, each pair varying exactly one of 40 named style features
(formality, hedging, punctuation habits, etc.) while holding content fixed —
directly targeting the topic/style entanglement problem Wegmann et al.
identified. Comes with SynthSTEL, a content-controlled evaluation set along
those 40 axes. A follow-up, mStyleDistance (arXiv:2502.15168), extends this
multilingually. This is currently the strongest documented open model for
"style similarity independent of what's being talked about," and its 40
named axes are a strong candidate feature list for a style-profile schema.

**Wegmann et al. and STEL** ("Same Author or Just Same Topic? Towards
Content-Independent Style Representations," Wegmann, Schraagen & Nguyen,
RepL4NLP workshop 2022, arXiv:2204.04907,
[aclanthology.org/2022.repl4nlp-1.26](https://aclanthology.org/2022.repl4nlp-1.26/);
model: [huggingface.co/AnnaWegmann/Style-Embedding](https://huggingface.co/AnnaWegmann/Style-Embedding)).
Diagnosed the core problem later work fixes: embeddings trained on
authorship-verification objectives absorb topic information because authors
have recurring subjects. Their fix trains on same-author/different-topic
contrastive triplets. STEL (and the harder variant STEL-or-Content) is the
resulting benchmark: given an anchor sentence and two candidates, pick the
one that shares the anchor's *style* rather than its *content* or *topic*.
A 2026 successor, STEB (Style Text Embedding Benchmark, arXiv:2606.31741),
broadens this evaluation; iBERT (arXiv:2510.09882) pursues interpretable
style embeddings via "sense decomposition" as a more explainable
alternative to black-box contrastive embeddings — worth a look if the skill
ever needs to explain *what specifically* differs between a draft and a
target voice, not just flag that it differs.

**Design takeaway for neural embeddings.** For a "does this draft sound like
person X" scorer: use LUAR or StyleDistance to get a cosine-similarity
number, but calibrate it against known same-author and cross-author
baselines for the corpus size available (see PersonalBench numbers, section
5) rather than treating any single threshold as universal — absolute
similarity scores are not comparable across corpora sizes or genres without
a reference floor/ceiling.

## 3. LLM personalization and persona-conditioned writing

**LaMP** (Salemi et al., "When Large Language Models Meet Personalization,"
ACL 2024, [aclanthology.org/2024.acl-long.399](https://aclanthology.org/2024.acl-long.399.pdf)).
A benchmark suite spanning classification, regression, and generation tasks
personalized per-user, with both a user-based split (generalize to new
users) and a time-based split (generalize to a user's future behavior). Its
generation tasks (e.g., personalized email/review completion) are the
closest existing benchmark shape to "write like this person."

**LongLaMP** (Kumar et al., 2024, arXiv:2407.11016). Extends the LaMP
framing to long-form generation (reviews, blog posts, emails) and evaluates
retrieval-augmented generation: retrieve a user's relevant past writing and
condition the LLM on it directly, rather than on a distilled persona
summary. This is the direct academic analogue of "give the model exemplars
from the target person's corpus," as opposed to a profile.

**Persona description vs. exemplar retrieval — which wins.** A 2026
negative-result paper ("Prompt-Space Meta-Learning Does Not Transfer Across
Users," arXiv:2609.01615) and adjacent work found the effect is
task-dependent: persona summaries dominate or tie for writing-style and
categorization tasks, but retrieval-augmented few-shot exemplars dominate
decisively for tasks needing precise numeric calibration (e.g., rating
prediction), because distilling a user into a natural-language persona
throws away fine-grained calibration signal that raw examples preserve. A
hybrid of both was flagged as promising but under-tested. Practical
implication for the skill: a filled-in descriptive voice profile is the
right default artifact for *writing* tasks (this skill's actual target),
but don't assume the same holds if the skill set is ever extended to
numeric/rating-style tasks.

**STYLL** (Patel et al., building on "Low-Resource Authorship Style
Transfer," arXiv:2212.08986). An entirely in-context, no-finetuning method:
prompt an LLM with a handful of texts written by the target author plus
style descriptors, and ask it to rewrite new content in that voice. Its
selling point over STRAP-style methods is that it needs no per-author
fine-tuning, which matters for a skill meant to work from "a few emails a
user pastes in," not a curated fine-tuning corpus.

**TinyStyler** (Horvitz et al., EMNLP Findings 2024, arXiv:2406.15586,
code: [github.com/zacharyhorvitz/TinyStyler](https://github.com/zacharyhorvitz/TinyStyler),
weights: [huggingface.co/tinystyler/tinystyler](https://huggingface.co/tinystyler/tinystyler)).
An 800M-parameter model trained to reconstruct paraphrases conditioned on an
authorship embedding of the *original* text; at inference, swapping in a
*target* author's embedding performs few-shot style transfer, and
interpolating between source and target embeddings gives a tunable
style-strength vs. meaning-preservation knob. Reported to outperform
prompted GPT-4 on authorship style transfer despite being tiny and requiring
no per-author fine-tuning. This is strong evidence that a good style
*embedding* plus a lightweight conditioned generator can beat "stuff
exemplars into a big model's prompt" — worth citing as the argument for
building (or at least measuring against) an embedding-based fidelity loop,
even if the skill's actual generator stays a general-purpose LLM prompted
with a profile.

**PersonalBench / the authorship gap** (Sawant, arXiv:2608.19746, building on
"Theory-Grounded Evaluation Exposes the Authorship Gap in LLM
Personalization," arXiv:2604.26460). Evaluates inference-time
personalization (not fine-tuning) across 50 authors and two model families
using three independent lenses: LUAR similarity, LLM-as-judge, and automated
stylometrics. Headline finding: personalization methods do differentiate
authors from each other (LUAR AUC 0.918 for telling authors apart), but none
of them cross the human/LLM boundary — generated text similarity to the
*real* target author sits at 0.484-0.508, below the human cross-author
floor of 0.626 (with a human same-author ceiling of 0.756). The paper's
interpretation: the generating LLM's own idiolect dominates over whatever
persona conditioning it's given. This is the strongest available evidence
that current techniques (whichever an agent skill would realistically use)
have a hard ceiling on fidelity, and that "the model's own voice bleeds
through" is not a hypothesis but a measured, named phenomenon.

## 4. AI-text detection features and "LLM idiolect"

**Idiosyncrasies in Large Language Models** (Sun, Yin et al., ICML 2025,
arXiv:2502.12150, [openreview.net/forum?id=FCZ3jVzmTZ](https://openreview.net/forum?id=FCZ3jVzmTZ)).
Frames "which LLM wrote this" as a classification task and fine-tunes text
embedding models to solve it, reaching 97.1% accuracy distinguishing
ChatGPT, Claude, Grok, Gemini, and DeepSeek on held-out text. Two findings
matter for a de-machining checklist: (a) the idiosyncrasies are rooted in
word-level distributions — i.e., they're the same *kind* of signal Burrows'
Delta looks for, just model-specific rather than person-specific — and (b)
the signal survives translation, summarization, and rewriting by another
LLM, meaning it's baked into word choice at a level that simple
post-editing (swap a few flagged words) won't fully remove.

**Delve and friends — the most citable overused-word list.** "Why Does
ChatGPT 'Delve' So Much?" (arXiv:2412.11385) and Kobak, Gonzalez-Marquez &
Horvát's "Delving into ChatGPT usage in academic writing through excess
vocabulary" (arXiv:2406.07016, later versions covering "LLM-assisted writing
in biomedical publications"; related Nature Human Behaviour piece by Liang
et al., 2025) together give the field's best-quantified word list. Kobak et
al. analyzed 14M+ PubMed abstracts (2010-2024) and found a step change after
ChatGPT's release concentrated in 21 focal words, dominated by verbs (66%)
and adjectives (14%): **delve** (~28x), **showcasing** (~10x),
**underscores** (~14x), plus intricate, commendable, meticulous, surpass,
elevate, foster, tapestry, realm, navigate, landscape, pivotal, resonate,
testament, compelling, paramount, crucial, unwavering, alignment. A useful
secondary cluster called out elsewhere (mosaic, ecosystem, symphony,
labyrinth, beacon, cornerstone, bedrock, cacophony, kaleidoscope, odyssey)
recurs across informal write-ups but is less rigorously quantified —
treat that second list as plausible-but-not-peer-reviewed. Estimated
prevalence: up to 10% of 2024 PubMed abstracts and up to ~17.5% of computer
science preprints show detectable LLM-modified text by this method — this
is corpus-level evidence the markers are common enough to matter, not a
curiosity.

**Structural/behavioral markers (weaker evidence, still worth listing).**
Recurring informal and semi-formal claims across 2025-2026 sources:
uniform sentence-length cadence (low "burstiness") as possibly the most
durable tell because it survives prompt rewrites better than any single
word; low perplexity (locally predictable phrasing) as a companion signal;
heavy hedging and over-explanation (attributed particularly to Claude-style
output in informal comparisons); list-heavy/structured phrasing (attributed
to Gemini-style output); and em-dash overuse, which is popular in public
discourse but explicitly disputed by more careful 2025-2026 sources — one
paper on misclassified autistic writing (arXiv:2607.14729) argues
punctuation/rhythm-based detectors produce false positives against
legitimately different but human writing styles, and multiple
"how-to-geek"-tier and Duey/McGill pieces converge on "em-dash discourse is
a distraction, cadence uniformity is the real tell." **Recommendation for
the skill: treat burstiness (sentence-length variance) and the specific
Kobak word list as high-confidence checklist items; treat em-dash rate,
hedging density, and list-density as lower-confidence secondary flags, and
never gate solely on punctuation.**

**Watermarking and adversarial evasion — explicitly out of scope.** Papers
like "Large Language Models can be Guided to Evade AI-Generated Text
Detection" (arXiv:2305.10847) exist but describe *detector evasion*
techniques; per this project's scope, the goal here is authentic de-slop
writing and legitimate voice-matching, not evading detection, so these are
flagged as read-and-exclude rather than a source of design patterns.

## 5. Evaluation: measuring voice fidelity

Three broad methodologies recur, and the literature is fairly consistent
that no single one suffices:

**Human evaluation.** "How Well Do LLMs Imitate Human Writing Style?"
(arXiv:2509.24930) evaluated five LLMs (Llama, Qwen, Mixtral families) across
zero/one/few-shot and completion prompting, and found prompting *strategy*
affects fidelity more than model size — a directly actionable finding for
a skill (i.e., how the profile/exemplars are presented in the prompt matters
more than swapping in a bigger model). It also reports the perplexity gap
noted in finding #8 (human 29.5 vs. LLM 15.2 perplexity even under style
match) — a caution that "sounds like them" and "reads with human-level
unpredictability" are separable and both worth checking. A companion study,
"Catch Me If You Can? Not Yet: LLMs Still Struggle to Imitate the Implicit
Writing Styles of Everyday Authors" (arXiv:2509.14543), ran ~40,000
generations per model across news, email, forum, and blog domains from 400+
real authors, finding LLMs approximate structured-register styles (news,
email) far better than informal registers (blogs, forums) — a useful prior
for setting expectations by document type. A separate protocol (arXiv
2601.18353, fine-tuned book-length writing) used blind Prolific-recruited
human judges rating short excerpts against a named target author's style —
a workable template for a lightweight human-eval protocol if the skill ever
needs one: blind pairwise or Likert rating by people unfamiliar with which
sample is real vs. generated, scored on stylistic fidelity separately from
writing quality.

**Embedding/stylometric similarity as automated proxy.** LUAR cosine
similarity and stylometric feature-distance (Burrows-Delta-style) are the
two automated proxies used across PersonalBench and related papers, always
calibrated against a same-author ceiling and cross-author floor computed
from real human data in the same corpus — an important methodological
detail: a raw similarity number is meaningless without those two reference
points.

**LLM-as-judge — use with real caution.** The 2024-2026 LLM-as-judge
literature (survey: arXiv:2411.15594; bias study: arXiv:2608.18091;
construct-validity critique: arXiv:2608.24419; benchmark:
"Judging the Judges," arXiv:2604.23178) documents position bias, verbosity
bias, and self-preference/family bias (a judge favoring output from its own
model family) severe enough that frontier judges fail more than half of
adversarial bias-test batteries (per the "JudgeBiasBench"-style claim
surfaced in these searches — this specific number is a secondary-source
citation, not independently re-derived here, and should be treated as
indicative rather than exact). Simple formatting changes were also shown to
break judge consistency even among models that pass plain accuracy checks.
**Design implication: never rely on a single LLM-as-judge call to certify
"this sounds like person X."** At minimum, triangulate an LLM-judge verdict
against an embedding-similarity score computed the same way PersonalBench
does (with same-author/cross-author reference points from the person's own
corpus), and reserve human review for any output that will carry the
person's name publicly.

## 6. Ethics and consent

**Deanonymization is not hypothetical and doesn't need much text.**
Stylometric methods can positively identify an author from as little as a
few thousand words, and adversarial-stylometry research (Whonix's summary;
"Reproduction and Replication of an Adversarial Stylometry Experiment,"
arXiv:2208.07395; the foundational "Adversarial Stylometry: Circumventing
Authorship Recognition to Preserve Privacy and Anonymity") exists
specifically because employers, harassers, and platforms already use these
techniques to unmask anonymous writers, and *defenders* (obfuscation,
imitation of someone else's style) are an active countermeasure area. A 2026
survey, "Privacy Issues in Stylometric Methods" (MDPI, 2410-387X/6/2/17),
frames this as an open, unresolved harms area, not a solved problem.

**Two-sided risk for this specific skill.** Building a per-person voice
profile from a real corpus is, by construction, building a
re-identification tool aimed at that same person — useful for its intended
purpose (letting them or an authorized agent write as them) and dangerous if
the profile or corpus leaks, is built without consent, or is applied to
someone other than its subject (impersonation). A 2026 paper,
"Assessing Deanonymization Risks with Stylometry-Assisted LLM Agents"
(awesomepapers.io/llm-papers/papers/2602.23079 — secondary listing, original
venue not independently confirmed here, flag as **unverified primary
source**), specifically studies LLM agents as a deanonymization tool,
underscoring that the exact capability this skill wants to build
(LLM + style profile → text in someone's voice) is dual-use by design.

**Design implications:**
- Treat the profile template itself as sensitive personal data — it *is*
  a compact re-identifying fingerprint of the source person, arguably more
  so than the raw corpus, since it's distilled to exactly the durable,
  hard-to-fake markers (function-word habits, punctuation, discourse
  markers) that deanonymization relies on.
- Require explicit, recorded consent before building or using a profile of
  a real, identifiable person who isn't the operator themselves (e.g., don't
  let an agent silently build "sounds like my coworker" from their Slack
  history without them knowing).
- Default to least-retention: prefer deriving a profile once and discarding
  raw corpus text where possible, rather than keeping exemplars around
  indefinitely, especially exemplars that might contain the source person's
  PII incidental to their writing (addresses, other people's names, etc.).
- Separate "write in a *generic* de-slopped human register" (no consent
  question, this is just better writing) from "write *as* a specific named
  person" (a consent question every time) at the product level — the skill
  set's own framing already does this (goal 1 vs. goal 2), and the research
  above is the evidentiary case for keeping that split sharp rather than
  blurring it into one "make it sound human" feature.

## Design implications for the skill set

**What to extract into a profile template** (ranked by evidence strength):
1. Function-word tendencies and habitual discourse markers (Burrows-Delta
   tradition; strongest, most stable signal; captures "how they connect
   ideas" not "what they talk about").
2. Sentence-length distribution / burstiness — capture the person's actual
   range and rhythm, not just an average.
3. Punctuation habits (including but not overweighting em-dash rate).
4. Named style axes from StyleDistance's 40-feature taxonomy (formality,
   hedging, directness, etc.) as a structured checklist rather than
   inventing categories from scratch.
5. A short curated exemplar set (a handful of verbatim passages) alongside
   the descriptive profile — evidence favors persona-description-as-default
   for writing tasks, but exemplars still add signal and are cheap to
   include.
6. Explicitly *exclude* topic/content vocabulary from "voice" — that's
   subject matter, not style, and conflating the two is the exact failure
   mode Wegmann et al. identified and StyleDistance was built to fix.

**How to score fidelity** (don't rely on one method):
- Automated: LUAR or StyleDistance cosine similarity between draft and a
  held-out sample of the person's real writing, calibrated against that
  person's own same-sample ceiling and a random cross-author floor —
  never an absolute threshold.
- Checklist: scan against the Kobak 21-word list and a small burstiness
  check (sentence-length coefficient of variation) as fast, cheap,
  high-confidence de-slop gates that apply regardless of whose voice is
  targeted.
- Judgment: an LLM-as-judge pass is fine as a first filter but must not be
  the final gate for anything published under the person's name; add a
  human spot-check for high-stakes output (the same discipline PersonalBench
  and the LLM-as-judge bias literature both point to).

**Recommended open models (sizes/licenses as found):**
- LUAR — transformer-based (RoBERTa-scale), open weights,
  [github.com/LLNL/LUAR](https://github.com/llnl/luar) — best default for
  an authorship-similarity score; well-established as ground truth in later
  benchmarks.
- StyleDistance / mStyleDistance — open weights on Hugging Face
  ([huggingface.co/StyleDistance/styledistance](https://huggingface.co/StyleDistance/styledistance)) —
  best default for a *content-independent* style-similarity score, and its
  40-axis taxonomy doubles as a profile schema.
- TinyStyler — 800M parameters, open weights and code — the reference point
  if the skill set ever wants a dedicated small style-transfer generator
  instead of prompting a general-purpose LLM; not necessary for a v1 that
  just prompts with a profile, but worth citing as the ceiling to benchmark
  against.

**What "sounds human" (goal 1) and "sounds like a specific person" (goal 2)
share vs. don't:**
- Share: avoid the Kobak word list, vary sentence length, avoid uniform
  list-heavy structure, match register to context.
- Diverge: goal 2 additionally needs the person's actual function-word and
  punctuation fingerprint, which is *not* generic de-slopping — it can mean
  intentionally reintroducing a habit (e.g., a real person's genuine
  em-dash habit, or their actual sentence-fragment tendency) that goal 1
  would otherwise flag as a "tell." The profile template should let goal-2
  overrides win over goal-1 defaults explicitly, rather than silently
  averaging the two.

## Notes on verification status

Flagged inline above; summarized here for quick reference. **Unverified or
secondary-source-only:** the exact figure "frontier models fail 50%+ bias
tests" (cited from a secondary blog summarizing "JudgeBiasBench"-style
work, not independently re-derived); the arxiv listing for "Assessing
Deanonymization Risks with Stylometry-Assisted LLM Agents" (found via a
paper-aggregator site, not confirmed against arXiv directly). Everything
else cited above was checked against at least the paper's own abstract page
(arXiv, ACL Anthology, or the official Hugging Face/GitHub artifact).
