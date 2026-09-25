const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const test = require('node:test');
const { JSDOM } = require('../web/node_modules/jsdom');

function setup(t, html = '') {
  const page = new JSDOM(`<body>${html}</body>`, { url: 'https://chatgpt.com/c/conversation-a', runScripts: 'outside-only' });
  const window = page.window;
  const sent = [];
  let send = async (payload) => { sent.push(JSON.parse(JSON.stringify(payload))); return { ok: true, accepted: true }; };
  window.ChatCmdRuntime = { sendMessage: (payload) => send(payload) };
  for (const file of ['content-chatgpt-transcript.js', 'content-chatgpt-observer.js']) window.eval(readFileSync(join(__dirname, file), 'utf8'));
  t.after(() => { window.dispatchEvent(new window.Event('pagehide')); window.close(); });
  return { window, sent, setSend: (handler) => { send = handler; },
    add: (html) => window.document.body.insertAdjacentHTML('beforeend', html) };
}
const user = (id, text = 'Hello') => `<section data-testid="conversation-turn-${id}"><div data-message-author-role="user" data-message-id="${id}">${text}</div></section>`;
const assistant = (id, content) => `<section data-testid="conversation-turn-${id}"><div data-message-author-role="assistant" data-message-id="${id}">${content}</div></section>`;
const newUser = (turn, id, text) => `<div data-chatgpt-search-unit-key="fallback-turn-${turn}:0:user" data-chatgpt-search-message-ids="${id}"><div data-user-message-bubble="true"><p>${text}</p></div><button>Copy user</button></div>`;
const newAssistant = (turn, id, text) => `<div data-chatgpt-search-unit-key="fallback-turn-${turn}:2:assistant" data-chatgpt-search-message-ids="${id}"><div data-chatgpt-selection-message-id="${id}"><div data-markdown-text-style="assistant-message"><p>${text}</p></div></div><button>Copy answer</button></div>`;

function recorder(t, environment) {
  const capture = environment.window.ChatCmdObserver.create('request-a', 'Hello');
  t.after(() => capture.stop());
  return capture;
}

test('records the new user turn only; initial empty snapshot creates its bubble', async (t) => {
  const env = setup(t, user('old-user') + assistant('old-assistant', '<div class="markdown">Old answer</div>'));
  const capture = recorder(t, env);
  await capture.bind();
  assert.equal(env.sent.length, 0);
  env.add(user('new-user'));
  await capture.flush();
  assert.deepEqual(env.sent.at(-1).messages, []);
  env.add(assistant('new-assistant', '<div class="markdown"><p>New answer</p></div>'));
  await capture.flush();
  assert.equal(capture.answer, 'New answer');
  assert.equal(env.sent.at(-1).messages.length, 1);
});

test('captures streamed answers from the current ChatGPT conversation layout', async (t) => {
  const env = setup(t, newUser(0, 'old-user', 'Old') + newAssistant(0, 'old-answer', 'Old answer'));
  const capture = recorder(t, env);
  env.add(newUser(1, 'new-user', 'Hello') + newAssistant(1, 'new-answer', 'First'));
  await capture.bind();
  assert.equal(capture.userMessageId, 'new-user');
  assert.equal(capture.answer, 'First');
  assert.deepEqual(env.sent.at(-1).messages.map(({ id, content }) => ({ id, content })), [{ id: 'new-answer:0', content: 'First' }]);
  env.window.document.querySelector('[data-markdown-text-style]').innerHTML = '<p>Old answer</p>';
  env.window.document.querySelectorAll('[data-markdown-text-style]')[1].innerHTML = '<p>Final <strong>answer</strong></p>';
  await capture.flush(true);
  assert.equal(capture.answer, 'Final **answer**');
  assert.equal(env.sent.at(-1).messages.length, 1);
  assert.equal(env.sent.at(-1).completed, true);
  assert.doesNotMatch(capture.answer, /Copy answer|Old answer/);
});

test('binds a ChatCMD turn by its request marker when ChatGPT reformats the prompt', async (t) => {
  const requestId = '11111111-1111-4111-8111-111111111111';
  const submitted = `Hello\n\n[[CHATCMD-REQUEST:${requestId}]]\nRouting footer`;
  const env = setup(t, newUser(0, 'old-user', 'Old'));
  const capture = env.window.ChatCmdObserver.create(requestId, submitted);
  t.after(() => capture.stop());
  env.add(newUser(1, 'new-user', `Hello [[CHATCMD-REQUEST:${requestId}]] Routing footer shown differently`)
    + newAssistant(1, 'new-answer', 'Captured answer'));
  await capture.bind();
  assert.equal(capture.userMessageId, 'new-user');
  assert.equal(capture.answer, 'Captured answer');
  assert.equal(env.sent.at(-1).messages[0].content, 'Captured answer');
});

test('streams updates under a stable identity without duplicating nested markdown', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  env.add(user('u') + assistant('a', '<div class="markdown">First<div class="markdown"> nested</div></div>'));
  await capture.bind();
  const id = env.sent.at(-1).messages[0].id;
  env.window.document.querySelector('.markdown').innerHTML = '<p>First extended answer</p>';
  await capture.flush();
  assert.equal(env.sent.at(-1).messages.length, 1);
  assert.equal(env.sent.at(-1).messages[0].id, id);
  assert.equal(env.sent.at(-1).messages[0].content, 'First extended answer');
  const count = env.sent.length;
  await capture.flush();
  assert.equal(env.sent.length, count, 'unchanged DOM does not send another snapshot');
});

test('preserves commentary after collapse and captures role-less final markdown', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  env.add(user('u') + '<section data-testid="conversation-turn-thought"><div data-interrupted="true"><div class="markdown">Visible thought summary</div></div></section>');
  await capture.bind();
  env.window.document.querySelector('[data-interrupted]').remove();
  env.add('<section data-testid="conversation-turn-final"><div class="markdown"><p>Final answer</p></div></section>');
  await capture.flush(true);
  assert.deepEqual(env.sent.at(-1).messages.map((message) => message.content), ['Visible thought summary', 'Final answer']);
  assert.equal(env.sent.at(-1).messages[0].kind, 'commentary');
  assert.equal(env.sent.at(-1).completed, true);
  assert.equal(capture.answer, 'Final answer');
});

test('excludes tools, controls, hidden nodes and extension surfaces while preserving code', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  env.add(user('u') + assistant('a', '<div class="markdown"><p>Public **text**</p><button>Copy</button><div hidden>Hidden</div><pre><code>const a = 1;\nconsole.log(a);</code></pre><a href="javascript:alert(1)">Unsafe link</a></div>')
    + '<section data-testid="conversation-turn-tool"><div data-tool-call-id="tool"><div class="markdown">Tool secret</div></div><div class="clf-stream"><div class="markdown">Own UI</div></div><div style="display:none"><div class="markdown">Invisible</div></div></section>');
  await capture.bind();
  const content = env.sent.at(-1).messages.map((message) => message.content).join('\n');
  assert.match(content, /```\nconst a = 1;/);
  assert.doesNotMatch(content, /Copy|Hidden|Tool secret|Own UI|Invisible|javascript:/);
});

test('never sends a later user turn or content from another conversation', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  env.add(user('u') + assistant('a', '<div class="markdown">Original</div>'));
  await capture.bind();
  const count = env.sent.length;
  env.add(user('u-next') + assistant('a-next', '<div class="markdown">Another answer</div>'));
  await capture.flush();
  assert.equal(env.sent.length, count);
  assert.equal(capture.active, false);
  assert.equal(capture.answer, 'Original');
});

test('navigation freezes the old snapshot even when prompts are identical', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  env.add(user('u') + assistant('a', '<div class="markdown">Original</div>'));
  await capture.bind();
  env.window.history.pushState({}, '', '/c/conversation-b');
  env.window.document.querySelector('.markdown').textContent = 'Different chat';
  await capture.flush();
  assert.equal(capture.answer, 'Original');
  assert.equal(env.sent.length, 1);
});

test('retries a failed snapshot with the same revision, then resumes from checkpoint', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  env.add(user('u') + assistant('a', '<div class="markdown">Durable public text</div>'));
  let pending;
  env.setSend(async (payload) => { pending = payload; return { ok: false }; });
  assert.equal(await capture.bind(), false);
  const failedRevision = pending.revision;
  env.setSend(async (payload) => { env.sent.push(payload); return { ok: true, accepted: true }; });
  assert.equal(await capture.flush(), true);
  assert.equal(env.sent.at(-1).revision, failedRevision);
  capture.stop();
  const resumed = env.window.ChatCmdObserver.create('request-a', 'Hello', { resumed: true });
  t.after(() => resumed.stop());
  await resumed.bind();
  assert.equal(resumed.answer, 'Durable public text');
  assert.equal(env.sent.at(-1).messages.length, 1);
});

test('one in-flight write coalesces newer text and waits for final acknowledgement', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  env.add(user('u') + assistant('a', '<div class="markdown">First</div>'));
  let resolve;
  env.setSend((payload) => { env.sent.push(JSON.parse(JSON.stringify(payload))); return new Promise((done) => { resolve = done; }); });
  const first = capture.bind();
  env.window.document.querySelector('.markdown').textContent = 'First plus final';
  const final = capture.flush(true);
  assert.equal(env.sent.length, 1);
  env.setSend(async (payload) => { env.sent.push(payload); return { ok: true, accepted: true }; });
  resolve({ ok: true, accepted: true });
  await Promise.all([first, final]);
  assert.equal(env.sent.length, 2);
  assert.equal(env.sent[0].messages[0].content, 'First');
  assert.equal(env.sent[1].messages[0].content, 'First plus final');
  assert.equal(env.sent[1].completed, true);
});

test('caps Unicode transcript size and ignores invalidated owners', async (t) => {
  const env = setup(t);
  let current = true;
  const capture = env.window.ChatCmdObserver.create('request-a', 'Hello', { current: () => current });
  t.after(() => capture.stop());
  env.add(user('u') + assistant('a', `<div class="markdown">${'🦀'.repeat(100_010)}</div>`));
  await capture.bind();
  assert.equal([...env.sent.at(-1).messages[0].content].length, 100_000);
  current = false;
  env.window.document.querySelector('.markdown').textContent = 'New owner';
  await capture.flush();
  assert.equal(env.sent.length, 1);
});

for (const provisionalId of ['WEB:test', 'local-chatgpt:test']) {
  test(`${provisionalId} may become canonical only within the owned user turn`, async (t) => {
    const env = setup(t);
    env.window.history.replaceState({}, '', `/c/${encodeURIComponent(provisionalId)}`);
    const capture = recorder(t, env);
    env.add(user('u') + assistant('a', '<div class="markdown">Before promotion</div>'));
    await capture.bind();
    assert.equal(env.sent.at(-1).conversationId, provisionalId);
    env.window.history.replaceState({}, '', '/c/canonical-chat');
    env.window.document.querySelector('.markdown').textContent = 'After promotion';
    await capture.flush();
    assert.equal(capture.active, true);
    assert.equal(env.sent.at(-1).conversationId, 'canonical-chat');
    assert.equal(env.sent.at(-1).messages.length, 1);
  });
}

test('flush can publish an already scanned revision without rescanning the DOM', async (t) => {
  const env = setup(t);
  const capture = recorder(t, env);
  const querySelectorAll = env.window.document.querySelectorAll.bind(env.window.document);
  let queries = 0;
  env.window.document.querySelectorAll = (...args) => { queries += 1; return querySelectorAll(...args); };
  capture.scan();
  assert.ok(queries > 0);
  const afterScan = queries;
  assert.equal(await capture.flush(false, false), false);
  assert.equal(queries, afterScan);
});
