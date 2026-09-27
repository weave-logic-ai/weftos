#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';

const baseUrl = process.env.WEFTOS_DASHBOARD_URL ?? 'https://weftos-dashboard.vercel.app';
const tokenFile = process.env.WEFTOS_BOARD_TOKEN_FILE ?? join(homedir(), '.config/weftos/board-token');
const statuses = new Set(['todo', 'in_progress', 'review', 'done']);
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function usage() {
  console.log(`WeftOS dashboard board
  dashboard-board.mjs ready [--json]
  dashboard-board.mjs list [todo|in_progress|review|done] [--json]
  dashboard-board.mjs goals [--json]
  dashboard-board.mjs show <ticket UUID|WEFT-N> [--json]
  dashboard-board.mjs create <stable-source-key> <title> <description> [source URL]
  dashboard-board.mjs claim <ticket UUID|WEFT-N>
  dashboard-board.mjs move <ticket UUID|WEFT-N> <status>
  dashboard-board.mjs note <ticket UUID|WEFT-N> <comment>
  dashboard-board.mjs link <ticket UUID|WEFT-N> <goal UUID>
  dashboard-board.mjs unlink <ticket UUID|WEFT-N>
  dashboard-board.mjs done <ticket UUID|WEFT-N> <tests, build, and shipped evidence>

Credential: WEFTOS_BOARD_TOKEN or mode-600 ${tokenFile}`);
}

async function boardToken() {
  const token = process.env.WEFTOS_BOARD_TOKEN ?? (await readFile(tokenFile, 'utf8')).trim();
  if (!/^wfb_[a-f0-9]{64}$/.test(token)) throw new Error('Invalid board credential format');
  return token;
}

async function request(token, method, path, body) {
  const response = await fetch(new URL(path, baseUrl), {
    method,
    headers: { Authorization: `Bearer ${token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(15_000),
    cache: 'no-store',
  });
  const result = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(`Board HTTP ${response.status}: ${result.error ?? 'request failed'}`);
  return result;
}

async function allTickets(token, status = null) {
  const items = [];
  for (let offset = 0; offset < 10_000; offset += 100) {
    const query = new URLSearchParams({ limit: '100', offset: String(offset) });
    if (status) query.set('status', status);
    const page = await request(token, 'GET', `/api/harness/tickets?${query}`);
    if (!Array.isArray(page.items)) throw new Error('Board returned an invalid page');
    items.push(...page.items);
    if (page.items.length < 100) return items;
  }
  throw new Error('Board pagination exceeded 10,000 items');
}

async function allGoals(token) {
  const items = [];
  for (let offset = 0; offset < 10_000; offset += 100) {
    const page = await request(token, 'GET', `/api/harness/goals?limit=100&offset=${offset}`);
    if (!Array.isArray(page.items)) throw new Error('Dashboard returned an invalid goal page');
    items.push(...page.items);
    if (page.items.length < 100) return items;
  }
  throw new Error('Goal pagination exceeded 10,000 goals');
}

function shortRef(ticket) {
  return ticket.source_url?.match(/WEFT-\d+$/)?.[0] ?? ticket.id;
}

async function resolve(token, ref) {
  const tickets = await allTickets(token);
  const issue = tickets.find((ticket) => ticket.id === ref || ticket.source_id === ref || shortRef(ticket) === ref.toUpperCase());
  if (!issue) throw new Error(`Ticket not found: ${ref}`);
  return issue;
}

function printList(items) {
  for (const ticket of items) {
    console.log(`${shortRef(ticket).padEnd(12)} ${ticket.status.padEnd(11)} ${ticket.priority.padEnd(7)} ${ticket.title}`);
  }
  console.log(`${items.length} ticket(s)`);
}

async function main() {
  const args = process.argv.slice(2);
  const json = args.includes('--json');
  const words = args.filter((arg) => arg !== '--json');
  const [command, ref, ...rest] = words;
  if (!command || command === 'help' || command === '--help') return usage();
  const token = await boardToken();
  if (command === 'ready' || command === 'list') {
    const status = command === 'ready' ? 'todo' : (ref ?? null);
    if (status && !statuses.has(status)) throw new Error(`Invalid status: ${status}`);
    const items = await allTickets(token, status);
    return json ? console.log(JSON.stringify(items, null, 2)) : printList(items);
  }
  if (command === 'goals') {
    const goals = await allGoals(token);
    if (json) return console.log(JSON.stringify(goals, null, 2));
    for (const goal of goals) console.log(`${goal.id} ${goal.status.padEnd(9)} ${goal.title}`);
    return console.log(`${goals.length} goal(s)`);
  }
  if (!ref) throw new Error(`${command} requires a ticket reference`);
  if (command === 'create') {
    const [title, description, sourceUrl] = rest;
    if (!title || !description || description.trim().length < 20) {
      throw new Error('create requires a quoted title and a description of at least 20 characters');
    }
    const created = await request(token, 'POST', '/api/harness/tickets', {
      action: 'create', source_key: ref, title, description, ...(sourceUrl ? { source_url: sourceUrl } : {}),
    });
    return console.log(json ? JSON.stringify(created, null, 2) : `${created.id} · ${created.created ? 'created' : 'already exists'}`);
  }
  const ticket = await resolve(token, ref);
  if (command === 'show') return console.log(json ? JSON.stringify(ticket, null, 2) : `${shortRef(ticket)} · ${ticket.status}\n${ticket.title}\n${ticket.description}\n${ticket.source_url ?? ''}`);
  if (command === 'link' || command === 'unlink') {
    const goalId = command === 'unlink' ? null : rest[0];
    if (command === 'link' && !uuidPattern.test(goalId ?? '')) {
      throw new Error('link requires a goal UUID from this project');
    }
    const linked = await request(token, 'POST', '/api/harness/tickets', {
      ticket_id: ticket.id, action: 'goal', value: goalId,
    });
    return console.log(json ? JSON.stringify(linked, null, 2) : `${shortRef(ticket)} → ${linked.goal_id ?? 'no goal'}`);
  }
  let result;
  if (command === 'claim') {
    result = await request(token, 'POST', '/api/harness/tickets', { ticket_id: ticket.id, action: 'claim' });
  } else if (command === 'move') {
    if (!statuses.has(rest[0])) throw new Error('move requires a valid status');
    result = await request(token, 'POST', '/api/harness/tickets', { ticket_id: ticket.id, action: 'status', value: rest[0] });
  } else if (command === 'note' || command === 'done') {
    const body = rest.join(' ').trim();
    if (!body) throw new Error(`${command} requires a comment`);
    result = await request(token, 'POST', '/api/harness/tickets', { ticket_id: ticket.id, action: 'comment', value: body });
    if (command === 'done') result = await request(token, 'POST', '/api/harness/tickets', { ticket_id: ticket.id, action: 'status', value: 'done' });
  } else {
    throw new Error(`Unknown command: ${command}`);
  }
  console.log(json ? JSON.stringify(result, null, 2) : `${shortRef(ticket)} → ${result.status}`);
}

main().catch((error) => { console.error(error instanceof Error ? error.message : String(error)); process.exitCode = 1; });
