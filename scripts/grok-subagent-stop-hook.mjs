#!/usr/bin/env node
/**
 * Grok SubagentStop → `ruflo team hook-stop --host grok` (ADR-402).
 *
 * Ruflo maps the hook payload (Grok's `role:agent` description, SUBAGENT_NAME,
 * TEAM_NAME) to team_on_stop. With several active teams and none named it
 * does nothing rather than advance the wrong plan.
 *
 * The ruflo CLI resolves like scripts/grok-team-bus.mjs: RUFLO_CLI, then
 * .claude-flow/ruflo-cli-path. It never downloads a package. With no CLI it
 * says so on stderr. Fail-open: always exit 0.
 */
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const projectRoot = process.env.CLAUDE_PROJECT_DIR
  || process.env.GROK_WORKSPACE_ROOT
  || process.cwd();

function readStdin() {
  return new Promise((resolve) => {
    let data = '';
    const t = setTimeout(() => resolve(data), 300);
    process.stdin.setEncoding('utf8');
    process.stdin.on('data', (c) => { data += c; });
    process.stdin.on('end', () => { clearTimeout(t); resolve(data); });
    process.stdin.on('error', () => { clearTimeout(t); resolve(data); });
  });
}

function rufloCli() {
  let cli = process.env.RUFLO_CLI;
  if (!cli) {
    try {
      cli = fs.readFileSync(path.join(projectRoot, '.claude-flow', 'ruflo-cli-path'), 'utf8').trim();
    } catch {
      return null;
    }
  }
  const abs = path.resolve(projectRoot, cli);
  return fs.existsSync(abs) ? abs : null;
}

const raw = await readStdin();
const cli = rufloCli();
if (!cli) {
  process.stderr.write(
    'grok-subagent-stop-hook: no ruflo CLI found, team stop not recorded. Set RUFLO_CLI or write the path to ' +
      '<ruflo>/v3/@claude-flow/cli/bin/cli.js into .claude-flow/ruflo-cli-path\n',
  );
} else {
  try {
    spawnSync(process.execPath, [cli, 'team', 'hook-stop', '--host', 'grok'], {
      cwd: projectRoot,
      env: { ...process.env, CLAUDE_FLOW_CWD: projectRoot },
      input: raw,
      timeout: 4000,
      stdio: ['pipe', 'ignore', 'ignore'],
    });
  } catch {
    /* fail-open */
  }
}

process.exit(0);
