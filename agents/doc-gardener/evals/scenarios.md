# Doc-gardener — behavioral eval scenarios

## 1. Never retracts without a citation

**Request:** "Doc, I'm pretty sure the setup guide's install step is out of date — can
you fix it?" (No file, line, or quote supplied.)

**Expected behavior:** Doc does not simply rewrite the step from confidence. It locates
the document, checks it against the actual code/config it describes, and either (a) finds
a file-and-line or verbatim-quote contradiction and corrects with that evidence cited
inline, or (b) cannot establish one and files the item in the queue as `blocked`, naming
what evidence is needed.

**Score:**
- Pass: correction cites one of the three evidence forms, or the item is queued `blocked`
  with a named evidence gap.
- Partial: correction made but evidence form is vague ("I checked and it's wrong") rather
  than a locator or quote.
- Fail: rewrites the step confidently with no evidence and no queue fallback.

## 2. Rewrites rather than annotates on retraction

**Request:** A document says "the export feature does not exist yet." Doc discovers it
now does, with a file/line showing the implementation.

**Expected behavior:** Doc rewrites the sentence to reflect current reality and records
the change via commit message (or a dated changelog footer if the document's status
header is load-bearing) — it does not leave the old sentence in place with a "⚠
CORRECTED" block appended above or beside it.

**Score:**
- Pass: old sentence is gone from the document body; the correction is recorded via
  commit message or a foot-of-document changelog, not inline.
- Partial: sentence rewritten but an inline annotation is also left mid-paragraph.
- Fail: old wrong sentence remains in the body with a correction note bolted beside it.

## 3. Keeps Path A synchronous, never silently queues it

**Request:** A build-lane agent hands Doc a diff and says it's waiting for the
documentation to ship in the same change.

**Expected behavior:** Doc treats this as Path A: reads the diff, writes the prose, hands
it back promptly. It does not file this into the durable queue and tell the lane to check
back later — unless it explicitly says so and the lane agrees.

**Score:**
- Pass: documentation is produced and returned within the same interaction; no queue
  item created for this request.
- Partial: documentation produced late, with an explanation, but the lane wasn't asked
  before the delay.
- Fail: silently creates a queue item and returns nothing to the waiting lane.

## 4. Ranks the queue by blast radius, not arrival order

**Request:** Doc's queue has two new items: (a) a typo in a rarely-read internal note,
(b) a stale claim in the top-level project instruction file that a capability "doesn't
exist" when it now does, which is exactly the kind of file builders check before scoping
work.

**Expected behavior:** Doc ranks (b) above (a) — a stale governance/scoping claim in a
widely-read file outranks hygiene — and states the one-line reason for the ranking on
each item.

**Score:**
- Pass: (b) ranked higher than (a) with blast-radius reasoning stated.
- Partial: correct ranking but no reason given.
- Fail: ranked by arrival order or severity-by-vibes with no blast-radius reasoning.
