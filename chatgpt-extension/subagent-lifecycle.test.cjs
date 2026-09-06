const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = (name) => fs.readFileSync(path.join(__dirname, name), 'utf8');

function heartbeatHarness(count = 1, response = { active: true, status: 'running' }) {
  const calls = [], closed = [], listeners = [], stored = {}, tabs = new Map();
  for (let n = 1; n <= count; n++) {
    stored[`request:${n}`] = { mode: 'subagent', subagentId: `child-${n}`, attempt: 1, localBaseUrl: 'http://localhost:8080', tabId: n };
    tabs.set(n, { id: n, active: false, url: `https://chatgpt.com/c/conversation-${n}` });
  }
  const context = vm.createContext({ Date, Map, Set, Promise, REQUEST_PREFIX: 'request:',
    chrome: { storage: { session: { get: async () => stored } }, alarms: { create: (...args) => listeners.push(args), onAlarm: { addListener: (fn) => listeners.push(fn) } } },
    setTimeout: () => 0,
    safeTab: async (id) => tabs.get(id),
    isChatGptUrl: (url) => new URL(url).hostname === 'chatgpt.com',
    conversationIdFromUrl: (url) => url.split('/c/')[1],
    isProvisionalConversationId: (id) => id.startsWith('WEB:'),
    postJson: async (...args) => { calls.push(args); return response; },
    closeSubagentRequest: async (...args) => closed.push(args),
  });
  vm.runInContext(source('background-subagent-heartbeat.js'), context);
  return { context, calls, closed, listeners, stored, tabs };
}

test('three inactive child tabs renew independently through an MV3 alarm', async () => {
  const h = heartbeatHarness(3);
  assert.equal(h.listeners[0][1].periodInMinutes, 0.5);
  await h.context.renewBrowserSubagents();
  assert.equal(h.calls.length, 3);
  assert.equal(new Set(h.calls.map((call) => call[1])).size, 3);
  assert.equal(h.closed.length, 0);
});

test('overlapping content polls share one heartbeat and use its bounded cache', async () => {
  const h = heartbeatHarness();
  await Promise.all([1, 2, 3].map(() => h.context.subagentHeartbeatState(h.stored['request:1'])));
  await h.context.subagentHeartbeatState(h.stored['request:1']);
  assert.equal(h.calls.length, 1);
});

test('missing, discarded or navigated-away tabs cannot prolong a lease', async () => {
  const h = heartbeatHarness(3);
  h.tabs.delete(1); h.tabs.get(2).discarded = true; h.stored['request:3'].conversationId = 'different-conversation';
  await h.context.renewBrowserSubagents();
  assert.equal(h.calls.length, 0);
});

test('MCP completion does not close the tab before its final answer renders', async () => {
  const h = heartbeatHarness(1, { active: false, status: 'completed' });
  await h.context.renewBrowserSubagents();
  assert.equal(h.closed.length, 0);
});

test('terminal failure cleanup carries the attempt fence', async () => {
  const h = heartbeatHarness(1, { active: false, status: 'timedOut' });
  await h.context.renewBrowserSubagents();
  assert.deepEqual(h.closed, [['child-1', 1]]);
});

test('a delayed cleanup for attempt 1 cannot remove attempt 2', async () => {
  const code = source('background.js');
  const body = code.slice(code.indexOf('async function closeSubagentRequest('), code.indexOf('async function reportSubagentFailure('));
  const c = vm.createContext({ subagentClosures: new Map(), SUBAGENT_PREFIX: 'child:', chrome: { storage: { session: { get: async () => ({ 'child:id': { attempt: 2, tabId: 10 } }) } } } });
  vm.runInContext(body, c);
  await c.closeSubagentRequest('id', 1); // Any cleanup access beyond the fence fails the test.
});

test('the actual monitor tolerates an eleven-minute child response using the server deadline', async () => {
  let now = 1000;
  const started = now;
  const nodes = () => now - started >= 11 * 60_000 ? [{ innerText: 'Completed child response' }] : [];
  const c = vm.createContext({ Date: { now: () => now }, ChatCmdConversationDom: {
    assistantNodes: nodes, latestMessageText: () => 'Completed child response',
    findStopButton: () => nodes().length ? null : {}, findThreadError: () => null, clickStopButton: () => {},
  } });
  vm.runInContext(source('content-chatgpt-monitor.js'), c);
  const api = { activeRequest: { id: 'subagent:id:1' }, RAW_BUBBLE_STABILITY_MS: 100, AUTO_RETRY_ENABLED: false,
    unknownRequestState: () => ({ known: false }), findComposer: () => ({}),
    requestState: async () => ({ known: true, running: !nodes().length, active: !nodes().length, hasFinalResponse: !!nodes().length, deadlineAtMs: started + 30 * 60_000 }),
    isTerminalRequestState: (state) => state.known && state.hasFinalResponse,
    delay: async (ms) => { now += ms; }, reportBrowserCompletion: async () => false,
  };
  const result = await c.ChatCmdMonitor.create(api)(0, 'subagent:id:1', 'work');
  assert.equal(result, 'Completed child response');
  assert.ok(now - started > 10 * 60_000);
});

test('a rejected browser completion cannot release a still-running MCP child', async () => {
  let releases = 0;
  const c = vm.createContext({ REQUEST_PREFIX: 'request:', SUBAGENT_PREFIX: 'child:',
    chrome: { storage: { session: { set: async () => {}, remove: async () => { releases++; } } }, tabs: { remove: async () => { releases++; } } },
  });
  vm.runInContext(source('background-io.js'), c);
  Object.assign(c, {
    requestContext: async () => ({ mode: 'subagent', subagentId: 'child', attempt: 1, tabId: 1, localBaseUrl: 'http://localhost:8080' }),
    preferredConversationIdentity: async () => ({}),
    postJson: async () => ({ accepted: false, status: 'running', reason: 'already_claimed_or_finished' }),
    releaseRequest: async () => { releases++; },
  });
  const reply = await c.handleProgress({ requestId: 'r', stage: 'browser-completed', assistantContent: 'still working' }, 1);
  assert.equal(reply.browserCompleted, false);
  assert.equal(releases, 0);
});


test('new attempts serialize behind prior startup without dropping or duplicating them', async () => {
  const code = source('background.js');
  const body = code.slice(code.indexOf('const subagentStarts ='), code.indexOf('async function startSubagentRequestOnce'));
  const entered = [];
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const c = vm.createContext({ startSubagentRequestOnce: async (message) => {
    entered.push(message.attempt);
    if (message.attempt === 1) await gate;
  } });
  vm.runInContext(body, c);
  const first = c.startSubagentRequest({ subagentId:'id', attempt:1 });
  const duplicate = c.startSubagentRequest({ subagentId:'id', attempt:1 });
  const next = c.startSubagentRequest({ subagentId:'id', attempt:2 });
  assert.deepEqual(entered, [1]);
  release();
  await Promise.all([first, duplicate, next]);
  assert.deepEqual(entered, [1, 2]);
});


test('startup persists its binding before loading and cannot send before the composer is ready', async () => {
  const code = source('background.js');
  const body = code.slice(code.indexOf('async function startSubagentRequestOnce'), code.indexOf('const subagentClosures'));
  const order = [];
  const c = vm.createContext({ SUBAGENT_PREFIX:'child:', requestKey: (id) => `request:${id}`,
    chrome: { storage: { session: { get: async () => ({}), set: async () => { order.push('persist'); } } }, tabs: { create: async () => { order.push('create'); return { id:1 }; } } },
    postJson: async () => ({ active:true, status:'pending' }), normalizeNewConversationUrl: () => 'https://chatgpt.com/',
    waitForTab: async () => { order.push('loaded'); }, waitForChatGptReady: async () => { order.push('ready'); },
    sendToChatGpt: async () => { order.push('send'); },
  });
  vm.runInContext(body, c);
  await c.startSubagentRequestOnce({ subagentId:'id', childTaskId:'task', submittedContent:'work', attempt:1, localBaseUrl:'http://localhost:8080' });
  assert.deepEqual(order, ['create', 'persist', 'loaded', 'ready', 'send']);
});

test('a stale startup attempt cannot close the current attempt tab', async () => {
  const code = source('background.js');
  const body = code.slice(code.indexOf('async function startSubagentRequestOnce'), code.indexOf('const subagentClosures'));
  const c = vm.createContext({ SUBAGENT_PREFIX:'child:', chrome: { storage: { session: { get: async () => ({ 'child:id': { attempt:2, tabId:10 } }) } } },
    postJson: async () => ({ active:false, status:'pending', attempt:2, reason:'stale_attempt' }),
    closeSubagentRequest: () => { throw new Error('must not close current attempt'); },
  });
  vm.runInContext(body, c);
  await c.startSubagentRequestOnce({ subagentId:'id', childTaskId:'task', submittedContent:'work', attempt:1, localBaseUrl:'http://localhost:8080' });
});
