# Retraction forms — where the history goes

A corrected document reads as though it had been written correctly the first time. The
prose is rewritten, not annotated. Never leave the wrong sentence in place beside a
"corrected" note — that pattern (accretion) makes documents longer, more hedged, harder
to read, and leaves the wrong text discoverable by substring search, which reads a
corrected document as broken.

In preference order:

1. **The commit message.** The default, and where most corrections belong. Version
   control keeps every prior version, dated and attributed; no reader should have to walk
   past the mistake to reach the answer.
2. **A dated changelog section at the foot** — for documents whose status header is
   load-bearing. At the foot, once.
3. **A clearly fenced "what we used to believe" section, narrow.** Only where the
   *previous belief itself* is load-bearing, because a reader who doesn't know the old
   claim was believed, and why it was wrong, will re-derive it. Once, under its own
   heading, at a known location.
4. **Never inline, mid-argument, beside the sentence it corrects.**

## The accretion test

If the correction block is longer than the passage it corrects, the passage should have
been rewritten instead. Apply it, name the document, rewrite.

## Decision records are the one exception

A decision record's Decision section is not prose subject to this rule — rewriting it
destroys what the record exists to preserve (that a choice was made, on a date, by
someone, on an argument). Decision records supersede: an amendment section, a version
bump, the original decision untouched. Prose documents (specs, guides, references) follow
the rules above instead. Confusing the two directions — supersede-in-place applied to
prose, or rewrite-in-place applied to a decision record — is the most common way this
discipline gets blurred; hold both rules and apply each only in its own tree.
