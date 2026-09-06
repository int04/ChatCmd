'use strict';
const assert = require('node:assert/strict');
const test = require('node:test');
const { createHash } = require('node:crypto');
const { protocol, job, BODY } = require('./compact-test-fixtures.cjs');
const { contentFixture } = require('./compact-test-content.cjs');

// Contract tests for the instructions, not claims that an LLM preserves all context.
test('new handoff prompt explains every operational section and prioritizes evidence over plans', () => {
  const p = protocol();
  const prompt = p.handoffPrompt(job());
  for (const heading of ['TASK', 'USER REQUIREMENTS', 'CURRENT STATE', 'VERIFIED WORK', 'IN PROGRESS',
    'PLANNED / DECIDED', 'FAILED / UNRESOLVED', 'FILES', 'VALIDATION', 'ENVIRONMENT', 'NEXT ACTIONS', 'DO NOT REPEAT']) {
    assert.ok(prompt.includes('\n' + heading + ' — '), `Missing section guidance: ${heading}`);
  }
  for (const rule of [/later corrections/i, /hypothes/i, /executionId/, /stale/i,
    /delegated/i, /installed/i, /not new permission/i, /70,000/, /do not call.*tools/i]) assert.match(prompt, rule);
  assert.ok(prompt.length < 12000, 'keep the input instruction bounded');
  assert.equal(prompt.split('\n')[0], p.marker('HANDOFF', job().id));
  assert.ok(prompt.includes(p.marker('HANDOFF-END', job().id)));
});

test('legacy handoff text remains byte-identical for already staged/sent jobs', () => {
  const p = protocol();
  const legacy = p.handoffPrompt(job(), 1);
  // Hash captured from the shipped 0.1.8 prompt before this change.
  assert.equal(createHash('sha256').update(legacy).digest('hex'), '466602796f0f55b349f9658c2a4d98d3300149bf293f579610d9c82a2c7bdac6');
  assert.notEqual(p.handoffPrompt(job()), legacy);
});

for (const version of [1, 2]) {
  test(`version ${version} exact prompt recovers its public handoff after content reload`, (t) => {
    const env = contentFixture(t);
    const value = job();
    env.user(env.protocol.handoffPrompt(value, version));
    env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
    env.reload();
    assert.equal(env.settled(value).handoffText, BODY);
  });

  test(`version ${version} already-staged draft sends once without being replaced by another version`, async (t) => {
    const env = contentFixture(t);
    const value = job();
    const prompt = env.protocol.handoffPrompt(value, version);
    env.composer().value = prompt;
    const token = await env.ready(value);
    assert.equal(env.composer().value, prompt);
    assert.equal(env.state.writes.length, 0);
    assert.equal((await env.message('dispatch', value, 'HANDOFF', token)).sent, true);
    assert.equal((await env.message('dispatch', value, 'HANDOFF', token)).sent, true);
    assert.equal(env.state.clicks, 1);
  });
}

test('accepting legacy format never permits a marker with changed instructions', (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user(env.protocol.handoffPrompt(value, 1) + '\nDifferent instructions.');
  env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
  assert.equal(env.settled(value).markerFound, false);
  assert.equal(env.settled(value).handoffText, null);
});

test('new and legacy prompts with the same operation still fail closed as duplicate turns', (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user(env.protocol.handoffPrompt(value, 1), 'legacy');
  env.user(env.protocol.handoffPrompt(value), 'new');
  assert.throws(() => env.probe(value), /nhiều tin nhắn/i);
});
