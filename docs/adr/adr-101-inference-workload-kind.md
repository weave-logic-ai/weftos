# ADR-101: The inference workload kind and migration of local model hosting

- **Status**: Accepted (2026-09-29; decisions settled 2026-09-29, implementation tracked on cards mesh-placement-01..23)
- **Date**: 2026-09-28
- **Deciders**: Platform / ops. Open questions settled 2026-09-29 (defaults accepted by user); status stays Proposed until implemented, but the decisions below are settled pending implementation.
- **Depends-On**: ADR-099 (governed workload placement)
- **Relates-To**: ADR-060 (local Hermes serving and KV management), ADR-018 (Hermes as clawft-llm provider), ADR-100, `~/llm` ADR-0004 (open capability vocabulary), 0016 (local model serving), 0022 (model storage tiers), 0025 (ruflo local models across hosts; title only, not read)
- **Amends**: how ADR-060 / ADR-018 endpoints are configured (static URLs become placement-resolved). Does not change their model, KV, or provider-protocol decisions.

## Context

Requirement: "this really should be the same way local inference is hosted long term", meaning model servers become governed, placed workloads with accelerators as placement targets.

### How local inference is hosted today

**Serving side, `~/llm` (the model lab; `USING_LOCAL_MODELS.md` is its map):**
- `bin/serve-llamacpp <model.gguf> [--draft] [--kv] [--ctx] [--port 8090] [--host]` wraps `llama-server` (OpenAI-compatible, port 8090 default, speculative decoding, KV quantization, Metal on this Mac). This is the Hermes recipe from ADR-060.
- `bin/serve` wraps `mlx_lm.server` (ports 8081-8085, 8092 in the roster).
- Ollama (`:11434`, MLX backend on Apple Silicon since 0.19) for managed models and those `mlx_lm` cannot load; TTS model `orpheus-tts` is served by Ollama.
- `bin/queue roster` prints the measured roster: role, backend, model, serve command (for example `coder-daily` = Qwen3-Coder-Next-4bit on `:8081`, `planner` on `:8082`, `swarm-micro` on `:8085`, `embed`, `vlm`). The source is `docs/models/queue.yaml`.
- Weights live on a removable drive `/Volumes/ai-models` by default, with a small "hot tier" promoted to internal storage (`bin/modelstore`, ADR-0022, `docs/models/storage-tiers.yaml`); Ollama's store moves with `OLLAMA_MODELS`. A detached drive leaves dangling links.
- Concurrency rules are manual: co-residency limits are written in docs ("do not co-reside" planner and Next-coder) and memory is a live constraint (ADR-0016).
- I observed on 2026-09-28: no listener on `:8090`; Ollama on `:11434` up with no model loaded. This machine is an M5 Max, 128 GB unified memory.

**Consumer side, clawft:**
- Constants: `DEFAULT_LOCAL_LLM_SERVICE_URL = http://127.0.0.1:8090`, `DEFAULT_LOCAL_LLM_API_BASE = .../v1`, model `hermes-4.3-36b` (`clawft-types/src/config/local_llm.rs:24-39`); duplicate `DEFAULT_LLM_SERVICE_URL` (`clawft-service-llm/src/lib.rs:72`); `HERMES_SERVING_BASE_URL` and Ollama `http://localhost:11434/v1` (`clawft-llm/src/local_provider.rs:35-50`); builtin providers `local` (prefix `local/`) and `ollama` (`clawft-llm/src/config.rs:133-150`).
- `ProviderRouter::route` picks a provider by model-name prefix; each provider holds a fixed `base_url` (`clawft-llm/src/router.rs:80-110`).
- Resolution order env, then `[kernel.llm]`, then ADR-060 defaults, shared by daemon and CLI (`clawft-core/src/local_llm_bridge.rs`).
- The daemon registers the LLM client as kernel service `llm` with lifecycle flag, health check (`/health` on llama-server) and a chain-anchored contract (`clawft-weave/src/llm_service.rs`).
- Other hardcoded consumers: Orpheus TTS URL `http://127.0.0.1:11434/api/generate` (`clawft-voice-tts/src/orpheus.rs:34`, `clawft-voice-talk/src/tts.rs:77`), the voice loop bound to `:8090` (`clawft-voice-talk/src/llm.rs:24`).
- Per repo memory notes, `.clawft/config.json` can override `weave.toml`, and a bare model name resolves to the openai provider; a migration must not break those precedences.

**Nothing in the kernel today knows about GPUs, NPUs, or model formats** (grep for Metal / CoreML / CUDA / Hailo / Coral in `clawft-kernel` and `clawft-types` finds none).

So today a model server is a hand-started process bound to a fixed local port, described in `~/llm` docs, reached by fixed URLs. It cannot move to a better node, fail over, be governed as a workload, or be found by remote consumers.

## Decision

An inference endpoint is a **placed workload** (`kind = "inference"`) on the ADR-099 layer with a **stable mesh address**. The provider router resolves it through the placement layer instead of a hardcoded `api_base`. Existing constants remain as fallbacks so nothing breaks while the migration proceeds.

### 1. Inference workload spec

```
InferenceSpec {
  role: "coder-daily",                 // stable name consumers use; maps to ~/llm roster roles
  model: ModelRef { manifest: <hash>, name: "Qwen3-Coder-Next-4bit", format: "mlx" | "gguf" | ... },
  runtime: "infer.mlx-lm" | "infer.llamacpp" | "infer.ollama" | ...,   // capability ids, not code
  serve: { ctx, kv_quant, draft_model?, sampling defaults, chat_template_args },
  memory: { weights_bytes, kv_budget_bytes },
  excludes: ["role:planner"],          // co-residency
  latency_class: "interactive" | "batch",
  affinity: { sticky: true },          // warm KV state
  api: "openai-v1",
}
```

The current `bin/queue roster` rows (role, backend, model, serve args, port) translate one to one; importing `queue.yaml` produces specs. `~/llm` remains the model lab and catalog; it stops being the process launcher of record once the adapter can do that (migration step 3).

### 2. Requirements derived

- `runtime.infer.<x>` present (or a generic `runtime.native` plus the server binary), and a **format** capability the runtime supports for this model (`format.gguf` for llama.cpp, `format.mlx` for mlx-lm, `format.coreml` / `format.onnx` / `format.tflite` / `format.hef` for NPU / TPU runtimes);
- an accelerator: `accel.gpu.metal` on this Mac, `accel.gpu.cuda`, `accel.gpu.rocm`, `accel.gpu.vulkan`, `accel.npu.*`, `accel.tpu.*`, `accel.tsu.*`, or none (CPU fallback, explicit only). Attribute predicates on precision (`int4`, `mxfp4`, `fp16`) and supported formats;
- **memory**: `weights_bytes + kv_budget_bytes` against `mem.unified` free (Apple) or `mem.vram` free (discrete GPU). On unified memory this is one pool; the placer must subtract already-placed inference and other workloads from the same pool and honor `excludes`;
- trust tier and provenance from policy.

**Locality is a preference for inference, not a hard filter:** where the model shards already are (`model.present`), where the storage tier is mounted (`store.tier.external.mounted`), and proximity to the heaviest consumer (voice loop latency). A cold external drive counts as absent.

### 3. Model artifacts

Follow ADR-099 section 6, plus:
- **Model manifest**: per-shard `{path, size, blake3}` (safetensors shards, one GGUF file, MLX quant directories), optional tokenizer / chat-template hashes, source (HF repo and revision, Ollama tag), and an operator signature over the manifest.
- Upstream signing is rare, so trust is **operator attestation of a hash manifest**, established at adoption ("trust on first use by an operator", pinned thereafter). A model file whose hash later differs is refused.
- **Adopt in place**: hash the HF cache / `mlx-quants` / Ollama blobs where they lie and register them as artifacts; do not copy. Weights of 20-60 GB (for example the 45 GB Qwen3-Coder-Next 4-bit) are never re-shipped when a node already holds them.
- **Transfer only when needed and allowed**: policy `allow_weight_transfer` with a size ceiling; otherwise place where the bytes are or return `Unplaceable` with the reason. Transfers are chunked and resumable, and the placer records the fetch-versus-relocate choice.
- Storage-tier facts come from probes (drive mounted, free space), because the removable drive can vanish; the workload becomes `Degraded` or `Lost` (not silently broken) if shards disappear under it.

### 4. Adapters

`infer.llamacpp`, `infer.mlx-lm`, `infer.ollama` (each `provides()` its runtime, format, and accelerator capabilities after a probe such as `llama-server --version`, Metal availability, or `ollama` reachable). Each supports:
- **Adopted** mode: register and health-check a server that is already running (today's state), without controlling it. This is the first migration step and needs no behavior change in `~/llm`.
- **Managed** mode: start the server through the native (or container) adapter with the spec's serve args, health-probe `/health` and `/v1/models`, restart with backoff, drain and unload on request. Ollama is special: it manages its own load / unload, so the adapter drives its API and reports state rather than owning the process.
- Health means: process up, `/health` ok, model listed, and (managed) a tiny completion succeeds. `Degraded("loading model")` while a 503 is returned, matching `llm_service.rs`.
- Accelerator-specific inference runners (Hailo HEF, Coral TFLite, RKNN, Qualcomm, TSU samplers) are **deferred** until the hardware exists; they implement the same trait and add capability ids.

### 5. Stable mesh address and provider resolution

Each running inference instance is announced as a `ServiceAdvertisement` named `infer.<role>` (`mesh_service_adv.rs:16`) with metadata (model hash, api, node id, port, load). Consumers resolve `infer.<role>` through a `PlacementResolver`:

- **Compatibility proxy (recommended primary path).** Each node that has consumers runs a **loopback endpoint proxy**: a stable local listener (for example `127.0.0.1:8090`) that forwards OpenAI-compatible HTTP to the current placed instance, locally when it is on this node and over the Noise mesh otherwise. The advertised address never changes when the workload moves. Existing clients, including `~/llm` tools, Grok's `local-*` aliases, the voice loop, Orpheus TTS, and any `apiBase` configuration, keep working unchanged.
- **Direct resolution (for clawft internals).** `ProviderRouter` gains a `resolve_base_url(role)` hook consulted per request with a short TTL cache and invalidation on `MeshPeerEvent` / service-advertisement change. The `local` and `ollama` builtin providers keep their prefixes (`local/`, `ollama/`), which now map to roles; `base_url` becomes a fallback used when resolution fails or the layer is disabled.
- Precedence is unchanged: env, then `[kernel.llm]`, then placement, then the ADR-060 constants. An explicit `LLM_SERVICE_URL` still wins (operator escape hatch).
- Service `llm` in the kernel (`llm_service.rs`) keeps its lifecycle flag and chain contract and gains the resolved endpoint as its source of truth.

### 6. Stickiness, KV, and latency

ADR-060 records that llama.cpp KV slot save/restore is prefix-only and context-shift is off, so an agent loop's speed depends on a warm KV cache on one server. Therefore inference declares `affinity.sticky = true`: no automatic migration on load; migration only on node loss, operator request, or the current node failing constraints, and each migration is chained with a "KV cold" note. Voice (`latency_class = interactive`) adds a hard RTT bound between consumer and server (proposal 30 ms) so a voice consumer is never placed against a remote-LAN server by score alone.

### 7. Governance

Actions from ADR-099 (`workload.place`, `.load`, `.start`, `.unload`, `.migrate`) with effect context including: accelerator use, memory reserved, model source and manifest hash, whether weights are transferred, and consumers exposed to (loopback only, LAN, mesh). Exposure beyond loopback is a separate governed decision, because a `--host 0.0.0.0` model server is currently open to the LAN without authentication (as `~/llm` uses to let Apple containers reach it).

### 8. Migration plan (end state: an inference endpoint is a placed workload)

1. **Observe**: register the currently running servers on this Mac (`:8090`, `:8081...`, `:11434`) as `Adopted` inference workloads with capabilities probed, appearing in `weaver workload list` and the chain. No behavior change.
2. **Describe**: import `~/llm/docs/models/queue.yaml` roster into `InferenceSpec`s and model manifests (adopt in place, operator-signed).
3. **Manage**: switch `Adopted` to `Managed` per role, with the adapter launching `serve-llamacpp` / `serve` / Ollama; `~/llm` scripts remain the underlying commands, invoked through the adapter.
4. **Resolve**: stand up the loopback proxy and `PlacementResolver`; move `orpheus`, voice loop, `clawft-llm` providers, and `clawft-service-llm` onto it; leave constants as fallback.
5. **Distribute**: add a second inference-capable node (for example a Linux GPU box or another Mac) and demonstrate role placement, failover, and weights locality.
6. Only then consider remote GPU / NPU / TPU / TSU nodes, once such hardware and adapters exist.

## Consequences

Positive: local inference gains governance, health, failover, memory-budget enforcement instead of hand-kept rules, and the ability to use a better node; consumers stop caring where a model runs; accelerator support is a vocabulary entry plus an adapter.

Cost: model-manifest and large-artifact machinery; a proxy and resolver; import of the `~/llm` roster; care around sticky KV; a security decision about network exposure.

Risks: a placement bug takes local inference down (mitigation: `Adopted` first, constants as fallback, opt-in per role); unified-memory accounting errors causing OOM (mitigation: reserve with margin, admission self-check, measure); operator confusion between `~/llm` and the layer (mitigation: `~/llm` stays the lab, and only the launcher-of-record role moves).

## Decisions (Decided 2026-09-29, defaults accepted by user)

1. **Inference clients**: keep the loopback proxy as the compatibility strategy; existing clients are unchanged. Rationale: zero client churn, stable address across moves.
2. **`~/llm`**: stays a separate repo. The adapter calls its roster (`docs/models/queue.yaml`, `bin/queue`) and serve scripts (`bin/serve-llamacpp`, `bin/serve`) rather than porting them. Rationale: it is the model lab and catalog; only the launcher-of-record role moves.
3. **Model trust**: operator attestation over a hash manifest (section 3). Rationale: upstream rarely signs weights.
4. **Weight transfer**: no hard ceiling in v1; adopt-in-place is allowed on removable drives, and a detached drive marks the workload Degraded. Rationale: ships value early; drive loss is visible, not silent.
5. **`0.0.0.0` bindings**: permitted only with authentication, via an explicit chained governance Permit (section 7). Rationale: today's unauthenticated LAN exposure should become a recorded decision.
6. **First slice**: `coder-daily`, the Hermes `local/` role, and Orpheus TTS. Rationale: these are the consumers actually wired in clawft and the voice path.
7. **Vocabulary changes** go through the governance path (ADR-099 decision 6).
8. **Measured throughput**: `perf.infer.tok_s{model}` measured capabilities feed scoring for `interactive` roles (ADR-099 section 2).
