// Startup failure is not proof that a child stopped. Let the server arbitrate before cleanup.
const subagentFailureReports = new Map();

async function settleSubagentStartupFailure(subagentId, attempt, localBaseUrl, error) {
  attempt = Number(attempt);
  if (!subagentId || !localBaseUrl || !Number.isInteger(attempt) || attempt < 1 || attempt > 3) return;
  const key = `${subagentId}:${attempt}`;
  if (subagentFailureReports.has(key)) return subagentFailureReports.get(key);
  const work = (async () => {
    try {
      const bindingKey = `${SUBAGENT_PREFIX}${subagentId}`;
      const stored = await chrome.storage.session.get(bindingKey);
      const binding = stored[bindingKey];
      if (binding && Number(binding.attempt) !== attempt) return;
      const result = await postJson(localBaseUrl, `/api/local/subagents/${encodeURIComponent(subagentId)}/fallback/result`, {
        attempt, status: 'failed', errorMessage: errorMessage(error),
      });
      // Never close a claimed child, overwrite newer bindings, or retry an unknown ACK.
      if (result?.accepted !== true) return;
      await closeSubagentRequest(subagentId, attempt);
    } catch (reportError) {
      // Keep request state when delivery is uncertain. The server's lease/deadline still applies.
      await logExtension('error', 'subagent-startup',
        `Cannot acknowledge startup failure for ${subagentId}, attempt ${attempt}: ${errorMessage(reportError)}`)
        .catch(() => undefined);
    }
  })();
  subagentFailureReports.set(key, work);
  try { return await work; } finally {
    if (subagentFailureReports.get(key) === work) subagentFailureReports.delete(key);
  }
}
