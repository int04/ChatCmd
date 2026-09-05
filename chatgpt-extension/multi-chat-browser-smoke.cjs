// Real Chrome/extension, disposable profile. The page and loopback API are fixtures.
// Exercise overlapping conversations, not successive single-tab smoke tests.
const assert = require('node:assert/strict');
const path = require('node:path');
const { readFileSync } = require('node:fs');
const { launch, until, sleep } = require('./test-support/chrome-pipe.cjs');
const { html, fixtureApi } = require('./test-support/render-fixture.cjs');

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
async function run() {
  const faults = new Map();
  const sends = [];
  const inFlight = new Map();
  const peaks = new Map();
  const api = await fixtureApi({ async beforeObservation(snapshot) {
    const { id } = snapshot;
    const count = (inFlight.get(id) || 0) + 1;
    inFlight.set(id, count); peaks.set(id, Math.max(peaks.get(id) || 0, count));
    sends.push({ ...snapshot, at: Date.now() });
    try {
      const fault = faults.get(id);
      if (fault && !fault.used && snapshot.messages.length) {
        fault.used = true; fault.entered.resolve();
        await fault.release.promise;
        return 503; // Force retry after a delayed failure, only for this conversation.
      }
    } finally { inFlight.set(id, (inFlight.get(id) || 1) - 1); }
  } });
  let chrome;
  const failures = [];
  const results = [];
  try {
    chrome = await launch();
    const extension = await chrome.call('Extensions.loadUnpacked', { path: path.resolve(__dirname) });
    const worker = await until(async () => (await chrome.call('Target.getTargets')).targetInfos
      .find((target) => target.type === 'service_worker' && target.url.startsWith(`chrome-extension://${extension.id}/`)), 'extension worker');
    const { sessionId: workerSession } = await chrome.call('Target.attachToTarget', { targetId: worker.targetId, flatten: true });
    await chrome.evaluate(workerSession, `configureApprovalBridge(${JSON.stringify(api.base)})`);
    const { targetId: foreground } = await chrome.call('Target.createTarget', { url: 'about:blank' });
    chrome.events.on('Runtime.exceptionThrown', (value) => failures.push(value.exceptionDetails.exception?.description || value.exceptionDetails.text));
    chrome.events.on('Fetch.requestPaused', (value, session) => {
      const document = value.resourceType === 'Document';
      void chrome.call('Fetch.fulfillRequest', { requestId: value.requestId, responseCode: document ? 200 : 204,
        responseHeaders: [{ name: 'Content-Type', value: 'text/html; charset=utf-8' }],
        body: document ? Buffer.from(html).toString('base64') : '' }, session).catch((error) => failures.push(error.message));
    });
    async function page(name) {
      const { targetId } = await chrome.call('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await chrome.call('Target.attachToTarget', { targetId, flatten: true });
      await chrome.call('Runtime.enable', {}, sessionId);
      await chrome.call('Page.enable', {}, sessionId);
      await chrome.call('Fetch.enable', { patterns: [{ urlPattern: '*' }] }, sessionId);
      await chrome.call('Page.navigate', { url: `https://chatgpt.com/c/${name}` }, sessionId);
      const evaluate = (expression) => chrome.evaluate(sessionId, expression);
      await until(() => evaluate('typeof window.begin === "function" && document.documentElement.dataset.chatcmdRenderProtocol === "1"'), 'document-start helper');
      return { name, targetId, sessionId, eval: evaluate };
    }
    async function hidden(pages) {
      await chrome.call('Target.activateTarget', { targetId: foreground });
      await until(async () => (await Promise.all(pages.map((p) => p.eval('document.hidden && !document.hasFocus()')))).every(Boolean), 'all source tabs hidden');
    }
    const observed = (p) => api.observations.filter((item) => item.id === p.id);
    const hasText = (p, text) => observed(p).some((item) => item.messages.some((part) => part.content.includes(text)));
    const text = (p, suffix) => `${p.marker} ${suffix}`;
    async function queue(p, suffix, final = false) {
      await p.eval(`queueText(${JSON.stringify(text(p, suffix))},${final})`);
    }
    async function begin(p, index) {
      // Identical prompts AND identical u1/a1 message IDs across every page are intentional.
      // A per-conversation identity fence must prevent collisions nevertheless.
      if (index % 2 === 0) {
        p.id = `native-${p.name}-u1`;
        await p.eval('begin("Identical multi-chat prompt")');
      } else {
        p.id = `dispatch-${p.name}`;
        api.requests.set(p.id, { id: p.id, taskId: `task-${p.id}`, turnId: `turn-${p.id}`,
          status: 'running', submittedContent: 'Identical multi-chat prompt', hasFinalResponse: false });
        await chrome.evaluate(workerSession, `(async () => {
          const tab = (await chrome.tabs.query({url:'https://chatgpt.com/*'})).find(t => t.url.endsWith(${JSON.stringify('/' + p.name)}));
          await chrome.storage.session.set({[${JSON.stringify('chatcmd-request:' + p.id)}]:{tabId:tab.id,localBaseUrl:${JSON.stringify(api.base)}}});
          return sendToChatGpt(tab.id,{type:'chatcmd-chatgpt-run',requestId:${JSON.stringify(p.id)},submittedContent:'Identical multi-chat prompt',model:'Auto'});
        })()`);
      }
      await until(() => api.requests.has(p.id) && observed(p).length > 0, 'enrollment and empty snapshot');
    }
    function assertIsolation(pages) {
      for (const p of pages) {
        const snapshots = observed(p);
        assert.ok(snapshots.length, `snapshots for ${p.name}`);
        let revision = 0;
        for (const snapshot of snapshots) {
          assert.equal(snapshot.conversationId, p.name);
          assert.equal(snapshot.userMessageId, 'u1');
          assert.ok(snapshot.revision > revision, 'accepted revisions strictly increase');
          revision = snapshot.revision;
          for (const message of snapshot.messages) {
            assert.ok(message.content.startsWith(p.marker), `foreign transcript leaked into ${p.name}`);
            for (const other of pages) if (other !== p) assert.ok(!message.content.includes(other.marker));
          }
        }
        assert.equal(peaks.get(p.id), 1, 'one snapshot in flight per conversation');
      }
      assert.equal(new Set(pages.map((p) => api.requests.get(p.id).taskId)).size, pages.length);
    }
    async function scenario(count, closeOne = false) {
      const tag = closeOne ? 'close' : String(count);
      const pages = [];
      // Finish loading all documents before concurrently starting their requests.
      for (let i = 0; i < count; i++) {
        const p = await page(`multi-${tag}-${i}`);
        p.marker = `CHAT_${tag}_${i}:`;
        pages.push(p);
      }
      await hidden(pages);
      await Promise.all(pages.map(begin));
      await hidden(pages);
      const slow = pages[0];
      const fault = { entered: deferred(), release: deferred(), used: false };
      faults.set(slow.id, fault);
      try {
        const started = Date.now();
        await Promise.all(pages.map((p) => queue(p, 'first')));
        await until(() => fault.used, 'one tab has a pending HTTP acknowledgement');
        await until(() => pages.slice(1).every((p) => hasText(p, 'first')), 'other tabs stream before slow acknowledgement');
        const firstLatencyMs = pages.slice(1).map((p) => ({ conversation: p.name,
          ms: observed(p).find((s) => s.messages.length)?.receivedAtMs - started }));
        // Burst updates are interleaved on all tabs while the slow tab's write stays blocked.
        for (let round = 1; round <= 12; round++) {
          await Promise.all(pages.map((p) => queue(p, `burst-${round} ${'x'.repeat(round * 512)}`)));
          await sleep(75);
        }
        await until(() => pages.slice(1).every((p) => hasText(p, 'burst-12')), 'fast tabs reach newest text while another is blocked');
        assert.equal(inFlight.get(slow.id), 1);
        assert.equal(sends.filter((s) => s.id === slow.id && s.messages.length).length, 1,
          'slow tab must not queue concurrent writes');
        fault.release.resolve();
        await until(() => hasText(slow, 'burst-12'), 'delayed 503 retries/coalesces to newest text');
        let survivors = pages;
        if (closeOne) {
          const closed = pages.at(-1);
          await chrome.call('Target.closeTarget', { targetId: closed.targetId });
          survivors = pages.slice(0, -1);
          await Promise.all(survivors.map((p) => queue(p, 'still streaming after sibling closes')));
          await until(() => survivors.every((p) => hasText(p, 'still streaming')), 'remaining tabs continue independently');
          assert.notEqual(api.requests.get(closed.id).status, 'completed', 'closing another tab is not our final signal');
        }
        await Promise.all(survivors.map((p) => queue(p, 'FINAL', true)));
        await until(() => survivors.every((p) => api.requests.get(p.id).status === 'completed'), 'independent final completion');
        for (const p of survivors) {
          assert.equal(api.requests.get(p.id).assistantContent, text(p, 'FINAL'));
          assert.ok(hasText(p, 'FINAL'));
          assert.equal(await p.eval('document.hidden && !document.hasFocus()'), true);
        }
        assertIsolation(pages);
        // Completed/idle tabs should release their worker wake loop rather than cost N loops forever.
        await sleep(1500);
        const wakeCount = () => chrome.evaluate(workerSession, `typeof __multiWakeCount === 'number' ? __multiWakeCount : 0`);
        const beforeIdle = await wakeCount();
        const writesBeforeIdle = api.observations.length;
        await sleep(1500);
        const idleWakes = (await wakeCount()) - beforeIdle;
        assert.equal(api.observations.length, writesBeforeIdle, 'no idle snapshot spam');
        assert.equal(idleWakes, 0, 'completed tabs no longer wake the worker');
        results.push({ concurrentChats: count, closeOne, remainingCompleted: survivors.length,
          samePromptsAndMessageIds: true, allHidden: true, firstLatencyMs,
          snapshots: pages.map((p) => ({ conversation: p.name, count: observed(p).length })),
          peakWritesPerConversation: Math.max(...pages.map((p) => peaks.get(p.id))),
          slowTabDidNotBlockOthers: true, idleWakes, durationMs: Date.now() - started });
        for (const p of survivors) await chrome.call('Target.closeTarget', { targetId: p.targetId });
      } finally { fault.release.resolve(); faults.delete(slow.id); }
    }
    // Observe messages, without modifying dispatch or scheduling behavior.
    await chrome.evaluate(workerSession, `globalThis.__multiWakeCount = 0; chrome.runtime.onMessage.addListener(m => { if(m?.type === 'chatcmd-capture-wake') globalThis.__multiWakeCount++; return false; });`);
    await scenario(2);
    await scenario(3);
    await scenario(3, true);
    assert.deepEqual(failures, [], 'no page runtime exceptions');
    const manifest = JSON.parse(readFileSync(path.join(__dirname, 'manifest.json'), 'utf8'));
    console.log(JSON.stringify({ browser: chrome.version.product, extension: manifest.version,
      scope: 'real Chrome/extension/HTTP; fixture page and API; no signed-in ChatGPT', results }, null, 2));
  } finally {
    for (const fault of faults.values()) fault.release.resolve();
    if (chrome) await chrome.close();
    await api.close();
  }
}
run().catch((error) => { console.error(error); process.exitCode = 1; });
