# ECC Symposium: Documentation Update Guide

**Instructions for documentation agents updating ClawFT docs with ECC methods and architecture**

---

## Scope

This guide directs documentation agents to update ALL relevant ClawFT documentation to incorporate the Ephemeral Causal Cognition (ECC) paradigm, the CMVG data structure, and the cognitive tick architecture discovered in the Mentra smart glasses project. Updates range from adding new sections to existing documents, to cross-referencing ECC concepts in architecture decisions, to updating the glossary and roadmap.

**Principle**: ECC is not a separate feature -- it is a foundational cognitive layer that touches every WeftOS subsystem. Documentation should reflect this by integrating ECC concepts where they naturally arise, not by isolating them in a single document.

---

## 1. Core Architecture Documents

### 1.1 `docs/architecture/overview.md`

**Action**: Add a new section "Cognitive Layer (ECC)" after the existing architecture overview.

**Content to add**:
- The three-layer CMVG data structure (semantic vectors, causal edges, Merkle provenance)
- How CMVG maps to existing WeftOS pillars: ExoChain (provenance), Resource Tree (namespace), Governance Gate (gating)
- The cognitive tick concept and its relationship to the agent loop
- The "inverted persistence" principle (ephemeral cognition, permanent memory)
- Reference to `docs/weftos/ecc-symposium/01-research-synthesis.md` for full background

### 1.2 `docs/adr/adr-049-weftos-kernel.md` (was `docs/architecture/adr-028-weftos-kernel.md`; renumbered 2026-04-28 / WEFT-140)

**Action**: Add a subsection under kernel design decisions documenting the ECC integration architecture.

**Content to add**:
- New feature flag: `ecc` (gates HNSW kernel service, CausalGraph, CognitiveTick)
- Three new kernel modules: `hnsw.rs`, `causal.rs`, `cognitive_tick.rs`
- How these follow the existing "Adding a New Gated Subsystem" pattern
- HNSW already exists in clawft-core; the kernel integration wraps it with chain/tree/gate

### 1.3 `docs/architecture/wasm-browser-portability-analysis.md`

**Action**: Add ECC portability considerations.

**Content to add**:
- `micro_hnsw.rs` already targets WASM (8KB budget, 1024 vectors)
- Cognitive tick on WASM targets: `performance.now()` instead of `tokio::time::interval`
- Spectral analysis may need to be offloaded from browser WASM to a service worker
- The 3.6ms ARM64 tick budget was proven on Cortex-A53; WASM timing needs benchmarking

---

## 2. WeftOS Documents

### 2.1 `docs/weftos/architecture.md`

**Action**: Major update -- add ECC as the fourth pillar alongside ExoChain, Resource Tree, and Governance Gate.

**Content to add**:
- **CMVG as Fourth Pillar**: The Causal Merkle Vector Graph provides the cognitive substrate
  - Semantic Layer: HNSW index (backed by instant-distance, registered at `/kernel/services/hnsw`)
  - Causal Layer: CausalGraph DAG (typed/weighted edges, backed by DashMap)
  - Provenance Layer: ExoChain (append-only hash chain, RVF persistence)
- **Cognitive Tick**: A 50ms fixed-rate loop (sense-embed-search-update-commit)
- **Tiered Inference**: WeftOS runs on-body (Tier 1) with ECC; offloads to near-body/cloud (Tier 2-3) for LLMs
- Reference the Mentra benchmark data: 3.6ms per tick, 76% headroom, 0% thermal drift

### 2.2 `docs/weftos/kernel-modules.md`

**Action**: Add three new kernel module descriptions.

**Content to add**:
```
## ECC Modules (Feature: `ecc`)

### hnsw.rs -- Vector Index Service
- SystemService implementation wrapping clawft-core's HnswStore
- Registered at /kernel/services/hnsw in the resource tree
- Chain events: hnsw.insert, hnsw.search, hnsw.rebuild
- Gate action: ecc.search (EffectVector: low risk, medium performance)
- Uses instant-distance with configurable ef_search/ef_construction

### causal.rs -- Causal Graph
- In-memory DAG with DashMap<NodeId, Vec<CausalEdge>>
- Edge types: Causes, Inhibits, Correlates, Enables, Follows, Contradicts
- Each edge mutation logged to ExoChain (causal.link event)
- Graph metadata registered at /kernel/services/causal-graph
- Traversal: ancestors(n), descendants(n), causes_of(n), effects_of(n)

### cognitive_tick.rs -- Cognitive Tick Loop
- SystemService with tokio::time::interval(50ms)
- Per-tick: sense -> embed -> search -> update -> commit
- Background: spectral analysis every ~100 ticks
- Drift monitoring with budget alerts
- Sends KernelMessage to agent loop on state changes
```

### 2.3 `docs/weftos/k-phases.md`

**Action**: Update the phase roadmap to include ECC integration milestones.

**Content to add**:
```
### ECC Integration Phases (Parallel to K4-K6)

- **ECC-1 (K4)**: CausalGraph module + HNSW kernel integration + CognitiveTick service
- **ECC-2 (K4-K5)**: Spectral analysis + ECC-specific gate actions + permission scoping
- **ECC-3 (K5-K6)**: CMVG wire protocol (zstd + delta encoding) + distributed delta sync
```

### 2.4 `docs/weftos/integration-patterns.md`

**Action**: Add ECC as a worked example of the "Adding a New Gated Subsystem" pattern.

**Content to add**: Walk through how the HNSW service follows all 7 steps:
1. Define module (`hnsw.rs`)
2. Add chain logging (`hnsw.insert`, `hnsw.search`)
3. Register in resource tree (`/kernel/services/hnsw`)
4. Add gate check (`ecc.search` action with EffectVector)
5. Wire into boot (kernel.boot_ecc())
6. Add CLI commands (`weaver ecc search`, `weaver ecc status`)
7. Write tests (unit + integration with chain verification)

---

## 3. Guide Documents

### 3.1 `docs/guides/rvf.md`

**Action**: Add CMVG-specific RVF usage.

**Content to add**:
- RVF segments for CMVG delta sync (vector deltas + causal edge deltas)
- Planned zstd compression for vector segments (~5:1 ratio)
- BLAKE3 as alternative hash algorithm (already available via tilezero)

### 3.2 `docs/guides/routing.md`

**Action**: Add HNSW-based semantic routing.

**Content to add**:
- ECC enables semantic service discovery: "find a service that does X" via HNSW search
- Complements existing name-based routing (ServiceRegistry) and capability-checked routing (A2ARouter)
- Maps to K2 decision D17 (tiny-dancer intelligent routing)

### 3.3 `docs/guides/voice.md`

**Action**: Add Mentra TTS/audio findings.

**Content to add**:
- Piper TTS at RTF 0.38 on ARM64 (real-time capable)
- Filler phrase latency masking pattern (~800ms perceived response)
- Speaker ID via ecapa-tdnn + HNSW (applicable to voice-first agents)

### 3.4 `docs/guides/weftos-deferred-requirements.md`

**Action**: Review and update deferred items that ECC addresses.

**Content to add**:
- "Intelligent routing" (D17) -- addressed by HNSW semantic search
- "SONA integration" (D18) -- CMVG IS the learning infrastructure
- "Merkle tree indexing for O(log n) proofs" -- HNSW provides O(log n) search
- "Tree sync / Merkle replication" -- CMVG delta sync addresses this

---

## 4. Reference Documents

### 4.1 `docs/reference/tools.md`

**Action**: Add ECC tool specifications.

**Content to add**: New tool category `ecc.*`:
```
ecc.embed     -- Embed input into vector space (HNSW insertion)
ecc.search    -- HNSW similarity search for nearest neighbors
ecc.causal.link -- Create a causal edge in the CMVG
ecc.causal.query -- Traverse causal graph
ecc.merkle.commit -- Compute and commit per-tick Merkle root
ecc.spectral.analyze -- Run Lambda_2 spectral analysis
ecc.tick.status -- Query current tick state
```

Each with: JSON Schema parameters, gate action, 5D EffectVector, native flag.

### 4.2 `docs/reference/config.md`

**Action**: Add ECC configuration options.

**Content to add**:
```toml
[ecc]
tick_interval_ms = 50        # Cognitive tick interval
tick_budget_ms = 15          # Maximum compute time per tick
hnsw_ef_search = 100         # HNSW search parameter
hnsw_ef_construction = 200   # HNSW build parameter
hnsw_dimensions = 384        # Embedding dimensionality
spectral_interval_ticks = 100 # Background spectral analysis interval
max_causal_edges = 10000     # Maximum causal graph edges in memory
```

### 4.3 `docs/reference/security.md`

**Action**: Add ECC security considerations.

**Content to add**:
- CMVG provenance provides verifiable reasoning history (tamper-evident via Merkle chain)
- Cognitive gate actions prevent unauthorized agents from modifying the causal graph
- Spectral analysis detects graph incoherence (potential manipulation)
- HNSW index access is capability-gated per agent

---

## 5. Deployment Documents

### 5.1 `docs/deployment/wasm.md`

**Action**: Add ECC WASM deployment notes.

**Content to add**:
- `micro_hnsw.rs` for WASM targets (8KB budget, 1024 vectors)
- Cognitive tick uses `performance.now()` on WASM instead of tokio timers
- Spectral analysis offloaded to service worker or server

### 5.2 `docs/deployment/docker.md`

**Action**: Add ECC feature flag documentation.

**Content to add**:
- `--features ecc` enables HNSW kernel service, CausalGraph, CognitiveTick
- Container image variants: `clawft:latest` (no ECC) vs `clawft:ecc` (with ECC)
- Resource requirements: +50-100MB RAM for HNSW index + causal graph

---

## 6. Skills & Identity Documents

### 6.1 `docs/skills/clawft/TOOLS.md`

**Action**: Add ECC tools to the tool catalog.

### 6.2 `docs/skills/clawft/IDENTITY.md`

**Action**: Update ClawFT's identity to include cognitive capabilities.

**Content to add**: ClawFT is not just an agent orchestrator -- with ECC, it provides a cognitive substrate where 80-90% of intelligence comes from vector geometry and causal graph traversal, with LLMs reserved for genuinely novel situations.

---

## 7. Symposium Cross-References

### 7.1 K2 Symposium documents

**Action**: Add notes to the following K2 documents referencing ECC symposium findings:

- `k2-symposium/04-industry-landscape.md` -- Add note that ECC addresses the "O(log n) proof efficiency" gap via HNSW
- `k2-symposium/05-ruv-ecosystem.md` -- Add note that HNSW (instant-distance) is already implemented in clawft-core
- `k2-symposium/07-qa-roundtable.md` -- Add notes on Q16 (causal ordering), Q17 (intelligent routing), Q18 (learning infrastructure) referencing ECC answers

### 7.2 K3 Symposium documents

**Action**: Add notes to the following K3 documents:

- `k3-symposium/06-qa-roundtable.md` -- Note that ECC tools (ecc.*) should be included in the K4 tool implementation priority
- `k3-symposium/07-symposium-results-report.md` -- Note that D14 (tiny-dancer for tool routing) aligns with HNSW semantic routing

---

## 8. Glossary Additions

The following terms should be added to any glossary or terminology reference in the documentation:

| Term | Definition |
|------|-----------|
| **ECC** | Ephemeral Causal Cognition -- paradigm where cognition is transient (per-tick) and memory is permanent (Merkle DAG) |
| **CMVG** | Causal Merkle Vector Graph -- three-layer structure: vectors + causal edges + Merkle provenance |
| **Cognitive Tick** | 50ms loop: sense -> embed -> search -> update -> commit |
| **Lambda_2 / Fiedler Value** | Second-smallest eigenvalue of the normalized Laplacian; metacognition about graph coherence |
| **Tiered Inference** | Small/fast at edge + medium/near + large/slow in cloud |
| **Inverted Persistence** | ECC pattern: ephemeral cognition + permanent memory (opposite of LLM: permanent weights + ephemeral context) |
| **Geometric Transformer** | Message passing where node embeddings aggregate weighted neighbor embeddings |
| **DEMOCRITUS** | Academic algorithm for LLM causal relation extraction (Mahadevan 2025) |

---

## Execution Notes for Documentation Agents

1. **Do not create new files** unless explicitly listed above. Prefer editing existing documents.
2. **Maintain existing document structure** -- add ECC sections at appropriate locations within each document, do not reorganize.
3. **Cross-reference**: Link to `docs/weftos/ecc-symposium/01-research-synthesis.md` for full ECC background. Link to `docs/mentra/MENTRA_RESEARCH_INDEX.md` for Mentra hardware details.
4. **Use consistent terminology**: Always "ECC" not "ephemeral causal cognition" after first use. Always "CMVG" not "Causal Merkle Vector Graph" after first use. Always "cognitive tick" not "ECC tick" or "tick loop."
5. **Cite benchmarks**: When mentioning performance, cite "3.6ms per tick on ARM Cortex-A53 (Mentra BQ-7)" rather than vague claims.
6. **Feature flag convention**: All ECC code is behind `--features ecc` in Cargo.toml. Document this consistently.
