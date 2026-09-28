# [39] LLaVA-Plus: Learning to Use Tools for Creating Multimodal Agents

**Citation:** Liu et al. (2024a). S. Liu, H. Cheng, H. Liu, H. Zhang, F. Li, T. Ren, X. Zou, J. Yang, H. Su, J. Zhu, et al. "Llava-plus: learning to use tools for creating multimodal agents." In European Conference on Computer Vision (ECCV 2024), pp. 126–142.
**arXiv:** https://arxiv.org/abs/2311.05437 (submitted Nov 9 2023) · Project page: https://llava-vl.github.io/llava-plus/ · Venue: ECCV 2024 (Springer LNCS, DOI 10.1007/978-3-031-72970-6_8)

## Summary
LLaVA-Plus is a general-purpose multimodal assistant that extends LLaVA with a "skill repository" of pretrained vision and vision-language models (detectors, segmenters, generators, external knowledge retrievers). Given a user's multimodal query, the model is trained end-to-end (via instruction-tuning data covering tool-use examples) to select and invoke relevant tools, then compose their outputs into a final response. Unlike LLM-based tool-use methods that only reason over text, LLaVA-Plus keeps the query image in context throughout the full interaction, which the authors report improves tool-use performance and unlocks new multimodal scenarios (e.g. chained visual generation/editing).

## Method Specifics
- **Tool API shape:** natural-language/text-based tool invocation, not JSON schemas or code generation — the model emits an `X_skill_use` token sequence naming the tool and its arguments as text.
- **Output return:** tool results (`X_skill_result`) come back as text, images, or segmentation masks and are re-inserted into the ongoing multimodal conversation for the model to continue reasoning over.
- **Planner vs executor:** no explicit planner/executor split — a single LMM interleaves tool calls and reasoning inline in one autoregressive sequence (unified single-model design).
- **Error handling/repair:** not documented in the fetched content — no evidence of a retry/self-correction/verification loop distinct from the model's own generation.

## Quantitative Results
Abstract claims new SoTA on VisIT-Bench; no specific numeric scores were found in the fetched content — treat as "not found" rather than guessed.

## Code/Weights/License
Code released on GitHub ("LLaVA-Plus-Codebase"), dataset on Hugging Face, model checkpoints and a demo also released. License: research-use-only — "data, code and checkpoint is intended and licensed for research use only," dataset under CC BY-NC-4.0; commercial deployment explicitly prohibited.

## Relation to Skill-3D
Cited at §2.2 (MLLM Agents), specifically in the sentence: "A complementary line of work trains VLMs to use tools through supervised fine-tuning or reinforcement learning Liu et al. (2024a); Wang et al. (2025a); Han et al. (2025) ..." — grouped as an example of the SFT/RL-trained-tool-use paradigm, contrasted with prompting-based tool augmentation cited earlier in the same section.

## WeftOS Relevance
**Verdict: PATTERN.** The single-model "skill repository + inline tool invocation" pattern (one LMM planning and executing text-tagged tool calls in one sequence) is architecturally simple and worth noting as a design point, but the text-tag API is less robust than typed/structured tool calls WeftOS's MCP server already uses. No metric geometry, no egocentric/robotics angle, and the non-commercial license rules out direct reuse of weights — useful only as a conceptual reference for "how much can a single model do without a separate planner."
