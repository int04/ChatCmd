const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { JSDOM } = require('../web/node_modules/jsdom');
const source = (name) => fs.readFileSync(path.join(__dirname, name), 'utf8');
const user = (id, text = 'Read files') => `<div data-message-author-role="user" data-message-id="${id}">${text}</div>`;
const answer = (id, text) => `<section data-turn="assistant"><div data-message-id="${id}"><div class="markdown">${text}</div></div></section>`;

function observer(t) {
  const page = new JSDOM('<body></body>', { url: 'https://chatgpt.com/c/child', runScripts: 'outside-only' });
  const w = page.window, sent = [];
  w.ChatCmdRuntime = { sendMessage: async (value) => { sent.push(value); return { ok: true, accepted: false }; } };
  w.eval(source('content-chatgpt-transcript.js')); w.eval(source('content-chatgpt-observer.js'));
  const capture = w.ChatCmdObserver.create('subagent:child:1', 'Read files');
  const add = (html) => w.document.body.insertAdjacentHTML('beforeend', html);
  t.after(() => { capture.stop(); w.close(); });
  return { w, sent, capture, add };
}

test('browser child observes role-less final answer without sending parent observation requests', async (t) => {
  const h = observer(t);
  h.add(user('u') + answer('a', 'Inspected two files'));
  assert.equal(await h.capture.bind(), true);
  assert.equal(h.capture.answer, 'Inspected two files');
  assert.equal(h.capture.completionEvidence.userMessageId, 'u');
  assert.ok(h.capture.completionEvidence.assistantMessageId);
  assert.equal(h.sent.length, 0);
  await h.capture.flush(true);
  h.capture.finish();
  assert.ok(h.w.sessionStorage.getItem('chatcmd-think:subagent:child:1'), 'unacknowledged result retained');
});

test('browser child excludes tool output and commentary and fences later user/route changes', async (t) => {
  const h = observer(t);
  h.add(user('u') + '<section data-turn="assistant"><div data-interrupted><div class="markdown">Working...</div></div><div data-tool-call-id="t"><div class="markdown">tool data</div></div></section>');
  await h.capture.bind();
  assert.equal(h.capture.answer, '');
  assert.equal(h.capture.completionEvidence, null);
  h.add(answer('a', 'Final'));
  h.capture.scan();
  assert.equal(h.capture.answer, 'Final');
  h.add(user('u2', 'Different request'));
  h.capture.scan();
  assert.equal(h.capture.active, false);
  assert.equal(h.capture.completionEvidence, null);
  const h2 = observer(t);
  h2.add(user('u') + answer('a', 'Final'));
  await h2.capture.bind();
  h2.w.history.pushState({}, '', '/c/another');
  h2.capture.scan();
  assert.equal(h2.capture.completionEvidence, null);
});

test('browser child clears checkpoint only after server completion acknowledgement', async (t) => {
  const h = observer(t);
  h.add(user('u') + answer('a', 'Final'));
  await h.capture.bind();
  h.capture.acknowledgeCompletion(); h.capture.finish();
  assert.equal(h.w.sessionStorage.getItem('chatcmd-think:subagent:child:1'), null);
});

test('monitor gives two independent children stable proof and waits for real completion ACK', async () => {
  for (const id of ['first', 'second']) {
    let now = 1000, pings = 0;
    const calls = [];
    const c = vm.createContext({ Date: { now: () => now }, ChatCmdConversationDom: {
      assistantNodes: () => [], latestMessageText: () => 'Final',
      findStopButton: () => now < 5000 ? {} : null, findThreadError: () => null, clickStopButton: () => {},
    } });
    vm.runInContext(source('content-chatgpt-monitor.js'), c);
    const recorder = { active: true, answer: 'Final', hasTurn: true,
      completionEvidence: { protocol: 1, userMessageId: `u-${id}`, assistantMessageId: `a-${id}` },
      scan() {}, flush: async () => true };
    const api = { activeRequest: { id: `subagent:${id}:1`, observer: recorder },
      COMPLETION_PING_INTERVAL_MS: 1000, AUTO_RETRY_ENABLED: false,
      unknownRequestState: () => ({ known: false }), findComposer: () => ({}),
      requestState: async () => ({ known: true, running: true, active: true, deadlineAtMs: 100000 }),
      isTerminalRequestState: () => false, delay: async (ms) => { now += ms; },
      reportBrowserCompletion: async (...args) => { calls.push(args); return ++pings === 3; },
    };
    assert.equal(await c.ChatCmdMonitor.create(api)(0, api.activeRequest.id, 'Read files'), 'Final');
    assert.equal(calls.length, 3);
    for (const [requestId, text, proof] of calls) {
      assert.equal(requestId, `subagent:${id}:1`); assert.equal(text, 'Final');
      assert.equal(proof.userMessageId, `u-${id}`); assert.ok(proof.stableForMs >= 12000);
      assert.equal(proof.generating, false);
    }
    assert.ok(now >= 17000, 'no completion while stop button visible or before stability window');
  }
});

function background(reply) {
  const posted = [], timers = [], closed = [];
  const c = vm.createContext({ REQUEST_PREFIX: 'request:', SUBAGENT_PREFIX: 'child:',
    chrome: { storage: { session: { set: async () => {} } } },
    setTimeout: (fn) => { timers.push(fn); },
    closeSubagentRequest: async (...args) => closed.push(args),
  });
  vm.runInContext(source('background-io.js'), c);
  Object.assign(c, {
    requestContext: async () => ({ mode: 'subagent', subagentId: 'child', attempt: 1, tabId: 4, localBaseUrl: 'http://localhost:8080' }),
    preferredConversationIdentity: async () => ({}),
    postJson: async (...args) => { posted.push(args); return reply; },
  });
  return { c, posted, timers, closed };
}

test('background keeps request/tab on missing ACK, stale attempt, or running rejection', async () => {
  for (const reply of [undefined, {}, { accepted: false, status: 'running' },
    { accepted: false, reason: 'stale_attempt', attempt: 2 }, { accepted: false, status: 'completed', attempt: 2 }]) {
    const h = background(reply);
    const result = await h.c.handleProgress({ requestId: 'subagent:child:1', stage: 'browser-completed', assistantContent: 'Final' }, 4);
    assert.equal(result.browserCompleted, false);
    assert.equal(h.timers.length, 0);
  }
});

test('background forwards proof and closes only matching attempt after positive ACK', async () => {
  const h = background({ accepted: true, completed: true, attempt: 1, completionSource: 'browserFinal' });
  const proof = { protocol: 1, stableForMs: 12000 };
  const result = await h.c.handleProgress({ requestId: 'subagent:child:1', stage: 'browser-completed', assistantContent: 'Final', completionEvidence: proof }, 4);
  assert.equal(h.posted[0][2].completionEvidence, proof);
  assert.equal(result.browserCompleted, true);
  assert.equal(h.closed.length, 0);
  h.timers[0](); await new Promise(setImmediate);
  assert.deepEqual(h.closed, [['child', 1]]);
});

test('content result does not mistake an OK transport reply for final acknowledgement', async () => {
  const code = source('content-chatgpt.js');
  const body = code.slice(code.indexOf('async function reportRequestResult('), code.indexOf('async function waitForComposer('));
  let acknowledged = 0;
  const owner = { id: 'subagent:child:1', resultReported: false,
    observer: { flush: async () => true, acknowledgeCompletion: () => { acknowledged++; } } };
  const c = vm.createContext({ activeRequest: owner, requestObservationLost: () => false, progress: async () => ({ completed: false }) });
  vm.runInContext(body, c);
  assert.equal(await c.reportRequestResult({ requestId: owner.id, status: 'completed' }), false);
  assert.equal(owner.resultReported, false);
  c.progress = async () => ({ completed: true });
  assert.equal(await c.reportRequestResult({ requestId: owner.id, status: 'completed' }), true);
  assert.equal(owner.resultReported, true); assert.equal(acknowledged, 1);
});
