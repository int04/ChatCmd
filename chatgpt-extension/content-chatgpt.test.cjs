const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const extensionRoot = __dirname;
const source = readFileSync(join(extensionRoot, 'content-chatgpt.js'), 'utf8');
const domSource = readFileSync(join(extensionRoot, 'content-chatgpt-dom.js'), 'utf8');
const backgroundSource = readFileSync(join(extensionRoot, 'background.js'), 'utf8');
const backgroundIoSource = readFileSync(join(extensionRoot, 'background-io.js'), 'utf8');
const uiHelperSource = readFileSync(join(extensionRoot, 'content-chatgpt-ui.js'), 'utf8');
const manifest = JSON.parse(readFileSync(join(extensionRoot, 'manifest.json'), 'utf8'));
const localUiSource = readFileSync(join(extensionRoot, '..', 'web', 'src', 'chatgpt', 'ChatGptConversation.tsx'), 'utf8');

function loadBridge(statusHandler = () => Promise.resolve({ ok: true, known: true, running: true, active: true })) {
  const context = {
    console, queueMicrotask, setTimeout, clearTimeout,
    __assistantNodes: [],
    __completionResponse: { ok: true, browserCompleted: true, hasFinalResponse: true },
    __completionPings: 0,
    __sendButton: null,
    __stopButton: null,
    chrome: { runtime: {
      onMessage: { addListener() {} },
      sendMessage(message, callback) {
        if (typeof callback === 'function') { callback({ ok: false }); return undefined; }
        if (message?.type === 'chatcmd-chatgpt-request-status') return statusHandler(message);
        if (message?.stage === 'browser-completed') {
          context.__completionPings += 1;
          return Promise.resolve(context.__completionResponse);
        }
        return Promise.resolve({ ok: true });
      },
    } },
    document: { visibilityState: 'visible', addEventListener() {}, querySelectorAll() { return []; } },
    window: {
      location: { pathname: '/c/test-conversation', href: 'https://chatgpt.com/c/test-conversation' },
      addEventListener() {},
    },
  };
  context.ChatCmdConversationDom = {
    assistantNodes: () => context.__assistantNodes,
    clickStopButton() {},
    findSendButton: () => context.__sendButton,
    findStopButton: () => typeof context.__stopButton === 'function' ? context.__stopButton() : context.__stopButton,
    findThreadError: () => context.__threadError || null,
    findVisible: () => null,
    isVisible: () => true,
    latestMessageText: () => context.__assistantNodes.at(-1)?.innerText || '',
    normalize: (value) => String(value || '').trim().toLowerCase(),
  };
  vm.createContext(context);
  vm.runInContext(source, context, { filename: 'content-chatgpt.js' });
  return context;
}

function prepareMonitor(context, state, { text = 'Phản hồi hoàn tất', sendReady = true, threadError = false } = {}) {
  context.__requestState = state;
  context.__assistantNodes = text ? [{ innerText: text, textContent: text }] : [];
  context.__sendButton = sendReady ? {} : null;
  context.__threadError = threadError ? {} : null;
  vm.runInContext(`
    globalThis.__now = 0;
    globalThis.__submitCalls = 0;
    globalThis.__composerWrites = [];
    Date.now = () => globalThis.__now;
    activeRequest = { id: 'request-1', stopRequested: false, retryCount: 0, resultReported: false };
    findComposer = () => ({});
    requestState = async () => globalThis.__requestState;
    setComposerText = (_composer, value) => { globalThis.__composerWrites.push(value); };
    submitPrompt = async () => { globalThis.__submitCalls += 1; };
    delay = async (ms) => { globalThis.__now += ms; };
  `, context);
}

test('automatic retry resubmits the original prompt when no progress was observed', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: '' });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL PROMPT')", context), /2 lần tự động gửi lại/i);
  assert.equal(context.__submitCalls, 2);
  assert.deepEqual(Array.from(context.__composerWrites), ['ORIGINAL PROMPT', 'ORIGINAL PROMPT']);
});

test('an interruption after execution progress asks ChatGPT to continue without redoing work', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: '', threadError: true });
  context.__stopButton = () => context.__now < 3_000 ? {} : null;
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL PROMPT')", context), /2 lần tự động gửi lại/i);
  assert.equal(context.__submitCalls, 2);
  assert.equal(context.__composerWrites.length, 2);
  assert.match(context.__composerWrites[0], /Tôi vừa bị gián đoạn kết nối/);
  assert.doesNotMatch(context.__composerWrites[0], /ORIGINAL PROMPT/);
});

test('partial assistant text followed by an error also uses the continuation prompt', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: 'Đã sửa một phần', threadError: true });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL PROMPT')", context), /2 lần tự động gửi lại/i);
  assert.match(context.__composerWrites[0], /không làm lại những phần đã xong/);
});

test('a ChatGPT error retries while the request is active and send is ready', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: '', threadError: true });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'RETRY ME')", context), /2 lần tự động gửi lại/i);
  assert.equal(context.__submitCalls, 2);
});

test('unknown backend state never authorizes an automatic resend', async () => {
  const context = loadBridge(() => Promise.resolve({ ok: false, error: 'offline' }));
  prepareMonitor(context, { known: false, running: null, stopRequested: false, hasFinalResponse: false, active: null }, { sendReady: false });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'DO NOT RETRY')", context), /Quá lâu/);
  assert.equal(context.__submitCalls, 0);
});

test('raw assistant bubble pings backend finalization and locks retry', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true });
  assert.equal(await vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL')", context), 'Phản hồi hoàn tất');
  assert.equal(context.__completionPings, 1);
  assert.equal(context.__submitCalls, 0);
  assert.equal(vm.runInContext('activeRequest.resultReported', context), true);
});

test('a failed completion ping never turns an existing raw bubble into a resend', async () => {
  const context = loadBridge();
  context.__completionResponse = { ok: false, error: 'backend unavailable' };
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'NEVER DUPLICATE')", context), /Quá lâu/);
  assert.ok(context.__completionPings > 1);
  assert.equal(context.__submitCalls, 0);
});

test('backend final response completes without a browser ping or retry', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: false, stopRequested: false, hasFinalResponse: true, active: false });
  assert.equal(await vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL')", context), 'Phản hồi hoàn tất');
  assert.equal(context.__completionPings, 0);
  assert.equal(context.__submitCalls, 0);
});

test('background exposes browser completion and the known status contract', async () => {
  assert.match(backgroundIoSource, /stage === 'browser-completed'/);
  assert.match(backgroundIoSource, /\/browser-completed/);
  const context = { chrome: {} };
  vm.createContext(context);
  vm.runInContext(backgroundIoSource, context, { filename: 'background-io.js' });
  const missing = await vm.runInContext("bridgeRequestState('', 7)", context);
  assert.equal(missing.known, false);
});

test('Vietnamese stop controls are detected by the shared DOM helper', () => {
  class FakeElement {
    constructor(label) { this.label = label; this.textContent = ''; }
    getAttribute(name) { return name === 'aria-label' ? this.label : null; }
    getBoundingClientRect() { return { width: 10, height: 10 }; }
  }
  for (const label of ['Dừng tạo phản hồi', 'Ngừng tạo']) {
    const button = new FakeElement(label);
    const context = {
      Element: FakeElement,
      Node: { DOCUMENT_POSITION_FOLLOWING: 4 },
      getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
      document: { querySelectorAll: (selector) => selector === 'button' ? [button] : [] },
    };
    vm.createContext(context);
    vm.runInContext(domSource, context, { filename: 'content-chatgpt-dom.js' });
    context.__button = button;
    assert.equal(vm.runInContext('ChatCmdConversationDom.findStopButton() === globalThis.__button', context), true);
  }
});

test('content scripts load helpers before the request runner', () => {
  const entry = manifest.content_scripts.find((item) => item.matches.includes('https://chatgpt.com/*'));
  assert.deepEqual(entry.js, ['content-chatgpt-ui.js', 'content-chatgpt-dom.js', 'content-chatgpt.js']);
});

test('all extension sources stay within the 500-line maintenance limit', () => {
  const lineCount = (value) => value.trimEnd().split(/\r?\n/).length;
  for (const [name, value] of Object.entries({
    'background.js': backgroundSource, 'background-io.js': backgroundIoSource,
    'content-chatgpt.js': source, 'content-chatgpt-dom.js': domSource,
    'content-chatgpt-ui.js': uiHelperSource,
  })) assert.ok(lineCount(value) <= 500, `${name} has ${lineCount(value)} lines`);
});

test('local UI keeps failed dispatches for explicit user control', () => {
  assert.doesNotMatch(localUiSource, /RETRY_DELAY_SECONDS|retryTimer/);
  assert.match(localUiSource, /catch \(reason\) \{ setError\(errorText\(reason\)\); \}/);
});
