# [37] InsightX Agent: An LMM-based Agentic Framework with Integrated Tools for Reliable X-ray NDT Analysis

**Citation:** Liu et al. (2025b). J. Liu, H. Wang, Y. Zhang, X. Luo, J. Hu, Z. Liu, and M. Xie. "InsightX agent: an lmm-based agentic framework with integrated tools for reliable x-ray ndt analysis." arXiv preprint arXiv:2507.14899.
**arXiv:** https://arxiv.org/abs/2507.14899 (v1 Jul 20 2025, latest v3 Feb 25 2026)

## Summary
InsightX Agent targets industrial X-ray non-destructive testing (NDT), where existing deep-learning detectors lack interactivity, interpretability, and self-assessment. It positions a Large Multimodal Model (LMM) as a central orchestrator that coordinates two specialized tools: a Sparse Deformable Multi-Scale Detector (SDMSD) that proposes candidate defect regions from multi-scale X-ray feature maps, and an Evidence-Grounded Reflection (EGR) tool that walks the LMM through a chain-of-thought-style review (context assessment, per-defect analysis, false-positive elimination, confidence recalibration, quality assurance) to validate/refine SDMSD's proposals before the agent commits to a final answer.

## Method Specifics
- **Tool API shape:** structured tool invocation — the LMM calls the SDMSD detector and the EGR reflection tool as discrete modules rather than generating executable code.
- **Output return:** detector outputs are region proposals (boxes/scores) fed back as structured evidence; EGR's output is a structured multi-stage critique (context/defect/FP/confidence fields) that the LMM consumes to revise its judgment.
- **Planner vs executor:** single LMM acts as the central orchestrator/planner; SDMSD and EGR are separate specialist executors it calls, not separate planning models.
- **Error handling/repair:** EGR is explicitly a self-verification loop — it re-examines SDMSD's raw proposals, eliminates false positives, and recalibrates confidence before finalizing, i.e. a built-in reflection/repair stage rather than blind trust in the first detector pass.

## Quantitative Results
GDXray+ dataset: object-detection F1-score of 96.35% (one WebFetch summary reported 96.54%; treat the exact digit as unverified pending direct read of the abstract — F1 in the mid-90s range is the reliable takeaway). No other benchmark numbers found in the fetched content.

## Code/Weights/License
Not found — no code or dataset repository link was located in the fetched abstract/summary content; license unverifiable.

## Relation to Skill-3D
Cited at §2.2 (MLLM Agents) in a list of "Recent tool-augmented VLM agents ... developed for long-video understanding, high-resolution image analysis, medical diagnosis, and general visual reasoning" alongside Lyu et al. (2025) and others — i.e. grouped as an example of domain-specific tool-augmented VLM agents, not analyzed individually beyond that citation cluster.

## WeftOS Relevance
**Verdict: SKIP.** Domain is industrial X-ray NDT, not egocentric/3D spatial reasoning; its single useful transferable idea — a structured self-verification/reflection tool that recalibrates confidence on detector output — is a generic pattern already covered by better-documented systems. No metric geometry, no code availability confirmed, no robotics/egocentric relevance to MentraOS capture.
