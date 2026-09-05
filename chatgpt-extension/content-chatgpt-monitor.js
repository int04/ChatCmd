globalThis.ChatCmdMonitor = Object.freeze({ create(api) {
  const { assistantNodes, latestMessageText, findStopButton, findThreadError, clickStopButton } = globalThis.ChatCmdConversationDom;
  return async function waitForAssistant(previousCount, requestId, submittedContent) {
  let baselineCount = previousCount;
  let lastText = '';
  let stableSince = 0;
  let lastActivityAt = Date.now();
  let lastStateCheckAt = 0;
  let lastCompletionPingAt = 0;
  let lastRequestState = api.unknownRequestState();
  let observedProgress = false;
  const startedAt = Date.now();
  while (Date.now() - startedAt < 10 * 60_000) {
    if (!api.activeRequest || api.activeRequest.id !== requestId || api.activeRequest.resultReported) return latestMessageText('assistant');
    const now = Date.now();
    const nodes = assistantNodes();
    const latest = nodes.at(-1);
    const recorder = api.activeRequest.observer;
    if (recorder) {
      recorder.scan();
      if (!recorder.active) return recorder.answer;
      void recorder.flush();
    }
    const text = recorder ? recorder.answer : (latest?.innerText?.trim() || latest?.textContent?.trim() || '');
    const stopButton = findStopButton();
    const threadError = findThreadError();

    if (now - lastStateCheckAt > 800) {
      lastStateCheckAt = now;
      lastRequestState = await api.requestState(requestId);
      if (lastRequestState.stopRequested && api.activeRequest?.id === requestId && !api.activeRequest.stopRequested) {
        api.activeRequest.stopRequested = true;
        clickStopButton();
        await api.delay(250);
        continue;
      }
    }

    if (api.activeRequest?.id === requestId && api.activeRequest.stopRequested) {
      clickStopButton();
      if (!findStopButton() && (lastRequestState.stopRequested || api.isTerminalRequestState(lastRequestState))) return text;
    }

    if (stopButton) {
      observedProgress = true;
      lastActivityAt = now;
    }
    const hasNewAssistantText = (recorder ? recorder.hasTurn : nodes.length > baselineCount) && Boolean(text);
    if (hasNewAssistantText) observedProgress = true;
    if (hasNewAssistantText && !threadError) {
      if (text !== lastText) {
        lastText = text;
        stableSince = now;
        lastActivityAt = now;
      } else if (!stableSince) {
        stableSince = now;
      }

      if (stopButton) stableSince = now;
      const stableMs = stableSince ? now - stableSince : 0;
      const settleMs = recorder ? 4_000 : api.RAW_BUBBLE_STABILITY_MS;
      if (!stopButton && stableMs >= settleMs && api.isTerminalRequestState(lastRequestState)) {
        if (!recorder || await recorder.flush(true)) return text;
      }
      if (
        !stopButton && !threadError && api.findComposer() &&
        stableMs >= settleMs && now - lastCompletionPingAt >= api.COMPLETION_PING_INTERVAL_MS
      ) {
        lastCompletionPingAt = now;
        if (await api.reportBrowserCompletion(requestId, text)) return text;
      }
    }

    if (!stopButton && api.findComposer()) {
      const idleMs = now - lastActivityAt;
      const reason = threadError && idleMs >= api.ERROR_INTERRUPT_GRACE_MS
        ? 'thread_error'
        : idleMs >= api.SILENT_RETRY_GRACE_MS && !hasNewAssistantText
          ? 'send_ready_without_final'
          : null;
      if (reason && api.AUTO_RETRY_ENABLED) {
        lastRequestState = await api.requestState(requestId);
        if (!lastRequestState.known || lastRequestState.hasFinalResponse || !lastRequestState.active) {
          await api.delay(350);
          continue;
        }
        if ((api.activeRequest?.retryCount || 0) >= api.MAX_AUTO_RETRIES) {
          throw new Error(`ChatGPT vẫn chưa có phản hồi cuối sau ${api.MAX_AUTO_RETRIES} lần tự động gửi lại.`);
        }
        baselineCount = nodes.length;
        lastText = '';
        stableSince = 0;
        await api.retryPrompt(requestId, observedProgress ? api.INTERRUPTED_PROGRESS_PROMPT : submittedContent, reason, observedProgress);
        lastActivityAt = Date.now();
        await api.delay(650);
        continue;
      }
    }
    await api.delay(350);
  }
  throw new Error('Quá lâu chưa nhận được phản hồi hoàn tất từ ChatGPT.');
};
} });
