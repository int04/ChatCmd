'use strict';

const vm = require('node:vm');
const { source, clone, event, job } = require('./compact-test-fixtures.cjs');
const PREFIX = 'chatcmd-compact-job:';

function world() {
  return {
    store: {}, session: {}, jobs: new Map(), resumes: new Map(), requestsById: new Map(),
    tabs: [], alarms: new Map(), nextTabId: 100,
    calls: [], requests: [], checkpoints: [], effects: [], creates: [],
    route: async () => { throw new Error('Unexpected Chrome message: supply a tab receiver'); },
  };
}

// A fresh VM represents a fresh worker process. Persisted browser storage, backend
// jobs and tabs live outside it so restart tests cannot accidentally retain globals.
// Real background-io.js performs URL validation, HTTP serialization and messaging.
async function workerFixture(t, shared = world()) {
  const timers = new Map();
  let timerId = 0;
  const local = storage(shared.store, 'local');
  const session = storage(shared.session, 'session');
  function storage(data, area) {
    return {
      async get(keys) {
        if (keys === null) return clone(data);
        if (typeof keys === 'string') return { [keys]: clone(data[keys]) };
        return Object.fromEntries(keys.map((key) => [key, clone(data[key])]));
      },
      async set(values) {
        await shared.beforeStore?.(values, area);
        Object.assign(data, clone(values));
        shared.effects.push({ type: 'persist', area, values: clone(values) });
      },
      async remove(keys) {
        for (const key of Array.isArray(keys) ? keys : [keys]) delete data[key];
        shared.effects.push({ type: 'remove', area, keys: clone(keys) });
      },
    };
  }
  const chrome = {
    storage: { local, session },
    runtime: { onMessage: event(), onStartup: event(), onInstalled: event(), getManifest: () => JSON.parse(source('manifest.json')) },
    scripting: { executeScript: async (options) => {
      shared.effects.push({ type: 'inject', options: clone(options) });
      await shared.onInject?.(options.target.tabId);
    } },
    alarms: {
      onAlarm: event(), get: async (name) => clone(shared.alarms.get(name)),
      create: async (name, info) => { shared.alarms.set(name, clone(info)); },
    },
    tabs: {
      onUpdated: event(), onReplaced: event(), onRemoved: event(),
      query: async () => clone(shared.tabs.filter((tab) => new URL(tab.url).origin === 'https://chatgpt.com')),
      get: async (id) => {
        const tab = shared.tabs.find((item) => item.id === id);
        if (!tab) throw new Error('No tab with id');
        return clone(tab);
      },
      create: async (options) => {
        const tab = { id: shared.nextTabId++, status: 'complete', ...clone(options) };
        shared.creates.push(clone(tab));
        shared.effects.push({ type: 'create', tab: clone(tab) });
        shared.tabs.push(tab);
        await shared.afterCreate?.(tab);
        return clone(tab);
      },
      remove: async (id) => {
        await shared.beforeRemove?.(id);
        if (!shared.tabs.some((tab) => tab.id === id)) throw new Error('No tab with id');
        shared.tabs = shared.tabs.filter((tab) => tab.id !== id);
        shared.effects.push({ type: 'close-tab', tabId: id });
        await shared.afterRemove?.(id);
      },
      update: async (id, patch) => {
        const tab = shared.tabs.find((item) => item.id === id);
        if (!tab) throw new Error('No tab with id');
        Object.assign(tab, clone(patch));
        shared.effects.push({ type: 'update-tab', tabId: id, patch: clone(patch) });
        return clone(tab);
      },
      sendMessage: async (tabId, message) => {
        shared.calls.push({ tabId, ...clone(message) });
        shared.effects.push({ type: 'message', tabId, message: clone(message) });
        if (message.type === 'chatcmd-content-alive') return shared.contentHealth || {
          ok: true, kind: 'chatgpt', compactProtocol: 3, captureProtocol: 2,
          clockProtocol: 1, renderProtocol: 1, captureReady: true,
        };
        return shared.route(tabId, clone(message));
      },
    },
  };
  function response(status, value) {
    return { ok: status >= 200 && status < 300, status, json: async () => clone(value) };
  }
  async function fetch(url, options) {
    const { pathname } = new URL(url);
    const body = options.body ? JSON.parse(options.body) : null;
    shared.requests.push({ url, path: pathname, method: options.method, body, headers: clone(options.headers) });
    if (shared.offline) throw new Error('Local API offline');
    if (pathname === '/api/local/chatgpt/compact/pending' && options.method === 'GET') {
      return response(200, { jobs: [...shared.jobs.values()].filter((value) => !['completed', 'cancelled'].includes(value.phase)) });
    }
    const resume = pathname.match(/^\/api\/local\/chatgpt\/compact\/([^/]+)\/resume$/);
    if (resume && options.method === 'POST') {
      return response(200, shared.resumes.get(decodeURIComponent(resume[1])) || { requestId: null });
    }
    const requestMatch = pathname.match(/^\/api\/local\/chatgpt\/requests\/([^/]+)$/);
    if (requestMatch && options.method === 'GET') {
      const request = shared.requestsById.get(decodeURIComponent(requestMatch[1]));
      return request ? response(200, request) : response(404, { detail: 'No request' });
    }
    const matched = pathname.match(/^\/api\/local\/chatgpt\/compact\/([^/]+)(\/checkpoint)?$/);
    if (!matched) throw new Error(`Unexpected API endpoint: ${pathname}`);
    const id = decodeURIComponent(matched[1]);
    if (!shared.jobs.has(id)) return response(404, { detail: 'No job' });
    if (options.method === 'GET' && !matched[2]) return response(200, shared.jobs.get(id));
    if (options.method !== 'POST' || !matched[2]) throw new Error('Unexpected API method');
    shared.checkpoints.push({ id, ...clone(body) });
    await shared.beforeCheckpoint?.(body, id);
    const previous = shared.jobs.get(id);
    if (body.expectedRevision !== previous.revision) return response(409, { detail: 'Stale compact revision' });
    const { expectedRevision, ...patch } = body;
    const next = { ...previous, ...patch, revision: expectedRevision + 1 };
    shared.jobs.set(id, clone(next));
    shared.effects.push({ type: 'checkpoint', id, job: clone(next) });
    // Fault injection occurs AFTER durable acceptance, not instead of the SUT.
    await shared.afterCheckpoint?.(body, id);
    return response(200, next);
  }
  const context = vm.createContext({
    chrome, fetch, URL, AbortSignal, console, Error,
    approvalBaseUrl: 'http://127.0.0.1:8080',
    REQUEST_PREFIX: 'chatcmd-request:', LOG_KEY: 'compact-test-log', MAX_LOGS: 20,
    setTimeout: (fn) => { timers.set(++timerId, fn); return timerId; },
    clearTimeout: (id) => timers.delete(id),
    bindConversationTab: async (...args) => shared.effects.push({ type: 'bind', args: clone(args) }),
    bindReturnSource: async (...args) => shared.effects.push({ type: 'return', args: clone(args) }),
    forgetRecoveryRequest: async (...args) => shared.effects.push({ type: 'forget', args: clone(args) }),
    // The ordinary working-turn runner is outside Compact's module boundary.
    startRequest: async (message) => {
      shared.effects.push({ type: 'work-dispatch', message: clone(message) });
      await shared.onWorkDispatch?.(message);
    },
    reportFailure: async (...args) => shared.effects.push({ type: 'work-failure', args: clone(args) }),
    injectChatGptScripts: async (tabId) => { await shared.onInject?.(tabId); },
  });
  for (const file of ['background-io.js', 'background-recovery.js', 'compact-protocol.js', 'background-compact-destination.js', 'background-compact.js']) {
    vm.runInContext(source(file), context, { filename: file });
  }
  // Read references from the unmodified VM lexical environment, never patch source.
  const api = vm.runInContext('({ compactTick, runCompactJob, compactRecord, compactCheckpoint, '
    + 'startCompactJob, recoverCompactJobs, locateCompactDestination, compactDestination, '
    + 'finishCompactBrowser, compactOwnsTab, jobs: compactJobs, flights: compactFlights })', context);
  await api.recoverCompactJobs();
  await Promise.all([...api.flights.values()]);
  t.after(() => timers.clear());

  function seed(value = job(), record = {}) {
    shared.jobs.set(value.id, clone(value));
    shared.store[PREFIX + value.id] = {
      localBaseUrl: 'http://127.0.0.1:8080', oldConversationId: value.oldConversationId,
      oldConversationUrl: value.oldConversationUrl, sourceTabId: 1,
      sourceSend: 'not-attempted', destinationSend: 'not-attempted',
      initialOpenAllowed: false, initialized: true, ...clone(record),
    };
    return clone(value);
  }
  return {
    shared, api, context, chrome, timers, seed,
    record: (id = job().id) => clone(shared.store[PREFIX + id]),
    serverJob: (id = job().id) => clone(shared.jobs.get(id)),
    tick: (id = job().id) => api.compactTick(id),
    run: (id = job().id) => api.runCompactJob(id),
    restart: () => workerFixture(t, shared),
    sends: (kind) => shared.calls.filter((call) => call.type === 'chatcmd-compact-dispatch' && (!kind || call.kind === kind)),
  };
}

function receiver(overrides = {}) {
  // Transport fixture only: tests select environmental observations. Production
// compactTick/compactDestination alone decide checkpoints, dispatch and recovery.
  return async (_tabId, message) => {
    if (message.type === 'chatcmd-compact-locate') return { ok: true, markerFound: false };
    if (message.type === 'chatcmd-compact-clear') return { ok: true };
    if (message.type === 'chatcmd-compact-probe') return {
      ok: true, documentToken: 'document-1', markerFound: false, generating: false,
      conversationId: message.kind === 'HANDOFF' ? message.job.oldConversationId : '',
      conversationUrl: message.kind === 'HANDOFF' ? message.job.oldConversationUrl : 'https://chatgpt.com/',
      composerReady: true, draft: false, ...overrides,
    };
    if (message.type === 'chatcmd-compact-prepare') return { ok: true, ready: true, documentToken: 'document-1' };
    if (message.type === 'chatcmd-compact-dispatch') return { ok: true, sent: true };
    throw new Error(`Unexpected content action: ${message.type}`);
  };
}

module.exports = { workerFixture, world, receiver, PREFIX };
