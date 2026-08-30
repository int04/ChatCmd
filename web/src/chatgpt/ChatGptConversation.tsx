import { Bot, CircleAlert, CircleStop, ExternalLink, FolderOpen, LoaderCircle, MessageSquarePlus, Send, ShieldCheck, Sparkles, Unplug, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';

import { api } from '../api';
import { chatGptExtensionAvailable, chatGptExtensionStatus, closeChatGptConversationTab, dispatchChatGptRequest, focusChatGptConversationTab, openChatGptConversationTab, prepareChatGptModelTab, stopChatGptRequest } from '../chatgptBridge';
import { Modal } from '../components';
import { tr } from '../i18n';
import { canonicalProjectPath } from '../tasks/workspaceProjects';
import type { Agent } from '../types';
import { useLoad } from '../useLoad';

const DEFAULT_MODEL = 'Auto';
const SEND_DISABLED_ERROR_VI = 'Nút gửi ChatGPT đang bị vô hiệu hóa.';
const SEND_DISABLED_ERROR_EN = 'The ChatGPT send button is disabled.';
const RETRY_DELAY_SECONDS = 10;

export function NewChatGptConversation() {
  const agents = useLoad(api.agents, []);
  const projects = useLoad(api.workspaceProjects, []);
  const location = useLocation();
  const navigate = useNavigate();
  const launchProjectFolder = routeProjectFolder(location.state);
  const enabledAgents = useMemo(() => (agents.data ?? []).filter((agent) => agent.enabled), [agents.data]);
  const [agentId, setAgentId] = useState('');
  const [projectFolder, setProjectFolder] = useState(launchProjectFolder);
  const [folderMenuOpen, setFolderMenuOpen] = useState(false);
  const [content, setContent] = useState('');
  const [folderPicking, setFolderPicking] = useState(false);
  const [modelTabOpening, setModelTabOpening] = useState(false);
  const [confirmWithoutFolder, setConfirmWithoutFolder] = useState(false);
  const [extensionReady, setExtensionReady] = useState<boolean | null>(null);
  const [chatGptTabOpen, setChatGptTabOpen] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    if (!agentId && enabledAgents[0]) {
      setAgentId(enabledAgents[0].id);
    }
  }, [agentId, enabledAgents]);
  useEffect(() => {
    if (!launchProjectFolder) return;
    setProjectFolder(launchProjectFolder);
  }, [launchProjectFolder]);
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

  const setProjectFolderFromUser = (path: string) => {
    setProjectFolder(path);
  };

  const selectAgent = (nextAgentId: string) => {
    setAgentId(nextAgentId);
  };

  const pickFolder = async () => {
    if (busy || folderPicking) return;
    setFolderPicking(true); setError('');
    try {
      const result = await api.pickProjectFolder();
      if (result.path) {
        setProjectFolderFromUser(result.path);
        setFolderMenuOpen(false);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Không thể mở trình chọn thư mục.');
    } finally { setFolderPicking(false); }
  };

  const chooseModel = async () => {
    if (busy || modelTabOpening) return;
    setModelTabOpening(true); setError('');
    try {
      await prepareChatGptModelTab();
      setExtensionReady(true);
      setChatGptTabOpen(true);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : tr('Could not open ChatGPT to choose a model.'));
    } finally { setModelTabOpening(false); }
  };

  const sendNewConversation = async (allowWithoutFolder: boolean) => {
    if (!agentId || !content.trim() || busy) return;
    if (!projectFolder.trim() && !allowWithoutFolder) {
      setConfirmWithoutFolder(true);
      return;
    }
    setConfirmWithoutFolder(false);
    setBusy(true); setError('');
    try {
      const status = await chatGptExtensionStatus();
      setExtensionReady(status.ready); setChatGptTabOpen(status.chatGptTabOpen);
      if (!status.ready) throw new Error(tr('ChatCMD ChatGPT Bridge extension is not ready. Enable or reload it, then try again.'));
      const request = await api.createChatGptRequest({ agentId, model: DEFAULT_MODEL, projectFolder: projectFolder.trim(), content: content.trim() });
      await dispatchChatGptRequest({ requestId: request.id, submittedContent: request.submittedContent, model: request.model });
      const taskId = await waitForTaskBinding(request.id);
      navigate(`/tasks/${encodeURIComponent(taskId)}`, { replace: true });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : tr('Could not send the message to ChatGPT.'));
      setBusy(false);
    }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void sendNewConversation(false);
  };

  const selectedAgent = enabledAgents.find((agent) => agent.id === agentId);

  return <div className="chatgpt-new-shell">
    <section className="chatgpt-new-card chatgpt-chat-window">
      <header className="chatgpt-chat-topbar">
        <div className="chatgpt-chat-identity">
          <span className="chatgpt-logo"><Bot /></span>
          <div><strong>ChatGPT</strong><small>{tr('Send using the signed-in ChatGPT session in Chrome / Edge.')}</small></div>
        </div>
        <div className="chatgpt-chat-controls">
          <span className={`chatgpt-connection-dot ${extensionReady === false ? 'missing' : extensionReady ? 'ready' : ''}`} title={extensionReady === null ? tr('Checking extension…') : extensionReady ? tr('Extension ready') : tr('Extension not connected')}>{extensionReady === null ? <LoaderCircle className="spin" /> : extensionReady ? <MessageSquarePlus /> : <Unplug />}</span>
          <button className="chatgpt-log-link" type="button" onClick={() => window.dispatchEvent(new Event('chatcmd:open-extension-logs'))}>{tr('View extension logs')}</button>
        </div>
      </header>

      {extensionReady && chatGptTabOpen === false && <div className="chatgpt-chat-notice" role="status"><CircleAlert /><span><strong>{tr('No blank ChatGPT tab is open for a new conversation.')}</strong> {tr('After you send the message, ChatCMD will automatically open a new ChatGPT tab and continue there.')}</span></div>}

      <div className="chatgpt-chat-thread" aria-live="polite">
        <div className="chatgpt-ai-message">
          <span className="chatgpt-message-avatar"><Bot /></span>
          <div className="chatgpt-message-copy"><strong>ChatGPT</strong><p>{selectedAgent ? `Bạn muốn mình giao công việc gì cho @${selectedAgent.name}?` : 'Chọn một MCP agent để bắt đầu cuộc trò chuyện.'}</p><small>Yêu cầu của bạn sẽ được gửi qua ChatGPT và agent sẽ thực hiện công việc trong ChatCMD.</small></div>
        </div>
        {content.trim() && <div className="chatgpt-user-message"><div>{content}</div></div>}
      </div>

      <form className="chatgpt-chat-composer" onSubmit={(event) => void submit(event)}>
        {error && <p className="chatgpt-form-error" role="alert"><CircleAlert />{error}</p>}
        <div className="chatgpt-composer-context">
          <label className="chatgpt-agent-picker chatgpt-composer-agent"><span>{tr('MCP agent')}</span><select value={agentId} onChange={(event) => selectAgent(event.target.value)} disabled={busy || agents.loading} required>
            {!enabledAgents.length && <option value="">{tr('No enabled agent')}</option>}
            {enabledAgents.map((agent) => <option value={agent.id} key={agent.id}>@{agent.name}</option>)}
          </select></label>
          <div className="chatgpt-folder-picker">
            <span>Thư mục dự án</span>
            <div className="chatgpt-folder-picker-control">
              <button className={`chatgpt-folder-select ${projectFolder ? '' : 'empty'}`} type="button" onClick={() => { setFolderMenuOpen(true); void projects.reload(); }} disabled={busy} title={projectFolder || 'Chọn thư mục dự án'}>
                <FolderOpen /><span>{projectFolder || 'Chọn thư mục'}</span>
              </button>
              {projectFolder && <button className="chatgpt-folder-clear" type="button" onClick={() => setProjectFolderFromUser('')} disabled={busy} aria-label="Bỏ chọn thư mục"><X /></button>}
            </div>
          </div>
          <div className="chatgpt-model-picker">
            <span>{tr('Model')}</span>
            <div className="chatgpt-model-picker-row">
              <button className="chatgpt-model-select" type="button" onClick={() => void chooseModel()} disabled={busy || modelTabOpening}>
                {modelTabOpening ? <LoaderCircle className="spin" /> : <Sparkles />}<span>{tr('Choose model')}</span><ExternalLink />
              </button>
              <small>{tr('Stronger models can take longer to complete the request.')}</small>
            </div>
          </div>
        </div>
        <div className="chatgpt-chat-input-wrap">
          <textarea rows={3} value={content} onChange={(event) => setContent(event.target.value)} disabled={busy} placeholder={tr('Enter a request for ChatGPT…')} required />
          <button className="chatgpt-chat-send" type="submit" aria-label={tr('Send to ChatGPT')} disabled={busy || !agentId || !content.trim() || extensionReady === false}>{busy ? <LoaderCircle className="spin" /> : <Send />}</button>
        </div>
        <div className="chatgpt-chat-composer-meta"><span>{selectedAgent ? `Gửi tới @${selectedAgent.name}` : tr('No enabled agent')}</span><span><ShieldCheck />{tr('Actual message')}: <code>{selectedPrompt(enabledAgents, agentId, projectFolder, content)}</code></span></div>
      </form>
    </section>
    {folderMenuOpen && <Modal className="workspace-folder-modal" title="Chọn thư mục dự án" description="Chọn một dự án đã lưu hoặc mở trình chọn folder trên máy." close={() => !folderPicking && setFolderMenuOpen(false)}><div className="workspace-folder-choices"><div className="workspace-folder-project-list">{projects.loading ? <p className="workspace-folder-empty"><LoaderCircle className="spin" /> Đang tải dự án…</p> : projects.data?.length ? projects.data.map((project) => <button className={`workspace-folder-project ${canonicalProjectPath(projectFolder) === canonicalProjectPath(project.path) ? 'selected' : ''}`} type="button" onClick={() => { setProjectFolderFromUser(project.path); setFolderMenuOpen(false); }} key={project.id}><strong>{project.name}</strong><small>{project.path}</small></button>) : <p className="workspace-folder-empty">{projects.error || 'Chưa có dự án đã lưu.'}</p>}</div><button className="workspace-folder-browse" type="button" onClick={() => void pickFolder()} disabled={folderPicking}>{folderPicking ? <LoaderCircle className="spin" /> : <FolderOpen />}<span><strong>Chọn folder</strong><small>Mở trình chọn thư mục trên máy</small></span></button></div></Modal>}
    {confirmWithoutFolder && <div className="modal-backdrop chatgpt-folder-warning-backdrop">
      <div className="modal chatgpt-folder-warning" role="alertdialog" aria-modal="true" aria-labelledby="chatgpt-folder-warning-title">
        <span className="chatgpt-folder-warning-icon"><CircleAlert /></span>
        <div><h2 id="chatgpt-folder-warning-title">Bạn chưa chọn thư mục</h2><p>Chọn thư mục dự án cụ thể giúp AI làm việc tốt hơn trên môi trường đó, bạn có muốn vẫn tiếp tục mà không có thư mục không?</p></div>
        <div className="modal-actions"><button className="button secondary" type="button" onClick={() => setConfirmWithoutFolder(false)}>Hủy</button><button className="button primary" type="button" onClick={() => void sendNewConversation(true)}>Tiếp tục mà không cần thư mục</button></div>
      </div>
    </div>}
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
  }, [bridge.data?.activeRequestId, reloadBridge]);
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

  const openTab = async () => {
    if (!bridge.data || busy) return;
    setError('');
    try {
      await openChatGptConversationTab(bridge.data.conversationUrl);
      setChatGptTabOpen(true);
      setChatGptReady(false);
    } catch (reason) { setError(errorText(reason)); }
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
  if (chatGptTabOpen === false) return <div className="chatgpt-tab-required" role="alert"><CircleAlert /><div><strong>{tr('This conversation’s ChatGPT tab is closed')}</strong><span>{tr('ChatCMD must keep this exact ChatGPT tab open to send messages and track response status. Reopen the conversation and keep the tab open in your browser.')}</span><button type="button" onClick={() => void openTab()} disabled={busy}><ExternalLink />{tr('Open ChatGPT conversation')}</button></div></div>;
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

function selectedPrompt(agents: Agent[], agentId: string, projectFolder: string, content: string) {
  const name = agents.find((agent) => agent.id === agentId)?.name || 'agent';
  const folder = projectFolder.trim();
  return folder
    ? `Sử dụng plugin @${name}\n\nThư mục dự án: ${folder}\n\nđể thực hiện yêu cầu sau: ${content || '…'}`
    : `Sử dụng plugin @${name} để thực hiện yêu cầu sau:\n\n${content || '…'}`;
}

function routeProjectFolder(state: unknown) {
  if (!state || typeof state !== 'object' || Array.isArray(state)) return '';
  const value = (state as Record<string, unknown>).projectFolder;
  return typeof value === 'string' ? value.trim() : '';
}

function isSendDisabledError(value: string) { return value.includes(SEND_DISABLED_ERROR_VI) || value.includes(SEND_DISABLED_ERROR_EN); }
function errorText(reason: unknown) { return reason instanceof Error ? reason.message : tr('Could not complete the ChatGPT request.'); }
