---
version: 0.1.0
name: memory-routing
description: |
  Routes a "worth remembering" record to the correct store, dedupes against the store's
  existing entries, and acknowledges by read-back (a re-read from disk), never by a bare
  status. Wraps whatever memory-store CLI/index the project declares in
  `.agents/project-context.md`.
  Use when: "remember this", "where should this lesson go", "sweep the fallback queue",
  "is this already in the store".
  Chain with: doc-garden when the record is actually a decision or document; board-steward
  when it's actually work someone must do.
  NOT for: deciding what an outcome is (a person's call); writing confidential data about
  a third party into a shared store (refuse, route to the project's own accept flow);
  writing docs or the board directly.
argument-hint: "[manifest.json] [--apply] [--sweep]"
allowed-tools: Read, Write, Grep, Glob, Bash
---

# Memory Routing

Decides which store a "worth remembering" record belongs in, files it with four
write-time marks, and proves the filing by reading the store back from disk.

## Bootstrap

1. Read `.agents/project-context.md` for the `memory_store` block: store path, index
   file, budget, fallback path, health-check command.
2. Run the health-check command (if one exists) and report its verdict before taking new
   work.
3. Sweep the fallback path for anything staged while you were unavailable.

## UX Rules

1. Acknowledge only by read-back — three lines, from a fresh read of the store, never
   from the write call's return value.
2. State the dedupe verdict for every item: already present (cite the path) or genuinely
   new.
3. Refuse silently-empty provenance — a claim with no traceable origin is not filed.
4. Never infer `carryable: true` — it needs a person or a person-written rule behind it.

## Workflow

1. **Sweep first, always** — check the fallback directory for staged manifests, file
   each one, read it back, remove it from the fallback only after every item in it has a
   successful read-back.
2. **Classify each incoming item** by what it IS, not by where the producer thinks it
   goes: `lesson` → you file it; `decision` → delegate to the documentation agent;
   `document` / card-file entry → delegate to the documentation agent; `work` → delegate
   to the steward; `confidential third-party fact` → refuse, name the project's accept
   flow; `unverified` / no provenance → refuse to the producer.
3. **Dedupe** — read the store's existing slugs/descriptions before filing; refuse (with
   the matched path) anything already present.
4. **File** — write-time marks required for a lesson: destinations (`packages`),
   provenance, carryability (person-set), durability (`sugar` vs `lignin`). Missing
   marks → convert prose into the shape yourself and say you did; missing *evidence* →
   refuse, do not guess.
5. **Read back** — re-read the file from disk and the index pointer; print the three-line
   read-back. A read-back that fails means the item is not filed, regardless of what the
   write call returned.
6. **Fan out only above the trigger** — five or more same-batch items each needing real
   per-item authoring, and zero refusals in the batch. Otherwise work serially.

## Errors

- `write returned success, read-back shows nothing` → treat as not filed; retry the write,
  re-verify; report the discrepancy if it persists.
- `item has no provenance` → refuse to the producer, name what's missing (source, origin,
  captured_at).
- `carryable claimed without verified_by` → refuse; carryability needs a named human or a
  documented rule.
- `item already exists in store` → refuse the write, report the existing path instead of
  filing a duplicate.

## Reference docs

- `references/write-time-marks.md` — the four marks, in full, with examples of what each
  looks like filled in.
