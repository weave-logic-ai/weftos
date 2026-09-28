# Team bus: Codex and command-host adapters — review

Review of the Codex and generic command-host adapters on Ruflo's host-agnostic team bus (follow-up to ruvnet/ruflo PR #3512, ADR-402), plus the WeftOS side that joins a team as a `weft` command host. Design: [team-bus-codex-weftos-design.md](team-bus-codex-weftos-design.md).

Date: 2026-09-28. Reviewer: team-reviewer.

## What was reviewed

| Repo | Branch | Base | Reviewed head |
|---|---|---|---|
| Ruflo (`~/dev/ruflo-wt-team-bus-hosts`) | `feat/team-bus-codex-weftos` (pushed to the fork) | `a0f8cd71e` | `ed38fe065`, then `83f82f2c5` (reviewer fix) |
| WeftOS (`~/weftos-wt-team-bus`) | `feat/team-bus-participant` (not pushed) | `7bae6d40` | `accb0086` |

The review ran in three rounds. Round 1 was against the builder's first local rebase (`c395f93c1`), round 2 against `e3ca55c45`, and the final pass against `ed38fe065`. The builder rebased twice as the base PR moved (`fef3d8d55` → `9f0f8f4ca` → `a0f8cd71e`).

## User rulings, checked

| Ruling | Result |
|---|---|
| Generic command host; no named weftos host in Ruflo | Met. Built-ins are `grok`, `claude`, `codex`. Any other label comes from the project's `.claude-flow/team-hosts.json` through the command adapter. The hygiene grep finds no `weft`/`weftos`/`clawft` in the Ruflo diff. |
| Probe model turns opt-in only | Met. `probe-host-live.mjs` without `--execute`/`--live` starts no model process. All no-model probes exit 0 on `a0f8cd71e`. |
| Codex SubagentStop hook ON by default in `init --codex` (project `.codex/hooks.json`), with an opt-out | Met as of `e3ca55c45`. A real `ruflo init --codex` writes the entry and prints the `/hooks` trust step. `--no-team-hooks` writes no `hooks.json`. The hook merge does not touch `~/.codex/config.toml`. |
| `codex exec --json` stop parsing stays the main path | Met. `ruflo team run` takes the outcome from `turn.completed`/`turn.failed` plus the exit code. The hook is a second path, de-duplicated by `runId`. |
| One ADR-402 covering generic, Codex and Grok; no ADR-320 reference to our ADR | Met. `git diff a0f8cd71e | grep '^+.*ADR-320'` is empty. The ADR-320 references left in the tree are upstream's own (ChannelGuard and others). |
| WeftOS-only Grok `subagent_type` names dropped | Met. Ruflo's Grok plan is `{contract, spawn{description, background, isolation}, advisory}`. The advisory `subagent_type` values are the canonical `explore`/`plan`/`general-purpose`, the same as base. The WeftOS `.grok/skills/agent-teams-grok/SKILL.md` now uses the canonical template. |

## Spawn plans vs the real CLIs

Checked against `codex --help`, `codex exec --help` (codex-cli 0.157.1) and `weft --help` / `weft agent --help`.

- Codex plan: `exec --sandbox read-only|workspace-write [--worktree] --skip-git-repo-check --json -o {resultFile} -C {cwd} -c mcp_servers.<srv>.env.CLAUDE_FLOW_CWD="{teamRoot}" [-m model] -`. Every flag exists. There is no `--full-auto` (absent in 0.157) and no `--dangerously-bypass-*`. The prompt goes over stdin and stdin is then closed.
- `weft` host: `weft agent -m {prompt}`. `-m/--message` is the non-interactive single-message flag. No other flags are used.
- The WeftOS guide's `weft mcp add ruflo --env … --internal-only=false -- node …/cli.js mcp start` matches `weft mcp add --help`. `CLAUDE_FLOW_MCP_TOOLS` is honored by Ruflo's `mcp start`.

## Defects

| # | Severity | Where | Finding | Status |
|---|---|---|---|---|
| 1 | High | Ruflo branch | The branch was based on `fef3d8d55` and edited ADR-320; the base had moved to `9f0f8f4ca`/ADR-402. | Fixed by builder (rebased onto `a0f8cd71e`, ADR-402 only). |
| 2 | High | `codex/src/initializer.ts`, `codex/src/cli.ts`, `cli/src/commands/init.ts`, ADR | The Codex hook was opt-in (`--team-hooks`), against the ruling. | Fixed by builder in `e3ca55c45` (`teamHooks` defaults to true, `--no-team-hooks`). |
| 3 | Medium | `cli/src/mcp-tools/team-runner.ts` | Exec children never saw queued inbox messages unless they had Ruflo MCP. `weft mcp list` is empty, so a weft teammate could not read its inbox. E2E showed a queued `task` left unread after `team run`. | Fixed by builder in `ed38fe065`. The runner drains the inbox into a `=== Messages for you ===` block before `Task:`. E2E confirms the child sees it. |
| 4 | Medium | `cli/src/mcp-tools/team-runner.ts:295` | After #3, a failed run (non-zero exit, timeout, spawn error, `turn.failed`) left the drained messages archived. The "retry with `ruflo team run`" hint then re-ran without them. Reproduced: 1 queued → failed run → 0 queued, and the retry dry-run showed `inboxMessages: 0`. | **Fixed by reviewer** in `83f82f2c5` (pushed). On failure the runner queues the delivered messages again, with a new unit test and an ADR sentence. After the fix: 1 queued → failed run → 1 queued. |
| 5 | Low | `codex/package.json` | `@claude-flow/codex` gained `runHeadlessProcess`/`buildWorkerEnvironment` but stayed 3.0.3. | Fixed by builder (3.0.4 in the v3 workspace). The root umbrella `package.json`/`package-lock` stay at 3.0.3 because 3.0.4 is not on npm. **Release item for the lead.** |
| 6 | Low | Codex hook command | It was `npx -y ruflo@latest …`, a network fetch on every SubagentStop. | Fixed by builder: `npx --no-install ruflo team hook-stop --host codex \|\| echo "…no local ruflo CLI found…" 1>&2`. Exits 0 and never downloads. |
| 7 | Low | Codex hook message | With an *older* `ruflo` reachable by `npx --no-install` (no `team` command), the hook prints Ruflo's "Unknown command: team" and then "no local ruflo CLI found". It still exits 0, but the second message is wrong in that case. Suggested text: "no ruflo CLI with `team` found". | Open (cosmetic). |
| 8 | Low | WeftOS docs, `.grok` skill | ADR-320 links, and SKILL.md told Grok to set `subagent_type` and prefer `ruflo-*` agent types. | Fixed by builder (`4e358e33`). |
| 9 | Low | ADR-402 | It said init "never writes `~/.codex/config.toml`". The pre-existing `registerMCPServer` step does add `[mcp_servers.ruflo]` there when it is missing. | Fixed by builder (the ADR now scopes the claim to the hook merge and notes the MCP step). |
| 10 | Info | Ruflo commits | The author email is `mathew@weavelogic.ai` on every commit (machine git config). Commit subjects `4c47358bc` "…opt-in team hook" and `bc6314e03` are stale after #2. | Left for the lead's squash. |
| 11 | Info | `probe-host-live.mjs` | It writes `docs/benchmarks/host-live-latest.{json,md}` by default (as at base), and on this machine they hold 13 local paths. My runs produced them twice; I deleted them both times. | Do not commit them. |

## Verification (final pass, `ed38fe065` + `83f82f2c5`, base `a0f8cd71e`)

| Command | Exit | Result |
|---|---|---|
| `codex: npm run build` | 0 | |
| `codex: npx tsc --noEmit -p .` | 0 | |
| `codex: vitest run tests/{process,dual-mode,dual-mode-stdin-2947,initializer,generators}.test.ts` | 0 | 126/126 |
| `codex: vitest run` (full suite) | 1 | 254/259. The 5 failures are in `worktrees.test.ts` (4) and `harness-repository-state.test.ts` (1), which the diff does not touch. Builder reports the same at base; not re-run at base by me. |
| `cli: npx tsc --noEmit -p .` | 0 | |
| `cli: npm run build` | 0 | |
| `cli: vitest run` team-tools, team-hosts, team-hosts-stop-identity, team-run, team-format, grok-bus-shim, guidance-brain | 0 | 55/55 (after the reviewer fix; 54/54 before) |
| `cli: vitest run __tests__/init-dual-native-2636-2637.test.ts` | 1 | 1/2. It expects `.mcp.json` from the Claude scaffold, a part of init this diff does not change. Builder reports it fails at base; not re-run at base by me. |
| `node scripts/audit-codex-integration.mjs` | 0 | |
| `node scripts/bench-host-conformance.mjs --host all --no-report` | 0 | 115 pass, 0 critical, 1 warn (optional `security_` prefix, same as before) |
| `node scripts/bench-grok-host-conformance.mjs --no-report` (alias) | 0 | 91 pass, 0 critical, 1 warn |
| `node scripts/probe-host-live.mjs --host codex` | 0 | 7 pass, 0 fail, 2 warn, 4 skip |
| `node scripts/probe-host-live.mjs --host grok` | 0 | 18 pass, 0 fail (the `handoff` FAIL is gone after #3512's fix) |
| `node scripts/probe-host-live.mjs --host command` | 0 | 0 pass, 2 skip |
| `node scripts/probe-host-live.mjs --host all` | 0 | 27 pass, 0 fail, 3 warn, 10 skip |
| WeftOS: `RUFLO_CLI=… node --test scripts/grok-team-bus.interop.test.mjs` | 0 | 3/3 pass |
| WeftOS: `node --test scripts/grok-team-bus.interop.test.mjs` (no CLI) | 0 | 3 skipped |
| WeftOS: `scripts/build.sh check` | 0 | |
| `ruflo init --codex` with an isolated `CODEX_HOME` | 0 | writes `.codex/hooks.json` with the SubagentStop entry |
| Hygiene: `git diff a0f8cd71e` added lines vs `WEFT-\|weave-?logic\|/Users/\|mathewbeane\|clawft\|weftos\|feat/grok-host\|feat/team-bus\|0.8-metaharness\|aepod\|Board:` | — | no matches |
| Hygiene: changed files vs stray store files, `.gitignore`, verification files, `docs/benchmarks/host-live*` | — | no matches |

## End-to-end log (no model calls)

The harness is a temp project with `.claude-flow/team-hosts.json` declaring `weft` (the real WeftOS entry, dry-run only), `fakeweft` (a node one-liner standing in for `weft agent -m`), and `failer` (exits 3). The CLI is the worktree build, run through `ruflo team <verb> --params`.

1. `team create {name:mixed, host:codex}` → ok.
2. `team spawn reviewer` (role reviewer, `hosts:["codex"]`, next `wefty`) → Codex exec plan with `--sandbox read-only`, stdin prompt, and the `CODEX_HOME`/`OPENAI_*` passEnv.
3. `team spawn wefty` (role coder, `hosts:["weft","fakeweft"]`) → command-host plans with `weft agent -m {prompt}`.
4. Rejections: `next:["../evil"]` → "next contains path traversal"; `hosts:["nope"]` → "Unknown host … declare others in .claude-flow/team-hosts.json". Both exit 1.
5. `team plan [reviewer, wefty]` → ok.
6. `team run --dry-run` for both. The Codex argv is filled (`-o …/runs/reviewer-run_….last.txt`, `-C <root>`, `-c mcp_servers.claude-flow.env.CLAUDE_FLOW_CWD="<root>"`; `{mcpServer}` resolved from `codex mcp list --json`). The weft argv is `weft agent -m "<protocol prompt>"`.
7. Messages both ways: reviewer → wefty and wefty → reviewer, each read back from the other's inbox.
8. The lead queues a `task` for wefty.
9. Simulated Codex SubagentStop (`{"hook_event_name":"SubagentStop","agent_type":"reviewer"}` piped to `team hook-stop --host codex`, `TEAM_NAME=mixed`) → `handled: true`. The reviewer is marked `idle`, the next step `wefty` is `ready`, and the result has `assign.agent: "wefty"`. Exit 0.
10. `team on-stop` twice with `runId: dup1` → the second call returns `duplicate: true`.
11. `team run --agent wefty --host fakeweft` → outcome `done`, exit 0. The child printed `FAKEWEFT inbox=true beforeTask=true queued=true`, so the queued task arrived in the prompt before `Task:`. The run file lists `inboxDelivered` = 1 id, and wefty's inbox is empty (archived). The result went to `lead` (wefty has no `next`) and the plan completed.
12. Simulated Grok stop (`{"description":"tester:gk"}` → `team hook-stop --host grok`) → normalized to agent `gk`, `handled: true`.
13. `team hook-stop` with non-JSON stdin → exit 0.
14. Failure path (team `f2`, `failer`): `team run` → outcome `failed`, exit 3, reason `exit code 3: boom`. The result went to `lead` only, the step and member are `failed`, the next agent's inbox stayed empty, and the plan did not advance.
15. Failure plus queued message (team `f3`), after `83f82f2c5`: 1 queued → failed run → 1 queued, and the retry dry-run shows `inboxMessages: 1`.
16. Codex hook command with no usable CLI: exit 0 plus the stderr message (see defect 7 for the wording issue).

## Side effects during review

- My first `ruflo init --codex` run (not isolated) let the base `registerMCPServer` add `[mcp_servers.ruflo]` (`npx -y ruflo@latest mcp start`) to `~/.codex/config.toml`. No `ruflo` entry existed before: the base code only adds when none is named `ruflo`, and a lookup at 01:13 resolved to `claude-flow`. I removed it with `codex mcp remove ruflo`. A diff of the pre-revert copy against the current file shows only those 4 lines, and the `claude-flow` entry is unchanged. Later init runs used an isolated `CODEX_HOME`, and the config checksum stayed the same through the final pass.
- The probe report files noted in defect 11 were deleted.

## Manual checks that spend model calls (not run)

These need approval, and are for the lead or the user:

1. `node scripts/probe-host-live.mjs --host codex --execute`: one headless `codex exec --ephemeral` turn.
2. `node scripts/probe-host-live.mjs --host codex --live`: a two-step team through `ruflo team run`, with the outcome taken from `--json` events.
3. `node scripts/probe-host-live.mjs --host grok --execute`.
4. From the WeftOS worktree: `node ~/dev/ruflo-wt-team-bus-hosts/scripts/probe-host-live.mjs --host command --command-label weft --live --project . --cli ~/dev/ruflo-wt-team-bus-hosts/v3/@claude-flow/cli/bin/cli.js`, a real `weft agent -m` teammate turn.
5. Native Codex subagent stop: in a project after `init --codex`, trust the SubagentStop entry in `/hooks`, have the lead spawn a native subagent, and confirm `team_on_stop` fires. The Codex SubagentStop payload field names are not verified: `stopIdentity` accepts several candidates (`agent_type`, `agent_id`, `name`, …), which is the main untested assumption.

## Verdict

**Approve with notes.** Every user ruling is met, the plans match the real CLIs, the upstream diff is clean, and the no-model verification and E2E pass. The one functional defect left after the builder's rounds (#4) is fixed in `83f82f2c5`. Open items are cosmetic (#7), release bookkeeping (#5: umbrella package at codex 3.0.3), and squash hygiene (#10). Push the WeftOS branch (`accb0086`) only after the Ruflo side lands, because its shim needs a Ruflo that ships `ruflo team`.
