'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { JSDOM } = require('../web/node_modules/jsdom');
const source = readFileSync(join(__dirname, 'content-chatgpt-render.js'), 'utf8');

function fixture(t) {
  const page = new JSDOM('<body></body>', { url: 'https://chatgpt.com/c/hidden', runScripts: 'outside-only' });
  const w = page.window;
  const jobs = new Map();
  const pulses = [];
  let sequence = 0;
  Object.defineProperty(w.document, 'visibilityState', { get: () => 'hidden' });
  w.ChatCmdRuntime = { install: () => ({ id: 'render-bridge' }), current: () => true };
  w.ChatCmdCaptureClock = {
    later(fn, ms) { const id = ++sequence; jobs.set(id, { fn, ms }); return id; },
    cancel(id) { jobs.delete(id); },
  };
  w.ChatCmdController = { current: () => null, active: null };
  w.document.addEventListener('chatcmd:render-pulse', (event) => pulses.push(JSON.parse(event.detail)));
  w.eval(source);
  t.after(() => { w.ChatCmdRenderBridge?.stop(); w.close(); });
  return { w, jobs, pulses };
}

test('explicit compact lease keeps hidden render pulses alive without an active request', (t) => {
  const env = fixture(t);
  assert.equal(env.pulses.at(-1).active, false);
  assert.equal(env.jobs.size, 0);

  env.w.ChatCmdRenderBridge.setLease('compact', true);
  assert.equal(env.pulses.at(-1).active, true);
  assert.equal(env.jobs.size, 1);
  const [firstId, first] = [...env.jobs.entries()][0];
  assert.equal(first.ms, 250);

  env.jobs.delete(firstId);
  first.fn();
  assert.equal(env.pulses.at(-1).active, true);
  assert.equal(env.jobs.size, 1, 'hidden lease must re-arm through the worker-backed capture clock');

  env.w.ChatCmdRenderBridge.setLease('compact', false);
  assert.equal(env.pulses.at(-1).active, false);
  const [lastId, last] = [...env.jobs.entries()][0];
  env.jobs.delete(lastId);
  last.fn();
  assert.equal(env.pulses.at(-1).active, false);
  assert.equal(env.jobs.size, 0, 'released lease must stop the hidden heartbeat');
});
