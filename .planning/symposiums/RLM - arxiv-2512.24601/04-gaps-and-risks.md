# RLM (arXiv:2512.24601) — Gaps, Mismatches, and Risks

**Author:** gaps-and-risks specialist
**Date:** 2026-04-23
**Paper:** Zhang, Kraska, Khattab. *Recursive Language Models* (arXiv:2512.24601v2, Jan 2026).
**Scope:** the negative space — what the paper *assumes* that WeftOS cannot afford to assume, what WeftOS *has* that the paper ignores, what breaks if we copy the method naively, and what the panel still has to resolve. No adoption recommendation; read alongside the other three panel notes.

---

## 1. What the paper assumes that WeftOS cannot

### 1.1 Trust model: "a Python REPL with a `context` variable"

Algorithm 1 (`paper §2`) binds the user prompt `P` to a Python variable and lets the root LLM emit Python that is `exec`'d against it, with `llm_query` as a callable. The paper's threat model is that there isn't one — it is a single-tenant research harness where the prompt, the REPL, and arbitrary provider sub-calls are all trusted. WeftOS is the opposite shape: every `tool.exec`, `sandbox.execute`, `shell.exec`, and `wasm.execute` runs through `crates/clawft-kernel/src/chain.rs` with a pre-call `GateBackend::check()` (pattern at `cluster.rs:558-628`). A "write Python, we'll run it" primitive at the *root* of an agent loop is a full bypass of that gate. The paper is silent on this because its model-vs-data boundary is not a capability boundary.

### 1.2 Network model: cloud frontier APIs, seconds of slack

Figure 3 and §4 report costs per provider API call on GPT-5 and Qwen3-Coder-480B via Fireworks; Appendix B ("RLMs without asynchronous LM calls are slow") concedes 10s–100s of seconds of wall time per trajectory. WeftOS's latency budget is shaped by the 1 ms ECC cognitive tick (`DEMOCRITUS` in `clawft-core/src/agent/loop_core.rs`) and by ESP32 sensor bridges over TCP from a WSL2 host — per-frame deadlines in milliseconds, not seconds. The RLM paradigm cannot live inside a tick; any adoption is out-of-band, and the paper's headline numbers do not carry.

### 1.3 Consistency model: no state, no peers, no log

RLM trajectories build REPL state, emit `Final`, and evaporate. There is no durable log, no replay, no multi-node view, no hash-linked envelope. WeftOS commits to the opposite: every governance-bearing action is a SHAKE-256 hash-linked event on the Governance channel, Ed25519 + ML-DSA-65 dual-signed (see `exochain-logging/01-exochain-specialist.md §1`). A REPL variable named `context` has no corresponding substrate path and no `ChainEvent` lineage; the paper does not even *have* the concept we can't give up.

### 1.4 Hardware access pattern: no sensors, no actuators, no real time

The four benchmarks (S-NIAH, BrowseComp-Plus 1K, OOLONG, OOLONG-Pairs, LongBench-v2 CodeQA) are static-text, single-turn prompt-in / answer-out. WeftOS's primary input is the ESP32 fleet in `.planning/weftos_sensors.md` — I2S mics, MLX90640 thermal, VL53L1X ToF, IMUs, LiDAR, UWB — with bounded jitter and streaming arrival. Nothing in RLM's design accommodates back-pressure, frame drops, or actuator deadlines. "Ω(|P|²) semantic work" is fine on 10M-token offline corpora and fatal on a 1 kHz sensor tick.

### 1.5 Concurrency model: synchronous Python in one process

Algorithm 1 is strictly sequential; §6 flags async as future work. WeftOS is Rust + tokio with `mesh_runtime`, a2a IPC, and four-channel cross-node replication in flight. Making a Python REPL the root of the reasoning loop inverts the architecture: Python orchestrates, Rust becomes a subordinate callee. If we pursue RLM at all, the language boundary has to go the other way — Rust orchestrates, the REPL (if any) is a bounded tool.

### 1.6 Prompt = data = code

"Treat the prompt as an external environment" collapses user data, tool output, and executable code into one Python variable that the LLM rewrites with regex slices and `exec`. WeftOS keeps those three in three places with three governance classes: user data on the substrate (`weft://…`, `clawft-substrate`), tool output on the A2A bus with typed envelopes (K2), executable code in the WASM sandbox behind capability RBAC (K3). Flattening them into `context: str` is not a language choice, it's a policy regression.

### 1.7 Model access pattern: one root, unlimited sub-calls

RLM assumes `llm_query` is cheap, idempotent, and always available. WeftOS's k_level observe-only tier and `eml.*` drift/observe/recall events already treat the LLM as a budgeted, logged, governed resource. "Launch Ω(|P|²) processes to understand or transform all parts of P" is exactly the workload pattern that `hnsw.eml.observe/recall` at 10k+/s was built to *contain*, not enable.

---

## 2. What WeftOS has that the paper does not address

### 2.1 A first-class ontology, not a string variable

The substrate's path-based ontology (`weft://…`, `clawft-substrate`, `exo-resource-tree`) is a structured, HNSW-indexed, checkpointable resource tree. RLM offloads "context" into `context: str` and then rediscovers structure by having an LLM emit regexes. We already have the structure. Any recursion/decomposition can address substrate paths directly, and the resource tree's HNSW search is a strictly better primitive than "write regex code to find the sub-tree you want". The paper is silent on structured addressing.

### 2.2 ExoChain's four-channel split + `k_level` observe-only

The exochain-logging symposium has landed on Governance / Fabric / Stream / Diag with per-channel checkpoint cadence, RVF subtype `0x42` batched columnar for high-rate producers, and a `k_level: u8` runtime tag (`01-exochain-specialist.md §2.2`). The paper has no vocabulary for routing events by rate-class or governance-weight. A naïve RLM trajectory emits 10³–10⁴ sub-call events; the channel split is how we keep those off Governance and anchored as a window summary on Stream. We paid that engineering cost; the paper didn't.

### 2.3 ML-DSA-65 + Ed25519 dual signing, SHAKE-256 linking

`chain.rs:1409-1427` dual-signs every checkpoint with Ed25519 and post-quantum ML-DSA-65. The paper produces unsigned, unreproducible trajectories. Where the *decision trace* has audit value (governance review, RVF export, regulated industries), WeftOS's artifact is strictly richer.

### 2.4 Governance with 22 rules, 0.7 threshold, effect vectors

The `EffectVector { risk, fairness, privacy, novelty, security }` (`governance.rs:117-201`) plus the threshold-0.7 rule set gives us pre-call admission. RLM has no admission mechanism; every sub-call goes through. If we want RLM-shaped reasoning, the gate needs to fire at least at "should this sub-trajectory spawn" granularity — architecture the paper does not discuss.

### 2.5 Arrow IPC + typed envelopes

K2's typed envelopes and the Arrow IPC commitment for the four ExoChain channels give us a columnar, zero-copy, schema-versioned transport. The paper uses `print()` + "truncated stdout" as its IPC between root and REPL (§2, fn.1). Anything RLM-like we build must not degrade to stringly-typed stdout.

### 2.6 Surface composer + resource-tree checkpointing

`clawft-surface` already materializes views of the resource tree at bounded cost, and the resource tree is checkpointable via kernel save/load. We already have a persistent, navigable "environment" that survives across agent turns — precisely what RLMs fake with an in-memory Python REPL. We can host RLM-style symbolic recursion on the substrate *with* persistence; the paper cannot.

### 2.7 Sensor primary plane (LeWM, ADRs 048–058)

ROADMAP.md asserts the sensor pipeline is the primary, self-sufficient plane and any reasoning service is an additive consumer. The paper takes the inverse stance (reasoning primary, context is a string). Ours is load-bearing for ADR-058's decoupling invariant and cannot be reversed for an RLM-shaped top-of-stack.

---

## 3. Risks of naive adoption

### 3.1 Governance bypass via `llm_query`

If `llm_query` is exposed to a root LLM in a Python REPL with the substrate bound as `context`, the model can issue sub-queries against arbitrary substrate slices without touching `GateBackend::check()`. A trajectory that does `llm_query(substrate["/secrets/..."])` reads capability-protected data and logs nothing governance-bearing. Fixing this means wrapping `llm_query` itself as a gated tool in `clawft-kernel/src/chain.rs`, which negates much of the "launch Ω(|P|²) calls" speed argument.

### 3.2 Chain flood on the wrong channel

A single OOLONG-Pairs-style trajectory emits thousands of sub-LLM calls. If each emits `tool.exec` or `eml.recall` on Governance, boot-time `verify_integrity()` goes from O(governance) to O(trajectory). Naïve adoption re-introduces the rate-mismatch the four-channel redesign is meant to eliminate (`01-exochain-specialist.md §3.1`).

### 3.3 Cognitive-tick starvation

The 1 ms ECC tick (`DEMOCRITUS`, `clawft-core/src/agent/loop_core.rs`) shares a tokio runtime with anything co-hosted. Blocking HTTP to a frontier provider inside that runtime starves the tick. Any RLM host must run in a dedicated runtime or separate process with a hard budget; results return via substrate/A2A, never by blocking a reasoner in the hot loop.

### 3.4 Sensor frame loss on RLM-induced back-pressure

If RLM sub-calls saturate outbound TCP to an LLM provider, the substrate bus back-pressures upstream and `StreamWindowAnchor` (`stream_anchor.rs`) starts dropping frames; `stream.idle_disable` fires. Sensors go dark while "reasoning" is in flight. The paper does not contemplate a shared resource across reasoning and perception; we do.

### 3.5 Prompt-injection attack surface on `context`

The paper hands the root LLM untrusted text and tells it to `exec` Python that touches it. Any adversarial substring in the substrate can redirect the REPL. With WeftOS data (user content, tool output, cross-node mesh payloads, signed chain events), the attack surface is broader than any cited benchmark. Adoption must treat `context` as adversarial; the paper's prompts do not.

### 3.6 Non-determinism and unreplayability

LLM sampling plus floating-point reductions in sub-calls make trajectories non-deterministic. The paper does not address replay. `weaver chain verify` expects reproducibility. If an RLM decision feeds a governance event, the event stops being independently verifiable — a hard incompatibility with K3 tool signing (D9) and "universal witness by default".

### 3.7 Dependency-class creep

RLM as stated requires CPython, a provider SDK, and an interpreter. WeftOS ships a native Rust CLI with mutually exclusive `native` / `browser` features (MEMORY.md); CPython as a hard dep breaks `scripts/build.sh wasi` and `scripts/build.sh browser`. A pure-Rust + wasmtime reimplementation is net-new engineering, not "adopting the paper".

### 3.8 Mesh replication of trajectory state

Fabric replicates via `SyncStreamType::Chain`/`::Ipc` (`mesh_chain.rs:201-212`). RLM REPL state is process-local and large. Replicating a multi-megabyte `context` across peers breaks Fabric rate assumptions; not replicating it makes multi-node RLM decisions non-auditable. Either answer breaks an existing invariant.

### 3.9 Cost-tail risk becomes deadline-miss risk

Paper Figure 3 shows p95 API-cost spikes far above median from long trajectories. For us that tail is a *deadline miss*, not a cost line. Cancelling mid-flight has side effects (allocations, partial sub-calls already billed) and the paper describes no cancellation semantics.

### 3.10 Fine-tuning path collides with our model story

§4 Obs. 6 leans on fine-tuning a small model (RLM-Qwen3-8B) on 1000 trajectories. Our story is budgeted, observed, governed LLM access via the tiered router (ROADMAP Sprint 10 Week 2-3: OpenRouter). A first-party "native RLM" implies a model artifact, training pipeline, and supply-chain risk class we do not have.

---

## 4. Open questions for the panel

1. **Is RLM a reasoner pattern or a substrate pattern?** If the latter, does substrate + HNSW already satisfy the "prompt as environment" invariant without any REPL?
2. **Narrowest wrapping of `llm_query` that preserves the gate?** Which crate owns it, and does every sub-call emit `tool.exec` on Diag (cheap) or Governance (auditable but floods)?
3. **Where does the REPL live, if anywhere?** wasmtime-hosted Python, out-of-process CPython, or a Rust-native symbolic-recursion harness that keeps the insight and drops the Python dep?
4. **Budget enforcement — per-trajectory or per-tick?** What is the cancellation primitive for an overrunning trajectory, and how does it mesh with tokio cancellation and the 1 ms ECC tick?
5. **Does the root LLM ever see signed substrate data directly?** Or only paths + HNSW handles, with dereferencing wrapped by capability checks?
6. **Replayability contract.** If an RLM trajectory feeds a governance decision, do we snapshot (model, seed, prompt, sub-call trace) to Governance, or refuse the input path entirely?
7. **Which channel carries sub-call events?** Stream with a rolling manifest, Diag with truncation, or a new `reasoning` channel?
8. **Mesh partition behavior?** Is an in-flight trajectory bound to its origin node, or does it survive peer failure via replicated state?
9. **Does RLM have a place below K6, or only above?** Can a sensor-pipeline component ever use RLM-shaped recursion, or is it strictly K5+?
10. **Provider-down fallback.** WSL2 → ESP32 deployments can be partitioned from any cloud LLM. Does RLM degrade to local-only (Mock / AST-aware / SentenceTransformer per K3c), or become unavailable?

---
