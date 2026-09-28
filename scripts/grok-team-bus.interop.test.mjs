// Interop test: scripts/grok-team-bus.mjs (shim) and `ruflo team` share one
// on-disk format, and Ruflo's team_* handlers are its only writer (ADR-402,
// upstream ruvnet/ruflo PR #3512 + #3513 — templates/grok/scripts/
// grok-team-bus.mjs + grok-team-store.mjs are the single store the CLI
// loads for every read and write; there is no v0/schemaVersion migration
// path in that store, so this test does not exercise one).
//
//   RUFLO_CLI=<ruflo>/v3/@claude-flow/cli/bin/cli.js node --test scripts/grok-team-bus.interop.test.mjs
//
// Without RUFLO_CLI (or .claude-flow/ruflo-cli-path) every case reports SKIP.
import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, '..');
const SHIM = join(HERE, 'grok-team-bus.mjs');

// team.json top-level keys written by the fixed store (grok-team-store.mjs).
const TEAM_KEYS = ['createdAt', 'host', 'id', 'maxAgents', 'members', 'name', 'plan', 'status', 'topology'];

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
    // Per-team mailbox (ADR-402 fix): teams/<team>/mailbox/<agent>/, not the
    // old global .claude-flow/swarm/mailbox/<agent>/.
    assert.ok(existsSync(join(root, '.claude-flow', 'teams', 'demo', 'mailbox', 'developer')));

    const stop = ruflo(root, 'on-stop', { team: 'demo', agent: 'architect', outcome: 'done', runId: 'run_a_1' });
    assert.equal(stop.advanced, true);
    const status = shim(root, 'status', '--team', 'demo');
    assert.equal(status.team.plan.index, 1);

    const team = readTeam(root, 'demo');
    assert.deepEqual(Object.keys(team).sort(), TEAM_KEYS);
    assert.equal(team.members.architect.lastOutcome, 'done');
    assert.equal(team.members.architect.lastStopRunId, 'run_a_1');

    // Spawn description carries role:agent@team (ADR-402 identity format).
    const spawn = shim(root, 'spawn', '--team', 'demo', '--agent', 'reviewer', '--role', 'reviewer');
    assert.equal(spawn.spawnPlan.host.grok.spawn.description, 'reviewer:reviewer@demo');

    // A repeated runId on a later on-stop is a no-op (dedupe), and a
    // failed outcome does not advance the plan.
    const dup = ruflo(root, 'on-stop', { team: 'demo', agent: 'architect', outcome: 'done', runId: 'run_a_1' });
    assert.equal(dup.duplicate, true);
    const failStop = ruflo(root, 'on-stop', { team: 'demo', agent: 'reviewer', outcome: 'failed', reason: 'boom' });
    assert.equal(failStop.advanced, false);
    assert.equal(readTeam(root, 'demo').members.reviewer.status, 'failed');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('(b) 8 parallel on-stop calls through the shim and ruflo keep team.json valid', { skip }, async () => {
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
