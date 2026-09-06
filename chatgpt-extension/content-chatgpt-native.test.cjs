const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const test = require('node:test');
const { JSDOM } = require('../web/node_modules/jsdom');
const source = (name) => readFileSync(join(__dirname, name), 'utf8');
const question = (id) => `<section data-turn="user" data-turn-id="${id}"><div data-message-id="${id}"><p>First line</p><p>Second line</p><button>Show more</button></div></section>`;

test('role-less user prose excludes controls and preserves paragraph boundaries', async (t) => {
  const page = new JSDOM('<body></body>', { url: 'https://chatgpt.com/c/one', runScripts: 'outside-only' });
  t.after(() => { page.window.ChatCmdNativeCapture?.stop(); page.window.dispatchEvent(new page.window.Event('pagehide')); page.window.close(); });
  const w = page.window;
  const sent = [];
  w.ChatCmdRuntime = { sendMessage: async (payload) => { sent.push(payload); return { ok: true, accepted: true }; } };
  w.eval(source('content-chatgpt-transcript.js'));
  w.eval(source('content-chatgpt-observer.js'));
  const observer = w.ChatCmdObserver.create('r', 'First line\nSecond line');
  t.after(() => observer.stop());
  w.document.body.innerHTML = question('u') + '<section data-turn="assistant"><div data-interrupted="false"><div class="markdown"><p>Visible progress</p></div><span class="tool-message"><button aria-label="Open tool call list">Tool</button></span></div></section>';
  await observer.bind();
  assert.equal(observer.hasTurn, true);
  assert.equal(sent[0].messages[0].content, 'Visible progress');
  assert.equal(sent[0].userMessageId, 'u');
});

test('a new direct question is not swallowed by the previous request slot', async (t) => {
  const page = new JSDOM(`<body>${question('old')}</body>`, { url: 'https://chatgpt.com/c/one', runScripts: 'outside-only' });
  t.after(() => { page.window.ChatCmdNativeCapture?.stop(); page.window.dispatchEvent(new page.window.Event('pagehide')); page.window.close(); });
  const w = page.window;
  const enrolled = [];
  let active = { observer: { userMessageId: 'old', active: true } };
  w.ChatCmdRuntime = { sendMessage: async (payload) => {
    if (payload.type === 'chatcmd-chatgpt-native-turn') {
      enrolled.push(payload.userMessageId);
      return { ok: true, request: { id: payload.userMessageId, status: 'running' } };
    }
    return { ok: true };
  } };
  w.ChatCmdController = { current: () => true, get active() { return active; }, adopt: async () => {} };
  w.eval(source('content-chatgpt-transcript.js'));
  w.eval(source('content-chatgpt-native.js'));
  await w.ChatCmdNativeCapture.tick();
  w.document.body.insertAdjacentHTML('beforeend', question('next'));
  await w.ChatCmdNativeCapture.tick(); // Old recorder has not seen the DOM mutation yet.
  active = null;
  await w.ChatCmdNativeCapture.tick();
  assert.deepEqual(enrolled, ['next']);
});

test('late native enrollment never adopts a different conversation', async (t) => {
  const page = new JSDOM(`<body>${question('u')}</body>`, { url: 'https://chatgpt.com/c/one', runScripts: 'outside-only' });
  t.after(() => { page.window.ChatCmdNativeCapture?.stop(); page.window.dispatchEvent(new page.window.Event('pagehide')); page.window.close(); });
  const w = page.window;
  let resolve;
  const adopted = [];
  w.ChatCmdRuntime = { sendMessage: () => new Promise((done) => { resolve = done; }) };
  w.ChatCmdController = { current: () => true, active: null, adopt: async (value) => adopted.push(value) };
  w.eval(source('content-chatgpt-transcript.js'));
  w.eval(source('content-chatgpt-native.js'));
  const pending = w.ChatCmdNativeCapture.tick();
  w.history.pushState({}, '', '/c/two');
  resolve({ ok: true, request: { id: 'request-one' } });
  await pending;
  assert.deepEqual(adopted, []);
});
