'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { protocol, job, BODY } = require('./compact-test-fixtures.cjs');

const p = protocol();
const own = job();

test('operation markers round-trip both generated prompts without replacing task identity', () => {
  for (const [kind, make] of [['HANDOFF', p.handoffPrompt], ['RESUME', p.resumePrompt]]) {
    const prompt = make({ ...own, handoffText: BODY });
    const parsed = p.parse(prompt);
    assert.equal(parsed[1], kind);
    assert.equal(parsed[2], own.id);
    assert.equal(prompt.split('\n')[0], p.marker(kind, own.id));
  }
  const resume = p.resumePrompt({ ...own, handoffText: BODY });
  assert.ok(resume.includes(`EXISTING ChatCMD task ${own.taskId}`));
  assert.ok(resume.includes(own.oldConversationUrl));
  assert.ok(resume.includes(own.projectFolder));
  assert.ok(resume.includes(`<handoff>\n${BODY}\n</handoff>`));
  assert.equal(own.taskId, 'task-chat-original');
});

test('parse rejects partial, embedded, wrong-kind and malformed operation markers', () => {
  for (const text of [
    'quoted ' + p.marker('HANDOFF', own.id),
    p.marker('HANDOFF-END', own.id), p.marker('HANDOFF', 'short'),
    p.marker('HANDOFF', 'x'.repeat(81)), p.marker('HANDOFF', 'bad id 000'),
    p.marker('HANDOFF', own.id) + 'unseparated', '[[CHATCMD-HANDOFF:unterminated',
  ]) assert.equal(p.parse(text), null, text);
  assert.equal(p.parse(' \r\n' + p.marker('RESUME', 'abcdefgh') + '\r\n')[2], 'abcdefgh');
});

test('handoff requires its own complete end fence and 200..100000 body characters', () => {
  const end = p.marker('HANDOFF-END', own.id);
  assert.equal(p.handoffText('x'.repeat(199) + '\n' + end, own.id), null);
  assert.equal(p.handoffText('x'.repeat(200) + '\n' + end, own.id), 'x'.repeat(200));
  assert.equal(p.handoffText('x'.repeat(100000) + '\n' + end, own.id)?.length, 100000);
  assert.equal(p.handoffText('x'.repeat(100001) + '\n' + end, own.id), null);
  assert.equal(p.handoffText(BODY, own.id), null);
  assert.equal(p.handoffText(BODY + '\n' + p.marker('HANDOFF-END', 'another-job-000'), own.id), null);
  assert.equal(p.handoffText(BODY + '\n' + end + '\nextra text', own.id), null);
  assert.equal(p.handoffText(BODY + '\n' + end.slice(0, -1), own.id), null);
  assert.equal(p.handoffText(BODY + '\r\n' + end + '\r\n', own.id), BODY);
});

test('end fence must be on its own final line, not appended inside body prose', () => {
  const text = BODY + p.marker('HANDOFF-END', own.id);
  assert.equal(p.handoffText(text, own.id), null, 'Inline marker is not the requested final-line fence');
});

test('canonical prompt comparison tolerates CRLF and NBSP but not changed content', () => {
  assert.equal(p.canonical(' \r\na\u00a0b\rline\r\n '), 'a b\nline');
  const prompt = p.handoffPrompt(own);
  assert.equal(p.canonical(prompt.replaceAll('\n', '\r\n')), p.canonical(prompt));
  assert.notEqual(p.canonical(prompt + '\nchanged'), p.canonical(prompt));
});

test('destination preserves ordinary, project and custom GPT routing with an operation tag', () => {
  const suffix = '#chatcmd-compact=' + own.id;
  assert.equal(p.destinationUrl(own.oldConversationUrl, own.id), 'https://chatgpt.com/' + suffix);
  assert.equal(p.destinationUrl('https://chatgpt.com/g/g-p-project/c/old', own.id),
    'https://chatgpt.com/g/g-p-project/project' + suffix);
  assert.equal(p.destinationUrl('https://chatgpt.com/g/g-custom/c/old', own.id),
    'https://chatgpt.com/g/g-custom' + suffix);
  for (const url of ['http://chatgpt.com/c/old', 'https://chatgpt.com.evil/c/old', 'https://example.com/c/old']) {
    assert.throws(() => p.destinationUrl(url, own.id), /Invalid/);
  }
});

test('protocol exposes exactly the four workflow statuses and only true terminal phases', () => {
  assert.deepEqual([...p.steps], ['Preparing', 'Writing the handoff', 'Saving it', 'Opening the new chat']);
  assert.deepEqual([...p.phases], ['preparing', 'writing_handoff', 'saving_handoff', 'opening_new_chat']);
  for (const phase of p.phases) assert.equal(p.terminal({ phase }), false);
  for (const phase of ['completed', 'cancelled']) assert.equal(p.terminal({ phase }), true);
  assert.equal(p.terminal(null), false);
});
