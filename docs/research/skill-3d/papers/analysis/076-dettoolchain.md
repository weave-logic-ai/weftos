# Ref 76 — Wu et al. (2024), DetToolChain

**Citation:** Y. Wu, Y. Wang, S. Tang, W. Wu, T. He, W. Ouyang, P. Torr, and J. Wu. "DetToolChain: A New Prompting Paradigm to Unleash Detection Ability of MLLM." In *European Conference on Computer Vision (ECCV)*, pp. 164–182.

**Source:** ECCV 2024 (published version DOI 10.1007/978-3-031-73411-3_10); originally posted as arXiv:2403.12488, https://arxiv.org/abs/2403.12488. Code: https://github.com/yixuan730/DetToolChain.

## Summary
DetToolChain is a training-free prompting framework that unlocks zero-shot object-detection ability in general-purpose MLLMs (GPT-4V, Gemini) without any fine-tuning. It packages a "detection prompting toolkit" of visual-processing prompts inspired by classic high-precision detection priors — e.g. zoom-in region crops, overlaying measurement rulers/compasses on the image, overlaying scene graphs for context — together with a detection-specific chain-of-thought that decomposes a detection task into subtasks, diagnoses the current prediction, and plans progressive bounding-box refinements.

## Method specifics
- **Tool API shape:** Structured visual-prompting operations (a fixed toolkit of image-overlay/crop actions selected and sequenced via chain-of-thought prompts), not free code generation and not opaque JSON function calls — closer to a curated menu of visual prompt templates the MLLM chooses from at each step.
- **Return path:** Tool outputs are modified images (zoomed crops, ruler/compass overlays, scene-graph overlays) fed back into the MLLM's visual input for the next reasoning step; the model's textual predictions (box coordinates, diagnoses) are interleaved with these images across the chain.
- **Planner/executor split:** Single MLLM performs both planning (deciding which detection prompt to apply next) and "execution" (reading overlaid rulers/compasses to estimate coordinates) — the toolkit and chain-of-thought scaffold the same model rather than splitting into separate planner/executor components.
- **Error handling:** Explicit self-diagnosis/refinement loop — the detection chain-of-thought lets the MLLM "diagnose the detection results and reason the next prompts to be applied," i.e., an iterative box-refinement loop driven by the model's own critique of its prior prediction rather than an external verifier.

## Results
GPT-4V + DetToolChain over baseline state-of-the-art detectors: +21.5% AP50 on MS-COCO Novel class set (open-vocabulary detection); +24.23% accuracy on RefCOCO val set (zero-shot referring expression comprehension); +14.5% AP on D-cube (describe object detection, FULL setting).

## Code / license
Code released at github.com/yixuan730/DetToolChain (installation + `python main.py` demo confirmed present). GitHub API reports the repository has no declared license (license: null) — treat as all-rights-reserved / unlicensed absent a LICENSE file; do not assume permissive reuse.

## Relation to Skill-3D
Cited at Skill-3D §2.2: "A complementary line of work trains VLMs to use tools through supervised fine-tuning or reinforcement learning ... Wu et al. (2024) [= this paper] ..." — note this is arguably a mis-grouping by Skill-3D, since DetToolChain is training-free/prompting-based (like Visual ChatGPT), not an SFT/RL-trained tool agent; it is nonetheless listed alongside MLLM-Tool, Tang et al. (2025b), and VTool-R1 in that citation cluster.

## WeftOS relevance
**PATTERN.** The self-diagnose-then-refine loop (read current box estimate, critique it, choose the next corrective visual prompt) is a strong, directly reusable pattern for WeftOS tool-calling agents doing iterative visual grounding. The ruler/compass overlays used to read off coordinates are appearance-based pixel-space estimates rendered onto a 2D image, not calibrated real-world measurements — a clear "honest geometry" violation risk if WeftOS ever treats such overlay-read coordinates as metric without an actual depth/calibration pipeline. No robotics/egocentric focus (static image detection benchmarks only).
