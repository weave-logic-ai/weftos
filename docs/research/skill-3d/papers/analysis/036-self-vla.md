# [36] Liu et al. (2026a) — SELF-VLA: A Skill Enhanced Agentic Vision-Language-Action Framework for Contact-Rich Disassembly

**Citation:** C. Liu, S. Tian, X. Liang, M. Zheng. *SELF-vla: a skill enhanced agentic vision-language-action framework for contact-rich disassembly.* arXiv:2603.11080.
**Source:** https://arxiv.org/abs/2603.11080

## Summary
SELF-VLA targets robotic disassembly of end-of-life electronics — a physically contact-rich, long-horizon manipulation domain where pure end-to-end vision-language-action (VLA) models fail because they need extensive per-task retraining. It injects **explicit disassembly skills** into a VLA model rather than relying purely on learned end-to-end policies.

## Method
- **Architecture:** a VLA backbone (vision + language + action) augmented with an explicit skill layer that encodes known disassembly sub-routines (e.g., screw removal, clip release, prying sequences), rather than learning each variant from scratch.
- Targeted at "complex, contact-rich, long-horizon" manipulation where sequential precision matters — explicit skills act as a structural prior on top of the learned policy.

## Results
- Significantly outperforms state-of-the-art end-to-end VLA baselines on **two contact-rich disassembly tasks** (specific numbers not available from the abstract).

## Code / License
No code/license information found in the fetched content.

## Relation to Skill-3D
Cited in §2.3's second cluster (skills as procedural memory for decision-time guidance). It is the only reference in the whole 18-item cluster grounded in **physical robotic manipulation** rather than digital/tool-use agents — evidence that the "explicit skill as structural prior over an end-to-end policy" pattern generalizes across modalities, from digital tool-use (most of this cluster) to physical contact-rich manipulation.

## WeftOS Relevance — Verdict: **SKIP**
Domain (robotic disassembly manipulation) has no direct bearing on a Rust agent-OS skill library for digital tool-use / spatial reasoning / smart-glasses capture. The only transferable idea — "inject explicit skills as a structural prior rather than relying purely on an end-to-end learned policy" — is already the premise of every other reference in this cluster and of Skill-3D itself, so this reference adds no new design information for WeftOS beyond confirming the pattern's cross-domain validity.
