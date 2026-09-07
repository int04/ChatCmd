const APPROVAL_BASE_URL_KEY = 'chatcmd-approval-base-url';
const DEFAULT_APPROVAL_BASE_URL = 'http://127.0.0.1:8080';
const approvalItems = new Map();
let approvalBaseUrl = DEFAULT_APPROVAL_BASE_URL;
let approvalSocket = null;
let approvalReconnectTimer = null;
let approvalReconnectAttempt = 0;
let approvalConnectionGeneration = 0;
let approvalSoundEnabled = true;

async function startApprovalBridge() {
  const stored = await chrome.storage.local.get(APPROVAL_BASE_URL_KEY);
  const configured = stored[APPROVAL_BASE_URL_KEY];
  if (configured) {
    try { approvalBaseUrl = localOrigin(configured); } catch { /* use default */ }
  }
  connectApprovalSocket();
}

async function configureApprovalBridge(value) {
  if (!value) return;
  const next = localOrigin(value);
  if (next === approvalBaseUrl) return;
  approvalBaseUrl = next;
  await chrome.storage.local.set({ [APPROVAL_BASE_URL_KEY]: next });
  approvalConnectionGeneration += 1;
  if (approvalReconnectTimer) clearTimeout(approvalReconnectTimer);
  approvalReconnectTimer = null;
  approvalSocket?.close();
  approvalSocket = null;
  approvalReconnectAttempt = 0;
  connectApprovalSocket();
}

function connectApprovalSocket() {
  const generation = approvalConnectionGeneration;
  let socket;
  try {
    const url = new URL(approvalBaseUrl);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    url.pathname = '/ws';
    url.search = '';
    url.hash = '';
    socket = new WebSocket(url.href);
  } catch {
    scheduleApprovalReconnect(generation);
    return;
  }
  approvalSocket = socket;
  let heartbeatTimer = null;

  socket.onopen = () => {
    socket.send(JSON.stringify({ type: 'client.ready', client: 'chatgpt-extension' }));
    heartbeatTimer = setInterval(() => {
      if (socket.readyState !== WebSocket.OPEN) return;
      try { socket.send(JSON.stringify({ type: 'client.ping' })); } catch { socket.close(); }
    }, 20_000);
    approvalReconnectAttempt = 0;
    void resyncApprovalQueue();
  };

  socket.onmessage = ({ data }) => {
    if (typeof data !== 'string') {
      socket.close();
      return;
    }
    let event;
    try { event = JSON.parse(data); } catch { socket.close(); return; }
    void handleApprovalEvent(event).catch(() => socket.close());
  };
  socket.onerror = () => socket.close();
  socket.onclose = () => {
    if (heartbeatTimer) clearInterval(heartbeatTimer);
    heartbeatTimer = null;
    if (approvalSocket === socket) approvalSocket = null;
    if (generation === approvalConnectionGeneration) scheduleApprovalReconnect(generation);
  };
}

function scheduleApprovalReconnect(generation) {
  if (generation !== approvalConnectionGeneration || approvalReconnectTimer) return;
  const delayMs = Math.min(30_000, 500 * (2 ** approvalReconnectAttempt++));
  approvalReconnectTimer = setTimeout(() => {
    approvalReconnectTimer = null;
    if (generation === approvalConnectionGeneration) connectApprovalSocket();
  }, delayMs);
}

async function handleApprovalEvent(event) {
  if (!event || typeof event !== 'object') return;
  if (event.type === 'system.resync_required' || event.type === 'system.connected') {
    await resyncApprovalQueue();
    return;
  }
  if (event.type === 'conversation.approval_pending' || event.type === 'approval.pending' || event.type === 'plan.question_pending') {
    await resyncApprovalQueue();
    return;
  }
  if (event.type === 'conversation.approval_resolved' && event.taskId) {
    approvalItems.delete(conversationApprovalKey(event.taskId));
    await broadcastApprovalState();
    return;
  }
  if (event.type === 'approval.resolved' && event.taskId) {
    const activityId = event.payload?.activityId;
    if (activityId) approvalItems.delete(activityApprovalKey(event.taskId, activityId));
    await broadcastApprovalState();
    return;
  }
  if (event.type === 'plan.question_resolved') {
    const questionId = event.payload?.questionId;
    if (questionId) approvalItems.delete(planQuestionKey(questionId));
    await broadcastApprovalState();
  }
}

async function resyncApprovalQueue() {
  try {
    const [conversations, activities, planQuestions] = await Promise.all([
      getJson(approvalBaseUrl, '/api/local/tasks/approvals/pending'),
      getJson(approvalBaseUrl, '/api/local/tasks/activity-approvals/pending'),
      getJson(approvalBaseUrl, '/api/local/plan/questions/pending'),
    ]);
    const next = new Map();
    for (const task of Array.isArray(conversations) ? conversations : []) {
      if (!task?.id || task.allowExecute !== null) continue;
      const item = {
        key: conversationApprovalKey(task.id),
        kind: 'conversation',
        taskId: task.id,
        title: task.title || task.id,
        deadlineUtc: task.approvalDeadlineUtc || null,
        createdAtUtc: task.createdAtUtc || null,
      };
      next.set(item.key, item);
    }
    for (const approval of Array.isArray(activities) ? activities : []) {
      if (!approval?.taskId || !approval?.activityId) continue;
      const item = {
        key: activityApprovalKey(approval.taskId, approval.activityId),
        kind: 'activity',
        taskId: approval.taskId,
        activityId: approval.activityId,
        turnId: approval.turnId || undefined,
        tool: approval.tool || 'tool',
        input: approval.input ?? null,
        deadlineUtc: approval.approvalDeadlineUtc || null,
        createdAtUtc: approval.createdAtUtc || null,
      };
      next.set(item.key, item);
    }
    for (const question of Array.isArray(planQuestions) ? planQuestions : []) {
      if (!question?.id || !question?.taskId || !Array.isArray(question.options) || question.options.length !== 2) continue;
      const item = {
        key: planQuestionKey(question.id),
        kind: 'plan',
        questionId: question.id,
        taskId: question.taskId,
        turnId: question.turnId || undefined,
        question: question.question || '',
        options: question.options,
        createdAtMs: Number(question.createdAtMs) || Date.now(),
        deadlineAtMs: Number(question.deadlineAtMs) || 0,
      };
      next.set(item.key, item);
    }
    approvalItems.clear();
    for (const [key, item] of next) approvalItems.set(key, item);
    await broadcastApprovalState();
  } catch {
    // The local app may be stopped, not authenticated yet, or reconnecting.
  }
}

function configureApprovalSound(enabled) {
  approvalSoundEnabled = enabled !== false;
  void broadcastApprovalState();
}

async function approvalBridgeState() {
  return { items: sortedApprovalItems(), baseUrl: approvalBaseUrl, connected: Boolean(approvalSocket && approvalSocket.readyState === WebSocket.OPEN), soundEnabled: approvalSoundEnabled };
}

async function resolveGlobalApproval(message) {
  const item = message?.item;
  const decision = message?.decision;
  if (!item?.taskId || !item?.kind) throw new Error('Yêu cầu phê duyệt không hợp lệ.');
  if (item.kind === 'conversation') {
    if (!['allow', 'reject'].includes(decision)) throw new Error('Quyết định phê duyệt đoạn trò chuyện không hợp lệ.');
    await postJson(approvalBaseUrl, `/api/local/tasks/${encodeURIComponent(item.taskId)}/${decision === 'allow' ? 'approve-execution' : 'reject-execution'}`, {});
    approvalItems.delete(conversationApprovalKey(item.taskId));
  } else if (item.kind === 'activity') {
    if (!item.activityId || !['allow', 'allowSimilar', 'reject'].includes(decision)) throw new Error('Quyết định phê duyệt lệnh không hợp lệ.');
    try {
      await postJson(approvalBaseUrl, `/api/local/tasks/${encodeURIComponent(item.taskId)}/activities/${encodeURIComponent(item.activityId)}/approval`, {
        turnId: item.turnId || undefined,
        decision,
        reason: typeof message.reason === 'string' && message.reason.trim() ? message.reason.trim() : undefined,
      });
    } catch (error) {
      if (!String(errorMessage(error)).toLowerCase().includes('no longer pending') && !String(errorMessage(error)).toLowerCase().includes('resolved')) throw error;
    }
    approvalItems.delete(activityApprovalKey(item.taskId, item.activityId));
  } else if (item.kind === 'plan') {
    if (!item.questionId) throw new Error('Thiếu question ID của Plan Mode.');
    let body;
    if (message.answerKind === 'option') {
      const optionIndex = Number(message.optionIndex);
      if (optionIndex !== 1 && optionIndex !== 2) throw new Error('Lựa chọn Plan Mode không hợp lệ.');
      body = { kind: 'option', optionIndex };
    } else if (message.answerKind === 'custom') {
      const text = String(message.answerText || '').trim();
      if (!text) throw new Error('Câu trả lời tùy chỉnh không được để trống.');
      body = { kind: 'custom', text };
    } else {
      throw new Error('Kiểu câu trả lời Plan Mode không hợp lệ.');
    }
    await postJson(approvalBaseUrl, `/api/local/plan/questions/${encodeURIComponent(item.questionId)}/answer`, body);
    approvalItems.delete(planQuestionKey(item.questionId));
  } else {
    throw new Error('Loại phê duyệt không được hỗ trợ.');
  }
  await broadcastApprovalState();
  return { resolved: true };
}

async function broadcastApprovalState() {
  const payload = { type: 'chatcmd-global-approval-state', items: sortedApprovalItems(), soundEnabled: approvalSoundEnabled };
  const tabs = await chatGptTabs();
  await Promise.all(tabs.filter((tab) => tab.id).map(async (tab) => {
    try { await sendToChatGpt(tab.id, payload, { quiet: true }); } catch { /* tab can still be loading */ }
  }));
}

function sortedApprovalItems() {
  return [...approvalItems.values()].sort((left, right) => {
    const leftTime = left.kind === 'plan' ? Number(left.createdAtMs) || 0 : Date.parse(left.createdAtUtc || '') || 0;
    const rightTime = right.kind === 'plan' ? Number(right.createdAtMs) || 0 : Date.parse(right.createdAtUtc || '') || 0;
    return leftTime - rightTime || left.key.localeCompare(right.key);
  });
}

function conversationApprovalKey(taskId) { return `conversation:${taskId}`; }
function activityApprovalKey(taskId, activityId) { return `activity:${taskId}:${activityId}`; }
function planQuestionKey(questionId) { return `plan:${questionId}`; }

void startApprovalBridge();
