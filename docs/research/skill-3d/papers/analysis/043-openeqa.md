# [43] Majumdar et al. (2024) — OpenEQA: Embodied Question Answering in the Era of Foundation Models

**Citation:** A. Majumdar, A. Ajay, X. Zhang, P. Putta, S. Yenamandra, M. Henaff, S. Silwal, P. Mcvay, O. Maksymets, S. Arnaud, et al. In CVPR 2024, pp. 16488–16498.
**Venue/ID:** CVPR 2024 (Meta/FAIR). No arXiv preprint located under a resolvable ID — CVPR openaccess PDF: openaccess.thecvf.com/content/CVPR2024/papers/Majumdar_OpenEQA...pdf. Code: github.com/facebookresearch/open-eqa (MIT license; **repo archived/read-only as of 2025-11-01**).

## Summary
OpenEQA formulates Embodied Question Answering (EQA) as: understand an environment well enough to answer natural-language questions about it, via either episodic memory (e.g. smart-glasses-style prior observation) or active exploration (mobile robot). It is framed explicitly around egocentric/wearable capture as one of its two core use cases — directly relevant to a MentraOS glasses capture pipeline.

## Specifics
- **Task types:** open-vocabulary, free-form natural-language QA over an environment, seven question categories (object recognition, attribute, spatial understanding, functional reasoning, etc.), two settings — episodic-memory EQA (glasses-style) and active-exploration EQA (robot-style).
- **Data sources:** real-world environments including HM3D scenes; questions authored by humans grounded in real home/office environments.
- **Size:** >1,600 human-generated questions across >180 real-world environments.
- **Metrics:** "LLM-Match" — an automatic GPT-4-powered evaluation protocol scored against human judgement for correlation, rather than exact-match string scoring (necessary because answers are open-ended, not multiple-choice).
- **Ground-truth provenance:** human-authored questions and reference answers, grounded in the specific captured environment.
- **Known flaws:** LLM-as-judge introduces its own bias/variance; not independently corroborated from the fetched material whether this has been challenged in follow-up work.
- **License:** MIT for the code repo; data license not explicitly stated in the fetched pages.

## Key results
State-of-the-art foundation models (incl. GPT-4V-era models) lag well behind human-level performance on OpenEQA at time of publication.

## How Skill-3D uses it
Cited only in the related-work list of "dedicated benchmarks" for spatial reasoning (paper-2606.07436.txt line 147) — not used in Skill-3D's own evaluation suite (VSI-Bench/BLINK/CV-3D/MMSI-Bench only).

## WeftOS relevance — Verdict: ADOPT (as the egocentric-glasses eval anchor)
OpenEQA is the closest existing benchmark to WeftOS's MentraOS smart-glasses use case (it literally frames episodic-memory EQA around glasses-style capture) and is open-vocabulary/free-form rather than multiple-choice, testing real deployed-agent behavior rather than a multiple-choice proxy. The repo being archived (read-only, not actively maintained) means WeftOS should fork/vendor the eval harness rather than depend on upstream updates. This is the strongest candidate basis for a WeftOS-native egocentric-glasses eval set — see [[papers/benchmarks-and-rl]] for the proposed glasses eval-set design.
