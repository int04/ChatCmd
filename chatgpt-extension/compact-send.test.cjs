'use strict';
const assert = require('node:assert/strict');
const test = require('node:test');
const { contentFixture } = require('./compact-test-content.cjs');
const { job, BODY } = require('./compact-test-fixtures.cjs');

for (const kind of ['HANDOFF', 'RESUME']) {
  const compactJob = () => job({ handoffText: BODY });
  const options = kind === 'RESUME' ? { url: 'https://chatgpt.com/' } : {};
  test(`${kind}: multi-paragraph contenteditable can prepare and send without manual intervention`, async (t) => {
    const env = contentFixture(t, options);
    const editor = env.w.document.createElement('div');
    editor.id = 'prompt-textarea'; editor.contentEditable = 'true';
    env.composer().replaceWith(editor);
    env.controller.setComposerText = (node, text) => {
      env.state.writes.push(text);
      // ProseMirror stores line breaks as element boundaries, not textContent newlines.
      node.replaceChildren(...text.split('\n').map((line) => {
        const p = env.w.document.createElement('p'); p.textContent = line; return p;
      }));
    };
    const value = compactJob();
    const token = env.probe(value, kind).documentToken;
    const ready = await env.message('prepare', value, kind, token);
    assert.equal(ready.ready, true, 'paragraph boundaries must not be mistaken for an unrelated draft');
    const result = await env.message('dispatch', value, kind, token);
    assert.equal(result.sent, true, result.error);
    assert.equal(env.state.clicks, 1);
  });

  test(`${kind}: preparing waits for Send to enable without rewriting the staged prompt`, async (t) => {
    const env = contentFixture(t, options);
    const value = compactJob();
    const button = env.w.document.querySelector('[data-testid="send-button"]');
    button.disabled = true;
    const token = env.probe(value, kind).documentToken;
    const first = await env.message('prepare', value, kind, token);
    assert.equal(first.ready, false, 'a typed prompt does not prove Send is ready');
    assert.equal(env.state.clicks, 0);
    button.disabled = false;
    assert.equal((await env.message('prepare', value, kind, token)).ready, true);
    assert.equal(env.state.writes.length, 1, 'do not reset React input state on each poll');
    assert.equal((await env.message('dispatch', value, kind, token)).sent, true);
    assert.equal(env.state.clicks, 1);
  });

  test(`${kind}: a disabled button at dispatch explicitly proves no click, but a real click is never repeated`, async (t) => {
    const env = contentFixture(t, options);
    const value = compactJob();
    const token = await env.ready(value, kind);
    const button = env.w.document.querySelector('[data-testid="send-button"]');
    button.disabled = true;
    const deferred = await env.message('dispatch', value, kind, token);
    assert.equal(deferred.ok, true);
    assert.equal(deferred.sent, false);
    assert.equal(deferred.retryable, true);
    assert.equal(deferred.documentToken, token);
    assert.equal(env.state.clicks, 0);
    button.disabled = false;
    assert.equal((await env.message('dispatch', value, kind, token)).sent, true);
    // Simulate delayed DOM acceptance: no marked user message is visible yet.
    await env.message('dispatch', value, kind, token);
    assert.equal(env.state.clicks, 1);
  });
}
