async function locateCompactDestination(job, record, tabs) {
  const tagged = tabs.filter((tab) => {
    try { return new URL(tab.url).hash === `#chatcmd-compact=${encodeURIComponent(job.id)}`; }
    catch { return false; }
  });
  if (tagged.length > 1) throw new Error('Có nhiều tab nhận cùng handoff. Không tự chọn để tránh gắn nhầm cuộc trò chuyện.');
  if (tagged[0]) return tagged[0];
  if (job.newConversationId) {
    const canonical = tabs.find((tab) => conversationIdFromUrl(tab.url || '') === job.newConversationId);
    if (canonical) return canonical;
  }
  const bound = tabs.find((tab) => tab.id === record.destinationTabId);
  if (bound && !job.newConversationId && !conversationIdFromUrl(bound.url || '') && isChatGptUrl(bound.url)) return bound;
  // Chrome's tab URL can lag behind ChatGPT's SPA navigation, and a browser restart can
  // also change tab ids after the operation hash is removed. The exact RESUME user marker
  // is durable ownership evidence, so use it even when the browser still reports a home/
  // project URL or when a known canonical id is temporarily absent from chrome.tabs.query.
  const matches = [];
  for (const tab of tabs) {
    const conversationId = conversationIdFromUrl(tab.url || '');
    const mayBeStaleDestination = Boolean(job.newConversationId || tab.id === record.destinationTabId);
    if (!tab.id || !isChatGptUrl(tab.url) || sameConversationUrl(tab.url, job.oldConversationUrl)
      || (!conversationId && !mayBeStaleDestination)) continue;
    try {
      const found = await chrome.tabs.sendMessage(tab.id, { type: 'chatcmd-compact-locate', job, kind: 'RESUME' });
      if (found?.ok && found.markerFound) matches.push(tab);
    } catch { /* an unrelated or not-yet-loaded document does not prove ownership */ }
  }
  if (matches.length > 1) throw new Error('Hai cuộc trò chuyện chứa cùng handoff; cần kiểm tra thủ công trước khi chuyển task.');
  return matches[0] || null;
}
async function compactDestination(job, record, tabs) {
  if (!job.handoffText) throw new Error('Chưa có bản handoff đã lưu. Không mở cuộc trò chuyện rỗng.');
  let destination = await locateCompactDestination(job, record, tabs);
  if (!destination?.id) {
    if (record.destinationOpened || record.destinationSend === 'dispatched-unresolved' || record.destinationSend === 'unknown') {
      await compactDetail(record, job, 'Đang chờ mở lại tab ChatGPT mới đã tạo. Không tạo chat thứ hai; hãy khôi phục tab đã đóng hoặc mở chat có tin nhắn handoff.');
      return;
    }
    // Do not resurrect a tab the user just closed. A loaded source is required for
    // the first opening, while an existing destination can finish without the source.
    if (!tabs.some((tab) => conversationIdFromUrl(tab.url || '') === job.oldConversationId)) {
      await compactDetail(record, job, 'Handoff đã lưu an toàn. Mở lại ChatGPT cũ để tiếp tục mở cuộc trò chuyện mới.');
      return;
    }
    record = await saveCompactRecord(job.id, { ...record, destinationOpened: true,
      destinationUrl: ChatCmdCompactProtocol.destinationUrl(job.oldConversationUrl, job.id) });
    destination = await chrome.tabs.create({ url: record.destinationUrl, active: false });
    record = await saveCompactRecord(job.id, { ...record, destinationTabId: destination.id });
    return;
  }
  record = await saveCompactRecord(job.id, { ...record, destinationTabId: destination.id, destinationOpened: true });
  const probe = await compactSend(destination.id, 'probe', job, 'RESUME');
  if (probe.markerFound && probe.conversationId && !isProvisionalConversationId(probe.conversationId)) {
    // A later user turn does not invalidate the destination. It is the normal race when the
    // user continues immediately after ChatGPT acknowledges RESUME but before the worker's
    // final checkpoint. The exact operation marker still proves this conversation owns the
    // handoff; RuntimeHost separately prevents local tools from running until attachment.
    // Publish the recoverable destination identity first, then immediately re-probe the
    // same tab instead of sleeping for another scheduler tick before the final commit.
    let confirmed = probe;
    if (job.newConversationId !== probe.conversationId) {
      job = await compactCheckpoint(record, job, { newConversationId: probe.conversationId,
        newConversationUrl: probe.conversationUrl, detail: null });
      confirmed = await compactSend(destination.id, 'probe', job, 'RESUME');
    }
    if (!confirmed.markerFound || confirmed.conversationId !== job.newConversationId
      || isProvisionalConversationId(confirmed.conversationId) || confirmed.generating) return;
    job = await compactCheckpoint(record, job, { phase: 'completed', newConversationId: confirmed.conversationId,
      newConversationUrl: confirmed.conversationUrl, detail: null });
    await finishCompactBrowser(job, record);
    return;
  }
  if (!['not-attempted', 'not-sent'].includes(record.destinationSend)) {
    await compactDetail(record, job, 'Đang đối chiếu lần gửi handoff vào chat mới. Không gửi lần hai khi kết quả chưa rõ; khôi phục đúng tab để tiếp tục.');
    return;
  }
  if (probe.conversationId || probe.generating) throw new Error('Tab đích không còn là cuộc trò chuyện trống. Không ghi đè hoặc gửi handoff vào chat khác.');
  const ready = await compactSend(destination.id, 'prepare', job, 'RESUME', probe.documentToken);
  if (!ready.ready) return;
  // Recheck the DB immediately before the irreversible dispatch (cancel/stale guard).
  job = await compactCheckpoint(record, job, { detail: 'Đang chuyển bản handoff đã lưu sang cuộc trò chuyện mới.' });
  await compactDispatch(destination.id, job, record, 'RESUME', probe.documentToken);
}
async function finishCompactBrowser(job, record) {
  if (!ChatCmdCompactProtocol.terminal(job)) return;
  // Finished jobs must never close a source the user later opens from history.
  if (record.finished) { compactJobs.delete(job.id); return; }
  for (const tabId of [record.sourceChatTabId, record.destinationTabId].filter(Boolean)) {
    try { await chrome.tabs.sendMessage(tabId, { type: 'chatcmd-compact-clear', jobId: job.id }); } catch { /* closed */ }
  }
  if (job.phase === 'completed') {
    if (job.oldRequestId) { await releaseRequest(job.oldRequestId); await forgetRecoveryRequest(job.oldRequestId); }
    const tabs = await chatGptTabs();
    const destination = tabs.find((tab) => conversationIdFromUrl(tab.url || '') === job.newConversationId);
    if (!destination?.id) return; // Resume when this exact chat is reopened; do not create another one.
    await bindConversationTab(job.newConversationId, destination.id, { localBaseUrl: record.localBaseUrl, requestId: null });
    if (record.sourceTabId && await safeTab(record.sourceTabId)) await bindReturnSource(destination.id, record.sourceTabId);
    let closeError;
    try { record = await retireCompactSource(job, record, destination); }
    catch (error) {
      closeError = error;
      record = await compactRecord(job.id) || record;
      await logExtension('warn', 'compact', `Đã chuyển handoff nhưng chưa đóng được tab nguồn: ${errorMessage(error)}`);
    }
    // A transient tab-close failure must not prevent an explicitly requested continuation.
    if (job.continueAfterCompact === true) record = await resumeCompactWork(job, record);
    if (closeError) throw closeError; // Keep cleanup unfinished for durable recovery, not a new compact.
  }
  await saveCompactRecord(job.id, { ...record, finished: true, finishedAt: Date.now() });
  compactJobs.delete(job.id);
}

function compactTabMatches(tab, conversationId) {
  return Boolean(tab && tab.status !== 'loading' && conversationIdFromUrl(tab.url || '') === conversationId
    && (!tab.pendingUrl || conversationIdFromUrl(tab.pendingUrl) === conversationId));
}

async function retireCompactSource(job, record, destination) {
  if (['closed', 'skipped'].includes(record.sourceClose?.state)) return record;
  const tabId = record.sourceChatTabId; // Never scan all tabs with the archived URL.
  const finish = (state, reason) => saveCompactRecord(job.id, { ...record,
    sourceClose: { ...record.sourceClose, state, tabId: tabId || null, reason } });
  if (job.phase !== 'completed' || !job.handoffText || !job.newConversationId
    || job.newConversationId === job.oldConversationId) return finish('skipped', 'attachment-not-confirmed');
  if (!Number.isInteger(tabId) || tabId <= 0 || tabId === destination.id || tabId === record.sourceTabId) {
    return finish('skipped', 'source-tab-not-owned');
  }
  let source = await safeTab(tabId);
  if (!source) return finish('closed', 'already-closed');
  if (!compactTabMatches(source, job.oldConversationId)) return finish('skipped', 'source-navigated');
  if (!compactTabMatches(await safeTab(destination.id), job.newConversationId)) {
    throw new Error('ChatGPT mới chưa sẵn sàng; giữ tab nguồn để khôi phục.');
  }
  const check = await compactSend(tabId, 'close-check', job, 'HANDOFF');
  if (check.safeToClose !== true || !check.documentToken || !check.userMessageId
    || check.conversationId !== job.oldConversationId) return finish('skipped', 'source-has-new-state');
  if (record.sourceClose?.state === 'closing'
    && (record.sourceClose.documentToken !== check.documentToken || record.sourceClose.userMessageId !== check.userMessageId)) {
    return finish('skipped', 'source-document-replaced');
  }
  // Persist the exact document BEFORE removal. A restarted worker cannot close a
  // reopened reference document, even if Chrome reused the original tab id.
  record = await saveCompactRecord(job.id, { ...record, sourceClose: {
    state: 'closing', tabId, documentToken: check.documentToken, userMessageId: check.userMessageId,
  } });
  if (source.active && Number.isInteger(source.windowId) && source.windowId === destination.windowId) {
    await chrome.tabs.update(destination.id, { active: true });
  }
  source = await safeTab(tabId);
  if (!source) return finish('closed', 'already-closed');
  if (!compactTabMatches(source, job.oldConversationId)) return finish('skipped', 'source-navigated');
  if (!compactTabMatches(await safeTab(destination.id), job.newConversationId)) {
    throw new Error('Tab ChatGPT mới đã đổi hoặc đóng; chưa đóng tab nguồn.');
  }
  const finalCheck = await compactSend(tabId, 'close-check', job, 'HANDOFF', check.documentToken);
  if (finalCheck.safeToClose !== true || finalCheck.documentToken !== check.documentToken
    || finalCheck.userMessageId !== check.userMessageId || finalCheck.conversationId !== job.oldConversationId) {
    return finish('skipped', 'source-changed-before-close');
  }
  await chrome.tabs.remove(tabId);
  return finish('closed', 'handoff-attached');
}

async function resumeCompactWork(job, record) {
  if (job.continueAfterCompact !== true) return record;
  const result = await postJson(record.localBaseUrl, `${compactPath(job.id)}/resume`, {});
  if (!result.requestId) return record; // A real user message already continued this task.
  const request = await getJson(record.localBaseUrl, `/api/local/chatgpt/requests/${encodeURIComponent(result.requestId)}`);
  if (request.status !== 'queued' || record.resumeDispatch) return record;
  record = await saveCompactRecord(job.id, { ...record, resumeRequestId: result.requestId, resumeDispatch: 'dispatched-unresolved' });
  // The ordinary bridge observes the working answer on the SAME task. Persist the
  // fence before invoking it; uncertain dispatch is never blindly replayed.
  try {
    await startRequest({ requestId: result.requestId, localBaseUrl: record.localBaseUrl,
      conversationUrl: job.newConversationUrl, model: job.oldModel,
      submittedContent: request.submittedContent, sourceTabId: record.sourceTabId });
  } catch (error) {
    await reportFailure(result.requestId, record.localBaseUrl, errorMessage(error));
  }
  return record;
}
