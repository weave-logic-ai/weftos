# Ref 64 — Wang et al. (2025a), MLLM-Tool

**Citation:** C. Wang, W. Luo, S. Dong, X. Xuan, Z. Li, L. Ma, and S. Gao (full author list per venue: C. Wang, W. Luo, Q. Chen, H. Mai, J. Guo, S. Dong, X. Xuan, Z. Li, L. Ma, S. Gao). "MLLM-Tool: A Multimodal Large Language Model for Tool Agent Learning." In *2025 IEEE/CVF Winter Conference on Applications of Computer Vision (WACV)*, pp. 6678–6687.

**Source:** WACV 2025 (Tucson, AZ, Feb 26–Mar 6 2025); originally posted as arXiv:2401.10727 (v2). Proceedings PDF: https://openaccess.thecvf.com/content/WACV2025/papers/Wang_MLLM-Tool_A_Multimodal_Large_Language_Model_for_Tool_Agent_Learning_WACV_2025_paper.pdf. IEEE Xplore: 10943671. Code: https://github.com/MLLM-Tool/MLLM-Tool.

## Summary
MLLM-Tool addresses a gap in earlier text-only tool-agent LLMs: real user instructions are often multimodal (an image/audio clip plus a short, ambiguous text query), and text-only tool selectors frequently pick the wrong tool because they cannot see the accompanying media. MLLM-Tool combines ImageBind (a unified multimodal encoder covering six modalities) with an open-source LLM backbone (Vicuna/LLaMA/LLaMA2/LLaMA2-Chat) so the model can condition tool selection on the actual image/audio input, not just the text. The authors build ToolMMBench, a benchmark scraped from HuggingFace model cards, specifically constructed so that many instructions have multiple plausible tools (identical or synonymous functions), stress-testing disambiguation.

## Method specifics
- **Tool API shape:** Structured tool *selection*, not code generation — the model outputs a single tool identifier/name from a fixed catalog of HuggingFace-hosted tools; it does not synthesize or execute code.
- **Return path:** Not found in detail — the task is framed as tool recommendation (classification-style output) rather than a full call-execute-observe loop with results fed back in.
- **Planner/executor split:** Single model does both understanding and tool selection in one forward pass; no separate planner/executor or multi-turn dispatch described.
- **Error handling:** Not found — no retry, verification, or self-correction mechanism described; evaluation is single-shot tool-recommendation accuracy.

## Results
Benchmark: ToolMMBench (custom, HuggingFace-derived, multimodal instructions with synonymous-tool ambiguity). Exact accuracy numbers were not retrievable from the fetched abstract/summary content — mark as not found; the abstract confirms qualitatively that the model "is capable of recommending appropriate tools for multi-modal instructions" and improves over text-only tool selectors.

## Code / license
Code and ToolMMBench data released at github.com/MLLM-Tool/MLLM-Tool. License: not found (not verified via GitHub API in this pass).

## Relation to Skill-3D
Cited at Skill-3D §2.2: "A complementary line of work trains VLMs to use tools through supervised fine-tuning or reinforcement learning Liu et al. (2024a); Wang et al. (2025a) [= this paper]; Han et al. (2025); Tang et al. (2025b); Wu et al. (2024); ..." — grouped as a training-based (SFT) tool-agent approach, contrasted with Skill-3D's prompted skill-library mechanism.

## WeftOS relevance
**PATTERN.** The core problem MLLM-Tool solves — text-only tool selectors misfiring because they can't see the accompanying image — is directly applicable to WeftOS MCP tool routing for Claude/Grok/Codex when image/scene context is present; worth reusing the "condition tool choice on multimodal input, not just text" principle. It does not touch metric geometry or egocentric capture, so it's tool-routing pattern only, not architecture to adopt wholesale.
