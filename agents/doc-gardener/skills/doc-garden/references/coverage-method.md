# Documentation coverage — the method

Borrow test coverage's shape: a hard denominator first, a semantic index second, and
never blend either into one health score.

## Step 1 — enumerate the units, don't embed them

Pull the unit list from the repo itself: exported symbols, API routes, migrations,
decision-record decisions and their open questions, config flags, scripts, CI gates.
Countable, reproducible, grep-checkable. This is a *presence* measure, not a *quality*
measure — say so wherever the number is reported, because someone will try to drive it
up, and driving it up is far easier than documenting anything.

## Step 2 — binary, per-unit, with first-class exclusion

For each unit: does any document *name* it? Yes/no, per unit, not aggregated. A unit can
be marked "deliberately undocumented, with a reason" — without this, every internal
helper reads as a permanent gap and the report becomes noise nobody opens.

## Step 3 — only then, the semantic index

If the project has a semantic/vector index over docs and source, use it to find what grep
can't: a document discussing a concept without naming the symbol (covered in substance,
uncovered by grep), or one naming a symbol while describing something else. A semantic
gap is a **candidate**, never a finding — it enters the queue with its similarity score
and is verified by reading before it's written up. Absence in the index is not absence in
the world.

## Step 4 — compare against test coverage only where the population is identical

If the project also tracks test coverage over the same unit list, three separate
questions become answerable: Tested? Documented? Decided (has a governing decision
record)? The useful cells are tested-but-undocumented (cheapest documentation there is —
draft from the test) and documented-but-untested (a claim with no enforcement). Never
blend the three into one score.

## Phase it

Phase 0 needs no index and no coverage report at all — the queue stands alone. A
confident, wrong coverage report is worse than none, because it retires the question.
Ship the framing with the first report: this measures presence, not quality; it is
expected to look bad initially; driving it up is easier than documenting or testing
anything.
