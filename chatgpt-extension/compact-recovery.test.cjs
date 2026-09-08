'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { job, BODY, clone } = require('./compact-test-fixtures.cjs');
const { workerFixture, world, receiver, PREFIX } = require('./compact-test-worker.cjs');

async function sourceWorker(t, value = job(), record = {}) {
  const env = await workerFixture(t);
  env.seed(value, record);
  env.shared.tabs = [{ id: 7, url: value.oldConversationUrl }];
  env.shared.route = receiver();
  return env;
}

test('source checkpoint and persistent dispatch fence precede Send; reload cannot resend', async (t) => {
  const env = await sourceWorker(t);
  await env.tick();
  assert.equal(env.serverJob().phase, 'writing_handoff');
  assert.equal(env.record().sourceSend, 'dispatched-unresolved');
  assert.equal(env.record().sourceDocument, 'document-1');
  assert.equal(env.sends('HANDOFF').length, 1);
  const effects = env.shared.effects;
  const checkpoint = effects.findIndex((entry) => entry.type === 'checkpoint' && entry.job.phase === 'writing_handoff');
  const fence = effects.findIndex((entry) => entry.type === 'persist'
    && entry.values[PREFIX + job().id]?.sourceSend === 'dispatched-unresolved');
  const click = effects.findIndex((entry) => entry.type === 'message' && entry.message.type === 'chatcmd-compact-dispatch');
  assert.ok(checkpoint >= 0 && checkpoint < fence && fence < click);
  await env.tick();
  const restarted = await env.restart();
  await restarted.tick();
  assert.notEqual(restarted.context, env.context);
  assert.equal(env.sends('HANDOFF').length, 1);
});

test('two concurrent runCompactJob calls share one source dispatch flight', async (t) => {
  const env = await sourceWorker(t);
  await Promise.all([env.run(), env.run(), env.run()]);
  assert.equal(env.sends('HANDOFF').length, 1);
  assert.equal(env.shared.checkpoints.filter((patch) => patch.phase === 'writing_handoff').length, 1);
  assert.equal(env.api.flights.size, 0);
});

test('persisted unresolved source fence prevents resend even if server still says preparing', async (t) => {
  const env = await sourceWorker(t, job(), { sourceSend: 'dispatched-unresolved', sourceDocument: 'old-document' });
  await env.tick();
  await (await env.restart()).tick();
  assert.equal(env.sends().length, 0);
  assert.equal(env.shared.calls.filter((call) => call.type === 'chatcmd-compact-prepare').length, 0);
  assert.equal(env.serverJob().phase, 'preparing');
  assert.ok(env.serverJob().detail);
});

test('lost writing checkpoint response cannot cause a source send on restart', async (t) => {
  const env = await sourceWorker(t);
  env.shared.afterCheckpoint = async (patch) => {
    if (patch.phase === 'writing_handoff') {
      env.shared.afterCheckpoint = null;
      throw new Error('Checkpoint committed; HTTP response lost');
    }
  };
  await assert.rejects(env.tick(), /response lost/);
  assert.equal(env.serverJob().phase, 'writing_handoff');
  assert.equal(env.record().sourceSend, 'not-attempted');
  const restarted = await env.restart();
  await restarted.tick();
  assert.equal(env.sends().length, 0, 'Unknown cross-document permit must fail closed');
});

test('source dispatch acknowledgement loss retains disk fence across fresh worker globals', async (t) => {
  const env = await sourceWorker(t);
  const normal = receiver();
  env.shared.route = async (tabId, message) => {
    if (message.type === 'chatcmd-compact-dispatch') throw new Error('Send happened but acknowledgement was lost');
    return normal(tabId, message);
  };
  await assert.rejects(env.tick(), /acknowledgement was lost/);
  assert.equal(env.record().sourceSend, 'dispatched-unresolved');
  env.shared.route = receiver();
  await (await env.restart()).tick();
  assert.equal(env.sends('HANDOFF').length, 1);
});

test('failure persisting source fence blocks click; server phase remains a recovery barrier', async (t) => {
  const env = await sourceWorker(t);
  env.shared.beforeStore = async (values) => {
    if (values[PREFIX + job().id]?.sourceSend === 'dispatched-unresolved') throw new Error('Storage quota failure');
  };
  await assert.rejects(env.tick(), /quota/);
  assert.equal(env.sends().length, 0);
  assert.equal(env.serverJob().phase, 'writing_handoff');
  env.shared.beforeStore = null;
  await (await env.restart()).tick();
  assert.equal(env.sends().length, 0);
});

test('lost handoff saving response resumes from durable body, never asks source again', async (t) => {
  const value = job({ phase: 'writing_handoff' });
  const env = await sourceWorker(t, value, { sourceSend: 'dispatched-unresolved' });
  env.shared.route = receiver({ markerFound: true, userMessageId: 'owned-user', handoffText: BODY });
  env.shared.afterCheckpoint = async (patch) => {
    if (patch.phase === 'saving_handoff') {
      env.shared.afterCheckpoint = null;
      throw new Error('Saved handoff response lost');
    }
  };
  await assert.rejects(env.tick(), /response lost/);
  assert.equal(env.serverJob().handoffText, BODY);
  await env.restart();
  assert.equal(env.serverJob().phase, 'opening_new_chat');
  assert.equal(env.shared.checkpoints.filter((patch) => patch.phase === 'saving_handoff').length, 1);
  assert.equal(env.sends().length, 0);
});

test('closed source pauses without recreating a tab or sending a new handoff', async (t) => {
  const env = await sourceWorker(t);
  env.shared.tabs = [];
  await env.tick();
  await (await env.restart()).tick();
  assert.equal(env.shared.creates.length, 0);
  assert.equal(env.sends().length, 0);
  assert.equal(env.serverJob().phase, 'preparing');
  assert.ok(env.serverJob().detail.includes('cũ'));
});

test('initial source opening is consumed before tab creation and never repeated after lost result', async (t) => {
  const env = await sourceWorker(t, job(), { initialOpenAllowed: true });
  env.shared.tabs = [];
  env.shared.afterCreate = async (tab) => {
    assert.equal(env.record().initialOpenAllowed, false);
    env.shared.tabs = env.shared.tabs.filter((item) => item.id !== tab.id);
    throw new Error('Tab created then closed before result arrived');
  };
  await assert.rejects(env.tick(), /before result/);
  env.shared.afterCreate = null;
  await (await env.restart()).tick();
  assert.equal(env.shared.creates.length, 1);
  assert.equal(env.sends().length, 0);
});

test('CAS conflict before the source dispatch cancels irreversible work', async (t) => {
  const env = await sourceWorker(t);
  env.shared.beforeCheckpoint = async (patch, id) => {
    if (patch.phase === 'writing_handoff') {
      const current = env.shared.jobs.get(id);
      env.shared.jobs.set(id, { ...current, revision: current.revision + 1, phase: 'cancelled' });
    }
  };
  await assert.rejects(env.tick(), /Stale compact revision/);
  assert.equal(env.sends().length, 0);
  assert.equal(env.record().sourceSend, 'not-attempted');
  await env.restart();
  assert.equal(env.serverJob().phase, 'cancelled');
  assert.equal(env.record().finished, true);
  assert.equal(env.shared.effects.filter((entry) => entry.type === 'bind').length, 0);
});

test('start rejects a fetched job for another task before writing storage or dispatching', async (t) => {
  const env = await workerFixture(t);
  const value = job();
  env.shared.jobs.set(value.id, clone(value));
  await assert.rejects(env.api.startCompactJob({
    jobId: value.id, taskId: 'different-task', localBaseUrl: 'http://127.0.0.1:8080',
  }, { tab: { id: 1, url: 'http://127.0.0.1:8080/tasks/task-chat-original' } }), /another task/);
  assert.equal(env.record(), undefined);
  assert.equal(env.sends().length, 0);
  assert.equal(env.shared.creates.length, 0);
});

test('start accepts only a local console and never replaces the authoritative task id', async (t) => {
  const env = await workerFixture(t);
  const value = job();
  env.shared.jobs.set(value.id, clone(value));
  const message = { jobId: value.id, taskId: value.taskId, localBaseUrl: 'http://127.0.0.1:8080' };
  await assert.rejects(env.api.startCompactJob(message, { tab: { id: 2, url: 'https://chatgpt.com/' } }), /local ChatCMD/);
  assert.equal(env.record(), undefined);
  // An existing record disables any initial automatic opening in this fixture.
  env.seed(value);
  const accepted = await env.api.startCompactJob(message, { tab: { id: 1, url: 'http://127.0.0.1:8080/tasks/original' } });
  await Promise.all([...env.api.flights.values()]);
  assert.equal(accepted.accepted, true);
  assert.equal(accepted.jobId, value.id);
  assert.equal(env.serverJob().taskId, value.taskId);
  assert.ok(env.shared.requests.every((request) => request.path.startsWith('/api/local/chatgpt/compact/')));
  assert.ok(env.shared.requests.every((request) => request.headers['X-ChatCmdClient'] === 'chatgpt-extension'));
});

test('pending server jobs with missing browser records recover without unsafe resurrection', async (t) => {
  for (const phase of ['writing_handoff', 'opening_new_chat']) {
    const env = await workerFixture(t);
    const value = job({ phase, handoffText: phase === 'opening_new_chat' ? BODY : null });
    env.shared.jobs.set(value.id, clone(value));
    env.shared.route = receiver();
    // In opening_new_chat, even a loaded source must not recreate an unknown destination.
    if (phase === 'opening_new_chat') env.shared.tabs = [{ id: 7, url: value.oldConversationUrl }];
    const restarted = await env.restart();
    assert.equal(restarted.record().sourceSend, 'unknown');
    if (phase === 'opening_new_chat') assert.equal(restarted.record().destinationSend, 'unknown');
    assert.equal(restarted.record().initialOpenAllowed, false);
    assert.equal(restarted.shared.creates.length, 0);
    assert.equal(restarted.sends().length, 0);
    assert.equal(restarted.shared.alarms.get('chatcmd-compact-recovery').periodInMinutes, 0.5);
  }
});

test('offline recovery preserves disk state and does not open or dispatch as a fallback', async (t) => {
  const env = await sourceWorker(t, job({ phase: 'writing_handoff' }), { sourceSend: 'dispatched-unresolved' });
  const before = env.record();
  env.shared.offline = true;
  await env.restart();
  assert.deepEqual(env.record(), before);
  assert.equal(env.shared.creates.length, 0);
  assert.equal(env.sends().length, 0);
});

test('idle recovery clears its alarm and prunes expired finished browser records', async (t) => {
  const shared = world();
  shared.alarms.set('chatcmd-compact-recovery', { periodInMinutes: 0.5 });
  shared.store[`${PREFIX}finished-old`] = {
    localBaseUrl: 'http://127.0.0.1:8080', finished: true,
    finishedAt: Date.now() - 25 * 60 * 60_000,
  };
  await workerFixture(t, shared);
  assert.equal(shared.store[`${PREFIX}finished-old`], undefined);
  assert.equal(shared.alarms.has('chatcmd-compact-recovery'), false);
});
