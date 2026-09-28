# Ref 71 — Wu et al. (2023), Visual ChatGPT

**Citation:** C. Wu, S. Yin, W. Qi, X. Wang, Z. Tang, and N. Duan. "Visual ChatGPT: Talking, Drawing and Editing with Visual Foundation Models." arXiv preprint arXiv:2303.04671.

**Source:** arXiv:2303.04671, https://arxiv.org/abs/2303.04671 (submitted Mar 8 2023). Code: https://github.com/microsoft/visual-chatgpt (repository since renamed/moved to chenfei-wu/TaskMatrix).

## Summary
Visual ChatGPT is one of the earliest systems to bolt a large set of specialized "Visual Foundation Models" (VFMs) — captioning, text-to-image (Stable Diffusion), inpainting, edge/pose detection, etc. — onto ChatGPT so users can converse in mixed text-and-image turns: send an image, ask ChatGPT to describe, generate, or edit it, and get an image back in the same conversation. Because ChatGPT itself is text-only, the system's core contribution is a prompting scheme ("Prompt Manager") that lets a frozen LLM orchestrate many heterogeneous, multi-input/multi-output visual models without any of them being retrained.

## Method specifics
- **Tool API shape:** Neither free code generation nor JSON function-calls in the modern sense — it uses natural-language "Templates," pre-defined execution flows/prompt patterns that tell ChatGPT how to sequence and invoke foundation models (some templates chain multiple models or spin up a fresh ChatGPT session for sub-tasks).
- **Return path:** Visual model outputs (generated/edited images) are sent back into the conversation directly as images, which the user sees and can further reference in the next turn; textual descriptions of image content are also injected into ChatGPT's prompt so the text-only LLM can "see" images by proxy.
- **Planner/executor split:** ChatGPT acts as the general planner/interface ("System Principle" + "Prompt Manager"), deciding which foundation model(s) to call; each VFM is a fixed, non-reasoning executor specialized to one operation (caption, generate, inpaint, detect edges, etc.).
- **Error handling:** Not found — no explicit retry/self-correction loop is documented in the fetched abstract/summary; robustness relies on prompt engineering and template design rather than runtime verification.

## Results
No quantitative benchmark results were found in the fetched abstract/summary — this is presented primarily as a system/demo paper rather than a benchmarked one; treat any performance claim as not found rather than guessed.

## Code / license
Open-sourced at github.com/microsoft/visual-chatgpt (now chenfei-wu/TaskMatrix). GitHub reports the repository's license as "Other" (a LICENSE.txt is present but not a standard SPDX-recognized license) — do not assume MIT/Apache without reading LICENSE.txt directly.

## Relation to Skill-3D
Cited at Skill-3D §2.2, opening sentence of the tool-augmentation paragraph: "Tool augmentation extends MLLM by allowing them to invoke external modules through prompting, structured APIs, or code generation. Representative systems demonstrate that external tools can compensate for limitations of end-to-end multimodal models Shen et al. (2023); Wu et al. (2023) [= this paper]; Surís et al. (2023)." — cited as a foundational/representative prompting-based tool-augmentation system, not a training-based one.

## WeftOS relevance
**PATTERN.** Historically important as the template that established "LLM-as-orchestrator over frozen specialist tools returning media back into the chat," which is structurally close to how WeftOS MCP exposes tools to Claude Code/Grok/Codex — worth citing as prior art for the orchestration pattern, not for adoption of its 2023-era prompt-template mechanics. No metric geometry and no egocentric/robotics angle; general image generation/editing only, so SKIP-adjacent beyond the orchestration-pattern citation.
