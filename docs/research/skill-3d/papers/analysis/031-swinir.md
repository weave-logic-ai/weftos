# [31] SwinIR: Image Restoration Using Swin Transformer

**Citation:** Liang et al. (2021). J. Liang, J. Cao, G. Sun, K. Zhang, L. Van Gool, R. Timofte. "Swinir: image restoration using swin transformer." Proceedings of the IEEE/CVF International Conference on Computer Vision (ICCVW), pp. 1833–1844.
**URL:** https://arxiv.org/abs/2108.10257 (code: https://github.com/JingyunLiang/SwinIR)

## Summary
SwinIR (2021) is a strong, now-classic baseline for image restoration — super-resolution, denoising, and JPEG-artifact reduction — built on the Swin Transformer. It uses shallow convolutional feature extraction, a deep feature-extraction stack of Residual Swin Transformer Blocks (RSTB, each a stack of Swin Transformer layers plus a residual connection), and a lightweight reconstruction head, beating prior CNN/GAN restoration SOTA by up to 0.14–0.45 dB while cutting parameters up to 67%.

## Architecture / I/O
Input: a single degraded RGB image (low-res, noisy, or compressed). Output: a restored/upscaled RGB image at the same or higher resolution. Purely 2D image-to-image; no depth, pose, or 3D output — not a metric-geometry tool at all, it is an image-quality preprocessor.

## Sizes / Variants
Classical SR (×2/×3/×4), lightweight SR, real-world SR, grayscale/color denoising, and JPEG-artifact-reduction checkpoints — roughly a dozen task-specific pretrained weights, ranging from ~1M params (lightweight) to ~12M params (classical/real-world). No "small/base/large" naming scheme; variants are per-task, not per-capacity.

## Licenses
- **Code:** Apache-2.0 (confirmed via repo license search).
- **Weights:** released alongside the code under the same repository; no separate restrictive weight license found — treat as Apache-2.0-compatible, but verify the specific checkpoint's README before commercial redistribution.

## ONNX / Rust
No confirmed official ONNX export, but SwinIR is architecturally a standard Swin-Transformer-based encoder-decoder, and ONNX export from the reference PyTorch code is a well-trodden community path (multiple third-party exports exist, unverified for correctness/parity). No `candle`/`burn`/`ort` first-party port found; a Swin Transformer block is implementable in `candle` (windowed attention + shifted windows) but nobody has published a ready SwinIR crate as of this check.

## Performance / Hardware
No official VRAM/latency numbers in the paper (2021, benchmarked on older GPUs, not directly comparable). Not separately measured in Skill-3D's efficiency table (Appendix B); it is grouped implicitly as one of the cheaper 2D tools. Given its small parameter count, CPU or Apple Silicon MPS inference is plausible with acceptable latency, but unverified here.

## Use in Skill-3D
Cited at §4.1 (`Liang et al. (2021)`) as one of the expert tools available to the agent — used as an image-quality/upsampling preprocessor to clean egocentric frames before feeding them to detection/depth/orientation experts, though the paper does not give SwinIR its own ablation line.

## WeftOS Relevance
**Verdict: PATTERN.** SwinIR itself is dated (2021) and easily supplanted by newer restoration models, but its RSTB pattern (windowed Swin attention + residual blocks for dense image-to-image prediction) is a reusable architectural template worth knowing when designing a Rust-native preprocessing stage for MentraOS glasses frames (motion blur, low light, compression artifacts from a single head-mounted RGB camera). Low priority to adopt verbatim; higher priority to note as the canonical "Swin-for-restoration" reference architecture if WeftOS ever needs a bespoke on-device restoration net.
