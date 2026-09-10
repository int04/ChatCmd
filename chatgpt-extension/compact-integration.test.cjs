'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { job, BODY } = require('./compact-test-fixtures.cjs');
const { contentFixture } = require('./compact-test-content.cjs');
const { workerFixture } = require('./compact-test-worker.cjs');

// This joins actual service-worker functions to the actual content listener, DOM
// controls, transcript parser and protocol. Only external browser/HTTP boundaries
// and the synthetic ChatGPT reply are fixtures; no compact decision is mocked.
async function integrated(t) {
  const worker = await workerFixture(t);
  const value = worker.seed();
  const source = contentFixture(t);
  const pages = new Map([[7, source]]);
  worker.shared.tabs = [{ id: 1, url: 'http://127.0.0.1:8080/tasks/task-chat-original' },
    { id: 7, url: value.oldConversationUrl }];
  let destination;
  const state = { dropDispatchKind: null };
  source.state.onClick = () => {
    source.user(source.composer().value, 'source-user');
    source.composer().value = '';
  };
  worker.shared.afterCreate = async (tab) => {
    destination = contentFixture(t, { url: tab.url });
    pages.set(tab.id, destination);
    destination.state.onClick = () => {
      destination.user(destination.composer().value, 'destination-user');
      destination.composer().value = '';
      tab.url = 'https://chatgpt.com/c/destination-canonical'; // ChatGPT drops the hash.
      destination.navigate(tab.url);
      destination.answer('Handoff received.');
    };
  };
  worker.shared.route = async (id, message) => {
    const page = pages.get(id);
    if (!page) throw new Error(`Unexpected tab ${id}`);
    const result = await page.message(message.type.replace('chatcmd-compact-', ''),
      message.job, message.kind, message.documentToken);
    if (message.type === 'chatcmd-compact-dispatch' && state.dropDispatchKind === message.kind) {
      state.dropDispatchKind = null;
      throw new Error('Send happened; response dropped');
    }
    return result;
  };
  async function saveHandoff() {
    await worker.tick();
    assert.equal(source.state.clicks, 1);
    source.answer(BODY + '\n' + source.protocol.marker('HANDOFF-END', value.id));
    source.probe(worker.serverJob());
    source.advance(1201);
    await worker.tick();
    assert.equal(worker.serverJob().phase, 'opening_new_chat');
    assert.equal(worker.serverJob().handoffText, BODY);
    assert.ok(destination, 'destination opens in the same worker flight after the durable handoff checkpoints');
  }
  async function openDestination() {
    await saveHandoff();
    assert.ok(destination);
    assert.equal(destination.state.clicks, 0);
    assert.ok(worker.shared.tabs.some((tab) => tab.id === 7), 'source stays until destination is attached');
    assert.equal(worker.shared.effects.filter((effect) => effect.type === 'close-tab').length, 0);
    return destination;
  }
  return { worker, source, pages, state, value, saveHandoff, openDestination,
    destination: () => destination };
}

test('source send recovers when ChatGPT renders the owned user turn without data-message-id', async (t) => {
  const worker = await workerFixture(t);
  const value = worker.seed();
  const source = contentFixture(t);
  worker.shared.tabs = [{ id: 7, url: value.oldConversationUrl }];
  source.state.onClick = () => {
    source.user(source.composer().value, null);
    source.composer().value = '';
  };
  worker.shared.route = (_id, message) => source.message(message.type.replace('chatcmd-compact-', ''),
    message.job, message.kind, message.documentToken);

  await worker.tick();
  assert.equal(source.state.clicks, 1);
  assert.equal(worker.serverJob().phase, 'writing_handoff');
  assert.equal(worker.record().sourceSend, 'dispatched-unresolved');

  source.answer(BODY + '\n' + source.protocol.marker('HANDOFF-END', value.id));
  source.probe(worker.serverJob());
  source.advance(1201);
  await worker.tick();

  assert.equal(worker.serverJob().phase, 'opening_new_chat');
  assert.equal(worker.serverJob().handoffText, BODY);
  assert.equal(worker.shared.creates.length, 1, 'the worker must leave Writing the handoff instead of stalling on its durable send fence');
  assert.equal(worker.sends('HANDOFF').length, 1, 'recovery must not resend the handoff');
});

test('destination attach recovers when ChatGPT renders the resume turn without data-message-id', async (t) => {
  const env = await integrated(t);
  const destination = await env.openDestination();
  const tab = env.worker.shared.tabs.find((item) => item.id !== 1 && item.id !== 7);
  assert.ok(tab, 'destination tab');
  destination.state.onClick = () => {
    destination.user(destination.composer().value, null);
    destination.composer().value = '';
    tab.url = 'https://chatgpt.com/c/destination-no-native-id';
    destination.navigate(tab.url);
    destination.answer('Handoff received.');
  };

  await env.worker.tick();
  assert.equal(destination.state.clicks, 1);
  assert.equal(env.worker.serverJob().phase, 'opening_new_chat');
  assert.equal(env.worker.record().destinationSend, 'dispatched-unresolved');

  await env.worker.tick();
  assert.equal(env.worker.serverJob().phase, 'completed');
  assert.equal(env.worker.serverJob().newConversationId, 'destination-no-native-id');
  assert.equal(env.worker.sends('RESUME').length, 1, 'destination recovery must not resend the resume handoff');
});

test('user continuing immediately after RESUME acknowledgement still completes same-task attachment', async (t) => {
  const env = await integrated(t);
  const destination = await env.openDestination();
  await env.worker.tick();
  assert.equal(destination.state.clicks, 1);
  assert.equal(env.worker.serverJob().phase, 'opening_new_chat');
  destination.user('tiếp tục công việc', 'working-user');
  destination.answer('conversation_compacting_or_archived', { id: 'blocked-working-answer' });
  await env.worker.tick();
  assert.equal(env.worker.serverJob().phase, 'completed');
  assert.equal(env.worker.serverJob().taskId, env.value.taskId);
  assert.equal(env.worker.serverJob().newConversationId, 'destination-canonical');
  assert.equal(env.worker.sends('RESUME').length, 1);
  assert.equal(env.worker.shared.creates.length, 1);
});

test('real content-worker round trip saves exact handoff, preserves task/model, and sends once per chat', async (t) => {
  const env = await integrated(t);
  const dest = await env.openDestination();
  await env.worker.tick(); // Prepare + irreversible resume click.
  assert.equal(dest.state.clicks, 1);
  assert.deepEqual(dest.state.models, [env.value.oldModel]);
  assert.equal(env.worker.serverJob().newConversationId, null);
  await env.worker.tick(); // Persist canonical identity, re-probe it, then complete in the same worker flight.
  assert.equal(env.worker.serverJob().newConversationId, 'destination-canonical');
  const completed = env.worker.serverJob();
  assert.equal(completed.phase, 'completed');
  assert.equal(completed.taskId, env.value.taskId);
  assert.equal(completed.oldConversationId, env.value.oldConversationId);
  assert.equal(completed.handoffText, BODY);
  assert.deepEqual(env.worker.shared.effects.filter((effect) => effect.type === 'close-tab').map((effect) => effect.tabId), [7]);
  const committed = env.worker.shared.effects.findIndex((effect) => effect.type === 'checkpoint' && effect.job.phase === 'completed');
  const retired = env.worker.shared.effects.findIndex((effect) => effect.type === 'close-tab');
  assert.ok(committed >= 0 && committed < retired, 'persist same-task replacement before closing source');
  assert.ok(env.worker.shared.tabs.some((tab) => tab.id === 1), 'ChatCMD tab remains open');
  assert.equal(env.worker.shared.creates.length, 1);
  assert.equal(env.source.state.clicks, 1);
  assert.equal(dest.state.clicks, 1);
  assert.equal(env.source.w.document.querySelector('[data-chatcmd-ui="compact"]'), null);
  assert.equal(dest.w.document.querySelector('[data-chatcmd-ui="compact"]'), null);
  const submitted = dest.w.ChatCmdTranscript.latestUser().content;
  assert.equal(submitted, dest.protocol.resumePrompt({ ...env.value, handoffText: BODY }));
  const phases = env.worker.shared.checkpoints.filter((patch) => patch.phase).map((patch) => patch.phase);
  assert.deepEqual(phases, ['writing_handoff', 'saving_handoff', 'opening_new_chat', 'completed']);
  assert.ok(env.worker.shared.checkpoints.every((patch) => patch.id === env.value.id && !('taskId' in patch)));
  assert.ok(env.worker.shared.requests.every((request) => request.path.startsWith('/api/local/chatgpt/compact/')));
  await env.worker.restart();
  assert.equal(env.source.state.clicks + dest.state.clicks, 2);
  assert.equal(env.worker.shared.creates.length, 1);
});

test('actual source click with lost response is never repeated after content AND worker reload', async (t) => {
  const env = await integrated(t);
  env.state.dropDispatchKind = 'HANDOFF';
  await assert.rejects(env.worker.tick(), /response dropped/);
  assert.equal(env.source.state.clicks, 1);
  const oldToken = env.source.probe(env.worker.serverJob()).documentToken;
  env.source.reload();
  assert.notEqual(env.source.probe(env.worker.serverJob()).documentToken, oldToken);
  const restarted = await env.worker.restart();
  await restarted.tick();
  assert.equal(env.source.state.clicks, 1);
  assert.equal(env.worker.sends('HANDOFF').length, 1);
  assert.equal(env.worker.serverJob().phase, 'writing_handoff');
  assert.equal(env.worker.shared.creates.length, 0);
});

test('actual destination click with lost response recovers via exact user marker after hash removal', async (t) => {
  const env = await integrated(t);
  const dest = await env.openDestination();
  env.state.dropDispatchKind = 'RESUME';
  await assert.rejects(env.worker.tick(), /response dropped/);
  assert.equal(dest.state.clicks, 1);
  assert.equal(dest.w.location.hash, '');
  dest.reload();
  const restarted = await env.worker.restart();
  await restarted.tick();
  assert.equal(restarted.serverJob().phase, 'completed');
  assert.equal(restarted.serverJob().taskId, env.value.taskId);
  assert.equal(env.worker.shared.creates.length, 1);
  assert.equal(env.source.state.clicks, 1);
  assert.equal(dest.state.clicks, 1);
});

test('lost final checkpoint response finishes browser cleanup after restart without duplicate dispatch', async (t) => {
  const env = await integrated(t);
  const dest = await env.openDestination();
  await env.worker.tick(); // Dispatch resume; the next tick can commit completion.
  env.worker.shared.afterCheckpoint = async (patch) => {
    if (patch.phase === 'completed') {
      env.worker.shared.afterCheckpoint = null;
      throw new Error('Completion committed, response lost');
    }
  };
  await assert.rejects(env.worker.tick(), /response lost/);
  assert.equal(env.worker.serverJob().phase, 'completed');
  assert.notEqual(env.worker.record().finished, true);
  assert.equal(env.worker.shared.effects.filter((effect) => effect.type === 'close-tab').length, 0);
  const restarted = await env.worker.restart();
  assert.equal(restarted.record().finished, true);
  assert.deepEqual(env.worker.shared.effects.filter((effect) => effect.type === 'close-tab').map((effect) => effect.tabId), [7]);
  assert.equal(env.worker.shared.creates.length, 1);
  assert.equal(env.source.state.clicks, 1);
  assert.equal(dest.state.clicks, 1);
  assert.equal(env.worker.shared.effects.filter((effect) => effect.type === 'bind').length, 1);
});

test('parallel tasks retain independent prompts, capture ownership and persistent source fences', async (t) => {
  const worker = await workerFixture(t);
  const pages = new Map();
  const jobs = [job(), job({ id: 'compact-job-0002', taskId: 'task-chat-second',
    oldConversationId: 'source-second', oldConversationUrl: 'https://chatgpt.com/c/source-second' })];
  for (const [index, value] of jobs.entries()) {
    worker.seed(value);
    const page = contentFixture(t, { url: value.oldConversationUrl });
    const tabId = 7 + index;
    worker.shared.tabs.push({ id: tabId, url: value.oldConversationUrl });
    pages.set(tabId, page);
    page.state.onClick = () => { page.user(page.composer().value, 'user-' + index); page.composer().value = ''; };
  }
  worker.shared.route = (id, message) => pages.get(id).message(message.type.replace('chatcmd-compact-', ''),
    message.job, message.kind, message.documentToken);
  await Promise.all(jobs.map((value) => worker.run(value.id)));
  for (const [index, value] of jobs.entries()) {
    const page = pages.get(7 + index);
    assert.equal(page.state.clicks, 1);
    page.answer(BODY + '\n' + value.taskId + '\n' + page.protocol.marker('HANDOFF-END', value.id));
    page.probe(worker.serverJob(value.id));
    page.advance(1201);
  }
  await Promise.all(jobs.map((value) => worker.run(value.id)));
  await worker.restart();
  for (const [index, value] of jobs.entries()) {
    assert.equal(worker.serverJob(value.id).handoffText, BODY + '\n' + value.taskId);
    assert.equal(worker.serverJob(value.id).taskId, value.taskId);
    assert.equal(worker.record(value.id).sourceSend, 'dispatched-unresolved');
    assert.equal(pages.get(7 + index).state.clicks, 1);
  }
  assert.equal(worker.sends('HANDOFF').length, 2);
});
