# Manifest contract notes

Detail that doesn't change turn-to-turn decision-making but matters when you're
authoring a manifest or reviewing one a producer handed you.

## Why three separate "owner" fields exist

A card conflates three distinct people if it only has one owner field:

- **asked_by** — who wanted this, in their own words. Needed to confirm scope and to
  close the loop later ("we built what you asked for").
- **turn_owner** — who owes the *next move*, right now. Changes over the life of the
  card. `unclear` is an acceptable value; a guess is not.
- **filed_by** — the producer/agent that got it onto the board. Provenance, not
  authority.

Conflating them is the single most common defect in a hand-written manifest — a card that
names one person and lets the reader assume they cover all three.

## Evidence tiers

1. **Directly stated** — a verbatim quote from the source, with a locator (timestamp,
   page, line).
2. **Implied** — a real need nobody said out loud. Allowed, but mark it `implied`, still
   quote the passage it was inferred from, and add an Open Question asking for
   confirmation.
3. **Fabricated** — not allowed under any framing. A fabricated card is indistinguishable
   from a real one on the board face, and someone will build it.

## Update vs. append

An "update" action should **replace** the card body with the manifest's new body, never
append a second body underneath the old one. If the project's board schema supports
history (an edit/version log), the *prior* body belongs there, not inline in the card.
Appending produces doubled cards that grow without bound and eventually need a dedicated
repair pass — treat any board adapter that only offers an append-style write as needing a
"replace with recorded history" wrapper before this skill's apply step uses it.

## Goal / outcome membership

Where the board schema has an explicit goal/outcome object distinct from ticket status,
keep the two vocabularies disjoint: a ticket is "done"; a goal is "met" — these are
different claims. If the adapter has no write path for goal membership yet, state the
intended membership on the card in prose and say plainly that the membership could not be
recorded in the object. Both halves of that sentence matter.
