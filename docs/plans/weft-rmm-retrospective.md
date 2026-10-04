> **Status 2026-10-04:** Phase 1 implemented (opt-in `agents.memory_recall`); see `docs/research/rmm-reflective-memory-management.md` §5 "Phase 1 as built".

## Source

- paper: Tan et al., ACL 2025, arXiv:2503.08026 — Reflective Memory Management (retrospective loop)
- research: `docs/research/rmm-reflective-memory-management.md` (canonical; **this ticket must not drift from that note**)
- code: `crates/clawft-core/src/agent/context.rs` (`# Relevant Memory:` dump of entire `MEMORY.md`)
- code: `crates/clawft-core/src/agent/memory.rs`
- code: `crates/clawft-core/src/agent/context_router/sona_rerank.rs` (`SonaSkillReranker::observe`)
- code: `crates/clawft-core/src/vector_store.rs`
- prior: WEFT-46 (Sona skill rerank, observe unwired), WEFT-665 (graft debris poisoning MEMORY.md), WEFT-347 (MemoryConsolidator)
- related ADR: ADR-058 (session RVF + HNSW), ADR-059 (embedder / Qwen3-Reranker), ADR-096 (no silent champion swap)

## Problem / gap

Long-term memory is still the nanobot dump: `ContextBuilder::build_messages` reads all of `MEMORY.md` and injects it as `# Relevant Memory:`. There is no topic id, no Top-K retrieval into the prompt, and no signal for which retrieved facts the generator actually used. `SonaSkillReranker` already has MicroLoRA + ReasoningBank + day-0 fail-open, but it reranks skills, `observe()` takes one scalar, and AgentLoop never calls it. RMM (arXiv:2503.08026) shows the missing loop is citation-attributed rewards on a **lightweight reranker** with the dense retriever frozen — RL on the retriever itself collapsed Acc 58.8 → 31.0 in their ablation. WEFT-665 already proved a poisoned `MEMORY.md` gets re-injected every turn.

## Acceptance criteria

- [ ] When `vector-memory` (or HNSW/RVF retrieval) is on, `build_messages` injects at most Top-M retrieved memory snippets, each tagged with a stable id (`m1`…`mM`). It does **not** dump the full `MEMORY.md`.
- [ ] Day-0 fail-open: untrained SONA / empty bank / retrieval-off preserves today's dump behaviour (compat with WEFT-46 fail-open).
- [ ] Generator prompt asks for citations of those ids in the **same** completion as the answer (no second judge call on the hot path).
- [ ] `SonaSkillReranker::observe` (or a sibling memory reranker sharing `SonaEngine`) accepts per-candidate `(id, +1 cited | −1 retrieved-but-ignored)`. AgentLoop wires this after the turn. Never-retrieved ids are not updated.
- [ ] Dense retriever / embedder is **not** updated from these rewards. Feature-gated so default `weft` does not take SONA as a hard dep.
- [ ] Tests: fail-open order preserved; Top-M cap; citation ±1 accounting; retrieval-on does not re-inject a WEFT-665-class poisoned full file.
- [ ] Build gate: `scripts/build.sh check` and targeted `scripts/build.sh test -p clawft-core` (and `clawft-tools` if memory_tool changes).
- [ ] Doc / tracker: this ticket's WEFT-N back-filled into `docs/research/rmm-reflective-memory-management.md` §0 and §7.

## Dependencies

- blocks: WEFT-733 (RMM prospective: session-end topic extract + merge/insert)
- blocked-by: none (WEFT-46 plumbing already shipped; observe wire is this ticket)
- upstream / external: `ruvector-sona` already pinned (WEFT-46). Do not fine-tune GTE/Qwen3 embedder here. Do not treat ADR-059 `Qwen3-Reranker-0.6B` as this adapter — they can stack later.

## Notes

- Paper numbers (GTE, LongMemEval): 69.8% R@5, 70.4% Acc. **Not** a WeftOS gate. Gate is dump-gone + citations-flow + fail-open.
- Circulating 43/37/28% "production" lifts are **not in the paper**. Keep them out of this ticket.
- Do not start with Gumbel/REINFORCE. Binary citation rewards into existing `observe` is phase 1; Gumbel is a follow-on on this ticket or a comment, not a blocker.
- Reranker-weight promote later = MetaHarness flywheel + `confirm=true` (ADR-096). No silent champion swap.
- Companion: ReasoningBank arXiv:2509.25140 distills *strategies*; this ticket is *which memories were used*.
