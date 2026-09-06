const assert = require('node:assert/strict');
const test = require('node:test');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const { JSDOM } = require('../web/node_modules/jsdom');
const source = (name) => readFileSync(join(__dirname, name), 'utf8');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(predicate) {
  const deadline = Date.now() + 3000;
  while (!predicate()) { assert.ok(Date.now() < deadline, 'background progress timed out'); await sleep(15); }
}

function fixture(t, visible = false) {
  const page = new JSDOM('<body><main></main></body>', { url: 'https://chatgpt.com/c/background', runScripts: 'outside-only' });
  const w = page.window;
  let visibility = visible ? 'visible' : 'hidden';
  Object.defineProperty(w.document, 'visibilityState', { get: () => visibility });
  // No tab timeout/interval is ever run. Only the service worker's host timers are real.
  let id = 0;
  const localTimers = new Map();
  w.setTimeout = (fn) => { localTimers.set(++id, fn); return id; };
  w.clearTimeout = (key) => localTimers.delete(key);
  w.setInterval = () => { throw new Error('capture cannot depend on an interval'); };
  const workerTimers = new Set();
  const sends = [];
  const wakes = [];
  let progress = async (message) => { sends.push(structuredClone(message)); return { ok: true, accepted: true }; };
  let listener;
  const worker = vm.createContext({
    chrome: { runtime: { onMessage: { addListener: (fn) => { listener = fn; } } }, tabs: { onRemoved: { addListener() {} } } },
    isChatGptUrl: (url) => String(url).startsWith('https://chatgpt.com/'),
    setTimeout: (fn, ms) => { const timer = setTimeout(fn, ms); workerTimers.add(timer); return timer; }, clearTimeout,
  });
  vm.runInContext(source('background-clock.js'), worker);
  w.chrome = { runtime: { id: 'test', sendMessage(message) {
    if (message.type === 'chatcmd-capture-wake') {
      wakes.push(message);
      return new Promise((resolve) => listener(message, { tab: { id: 7, url: w.location.href }, frameId: 0 }, resolve));
    }
    return progress(message);
  } } };
  for (const file of ['content-runtime.js', 'content-chatgpt-clock.js', 'content-chatgpt-transcript.js', 'content-chatgpt-observer.js']) w.eval(source(file));
  t.after(() => { w.dispatchEvent(new w.Event('pagehide')); w.ChatCmdCaptureClock.stop(); for (const timer of workerTimers) clearTimeout(timer); w.close(); });
  return { w, sends, wakes, localTimers,
    setProgress: (fn) => { progress = fn; },
    visible(value) { visibility = value ? 'visible' : 'hidden'; w.document.dispatchEvent(new w.Event('visibilitychange')); },
    add(html) { w.document.querySelector('main').insertAdjacentHTML('beforeend', html); },
  };
}
const html = '<section data-turn="user"><div data-message-id="u" data-message-author-role="user">Hello</div></section>'
  + '<section data-turn="assistant"><div class="markdown">First</div></section>';
function capture(t, env) {
  const recorder = env.w.ChatCmdObserver.create('background-request', 'Hello');
  env.add(html);
  t.after(() => recorder.stop());
  return recorder;
}

test('a hidden tab streams its trailing mutation without executing any tab timer', async (t) => {
  const env = fixture(t);
  const recorder = capture(t, env);
  await recorder.bind();
  env.w.document.querySelector('.markdown').textContent = 'First plus more';
  await until(() => env.sends.length === 2);
  assert.equal(recorder.answer, 'First plus more');
  assert.equal(env.sends[1].messages[0].content, 'First plus more');
  assert.equal(env.sends[1].messages[0].id, env.sends[0].messages[0].id);
  assert.ok(env.wakes.length > 0);
  assert.equal(env.w.document.visibilityState, 'hidden');
  assert.ok(env.sends[1].revision > env.sends[0].revision);
});

test('a failed hidden-tab snapshot retries the same revision without another DOM mutation', async (t) => {
  const env = fixture(t);
  const recorder = capture(t, env);
  env.setProgress(async (message) => {
    env.sends.push(structuredClone(message));
    return { ok: env.sends.length > 1, accepted: env.sends.length > 1 };
  });
  assert.equal(await recorder.bind(), false);
  await until(() => env.sends.length === 2);
  assert.equal(env.sends[0].revision, env.sends[1].revision);
  assert.equal(await recorder.flush(), true);
});

test('hidden in-flight updates coalesce and publish once after acknowledgement', async (t) => {
  const env = fixture(t);
  const recorder = capture(t, env);
  let ack;
  env.setProgress((message) => { env.sends.push(structuredClone(message)); return new Promise((resolve) => { ack = resolve; }); });
  const first = recorder.bind();
  env.w.document.querySelector('.markdown').textContent = 'First complete text';
  await sleep(30);
  assert.equal(env.sends.length, 1);
  env.setProgress(async (message) => { env.sends.push(structuredClone(message)); return { ok: true, accepted: true }; });
  ack({ ok: true, accepted: true });
  await first;
  await until(() => env.sends.length === 2);
  assert.equal(env.sends[1].messages[0].content, 'First complete text');
});

test('switching away reroutes a previously armed visible deadline to the worker', async (t) => {
  const env = fixture(t, true);
  let count = 0;
  env.w.ChatCmdCaptureClock.later(() => { count++; }, 70);
  assert.equal(env.wakes.length, 0);
  env.visible(false);
  await until(() => count === 1);
  for (const fn of [...env.localTimers.values()]) fn();
  await sleep(30);
  assert.equal(count, 1, 'local and worker wakes must not run a job twice');
});

test('cancelled deadlines and replaced clock owners never execute after a late worker reply', async (t) => {
  const env = fixture(t);
  let count = 0;
  const clock = env.w.ChatCmdCaptureClock;
  const id = clock.later(() => count++, 60);
  clock.cancel(id);
  clock.later(() => count++, 60);
  env.w.eval(source('content-chatgpt-clock.js'));
  await sleep(120);
  assert.equal(count, 0);
});

test('idle polling does not create a service-worker keepalive loop', async (t) => {
  const env = fixture(t);
  env.w.ChatCmdCaptureClock.later(() => {}, 750, { background: false });
  await sleep(70);
  assert.equal(env.wakes.length, 0);
});

test('pending old-conversation updates cannot be sent after navigation while hidden', async (t) => {
  const env = fixture(t);
  const recorder = capture(t, env);
  await recorder.bind();
  env.w.document.querySelector('.markdown').textContent = 'First changed';
  await sleep(20);
  env.w.history.pushState({}, '', '/c/different');
  env.w.document.querySelector('.markdown').textContent = 'Another conversation';
  await sleep(650);
  assert.equal(env.sends.length, 1);
  assert.equal(recorder.active, false);
});

test('worker clock validates sender, caps delay and bounds outstanding work', () => {
  let listener;
  let removed;
  const timers = new Map();
  const worker = vm.createContext({
    chrome: { runtime: { onMessage: { addListener(fn) { listener = fn; } } }, tabs: { onRemoved: { addListener(fn) { removed = fn; } } } },
    isChatGptUrl: (url) => url?.startsWith('https://chatgpt.com/'),
    setTimeout(fn, ms) { timers.set(fn, ms); return fn; }, clearTimeout(fn) { timers.delete(fn); },
  });
  vm.runInContext(source('background-clock.js'), worker);
  const results = [];
  const sender = { tab: { id: 7, url: 'https://chatgpt.com/c/test' }, frameId: 0 };
  const message = { type: 'chatcmd-capture-wake', delayMs: 999999 };
  for (const bad of [{}, { ...sender, frameId: 1 }, { ...sender, url: 'https://evil.example/' }]) {
    assert.equal(listener(message, bad, (r) => results.push(r)), false);
    assert.equal(results.at(-1).ok, false);
  }
  for (let i = 0; i < 4; i++) assert.equal(listener(message, sender, (r) => results.push(r)), true);
  assert.deepEqual([...timers.values()], [1000, 1000, 1000, 1000]);
  assert.equal(listener(message, sender, (r) => results.push(r)), false);
  removed(7);
  assert.equal(timers.size, 0);
});
