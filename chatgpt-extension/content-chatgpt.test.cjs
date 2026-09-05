const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const extensionRoot = __dirname;
// Unit harness exposes runner locals; integration tests load its real IIFE through the manifest.
const source = readFileSync(join(extensionRoot, 'content-chatgpt.js'), 'utf8').replace(/^\(\(\) => \{\n/, '').replace(/\}\)\(\);\s*$/, '').replace('const waitForAssistant =', 'let waitForAssistant =');
const monitorSource = readFileSync(join(extensionRoot, 'content-chatgpt-monitor.js'), 'utf8');
const runtimeSource = readFileSync(join(extensionRoot, 'content-runtime.js'), 'utf8');
const recoverySource = readFileSync(join(extensionRoot, 'background-recovery.js'), 'utf8');
const chatCmdSource = readFileSync(join(extensionRoot, 'content-chatcmd.js'), 'utf8');
const domSource = readFileSync(join(extensionRoot, 'content-chatgpt-dom.js'), 'utf8');
const backgroundSource = readFileSync(join(extensionRoot, 'background.js'), 'utf8');
const backgroundIoSource = readFileSync(join(extensionRoot, 'background-io.js'), 'utf8');
const backgroundTabsSource = readFileSync(join(extensionRoot, 'background-tabs.js'), 'utf8');
const uiHelperSource = readFileSync(join(extensionRoot, 'content-chatgpt-ui.js'), 'utf8');
const manifest = JSON.parse(readFileSync(join(extensionRoot, 'manifest.json'), 'utf8'));
const localUiSource = readFileSync(join(extensionRoot, '..', 'web', 'src', 'chatgpt', 'ChatGptConversation.tsx'), 'utf8');

function loadBridge(statusHandler = () => Promise.resolve({ ok: true, known: true, running: true, active: true })) {
  const attributes = new Map();
  const context = {
    console, queueMicrotask, setTimeout, clearTimeout,
    crypto: { randomUUID: () => 'runtime-test' },
    __assistantNodes: [],
    __completionResponse: { ok: true, browserCompleted: true, hasFinalResponse: true },
    __completionPings: 0,
    __sendButton: null,
    __stopButton: null,
    chrome: { runtime: {
      id: 'test-extension',
      onMessage: { addListener(listener) { context.__messageListener = listener; } },
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
    document: { documentElement: { setAttribute(key, value) { attributes.set(key, value); }, getAttribute(key) { return attributes.get(key) || null; } }, visibilityState: 'visible', addEventListener() {}, querySelectorAll() { return []; } },
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
  vm.runInContext(runtimeSource, context, { filename: 'content-runtime.js' });
  vm.runInContext(monitorSource, context, { filename: 'content-chatgpt-monitor.js' });
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

test('ready status trusts the usable composer even when an old local request is stale', () => {
  const context = loadBridge();
  vm.runInContext("findComposer = () => ({}); activeRequest = { id: 'stale', startedAt: 0, stopRequested: false };", context);
  let response;
  context.__messageListener({ type: 'chatcmd-chatgpt-ready' }, null, (value) => { response = value; });
  assert.equal(response.ok, true);
  assert.equal(response.ready, true);
  assert.equal(response.composerReady, true);
  assert.equal(response.generating, false);
});

test('ready status is still blocked while ChatGPT exposes a stop button', () => {
  const context = loadBridge();
  context.__stopButton = {};
  vm.runInContext('findComposer = () => ({})', context);
  let response;
  context.__messageListener({ type: 'chatcmd-chatgpt-ready' }, null, (value) => { response = value; });
  assert.equal(response.ready, false);
  assert.equal(response.generating, true);
});

test('a stale request slot does not block a new run once the composer is idle', async () => {
  const context = loadBridge();
  vm.runInContext(`
    Date.now = () => 5_000;
    findComposer = () => ({});
    activeRequest = { id: 'stale', startedAt: 0, stopRequested: false };
    runRequest = async () => {};
  `, context);
  let response;
  context.__messageListener(
    { type: 'chatcmd-chatgpt-run', requestId: 'new-request' },
    null,
    (value) => { response = value; },
  );
  assert.equal(response.ok, true);
  await Promise.resolve();
});

test('a superseded request monitor exits instead of touching the newer request', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: '' });
  vm.runInContext("activeRequest = { id: 'request-2', startedAt: 10_000, stopRequested: false, retryCount: 0, resultReported: false }", context);
  assert.equal(await vm.runInContext("waitForAssistant(0, 'request-1', 'OLD')", context), '');
  assert.equal(context.__submitCalls, 0);
});

test('automatic retry is temporarily disabled when no progress was observed', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: '' });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL PROMPT')", context), /Quá lâu/i);
  assert.equal(context.__submitCalls, 0);
  assert.deepEqual(Array.from(context.__composerWrites), []);
});

test('an interruption after execution progress does not send a continuation prompt while retry is disabled', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: '', threadError: true });
  context.__stopButton = () => context.__now < 3_000 ? {} : null;
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL PROMPT')", context), /Quá lâu/i);
  assert.equal(context.__submitCalls, 0);
  assert.equal(context.__composerWrites.length, 0);
});

test('partial assistant text followed by an error does not send the continuation prompt while retry is disabled', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: 'Đã sửa một phần', threadError: true });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'ORIGINAL PROMPT')", context), /Quá lâu/i);
  assert.equal(context.__composerWrites.length, 0);
});

test('a ChatGPT error does not retry while automatic retry is disabled', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true }, { text: '', threadError: true });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'RETRY ME')", context), /Quá lâu/i);
  assert.equal(context.__submitCalls, 0);
});

test('unknown backend state never authorizes an automatic resend', async () => {
  const context = loadBridge(() => Promise.resolve({ ok: false, error: 'offline' }));
  prepareMonitor(context, { known: false, running: null, stopRequested: false, hasFinalResponse: false, active: null }, { text: '', sendReady: false });
  await assert.rejects(vm.runInContext("waitForAssistant(0, 'request-1', 'DO NOT RETRY')", context), /Quá lâu/);
  assert.equal(context.__submitCalls, 0);
});

test('raw assistant bubble completes even when the empty composer has no send button', async () => {
  const context = loadBridge();
  prepareMonitor(
    context,
    { known: true, running: true, stopRequested: false, hasFinalResponse: false, active: true },
    { sendReady: false },
  );
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
  assert.match(backgroundSource, /importScripts\('background-io\.js', 'background-tabs\.js', 'approval-bridge\.js', 'background-recovery\.js', 'background-capture\.js', 'background-clock\.js'\)/);
  assert.match(backgroundIoSource, /stage === 'browser-completed'/);
  assert.match(backgroundIoSource, /\/browser-completed/);
  assert.match(backgroundTabsSource, /conversationReady: ready/);
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
    querySelectorAll() { return []; }
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

test('a stop-like button outside the unified composer does not mark ChatGPT as generating', () => {
  class FakeElement {
    constructor(label = '') { this.label = label; this.textContent = ''; }
    getAttribute(name) { return name === 'aria-label' ? this.label : null; }
    getBoundingClientRect() { return { width: 10, height: 10 }; }
    querySelectorAll() { return []; }
  }
  const composer = new FakeElement();
  const outsideStop = new FakeElement('Dừng chia sẻ màn hình');
  const context = {
    Element: FakeElement,
    Node: { DOCUMENT_POSITION_FOLLOWING: 4 },
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    document: {
      querySelectorAll(selector) {
        if (selector === 'form[data-type="unified-composer"]') return [composer];
        if (selector === 'button' || selector.includes('Dừng')) return [outsideStop];
        return [];
      },
    },
  };
  vm.createContext(context);
  vm.runInContext(domSource, context, { filename: 'content-chatgpt-dom.js' });
  assert.equal(vm.runInContext('ChatCmdConversationDom.findStopButton()', context), null);
});

test('content scripts load helpers before the request runner', () => {
  const entry = manifest.content_scripts.find((item) => item.matches.includes('https://chatgpt.com/*'));
  assert.deepEqual(entry.js, ['content-runtime.js', 'content-chatgpt-clock.js', 'content-chatgpt-render.js', 'content-chatgpt-ui.js', 'content-chatgpt-dom.js', 'content-chatgpt-transcript.js', 'content-chatgpt-observer.js', 'content-chatgpt-approval-ui.js', 'content-chatgpt-monitor.js', 'content-chatgpt.js', 'content-chatgpt-resume.js', 'content-chatgpt-native.js']);
});

test('new project tabs wait for a stable ChatGPT composer before sending', () => {
  assert.match(backgroundIoSource, /async function waitForChatGptReady/);
  assert.match(backgroundTabsSource, /await waitForTab\(tab\.id\);\s*await waitForChatGptReady\(tab\.id\);\s*return tab;/);
  assert.match(backgroundSource, /await waitForTab\(tab\.id\);\s*await waitForChatGptReady\(tab\.id\);\s*const requestId = `subagent:/);
});

test('all extension sources stay within the 500-line maintenance limit', () => {
  const lineCount = (value) => value.trimEnd().split(/\r?\n/).length;
  for (const [name, value] of Object.entries({
    'background.js': backgroundSource, 'background-io.js': backgroundIoSource,
    'background-tabs.js': backgroundTabsSource,
    'content-chatgpt.js': source, 'content-chatgpt-dom.js': domSource,
    'content-chatgpt-ui.js': uiHelperSource,
  })) assert.ok(lineCount(value) <= 500, `${name} has ${lineCount(value)} lines`);
});

test('extension reload recovery replaces invalidated content-script contexts', () => {
  const localEntry = manifest.content_scripts.find((item) => item.matches.includes('http://localhost/*'));
  assert.deepEqual(localEntry.js, ['content-runtime.js', 'content-chatcmd.js']);
  assert.match(backgroundSource, /recoverContentScriptsOnStartup\(\)/);
  assert.match(recoverySource, /chrome\.scripting\.executeScript/);
  assert.match(chatCmdSource, /ChatCmdRuntime\.current\(CONTENT_CONTEXT\)/);
  assert.match(runtimeSource, /Extension context invalidated/);
});

test('identity recovery binds by request id before falling back to prompt text', () => {
  assert.match(source, /dataset\.chatcmdRequestId = message\.requestId/);
  assert.match(source, /requestId: document\.documentElement\?\.dataset\?\.chatcmdRequestId/);
  assert.match(backgroundSource, /rememberRecoveryRequest\(message\.requestId/);
  assert.match(recoverySource, /probe\.requestId === requestId/);
  assert.match(recoverySource, /chrome\.storage\.local\.set/);
});

test('local UI keeps failed dispatches for explicit user control', () => {
  assert.doesNotMatch(localUiSource, /RETRY_DELAY_SECONDS|retryTimer/);
  assert.match(localUiSource, /catch \(reason\) \{ setError\(errorText\(reason\)\); \}/);
});

test('local UI does not block sending on the polling-only conversationReady signal', () => {
  const newConversationSend = localUiSource.slice(
    localUiSource.indexOf('const sendNewConversation'),
    localUiSource.indexOf('const submit'),
  );
  assert.match(newConversationSend, /if \(!status\.ready\)/);
  assert.doesNotMatch(newConversationSend, /conversationReady/);
  assert.doesNotMatch(localUiSource, /active \|\| chatGptReady !== true/);
  assert.doesNotMatch(localUiSource, /chatgpt-retry-warning/);
});

test('runner never completes a stale observation under the next conversation identity', async () => {
  const context = loadBridge();
  prepareMonitor(context, { known: true, running: true, active: true });
  vm.runInContext(`
    const capture = { active: true, scan() {}, bind: async () => true, answer: 'Old answer' };
    globalThis.ChatCmdObserver = { create: () => capture };
    waitForComposer = async () => ({});
    selectModel = async () => {};
    waitForConversationIdentity = async () => ({ conversationId: 'old', conversationUrl: 'https://chatgpt.com/c/old' });
    waitForAssistant = async () => { capture.active = false; return capture.answer; };
    globalThis.__results = [];
    reportRequestResult = async (payload) => { globalThis.__results.push(payload); };
  `, context);
  await vm.runInContext("runRequest({ requestId: 'request-1', submittedContent: 'Hello' })", context);
  assert.equal(context.__results.length, 0);
});

test('page render bridge runs in MAIN at document_start while capture stays isolated', () => {
  const main = manifest.content_scripts.find(item => item.world === 'MAIN');
  assert.deepEqual(main.js, ['page-chatgpt-render.js']);
  assert.equal(main.run_at, 'document_start');
  assert.deepEqual(main.matches, ['https://chatgpt.com/*']);
  assert.ok(!manifest.permissions.includes('debugger'));
  assert.match(recoverySource, /injectChatGptScripts/);
  assert.match(backgroundIoSource, /await injectChatGptScripts\(tabId\)/);
});
