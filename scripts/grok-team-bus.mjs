#!/usr/bin/env node
/**
 * Host-agnostic Agent Teams bus (ADR-320) — CLI shim.
 *
 * Keeps the original flag set, but every verb calls
 * `ruflo team <verb> --params <json>`, so the team_* handlers stay the only
 * writer of .claude-flow/teams/ and the mailbox. This script does no file
 * I/O of its own.
 *
 * Usage:
 *   node scripts/grok-team-bus.mjs create --name feature-auth --topology hierarchical
 *   node scripts/grok-team-bus.mjs spawn  --team feature-auth --agent architect --role architect
 *   node scripts/grok-team-bus.mjs send   --team feature-auth --to developer --summary "design" --message "..."
 *   node scripts/grok-team-bus.mjs inbox  --team feature-auth --agent developer [--peek]
 *   node scripts/grok-team-bus.mjs status --team feature-auth
 *   node scripts/grok-team-bus.mjs plan   --team feature-auth --steps '["architect","developer","tester","reviewer"]'
 *   node scripts/grok-team-bus.mjs on-stop --team feature-auth --agent architect
 *   node scripts/grok-team-bus.mjs shutdown --team feature-auth
 *
 * The ruflo CLI is a local checkout until a published release carries
 * `ruflo team` (see package.json weftos.rufloPinNote). It resolves as:
 *   1. RUFLO_CLI — path to <ruflo>/v3/@claude-flow/cli/bin/cli.js
 *   2. .claude-flow/ruflo-cli-path — one line holding that path (gitignored)
 * If neither resolves, the shim exits 2. It never writes team state itself.
 */

import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const DEFAULT_ROOT = process.env.CLAUDE_PROJECT_DIR
  || process.env.GROK_WORKSPACE_ROOT
  || process.cwd();

function parseArgs(argv) {
  const out = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith('--')) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (!next || next.startsWith('--')) {
        out[key] = true;
      } else {
        out[key] = next;
        i++;
      }
    } else {
      out._.push(a);
    }
  }
  return out;
}

const list = (v) => String(v).split(',').map((s) => s.trim()).filter(Boolean);

function defined(obj) {
  return Object.fromEntries(Object.entries(obj).filter(([, v]) => v !== undefined));
}

/** Map this script's flags to team_* tool input. */
const VERBS = {
  create: (a) => defined({
    name: a.name || a.team || `team_${Date.now()}`,
    topology: a.topology,
    maxAgents: a['max-agents'] || a.maxAgents ? Number(a['max-agents'] || a.maxAgents) : undefined,
    host: a.host || 'grok',
    force: a.force === true ? true : undefined,
  }),
  spawn: (a) => defined({
    team: a.team || a.name,
    agent: a.agent || a.member,
    role: a.role,
    prompt: a.prompt || a.message,
    next: a.next ? list(a.next) : undefined,
    hosts: a.hosts ? list(a.hosts) : undefined,
    model: a.model,
  }),
  send: (a) => defined({
    team: a.team,
    to: a.to || '*',
    message: a.message || a.content,
    summary: a.summary,
    from: a.from,
    type: a.type,
    priority: a.priority !== undefined ? Number(a.priority) : undefined,
  }),
  broadcast: (a) => defined({ team: a.team, message: a.message || a.content, summary: a.summary, from: a.from }),
  inbox: (a) => defined({ team: a.team, agent: a.agent || a.to, peek: a.peek === true ? true : undefined }),
  status: (a) => defined({ team: a.team || a.name }),
  plan: (a) => {
    let steps = [];
    if (a.steps) {
      try {
        steps = JSON.parse(a.steps);
      } catch {
        steps = list(a.steps);
      }
    }
    return { team: a.team, steps };
  },
  'on-stop': (a) => defined({ team: a.team, agent: a.agent, outcome: a.outcome, runId: a['run-id'], reason: a.reason }),
  shutdown: (a) => defined({ team: a.team || a.name }),
};

function resolveRuflo(projectRoot) {
  let cli = process.env.RUFLO_CLI;
  if (!cli) {
    try {
      cli = fs.readFileSync(path.join(projectRoot, '.claude-flow', 'ruflo-cli-path'), 'utf8').trim();
    } catch {
      /* not configured */
    }
  }
  if (!cli) return null;
  const abs = path.resolve(projectRoot, cli);
  return fs.existsSync(abs) ? { cmd: process.execPath, pre: [abs] } : null;
}

function print(obj) {
  process.stdout.write(JSON.stringify(obj, null, 2) + '\n');
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const verb = args._[0];
  const projectRoot = path.resolve(args.root || DEFAULT_ROOT);

  if (!VERBS[verb]) {
    print({
      ok: false,
      error: 'usage',
      commands: [
        'create --name <team> [--topology hierarchical] [--max-agents 8] [--host grok]',
        'spawn --team <team> --agent <name> --role <role> [--prompt "..."] [--next a,b] [--hosts grok,codex]',
        'send --team <team> --to <agent|*> --message "..." [--summary s] [--from lead]',
        'inbox --team <team> --agent <name> [--peek]',
        'status --team <team>',
        'plan --team <team> --steps \'["architect","developer","tester"]\'',
        'on-stop --team <team> --agent <name> [--outcome done|failed] [--run-id id]',
        'shutdown --team <team>',
      ],
    });
    process.exit(verb ? 1 : 0);
  }

  const ruflo = resolveRuflo(projectRoot);
  if (!ruflo) {
    print({
      ok: false,
      error: 'ruflo CLI not found. Set RUFLO_CLI, or write the path to <ruflo>/v3/@claude-flow/cli/bin/cli.js into .claude-flow/ruflo-cli-path',
    });
    process.exit(2);
  }
  const { cmd, pre } = ruflo;
  const r = spawnSync(cmd, [...pre, 'team', verb, '--params', JSON.stringify(VERBS[verb](args))], {
    cwd: projectRoot,
    env: { ...process.env, CLAUDE_FLOW_CWD: projectRoot },
    encoding: 'utf8',
  });
  if (r.error) {
    print({ ok: false, error: `ruflo CLI could not be started (${r.error.message}); set RUFLO_CLI to cli.js` });
    process.exit(2);
  }
  const out = (r.stdout || '').trim();
  try {
    const parsed = JSON.parse(out.slice(out.indexOf('{')));
    // `ok` mirrors `success` for callers of the original script.
    print({ ok: parsed.success !== false, ...parsed });
  } catch {
    if (out) process.stdout.write(out + '\n');
    if (r.stderr) process.stderr.write(r.stderr);
  }
  process.exit(r.status ?? 1);
}

main();
