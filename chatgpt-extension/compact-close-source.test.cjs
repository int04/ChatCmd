'use strict';
const assert = require('node:assert/strict');
const test = require('node:test');
const { job, BODY } = require('./compact-test-fixtures.cjs');
const { contentFixture } = require('./compact-test-content.cjs');
const { workerFixture } = require('./compact-test-worker.cjs');

// Real content safety checks + real worker finalization; Chrome's effects alone are fixtures.
async function completed(t, patch = {}) {
  const env = await workerFixture(t);
  const value = env.seed(job({ phase: 'completed', handoffText: BODY,
    newConversationId: 'new-chat', newConversationUrl: 'https://chatgpt.com/c/new-chat',
    continueAfterCompact: false, ...patch }), { sourceChatTabId: 7, destinationTabId: 9 });
  const page = contentFixture(t);
  page.user(page.protocol.handoffPrompt(value), 'source-handoff-user');
  page.answer(BODY + '\n' + page.protocol.marker('HANDOFF-END', value.id));
  env.shared.tabs = [
    { id: 1, url: 'http://127.0.0.1:8080/tasks/task-chat-original', active: true, windowId: 1 },
    { id: 7, url: value.oldConversationUrl, active: false, windowId: 1 },
    { id: 9, url: value.newConversationUrl, active: false, windowId: 1 },
    { id: 8, url: 'https://chatgpt.com/c/unrelated', active: false, windowId: 1 },
  ];
  env.shared.route = (id, message) => id === 7
    ? page.message(message.type.replace('chatcmd-compact-', ''), message.job, message.kind, message.documentToken)
    : Promise.resolve({ ok: true });
  return { ...env, value, page, closes: () => env.shared.effects.filter((effect) => effect.type === 'close-tab').map((effect) => effect.tabId) };
}

test('completed attachment closes only its source, independently of continuation opt-in, and leaves history data', async (t) => {
  const env = await completed(t);
  await env.tick();
  assert.deepEqual(env.closes(), [7]);
  assert.deepEqual(env.shared.tabs.map((tab) => tab.id), [1, 9, 8]);
  assert.equal(env.record().finished, true);
  assert.equal(env.record().sourceClose.state, 'closed');
  assert.equal(env.serverJob().oldConversationUrl, env.value.oldConversationUrl);
  assert.equal(env.serverJob().handoffText, BODY);
  assert.equal(env.serverJob().taskId, env.value.taskId);
  assert.equal(env.shared.effects.some((effect) => effect.type === 'work-dispatch'), false);
  assert.equal(env.shared.effects.some((effect) => effect.type === 'update-tab'), false, 'do not steal focus from ChatCMD');
  const bound = env.shared.effects.findIndex((effect) => effect.type === 'bind');
  const closed = env.shared.effects.findIndex((effect) => effect.type === 'close-tab');
  assert.ok(bound >= 0 && bound < closed, 'attach before retiring source');
});

test('active source transfers selection to the destination in the same window before closing', async (t) => {
  const env = await completed(t);
  env.shared.tabs.find((tab) => tab.id === 7).active = true;
  await env.tick();
  assert.deepEqual(env.closes(), [7]);
  const activated = env.shared.effects.findIndex((effect) => effect.type === 'update-tab' && effect.tabId === 9 && effect.patch.active);
  const closed = env.shared.effects.findIndex((effect) => effect.type === 'close-tab');
  assert.ok(activated >= 0 && activated < closed);
});

for (const phase of ['preparing', 'writing_handoff', 'saving_handoff', 'opening_new_chat', 'cancelled']) {
  test(`does not close the source in phase ${phase}`, async (t) => {
    const env = await completed(t, { phase });
    await env.api.finishCompactBrowser(env.value, env.record());
    assert.deepEqual(env.closes(), []);
  });
}

test('missing destination preserves source until that exact destination reopens', async (t) => {
  const env = await completed(t);
  env.shared.tabs = env.shared.tabs.filter((tab) => tab.id !== 9);
  await env.tick();
  assert.deepEqual(env.closes(), []);
  assert.notEqual(env.record().finished, true);
  env.shared.tabs.push({ id: 90, url: env.value.newConversationUrl });
  await env.restart();
  assert.deepEqual(env.closes(), [7]);
});

for (const situation of ['unrelated-url', 'pending-navigation', 'new-draft', 'new-user', 'generating', 'missing-marker']) {
  test(`source retirement preserves user state: ${situation}`, async (t) => {
    const env = await completed(t);
    const tab = env.shared.tabs.find((item) => item.id === 7);
    if (situation === 'unrelated-url') tab.url = 'https://example.com/c/source-conversation';
    if (situation === 'pending-navigation') tab.pendingUrl = 'https://chatgpt.com/c/unrelated';
    if (situation === 'new-draft') env.page.composer().value = 'Keep my draft';
    if (situation === 'new-user') env.page.user('I continued in this old tab', 'later-user');
    if (situation === 'generating') env.page.generating(true);
    if (situation === 'missing-marker') env.page.w.document.querySelector('main').replaceChildren();
    // A second copy of the old URL is a reference tab, never a fallback close target.
    env.shared.tabs.push({ id: 77, url: env.value.oldConversationUrl });
    await env.tick();
    assert.deepEqual(env.closes(), []);
    assert.equal(env.record().sourceClose.state, 'skipped');
    assert.equal(env.record().finished, true);
    assert.ok(env.shared.tabs.some((item) => item.id === 77));
  });
}

test('reopened archived links are never auto-closed on later worker wake or duplicate finalization', async (t) => {
  const env = await completed(t);
  await env.tick();
  assert.deepEqual(env.closes(), [7]);
  env.shared.tabs.push({ id: 7, url: env.value.oldConversationUrl }, { id: 77, url: env.value.oldConversationUrl });
  env.page.reload();
  const restarted = await env.restart();
  await restarted.tick();
  assert.deepEqual(env.closes(), [7]);
  assert.ok(env.shared.tabs.some((tab) => tab.id === 7));
  assert.ok(env.shared.tabs.some((tab) => tab.id === 77));
});

test('navigation after close intent is persisted cannot close the reused source tab', async (t) => {
  const env = await completed(t);
  env.shared.beforeStore = async (values) => {
    if (Object.values(values).some((value) => value?.sourceClose?.state === 'closing')) {
      env.shared.tabs.find((tab) => tab.id === 7).url = 'https://chatgpt.com/c/something-else';
    }
  };
  await env.tick();
  assert.deepEqual(env.closes(), []);
});

test('draft typed after close intent is persisted is checked again before removal', async (t) => {
  const env = await completed(t);
  env.shared.beforeStore = async (values) => {
    if (Object.values(values).some((value) => value?.sourceClose?.state === 'closing')) env.page.composer().value = 'Just typed';
  };
  await env.tick();
  assert.deepEqual(env.closes(), []);
  assert.equal(env.page.composer().value, 'Just typed');
});

test('failed removal leaves recoverable cleanup; restart retries only the same document', async (t) => {
  const env = await completed(t);
  env.shared.beforeRemove = async () => { throw new Error('Chrome temporarily refused'); };
  await env.run();
  assert.deepEqual(env.closes(), []);
  assert.notEqual(env.record().finished, true);
  assert.equal(env.record().sourceClose.state, 'closing');
  env.shared.beforeRemove = null;
  await env.restart();
  assert.deepEqual(env.closes(), [7]);
  assert.equal(env.record().finished, true);
});

test('lost removal reply and a reused tab id cannot close a newly opened reference document', async (t) => {
  const env = await completed(t);
  env.shared.afterRemove = async () => {
    env.shared.tabs.push({ id: 7, url: env.value.oldConversationUrl });
    env.page.reload();
    throw new Error('Removed, response lost');
  };
  await env.run();
  assert.deepEqual(env.closes(), [7]);
  assert.notEqual(env.record().finished, true);
  env.shared.afterRemove = null;
  await env.restart();
  assert.deepEqual(env.closes(), [7]);
  assert.ok(env.shared.tabs.some((tab) => tab.id === 7));
});

for (const protectedId of [1, 9]) {
  test(`a stale source binding cannot close the ChatCMD/destination tab ${protectedId}`, async (t) => {
    const env = await completed(t);
    const record = env.record();
    record.sourceChatTabId = protectedId;
    await env.api.finishCompactBrowser(env.value, record);
    assert.deepEqual(env.closes(), []);
    assert.equal(env.record().sourceClose.state, 'skipped');
  });
}

test('temporary source-close failure does not block opted-in work or duplicate its dispatch on recovery', async (t) => {
  const env = await completed(t, { continueAfterCompact: true });
  env.shared.resumes.set(env.value.id, { requestId: 'continue-owned' });
  env.shared.requestsById.set('continue-owned', {
    id: 'continue-owned', taskId: env.value.taskId, status: 'queued', submittedContent: 'Continue the same task.',
  });
  env.shared.beforeRemove = async () => { throw new Error('Chrome refused removal'); };
  await env.run();
  assert.equal(env.shared.effects.filter((effect) => effect.type === 'work-dispatch').length, 1);
  assert.notEqual(env.record().finished, true);
  env.shared.beforeRemove = null;
  await env.restart();
  assert.deepEqual(env.closes(), [7]);
  assert.equal(env.shared.effects.filter((effect) => effect.type === 'work-dispatch').length, 1);
  assert.equal(env.record().finished, true);
});

test('storage failure before close intent prevents tab removal', async (t) => {
  const env = await completed(t);
  env.shared.beforeStore = async (values) => {
    if (Object.values(values).some((value) => value?.sourceClose)) throw new Error('Storage unavailable');
  };
  await env.run();
  assert.deepEqual(env.closes(), []);
  assert.notEqual(env.record().finished, true);
});
