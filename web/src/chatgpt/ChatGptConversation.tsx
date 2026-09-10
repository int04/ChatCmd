import { Bot, CircleAlert, CircleStop, ExternalLink, FolderOpen, LoaderCircle, MessageSquarePlus, Send, ShieldCheck, Sparkles, Unplug, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { ClipboardEvent, FormEvent } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';

import { api } from '../api';
import { chatGptExtensionAvailable, chatGptExtensionStatus, closeChatGptConversationTab, dispatchChatGptRequest, focusChatGptConversationTab, openChatGptConversationTab, prepareChatGptModelTab, reconcileChatGptRequest, recoverChatGptIdentity, stopChatGptRequest } from '../chatgptBridge';
import { Modal } from '../components';
import { tr } from '../i18n';
import { canonicalProjectPath } from '../tasks/workspaceProjects';
import type { Agent } from '../types';
import { orderAgentsByRecentUse, rememberAgentUse } from './agentRecency';
import { useLoad } from '../useLoad';
import { ComposerFileInput } from './ComposerFileInput';
import { ChatGptAttachmentPreview } from './ChatGptAttachmentPreview';
export { ChatGptTaskComposer } from './ChatGptTaskComposer';
import { useCompactBridgeSync } from './compact/useCompactBridgeSync';
import { fileAttachmentPayloads, messageContentWithTextAttachments, textAttachmentFromPaste, type ChatGptTextAttachment } from './pasteAttachments';

const DEFAULT_MODEL = 'Auto';

export function NewChatGptConversation() {
  const agents = useLoad(api.agents, []);
  const projects = useLoad(api.workspaceProjects, []);
  const location = useLocation();
  const navigate = useNavigate();
  const launchProjectFolder = routeProjectFolder(location.state);
  const launchProjectChatGptUrl = routeProjectChatGptUrl(location.state);
  const preferRecentAgents = routeAgentOrder(location.state) === 'recent';
  const enabledAgents = useMemo(() => {
    const enabled = (agents.data ?? []).filter((agent) => agent.enabled);
    return preferRecentAgents ? orderAgentsByRecentUse(enabled) : enabled;
  }, [agents.data, preferRecentAgents]);
  const [agentId, setAgentId] = useState('');
  const agentLaunchKey = useRef(location.key);
  const [projectFolder, setProjectFolder] = useState(launchProjectFolder);
  const [folderMenuOpen, setFolderMenuOpen] = useState(false);
  const [content, setContent] = useState('');
  const [textAttachments, setTextAttachments] = useState<ChatGptTextAttachment[]>([]);
  const pasteSequence = useRef(0);
  const [folderPicking, setFolderPicking] = useState(false);
  const [modelTabOpening, setModelTabOpening] = useState(false);
  const [confirmWithoutFolder, setConfirmWithoutFolder] = useState(false);
  const [extensionReady, setExtensionReady] = useState<boolean | null>(null);
  const [chatGptTabOpen, setChatGptTabOpen] = useState<boolean | null>(null);
  const selectedProject = useMemo(() => (projects.data ?? []).find((project) => canonicalProjectPath(project.path) === canonicalProjectPath(projectFolder)), [projectFolder, projects.data]);
  const newConversationUrl = selectedProject?.chatGptProjectUrl?.trim() || (canonicalProjectPath(projectFolder) === canonicalProjectPath(launchProjectFolder) ? launchProjectChatGptUrl : '') || undefined;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    const routeChanged = agentLaunchKey.current !== location.key;
    if (routeChanged) agentLaunchKey.current = location.key;
    const currentAvailable = enabledAgents.some((agent) => agent.id === agentId);
    if ((routeChanged || !currentAvailable) && enabledAgents[0]) setAgentId(enabledAgents[0].id);
    if (!enabledAgents.length && agentId) setAgentId('');
  }, [agentId, enabledAgents, location.key]);
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
      setError(reason instanceof Error ? reason.message : tr('Could not open the folder picker.'));
    } finally { setFolderPicking(false); }
  };

  const chooseModel = async () => {
    if (busy || modelTabOpening) return;
    setModelTabOpening(true); setError('');
    try {
      await prepareChatGptModelTab(newConversationUrl);
      setExtensionReady(true);
      setChatGptTabOpen(true);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : tr('Could not open ChatGPT to choose a model.'));
    } finally { setModelTabOpening(false); }
  };

  const handlePaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const text = event.clipboardData.getData('text/plain');
    const attachment = textAttachmentFromPaste(text, pasteSequence.current + 1);
    if (!attachment) return;
    event.preventDefault();
    pasteSequence.current += 1;
    setTextAttachments((current) => [...current, attachment]);
  };
  const effectiveContent = messageContentWithTextAttachments(content, textAttachments);

  const sendNewConversation = async (allowWithoutFolder: boolean) => {
    if (!agentId || !effectiveContent || busy) return;
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
      const request = await api.createChatGptRequest({ agentId, model: DEFAULT_MODEL, projectFolder: projectFolder.trim(), content: effectiveContent });
      rememberAgentUse(agentId);
      await dispatchChatGptRequest({ requestId: request.id, submittedContent: request.submittedContent, model: request.model, newConversationUrl, attachments: fileAttachmentPayloads(textAttachments) });
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
          <div className="chatgpt-message-copy"><strong>ChatGPT</strong><p>{selectedAgent ? tr('What would you like me to assign to @{name}?', { name: selectedAgent.name }) : tr('Choose an MCP agent to start the conversation.')}</p><small>{tr('Your request will be sent through ChatGPT and the agent will perform the work in ChatCMD.')}</small></div>
        </div>
        {(content.trim() || textAttachments.length > 0) && <div className="chatgpt-user-message"><div>{content.trim() ? content : effectiveContent}</div></div>}
      </div>

      <form className="chatgpt-chat-composer" onSubmit={(event) => void submit(event)}>
        {error && <p className="chatgpt-form-error" role="alert"><CircleAlert />{error}</p>}
        <div className="chatgpt-composer-context">
          <label className="chatgpt-agent-picker chatgpt-composer-agent"><span>{tr('MCP agent')}</span><select value={agentId} onChange={(event) => selectAgent(event.target.value)} disabled={busy || agents.loading} required>
            {!enabledAgents.length && <option value="">{tr('No enabled agent')}</option>}
            {enabledAgents.map((agent) => <option value={agent.id} key={agent.id}>@{agent.name}</option>)}
          </select></label>
          <div className="chatgpt-folder-picker">
            <span>{tr('Project folder')}</span>
            <div className="chatgpt-folder-picker-control">
              <button className={`chatgpt-folder-select ${projectFolder ? '' : 'empty'}`} type="button" onClick={() => { setFolderMenuOpen(true); void projects.reload(); }} disabled={busy} title={projectFolder || tr('Choose project folder')}>
                <FolderOpen /><span>{projectFolder || tr('Choose folder')}</span>
              </button>
              {projectFolder && <button className="chatgpt-folder-clear" type="button" onClick={() => setProjectFolderFromUser('')} disabled={busy} aria-label={tr('Clear folder selection')}><X /></button>}
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
        {textAttachments.length > 0 && <div className="chatgpt-message-attachments" aria-label={tr('Attachments for the next message')}>
          {textAttachments.map((attachment) => <ChatGptAttachmentPreview key={attachment.id} attachment={attachment} onRemove={() => setTextAttachments((current) => current.filter((item) => item.id !== attachment.id))} />)}
        </div>}
        <ComposerFileInput
          value={content}
          setValue={setContent}
          attachments={textAttachments}
          setAttachments={setTextAttachments}
          onPaste={handlePaste}
          onError={setError}
          disabled={busy}
          placeholder={tr('Enter a request for ChatGPT…')}
          rows={3}
          variant="new"
          endAction={<button className="chatgpt-chat-send" type="submit" aria-label={tr('Send to ChatGPT')} disabled={busy || !agentId || !effectiveContent || extensionReady === false}>{busy ? <LoaderCircle className="spin" /> : <Send />}</button>}
        />
        <div className="chatgpt-chat-composer-meta"><span>{selectedAgent ? tr('Send to @{name}', { name: selectedAgent.name }) : tr('No enabled agent')}</span><span><ShieldCheck />{tr('Actual message')}: <code>{selectedPrompt(enabledAgents, agentId, projectFolder, effectiveContent)}</code></span></div>
      </form>
    </section>
    {folderMenuOpen && <Modal className="workspace-folder-modal" title={tr('Choose project folder')} description={tr('Choose a saved project or open the folder picker on this computer.')} close={() => !folderPicking && setFolderMenuOpen(false)}><div className="workspace-folder-choices"><div className="workspace-folder-project-list">{projects.loading ? <p className="workspace-folder-empty"><LoaderCircle className="spin" /> {tr('Loading projects…')}</p> : projects.data?.length ? projects.data.map((project) => <button className={`workspace-folder-project ${canonicalProjectPath(projectFolder) === canonicalProjectPath(project.path) ? 'selected' : ''}`} type="button" onClick={() => { setProjectFolderFromUser(project.path); setFolderMenuOpen(false); }} key={project.id}><strong>{project.name}</strong><small>{project.path}</small></button>) : <p className="workspace-folder-empty">{projects.error || tr('No saved projects yet.')}</p>}</div><button className="workspace-folder-browse" type="button" onClick={() => void pickFolder()} disabled={folderPicking}>{folderPicking ? <LoaderCircle className="spin" /> : <FolderOpen />}<span><strong>{tr('Choose folder')}</strong><small>{tr('Open the folder picker on this computer')}</small></span></button></div></Modal>}
    {confirmWithoutFolder && <div className="modal-backdrop chatgpt-folder-warning-backdrop">
      <div className="modal chatgpt-folder-warning" role="alertdialog" aria-modal="true" aria-labelledby="chatgpt-folder-warning-title">
        <span className="chatgpt-folder-warning-icon"><CircleAlert /></span>
        <div><h2 id="chatgpt-folder-warning-title">{tr('No folder selected')}</h2><p>{tr('Choosing a specific project folder helps the AI work better in that environment. Do you still want to continue without a folder?')}</p></div>
        <div className="modal-actions"><button className="button secondary" type="button" onClick={() => setConfirmWithoutFolder(false)}>{tr('Cancel')}</button><button className="button primary" type="button" onClick={() => void sendNewConversation(true)}>{tr('Continue without a folder')}</button></div>
      </div>
    </div>}
  </div>;
}

export function ChatGptTaskCard({ taskId }: { taskId: string }) {
  const bridge = useLoad(() => api.chatGptBridge(taskId), [taskId]);
  const refreshBridge = bridge.refresh;
  useCompactBridgeSync(bridge.data, refreshBridge);
  useEffect(() => {
    if (bridge.data?.conversationUrl) return;
    const timer = window.setInterval(() => void refreshBridge(), 2_000);
    return () => window.clearInterval(timer);
  }, [bridge.data?.conversationUrl, refreshBridge]);
  if (!bridge.data) return null;
  return <section className="task-info-section chatgpt-task-card"><strong>ChatGPT.com</strong><div><Bot /><span><b>{bridge.data.model}</b><small>{bridge.data.conversationId || tr('Syncing conversation ID…')}</small></span></div>{bridge.data.conversationUrl && <a href={bridge.data.conversationUrl} target="_blank" rel="noreferrer noopener"><ExternalLink />{tr('Open original conversation')}</a>}</section>;
}

function ExtensionState({ ready }: { ready: boolean | null }) {
  return <div className={`chatgpt-extension-state ${ready === false ? 'missing' : ready ? 'ready' : ''}`}>{ready === null ? <LoaderCircle className="spin" /> : ready ? <MessageSquarePlus /> : <Unplug />}<div><strong>{ready === null ? tr('Checking extension…') : ready ? tr('Extension ready') : tr('Extension not connected')}</strong><span>{ready === false ? tr('Install the extension from the chatgpt-extension folder, then reload this page.') : tr('Does not read cookies/tokens; the extension operates directly on the signed-in chatgpt.com tab.')}</span></div></div>;
}

async function waitForTaskBinding(requestId: string) {
  for (let index = 0; index < 240; index++) {
    const request = await api.chatGptRequest(requestId);
    if (request.status === 'failed') throw new Error(request.errorMessage || tr('ChatGPT extension could not create the conversation.'));
    if (request.taskId) return request.taskId;
    await new Promise((resolve) => window.setTimeout(resolve, 500));
  }
  throw new Error(tr('Sent to ChatGPT but no conversation ID was received. Open the ChatGPT tab to verify sign-in and try again.'));
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
function routeProjectChatGptUrl(state: unknown) {
  if (!state || typeof state !== 'object' || Array.isArray(state)) return '';
  const value = (state as Record<string, unknown>).chatGptProjectUrl;
  return typeof value === 'string' ? value.trim() : '';
}
function routeAgentOrder(state: unknown) {
  if (!state || typeof state !== 'object' || Array.isArray(state)) return '';
  return (state as Record<string, unknown>).agentOrder === 'recent' ? 'recent' : '';
}

function errorText(reason: unknown) { return reason instanceof Error ? reason.message : tr('Could not complete the ChatGPT request.'); }
