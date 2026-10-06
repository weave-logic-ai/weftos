# ADR-109: Agent environment profiles and a local doctor

- **Status**: Proposed (2026-10-05)
- **Deciders**: owner
- **Builds on**: ADR-108 (node actions over the heartbeat, pairing), the daemon `[dashboard]`
  reporter, `weaver doctor` (existing doctor framework in `clawft-rpc::doctor`), ADR-103 (user
  daemon on every member machine).

## Context

WeaveLogic work runs through several agent toolchains on each member's machine: Claude Code
(`~/.claude`: settings, `CLAUDE.md`, agents, skills, commands, plugins), OpenAI Codex
(`~/.codex`: `config.toml`, `AGENTS.md`, agents, hooks, plugins), Grok (`~/.grok`:
`config.toml`, agents, skills, commands, plugins), ruflo / claude-flow (`~/.claude-flow`:
`config.yaml`, policy; the pinned CLI version), and MCP server definitions (`~/.mcp.json`, per
project `.mcp.json`). The owner's setup is the reference; members' machines drift from it and
there is no way to see or fix that except by hand. The same folders also hold credentials,
sessions, histories and caches that must never leave the machine.

## Decision (proposed)

1. **A profile is a declarative, credential-free description of an agent environment.**
   - Tool versions and install source per CLI.
   - Settings with secret-looking values removed.
   - MCP server definitions with every environment value replaced by a `${NAME}` placeholder.
   - Instruction files (`CLAUDE.md`, `AGENTS.md`), agents, skills, commands and hooks as files.
   - Plugin and marketplace lists.
   - The ruflo pin and its non-secret config.
   - The secrets each tool needs, as names only, with where the member supplies them.
2. **Capture is deny-by-default.**
   - It reads an allowlist of paths per tool. Never read: credential, auth, token and key
     files; `.env*`; sessions; histories; databases; caches; logs; browser profiles; downloads;
     `installation_id`-style machine identifiers.
   - Values under keys matching token, secret, key, password, auth or cookie are dropped.
   - A secret scanner runs over the result before it is stored. Any hit aborts the capture and
     names the file.
3. **Layers.** An org baseline (from the owner's machine, reviewed) plus optional per-member
   overrides. Personal content in instruction files stays in the member layer unless promoted.
4. **The profile store is versioned and private.** Diffs are reviewable and every apply names
   the profile version it used.
5. **The doctor runs locally and reports to the dashboard.**
   - `weaver doctor agents` compares the machine with the profile and reports findings: tool
     missing or drifted, MCP server missing, setting differs, skill/agent missing or stale,
     ruflo pin mismatch, credential stored in a config file, and others.
   - Findings are ranked error / warn / info, each with a proposed fix.
   - It also runs each tool's own health check where one exists, such as ruflo's
     `doctor`, and folds the results in.
   - The reporter sends a summary (counts, top findings, profile version) as `report.doctor`;
     the dashboard shows it on the machine's panel. No file contents are sent, only finding
     codes, paths and hashes.
6. **Apply is consented and reversible.**
   - From the dashboard, a member picks findings to fix. That queues a `doctor.apply`
     node action (ADR-108).
   - The node shows the plan (files to write, commands to run), runs it after confirmation,
     backs up every file it changes, and reports the result.
   - Apply never writes credentials. Required secrets are listed for the member to supply.

## Phases

| Phase | Delivers | Check |
|---|---|---|
| A1 Capture | `weaver agents profile capture` (allowlist, redaction, secret scan) → a profile directory; owner reviews and commits the baseline. | Capturing the owner's machine yields a profile with zero scanner hits; known secret files are absent; MCP env values are placeholders. |
| A2 Doctor | `weaver doctor agents` against a profile; findings model; includes ruflo's own doctor. | Running it on a machine with a removed skill, an old CLI and a missing MCP server reports exactly those three. |
| A3 Report | `report.doctor` in the heartbeat; dashboard machine panel shows findings. | The dashboard shows a member machine's findings within one heartbeat. |
| A4 Apply | `doctor.apply` node action with plan preview, consent, backups, rollback. | Fixing the three findings from the dashboard clears them on the next doctor run; rollback restores the backups. |

## Consequences

- One reference setup for every member, visible drift, and guided fixes.
- New sensitive surface: capture and apply touch folders that also hold credentials. They
  are therefore allowlist-only, scanned, fail closed, and covered by negative tests (planted
  fake secrets in every tool folder must never appear in a profile or a report).

## Open questions

- Where the profile store lives: a private repo (git history, review by PR) or the dashboard
  (Supabase storage, edited in the UI)?
- Are instruction files (`CLAUDE.md`, `AGENTS.md`) part of the org baseline, or member-owned with
  only a shared section managed?
- Should apply install or upgrade the CLIs themselves, or only configure tools already installed?
