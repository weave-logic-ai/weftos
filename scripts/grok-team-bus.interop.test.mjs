// Interop test: scripts/grok-team-bus.mjs (shim) and `ruflo team` share one
// on-disk format, and Ruflo's team_* handlers are its only writer (ADR-320).
//
//   RUFLO_CLI=<ruflo>/v3/@claude-flow/cli/bin/cli.js node --test scripts/grok-team-bus.interop.test.mjs
//
// Without RUFLO_CLI (or .claude-flow/ruflo-cli-path) every case reports SKIP.
import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { spawn, spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, '..');
const SHIM = join(HERE, 'grok-team-bus.mjs');
const FIXTURE = join(HERE, 'fixtures', 'team-v0');

// team.json top-level keys written by Ruflo's TeamState (schemaVersion 1).
const TEAM_KEYS = ['createdAt', 'host', 'id', 'maxAgents', 'members', 'name', 'plan', 'schemaVersion', 'status', 'topology'];

function rufloCli() {
  let cli = process.env.RUFLO_CLI;
  if (!cli) {
    try {
      cli = readFileSync(join(REPO, '.claude-flow', 'ruflo-cli-path'), 'utf8').trim();
    } catch {
      return null;
    }
  }
  const abs = resolve(REPO, cli);
  return existsSync(abs) ? abs : null;
}

const CLI = rufloCli();
const skip = CLI ? false : 'ruflo CLI not resolvable';

function project() {
  return mkdtempSync(join(tmpdir(), 'weft-team-interop-'));
}

function env(root) {
  return { ...process.env, RUFLO_CLI: CLI, CLAUDE_FLOW_CWD: root, RUFLO_DAEMON_AUTOSTART: '0' };
}

function shim(root, ...args) {
  const r = spawnSync(process.execPath, [SHIM, ...args, '--root', root], { encoding: 'utf8', env: env(root), timeout: 60_000 });
  assert.equal(r.status, 0, `shim ${args[0]} failed: ${r.stdout}${r.stderr}`);
  return JSON.parse(r.stdout);
}

function ruflo(root, verb, params) {
  const r = spawnSync(process.execPath, [CLI, 'team', verb, '--params', JSON.stringify(params)], {
    cwd: root, encoding: 'utf8', env: env(root), timeout: 60_000,
  });
  assert.equal(r.status, 0, `ruflo team ${verb} failed: ${r.stdout}${r.stderr}`);
  return JSON.parse(r.stdout);
}

function readTeam(root, name) {
  return JSON.parse(readFileSync(join(root, '.claude-flow', 'teams', name, 'team.json'), 'utf8'));
}

test('(a) shim and ruflo team interleave on one team', { skip }, () => {
  const root = project();
  try {
    shim(root, 'create', '--name', 'demo');
    shim(root, 'plan', '--team', 'demo', '--steps', '["architect","developer"]');
    shim(root, 'spawn', '--team', 'demo', '--agent', 'architect', '--role', 'architect', '--next', 'developer');
    ruflo(root, 'send', { team: 'demo', to: 'developer', from: 'architect', message: 'Use layers' });
    const inbox = shim(root, 'inbox', '--team', 'demo', '--agent', 'developer');
    assert.equal(inbox.messages.length, 1);
    assert.equal(inbox.messages[0].content, 'Use layers');
    ruflo(root, 'on-stop', { team: 'demo', agent: 'architect' });
    const status = shim(root, 'status', '--team', 'demo');
    assert.equal(status.team.plan.index, 1);

    const team = readTeam(root, 'demo');
    assert.equal(team.schemaVersion, 1);
    assert.deepEqual(Object.keys(team).sort(), TEAM_KEYS);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('(b) a team.json written by the old script is advanced and stamped v1', { skip }, () => {
  const root = project();
  try {
    mkdirSync(join(root, '.claude-flow', 'teams', 'legacy'), { recursive: true });
    cpSync(join(FIXTURE, 'team.json'), join(root, '.claude-flow', 'teams', 'legacy', 'team.json'));
    cpSync(join(FIXTURE, 'mailbox'), join(root, '.claude-flow', 'swarm', 'mailbox'), { recursive: true });
    assert.equal(readTeam(root, 'legacy').schemaVersion, undefined);

    const stop = shim(root, 'on-stop', '--team', 'legacy', '--agent', 'architect');
    assert.equal(stop.assign.agent, 'developer');
    const team = readTeam(root, 'legacy');
    assert.equal(team.schemaVersion, 1);
    assert.equal(team.plan.index, 1);
    // The old flat Grok spawn plan is kept as data.
    assert.equal(team.members.architect.spawn.grok.subagent_type, 'plan');
    const inbox = shim(root, 'inbox', '--team', 'legacy', '--agent', 'developer', '--peek');
    assert.equal(inbox.messages.length, 1);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('(c) 8 parallel on-stop calls through the shim and ruflo keep team.json valid', { skip }, async () => {
  const root = project();
  try {
    shim(root, 'create', '--name', 'par');
    const agents = Array.from({ length: 8 }, (_, i) => `w${i}`);
    for (const a of agents) shim(root, 'spawn', '--team', 'par', '--agent', a, '--role', 'reviewer');

    const runs = agents.map((a, i) => new Promise((done) => {
      const argv = i % 2
        ? [CLI, 'team', 'on-stop', '--params', JSON.stringify({ team: 'par', agent: a })]
        : [SHIM, 'on-stop', '--team', 'par', '--agent', a, '--root', root];
      const child = spawn(process.execPath, argv, { cwd: root, env: env(root), stdio: 'ignore' });
      child.on('close', (code) => done(code));
    }));
    const codes = await Promise.all(runs);
    assert.deepEqual(codes, agents.map(() => 0));

    const team = readTeam(root, 'par');
    for (const a of agents) assert.equal(team.members[a].status, 'idle', a);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
