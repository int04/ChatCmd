// Dependency-free CDP harness. Starts only a disposable profile, never the user's browser.
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { EventEmitter } = require('node:events');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(test, label, timeout = 12000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { const value = await test(); if (value) return value; await sleep(75); }
  throw new Error(`Timed out: ${label}`);
}
async function launch() {
  const executable = process.env.CHATCMD_TEST_CHROME || 'C:/Program Files/Google/Chrome/Application/chrome.exe';
  if (!fs.existsSync(executable)) throw new Error('Set CHATCMD_TEST_CHROME to an installed Chrome executable.');
  const root = path.resolve(__dirname, '../../target/browser-capture-tests');
  fs.mkdirSync(root, { recursive: true });
  const profile = fs.mkdtempSync(path.join(root, 'render-'));
  const child = spawn(executable, ['--headless=new', '--no-first-run', '--no-default-browser-check',
    '--remote-debugging-pipe', '--enable-unsafe-extension-debugging', '--disable-background-networking',
    `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'], windowsHide: true });
  const events = new EventEmitter();
  const pending = new Map();
  let sequence = 0;
  let buffer = Buffer.alloc(0);
  let stderr = '';
  child.stderr.on('data', (data) => { stderr = (stderr + data).slice(-2000); });
  const fail = (error) => { for (const item of pending.values()) { clearTimeout(item.timer); item.reject(error); } pending.clear(); };
  child.on('error', fail);
  child.once('exit', (code) => fail(new Error(`Chrome exited ${code}: ${stderr}`)));
  child.stdio[4].on('data', (data) => {
    buffer = Buffer.concat([buffer, data]);
    let at;
    while ((at = buffer.indexOf(0)) >= 0) {
      const message = JSON.parse(buffer.subarray(0, at).toString('utf8'));
      buffer = buffer.subarray(at + 1);
      if (message.id) {
        const item = pending.get(message.id);
        if (!item) continue;
        pending.delete(message.id); clearTimeout(item.timer);
        if (message.error) item.reject(new Error(`${item.method}: ${JSON.stringify(message.error)}`));
        else item.resolve(message.result);
      } else events.emit(message.method, message.params, message.sessionId);
    }
  });
  function call(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 12000);
      pending.set(id, { method, timer, resolve, reject });
      child.stdio[3].write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + '\0');
    });
  }
  async function evaluate(session, expression) {
    const value = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }, session);
    if (value.exceptionDetails) throw new Error(value.exceptionDetails.exception?.description || value.exceptionDetails.text);
    return value.result.value;
  }
  async function close() {
    if (child.exitCode === null) {
      await call('Browser.close').catch(() => {});
      await Promise.race([new Promise((resolve) => child.once('exit', resolve)), sleep(1500)]);
      if (child.exitCode === null) child.kill();
    }
    fail(new Error('Harness closed')); events.removeAllListeners();
    try { fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 150 }); }
    catch { console.warn('Temporary Chrome profile still locked:', profile); }
  }
  return { call, events, evaluate, close, version: await call('Browser.getVersion') };
}
module.exports = { launch, until, sleep };
