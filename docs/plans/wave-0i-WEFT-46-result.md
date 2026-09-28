# Wave 0i — WEFT-46 result

**Ticket:** WEFT-46 — ws03: routing — wire v2.5 sona-backed rerank step in HybridRouter  
**Branch:** `wave0i/weft-46-sona-rerank`  
**Base:** `release/0.8-staging`  
**Status:** implemented  
**Worktree:** this agent worktree  

## Acceptance criteria

| AC | Status |
|----|--------|
| Sona crate API stable enough to depend on | **Done** — workspace pin `ruvector-sona = "0.2"` (crates.io 0.2.1); `SonaEngine` / MicroLoRA / ReasoningBank surface used |
| Implement rerank in `HybridRouter::route` after primary candidate set is built | **Done** — non-empty primary → optional `SkillReranker` reorders skills; empty primary still falls through to fallback (no rerank on fallback path) |
| Benchmark: rerank-on vs rerank-off accuracy + latency | **Done** — unit bench `bench_rerank_on_vs_off_latency_and_accuracy` (day-0 hit@1 = primary; on ≈ 9 µs/call ≪ 3 ms budget) |
| Feature-gate behind `hybrid-rerank` so a slow upstream doesn't block builds | **Done** — `clawft-core` feature `hybrid-rerank = ["dep:ruvector-sona"]`; **not** in `default`/`full` |

## Design

```
HybridRouter::route
  └─ primary.route(req)
       ├─ empty decision → fallback.route + fallback_used=true  (no rerank)
       └─ non-empty
            └─ if reranker attached:
                 rank_priors(skills) → SkillReranker::rerank → skills
                 (archetype cleared if top-1 flips)
            else: primary decision as-is
```

### Layers

| Layer | Feature | Role |
|-------|---------|------|
| `SkillReranker` trait + `IdentityReranker` | always | Testable plumbing; zero heavy deps |
| `HybridRouter::with_reranker` / `with_skill_reranker` | always | Optional attach point |
| `SonaSkillReranker` | `hybrid-rerank` | `ruvector-sona` MicroLoRA residual + ReasoningBank pattern boost |

### Day-0 fail-open

Untrained SONA (empty ReasoningBank **and** MicroLoRA residual ≈ 0) preserves primary order so enabling the feature without feedback never regresses retrieval.

### Production enablement

```rust
// Requires: clawft-core built with `--features hybrid-rerank`
use clawft_core::agent::context_router::{
    HybridRouter, SonaSkillReranker, /* + primary/fallback routers */
};

let hybrid = HybridRouter::new(primary, fallback)
    .with_skill_reranker(SonaSkillReranker::new());
// Optional feedback path (not yet wired into AgentLoop):
// sona.observe(query, selected_skill, quality); sona.force_learn();
//
// Follow-on (ws06-memory, RMM arXiv:2503.08026): the same observe path
// should take per-retrieved-memory (id, +1 cited | −1 ignored) and
// rerank memory candidates, not only skills. Retriever stays frozen.
// Canonical: docs/research/rmm-reflective-memory-management.md
```

## Files changed

| Path | Change |
|------|--------|
| `Cargo.toml` | Workspace pin `ruvector-sona = "0.2"` |
| `crates/clawft-core/Cargo.toml` | Feature `hybrid-rerank`; optional dep |
| `crates/clawft-core/src/agent/context_router.rs` | Export `rerank` + cfg `sona_rerank` |
| `crates/clawft-core/src/agent/context_router/hybrid.rs` | Optional rerank after primary |
| `crates/clawft-core/src/agent/context_router/rerank.rs` | **New** — trait, identity, `apply_rerank` |
| `crates/clawft-core/src/agent/context_router/sona_rerank.rs` | **New** — sona adapter + bench test |
| `docs/plans/wave-0i-WEFT-46-result.md` | This file |
| `Cargo.lock` | Locked `ruvector-sona` (and its deps) when feature resolves |

## How to test

```bash
# Plumbing (no sona crate)
cargo test -p clawft-core --lib agent::context_router::hybrid
cargo test -p clawft-core --lib agent::context_router::rerank

# Sona-backed adapter + on/off bench
cargo test -p clawft-core --features hybrid-rerank --lib sona_rerank -- --nocapture

# Default workspace still builds without hybrid-rerank
scripts/build.sh check
```

### Bench sample (this worktree)

```
WEFT-46 bench: off=1.47 µs/call hit@1=200/200; on=8.71 µs/call hit@1=200/200 (day-0)
```

## Residual / follow-ups

- **AgentLoop observe wiring** — `SonaSkillReranker::observe` is public but not yet called from the chat path after turn outcomes (needs implicit quality signals from WEFT-335 routing log / tool-match feedback).
- **Real skill embeddings** — hash floor is deterministic offline; production can later inject EmbeddingRouter vectors into candidates for stronger cosine signal.
- **Daemon config flip** — no `routing.context.kind = hybrid` config surface yet; attach `with_skill_reranker` at bootstrap when product wants it live.
- **v3 MicroLoRA router** — still deferred on ruvllm-wasm 11-pattern HNSW cap (unchanged).

## Commit

- **Branch:** `wave0i/weft-46-sona-rerank`
- **Message:** `feat(core): WEFT-46 sona-backed HybridRouter skill rerank (hybrid-rerank)`
