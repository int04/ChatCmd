'use strict';
const assert = require('node:assert/strict');
const test = require('node:test');
const { contentFixture } = require('./compact-test-content.cjs');
const { workerFixture } = require('./compact-test-worker.cjs');
const { job, BODY } = require('./compact-test-fixtures.cjs');

for (const kind of ['HANDOFF', 'RESUME']) {
  test(`${kind}: worker waits for Send, retries proven no-click after reload, never replays a lost actual click`, async (t) => {
    const worker = await workerFixture(t);
    const value = worker.seed(job({ phase: kind === 'HANDOFF' ? 'preparing' : 'opening_new_chat', handoffText: BODY }),
      kind === 'RESUME' ? { destinationOpened: true, destinationTabId: 7 } : {});
    const url = kind === 'HANDOFF' ? value.oldConversationUrl : `https://chatgpt.com/#chatcmd-compact=${value.id}`;
    const page = contentFixture(t, { url });
    worker.shared.tabs = [{ id: 7, url }];
    const button = page.w.document.querySelector('[data-testid="send-button"]');
    let loseClickReply = false;
    worker.shared.route = async (_id, message) => {
      const result = await page.message(message.type.replace('chatcmd-compact-', ''), message.job, message.kind, message.documentToken);
      if (message.type === 'chatcmd-compact-dispatch' && result.sent && loseClickReply) {
        loseClickReply = false;
        throw new Error('Actual click happened; response lost');
      }
      return result;
    };
    const field = kind === 'HANDOFF' ? 'sourceSend' : 'destinationSend';
    button.disabled = true;
    await worker.tick();
    assert.equal(page.state.clicks, 0);
    assert.equal(worker.record()[field], 'not-attempted');
    assert.equal(worker.sends().length, 0, 'no dispatch fence before Send readiness');
    button.disabled = false;
    // React disables/replaces Send during the HTTP checkpoint after prepare.
    worker.shared.beforeCheckpoint = async () => { button.disabled = true; };
    await worker.tick();
    assert.equal(page.state.clicks, 0);
    assert.equal(worker.record()[field], 'not-sent', 'only the exact document no-click ACK permits another attempt');
    worker.shared.beforeCheckpoint = null;
    page.reload();
    button.disabled = false;
    loseClickReply = true;
    const resumed = await worker.restart();
    assert.equal(page.state.clicks, 1);
    assert.equal(worker.record()[field], 'dispatched-unresolved');
    await resumed.tick();
    page.reload();
    await (await worker.restart()).tick();
    assert.equal(page.state.clicks, 1, 'actual click with lost reply must not be repeated');
    assert.equal(page.state.writes.length, 1, 'staged prompt is not rewritten between retries');
  });

  test(`${kind}: a no-click response for a different document cannot reopen the fence`, async (t) => {
    const worker = await workerFixture(t);
    const value = worker.seed(job({ phase: kind === 'HANDOFF' ? 'preparing' : 'opening_new_chat', handoffText: BODY }),
      kind === 'RESUME' ? { destinationOpened: true, destinationTabId: 7 } : {});
    const url = kind === 'HANDOFF' ? value.oldConversationUrl : `https://chatgpt.com/#chatcmd-compact=${value.id}`;
    const page = contentFixture(t, { url });
    worker.shared.tabs = [{ id: 7, url }];
    worker.shared.route = async (_id, message) => {
      if (message.type === 'chatcmd-compact-dispatch') return { ok: true, sent: false, retryable: true, documentToken: 'other-document' };
      return page.message(message.type.replace('chatcmd-compact-', ''), message.job, message.kind, message.documentToken);
    };
    await worker.tick();
    const field = kind === 'HANDOFF' ? 'sourceSend' : 'destinationSend';
    assert.equal(worker.record()[field], 'dispatched-unresolved');
    await (await worker.restart()).tick();
    assert.equal(worker.sends().length, 1);
    assert.equal(page.state.clicks, 0);
  });
}
