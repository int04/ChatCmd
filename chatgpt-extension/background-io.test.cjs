const assert = require('node:assert/strict');
const test = require('node:test');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');

test('a completed request restores its observation context from the bound tab', async () => {
  const requestId = '11111111-1111-4111-8111-111111111111';
  const conversationId = 'conversation-a';
  const url = `https://chatgpt.com/c/${conversationId}`;
  const stored = {};
  const posts = [];
  const context = vm.createContext({
    REQUEST_PREFIX: 'request:', CONVERSATION_PREFIX: 'conversation:',
    chrome: { tabs: { get: async () => ({ id: 7, url }) }, storage: { session: {
      get: async (key) => key === null ? stored : { [key]: stored[key] },
      set: async (values) => Object.assign(stored, values), remove: async (key) => { delete stored[key]; },
    } } },
    AbortSignal, URL,
    fetch: async (endpoint, options) => {
      if (options.method === 'GET') return { ok: true, json: async () => ({ id: requestId, conversationId, conversationUrl: url }) };
      posts.push({ path: new URL(endpoint).pathname, body: JSON.parse(options.body) });
      return { ok: true, status: 200, json: async () => ({ accepted: true }) };
    },
  });
  vm.runInContext(readFileSync(join(__dirname, 'background-io.js'), 'utf8'), context);
  stored[`conversation:${conversationId}`] = { tabId: 7, requestId, localBaseUrl: 'http://127.0.0.1:8080' };
  const response = await context.handleProgress({ stage: 'observation', requestId, conversationId,
    conversationUrl: url, userMessageId: 'user-a', revision: 1, messages: [{ id: 'answer-a', content: 'Visible answer' }] }, 7);
  assert.equal(response.accepted, true);
  assert.equal(posts[0].path, `/api/local/chatgpt/bridge/${requestId}/observation`);
  assert.equal(stored[`request:${requestId}`].tabId, 7);
});
