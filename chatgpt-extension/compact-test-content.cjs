'use strict';

const assert = require('node:assert/strict');
const { JSDOM } = require('../web/node_modules/jsdom');
const { source, event, job, BODY } = require('./compact-test-fixtures.cjs');

// The content module, DOM selectors and transcript parser are production code.
// Only controller effects (pause/model selection/composer write) and Chrome transport
// are substituted. No capture, marker matching or dispatch decision is reimplemented.
function contentFixture(t, options = {}) {
  const page = new JSDOM('<!doctype html><html><head></head><body><main></main>'
    + '<div id="input-shell"><form data-type="unified-composer">'
    + '<textarea id="prompt-textarea"></textarea>'
    + '<button type="button" data-testid="send-button">Send</button></form></div></body></html>', {
    url: options.url || job().oldConversationUrl, runScripts: 'outside-only',
  });
  const w = page.window;
  const onMessage = event();
  const state = { current: true, clicks: 0, stops: 0, pauses: 0, writes: [], models: [], wakes: [], now: 10000 };
  const controller = {
    current: () => state.current,
    findComposer: () => w.document.getElementById('prompt-textarea'),
    pauseForCompact: async () => { state.pauses++; await state.onPause?.(); },
    selectModel: async (model) => { state.models.push(model); await state.onModel?.(model); },
    setComposerText: (node, text) => { state.writes.push(text); node.value = text; },
  };
  const RealDate = w.Date;
  w.Date = class extends RealDate { static now() { return state.now; } };
  // jsdom has no layout. Supply layout dimensions, but retain real DOM selectors,
  // visibility CSS and the production functions deciding which control is usable.
  w.HTMLElement.prototype.getBoundingClientRect = function () {
    const hidden = w.getComputedStyle(this).display === 'none';
    return { width: hidden ? 0 : 400, height: hidden ? 0 : 40, top: 0, left: 0, right: 400, bottom: 40 };
  };
  w.chrome = { runtime: { onMessage } };
  w.ChatCmdController = controller;
  w.ChatCmdRuntime = { sendMessage: async (message) => { state.wakes.push(message); return { ok: true }; } };
  w.document.querySelector('[data-testid="send-button"]').addEventListener('click', () => {
    state.clicks++;
    state.onClick?.();
  });
  for (const file of ['compact-protocol.js', 'content-chatgpt-dom.js', 'content-chatgpt-transcript.js', 'content-chatgpt-compact.js']) {
    w.eval(source(file) + `\n//# sourceURL=${file}`);
  }
  t.after(() => { w.ChatCmdCompact?.dispose(); w.close(); });

  function user(text, id = 'user-owned', nested = false) {
    const outer = w.document.createElement('section');
    outer.dataset.turn = 'user';
    const node = nested ? w.document.createElement('div') : outer;
    node.dataset.messageAuthorRole = 'user';
    if (id) node.dataset.messageId = id;
    node.textContent = text;
    if (nested) outer.append(node);
    w.document.querySelector('main').append(outer);
    return outer;
  }
  function answer(text = BODY, { html, commentary = false, id = 'answer-owned' } = {}) {
    const outer = w.document.createElement('section');
    outer.dataset.turn = 'assistant';
    outer.dataset.messageId = id;
    const parent = w.document.createElement('div');
    if (commentary) parent.setAttribute('data-interrupted', 'false');
    const node = w.document.createElement('div');
    node.className = 'markdown';
    if (html !== undefined) node.innerHTML = html;
    else node.textContent = text;
    parent.append(node); outer.append(parent);
    w.document.querySelector('main').append(outer);
    return node;
  }
  function generating(value) {
    w.document.querySelector('[data-testid="stop-button"]')?.remove();
    if (!value) return;
    const button = w.document.createElement('button');
    button.type = 'button'; button.dataset.testid = 'stop-button';
    button.textContent = 'Stop';
    button.addEventListener('click', () => { state.stops++; button.remove(); });
    w.document.querySelector('form').append(button);
  }
  function message(action, compactJob, kind = 'HANDOFF', documentToken) {
    return new Promise((resolve, reject) => {
      let handled = false;
      const payload = { type: `chatcmd-compact-${action}`, job: compactJob, kind, documentToken };
      for (const listener of onMessage.listeners) {
        const pending = listener(payload, {}, (response) => { handled = true; resolve(response); });
        if (pending) handled = true;
      }
      if (!handled) reject(new Error(`No content listener handled ${action}`));
    });
  }
  function probe(compactJob = job(), kind = 'HANDOFF') { return w.ChatCmdCompact.probe(compactJob, kind); }
  function settled(compactJob = job(), kind = 'HANDOFF') {
    probe(compactJob, kind);
    state.now += 2501;
    return probe(compactJob, kind);
  }
  async function ready(compactJob = job(), kind = 'HANDOFF') {
    const { documentToken } = probe(compactJob, kind);
    const result = await message('prepare', compactJob, kind, documentToken);
    assert.equal(result.ok, true, result.error);
    assert.equal(result.ready, true);
    return documentToken;
  }
  return {
    w, page, state, controller, user, answer, generating, message, probe, settled, ready,
    protocol: w.ChatCmdCompactProtocol,
    composer: () => controller.findComposer(),
    advance: (ms) => { state.now += ms; },
    navigate: (url) => page.reconfigure({ url }),
    reload: () => w.eval(source('content-chatgpt-compact.js')),
  };
}

module.exports = { contentFixture };
