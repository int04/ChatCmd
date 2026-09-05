const assert = require('node:assert/strict');
const test = require('node:test');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { JSDOM } = require('../web/node_modules/jsdom');
const source = readFileSync(join(__dirname, 'page-chatgpt-render.js'), 'utf8');
function setup(t) {
  const page = new JSDOM('<body></body>', { url: 'https://chatgpt.com/c/one', runScripts: 'outside-only' });
  const w = page.window;
  let time = 1000, hidden = true, nextId = 0;
  const native = new Map();
  const errors = [];
  w.requestAnimationFrame = (fn) => { if (typeof fn !== 'function') throw new TypeError('callback'); const id = ++nextId; native.set(id, fn); return id; };
  w.cancelAnimationFrame = (id) => native.delete(Number(id));
  w.reportError = (error) => errors.push(error.message);
  Object.defineProperty(w.performance, 'now', { value: () => time });
  Object.defineProperty(w.document, 'visibilityState', { get: () => hidden ? 'hidden' : 'visible' });
  const original = w.requestAnimationFrame;
  w.eval(source);
  const pulse = (active = true, path = w.location.pathname) => w.document.dispatchEvent(new w.CustomEvent('chatcmd:render-pulse', {
    detail: JSON.stringify({ version: 1, path, active }),
  }));
  t.after(() => { w.__ChatCmdPageRender.dispose(); page.window.close(); });
  return { w, native, original, errors, pulse, advance(ms = 150) { time += ms; }, visible() { hidden = false; },
    fire(id, stamp = 777) { const fn = native.get(id); native.delete(id); fn?.(stamp); } };
}
test('hidden callbacks require a live pulse for the same path', (t) => {
  const e = setup(t); let count = 0;
  e.w.requestAnimationFrame(() => count++);
  e.pulse(false); e.pulse(true, '/c/other');
  assert.equal(count, 0);
  e.pulse(); assert.equal(count, 1); assert.equal(e.native.size, 0);
  e.pulse(); assert.equal(count, 1);
});
test('visible frames preserve native timestamp and cancellation handle', (t) => {
  const e = setup(t); e.visible(); const result = [];
  const id = e.w.requestAnimationFrame(function(time) { result.push([time, this === e.w]); });
  e.pulse(); assert.equal(result.length, 0);
  e.fire(id, 123.25); assert.deepEqual(result, [[123.25, true]]);
  e.pulse(); assert.equal(result.length, 1);
});
test('cancellation before or within fallback batch wins, without later double delivery', (t) => {
  const e = setup(t); const seen = [];
  const cancelled = e.w.requestAnimationFrame(() => seen.push('cancelled'));
  e.w.cancelAnimationFrame(cancelled);
  let later;
  e.w.requestAnimationFrame(() => { seen.push('first'); e.w.cancelAnimationFrame(later); });
  later = e.w.requestAnimationFrame(() => seen.push('later'));
  e.pulse(); e.visible(); for (const id of e.native.keys()) e.fire(id);
  assert.deepEqual(seen, ['first']);
});
test('reentrant frames run in a later bounded batch and use monotonic frame times', (t) => {
  const e = setup(t); const times = [];
  e.w.requestAnimationFrame((at) => { times.push(at); e.w.requestAnimationFrame((next) => times.push(next)); });
  e.pulse(); assert.equal(times.length, 1);
  e.pulse(); assert.equal(times.length, 1);
  e.advance(); e.pulse(); assert.deepEqual(times, [1000, 1150]);
});
test('callback exception does not starve remaining frames', (t) => {
  const e = setup(t); let done = false;
  e.w.requestAnimationFrame(() => { throw new Error('fixture failure'); });
  e.w.requestAnimationFrame(() => { done = true; });
  e.pulse(); assert.equal(done, true); assert.deepEqual(e.errors, ['fixture failure']);
});
test('old route callbacks are not executed by a new conversation pulse', (t) => {
  const e = setup(t); let old = false, next = false;
  e.w.requestAnimationFrame(() => { old = true; });
  e.w.history.pushState({}, '', '/c/two');
  e.w.requestAnimationFrame(() => { next = true; });
  e.pulse(); assert.equal(old, false); assert.equal(next, true);
});
test('home to canonical conversation promotion retains the pending first frame', (t) => {
  const e = setup(t); e.w.history.replaceState({}, '', '/'); let done = false;
  e.w.requestAnimationFrame(() => { done = true; });
  e.w.history.replaceState({}, '', '/c/new'); e.pulse(); assert.equal(done, true);
});
test('reinjection is idempotent and disposal does not drop native callbacks', (t) => {
  const e = setup(t); const wrapper = e.w.requestAnimationFrame;
  e.w.eval(source); assert.equal(e.w.requestAnimationFrame, wrapper);
  let count = 0; const id = e.w.requestAnimationFrame(() => count++);
  e.w.__ChatCmdPageRender.dispose(); assert.equal(e.w.requestAnimationFrame, e.original);
  e.pulse(); assert.equal(count, 0); e.fire(id); assert.equal(count, 1);
});
test('large callback queues have a finite per-pulse budget', (t) => {
  const e = setup(t); let count = 0;
  for (let i = 0; i < 1100; i++) e.w.requestAnimationFrame(() => count++);
  e.pulse(); assert.equal(count, 1000); e.advance(); e.pulse(); assert.equal(count, 1100);
});
