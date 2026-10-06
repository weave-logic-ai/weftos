# ADR-110: Agent transcript collection for shared memory and training

- **Status**: Accepted (2026-10-05; open questions decided by the owner the same day)
- **Deciders**: owner
- **Builds on**: ADR-108 (pairing, node actions), ADR-109 (agent environment profiles; local
  doctor), the daemon `[dashboard]` reporter, the mesh (Noise XX between node keys),
  photo-gallery as the primary node and main store.

## Context

Members work through Claude Code, OpenAI Codex and Grok. Each keeps its chat logs locally:
Claude Code session files under `~/.claude/projects/`, Codex under `~/.codex/sessions` (and
`history.jsonl`), Grok under `~/.grok/sessions`. Collected centrally, they become shared memory
(what was decided, how problems were solved, per project) and a corpus for evaluation and
training. They are also the most sensitive data WeaveLogic holds: client source and decisions,
pasted credentials, personal notes.

## Decision (proposed)

1. **A collector on every member machine, a store on photo-gallery.**
   - The user daemon gains a `[transcripts]` collector. It tails each tool's session files
     incrementally (offset per file) and ships new turns to a transcript service on PG.
   - The PG service runs under its own account with a 0700 store.
   - Transport is the mesh, inside the Noise XX session between node keys. There is no
     upload API on the public internet and nothing passes through the dashboard.
2. **Redact before anything leaves the machine.**
   - Every turn passes a secret scanner and a redactor first (tokens, keys, passwords,
     connection strings, private keys, `.env` content) and is replaced with typed placeholders.
   - A turn that cannot be scanned is held locally, not sent.
   - Redaction counts are reported. Raw values never are.
3. **Every turn is tied to a project.**
   - The session's working directory maps to a registered project ULID. Unmapped sessions go
     to an `unassigned` bucket the member can assign or drop.
   - Storage and access are partitioned by project and company.
   - Client transcripts are never pooled across clients by default.
4. **Consent and control per member.**
   - Collection is **on by default** for WeaveLogic member machines (owner decision). Each
     member gets a clear notice when their machine is enrolled: what is collected, where it
     goes, who can read it, and how to pause it or exclude projects.
   - Members can pause collection and exclude projects at any time.
   - A member can list, export and delete their own transcripts from the dashboard.
   - Raw transcripts of a project are readable by that project's members (owner decision).
     The dashboard shows collection status (counts, projects, last sync) to everyone with
     access.
5. **Two uses, two gates.**
   - *Shared memory*: indexed per project into the project's knowledge store (RVF/ruvector,
     following the existing brain conventions). It is searchable by members with access to
     that project.
   - *Training/evaluation*: a separate, explicit opt-in per project. Client projects are
     **excluded until the owner records, per company, that the client permits it** (owner
     decision). Internal projects may opt in. A training export is a
     dated, versioned snapshot with its inclusion rules recorded.
6. **Retention.** Configurable per project. Deleting a project's transcripts removes them
   from the store and its memory index.

## Phases

| Phase | Delivers | Check |
|---|---|---|
| T1 Service + collector (owner only) | Transcript service on PG (own account, store), collector on the owner's Mac for the three tools, redaction, project mapping. | The owner's new turns appear on PG within a minute, redacted (planted fake secrets never arrive), each tagged with the right project. |
| T2 Dashboard controls | Consent, pause, per-project exclusion, list/export/delete, collection status per machine. | A member can opt in, exclude a project, and delete a session from the dashboard; the PG store reflects it. |
| T3 Shared memory | Per-project indexing; search in the dashboard and for agents (MCP). | A question about a past decision in a project returns the turn where it was made, only for members with access to that project. |
| T4 Rollout | Install the collector on every developer machine through ADR-109 apply (with consent). | Each opted-in member machine reports a healthy collector in the doctor. |
| T5 Training exports | Opt-in per project, export snapshots with rules recorded. | An export contains only permitted projects, with redaction stats and its rule set. |

## Consequences

- Shared, project-scoped memory across members and tools; a governed training corpus.
- The highest-risk data flow WeftOS carries. Mitigations: redaction before transport,
  mesh-only transport, project partitioning, per-member consent, per-project training gates,
  and deletion. Negative tests are required: planted secrets, excluded projects, other
  members' sessions and revoked machines.
