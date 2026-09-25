const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const test = require('node:test');
const { JSDOM } = require('../web/node_modules/jsdom');
const source = (name) => readFileSync(join(__dirname, name), 'utf8');
const question = (id) => `<section data-turn="user" data-turn-id="${id}"><div data-message-id="${id}"><p>First line</p><p>Second line</p><button>Show more</button></div></section>`;

test('enrolls user messages from the current ChatGPT conversation layout', async (t) => {
  const page = new JSDOM('<body></body>', { url: 'https://chatgpt.com/c/one', runScripts: 'outside-only' });
  t.after(() => { page.window.ChatCmdNativeCapture?.stop(); page.window.close(); });
  const w = page.window;
  const enrolled = [];
  w.ChatCmdRuntime = { sendMessage: async (payload) => {
    if (payload.type === 'chatcmd-chatgpt-native-turn') enrolled.push(payload);
    return { ok: true, ignored: true };
  } };
  w.ChatCmdController = { current: () => true, active: null };
  w.eval(source('content-chatgpt-transcript.js'));
  w.eval(source('content-chatgpt-native.js'));
  w.document.body.insertAdjacentHTML('beforeend', '<div data-chatgpt-search-unit-key="fallback-turn-0:0:user" data-chatgpt-search-message-ids="new-user"><div data-user-message-bubble="true"><p>Hello from UI</p></div><button>Copy</button></div>');
  await w.ChatCmdNativeCapture.tick();
  assert.equal(enrolled.at(-1).userMessageId, 'new-user');
  assert.equal(enrolled.at(-1).content, 'Hello from UI');
});

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

test('a completed ChatCMD turn still resumes its browser transcript', async (t) => {
  const page = new JSDOM(`<body>${question('u')}</body>`, { url: 'https://chatgpt.com/c/one', runScripts: 'outside-only' });
  t.after(() => { page.window.ChatCmdNativeCapture?.stop(); page.window.close(); });
  const w = page.window;
  const adopted = [];
  w.ChatCmdRuntime = { sendMessage: async () => ({ ok: true, chatCmdTurn: true,
    request: { id: 'original', status: 'completed', hasFinalResponse: true } }) };
  w.ChatCmdController = { current: () => true, active: null, adopt: async (request) => adopted.push(request.id) };
  w.eval(source('content-chatgpt-transcript.js'));
  w.eval(source('content-chatgpt-native.js'));
  await w.ChatCmdNativeCapture.tick();
  await new Promise((resolve) => w.setTimeout(resolve, 0));
  assert.deepEqual(adopted, ['original']);
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

test('assistant streaming does not rescan native user enrollment', async (t) => {
  const page = new JSDOM(`<body>${question('u')}<section data-turn="assistant"><div class="markdown">First</div></section></body>`, {
    url: 'https://chatgpt.com/c/one', runScripts: 'outside-only',
  });
  t.after(() => { page.window.ChatCmdNativeCapture?.stop(); page.window.close(); });
  const w = page.window;
  w.ChatCmdRuntime = { sendMessage: async (payload) => payload.type === 'chatcmd-chatgpt-native-turn'
    ? { ok: true, ignored: true } : { ok: true } };
  w.ChatCmdController = { current: () => true, active: null, adopt: async () => {} };
  w.eval(source('content-chatgpt-transcript.js'));
  w.eval(source('content-chatgpt-native.js'));
  await new Promise((resolve) => w.setTimeout(resolve, 0));
  const querySelectorAll = w.document.querySelectorAll.bind(w.document);
  let userScans = 0;
  w.document.querySelectorAll = (selector) => {
    if (String(selector).includes('[data-message-author-role="user"]')) userScans += 1;
    return querySelectorAll(selector);
  };
  w.document.querySelector('.markdown').textContent = 'Streaming update';
  await new Promise((resolve) => w.setTimeout(resolve, 0));
  assert.equal(userScans, 0);
});
