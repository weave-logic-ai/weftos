# [21] He et al. (2026) — OpenClaw as Language Infrastructure: A Case-Centered Survey of a Public Agent Ecosystem in the Wild

**Citation:** C. He, X. Zhou, D. Wang, H. Xu, W. Liu, C. Miao. *OpenClaw as language infrastructure: a case-centered survey of a public agent ecosystem in the wild.* 2026.
**Source:** https://www.preprints.org/manuscript/202603.1060 — **primary source returned HTTP 403 (access blocked); this entry is built from third-party search snippets only and is not independently verified against the paper's own text.**

## Summary (unverified — from search-result snippets)
OpenClaw is described elsewhere in the surrounding literature (per WebSearch snippets, not this paper directly) as an open-source, self-hosted AI agent **gateway middleware** released in late 2025: a single persistent gateway process on user-owned hardware that connects heterogeneous messaging channels (Discord, Telegram, WhatsApp) to LLM-driven agent backends, with native tool invocation, a **four-tier memory hierarchy**, multi-agent routing, and dynamic capability expansion via a modular **Skills and plugin system**. This specific paper (He et al. 2026) is characterized in search snippets as a "case-centered survey" treating OpenClaw's growth (reportedly one of the most-starred GitHub projects in its category) as evidence that agents now run continuously, across heterogeneous platforms, using community-contributed skills **outside fully curated environments** — i.e., breaking the sandboxed-evaluation assumptions common in prior agent research.

## Method (unverified)
Described as a **case-centered survey** — i.e., empirical/qualitative analysis of a real, in-the-wild deployed agent ecosystem (OpenClaw's actual user base and skill/plugin marketplace) rather than a new method or benchmark. No method-level details (sampling approach, number of cases, coding scheme) could be confirmed from the sources available.

## Results
**Not found.** No quantitative results were retrievable from the primary source (blocked) or from search snippets.

## Code / License
**Not found** for this survey paper itself. (OpenClaw the software project is separately known to be open source, but that is not the same as this paper's own data/code availability, which was not confirmed.)

## How Skill-3D uses it
Cited in §2.3's first cluster — "skills distilled from historical interactions" (Xu and Yan 2026 [78], Li et al. 2026a [29], He et al. 2026 [21], Yang et al. 2026 [83]). Given the paper is a survey of a real-world deployed agent ecosystem rather than a lab method, it most likely supports Skill-3D's framing that skill-based reuse is now an established, real-world pattern for LLM agents — not just a research construct — reinforcing the motivation for the paper's own skill-library approach.

## WeftOS Relevance — Verdict: **WATCH (needs re-verification)**
If the search-snippet characterization is accurate, OpenClaw's **four-tier memory hierarchy** and its **modular Skills/plugin system operating outside curated evaluation environments** are directly relevant precedents for WeftOS's own goal of shipping skills to Claude Code, Grok, and Codex in the wild — a live, large-scale example of exactly the "skills delivered to real heterogeneous agent runtimes" problem WeftOS is solving for. However, because the primary source could not be read (403 Forbidden) and every claim above comes from search-engine snippets about OpenClaw generally rather than confirmed content of this specific paper, **do not cite specific numbers or architectural claims from this entry without re-fetching the primary source** (try an alternate mirror, Google Scholar cache, or requesting the PDF directly) before using it in any downstream WeftOS design decision.
