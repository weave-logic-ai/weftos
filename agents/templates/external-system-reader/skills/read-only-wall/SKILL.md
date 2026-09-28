---
version: 0.1.0
name: read-only-wall
description: |
  Reads a granted, live third-party system through a transport that refuses writes by
  construction, cites every claim with an object id and a last-modified date read from
  the object itself, and converts any write request into a finding for the system's own
  human administrator.
  Use when: "which object holds X", "what does the live system actually track", "who
  last changed this and when", "is our doc still true of the live system".
  Chain with: doc-garden or the domain-expert template's grounded-domain skill when a
  contradiction between the live system and project docs needs doc-first resolution.
  NOT for: any write, staged or direct; obtaining credentials for the requester;
  proposing a write-sync between the project's own system and the external one.
argument-hint: "[find|search|read] <object-or-query>"
allowed-tools: Read, Grep, Bash
---

# Read-Only Wall

Reads a granted external system through a write-refusing transport, with mandatory
per-object citation and freshness.

## Bootstrap

1. Read `.agents/project-context.md` for the `external_system` block: the read-only
   transport/CLI, its health-check ("whoami") command, and any local index/cache
   command.
2. Run the health-check before relying on the transport for anything substantive.
3. Read the project's own analysis of the estate (usage map, parity notes), if one
   exists.

## UX Rules

1. Narrow before you read — index/search first, then the one object, then the specific
   fields; a full dump is neither possible nor useful on a real estate.
2. Cite every claim with object type + id + name + the field it came from + the object's
   own last-modified date (never a coarse index-level timestamp).
3. Grade every answer: observed / documented / inferred / unknown.
4. On any write request, decline in one sentence and produce a finding for the system's
   named administrator instead.

## Workflow

1. **Locate** the object by name or by content search through the read-only
   index/search command.
2. **Read** the specific object — its shape, its rows/fields, its edit history if the
   transport exposes one — rather than pulling a broad export.
3. **Cite** with id, field, and the object's own modified-date, not an index-level one.
4. **Compare against project docs** if the question is about drift; surface both sides
   if they disagree, hand the contradiction to the project's doc-resolution owner.
5. **On a write request**: decline in one sentence, describe the exact object/field/
   change precisely enough to act on, and name it as a finding for the system's
   administrator.

## Errors

- `write attempted or requested` → refuse; this is the one hard rule of the whole
  template. Convert to a finding for the administrator.
- `index-level timestamp used as freshness evidence` → re-read the object directly; an
  index list's timestamp is known to lag the real one on live estates.
- `credential/auth failure` → stop and report; never retry around an auth failure or
  fall back to a different, less-controlled transport.
- `read would need to sweep hundreds of objects` → propose a narrower query instead of
  attempting the full sweep.

## Reference docs

None in the base template — a concrete project instance should add its own
`references/estate-shape.md` once the specific granted system and its usage map are
known.
