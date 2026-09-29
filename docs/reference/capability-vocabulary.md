# Capability vocabulary

<!-- Generated from config/capabilities.toml by the clawft-types test `vocabulary_doc_is_current`. Do not edit by hand; regenerate with WEFTOS_REGEN_DOCS=1 scripts/build.sh test clawft-types -->

WeftOS placement capability vocabulary (version 1). Advisory only: ids not listed here are accepted and matched; experimental ids use the `x.` prefix. Source: ADR-099 section 2.

Vocabulary digest (SHA-256): `97785a67d5a9581b32aabc402e686efb5598570c0688f3841e44d4f4ff8b4380`

## CPU (`cpu`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `cpu.arch.aarch64` | 64-bit ARM | `cores`: int, `model`: string | - |
| `cpu.arch.armv7` | 32-bit ARM (armhf) | `cores`: int, `model`: string | - |
| `cpu.arch.x86_64` | 64-bit x86 | `cores`: int, `model`: string | - |

## Operating system (`os`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `os.linux` | Linux | `distro`: string, `kernel`: string | - |
| `os.macos` | macOS | `version`: string | - |

## Runtimes (`runtime`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `runtime.container.apple` | Apple container runtime | `arches_emulated`: list, `arches_native`: list, `version`: string | - |
| `runtime.container.docker` | Docker-compatible engine (variant: orbstack, engine, desktop) | `arches_emulated`: list, `arches_native`: list, `variant`: string, `version`: string | - |
| `runtime.container.podman` | Podman | `arches_emulated`: list, `arches_native`: list, `version`: string | - |
| `runtime.infer.llamacpp` | llama.cpp server | `arches_native`: list, `formats`: list, `version`: string | - |
| `runtime.infer.mlx-lm` | mlx-lm server | `arches_native`: list, `formats`: list, `version`: string | - |
| `runtime.infer.ollama` | Ollama | `arches_native`: list, `formats`: list, `version`: string | - |
| `runtime.native` | Unprivileged native process | `arches_emulated`: list, `arches_native`: list | - |
| `runtime.wasm.wasmtime` | Wasmtime WASM host | `version`: string | - |

## Accelerators (GPU, NPU, TPU, TSU, other) (`accel`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `accel.gpu.cuda` | NVIDIA CUDA GPU | `device`: string, `formats`: list, `mem_bytes`: int, `mem_free_bytes`: int, `precisions`: list, `sdk_version`: string, `unified`: bool, `vendor`: string | - |
| `accel.gpu.metal` | Apple Metal GPU | `cores`: int, `device`: string, `formats`: list, `mem_bytes`: int, `mem_free_bytes`: int, `precisions`: list, `sdk`: string, `sdk_version`: string, `unified`: bool, `vendor`: string | - |
| `accel.gpu.rocm` | AMD ROCm GPU | `device`: string, `formats`: list, `mem_bytes`: int, `mem_free_bytes`: int, `precisions`: list, `sdk_version`: string, `unified`: bool, `vendor`: string | - |
| `accel.gpu.vulkan` | Vulkan compute GPU | `device`: string, `formats`: list, `mem_bytes`: int, `mem_free_bytes`: int, `precisions`: list, `unified`: bool, `vendor`: string | - |
| `accel.npu.ane` | Apple Neural Engine (via CoreML; no public utilisation API, so claimed or probed until measured) | `device`: string, `formats`: list, `precisions`: list, `unified`: bool | - |
| `accel.npu.hailo` | Hailo NPU (HEF) | `device`: string, `formats`: list, `precisions`: list | - |
| `accel.npu.qualcomm` | Qualcomm NPU | `device`: string, `formats`: list, `precisions`: list | - |
| `accel.npu.rknn` | Rockchip RKNN NPU | `device`: string, `formats`: list, `precisions`: list | - |
| `accel.other.<name>` | Other accelerator class | `device`: string, `formats`: list | - |
| `accel.tpu.cloud` | Cloud TPU | `device`: string, `formats`: list, `precisions`: list | - |
| `accel.tpu.coral` | Coral Edge TPU (TFLite delegate) | `device`: string, `formats`: list | - |
| `accel.tsu.<vendor>` | Thermodynamic sampling unit, per vendor | `device`: string, `formats`: list | - |

## Model and data formats (`format`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `format.coreml` | CoreML model | - | - |
| `format.gguf` | GGUF weights | - | - |
| `format.hef` | Hailo executable format | - | - |
| `format.mlx` | MLX weights | - | - |
| `format.onnx` | ONNX model | - | - |
| `format.safetensors` | safetensors weights | - | - |
| `format.tflite` | TensorFlow Lite model | - | - |

## Memory (`mem`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `mem.system` | Host memory | `free`: int, `total`: int | - |
| `mem.unified` | GPU and NPU share the system pool: count one pool, never add accelerator memory to it | `free`: int, `total`: int | - |
| `mem.vram` | Discrete device memory (one entry per device) | `device`: string, `free`: int, `total`: int | - |

## Feeds (`feed`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `feed.esp32-csi-udp` | ESP32 CSI UDP sensor feed | `bind`: string, `lan_id`: string | - |
| `feed.http-sensor` | HTTP sensor endpoint | `url_hash`: string | - |

## Storage (`store`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `store.tier.external` | External storage (drive) | `free`: int, `mounted`: bool | - |
| `store.tier.internal` | Internal storage | `free`: int | - |

## Models (`model`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `model.present` | Model shards held on this node | `shards`: list | - |

## Measured performance (`perf`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `perf.cog.cycle_ms` | Wall time of one cog cycle on a reference feed, per cog (param cog_id) | `cog_id`: string, `value`: number | measured |
| `perf.infer.prefill_tok_s` | Inference prefill tokens per second, per model (param model) | `model`: string, `value`: number | measured |
| `perf.infer.tok_s` | Inference decode tokens per second, per model (param model) | `model`: string, `value`: number | measured |

## Trust (`trust`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `trust.tier.discovered` | Seen on the mesh, not paired | - | - |
| `trust.tier.paired` | Paired through the operator pairing window | - | - |
| `trust.tier.pinned` | Operator-pinned | - | - |

## Node class (informational) (`node`)

| Id | Summary | Attributes | Expected provenance |
|---|---|---|---|
| `node.class.arm-server` | ARM server | - | - |
| `node.class.cognitum-seed` | Cognitum Seed | - | - |
| `node.class.dev-mac` | Developer Mac | - | - |
| `node.class.other` | Other | - | - |
| `node.class.pi5` | Raspberry Pi 5 | - | - |
