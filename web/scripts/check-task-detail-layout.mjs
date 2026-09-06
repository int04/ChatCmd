// Isolated headless-Chrome CSS smoke test. No live ChatGPT tabs or profiles are touched.
// Run: node scripts/check-task-detail-layout.mjs (CHROME_BIN may override browser discovery).
import { existsSync, mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import process from 'node:process';
import console from 'node:console';
const root = fileURLToPath(new URL('../..', import.meta.url));
const browser = [process.env.CHROME_BIN, 'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe', '/usr/bin/google-chrome', '/usr/bin/chromium',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'].find((file) => file && existsSync(file));
if (!browser) throw new Error('Set CHROME_BIN to an installed Chromium/Chrome executable.');
mkdirSync(join(root, '.smoke'), { recursive: true });
const temp = mkdtempSync(join(root, '.smoke', 'task-layout-'));
const styles = ['styles.css', 'claude-code-theme.css', 'tasks/taskDetailLayout.css']
  .map((file) => `<link rel="stylesheet" href="${pathToFileURL(join(root, 'web', 'src', file)).href}">`).join('');
const cases = [
  { width: 1440, height: 900, long: false, collapsed: false },
  { width: 900, height: 700, long: true, collapsed: false },
  { width: 1100, height: 700, long: true, collapsed: true },
  { width: 520, height: 844, long: true, collapsed: false },
];
try {
  for (const [index, sample] of cases.entries()) {
    const html = `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">${styles}</head>
    <body><div class="content-shell"><main class="tasks-main"><div class="tasks-workspace"><section class="tasks-detail-pane">
    <div class="task-detail-shell ${sample.collapsed ? 'sidebar-collapsed' : ''}"><div class="task-detail-body">
    <div class="task-chat-pane has-chatgpt-footer"><header class="task-detail-topbar" tabindex="0"><div ${sample.long ? 'style="height:450px"' : ''}><h1>Conversation layout regression</h1><p>Agent progress and task metadata</p></div><div class="task-detail-topbar-actions"><button>Toggle</button></div></header>
    <main class="task-chat-column"><div style="height:2400px">Conversation content</div></main><footer class="task-chat-footer"><textarea aria-label="Message"></textarea></footer></div>
    ${sample.collapsed ? '' : '<aside class="task-detail-sidebar"><header class="task-info-header">Task information</header><div style="min-height:3000px">Sidebar content</div></aside>'}
    </div></div></section></div></main></div><pre id="layout-result" hidden></pre><script>
    addEventListener('load', () => {
      const $ = (selector) => document.querySelector(selector);
      const header = $('.task-detail-topbar'), chat = $('.task-chat-column'), sidebar = $('.task-detail-sidebar'), body = $('.task-detail-body'), pane = $('.task-chat-pane');
      const rect = (element) => { const r = element.getBoundingClientRect(); return { top:r.top, left:r.left, right:r.right, bottom:r.bottom, width:r.width, height:r.height }; };
      const before = rect(header);
      chat.scrollTop = 120;
      const afterChat = rect(header), untouchedSidebar = sidebar?.scrollTop || 0;
      header.scrollTop = 80;
      if (sidebar) sidebar.scrollTop = 120;
      const result = { viewport: innerWidth, documentWidth: document.documentElement.scrollWidth, header:before, afterChat, body:rect(body), pane:rect(pane), sidebar:sidebar ? rect(sidebar) : null,
        headerOverflow:getComputedStyle(header).overflowY, headerScroll:header.scrollTop, chatScroll:chat.scrollTop, sidebarScroll:sidebar?.scrollTop, untouchedSidebar, chatHeight:chat.clientHeight, footer:rect($('.task-chat-footer')) };
      $('#layout-result').textContent = JSON.stringify(result);
    });</script></body></html>`;
    const file = join(temp, `case-${index}.html`);
    writeFileSync(file, html);
    const run = spawnSync(browser, ['--headless=new', '--disable-gpu', '--disable-extensions', '--disable-sync', '--disable-background-networking', '--no-first-run', '--no-default-browser-check', '--allow-file-access-from-files', '--force-device-scale-factor=1', `--user-data-dir=${join(temp, `profile-${index}`)}`, `--window-size=${sample.width},${sample.height}`, '--virtual-time-budget=1000', '--dump-dom', pathToFileURL(file).href], { encoding:'utf8', timeout:45000, maxBuffer:4*1024*1024, windowsHide:true });
    if (run.error || run.status !== 0) throw new Error(run.error?.message || run.stderr.slice(-2000));
    const match = run.stdout.match(/<pre id="layout-result" hidden[^>]*>([\s\S]*?)<\/pre>/);
    assert.ok(match, 'Browser must produce a layout report');
    const result = JSON.parse(match[1]);
    console.log(JSON.stringify({ ...sample, result }));
    assert.ok(result.documentWidth <= result.viewport + 1, 'no horizontal page overflow');
    assert.equal(result.headerOverflow, 'auto');
    assert.ok(result.chatScroll > 0, 'chat has its own scroll');
    assert.equal(result.untouchedSidebar, 0, 'chat scrolling must not scroll the sidebar');
    assert.equal(result.header.top, result.afterChat.top, 'header stays fixed while chat scrolls');
    assert.ok(result.chatHeight > 100, 'header leaves visible conversation space');
    assert.ok(result.footer.bottom <= result.pane.bottom + 1, 'composer fits inside chat pane');
    if (sample.long) assert.ok(result.headerScroll > 0, 'long header scrolls independently');
    if (result.viewport > 820 && !sample.collapsed) {
      assert.ok(Math.abs(result.header.top - result.sidebar.top) <= 1, 'sidebar starts at the same top as the chat header');
      assert.ok(result.sidebarScroll > 0, 'sidebar has its own scroll');
      assert.ok(Math.abs(result.sidebar.height - result.body.height) <= 1, 'sidebar uses the entire body height');
      assert.ok(result.sidebar.left >= result.pane.right - 1, 'columns do not overlap');
    }
    if (result.viewport <= 820) {
      assert.ok(Math.abs(result.pane.width - result.body.width) <= 1, 'mobile chat fills the body width');
      assert.ok(Math.abs(result.sidebar.width - result.body.width) <= 1, 'mobile sidebar fills the body width');
    }
    if (sample.collapsed) assert.ok(Math.abs(result.pane.width - result.body.width) <= 1, 'collapsed chat uses available width');
  }
  console.log('PASS: 4 isolated browser layout/scroll scenarios');
} finally {
  rmSync(temp, { recursive:true, force:true, maxRetries:8, retryDelay:250 });
}
