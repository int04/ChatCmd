'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { job, BODY } = require('./compact-test-fixtures.cjs');
const { workerFixture, receiver, PREFIX } = require('./compact-test-worker.cjs');

const opening = (patch = {}) => job({ phase: 'opening_new_chat', handoffText: BODY, ...patch });
const tagged = (value, id = 9) => ({ id, url: 'https://chatgpt.com/#chatcmd-compact=' + value.id });

async function destinationWorker(t, patch = {}, record = {}) {
  const env = await workerFixture(t);
  const value = opening(patch);
  env.seed(value, { sourceSend: 'dispatched-unresolved', ...record });
  env.shared.tabs = [{ id: 7, url: value.oldConversationUrl }, tagged(value)];
  env.shared.route = receiver();
  return env;
}

test('destination recovery uses the durable resume marker, not a recycled tab id or unrelated conversation', async (t) => {
  const env = await destinationWorker(t, {}, { destinationTabId: 41, destinationOpened: true });
  const value = env.serverJob();
  const tabs = [{ id: 7, url: value.oldConversationUrl },
    { id: 41, url: 'https://chatgpt.com/c/unrelated-reused-id' },
    { id: 42, url: 'https://chatgpt.com/' },
    { id: 50, url: 'https://chatgpt.com/c/owned-after-restart' }];
  env.shared.route = async (id, message) => {
    assert.equal(message.type, 'chatcmd-compact-locate');
    assert.equal(message.kind, 'RESUME');
    return { ok: true, markerFound: id === 50 };
  };
  const found = await env.api.locateCompactDestination(value, env.record(), tabs);
  assert.equal(found.id, 50);
  assert.deepEqual(env.shared.calls.filter((call) => call.type === 'chatcmd-compact-locate')
    .map((call) => call.tabId), [41, 42, 50]);
});

test('known destination identity falls back to the exact RESUME marker when Chrome URL is stale', async (t) => {
  const env = await destinationWorker(t, {
    newConversationId: 'destination-owned', newConversationUrl: 'https://chatgpt.com/c/destination-owned',
  }, { destinationTabId: 9, destinationOpened: true, destinationSend: 'dispatched-unresolved' });
  const tabs = [{ id: 9, url: 'https://chatgpt.com/' }];
  env.shared.route = async (id, message) => {
    assert.equal(message.type, 'chatcmd-compact-locate');
    assert.equal(message.kind, 'RESUME');
    return { ok: true, markerFound: id === 9 };
  };
  const found = await env.api.locateCompactDestination(env.serverJob(), env.record(), tabs);
  assert.equal(found.id, 9);
});

test('opening_new_chat completes when Chrome still reports home for the already attached destination', async (t) => {
  const env = await destinationWorker(t, {
    newConversationId: 'destination-owned', newConversationUrl: 'https://chatgpt.com/c/destination-owned',
  }, { destinationTabId: 9, destinationOpened: true, destinationSend: 'dispatched-unresolved' });
  env.shared.tabs = [{ id: 9, url: 'https://chatgpt.com/' }];
  const probe = receiver({ markerFound: true, generating: false, conversationId: 'destination-owned',
    conversationUrl: 'https://chatgpt.com/c/destination-owned' });
  env.shared.route = (id, message) => message.type === 'chatcmd-compact-locate'
    ? { ok: true, markerFound: id === 9 } : probe(id, message);
  await env.tick();
  assert.equal(env.serverJob().phase, 'completed');
  assert.equal(env.serverJob().taskId, job().taskId);
  assert.equal(env.shared.creates.length, 0);
  assert.equal(env.sends().length, 0);
});

test('lost destination tab id still recovers an open stale-url tab by exact RESUME marker', async (t) => {
  const env = await destinationWorker(t, {}, {
    destinationOpened: true, destinationSend: 'dispatched-unresolved', destinationTabId: undefined,
  });
  env.shared.tabs = [
    { id: 7, url: env.serverJob().oldConversationUrl },
    { id: 40, url: 'https://chatgpt.com/' },
    { id: 50, url: 'https://chatgpt.com/g/g-p-p/project' },
  ];
  const ownedProbe = receiver({ markerFound: true, generating: false, conversationId: 'destination-owned',
    conversationUrl: 'https://chatgpt.com/c/destination-owned' });
  env.shared.route = (id, message) => {
    if (message.type === 'chatcmd-compact-locate') return { ok: true, markerFound: id === 50 };
    return ownedProbe(id, message);
  };

  await env.tick();

  assert.equal(env.serverJob().phase, 'completed');
  assert.equal(env.serverJob().newConversationId, 'destination-owned');
  assert.equal(env.record().destinationTabId, 50);
  assert.equal(env.shared.creates.length, 0);
  assert.equal(env.sends().length, 0);
});

test('unrelated blank tabs are never adopted when persisted destination binding is absent', async (t) => {
  const env = await destinationWorker(t);
  const tabs = [{ id: 70, url: 'https://chatgpt.com/' }, { id: 71, url: 'https://chatgpt.com/g/g-p-p/project' }];
  assert.equal(await env.api.locateCompactDestination(env.serverJob(), env.record(), tabs), null);
  assert.equal(env.shared.calls.length, 0);
});

test('destination operation hash must match exactly, not a longer job id prefix', async (t) => {
  const env = await destinationWorker(t);
  const value = env.serverJob();
  const unrelated = { id: 90, url: tagged(value).url + '-another-job' };
  assert.equal(await env.api.locateCompactDestination(value, env.record(), [unrelated]), null);
});

test('multiple tagged destinations fail closed before any probe or send', async (t) => {
  const env = await destinationWorker(t);
  const value = env.serverJob();
  await assert.rejects(env.api.locateCompactDestination(value, env.record(), [tagged(value, 90), tagged(value, 91)]), /nhiều tab/);
  assert.equal(env.shared.calls.length, 0);
});

test('multiple canonical conversations with the resume marker never select an arbitrary winner', async (t) => {
  const env = await destinationWorker(t);
  env.shared.route = async () => ({ ok: true, markerFound: true });
  await assert.rejects(env.api.locateCompactDestination(env.serverJob(), env.record(), [
    { id: 90, url: 'https://chatgpt.com/c/copy-one' }, { id: 91, url: 'https://chatgpt.com/c/copy-two' },
  ]), /Hai cuộc trò chuyện/);
  assert.equal(env.sends().length, 0);
});

test('closed destination pauses without creating another chat across worker restart', async (t) => {
  const env = await destinationWorker(t, {}, { destinationTabId: 9, destinationOpened: true, destinationSend: 'dispatched-unresolved' });
  env.shared.tabs = env.shared.tabs.filter((tab) => tab.id !== 9);
  await env.tick();
  await (await env.restart()).tick();
  assert.equal(env.shared.creates.length, 0);
  assert.equal(env.sends().length, 0);
  assert.equal(env.serverJob().phase, 'opening_new_chat');
  assert.equal(env.serverJob().handoffText, BODY);
  assert.ok(env.serverJob().detail.includes('mở lại'));
});

test('first destination opening waits for closed source; durable handoff remains intact', async (t) => {
  const env = await destinationWorker(t);
  env.shared.tabs = [];
  await env.tick();
  assert.equal(env.shared.creates.length, 0);
  assert.equal(env.sends().length, 0);
  assert.equal(env.serverJob().handoffText, BODY);
  assert.ok(env.serverJob().detail.includes('cũ'));
});

test('destination opening intent is persisted before create and survives an unknown create result', async (t) => {
  const env = await destinationWorker(t);
  env.shared.tabs = env.shared.tabs.filter((tab) => tab.id === 7);
  env.shared.afterCreate = async (tab) => {
    assert.equal(env.record().destinationOpened, true);
    assert.equal(env.record().destinationUrl, tab.url);
    env.shared.tabs = env.shared.tabs.filter((item) => item.id !== tab.id);
    throw new Error('Created destination closed before response');
  };
  await assert.rejects(env.tick(), /before response/);
  env.shared.afterCreate = null;
  await (await env.restart()).tick();
  assert.equal(env.shared.creates.length, 1);
  assert.equal(env.sends().length, 0);
});

test('destination fence is durable before send and blocks resends until a canonical marker appears', async (t) => {
  const env = await destinationWorker(t);
  await env.tick();
  assert.equal(env.record().destinationSend, 'dispatched-unresolved');
  const effects = env.shared.effects;
  const fence = effects.findIndex((entry) => entry.type === 'persist'
    && entry.values[PREFIX + job().id]?.destinationSend === 'dispatched-unresolved');
  const dispatch = effects.findIndex((entry) => entry.type === 'message' && entry.message.type === 'chatcmd-compact-dispatch');
  assert.ok(fence >= 0 && fence < dispatch);
  await env.tick();
  await (await env.restart()).tick();
  assert.equal(env.sends('RESUME').length, 1);
  assert.equal(env.serverJob().phase, 'opening_new_chat');
});

test('lost destination pre-dispatch checkpoint response retries safely with at most one send', async (t) => {
  const env = await destinationWorker(t);
  let fault = true;
  env.shared.afterCheckpoint = async (patch) => {
    if (fault && !patch.phase && patch.detail?.includes('Đang chuyển')) {
      fault = false;
      throw new Error('Pre-dispatch checkpoint response lost');
    }
  };
  await assert.rejects(env.tick(), /response lost/);
  assert.equal(env.sends().length, 0);
  assert.equal(env.record().destinationSend, 'not-attempted');
  const restarted = await env.restart();
  await restarted.tick();
  await restarted.tick();
  assert.equal(env.sends('RESUME').length, 1);
});

test('CAS cancellation after prepare prevents destination dispatch', async (t) => {
  const env = await destinationWorker(t);
  env.shared.beforeCheckpoint = async (_patch, id) => {
    const current = env.shared.jobs.get(id);
    env.shared.jobs.set(id, { ...current, phase: 'cancelled', revision: current.revision + 1 });
  };
  await assert.rejects(env.tick(), /Stale compact revision/);
  assert.equal(env.sends().length, 0);
  assert.equal(env.record().destinationSend, 'not-attempted');
});

test('lost destination identity checkpoint response recovers canonical marker without another send', async (t) => {
  const env = await destinationWorker(t, {}, { destinationOpened: true, destinationSend: 'dispatched-unresolved' });
  env.shared.tabs = [{ id: 9, url: 'https://chatgpt.com/c/destination-owned' }];
  const probe = receiver({ markerFound: true, conversationId: 'destination-owned',
    conversationUrl: 'https://chatgpt.com/c/destination-owned' });
  env.shared.route = (id, message) => message.type === 'chatcmd-compact-locate'
    ? { ok: true, markerFound: true } : probe(id, message);
  env.shared.afterCheckpoint = async (patch) => {
    if (patch.newConversationId && !patch.phase) {
      env.shared.afterCheckpoint = null;
      throw new Error('Identity checkpoint response lost');
    }
  };
  await assert.rejects(env.tick(), /response lost/);
  assert.equal(env.serverJob().newConversationId, 'destination-owned');
  await env.restart();
  assert.equal(env.serverJob().phase, 'completed');
  assert.equal(env.serverJob().taskId, job().taskId);
  assert.equal(env.sends().length, 0);
  assert.equal(env.shared.creates.length, 0);
});

test('manual continuation commits destination binding before bootstrap acknowledgement finishes', async (t) => {
  const env = await destinationWorker(t, {
    continueAfterCompact: false,
    newConversationId: 'destination-owned',
    newConversationUrl: 'https://chatgpt.com/c/destination-owned',
  }, { destinationOpened: true, destinationSend: 'dispatched-unresolved', destinationTabId: 9 });
  env.shared.tabs = [{ id: 9, url: 'https://chatgpt.com/c/destination-owned' }];
  env.shared.route = receiver({ markerFound: true, generating: true, conversationId: 'destination-owned',
    conversationUrl: 'https://chatgpt.com/c/destination-owned' });

  await env.tick();

  assert.equal(env.serverJob().phase, 'completed');
  assert.equal(env.serverJob().taskId, job().taskId);
  assert.equal(env.record().finished, true);
  const bind = env.shared.effects.find((entry) => entry.type === 'bind');
  assert.deepEqual(bind.args, ['destination-owned', 9, { localBaseUrl: 'http://127.0.0.1:8080', requestId: null }]);
  assert.equal(env.shared.requests.filter((request) => request.path.endsWith('/resume')).length, 0);
  assert.equal(env.sends().length, 0);
});

test('automatic continuation still waits for bootstrap acknowledgement generation', async (t) => {
  const env = await destinationWorker(t, {
    continueAfterCompact: true,
    newConversationId: 'destination-owned',
    newConversationUrl: 'https://chatgpt.com/c/destination-owned',
  }, { destinationOpened: true, destinationSend: 'dispatched-unresolved', destinationTabId: 9 });
  env.shared.tabs = [{ id: 9, url: 'https://chatgpt.com/c/destination-owned' }];
  env.shared.route = receiver({ markerFound: true, generating: true, conversationId: 'destination-owned',
    conversationUrl: 'https://chatgpt.com/c/destination-owned' });

  await env.tick();

  assert.equal(env.serverJob().phase, 'opening_new_chat');
  assert.equal(env.shared.effects.filter((entry) => entry.type === 'bind').length, 0);
  assert.equal(env.shared.requests.filter((request) => request.path.endsWith('/resume')).length, 0);
});

test('tab navigated to an unrelated conversation is not reused as an empty destination', async (t) => {
  const env = await destinationWorker(t, {}, { destinationTabId: 9, destinationOpened: true });
  env.shared.tabs[1].url = 'https://chatgpt.com/c/unrelated';
  await env.tick();
  assert.equal(env.sends().length, 0);
  assert.equal(env.shared.creates.length, 0);
  assert.equal(env.shared.effects.filter((entry) => entry.type === 'bind').length, 0);
});

async function completedWorker(t, continueAfterCompact = true) {
  const env = await destinationWorker(t, { phase: 'completed', continueAfterCompact, newConversationId: 'destination-owned',
    newConversationUrl: 'https://chatgpt.com/c/destination-owned' }, { destinationTabId: 9, destinationOpened: true });
  env.shared.tabs = [{ id: 9, url: 'https://chatgpt.com/c/destination-owned' }];
  env.shared.resumes.set(job().id, { requestId: 'resume-request-original' });
  env.shared.requestsById.set('resume-request-original', {
    id: 'resume-request-original', taskId: job().taskId, status: 'queued', submittedContent: 'Continue this existing task.',
  });
  return env;
}

test('post-completion working continuation is persistently fenced and uses canonical conversation', async (t) => {
  const env = await completedWorker(t);
  env.shared.onWorkDispatch = async (message) => {
    assert.equal(env.record().resumeDispatch, 'dispatched-unresolved');
    assert.equal(env.record().resumeRequestId, message.requestId);
  };
  await env.tick();
  const work = env.shared.effects.find((effect) => effect.type === 'work-dispatch');
  assert.equal(work.message.requestId, 'resume-request-original');
  assert.equal(work.message.conversationUrl, 'https://chatgpt.com/c/destination-owned');
  assert.equal(work.message.model, job().oldModel);
  assert.equal(work.message.submittedContent, 'Continue this existing task.');
  assert.equal(env.serverJob().taskId, job().taskId);
  await (await env.restart()).tick();
  assert.equal(env.shared.effects.filter((effect) => effect.type === 'work-dispatch').length, 1);
});

test('working continuation failure reports requestId then localBaseUrl, not reversed arguments', async (t) => {
  const env = await completedWorker(t);
  env.shared.onWorkDispatch = async () => { throw new Error('Working request dispatch failed'); };
  await env.tick();
  const failure = env.shared.effects.find((effect) => effect.type === 'work-failure');
  assert.ok(failure, 'Failure must reach the ordinary bridge reporter');
  assert.deepEqual(failure.args, ['resume-request-original', 'http://127.0.0.1:8080', 'Working request dispatch failed']);
});

test('already running working continuation is not dispatched again', async (t) => {
  const env = await completedWorker(t);
  env.shared.requestsById.get('resume-request-original').status = 'running';
  await env.tick();
  assert.equal(env.shared.effects.filter((effect) => effect.type === 'work-dispatch').length, 0);
  assert.equal(env.record().finished, true);
});

test('completed job with closed destination waits to resume work until that exact chat reopens', async (t) => {
  const env = await completedWorker(t);
  env.shared.tabs = [];
  await env.tick();
  assert.notEqual(env.record().finished, true);
  assert.equal(env.shared.effects.filter((effect) => effect.type === 'work-dispatch').length, 0);
  assert.equal(env.shared.creates.length, 0);
  env.shared.tabs = [{ id: 90, url: 'https://chatgpt.com/c/destination-owned' }];
  await env.restart();
  assert.equal(env.record().finished, true);
  assert.equal(env.shared.effects.filter((effect) => effect.type === 'work-dispatch').length, 1);
  assert.equal(env.shared.creates.length, 0);
});

test('provisional conversation id cannot complete task rebinding', async (t) => {
  const env = await destinationWorker(t);
  env.shared.route = receiver({ markerFound: true, conversationId: 'WEB:provisional',
    conversationUrl: 'https://chatgpt.com/c/WEB:provisional' });
  await assert.rejects(env.tick());
  assert.equal(env.serverJob().phase, 'opening_new_chat');
  assert.equal(env.shared.effects.filter((entry) => entry.type === 'bind').length, 0);
  assert.equal(env.sends().length, 0);
});

test('a user turn after the exact RESUME marker does not deadlock destination attachment', async (t) => {
  const env = await destinationWorker(t);
  env.shared.route = receiver({ markerFound: true, superseded: true, generating: false,
    conversationId: 'destination-owned', conversationUrl: 'https://chatgpt.com/c/destination-owned' });
  await env.tick();
  assert.equal(env.serverJob().phase, 'completed');
  assert.equal(env.serverJob().newConversationId, 'destination-owned');
  assert.equal(env.serverJob().taskId, job().taskId);
  assert.equal(env.sends().length, 0);
});

for (const choice of [false, undefined]) {
  test('completed compact without explicit opt-in makes no continuation API request, including after reload: ' + choice, async (t) => {
    const env = await completedWorker(t, false);
    env.shared.jobs.get(job().id).continueAfterCompact = choice;
    await env.tick();
    await (await env.restart()).tick();
    assert.equal(env.record().finished, true);
    assert.equal(env.shared.requests.filter((r) => r.path.endsWith('/resume')).length, 0);
    assert.equal(env.shared.effects.filter((e) => e.type === 'work-dispatch').length, 0);
  });
}
