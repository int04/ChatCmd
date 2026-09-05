// Real Chrome + real unpacked extension, no automation flags that disable throttling.
// Page and local HTTP API are explicit test fixtures; no signed-in profile is used.
const assert = require('node:assert/strict');
const path = require('node:path');
const { launch, until, sleep } = require('./test-support/chrome-pipe.cjs');
const { html, fixtureApi } = require('./test-support/render-fixture.cjs');
async function run() {
  const api = await fixtureApi();
  let chrome;
  const failures = [];
  const snapshots = [];
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
      await until(() => chrome.evaluate(sessionId, 'typeof window.begin === "function" && document.documentElement.dataset.chatcmdRenderProtocol === "1"'), 'document-start render helper');
      const evalPage = (expression) => chrome.evaluate(sessionId, expression);
      return { targetId, sessionId, eval: evalPage, async hide() {
        await chrome.call('Target.activateTarget', { targetId: foreground });
        await until(() => evalPage('document.hidden && !document.hasFocus()'), 'real hidden unfocused tab');
      } };
    }
    const hasText = (id, text) => api.observations.some((item) => item.id === id && item.messages.some((part) => part.content.includes(text)));
    // Negative control: keep capture clock alive but remove only the MAIN render bridge.
    const control = await page('render-control');
    await control.eval('begin(); window.__ChatCmdPageRender.dispose();');
    const controlId = 'native-render-control-u1';
    await until(() => api.requests.has(controlId), 'control enrollment');
    await control.hide();
    await control.eval('queueText("Control text waits for focus")');
    await sleep(1400);
    assert.equal(await control.eval('window.nativeFrames'), 0, 'native rAF must actually stall in this Chrome run');
    assert.equal(hasText(controlId, 'Control text'), false);
    await chrome.call('Target.activateTarget', { targetId: control.targetId });
    await until(() => hasText(controlId, 'Control text'), 'control resumes only after focusing');
    console.log('CONTROL: capture timers alive; page frame/DOM and snapshot stalled while hidden, resumed on focus.');
    await chrome.call('Target.closeTarget', { targetId: control.targetId });

    for (const mode of ['native', 'chatcmd']) {
      const name = `render-${mode}`;
      const p = await page(name);
      let id;
      if (mode === 'native') {
        await p.eval('begin()');
        id = `native-${name}-u1`;
        await until(() => api.requests.has(id), 'native enrollment');
        await p.hide();
      } else {
        id = 'dispatch-render';
        api.requests.set(id, { id, taskId: 'task-' + id, turnId: 'turn-' + id, status: 'running',
          submittedContent: 'Render in background', userContent: 'Render in background', hasFinalResponse: false });
        await p.hide();
        await chrome.evaluate(workerSession, `(async () => {
          const tab = (await chrome.tabs.query({url:'https://chatgpt.com/*'})).find(t => t.url.endsWith('/${name}'));
          await chrome.storage.session.set({['chatcmd-request:${id}']:{tabId:tab.id,localBaseUrl:${JSON.stringify(api.base)}}});
          return sendToChatGpt(tab.id,{type:'chatcmd-chatgpt-run',requestId:'${id}',submittedContent:'Render in background',model:'Auto'});
        })()`);
        await until(() => api.observations.some((item) => item.id === id), 'background dispatch initial snapshot');
      }
      const start = Date.now();
      await p.eval('queueText("First public text while hidden")');
      await until(() => hasText(id, 'First public text while hidden'), `${mode} hidden frame -> DOM -> extension -> HTTP`);
      const streamLatencyMs = Date.now() - start;
      await p.eval('queueText("First public text while hidden, now complete", true)');
      await until(() => api.requests.get(id)?.status === 'completed', `${mode} hidden completion`);
      const state = await p.eval('({hidden:document.hidden,focused:document.hasFocus(),frames:window.nativeFrames,fallback:window.__ChatCmdPageRender.fallbackFrames,text:document.querySelector(".markdown")?.textContent})');
      assert.equal(state.hidden, true); assert.equal(state.focused, false);
      assert.equal(state.frames, 2); assert.ok(state.fallback >= 2);
      assert.ok(hasText(id, 'now complete'));
      snapshots.push({ mode, streamLatencyMs, elapsedMs: Date.now() - start, ...state });
      await chrome.call('Target.closeTarget', { targetId: p.targetId });
    }
    assert.deepEqual(failures, [], 'no page/extension runtime exceptions');
    console.log(JSON.stringify({ browser: chrome.version.product, extension: '0.1.6', realHiddenTabs: true,
      page: 'cached requestAnimationFrame fixture', transport: 'real Chrome content scripts/service worker/HTTP',
      localApi: 'isolated test double', results: snapshots }, null, 2));
  } finally { if (chrome) await chrome.close(); await api.close(); }
}
run().catch((error) => { console.error(error); process.exitCode = 1; });
