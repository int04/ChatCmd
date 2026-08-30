import { Bot, CircleAlert, CircleStop, ExternalLink, LoaderCircle, MessageSquarePlus, Send, Unplug } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useNavigate } from 'react-router-dom';

import { api } from '../api';
import { chatGptExtensionAvailable, chatGptExtensionStatus, closeChatGptConversationTab, dispatchChatGptRequest, focusChatGptConversationTab, stopChatGptRequest } from '../chatgptBridge';
import { tr } from '../i18n';
import type { Agent } from '../types';
import { useLoad } from '../useLoad';

const DEFAULT_MODEL = 'Auto';
const MODEL_SUGGESTIONS = ['Auto', 'Instant', 'Thinking', 'Pro'];
const SEND_DISABLED_ERROR_VI = 'Nút gửi ChatGPT đang bị vô hiệu hóa.';
const SEND_DISABLED_ERROR_EN = 'The ChatGPT send button is disabled.';
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
      if (!status.ready) throw new Error(tr('ChatCMD ChatGPT Bridge extension is not ready. Enable or reload it, then try again.'));
      const request = await api.createChatGptRequest({ agentId, model, content: content.trim() });
      await dispatchChatGptRequest({ requestId: request.id, submittedContent: request.submittedContent, model: request.model });
      const taskId = await waitForTaskBinding(request.id);
      navigate(`/tasks/${encodeURIComponent(taskId)}`, { replace: true });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : tr('Could not send the message to ChatGPT.'));
      setBusy(false);
    }
  };

  return <div className="chatgpt-new-shell">
    <section className="chatgpt-new-card">
      <header><span className="chatgpt-logo"><Bot /></span><div><span className="eyebrow">{tr('CHATGPT.COM')}</span><h1>{tr('New message')}</h1><p>{tr('Send using the signed-in ChatGPT session in Chrome / Edge.')}</p></div></header>
      <ExtensionState ready={extensionReady} />
      {extensionReady && chatGptTabOpen === false && <p className="chatgpt-input-warning" role="status"><CircleAlert /><span><strong>{tr('No blank ChatGPT tab is open for a new conversation.')}</strong> {tr('After you send the message, ChatCMD will automatically open a new ChatGPT tab and continue there.')}</span></p>}
      <button className="button secondary chatgpt-open-log-window" type="button" onClick={() => window.dispatchEvent(new Event('chatcmd:open-extension-logs'))}>{tr('View extension logs')}</button>
      <form onSubmit={(event) => void submit(event)}>
        <div className="chatgpt-form-grid">
          <label><span>{tr('MCP agent')}</span><select value={agentId} onChange={(event) => setAgentId(event.target.value)} disabled={busy || agents.loading} required>
            {!enabledAgents.length && <option value="">{tr('No enabled agent')}</option>}
            {enabledAgents.map((agent) => <option value={agent.id} key={agent.id}>@{agent.name}</option>)}
          </select></label>
          <label><span>{tr('ChatGPT model')}</span><input list="chatgpt-model-options" value={model} onChange={(event) => setModel(event.target.value)} disabled={busy} placeholder="Auto" /><datalist id="chatgpt-model-options">{MODEL_SUGGESTIONS.map((value) => <option value={value} key={value} />)}</datalist></label>
        </div>
        <label className="chatgpt-message-field"><span>{tr('Content')}</span><textarea rows={8} value={content} onChange={(event) => setContent(event.target.value)} disabled={busy} placeholder={tr('Enter a request for ChatGPT…')} required /></label>
        <div className="chatgpt-prompt-preview"><strong>{tr('Actual message')}</strong><code>{selectedPrompt(enabledAgents, agentId, content)}</code></div>
        {error && <p className="chatgpt-form-error" role="alert"><CircleAlert />{error}</p>}
        <footer><button className="button primary chatgpt-send-button" type="submit" disabled={busy || !agentId || !content.trim() || extensionReady === false}>{busy ? <LoaderCircle className="spin" /> : <Send />}<span>{busy ? tr('Sending to ChatGPT…') : tr('Send to ChatGPT')}</span></button></footer>
      </form>
    </section>
  </div>;
}

export function ChatGptTaskComposer({ taskId }: { taskId: string }) {
  const bridge = useLoad(() => api.chatGptBridge(taskId), [taskId]);
  const reloadBridge = bridge.reload;
  const [content, setContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [extensionReady, setExtensionReady] = useState<boolean | null>(null);
  const [chatGptTabOpen, setChatGptTabOpen] = useState<boolean | null>(null);
  const [chatGptReady, setChatGptReady] = useState<boolean | null>(null);
  const [retrySeconds, setRetrySeconds] = useState<number | null>(null);
  const retryGeneration = useRef(0);
  const retryTimer = useRef<number | null>(null);
  const active = Boolean(bridge.data?.activeRequestId && ['queued', 'running', 'stop_requested'].includes(bridge.data.activeStatus ?? ''));

  useEffect(() => {
    let disposed = false;
    const refresh = () => {
      const conversationUrl = bridge.data?.conversationUrl;
      if (!conversationUrl) return;
      void chatGptExtensionStatus(conversationUrl).then((status) => {
        if (disposed) return;
        setExtensionReady(status.ready);
        setChatGptTabOpen(status.ready && status.conversationTabOpen);
        setChatGptReady(status.ready && status.conversationTabOpen && status.conversationReady);
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
      if (['completed', 'stopped', 'failed'].includes(request.status)) void reloadBridge();
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
      setChatGptReady(status.ready && status.conversationTabOpen && status.conversationReady);
      if (!status.ready) throw new Error(tr('ChatCMD ChatGPT Bridge extension is not ready. Enable or reload it, then try again.'));
      if (!status.conversationTabOpen) throw new Error(tr('This conversation’s ChatGPT tab is no longer open. Reopen the ChatGPT conversation and try again.'));
      if (!status.conversationReady) throw new Error(tr('The ChatGPT tab is still loading the previous response. Wait until it is ready before sending another message.'));
      const request = await api.sendChatGptMessage(taskId, { model: DEFAULT_MODEL, content: message });
      await dispatchChatGptRequest({ requestId: request.id, submittedContent: request.submittedContent, model: request.model, conversationUrl: bridge.data.conversationUrl });
      const latest = await waitForDispatchState(request.id);
      if (latest.status === 'failed') throw new Error(latest.errorMessage || tr('Could not send the message to ChatGPT.'));
      if (generation !== retryGeneration.current) return;
      setRetrySeconds(null);
      setContent('');
      await reloadBridge();
    } catch (reason) {
      const text = errorText(reason);
      if (generation === retryGeneration.current && isSendDisabledError(text)) {
        setRetrySeconds(RETRY_DELAY_SECONDS);
        setError(`${tr('The ChatGPT send button is disabled.')} ${tr('Will retry in {seconds} seconds.', { seconds: RETRY_DELAY_SECONDS })}`);
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
    if (!message || busy || active || retrySeconds !== null || chatGptReady !== true || !bridge.data) return;
    const generation = retryGeneration.current;
    await sendContent(message, generation);
  };

  const cancelRetry = () => {
    retryGeneration.current += 1;
    if (retryTimer.current !== null) window.clearTimeout(retryTimer.current);
    retryTimer.current = null;
    setRetrySeconds(null);
    setError(tr('Automatic retry cancelled.'));
  };

  const stop = async () => {
    if (!bridge.data?.activeRequestId || busy) return;
    setBusy(true); setError('');
    try {
      if (!await chatGptExtensionAvailable()) throw new Error(tr('ChatCMD ChatGPT Bridge extension is not ready, so the stop command was not sent.'));
      const request = await api.stopChatGptMessage(taskId);
      await stopChatGptRequest(request.id);
      await reloadBridge();
    } catch (reason) { setError(errorText(reason)); }
    finally { setBusy(false); }
  };

  const focusTab = async () => {
    if (!bridge.data || busy) return;
    setError('');
    try { await focusChatGptConversationTab(bridge.data.conversationUrl); }
    catch (reason) { setError(errorText(reason)); }
  };

  const closeTab = async () => {
    if (!bridge.data || busy) return;
    setError('');
    try {
      await closeChatGptConversationTab(bridge.data.conversationUrl);
      setChatGptTabOpen(false);
      setChatGptReady(false);
    } catch (reason) { setError(errorText(reason)); }
  };

  if (bridge.loading) return <div className="chatgpt-composer loading"><LoaderCircle className="spin" /><span>{tr('Loading ChatGPT bridge…')}</span></div>;
  if (!bridge.data) return <div className="chatgpt-composer error"><CircleAlert /><span>{bridge.error || tr('ChatGPT bridge information is unavailable.')}</span></div>;
  if (extensionReady === false) return <div className="chatgpt-tab-required error" role="alert"><Unplug /><div><strong>{tr('Could not connect to ChatGPT Bridge')}</strong><span>{tr('Enable or reload the extension, then return to this conversation.')}</span></div></div>;
  if (chatGptTabOpen === false) return <div className="chatgpt-tab-required" role="alert"><CircleAlert /><div><strong>{tr('This conversation’s ChatGPT tab is closed')}</strong><span>{tr('ChatCMD must keep this exact ChatGPT tab open to send messages and track response status. Reopen the conversation and keep the tab open in your browser.')}</span><a href={bridge.data.conversationUrl} target="_blank" rel="noreferrer noopener"><ExternalLink />{tr('Open ChatGPT conversation')}</a></div></div>;
  return <form className="chatgpt-composer" onSubmit={(event) => void send(event)}>
    {!active && chatGptReady !== true && <div className="chatgpt-retry-warning" role="status"><LoaderCircle className="spin" /><span><strong>{tr('ChatGPT tab is still loading the previous response.')}</strong> {tr('Please wait until ChatGPT is fully ready before sending another message.')}</span></div>}
    {retrySeconds !== null && <div className="chatgpt-retry-warning" role="status"><CircleAlert /><span>{tr('The ChatGPT send button is disabled.')} {retrySeconds > 0 ? tr('Will retry in {seconds} seconds.', { seconds: retrySeconds }) : tr('Retrying…')}</span><button type="button" onClick={cancelRetry}>{tr('Cancel send')}</button></div>}
    <div className="chatgpt-composer-row">
      <textarea aria-label={tr('Next message to ChatGPT')} rows={2} value={content} onChange={(event) => setContent(event.target.value)} disabled={active || busy || retrySeconds !== null} placeholder={active ? tr('ChatGPT is responding…') : retrySeconds !== null ? tr('Waiting to retry…') : tr('Continue the ChatGPT conversation…')} />
      {active ? <button type="button" className="chatgpt-stop-button" onClick={() => void stop()} disabled={busy || bridge.data.activeStatus === 'stop_requested'}><CircleStop /><span>{bridge.data.activeStatus === 'stop_requested' ? tr('Stopping…') : tr('Stop')}</span></button>
        : <button type="submit" className="chatgpt-composer-send" disabled={busy || retrySeconds !== null || chatGptReady !== true || !content.trim()}><Send /><span>{chatGptReady === true ? tr('Send') : tr('Waiting for ChatGPT…')}</span></button>}
    </div>
    <div className="chatgpt-composer-meta chatgpt-composer-actions"><button type="button" onClick={() => void closeTab()} disabled={busy}>{tr('Close this tab')}</button><span aria-hidden="true">|</span><button type="button" onClick={() => void focusTab()} disabled={busy}>{tr('Change model')}</button></div>
    {error && <p className="chatgpt-form-error" role="alert"><CircleAlert />{error}</p>}
  </form>;
}

export function ChatGptTaskCard({ taskId }: { taskId: string }) {
  const bridge = useLoad(() => api.chatGptBridge(taskId), [taskId]);
  if (!bridge.data) return null;
  return <section className="task-info-section chatgpt-task-card"><strong>ChatGPT.com</strong><div><Bot /><span><b>{bridge.data.model}</b><small>{bridge.data.conversationId}</small></span></div><a href={bridge.data.conversationUrl} target="_blank" rel="noreferrer noopener"><ExternalLink />{tr('Open original conversation')}</a></section>;
}

function ExtensionState({ ready }: { ready: boolean | null }) {
  return <div className={`chatgpt-extension-state ${ready === false ? 'missing' : ready ? 'ready' : ''}`}>{ready === null ? <LoaderCircle className="spin" /> : ready ? <MessageSquarePlus /> : <Unplug />}<div><strong>{ready === null ? tr('Checking extension…') : ready ? tr('Extension ready') : tr('Extension not connected')}</strong><span>{ready === false ? tr('Install the extension from the chatgpt-extension folder, then reload this page.') : tr('Does not read cookies/tokens; the extension operates directly on the signed-in chatgpt.com tab.')}</span></div></div>;
}

async function waitForTaskBinding(requestId: string) {
  for (let index = 0; index < 240; index++) {
    const request = await api.chatGptRequest(requestId);
    if (request.taskId) return request.taskId;
    if (request.status === 'failed') throw new Error(request.errorMessage || tr('ChatGPT extension could not create the conversation.'));
    await new Promise((resolve) => window.setTimeout(resolve, 500));
  }
  throw new Error(tr('Sent to ChatGPT but no conversation ID was received. Open the ChatGPT tab to verify sign-in and try again.'));
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
  return tr('Use plugin @{name} to perform the following request:\n\n{content}', { name, content: content || '…' });
}

function isSendDisabledError(value: string) { return value.includes(SEND_DISABLED_ERROR_VI) || value.includes(SEND_DISABLED_ERROR_EN); }
function errorText(reason: unknown) { return reason instanceof Error ? reason.message : tr('Could not complete the ChatGPT request.'); }
