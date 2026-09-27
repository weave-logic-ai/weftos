#!/usr/bin/env node
// Source-local steward for native dashboard boards. A project-owned board uses
// its own adapter and publisher; subscribed_items are never written here.
import { createHash } from 'node:crypto';
import { mkdir, open, readFile, rename, unlink, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';

const baseUrl = process.env.WEFTOS_DASHBOARD_URL ?? 'https://weftos-dashboard.vercel.app';
const tokenFile = process.env.WEFTOS_BOARD_TOKEN_FILE ?? join(homedir(), '.config/weftos/board-token');
const stateFile = process.env.WEFTOS_STEWARD_STATE_FILE ?? join(homedir(), '.local/state/weftos/ticket-steward.json');
const rules = {
  'todo:in_progress': ['claim'],
  'in_progress:review': ['change', 'checks'],
  'review:done': ['review', 'delivery'],
  'done:todo': ['regression'],
  'done:in_progress': ['regression'],
};

function digest(value) { return createHash('sha256').update(JSON.stringify(value)).digest('hex'); }
function validReference(ref) {
  return ref && typeof ref === 'object' &&
    typeof ref.type === 'string' && typeof ref.url === 'string' && /^https?:\/\//.test(ref.url);
}

export function assess(ticket, decision) {
  if (!decision) return { outcome: 'unchanged' };
  if (decision.ticket_id !== ticket.id || typeof decision.expected_updated_at !== 'string' ||
      typeof decision.evidence?.reason !== 'string' || decision.evidence.reason.trim().length < 20 ||
      !Array.isArray(decision.evidence.references) || !decision.evidence.references.length ||
      !decision.evidence.references.every(validReference)) {
    return { outcome: 'queued', reason: 'invalid_evidence' };
  }
  if (decision.action !== 'status' && decision.action !== 'assign')
    return { outcome: 'queued', reason: 'invalid_action' };
  const types = new Set(decision.evidence.references.map((ref) => ref.type));
  if (decision.action === 'status') {
    const required = rules[`${decision.expected_status}:${decision.value}`];
    if (!required || !required.every((type) => types.has(type)))
      return { outcome: 'queued', reason: 'transition_needs_evidence_or_review' };
  } else if (!types.has('accepted_handoff') ||
             (decision.value !== null && (typeof decision.value !== 'string' || !decision.value.trim()))) {
    return { outcome: 'queued', reason: 'assignment_needs_accepted_handoff' };
  }
  // References supplied in a JSON file are attestations, not independently
  // verified evidence. Auto-authorization can be added with source verifiers.
  if (!(decision.authorization === 'human' && typeof decision.approved_by === 'string' && decision.approved_by.trim()))
    return { outcome: 'queued', reason: 'authorization_required' };
  return { outcome: 'ready', actionKey: digest({
    ticket_id: ticket.id, expected_updated_at: decision.expected_updated_at,
    action: decision.action, value: decision.value,
    evidence: { ...decision.evidence, approved_by: decision.approved_by },
  }) };
}

async function request(token, method, path, body) {
  const response = await fetch(new URL(path, baseUrl), {
    method, headers: { Authorization: `Bearer ${token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    cache: 'no-store', signal: AbortSignal.timeout(15_000),
  });
  const result = await response.json().catch(() => ({}));
  return { status: response.status, result };
}

async function tickets(token) {
  const all = [];
  for (let offset = 0; offset < 100_000; offset += 100) {
    const { status, result } = await request(token, 'GET', `/api/harness/tickets?limit=100&offset=${offset}`);
    if (status !== 200 || !Array.isArray(result.items)) throw new Error(`Board list failed at offset ${offset}`);
    all.push(...result.items);
    if (result.items.length < 100) return all;
  }
  throw new Error('Board pagination limit exceeded');
}

async function saveState(state) {
  await mkdir(dirname(stateFile), { recursive: true, mode: 0o700 });
  const next = `${stateFile}.${process.pid}.next`;
  await writeFile(next, JSON.stringify(state, null, 2) + '\n', { mode: 0o600 });
  await rename(next, stateFile);
}

async function acquireLock(path) {
  try { return await open(path, 'wx', 0o600); }
  catch (error) {
    if (error.code !== 'EEXIST') throw error;
    const oldPid = Number((await readFile(path, 'utf8')).trim());
    if (!Number.isInteger(oldPid) || oldPid < 1) throw new Error(`Invalid steward lock: ${path}`);
    try { process.kill(oldPid, 0); throw new Error(`Steward already running as PID ${oldPid}`); }
    catch (probe) { if (probe.code !== 'ESRCH') throw probe; }
    await unlink(path);
    return open(path, 'wx', 0o600);
  }
}

async function main() {
  const args = process.argv.slice(2);
  const apply = args.includes('--apply');
  const evidenceIndex = args.indexOf('--decisions');
  if (evidenceIndex < 0 || !args[evidenceIndex + 1]) {
    throw new Error('Usage: ticket-steward.mjs --decisions <approved JSON file> [--apply]');
  }
  const token = process.env.WEFTOS_BOARD_TOKEN ?? (await readFile(tokenFile, 'utf8')).trim();
  if (!/^wfb_[a-f0-9]{64}$/.test(token)) throw new Error('Invalid board credential format');
  const input = JSON.parse(await readFile(args[evidenceIndex + 1], 'utf8'));
  if (input.schema !== 1 || !Array.isArray(input.decisions)) throw new Error('Invalid decisions file');
  const byId = new Map();
  for (const decision of input.decisions) {
    if (typeof decision.ticket_id !== 'string' || byId.has(decision.ticket_id))
      throw new Error('Each decision needs a unique ticket_id');
    byId.set(decision.ticket_id, decision);
  }
  await mkdir(dirname(stateFile), { recursive: true, mode: 0o700 });
  const lock = `${stateFile}.lock`;
  const handle = await acquireLock(lock);
  try {
    await handle.writeFile(`${process.pid}\n`);
    const state = await readFile(stateFile, 'utf8').then(JSON.parse).catch((error) => {
      if (error.code === 'ENOENT') return { schema: 1, tickets: {} };
      throw error;
    });
    if (state.schema !== 1 || typeof state.tickets !== 'object') throw new Error('Invalid steward state');
    const all = await tickets(token);
    const known = new Set(all.map((ticket) => ticket.id));
    const unknown = [...byId.keys()].filter((id) => !known.has(id));
    if (unknown.length) throw new Error(`${unknown.length} decision ticket(s) are outside this board credential`);
    const counts = { scanned: 0, unchanged: 0, queued: 0, ready: 0, applied: 0, conflicts: 0, source_errors: 0 };
    for (const ticket of all) {
      const decision = byId.get(ticket.id);
      const verdict = assess(ticket, decision);
      const record = { checked_at: new Date().toISOString(), source_updated_at: ticket.updated_at,
        evidence_digest: decision ? digest(decision.evidence) : null,
        outcome: verdict.outcome, reason: verdict.reason ?? null };
      counts.scanned++;
      if (verdict.outcome === 'unchanged') counts.unchanged++;
      else if (verdict.outcome === 'queued') counts.queued++;
      else if (!apply) counts.ready++;
      else {
        const { status, result } = await request(token, 'POST', '/api/harness/steward', {
          ticket_id: ticket.id, action_key: verdict.actionKey,
          expected_updated_at: decision.expected_updated_at,
          action: decision.action, value: decision.value,
          evidence: { ...decision.evidence, approved_by: decision.approved_by },
        });
        if (status === 200) {
          const converged = decision.action === 'status' ? result.status === decision.value :
            (result.assignee ?? null) === (decision.value ?? null);
          record.outcome = converged ? 'applied' : 'superseded'; record.action_id = result.action_id;
          record.resulting_updated_at = result.updated_at;
          if (converged) counts.applied++;
          else counts.conflicts++;
        } else if (status === 409) {
          record.outcome = 'conflict'; record.reason = result.error;
          counts.conflicts++;
        } else {
          record.outcome = 'source_error'; record.reason = `HTTP ${status}: ${result.error ?? 'unknown'}`;
          counts.source_errors++;
        }
      }
      state.tickets[ticket.id] = record;
      await saveState(state);
    }
    state.last_sweep_at = new Date().toISOString();
    await saveState(state);
    console.log(JSON.stringify(counts));
    if (counts.source_errors) process.exitCode = 1;
  } finally {
    await handle.close();
    await unlink(lock);
  }
}

if (process.argv[1] && new URL(import.meta.url).pathname === process.argv[1])
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
