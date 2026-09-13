const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = (name) => fs.readFileSync(path.join(__dirname, name), 'utf8');
const tick = () => new Promise(setImmediate);

function admissionHarness() {
  const code = source('background.js');
  const body = code.slice(code.indexOf('chrome.runtime.onMessage.addListener('), code.indexOf('chrome.tabs.onRemoved'));
  let listener;
  const pending = new Map(), failures = [], replies = [];
  const c = vm.createContext({
    chrome: { runtime: { onMessage: { addListener: (fn) => { listener = fn; } } } },
    localOrigin: (url) => { assert.equal(url, 'http://localhost:8080'); return url; },
    configureApprovalBridge: async () => {},
    startSubagentRequest: (message) => new Promise((resolve, reject) => pending.set(message.subagentId, { resolve, reject })),
    reportSubagentFailure: async (...args) => failures.push(args),
    errorMessage: (error) => error.message,
  });
  vm.runInContext(body, c);
  return { pending, failures, replies, send(id) {
    return listener({ type: 'chatcmd-local-command', action: 'subagent-send', subagentId: id,
      childTaskId: `task-${id}`, submittedContent: 'Read one file', attempt: 1,
      localBaseUrl: 'http://localhost:8080' }, { tab: { id: 1 } }, (reply) => replies.push({ id, ...reply }));
  } };
}

test('two child admissions are acknowledged before either slow tab startup finishes', async () => {
  const h = admissionHarness();
  h.send('first'); h.send('second');
  await tick();
  assert.equal(h.replies.length, 2, 'ACK must not wait for tab load/composer readiness beyond the UI timeout');
  assert.ok(h.replies.every((r) => r.ok && r.accepted));
  assert.equal(h.failures.length, 0);
  h.pending.get('first').resolve(); h.pending.get('second').resolve();
  await tick();
  assert.equal(h.replies.length, 2, 'one ACK per request, not a second success callback');
});

test('startup failure after admission is reported through the result channel only once', async () => {
  const h = admissionHarness();
  h.send('first');
  await tick();
  assert.equal(h.replies[0]?.accepted, true);
  h.pending.get('first').reject(new Error('composer unavailable'));
  await tick();
  assert.equal(h.failures.length, 1);
  assert.equal(h.replies.length, 1, 'no competing error reply makes the UI retry the same failure');
  assert.equal(h.failures[0][0], 'first');
  assert.equal(h.failures[0][1], 1);
});

function failureHarness(reply, binding = { attempt: 1, tabId: 10, requestId: 'subagent:child:1' }) {
  const code = source('background.js');
  const body = code.slice(code.indexOf('async function reportSubagentFailure('), code.indexOf('async function stopRequest('));
  const removed = [], posted = [], closed = [], logs = [];
  const c = vm.createContext({ SUBAGENT_PREFIX: 'child:',
    chrome: { storage: { session: { get: async () => ({ 'child:child': binding }), remove: async (key) => removed.push(key) } },
      tabs: { remove: async (id) => removed.push(id) } },
    releaseRequest: async (id) => removed.push(id), safeTab: async (id) => ({ id }),
    postJson: async (...args) => { posted.push(args); return reply; },
    closeSubagentRequest: async (...args) => closed.push(args),
    logExtension: async (...args) => logs.push(args), errorMessage: (e) => e.message,
  });
  const helperPath = path.join(__dirname, 'background-subagent-failure.js');
  if (fs.existsSync(helperPath)) vm.runInContext(source('background-subagent-failure.js'), c);
  vm.runInContext(body, c);
  return { c, removed, posted, closed, logs };
}

test('late startup failure cannot delete or fail the newer attempt binding', async () => {
  const h = failureHarness({}, { attempt: 2, tabId: 20, requestId: 'subagent:child:2' });
  await h.c.reportSubagentFailure('child', 1, 'http://localhost:8080', new Error('old timeout'));
  assert.equal(h.posted.length, 0);
  assert.equal(h.removed.length, 0);
  assert.equal(h.closed.length, 0);
});

test('rejected startup failure does not close an already-claimed running child', async () => {
  const h = failureHarness({ accepted: false, status: 'running', reason: 'already_claimed_or_finished' });
  await h.c.reportSubagentFailure('child', 1, 'http://localhost:8080', new Error('late timeout'));
  assert.equal(h.posted.length, 1);
  assert.equal(h.removed.length, 0);
  assert.equal(h.closed.length, 0);
});

test('failed error-report transport retains binding instead of destroying the live child', async () => {
  const h = failureHarness({});
  h.c.postJson = async () => { throw new Error('local API unavailable'); };
  await h.c.reportSubagentFailure('child', 1, 'http://localhost:8080', new Error('startup error'));
  assert.equal(h.removed.length, 0);
  assert.equal(h.closed.length, 0);
  assert.equal(h.logs.length, 1);
});

test('two failure callbacks for one attempt share one server report', async () => {
  const h = failureHarness({ accepted: true });
  await Promise.all([1, 2].map(() => h.c.reportSubagentFailure('child', 1, 'http://localhost:8080', new Error('same failure'))));
  assert.equal(h.posted.length, 1);
  assert.equal(h.closed.length, 1);
});

test('stop, claim or newer attempt while a tab loads prevents stale prompt submission', async () => {
  const code = source('background.js');
  const body = code.slice(code.indexOf('async function startSubagentRequestOnce'), code.indexOf('const subagentClosures'));
  for (const current of [{ active: false, status: 'stopped', attempt: 1 },
    { active: true, status: 'running', attempt: 1 }, { active: true, status: 'pending', attempt: 2 }]) {
    let checks = 0, sent = 0;
    const c = vm.createContext({ SUBAGENT_PREFIX: 'child:', requestKey: (id) => `request:${id}`,
      chrome: { storage: { session: { get: async () => ({}), set: async () => {} } }, tabs: { create: async () => ({ id: 1 }) } },
      postJson: async () => ++checks === 1 ? { active: true, status: 'pending', attempt: 1 } : current,
      normalizeNewConversationUrl: () => 'https://chatgpt.com/', waitForTab: async () => {},
      waitForChatGptReady: async () => {}, sendToChatGpt: async () => { sent++; },
    });
    vm.runInContext(body, c);
    await c.startSubagentRequestOnce({ subagentId: 'child', childTaskId: 'task-child',
      submittedContent: 'Read one file', attempt: 1, localBaseUrl: 'http://localhost:8080' });
    assert.equal(checks, 2);
    assert.equal(sent, 0);
  }
});

test('accepted startup failure closes only its own attempt after server acknowledgement', async () => {
  const h = failureHarness({ accepted: true, retryScheduled: true, attempt: 2 });
  await h.c.reportSubagentFailure('child', 1, 'http://localhost:8080', new Error('startup error'));
  assert.equal(h.posted.length, 1);
  assert.deepEqual(h.closed, [['child', 1]]);
  assert.equal(h.removed.length, 0, 'cleanup uses the attempt-fenced close path');
});
