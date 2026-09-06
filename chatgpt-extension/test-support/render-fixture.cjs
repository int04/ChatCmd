// A tiny public UI fixture: text reaches the DOM ONLY through the page's cached rAF.
// The loopback API is a test double; Rust/SQLite contracts are covered by capture-integration.cjs.
const http = require('node:http');
const html = `<!doctype html><meta charset="utf-8"><title>Capture render fixture</title>
<main id="thread"></main><form data-type="unified-composer"><textarea id="prompt-textarea"></textarea>
<button type="button" data-testid="send-button">Send</button></form>
<script>
const frame = window.requestAnimationFrame.bind(window);
window.nativeFrames = 0;
window.begin = function(content = 'Render in background') {
  document.querySelector('#thread').innerHTML = '<section data-turn="user" data-turn-id="u1"><div data-message-author-role="user" data-message-id="u1"><p></p></div></section>';
  document.querySelector('p').textContent = content;
  const stop = document.createElement('button'); stop.type = 'button'; stop.dataset.testid = 'stop-button'; stop.textContent = 'Stop';
  document.querySelector('form').appendChild(stop);
};
window.queueText = function(text, final = false) {
  frame(() => {
    window.nativeFrames++;
    let node = document.querySelector('[data-turn="assistant"]');
    if (!node) { node = document.createElement('section'); node.dataset.turn = 'assistant'; node.dataset.testid = 'conversation-turn-answer'; node.innerHTML = '<div data-message-author-role="assistant" data-message-id="a1"><div class="markdown"></div></div>'; document.querySelector('#thread').appendChild(node); }
    node.querySelector('.markdown').textContent = text;
    if (final) document.querySelector('[data-testid="stop-button"]')?.remove();
  });
};
document.querySelector('[data-testid="send-button"]').onclick = () => begin(document.querySelector('textarea').value);
</script>`;
async function fixtureApi(options = {}) {
  const requests = new Map();
  const observations = [];
  const calls = [];
  const server = http.createServer(async (req, res) => {
    let data = '';
    for await (const chunk of req) { data += chunk; if (data.length > 1024 * 1024) { res.writeHead(413); res.end(); return; } }
    const body = data ? JSON.parse(data) : {};
    const route = req.url.split('?')[0];
    calls.push(route);
    let response = {};
    if (route.endsWith('/capture/capabilities')) response = { provider: 'chatcmd', captureProtocol: 2 };
    else if (route.endsWith('/capture/turns')) {
      const id = `native-${body.conversationId}-${body.userMessageId}`;
      if (!requests.has(id)) requests.set(id, { id, taskId: id, turnId: 'turn-' + id, status: 'running',
        conversationId: body.conversationId, conversationUrl: body.conversationUrl,
        userContent: body.content, submittedContent: body.content, hasFinalResponse: false });
      response = requests.get(id);
    } else if (/\/requests\//.test(route)) response = requests.get(decodeURIComponent(route.split('/').at(-1))) || {};
    else if (/\/bridge\//.test(route)) {
      const parts = route.split('/');
      const action = parts.at(-1), id = decodeURIComponent(parts.at(-2));
      const request = requests.get(id);
      if (!request) { res.writeHead(404); res.end('{}'); return; }
      if (action === 'observation') {
        // Test-only fault injection: a delayed/failing tab must not block another tab.
        const rejection = await options.beforeObservation?.({ id, ...body });
        if (rejection) {
          res.writeHead(rejection, { 'Content-Type': 'application/json' });
          res.end(JSON.stringify({ detail: 'Injected transient capture failure' })); return;
        }
        observations.push({ id, receivedAtMs: Date.now(), ...body });
        response = { accepted: true, revision: body.revision };
      } else if (action === 'started' || action === 'identity') {
        Object.assign(request, { conversationId: body.conversationId, conversationUrl: body.conversationUrl });
        response = request;
      } else if (action === 'browser-completed' || action === 'result') {
        Object.assign(request, { status: 'completed', assistantContent: body.assistantContent, hasFinalResponse: true });
        response = request;
      }
    } else if (route.endsWith('/pending')) response = [];
    res.writeHead(200, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(response));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { base: `http://127.0.0.1:${server.address().port}`, requests, observations, calls,
    async close() { server.closeAllConnections(); await new Promise((resolve) => server.close(resolve)); } };
}
module.exports = { html, fixtureApi };
