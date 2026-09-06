// MV3 alarms keep browser children alive even while their ChatGPT tab is inactive.
// Session-owned tab + attempt are checked before every renewal; no prompt is resent.
const subagentHeartbeatCache = new Map();
const subagentHeartbeatInFlight = new Map();
const SUBAGENT_HEARTBEAT_ALARM = 'chatcmd-subagent-heartbeat';

async function subagentHeartbeatState(context, force = false) {
  const key = `${context.subagentId}:${context.attempt}`;
  const existing = subagentHeartbeatInFlight.get(key);
  if (existing) return existing;
  const cached = subagentHeartbeatCache.get(key);
  if (!force && cached && Date.now() - cached.at < 15_000) return cached.value;
  const work = (async () => {
    const tab = await safeTab(context.tabId);
    if (!tab || !isChatGptUrl(tab.url || '') || tab.discarded) return { active: false, status: 'unavailable' };
    const liveId = conversationIdFromUrl(tab.url || '');
    if (context.conversationId && liveId && !isProvisionalConversationId(context.conversationId)
      && context.conversationId !== liveId) return { active: false, status: 'unavailable' };
    const value = await postJson(context.localBaseUrl, `/api/local/subagents/${encodeURIComponent(context.subagentId)}/fallback/heartbeat`, { attempt: context.attempt });
    subagentHeartbeatCache.set(key, { at: Date.now(), value });
    return value;
  })();
  subagentHeartbeatInFlight.set(key, work);
  try { return await work; } finally { subagentHeartbeatInFlight.delete(key); }
}

async function renewBrowserSubagents() {
  const stored = await chrome.storage.session.get(null);
  const contexts = Object.entries(stored).filter(([key, context]) => key.startsWith(REQUEST_PREFIX)
    && context?.mode === 'subagent' && context.localBaseUrl && context.tabId && context.subagentId);
  const activeKeys = new Set(contexts.map(([, value]) => `${value.subagentId}:${value.attempt}`));
  for (const key of subagentHeartbeatCache.keys()) if (!activeKeys.has(key)) subagentHeartbeatCache.delete(key);
  await Promise.allSettled(contexts.map(async ([, context]) => {
    const value = await subagentHeartbeatState(context, true);
    if (['failed', 'stopped', 'interrupted', 'timedOut'].includes(value.status)
      || value.reason === 'stale_attempt') {
      await closeSubagentRequest(context.subagentId, context.attempt);
    }
    // MCP completion precedes the actual final answer. Keep the tab until the
    // content monitor observes that answer and acknowledges browser completion.
  }));
}

chrome.alarms.create(SUBAGENT_HEARTBEAT_ALARM, { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === SUBAGENT_HEARTBEAT_ALARM) void renewBrowserSubagents().catch(() => undefined);
});
setTimeout(() => void renewBrowserSubagents().catch(() => undefined), 500);
