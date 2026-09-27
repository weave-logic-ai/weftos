import { test } from 'node:test';
import { strict as assert } from 'node:assert';
import { assess } from './ticket-steward.mjs';

const ticket = { id: '11111111-1111-4111-8111-111111111111', status: 'in_progress', updated_at: '2026-09-27T12:00:00Z' };
const ref = (type) => ({ type, url: `https://example.com/${type}` });
const decision = (overrides = {}) => ({
  ticket_id: ticket.id, expected_status: 'in_progress', expected_updated_at: ticket.updated_at,
  action: 'status', value: 'review', authorization: 'human', approved_by: 'project lead',
  evidence: { reason: 'Implementation and checks are recorded in linked receipts.',
    references: [ref('change'), ref('checks')] },
  ...overrides,
});

test('queues a review move when required check evidence is missing', () => {
  const result = assess(ticket, decision({ evidence: {
    reason: 'Implementation exists but no check receipt is available yet.', references: [ref('change')],
  } }));
  assert.equal(result.outcome, 'queued');
});

test('requires a human approval until source evidence verifiers exist', () => {
  assert.equal(assess(ticket, decision({ authorization: 'deterministic', approved_by: null })).outcome, 'queued');
  assert.equal(assess(ticket, decision()).outcome, 'ready');
});

test('uses the assessed status on replay after a source write', () => {
  const before = assess(ticket, decision());
  const after = assess({ ...ticket, status: 'review', updated_at: '2026-09-27T12:01:00Z' }, decision());
  assert.equal(after.outcome, 'ready');
  assert.equal(after.actionKey, before.actionKey);
});

test('reassignment needs an accepted handoff and approval', () => {
  const assign = decision({ action: 'assign', value: 'Alice', evidence: {
    reason: 'Alice accepted the next turn on the source board.', references: [ref('accepted_handoff')],
  } });
  assert.equal(assess(ticket, assign).outcome, 'ready');
  assert.equal(assess(ticket, { ...assign, evidence: { ...assign.evidence, references: [ref('comment')] } }).outcome, 'queued');
});
