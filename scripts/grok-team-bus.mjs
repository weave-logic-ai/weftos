#!/usr/bin/env node
/**
 * Host-agnostic Agent Teams bus (ADR-320) — filesystem MVP.
 *
 * Works from any host (Grok, Claude, Codex) without proprietary SendMessage.
 * State lives under <project>/.claude-flow/teams/ and mailboxes under
 * <project>/.claude-flow/swarm/mailbox/.
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
 * Spawn plan JSON (stdout on `spawn`) is what Grok maps to spawn_subagent.
 */

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_ROOT = process.env.CLAUDE_PROJECT_DIR
  || process.env.GROK_WORKSPACE_ROOT
  || process.cwd();

const ROLE_DEFAULTS = {
  researcher: { capability_mode: 'read-only', isolation: 'none', subagent_type: 'explore' },
  architect: { capability_mode: 'read-only', isolation: 'none', subagent_type: 'ruflo-architect' },
  developer: { capability_mode: 'all', isolation: 'worktree', subagent_type: 'ruflo-coder' },
  coder: { capability_mode: 'all', isolation: 'worktree', subagent_type: 'ruflo-coder' },
  tester: { capability_mode: 'all', isolation: 'worktree', subagent_type: 'ruflo-tester' },
  reviewer: { capability_mode: 'read-only', isolation: 'none', subagent_type: 'ruflo-reviewer' },
  security: { capability_mode: 'read-only', isolation: 'none', subagent_type: 'general-purpose' },
  coordinator: { capability_mode: 'all', isolation: 'none', subagent_type: 'general-purpose' },
};

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

function teamsRoot(projectRoot) {
  return path.join(projectRoot, '.claude-flow', 'teams');
}

function mailboxRoot(projectRoot) {
  return path.join(projectRoot, '.claude-flow', 'swarm', 'mailbox');
}

function ensureDir(p) {
  fs.mkdirSync(p, { recursive: true });
}

function teamDir(projectRoot, teamName) {
  return path.join(teamsRoot(projectRoot), safeName(teamName));
}

function safeName(name) {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(name)) {
    throw new Error(`Invalid name "${name}" — use alphanumeric, dash, underscore`);
  }
  return name;
}

function readJson(file, fallback = null) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return fallback;
  }
}

function writeJson(file, data) {
  ensureDir(path.dirname(file));
  fs.writeFileSync(file, JSON.stringify(data, null, 2) + '\n', 'utf8');
}

function nowIso() {
  return new Date().toISOString();
}

function cmdCreate(args, projectRoot) {
  const name = safeName(args.name || args.team || `team_${Date.now()}`);
  const dir = teamDir(projectRoot, name);
  if (fs.existsSync(path.join(dir, 'team.json')) && !args.force) {
    throw new Error(`Team "${name}" already exists (use --force to overwrite metadata carefully)`);
  }
  ensureDir(dir);
  ensureDir(path.join(dir, 'plan'));
  ensureDir(mailboxRoot(projectRoot));

  const team = {
    id: name,
    name,
    topology: args.topology || 'hierarchical',
    maxAgents: Number(args['max-agents'] || args.maxAgents || 8),
    status: 'active',
    createdAt: nowIso(),
    host: 'grok',
    members: {},
    plan: { steps: [], index: 0 },
  };
  writeJson(path.join(dir, 'team.json'), team);
  print({ ok: true, action: 'create', team });
}

function buildSpawnPlan(team, agentName, role, prompt, next) {
  const defaults = ROLE_DEFAULTS[role] || ROLE_DEFAULTS.coder;
  const protocol = [
    `You are "${agentName}" (role: ${role}) on team "${team.name}".`,
    `Host-agnostic Agent Teams bus (ADR-320). There is NO Claude SendMessage tool.`,
    `When you finish a deliverable, run:`,
    `  node scripts/grok-team-bus.mjs send --team ${team.name} --to <next> --summary "<short>" --message "<handoff>"`,
    `Or store under memory namespace team:${team.name} if MCP memory is available.`,
    next?.length
      ? `Primary next agent(s): ${next.join(', ')}`
      : `Report completion to the team lead (parent session).`,
    `Check inbox at start: node scripts/grok-team-bus.mjs inbox --team ${team.name} --agent ${agentName}`,
    '',
    'Task:',
    prompt || `(No task body — wait for inbox / lead instructions for role ${role}.)`,
  ].join('\n');

  return {
    teamId: team.id,
    name: agentName,
    role,
    prompt: protocol,
    next: next || [],
    host: {
      grok: {
        subagent_type: defaults.subagent_type,
        capability_mode: defaults.capability_mode,
        isolation: defaults.isolation,
        background: true,
        description: `${role}:${agentName}`,
      },
    },
  };
}

function cmdSpawn(args, projectRoot) {
  const teamName = safeName(args.team || args.name);
  const agent = safeName(args.agent || args.member);
  const role = args.role || agent;
  const dir = teamDir(projectRoot, teamName);
  const teamPath = path.join(dir, 'team.json');
  const team = readJson(teamPath);
  if (!team) throw new Error(`Team "${teamName}" not found — run create first`);

  const next = args.next
    ? String(args.next).split(',').map((s) => s.trim()).filter(Boolean)
    : [];

  const plan = buildSpawnPlan(team, agent, role, args.prompt || args.message || '', next);

  team.members[agent] = {
    name: agent,
    role,
    status: 'registered',
    registeredAt: nowIso(),
    next,
    spawn: plan.host,
  };
  writeJson(teamPath, team);
  ensureDir(path.join(mailboxRoot(projectRoot), agent));

  print({ ok: true, action: 'spawn', spawnPlan: plan, teamId: team.id });
}

function cmdSend(args, projectRoot) {
  const teamName = safeName(args.team);
  const to = args.to || '*';
  if (to !== '*') safeName(to);
  const team = readJson(path.join(teamDir(projectRoot, teamName), 'team.json'));
  if (!team) throw new Error(`Team "${teamName}" not found`);

  const msg = {
    id: `msg_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
    teamId: team.id,
    from: args.from || 'lead',
    to,
    summary: args.summary || '',
    content: args.message || args.content || '',
    type: args.type || 'handoff',
    priority: Number(args.priority ?? 2),
    timestamp: nowIso(),
  };

  const root = mailboxRoot(projectRoot);
  if (to === '*') {
    const members = Object.keys(team.members || {});
    if (members.length === 0) {
      ensureDir(path.join(root, '_broadcast'));
      writeJson(path.join(root, '_broadcast', `${msg.priority}_${msg.id}.json`), msg);
    } else {
      for (const m of members) {
        ensureDir(path.join(root, m));
        writeJson(path.join(root, m, `${msg.priority}_${msg.id}.json`), msg);
      }
    }
  } else {
    ensureDir(path.join(root, to));
    writeJson(path.join(root, to, `${msg.priority}_${msg.id}.json`), msg);
  }

  print({ ok: true, action: 'send', message: msg });
}

function cmdInbox(args, projectRoot) {
  const agent = safeName(args.agent || args.to);
  const dir = path.join(mailboxRoot(projectRoot), agent);
  if (!fs.existsSync(dir)) {
    print({ ok: true, action: 'inbox', agent, messages: [] });
    return;
  }
  const files = fs.readdirSync(dir)
    .filter((f) => f.endsWith('.json'))
    .sort();
  const messages = [];
  for (const f of files) {
    const full = path.join(dir, f);
    const msg = readJson(full);
    if (!msg) continue;
    messages.push(msg);
    if (!args.peek) {
      const archive = path.join(dir, 'archive');
      ensureDir(archive);
      fs.renameSync(full, path.join(archive, f));
    }
  }
  print({ ok: true, action: 'inbox', agent, messages, peek: !!args.peek });
}

function cmdStatus(args, projectRoot) {
  const teamName = safeName(args.team || args.name);
  const team = readJson(path.join(teamDir(projectRoot, teamName), 'team.json'));
  if (!team) throw new Error(`Team "${teamName}" not found`);

  const mail = {};
  for (const name of Object.keys(team.members || {})) {
    const dir = path.join(mailboxRoot(projectRoot), name);
    mail[name] = fs.existsSync(dir)
      ? fs.readdirSync(dir).filter((f) => f.endsWith('.json')).length
      : 0;
  }
  print({ ok: true, action: 'status', team, pendingMail: mail });
}

function cmdPlan(args, projectRoot) {
  const teamName = safeName(args.team);
  const teamPath = path.join(teamDir(projectRoot, teamName), 'team.json');
  const team = readJson(teamPath);
  if (!team) throw new Error(`Team "${teamName}" not found`);

  let steps = [];
  if (args.steps) {
    try {
      steps = JSON.parse(args.steps);
    } catch {
      steps = String(args.steps).split(',').map((s) => s.trim()).filter(Boolean);
    }
  }
  team.plan = {
    steps: steps.map((s, i) => (typeof s === 'string'
      ? { id: s, agent: s, status: i === 0 ? 'ready' : 'pending' }
      : s)),
    index: 0,
    updatedAt: nowIso(),
  };
  writeJson(teamPath, team);
  print({ ok: true, action: 'plan', plan: team.plan });
}

function cmdOnStop(args, projectRoot) {
  const teamName = safeName(args.team);
  const agent = safeName(args.agent);
  const teamPath = path.join(teamDir(projectRoot, teamName), 'team.json');
  const team = readJson(teamPath);
  if (!team) throw new Error(`Team "${teamName}" not found`);

  if (team.members[agent]) {
    team.members[agent].status = 'idle';
    team.members[agent].lastStopAt = nowIso();
  }

  // Advance plan if this agent was current step
  const plan = team.plan || { steps: [], index: 0 };
  const cur = plan.steps[plan.index];
  if (cur && (cur.agent === agent || cur.id === agent)) {
    cur.status = 'done';
    plan.index = Math.min(plan.index + 1, plan.steps.length);
    const next = plan.steps[plan.index];
    if (next) next.status = 'ready';
  }
  team.plan = plan;
  writeJson(teamPath, team);

  const nextStep = plan.steps[plan.index];
  print({
    ok: true,
    action: 'on-stop',
    agent,
    next: nextStep || null,
    assign: nextStep
      ? {
          hint: `Spawn or resume agent "${nextStep.agent}" for the next plan step`,
          agent: nextStep.agent,
        }
      : { hint: 'Plan complete — lead should synthesize' },
  });
}

function cmdShutdown(args, projectRoot) {
  const teamName = safeName(args.team || args.name);
  const teamPath = path.join(teamDir(projectRoot, teamName), 'team.json');
  const team = readJson(teamPath);
  if (!team) throw new Error(`Team "${teamName}" not found`);
  team.status = 'shutdown';
  team.shutdownAt = nowIso();
  for (const m of Object.values(team.members || {})) {
    m.status = 'shutdown';
  }
  writeJson(teamPath, team);
  print({ ok: true, action: 'shutdown', teamId: team.id });
}

function print(obj) {
  process.stdout.write(JSON.stringify(obj, null, 2) + '\n');
}

function usage() {
  print({
    ok: false,
    error: 'usage',
    commands: [
      'create --name <team> [--topology hierarchical] [--max-agents 8]',
      'spawn --team <team> --agent <name> --role <role> [--prompt "..."] [--next a,b]',
      'send --team <team> --to <agent|*> --message "..." [--summary s] [--from lead]',
      'inbox --team <team> --agent <name> [--peek]',
      'status --team <team>',
      'plan --team <team> --steps \'["architect","developer","tester"]\'',
      'on-stop --team <team> --agent <name>',
      'shutdown --team <team>',
    ],
  });
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const cmd = args._[0];
  const projectRoot = path.resolve(args.root || DEFAULT_ROOT);

  try {
    switch (cmd) {
      case 'create': return cmdCreate(args, projectRoot);
      case 'spawn': return cmdSpawn(args, projectRoot);
      case 'send': return cmdSend(args, projectRoot);
      case 'inbox': return cmdInbox(args, projectRoot);
      case 'status': return cmdStatus(args, projectRoot);
      case 'plan': return cmdPlan(args, projectRoot);
      case 'on-stop': return cmdOnStop(args, projectRoot);
      case 'shutdown': return cmdShutdown(args, projectRoot);
      default:
        usage();
        process.exit(cmd ? 1 : 0);
    }
  } catch (err) {
    print({ ok: false, error: err.message || String(err) });
    process.exit(1);
  }
}

main();
