'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { job, BODY } = require('./compact-test-fixtures.cjs');
const { workerFixture, receiver } = require('./compact-test-worker.cjs');

for (const continueAfterCompact of [false, true]) {
  test(`destination discovery restores a missing compact receiver before declaring its open tab lost: auto=${continueAfterCompact}`, async (t) => {
    const env = await workerFixture(t);
    const value = job({ phase: 'opening_new_chat', handoffText: BODY, continueAfterCompact });
    env.seed(value, { destinationOpened: true, destinationTabId: 9,
      destinationSend: 'dispatched-unresolved' });
    env.shared.tabs = [{ id: 9, url: 'https://chatgpt.com/c/received-handoff' }];
    env.shared.contentHealth = { ok: false };
    let alive = false;
    let injections = 0;
    env.shared.onInject = async (tabId) => {
      assert.equal(tabId, 9);
      injections += 1;
      alive = true;
      env.shared.contentHealth = { ok: true, kind: 'chatgpt', compactProtocol: 3,
        captureProtocol: 2, clockProtocol: 1, renderProtocol: 1, captureReady: true };
    };
    const probe = receiver({ markerFound: true, generating: false,
      conversationId: 'received-handoff', conversationUrl: env.shared.tabs[0].url });
    env.shared.route = (tabId, message) => {
      if (!alive) throw new Error('Could not establish connection. Receiving end does not exist.');
      if (message.type === 'chatcmd-compact-locate') return { ok: true, markerFound: true };
      return probe(tabId, message);
    };

    await env.tick();

    const scriptGroups = env.chrome.runtime.getManifest().content_scripts
      .filter((entry) => entry.matches.includes('https://chatgpt.com/*')).length;
    assert.equal(injections, scriptGroups, 'restore each MAIN/ISOLATED script group once, without repeated reinjection');
    assert.equal(env.serverJob().phase, 'completed');
    assert.equal(env.serverJob().taskId, value.taskId);
    assert.equal(env.serverJob().newConversationId, 'received-handoff');
    assert.equal(env.shared.creates.length, 0);
    assert.equal(env.sends().length, 0, 'a handoff already visible in the destination must never be resent');
  });
}
