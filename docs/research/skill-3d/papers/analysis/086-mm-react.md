# MM-REACT: Prompting ChatGPT for Multimodal Reasoning and Action

**Citation:** Z. Yang, L. Li, J. Wang, K. Lin, E. Azarnasab, F. Ahmed, Z. Liu, C. Liu, M. Zeng, and L. Wang. "MM-REACT: Prompting ChatGPT for Multimodal Reasoning and Action." arXiv preprint arXiv:2303.11381, 2023.

**arXiv:** 2303.11381 — https://arxiv.org/abs/2303.11381 (submitted March 20, 2023). Microsoft Azure AI. Project page: https://multimodal-react.github.io/, code: https://github.com/microsoft/MM-REACT.

## Summary
MM-REACT is an early (2023) system pairing ChatGPT with a pool of specialized "vision expert" APIs (e.g. Azure Computer Vision, Form Recognizer, Bing Search) to give a text-only LLM multimodal reasoning and action capability. It works purely through prompting — no fine-tuning — using a textual prompt design that can represent text descriptions, textualized spatial coordinates, and file-path placeholders for images/video, letting ChatGPT decide when to invoke a vision expert and how to combine multiple expert outputs (e.g. summing totals across several receipt images).

## Method specifics
- **Tool API shape:** natural-language/ReAct-style prompting, not structured JSON function calls and not code execution. ChatGPT is prompted to "seek help from a specific vision expert" within its generated text; image/video inputs are referenced via file-path placeholders rather than passed as raw pixels to the LLM.
- **Output return to model:** vision-expert outputs are **serialized to text** and appended to the running prompt/context to "further activate ChatGPT" for the next reasoning step — a pure text-in/text-out loop, no image re-insertion.
- **Planner vs executor:** single ChatGPT instance does both planning (deciding which expert to call) and final answer synthesis; the vision experts are external, fixed (non-agentic) tools, not a second reasoning model.
- **Error handling/repair:** none documented — no retry, verification, or self-correction loop described in available material.

## Key quantitative results
Not found — the abstract reports only qualitative zero-shot demonstrations (e.g., multi-receipt travel-cost calculation); no benchmark table was located in the accessible summary.

## Code/weights availability and license
Code, demo, video, and visualizations released at https://github.com/microsoft/MM-REACT and https://multimodal-react.github.io/. Repository license: MIT (per GitHub page).

## Skill-3D's citation
Cited at §2.1, in the sentence: "Multimodal Large Language Models (MLLMs) have shown growing capability in spatial reasoning, driven by stronger backbones Yang et al. (2023) [MM-REACT]; Wake et al. (2024); Shao et al. (2024a); ..." — listed as an early example of the backbone/capability lineage that enabled downstream spatial-reasoning MLLMs, not discussed for its tool mechanics specifically.

## WeftOS relevance
**Verdict: PATTERN.** MM-REACT is the historical ancestor of the "LLM orchestrates external perception tools via text" pattern that WeftOS's MCP-exposed tool architecture also implements (structured now, rather than free-text). No metric geometry is produced or claimed — all vision-expert outputs are appearance/text descriptions, consistent with "honest geometry" concerns only insofar as it shows the failure mode (unstructured text tool-calling) WeftOS's typed MCP tool schema is designed to avoid.
