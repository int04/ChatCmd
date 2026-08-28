import { Bot, CircleAlert, CircleStop, ExternalLink, LoaderCircle, MessageSquarePlus, Send, Unplug } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useNavigate } from 'react-router-dom';

import { api } from '../api';
import { chatGptExtensionAvailable, chatGptExtensionStatus, dispatchChatGptRequest, stopChatGptRequest } from '../chatgptBridge';
import type { Agent, ChatGptBridge } from '../types';
import { useLoad } from '../useLoad';

const DEFAULT_MODEL = 'Auto';
const MODEL_SUGGESTIONS = ['Auto', 'Instant', 'Thinking', 'Pro'];
const SEND_DISABLED_ERROR = 'Nút gửi ChatGPT đang bị vô hiệu hóa.';
const RETRY_DELAY_SECONDS = 10;

export function NewChatGptConversation() {
  const agents = useLoad(api.agents, []);
  const navigate = useNavigate();
  const enabledAgents = useMemo(() => (agents.data ?? []).filter((agent) => agent.enabled), [agents.data]);
  const [agentId, setAgentId] = useState('');
  const [model, setModel] = useState(DEFAULT_MODEL);
  const [content, setContent] = useState('');
  const [extensionReady, setExtensionReady] = useState<boolean | null>(null);
  const [chatGptTabOpen, setChatGptTabOpen] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => { if (!agentId && enabledAgents[0]) setAgentId(enabledAgents[0].id); }, [agentId, enabledAgents]);
  useEffect(() => {
    let disposed = false;
    const refresh = () => void chatGptExtensionStatus().then((status) => {
      if (disposed) return;
      setExtensionReady(status.ready);
      setChatGptTabOpen(status.chatGptTabOpen);
    });
    refresh();
    const timer = window.setInterval(refresh, 2_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, []);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!agentId || !content.trim() || busy) return;
    setBusy(true); setError('');
    try {
      const status = await chatGptExtensionStatus();
      setExtensionReady(status.ready); setChatGptTabOpen(status.chatGptTabOpen);
      if (!status.ready) throw new Error('ChatCMD ChatGPT Bridge extension is not ready. Enable or reload it, then try again.');
      if (!status.chatGptTabOpen) throw new Error('Không có tab ChatGPT nào đang mở. Hãy mở chatgpt.com rồi thử lại.');
      const request = await api.createChatGptRequest({ agentId, model, content: content.trim() });
      await dispatchChatGptRequest({ requestId: request.id, submittedContent: request.submittedContent, model: request.model });
      const taskId = await waitForTaskBinding(request.id);
      navigate(`/tasks/${encodeURIComponent(taskId)}`, { replace: true });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Không thể gửi tin nhắn sang ChatGPT.');
      setBusy(false);
    }
  };

  return <div className="chatgpt-new-shell">
    <section className="chatgpt-new-card">
      <header><span className="chatgpt-logo"><Bot /></span><div><span className="eyebrow">CHATGPT.COM</span><h1>Tin nhắn mới</h1><p>Gửi bằng phiên ChatGPT đã đăng nhập trong Chrome / Edge.</p></div></header>
      <ExtensionState ready={extensionReady} />
      {extensionReady && chatGptTabOpen === false && <p className="chatgpt-input-warning" role="alert"><CircleAlert /><span><strong>Chưa có tab ChatGPT trống để tạo cuộc trò chuyện mới.</strong> Hãy mở một tab chatgpt.com mới và giữ tab đó mở sau khi gửi.</span></p>}
      <form onSubmit={(event) => void submit(event)}>
        <div className="chatgpt-form-grid">
          <label><span>Agent MCP</span><select value={agentId} onChange={(event) => setAgentId(event.target.value)} disabled={busy || agents.loading} required>
            {!enabledAgents.length && <option value="">Chưa có agent đang bật</option>}
            {enabledAgents.map((agent) => <option value={agent.id} key={agent.id}>@{agent.name}</option>)}
          </select></label>
          <label><span>Model ChatGPT</span><input list="chatgpt-model-options" value={model} onChange={(event) => setModel(event.target.value)} disabled={busy} placeholder="Auto" /><datalist id="chatgpt-model-options">{MODEL_SUGGESTIONS.map((value) => <option value={value} key={value} />)}</datalist></label>
        </div>
        <label className="chatgpt-message-field"><span>Nội dung</span><textarea rows={8} value={content} onChange={(event) => setContent(event.target.value)} disabled={busy} placeholder="Nhập yêu cầu cho ChatGPT…" required /></label>
        <div className="chatgpt-prompt-preview"><strong>Tin nhắn thực tế</strong><code>{selectedPrompt(enabledAgents, agentId, content)}</code></div>
        {error && <p className="chatgpt-form-error" role="alert"><CircleAlert />{error}</p>}
        <footer><button className="button primary chatgpt-send-button" type="submit" disabled={busy || !agentId || !content.trim() || extensionReady === false || chatGptTabOpen === false}>{busy ? <LoaderCircle className="spin" /> : <Send />}<span>{busy ? 'Đang gửi sang ChatGPT…' : 'Gửi sang ChatGPT'}</span></button></footer>
      </form>
    </section>
  </div>;
}

export function ChatGptTaskComposer({ taskId }: { taskId: string }) {
  const bridge = useLoad(() => api.chatGptBridge(taskId), [taskId]);
  const [content, setContent] = useState('');
  const [model, setModel] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [extensionReady, setExtensionReady] = useState<boolean | null>(null);
  const [chatGptTabOpen, setChatGptTabOpen] = useState<boolean | null>(null);
  const [retrySeconds, setRetrySeconds] = useState<number | null>(null);
  const retryGeneration = useRef(0);
  const retryTimer = useRef<number | null>(null);
  const active = Boolean(bridge.data?.activeRequestId && ['queued', 'running', 'stop_requested'].includes(bridge.data.activeStatus ?? ''));

  useEffect(() => { if (bridge.data && !model) setModel(bridge.data.model || DEFAULT_MODEL); }, [bridge.data, model]);
  useEffect(() => {
    let disposed = false;
    const refresh = () => {
      const conversationUrl = bridge.data?.conversationUrl;
      if (!conversationUrl) return;
      void chatGptExtensionStatus(conversationUrl).then((status) => {
        if (disposed) return;
        setExtensionReady(status.ready);
        setChatGptTabOpen(status.ready && status.conversationTabOpen);
      });
    };
    refresh();
    const timer = window.setInterval(refresh, 1_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [bridge.data?.conversationUrl]);
  useEffect(() => {
    const requestId = bridge.data?.activeRequestId;
    if (!requestId) return;
    const timer = window.setInterval(() => void api.chatGptRequest(requestId).then((request) => {
      if (['completed', 'stopped', 'failed'].includes(request.status)) void bridge.reload();
    }).catch(() => undefined), 1_000);
    return () => window.clearInterval(timer);
  }, [bridge.data?.activeRequestId, bridge.reload]);
  useEffect(() => () => { if (retryTimer.current !== null) window.clearTimeout(retryTimer.current); }, []);
  useEffect(() => {
    if (retrySeconds === null || retrySeconds <= 0) return;
    const timer = window.setTimeout(() => setRetrySeconds((value) => value === null ? null : Math.max(0, value - 1)), 1_000);
    return () => window.clearTimeout(timer);
  }, [retrySeconds]);

  const sendContent = async (message: string, generation: number) => {
    if (!bridge.data || generation !== retryGeneration.current) return;
    setBusy(true); setError('');
    try {
      const status = await chatGptExtensionStatus(bridge.data.conversationUrl);
      setExtensionReady(status.ready);
      setChatGptTabOpen(status.ready && status.conversationTabOpen);
      if (!status.ready) throw new Error('ChatCMD ChatGPT Bridge extension is not ready. Enable or reload it, then try again.');
      if (!status.conversationTabOpen) throw new Error('Tab ChatGPT của cuộc trò chuyện này không còn mở. Hãy mở lại cuộc trò chuyện ChatGPT rồi thử lại.');
      const request = await api.sendChatGptMessage(taskId, { model: model || bridge.data.model, content: message });
      await dispatchChatGptRequest({ requestId: request.id, submittedContent: request.submittedContent, model: request.model, conversationUrl: bridge.data.conversationUrl });
      const latest = await waitForDispatchState(request.id);
      if (latest.status === 'failed') throw new Error(latest.errorMessage || 'Không thể gửi tin nhắn sang ChatGPT.');
      if (generation !== retryGeneration.current) return;
      setRetrySeconds(null);
      setContent('');
      await bridge.reload();
    } catch (reason) {
      const text = errorText(reason);
      if (generation === retryGeneration.current && isSendDisabledError(text)) {
        setRetrySeconds(RETRY_DELAY_SECONDS);
        setError(`${SEND_DISABLED_ERROR} Sẽ tự thử lại sau ${RETRY_DELAY_SECONDS} giây.`);
        if (retryTimer.current !== null) window.clearTimeout(retryTimer.current);
        retryTimer.current = window.setTimeout(() => {
          retryTimer.current = null;
          if (generation !== retryGeneration.current) return;
          setRetrySeconds(0);
          void sendContent(message, generation);
        }, RETRY_DELAY_SECONDS * 1_000);
      } else {
        setError(text);
      }
    } finally { setBusy(false); }
  };

  const send = async (event: FormEvent) => {
    event.preventDefault();
    const message = content.trim();
    if (!message || busy || active || retrySeconds !== null || !bridge.data) return;
    const generation = retryGeneration.current;
    await sendContent(message, generation);
  };

  const cancelRetry = () => {
    retryGeneration.current += 1;
    if (retryTimer.current !== null) window.clearTimeout(retryTimer.current);
    retryTimer.current = null;
    setRetrySeconds(null);
    setError('Đã hủy tự động gửi lại.');
  };

  const stop = async () => {
    if (!bridge.data?.activeRequestId || busy) return;
    setBusy(true); setError('');
    try {
      if (!await chatGptExtensionAvailable()) throw new Error('ChatCMD ChatGPT Bridge extension is not ready, so the stop command was not sent.');
      const request = await api.stopChatGptMessage(taskId);
      await stopChatGptRequest(request.id);
      await bridge.reload();
    } catch (reason) { setError(errorText(reason)); }
    finally { setBusy(false); }
  };

  if (bridge.loading) return <div className="chatgpt-composer loading"><LoaderCircle className="spin" /><span>Đang tải ChatGPT bridge…</span></div>;
  if (!bridge.data) return <div className="chatgpt-composer error"><CircleAlert /><span>{bridge.error || 'Không có thông tin ChatGPT bridge.'}</span></div>;
  if (extensionReady === false) return <div className="chatgpt-tab-required error" role="alert"><Unplug /><div><strong>Không kết nối được ChatGPT Bridge</strong><span>Hãy bật hoặc reload extension rồi quay lại cuộc trò chuyện này.</span></div></div>;
  if (chatGptTabOpen === false) return <div className="chatgpt-tab-required" role="alert"><CircleAlert /><div><strong>Tab ChatGPT của cuộc trò chuyện này đang đóng</strong><span>ChatCMD cần giữ đúng tab ChatGPT này mở để gửi tin nhắn và theo dõi trạng thái phản hồi. Mở lại cuộc trò chuyện rồi giữ tab đó trong trình duyệt.</span><a href={bridge.data.conversationUrl} target="_blank" rel="noreferrer noopener"><ExternalLink />Mở cuộc trò chuyện ChatGPT</a></div></div>;
  return <form className="chatgpt-composer" onSubmit={(event) => void send(event)}>
    {retrySeconds !== null && <div className="chatgpt-retry-warning" role="status"><CircleAlert /><span>{SEND_DISABLED_ERROR} {retrySeconds > 0 ? `Sẽ thử lại sau ${retrySeconds}s.` : 'Đang thử lại…'}</span><button type="button" onClick={cancelRetry}>Hủy gửi</button></div>}
    <div className="chatgpt-composer-row">
      <textarea aria-label="Tin nhắn tiếp theo cho ChatGPT" rows={2} value={content} onChange={(event) => setContent(event.target.value)} disabled={active || busy || retrySeconds !== null} placeholder={active ? 'ChatGPT đang phản hồi…' : retrySeconds !== null ? 'Đang chờ gửi lại…' : 'Nhắn tiếp cho ChatGPT…'} />
      {active ? <button type="button" className="chatgpt-stop-button" onClick={() => void stop()} disabled={busy || bridge.data.activeStatus === 'stop_requested'}><CircleStop /><span>{bridge.data.activeStatus === 'stop_requested' ? 'Đang dừng…' : 'Dừng'}</span></button>
        : <button type="submit" className="chatgpt-composer-send" disabled={busy || retrySeconds !== null || !content.trim()}><Send /><span>Gửi</span></button>}
    </div>
    <div className="chatgpt-composer-meta"><label>Model <input list="chatgpt-composer-models" value={model} onChange={(event) => setModel(event.target.value)} disabled={active || busy || retrySeconds !== null} /></label><datalist id="chatgpt-composer-models">{MODEL_SUGGESTIONS.map((value) => <option value={value} key={value} />)}</datalist><span>Qua extension · @{bridge.data.conversationId.slice(0, 8)}…</span></div>
    {error && <p className="chatgpt-form-error" role="alert"><CircleAlert />{error}</p>}
  </form>;
}

export function ChatGptTaskCard({ taskId }: { taskId: string }) {
  const bridge = useLoad(() => api.chatGptBridge(taskId), [taskId]);
  if (!bridge.data) return null;
  return <section className="task-info-section chatgpt-task-card"><strong>ChatGPT.com</strong><div><Bot /><span><b>{bridge.data.model}</b><small>{bridge.data.conversationId}</small></span></div><a href={bridge.data.conversationUrl} target="_blank" rel="noreferrer noopener"><ExternalLink />Mở cuộc trò chuyện gốc</a></section>;
}

function ExtensionState({ ready }: { ready: boolean | null }) {
  return <div className={`chatgpt-extension-state ${ready === false ? 'missing' : ready ? 'ready' : ''}`}>{ready === null ? <LoaderCircle className="spin" /> : ready ? <MessageSquarePlus /> : <Unplug />}<div><strong>{ready === null ? 'Đang kiểm tra extension…' : ready ? 'Extension đã sẵn sàng' : 'Chưa kết nối extension'}</strong><span>{ready === false ? 'Cài extension trong folder chatgpt-extension rồi reload trang này.' : 'Không đọc cookie/token; extension thao tác trực tiếp tab chatgpt.com đã đăng nhập.'}</span></div></div>;
}

async function waitForTaskBinding(requestId: string) {
  for (let index = 0; index < 240; index++) {
    const request = await api.chatGptRequest(requestId);
    if (request.taskId) return request.taskId;
    if (request.status === 'failed') throw new Error(request.errorMessage || 'ChatGPT extension không thể tạo cuộc trò chuyện.');
    await new Promise((resolve) => window.setTimeout(resolve, 500));
  }
  throw new Error('Đã gửi sang ChatGPT nhưng chưa nhận được conversation ID. Mở tab ChatGPT để kiểm tra đăng nhập rồi thử lại.');
}

async function waitForDispatchState(requestId: string) {
  for (let index = 0; index < 60; index++) {
    const request = await api.chatGptRequest(requestId);
    if (request.status !== 'queued') return request;
    await new Promise((resolve) => window.setTimeout(resolve, 250));
  }
  return api.chatGptRequest(requestId);
}

function selectedPrompt(agents: Agent[], agentId: string, content: string) {
  const name = agents.find((agent) => agent.id === agentId)?.name || 'agent';
  return `Sử dụng plugin @${name} để thực hiện yêu cầu sau:\n\n${content || '…'}`;
}

function isSendDisabledError(value: string) { return value.includes(SEND_DISABLED_ERROR); }
function errorText(reason: unknown) { return reason instanceof Error ? reason.message : 'Không thể thực hiện yêu cầu ChatGPT.'; }

