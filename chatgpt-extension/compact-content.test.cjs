'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { job, BODY } = require('./compact-test-fixtures.cjs');
const { contentFixture } = require('./compact-test-content.cjs');

function generation(env, value = job(), body = BODY) {
  env.user(env.protocol.handoffPrompt(value));
  return env.answer(body + '\n' + env.protocol.marker('HANDOFF-END', value.id));
}

test('captures only the owned public generation after a complete, stable end fence', (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user('An earlier unrelated question', 'older-user');
  env.answer('Earlier answer that must not appear', { id: 'older-answer' });
  generation(env, value);
  assert.equal(env.probe(value).handoffText, null);
  env.advance(1199);
  assert.equal(env.probe(value).handoffText, null);
  env.advance(1);
  assert.equal(env.probe(value).handoffText, BODY);
  assert.equal(env.probe(value).userMessageId, 'user-owned');
});

test('streaming and changed text reset stability; missing/wrong end fence never captures', (t) => {
  const env = contentFixture(t);
  const value = job();
  const answer = generation(env, value);
  env.generating(true);
  assert.equal(env.settled(value).handoffText, null);
  env.generating(false);
  env.advance(1200);
  assert.equal(env.probe(value).handoffText, BODY);
  answer.textContent = BODY + ' Still incomplete';
  assert.equal(env.settled(value).handoffText, null);
  answer.textContent = BODY + '\n' + env.protocol.marker('HANDOFF-END', 'other-job-0001');
  assert.equal(env.settled(value).handoffText, null);
});

test('same end fence under a different operation prompt cannot become this handoff', (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user(env.protocol.handoffPrompt({ ...value, id: 'other-job-0001' }));
  env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
  const result = env.settled(value);
  assert.equal(result.markerFound, false);
  assert.equal(result.handoffText, null);
});

test('same operation marker with a different prompt is not the exact owned generation', (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user(env.protocol.marker('HANDOFF', value.id) + '\nA different user request, not the generated handoff prompt.');
  env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
  const result = env.settled(value);
  assert.equal(result.markerFound, false, 'Marker prefix alone must not confer prompt ownership');
  assert.equal(result.handoffText, null);
});

test('duplicate marked turns fail closed, while nested wrappers count as one turn', (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user(env.protocol.handoffPrompt(value), 'first', true);
  env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
  assert.equal(env.settled(value).handoffText, BODY);
  env.user(env.protocol.handoffPrompt(value), 'second');
  assert.throws(() => env.probe(value), /nhiều tin nhắn|duplicate/i);
});

test('a later user turn supersedes the owned handoff and cannot contaminate capture', (t) => {
  const env = contentFixture(t);
  const value = job();
  generation(env, value);
  env.user('Unrelated newer request', 'newer-user');
  env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
  const result = env.settled(value);
  assert.equal(result.superseded, true);
  assert.equal(result.handoffText, null);
});

test('unidentified user nodes never authorize capture or locate', async (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user(env.protocol.handoffPrompt(value), null);
  env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
  assert.equal(env.settled(value).handoffText, null);
  assert.equal((await env.message('locate', value)).markerFound, false);
});

test('real transcript parser excludes tool roots, hidden surfaces, commentary and private state', (t) => {
  const env = contentFixture(t);
  const value = job();
  Object.defineProperty(env.w, '__privateModelState', { get() { throw new Error('Private state accessed'); } });
  env.user(env.protocol.handoffPrompt(value));
  env.answer('PUBLIC_COMMENTARY_NOT_HANDOFF', { commentary: true });
  env.w.document.querySelector('main').insertAdjacentHTML('beforeend',
    '<section data-turn="assistant"><div data-testid="tool-call-expanded"><div class="markdown">TOOL_SECRET</div></div>'
    + '<div data-tool-call-id="tool-1"><div class="markdown">TOOL_RESULT</div></div>'
    + '<div hidden><div class="markdown">HIDDEN_SECRET</div></div>'
    + '<div style="display:none"><div class="markdown">CSS_HIDDEN_SECRET</div></div></section>');
  env.answer(BODY + '\n' + env.protocol.marker('HANDOFF-END', value.id));
  assert.equal(env.settled(value).handoffText, BODY);
});

test('hidden descendants inside a public answer must not leak into persisted handoff', (t) => {
  const env = contentFixture(t);
  const value = job();
  env.user(env.protocol.handoffPrompt(value));
  env.answer('', { html: BODY + '<span style="display:none">PRIVATE_HIDDEN_SENTINEL</span>'
    + '<span style="visibility:hidden">PRIVATE_INVISIBLE_SENTINEL</span>'
    + '<br>' + env.protocol.marker('HANDOFF-END', value.id) });
  const result = env.settled(value);
  assert.equal(result.handoffText, BODY);
});

test('status card stays above input, announces all four steps, and updates without duplicating', async (t) => {
  const env = contentFixture(t);
  const value = job();
  let first;
  for (const [index, phase] of [...env.protocol.phases].entries()) {
    env.probe({ ...value, phase, detail: '<script>not executable</script>' });
    const panels = env.w.document.querySelectorAll('[data-chatcmd-ui="compact"]');
    assert.equal(panels.length, 1);
    const panel = panels[0];
    if (first) assert.equal(panel, first);
    first = panel;
    assert.equal(panel.nextElementSibling, env.w.document.querySelector('form'));
    assert.equal(panel.getAttribute('role'), 'status');
    assert.equal(panel.getAttribute('aria-live'), 'polite');
    assert.equal(panel.querySelector('strong').textContent, 'ChatGPT is writing the handoff');
    assert.equal(panel.querySelectorAll('li').length, 4);
    assert.ok(panel.querySelector('[aria-current="step"]').textContent.includes(env.protocol.steps[index]));
    assert.equal(panel.querySelectorAll('[data-done="true"]').length, index);
    assert.equal(panel.querySelector('script'), null);
    assert.equal(panel.querySelector('p').textContent, '<script>not executable</script>');
    assert.equal(env.w.ChatCmdCompact.busy, true);
  }
  assert.deepEqual(env.state.renderLeases.at(-1), ['compact', true]);
  await env.message('clear');
  assert.equal(env.w.document.querySelector('[data-chatcmd-ui="compact"]'), null);
  assert.equal(env.w.ChatCmdCompact.busy, false);
  assert.deepEqual(env.state.renderLeases.at(-1), ['compact', false]);
});

test('prepare preserves an existing user draft verbatim without model change or send', async (t) => {
  const env = contentFixture(t, { url: 'https://chatgpt.com/' });
  const value = job({ phase: 'opening_new_chat', handoffText: BODY });
  const draft = '  My unsent draft\nsecond line  ';
  env.composer().value = draft;
  const token = env.probe(value, 'RESUME').documentToken;
  const result = await env.message('prepare', value, 'RESUME', token);
  assert.equal(result.ok, false);
  assert.equal(env.composer().value, draft);
  assert.deepEqual(env.state.writes, []);
  assert.deepEqual(env.state.models, []);
  assert.equal(env.state.clicks, 0);
});

test('resume preparation selects the retained model once and retains the same task in prompt', async (t) => {
  const env = contentFixture(t, { url: 'https://chatgpt.com/' });
  const value = job({ phase: 'opening_new_chat', handoffText: BODY });
  await env.ready(value, 'RESUME');
  await env.ready(value, 'RESUME');
  assert.deepEqual(env.state.models, [value.oldModel]);
  assert.equal(env.composer().value, env.protocol.resumePrompt(value));
  assert.ok(env.composer().value.includes('EXISTING ChatCMD task task-chat-original'));
  assert.equal(env.state.clicks, 0);
});

test('draft entered while model selection is pending must not be overwritten', async (t) => {
  const env = contentFixture(t, { url: 'https://chatgpt.com/' });
  const value = job({ phase: 'opening_new_chat', handoffText: BODY });
  env.state.onModel = async () => { env.composer().value = 'User typed during model selection'; };
  const token = env.probe(value, 'RESUME').documentToken;
  await env.message('prepare', value, 'RESUME', token);
  assert.equal(env.composer().value, 'User typed during model selection');
  assert.equal(env.state.writes.length, 0);
});

test('stale documentToken after content reload cannot prepare or click Send', async (t) => {
  const env = contentFixture(t);
  const value = job();
  const oldToken = await env.ready(value);
  env.reload();
  assert.notEqual(env.probe(value).documentToken, oldToken);
  for (const action of ['prepare', 'dispatch']) {
    const result = await env.message(action, value, 'HANDOFF', oldToken);
    assert.equal(result.ok, false);
  }
  assert.equal(env.state.clicks, 0);
  assert.equal(env.state.writes.length, 1);
});

test('navigation while pause is pending aborts without touching the new page draft', async (t) => {
  const env = contentFixture(t);
  const value = job();
  env.state.onPause = async () => {
    env.navigate('https://chatgpt.com/c/another-conversation');
    env.composer().value = 'New page draft';
  };
  const token = env.probe(value).documentToken;
  const result = await env.message('prepare', value, 'HANDOFF', token);
  assert.equal(result.ready, false);
  assert.equal(env.composer().value, 'New page draft');
  assert.equal(env.state.writes.length, 0);
});

test('draft changed between prepare and dispatch blocks the irreversible click', async (t) => {
  const env = contentFixture(t);
  const value = job();
  const token = await env.ready(value);
  env.composer().value = 'User edited the prepared prompt';
  const result = await env.message('dispatch', value, 'HANDOFF', token);
  assert.equal(result.ok, false);
  assert.equal(env.state.clicks, 0);
  assert.equal(env.composer().value, 'User edited the prepared prompt');
});

test('dispatch with an existing owned prompt returns without another click', async (t) => {
  const env = contentFixture(t);
  const value = job();
  const token = await env.ready(value);
  env.user(env.protocol.handoffPrompt(value));
  const result = await env.message('dispatch', value, 'HANDOFF', token);
  assert.equal(result.sent, true);
  assert.equal(env.state.clicks, 0);
});

test('duplicate markers arriving after prepare must fail closed at dispatch too', async (t) => {
  const env = contentFixture(t);
  const value = job();
  const token = await env.ready(value);
  env.user(env.protocol.handoffPrompt(value), 'duplicate-1');
  env.user(env.protocol.handoffPrompt(value), 'duplicate-2');
  await env.message('dispatch', value, 'HANDOFF', token);
  assert.equal(env.state.clicks, 0, 'Ambiguous ownership must not cause a third send');
});

test('handoff preparation stops a running response but does not send in the same step', async (t) => {
  const env = contentFixture(t);
  const value = job();
  env.generating(true);
  const token = env.probe(value).documentToken;
  const result = await env.message('prepare', value, 'HANDOFF', token);
  assert.equal(result.ready, false);
  assert.equal(env.state.stops, 1);
  assert.equal(env.state.clicks, 0);
  assert.equal(env.state.writes.length, 0);
});

test('disabled send and active generation both prevent dispatch', async (t) => {
  const env = contentFixture(t);
  const value = job();
  const token = await env.ready(value);
  const button = env.w.document.querySelector('[data-testid="send-button"]');
  button.setAttribute('aria-disabled', 'true');
  assert.equal((await env.message('dispatch', value, 'HANDOFF', token)).sent, false);
  button.removeAttribute('aria-disabled');
  env.generating(true);
  assert.equal((await env.message('dispatch', value, 'HANDOFF', token)).sent, false);
  assert.equal(env.state.clicks, 0);
});
