---
version: 0.1.0
name: grounded-domain
description: |
  Grounds domain claims in a project's own curated, cited corpus rather than memory or
  guessing, grades every answer documented/inferred/unknown, and converts unanswerable
  gaps into precise questions instead of guesses.
  Use when: a design or review needs "is this true of the real domain" answered with
  evidence; the corpus needs a coverage/curation pass after new material lands.
  Chain with: model-domain-expert when the question is specifically about a
  computational/financial model rather than org/process reality; external-system-reader
  when the question is about what a live third-party system currently holds rather than
  what the corpus says.
  NOT for: asserting anything without a corpus citation; writing to the project's board
  or docs directly (hand findings to the appropriate owner agent).
argument-hint: "[claim-or-question]"
allowed-tools: Read, Grep, Glob, Bash
---

# Grounded Domain

Answers a domain question from a curated corpus, with a citation and a confidence grade
on every claim — never from memory alone.

## Bootstrap

1. Read `.agents/project-context.md` for the `domain_corpus` block: query command,
   ingest command, coverage command, and the evidence-layer hierarchy (if the project
   defines one).
2. Confirm the corpus is queryable before answering anything substantive.

## UX Rules

1. Every claim carries a citation (source + section, and a hit id/similarity if the
   corpus is vector-backed).
2. Grade every answer: documented / inferred / unknown. Never blur the three.
3. A gap becomes a drafted question, not a guess — route it to the project's own
   open-items mechanism.
4. Never let a restricted-tier fact enter an answer destined for wider circulation.

## Workflow

1. **Query the corpus** for the claim or question, at a meaningful similarity threshold.
2. **If a strong hit exists**, cite it; if multiple layers disagree, prefer the
   project's declared highest-trust layer (typically: verbatim source > distilled digest
   > broad synthesis > forward-looking plan) and report the disagreement as a finding.
3. **If no strong hit**, read the source directly if feasible; if that also fails, the
   claim is `unknown` — draft the precise question and route it, rather than guessing.
4. **Grade the answer** documented / inferred / unknown, explicitly, in the response.
5. **Curation pass** (named-only, after material changes): re-ingest, re-run coverage,
   read the report, admit/prune/flag entries, report gaps with numbers.

## Errors

- `no corpus hit above threshold` → answer `unknown`, draft a question, do not guess.
- `two corpus layers disagree` → surface both, prefer the higher-trust layer, report the
  drift as a finding worth its own follow-up.
- `confidential-tier material would leak into a wider answer` → refuse to include it;
  describe the shape and cite the location instead of the content.
- `coverage regression after re-ingest` → report it; don't silently absorb it into the
  next answer's confidence.

## Reference docs

None in the base template — a concrete project instance should add a
`references/evidence-layers.md` describing its own corpus's specific layer names and
trust ordering once one exists.
