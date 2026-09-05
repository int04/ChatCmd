// Invoked by the Rust integration test against an isolated real HTTP router/SQLite database.
// Chrome transport and page DOM are fixtures; shipped content/background scripts run unchanged.
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');
const { webcrypto } = require('node:crypto');
const { JSDOM } = require('../web/node_modules/jsdom');
const manifest = JSON.parse(readFileSync(join(__dirname, 'manifest.json')));
const script = (file) => readFileSync(join(__dirname, file), 'utf8');
const mainEntry = manifest.content_scripts.find(entry => entry.world === 'MAIN');
const isolatedEntry = manifest.content_scripts[0];
const chatEntries = [mainEntry, isolatedEntry];
const base = process.env.CHATCMD_CAPTURE_TEST_URL;
if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(base || '')) throw new Error('An isolated test API URL is required.');
const pages = new Map();
const backgrounds = [];
const injected = [];
const calls = [];
const timers = new Set();
function storage(initial = {}) {
  const entries = { ...initial };
  return { async get(key) { return structuredClone(key === null ? entries : { [key]: entries[key] }); },
    async set(values) { Object.assign(entries, structuredClone(values)); },
    async remove(keys) { for (const key of Array.isArray(keys) ? keys : [keys]) delete entries[key]; } };
}
async function dispatch(listeners, message, sender) {
  return new Promise((resolve, reject) => {
    let pending = false;
    let responded = false;
    const respond = (value) => { responded = true; resolve(value); };
    for (const listener of listeners) if (listener(message, sender, respond) === true) pending = true;
    if (!pending && !responded) reject(new Error('Could not establish connection. Receiving end does not exist.'));
  });
}
const stubEvent = { addListener() {}, removeListener() {} };
const chrome = {
  storage: { local: storage({ 'chatcmd-approval-base-url': base }), session: storage() },
  runtime: { id: 'capture-integration', getManifest: () => manifest, onMessage: { addListener: (listener) => backgrounds.push(listener) } },
  tabs: {
    async get(id) { const page = pages.get(id); return { id, url: page.window.location.href, status: 'complete' }; },
    async query() { return [...pages.keys()].map((id) => ({ id, url: pages.get(id).window.location.href })); },
    async sendMessage(id, message) { return dispatch(pages.get(id).listeners, message, {}); },
    onUpdated: stubEvent, onRemoved: stubEvent, onReplaced: stubEvent,
  },
  scripting: { async executeScript({ target, files, world = 'ISOLATED' }) {
    injected.push({ files: [...files], world });
    const page = pages.get(target.tabId);
    for (const file of files) page.window.eval(script(file));
  } },
};
const context = vm.createContext({ chrome, console, URL, AbortSignal, TextEncoder, TextDecoder,
  crypto: webcrypto, Uint8Array, ArrayBuffer, Blob, btoa, atob,
  setTimeout: (fn, ms) => { const timer = setTimeout(fn, ms); timers.add(timer); return timer; }, clearTimeout,
  setInterval: (fn, ms) => { const timer = setInterval(fn, ms); timers.add(timer); return timer; }, clearInterval,
  WebSocket: class { static OPEN = 1; readyState = 0; close() {} },
  fetch: async (url, options = {}) => {
    const response = await fetch(url, options);
    calls.push({ path: new URL(url).pathname, body: options.body ? JSON.parse(options.body) : undefined, status: response.status });
    return response;
  },
});
context.importScripts = (...files) => { for (const file of files) vm.runInContext(script(file), context, { filename: file }); };
vm.runInContext(script('background.js'), context, { filename: 'background.js' });

function page(id, conversation, load = true) {
  const dom = new JSDOM('<body><main id="thread"></main><form data-type="unified-composer"><textarea id="prompt-textarea"></textarea><button type="button" data-testid="send-button">Send</button></form></body>', {
    url: `https://chatgpt.com/c/${conversation}`, runScripts: 'outside-only', pretendToBeVisual: true,
  });
  const listeners = [];
  const window = dom.window;
  // Emulate a long-backgrounded tab: every page timer is paused for the entire test.
  // Worker timers, runtime messages, MutationObserver and the real local API remain live.
  Object.defineProperty(window.document, 'visibilityState', { get: () => 'hidden' });
  let pausedTimerId = 0;
  window.setTimeout = () => ++pausedTimerId;
  window.setInterval = () => ++pausedTimerId;
  window.clearTimeout = () => {};
  window.clearInterval = () => {};
  window.Element.prototype.getBoundingClientRect = () => ({ width: 10, height: 10 });
  window.chrome = { runtime: { id: chrome.runtime.id, onMessage: { addListener: (fn) => listeners.push(fn) },
    sendMessage(message, callback) {
      const result = dispatch(backgrounds, message, { tab: { id, url: window.location.href }, frameId: 0 });
      if (callback) { void result.then(callback, () => callback({ ok: false })); return; }
      return result;
    } } };
  const value = { window, listeners, close: () => {
    window.ChatCmdNativeCapture?.stop();
    window.ChatCmdController?.active?.observer?.stop();
    window.ChatCmdCaptureClock?.stop();
    window.ChatCmdRenderBridge?.stop();
    window.__ChatCmdPageRender?.dispose();
    window.close();
  } };
  pages.set(id, value);
  if (load) for (const entry of chatEntries) for (const file of entry.js) window.eval(script(file));
  return value;
}
function append(window, html) { window.document.querySelector('#thread').insertAdjacentHTML('beforeend', html); }
function busy(window) { window.document.querySelector('form').insertAdjacentHTML('beforeend', '<button type="button" data-testid="stop-button">Stop</button>'); }
function question(window, id, content, roleless = false) {
  append(window, `<section data-testid="conversation-turn-${id}" data-turn="user" data-turn-id="${id}"><div ${roleless ? '' : 'data-message-author-role="user"'} data-message-id="${id}"><p>${content.replace(/\n/g, '</p><p>')}</p><button>Show more</button></div></section>`);
}
function commentary(window) {
  append(window, '<section data-testid="conversation-turn-work" data-turn="assistant"><div data-interrupted="false"><div class="markdown">Preparing the answer</div><span class="tool-message"><button aria-label="Open tool call list">Tools</button></span></div></section>');
}
function answer(window) {
  append(window, '<section data-testid="conversation-turn-final" data-turn="assistant"><div class="markdown"><p>The final answer</p><pre><code>const ok = true;</code></pre></div></section>');
  window.document.querySelector('[data-testid="stop-button"]')?.remove();
}
async function until(predicate, label, timeout = 12000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (predicate()) return; await new Promise((resolve) => setTimeout(resolve, 50)); }
  throw new Error(`${label}: timed out. Recent calls: ${JSON.stringify(calls.slice(-8))}`);
}
async function run() {
  const direct = page(1, 'native-e2e');
  busy(direct.window);
  question(direct.window, 'native-user', 'First line\nSecond line', true);
  commentary(direct.window);
  await until(() => calls.some((call) => call.path.endsWith('/observation') && call.body.messages?.some((part) => part.content.includes('Preparing')) && call.status === 200), 'native streaming before MCP');
  assert.equal(calls.filter((call) => call.path.endsWith('/capture/turns')).length, 1);
  answer(direct.window);
  await until(() => calls.some((call) => call.path.endsWith('/browser-completed') && call.status === 200), 'native browser-only final');
  const before = calls.length;
  const bridged = page(2, 'bridge-e2e', false); // Missing receiver forces the production reinjection path.
  bridged.window.document.querySelector('[data-testid="send-button"]').addEventListener('click', () => {
    busy(bridged.window);
    question(bridged.window, 'bridge-user', bridged.window.document.querySelector('textarea').value);
    commentary(bridged.window);
  });
  await chrome.storage.session.set({ 'chatcmd-request:request-b': { localBaseUrl: base, tabId: 2 } });
  context.runPayload = { type: 'chatcmd-chatgpt-run', requestId: 'request-b', submittedContent: 'First line\nSecond line', model: 'Auto' };
  await vm.runInContext('sendToChatGpt(2, runPayload)', context);
  await until(() => calls.slice(before).some((call) => call.path === '/api/local/chatgpt/bridge/request-b/observation' && call.body.messages?.some((part) => part.content.includes('Preparing')) && call.status === 200), 'ChatCMD streaming before MCP');
  answer(bridged.window);
  await until(() => calls.some((call) => call.path === '/api/local/chatgpt/bridge/request-b/browser-completed' && call.status === 200), 'ChatCMD browser-only final');
  assert.ok(injected.length > 0, 'exercised production missing-receiver injection');
  for (const item of injected) assert.deepEqual(item.files, item.world === 'MAIN' ? mainEntry.js : isolatedEntry.js);
  assert.ok(injected.some(item => item.world === 'MAIN'));
  assert.ok(injected.some(item => item.world === 'ISOLATED'));
  // Reinject the complete current manifest into the same document: no lexical redeclaration,
  // no stale request handlers, and no second native request for the completed bridged turn.
  await vm.runInContext('injectChatGptScripts(2)', context);
  const health = await chrome.tabs.sendMessage(2, { type: 'chatcmd-content-alive', kind: 'chatgpt' });
  assert.equal(health.captureProtocol, 2);
  assert.equal(health.clockProtocol, 1);
  assert.equal(health.renderProtocol, 1);
  assert.equal(direct.window.document.visibilityState, 'hidden');
  assert.equal(bridged.window.document.visibilityState, 'hidden');
  assert.equal(health.captureReady, true);
  await new Promise((resolve) => setTimeout(resolve, 500));
  assert.ok(calls.every((call) => call.status < 400), 'all real API requests accepted');
  console.log(JSON.stringify({ native: 'streamed-and-completed-without-MCP', chatcmd: 'streamed-and-completed-without-MCP', injection: 'full-manifest', reinjection: 'safe', snapshots: calls.filter((call) => call.path.endsWith('/observation')).length }));
}
run().catch((error) => { console.error(error); process.exitCode = 1; }).finally(() => {
  for (const page of pages.values()) page.close();
  for (const timer of timers) { clearInterval(timer); clearTimeout(timer); }
});
