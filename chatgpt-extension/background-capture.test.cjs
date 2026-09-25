const assert = require('node:assert/strict');
const test = require('node:test');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');

const marker = '11111111-1111-4111-8111-111111111111';
const content = `Hello [[CHATCMD-REQUEST:${marker}]]`;
const url = 'https://chatgpt.com/c/conversation-1';
function fixture(existing) {
  const calls = { get: [], post: [], stored: [], bound: [] };
  const context = vm.createContext({
    chrome: { runtime: { onMessage: { addListener() {} } } },
    safeTab: async () => ({ id: 7, url }),
    conversationIdFromUrl: (value) => value?.split('/c/')[1] || '',
    isProvisionalConversationId: (value) => String(value).startsWith('WEB:'),
    conversationBindings: async () => ({}), conversationKey: (id) => id,
    localOrigin: (value) => value, approvalBaseUrl: 'http://127.0.0.1:8765',
    getJson: async (_base, path) => { calls.get.push(path); return path.includes('/requests/') ? existing : { provider: 'chatcmd', captureProtocol: 2 }; },
    postJson: async (_base, path, body) => { calls.post.push({ path, body }); return { id: 'native-1', conversationId: 'conversation-1' }; },
    requestKey: (id) => id, bindConversationTab: async (...args) => calls.bound.push(args),
    ChatCmdCompactProtocol: { parse: () => null },
  });
  context.chrome.storage = { session: { set: async (value) => calls.stored.push(value) } };
  vm.runInContext(readFileSync(join(__dirname, 'background-capture.js'), 'utf8'), context);
  const send = (text = content) => context.handleNativeTurn({ conversationId: 'conversation-1', conversationUrl: url,
    userMessageId: 'user-1', content: text }, { tab: { id: 7 }, frameId: 0 });
  return { send, calls };
}

test('resumes the original ChatCMD request instead of creating a duplicate native request', async () => {
  const existing = { id: marker, submittedContent: content, status: 'running' };
  const { send, calls } = fixture(existing);
  const response = await send();
  assert.equal(response.request, existing);
  assert.equal(response.chatCmdTurn, true);
  assert.equal(calls.post.length, 0);
  assert.equal(calls.stored[0][marker].tabId, 7);
  assert.equal(calls.bound[0][0], 'conversation-1');
});

test('an unrelated request marker still enrolls a native turn', async () => {
  const { send, calls } = fixture({ id: marker, submittedContent: 'Different prompt', status: 'running' });
  assert.equal((await send()).request.id, 'native-1');
  assert.equal(calls.post.length, 1);
});

test('a marker from another conversation cannot claim the current tab', async () => {
  const { send, calls } = fixture({ id: marker, submittedContent: content,
    conversationId: 'different-conversation', status: 'running' });
  assert.equal((await send()).request.id, 'native-1');
  assert.equal(calls.post.length, 1);
});

test('stopped ChatCMD requests do not create another request for the same turn', async () => {
  const { send, calls } = fixture({ id: marker, submittedContent: content, status: 'stopped' });
  assert.equal((await send()).ignored, true);
  assert.equal(calls.post.length, 0);
});
