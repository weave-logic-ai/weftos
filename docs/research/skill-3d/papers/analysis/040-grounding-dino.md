# [40] Grounding DINO: Marrying DINO with Grounded Pre-Training for Open-Set Object Detection

**Citation:** Liu et al. (2024b). S. Liu, Z. Zeng, T. Ren, F. Li, H. Zhang, J. Yang, Q. Jiang, C. Li, J. Yang, H. Su, et al. "Grounding dino: marrying dino with grounded pre-training for open-set object detection." European Conference on Computer Vision (ECCV), pp. 38–55.
**URL:** https://arxiv.org/abs/2303.05499 (code: https://github.com/IDEA-Research/GroundingDINO)

## Summary
Grounding DINO extends the closed-set DETR-style detector DINO with a text encoder and grounded pre-training, enabling open-set object detection from arbitrary text prompts (category names or free-form referring expressions). It reports 52.5 AP zero-shot on COCO without any COCO training data, and became a standard building block (e.g. paired with SAM in "Grounded-SAM") for text-to-box grounding pipelines.

## Architecture / I/O
Dual-encoder (image + text) with cross-modality fusion at multiple stages, feature enhancer, language-guided query selection, and a cross-modality decoder — a Transformer detector conditioned on language. Input: RGB image + text phrase(s). Output: 2D bounding boxes (+ confidence, matched phrase) per detected instance. No depth/pose output — purely 2D open-vocabulary localization, feeds downstream 3D tools rather than producing geometry itself.

## Sizes / Variants
Swin-T (tiny) and Swin-B (base) backbone variants are the two officially released checkpoints; Swin-T is the commonly deployed lightweight option (~172M params total pipeline-wide is a rough community estimate, unverified precisely here).

## Licenses
- **Code & weights:** **Apache-2.0** (confirmed).

## ONNX / Rust
Official and community ONNX exports are widely available and mature (Grounding DINO is one of the most frequently ONNX-exported open-vocab detectors; HF Transformers also ships a `GroundingDinoForObjectDetection` class with export support). No first-party `candle`/`burn` port found by name, but its DETR-style decoder plus a text encoder (typically BERT) are both well-represented individually in `candle-transformers` (DETR-family and BERT are both implemented there), meaning a Rust port is assembly work rather than research — moderate effort, not "does not exist."

## Performance / Hardware
Not separately isolated in Skill-3D's efficiency table, but grouped with the fast expert tools (segmentation/depth/orientation each ≤~1.5s; Pi3 reconstruction is the outlier at ~21s). Original paper reports GPU inference in the tens-of-ms to low-hundreds-of-ms range per image on datacenter GPUs (not independently re-verified here). Given mature ONNX exports and Swin-T's modest size, real-time-ish Apple Silicon inference via `ort`+CoreML EP is plausible.

## Use in Skill-3D
Cited at §4.1 (`Liu et al. (2024b)`) as the object-localization expert. Fig. 4 shows non-agentic/Think3D baselines "overuse" GroundingDINO by defaulting to it for nearly every task; Skill-3D instead keeps it as one of several experts used specifically for "object localization" and layout grounding, reserving DA3/Orient-Anything for their more specialized roles.

## WeftOS Relevance
**Verdict: ADOPT.** Apache-2.0 license, mature ONNX tooling, and component parts already present in `candle-transformers` make this the lowest-friction expert in the cluster to bring into a Rust-native WeftOS pipeline — either via `ort` on the official ONNX export short-term, or a native `candle` port assembled from existing DETR+BERT building blocks longer-term. Directly useful for egocentric glasses capture: text-prompted "find the mug / find the door" grounding is a natural fit for a voice-directed spatial agent.
