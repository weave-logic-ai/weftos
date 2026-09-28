# [78] Xu and Yan (2026) — Agent Skills for Large Language Models: Architecture, Acquisition, Security, and the Path Forward

**Citation:** R. Xu, Y. Yan. *Agent skills for large language models: architecture, acquisition, security, and the path forward.* arXiv:2602.12430. Accepted at Agent Skills '26 Workshop, ACM Conference on AI and Agentic Systems 2026.
**Source:** https://arxiv.org/abs/2602.12430

## Summary
A survey paper (not a new method) defining "agent skills" as **composable packages of instructions, code, and resources** that extend agent capability without retraining, and organizing the field's architecture, acquisition, deployment, and security concerns. Notably centers on the **SKILL.md specification** and **progressive context loading** — i.e., this survey is explicitly describing the same "skill as a file/folder loaded into an agent's context" paradigm WeftOS itself is targeting for Claude Code/Grok/Codex delivery, not just an abstract research construct.

## Method / Content
Organized in four dimensions:
1. **Architectural foundations** — the SKILL.md spec, progressive/lazy context loading, and how skills complement (rather than replace) MCP (Model Context Protocol) tool servers.
2. **Skill acquisition** — RL with skill libraries, autonomous skill discovery (cites SEAgent), compositional skill synthesis (overlaps with [26] Agentic Proposing).
3. **Deployment at scale** — the computer-use agent stack, GUI grounding, and benchmarking on OSWorld / SWE-bench.
4. **Security & governance** — vulnerability analysis of real, community-contributed skills, and a proposed trust/lifecycle framework.

## Results
- Key empirical finding: **26.1% of community-contributed skills contain vulnerabilities.**
- Proposes a **four-tier, gate-based permission model** (Skill Trust and Lifecycle Governance Framework) linking skill provenance to graduated deployment capability — untrusted skills get fewer permissions until vetted.

## Code / License
GitHub: `scienceaix/agentskills`. License: **CC BY-NC-ND 4.0** (non-commercial, no-derivatives — note this restricts reuse of the survey's own text/figures, not the surveyed skill format itself).

## Relation to Skill-3D
Cited in §2.3's first cluster ("skills distilled from historical interactions"). It's a survey rather than a competing method, so Skill-3D cites it as context for the general agent-skills paradigm rather than as a direct architectural precedent — but its SKILL.md-and-progressive-loading framing is exactly the delivery format Skill-3D's own dynamic skills would need to be packaged in to reach a Claude-Code-style consumer.

## WeftOS Relevance — Verdict: **ADOPT (security governance)**
This is the single most directly actionable reference for the "skills delivered to Claude Code, Grok and Codex" half of the WeftOS brief. Two concrete takeaways: (1) the **26.1% vulnerability rate in community skills is a hard number to plan around** — any WeftOS skill-ingestion path that accepts externally-sourced skills (not just self-generated ones) needs a scanning/review gate before trust, not optional hardening; (2) the **four-tier gate-based permission model tied to skill provenance** maps directly onto WeftOsecurity's existing governance-gate architecture (clawft-governance-specialist) and should be the reference design when wiring skill trust levels into that gate backend, rather than inventing a permission scheme from scratch.
