> **Status 2026-10-04:** implemented (opt-in `agents.memory_consolidation`, `topic_merge`); see `docs/research/rmm-reflective-memory-management.md` §5 "Phase 2 as built".

## Source

- paper: Tan et al., ACL 2025, arXiv:2503.08026 — Reflective Memory Management (prospective loop)
- research: `docs/research/rmm-reflective-memory-management.md`
- code: `crates/clawft-core/src/agent/memory.rs` (append-only `MEMORY.md`)
- prior: WEFT-347 (MemoryConsolidator: ConversationStore → MEMORY.md) — **grow this, do not add a second consolidator**
- prior: M3 store collapse — session-end hook is ConversationSink / chain, **not** retired `SessionManager`
- related ADR: ADR-058 resolution #2 already reserved "semantic re-chunking" as additive after v1 turn/tool-output granularity

## Problem / gap

Even after retrospective retrieval+citation lands, the bank is still filled by append-only markdown and WEFT-347 distillation into `MEMORY.md`. RMM prospective reflection splits a finished session into **topic units** (not turn/session cuts), nearest-neighbour against the bank, then **merge** (same topic / updated fact) or **insert** (new topic). Without merge, knowledge updates (preference reversed, allergy changed) leave contradictory paragraphs that retrieval will happily surface. ADR-058 explicitly deferred this re-chunking; this ticket is that follow-on.

## Acceptance criteria

- [ ] Session-end hook runs on ConversationSink / chain (not `SessionManager`).
- [ ] Extract topic snippets + summaries from the finished session (LLM or equivalent). Granularity is semantic topic, not turn/tool-output (ADR-058 v1 stays for L2 grafts; this is L3 bank organization).
- [ ] For each snippet: retrieve Top-K existing bank nodes; LLM (or deterministic equality on UNID) decides merge vs insert.
- [ ] Merge test: a later "user is not allergic to penicillin" **supersedes** the earlier allergy node rather than sitting beside it.
- [ ] Insert test: a genuinely new topic does not smash an unrelated node.
- [ ] Idempotent: re-running on the same transcript does not duplicate nodes.
- [ ] Does not invent a second consolidator — extend WEFT-347's module.
- [ ] Tests for merge/insert/idempotency; `scripts/build.sh check` + targeted crate tests.
- [ ] Doc / tracker: WEFT-N back-filled into `docs/research/rmm-reflective-memory-management.md`.

## Dependencies

- blocks: none
- blocked-by: WEFT-732 (RMM retrospective: retrieve Top-K, citation-rerank Top-M, stop MEMORY.md dump) — needs stable memory ids and a non-dump inject path before merge is observable in context
- upstream / external: none

## Notes

- Paper: PR approaches oracle-per-instance granularity on a 100-sample LongMemEval slice; mixed turn+session is worse than either alone (noise).
- Do not pass raw session transcripts into working context; store `(topic summary as key, raw segment as value)` as in the paper.
- Privacy: the bank is personal data (paper Limitations). Follow existing MEMORY.md sanitization; no new store that bypasses `sanitize_content`.
- Gumbel exploration of newly merged nodes is Phase 3 in the research note, not this ticket.
