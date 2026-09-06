import { CircleAlert, CircleStop, ExternalLink, LoaderCircle, Send, Unplug } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent } from 'react';
import { api } from '../api';
import { chatGptExtensionAvailable, chatGptExtensionStatus, closeChatGptConversationTab, dispatchChatGptRequest, focusChatGptConversationTab, openChatGptConversationTab, reconcileChatGptRequest, recoverChatGptIdentity, stopChatGptRequest } from '../chatgptBridge';
import { tr } from '../i18n';
import { useLoad } from '../useLoad';
import { ChatGptMessageQueuePanel, type ChatGptQueueMode } from './ChatGptMessageQueue';
import { CompactAction, CompactStatusCard } from './compact/CompactControls';
import { useCompact } from './compact/CompactProvider';
import { useCompactBridgeSync } from './compact/useCompactBridgeSync';
import { compactText } from './compact/copy';
const DEFAULT_MODEL = 'Auto';

export function ChatGptTaskComposer({ taskId }: { taskId: string }) {
  const bridge = useLoad(() => api.chatGptBridge(taskId), [taskId]);
  const reloadBridge = bridge.refresh;
  const compact = useCompact();
  const bridgeSync = useCompactBridgeSync(bridge.data, bridge.refresh);
  const compactPaused = compact?.blocked ?? false;
  const [identitySyncError, setIdentitySyncError] = useState<{ taskId: string; message: string } | null>(null);
  const syncError = identitySyncError?.taskId === taskId ? identitySyncError.message : '';
  const [content, setContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [extensionReady, setExtensionReady] = useState<boolean | null>(null);
  const [chatGptTabOpen, setChatGptTabOpen] = useState<boolean | null>(null);
  const [chatGptReady, setChatGptReady] = useState<boolean | null>(null);
  const [queueMode, setQueueMode] = useState<ChatGptQueueMode | null>(null);
  const previousExtensionReady = useRef<boolean | null>(null);
  const resumeCompact = compact?.resume;
  useEffect(() => {
    if (previousExtensionReady.current === false && extensionReady === true) void resumeCompact?.();
    previousExtensionReady.current = extensionReady;
  }, [extensionReady, resumeCompact]);
  const conversationUrl = bridge.data?.conversationUrl || undefined;
  const active = Boolean(bridge.data?.activeRequestId && ['queued', 'running', 'stop_requested'].includes(bridge.data.activeStatus ?? ''));
  const answerCompletedWaitingForUi = active && bridge.data?.taskStatus === 'completed' && chatGptReady !== true;

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
    if (!requestId || compactPaused) return;
    let disposed = false;
    const refresh = () => void (async () => {
      const request = await api.chatGptRequest(requestId);
      if (disposed) return;
      if (request.hasFinalResponse) {
        await reconcileChatGptRequest(requestId).catch(() => undefined);
      }
      if (['completed', 'stopped', 'failed'].includes(request.status)) await reloadBridge();
    })().catch(() => undefined);
    refresh();
    const timer = window.setInterval(refresh, 1_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [bridge.data?.activeRequestId, compactPaused, reloadBridge]);
  useEffect(() => {
    if (conversationUrl || compactPaused) return;
    const requestId = bridge.data?.latestRequestId;
    const submittedContent = bridge.data?.latestSubmittedContent;
    if (!requestId || !submittedContent) return;
    let disposed = false;
    let recovering = false;
    const recover = () => void (async () => {
      if (disposed || recovering) return;
      recovering = true;
      try {
        const result = await recoverChatGptIdentity(requestId, submittedContent);
        if (!disposed) setIdentitySyncError(result.recovered === false ? {
          taskId,
          message: `${tr('ChatGPT conversation identity is still syncing.')} (${result.reason || 'identity_not_confirmed'})`,
        } : null);
      } catch (reason) {
        if (!disposed) setIdentitySyncError({ taskId, message: errorText(reason) });
      } finally {
        // Another callback may have persisted the URL even if recovery failed.
        if (!disposed) await reloadBridge();
        recovering = false;
      }
    })();
    recover();
    const timer = window.setInterval(recover, 2_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [taskId, bridge.data?.latestRequestId, bridge.data?.latestSubmittedContent, conversationUrl, compactPaused, reloadBridge]);
  const sendContent = async (message: string, clearComposer = true): Promise<boolean> => {
    if (!bridge.data || busy || compact?.isBlocked() || bridgeSync) return false;
    setBusy(true); setError('');
    try {
      if (!conversationUrl) throw new Error(tr('ChatGPT conversation identity is still syncing.'));
      const status = await chatGptExtensionStatus(conversationUrl);
      setExtensionReady(status.ready);
      setChatGptTabOpen(status.ready && status.conversationTabOpen);
      setChatGptReady(status.ready && status.conversationTabOpen && status.conversationReady);
      if (!status.ready) throw new Error(tr('ChatCMD ChatGPT Bridge extension is not ready. Enable or reload it, then try again.'));
      if (!status.conversationTabOpen) throw new Error(tr('This conversation’s ChatGPT tab is no longer open. Reopen the ChatGPT conversation and try again.'));
      if (!status.conversationReady) throw new Error(tr('ChatGPT is not ready for another message yet.'));
      if (compact?.isBlocked() || bridgeSync) return false;
      const request = await api.sendChatGptMessage(taskId, { model: DEFAULT_MODEL, content: message });
      await dispatchChatGptRequest({ requestId: request.id, submittedContent: request.submittedContent, model: request.model, conversationUrl });
      const latest = await waitForDispatchState(request.id);
      if (latest.status === 'failed') throw new Error(latest.errorMessage || tr('Could not send the message to ChatGPT.'));
      if (clearComposer) setContent('');
      await reloadBridge();
      return true;
    } catch (reason) {
      setError(errorText(reason));
      return false;
    } finally { setBusy(false); }
  };

  const send = async (event: FormEvent) => {
    event.preventDefault();
    const message = content.trim();
    if (!message || busy || active || compactPaused || bridgeSync || extensionReady !== true || chatGptTabOpen !== true || chatGptReady !== true || !bridge.data) return;
    await sendContent(message);
  };

  const stop = async () => {
    if (!bridge.data?.activeRequestId || busy || compact?.isBlocked()) return;
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
    if (!bridge.data || !conversationUrl || busy || compact?.isBlocked() || bridgeSync) return;
    setError('');
    try {
      await openChatGptConversationTab(conversationUrl);
      setChatGptTabOpen(true);
      setChatGptReady(false);
    } catch (reason) { setError(errorText(reason)); }
  };

  const focusTab = async () => {
    if (!bridge.data || !conversationUrl || busy || compact?.isBlocked() || bridgeSync) return;
    setError('');
    try { await focusChatGptConversationTab(conversationUrl); }
    catch (reason) { setError(errorText(reason)); }
  };

  const closeTab = async () => {
    if (!bridge.data || !conversationUrl || busy || compact?.isBlocked() || bridgeSync) return;
    setError('');
    try {
      await closeChatGptConversationTab(conversationUrl);
      setChatGptTabOpen(false);
      setChatGptReady(false);
    } catch (reason) { setError(errorText(reason)); }
  };

  const recoveryError = bridge.error || syncError;
  const connectionNotice = bridge.loading
    ? <div className="chatgpt-composer loading"><LoaderCircle className="spin" /><span>{tr('Loading ChatGPT bridge…')}</span></div>
    : !bridge.data
      ? <div className="chatgpt-composer error" role="alert"><CircleAlert /><span>{bridge.error || tr('ChatGPT bridge information is unavailable.')}</span></div>
      : !conversationUrl
        ? <div className={`chatgpt-composer ${recoveryError ? 'error' : 'loading'}`} role={recoveryError ? 'alert' : 'status'}>{recoveryError ? <CircleAlert /> : <LoaderCircle className="spin" />}<span>{recoveryError || tr('ChatGPT conversation identity is still syncing.')}</span></div>
        : extensionReady === false
          ? <div className="chatgpt-tab-required error" role="alert"><Unplug /><div><strong>{tr('Could not connect to ChatGPT Bridge')}</strong><span>{tr('Enable or reload the extension, then return to this conversation.')}</span></div></div>
          : chatGptTabOpen === false
            ? <div className="chatgpt-tab-required" role="alert"><CircleAlert /><div><strong>{tr('This conversation’s ChatGPT tab is closed')}</strong><span>{tr('ChatCMD must keep this exact ChatGPT tab open to send messages and track response status. Reopen the conversation and keep the tab open in your browser.')}</span><button type="button" onClick={() => void openTab()} disabled={busy || compactPaused || bridgeSync}><ExternalLink />{tr('Open ChatGPT conversation')}</button></div></div>
            : bridgeSync ? <p role="status">{compactText('bridgeSync')}</p> : null;
  return <>
    <CompactStatusCard />
    {connectionNotice}
    <ChatGptMessageQueuePanel
      taskId={taskId}
      openMode={queueMode}
      onOpenModeChange={setQueueMode}
      paused={compactPaused || bridgeSync}
      canAutoSend={!compactPaused && !bridgeSync && !active && !busy && extensionReady === true && chatGptTabOpen === true && chatGptReady === true}
      onAutoSend={(message) => sendContent(message, false)}
    />
    <form className="chatgpt-composer" hidden={!conversationUrl} onSubmit={(event) => void send(event)}>
      <div className="chatgpt-composer-row">
        <textarea aria-label={tr('Next message to ChatGPT')} rows={2} value={content} onChange={(event) => setContent(event.target.value)} disabled={active || busy || compactPaused || bridgeSync || extensionReady === false || chatGptTabOpen === false} placeholder={answerCompletedWaitingForUi ? tr('Answer completed; waiting for the ChatGPT UI before continuing.') : active ? tr('ChatGPT is responding…') : tr('Continue the ChatGPT conversation…')} />
        {active ? <button type="button" className="chatgpt-stop-button" onClick={() => void stop()} disabled={busy || compactPaused || bridgeSync || bridge.data?.activeStatus === 'stop_requested'}><CircleStop /><span>{bridge.data?.activeStatus === 'stop_requested' ? tr('Stopping…') : tr('Stop')}</span></button>
          : <button type="submit" className="chatgpt-composer-send" disabled={busy || compactPaused || bridgeSync || extensionReady !== true || chatGptTabOpen !== true || chatGptReady !== true || !content.trim()}><Send /><span>{tr('Send')}</span></button>}
      </div>
      <div className="chatgpt-composer-meta chatgpt-composer-actions">
        <button type="button" onClick={() => void closeTab()} disabled={busy || compactPaused || bridgeSync}>{tr('Close this tab')}</button><span aria-hidden="true">|</span>
        <button type="button" onClick={() => void focusTab()} disabled={busy || compactPaused || bridgeSync}>{tr('Change model')}</button><span aria-hidden="true">|</span>
        <button type="button" onClick={() => setQueueMode('queued')} disabled={busy || compactPaused || bridgeSync}>{tr('Queue another message')}</button><span aria-hidden="true">|</span>
        <button type="button" onClick={() => setQueueMode('immediate')} disabled={busy || compactPaused || bridgeSync}>{tr('Send immediate message')}</button>
        <span aria-hidden="true">|</span><CompactAction disabled={busy || bridgeSync || !conversationUrl} />
      </div>
      {error && <p className="chatgpt-form-error" role="alert"><CircleAlert />{error}</p>}
    </form>
  </>;
}


async function waitForDispatchState(requestId: string) {
  for (let index = 0; index < 60; index++) {
    const request = await api.chatGptRequest(requestId);
    if (request.status !== 'queued') return request;
    await new Promise((resolve) => window.setTimeout(resolve, 250));
  }
  return api.chatGptRequest(requestId);
}
function errorText(reason: unknown) { return reason instanceof Error ? reason.message : tr('Could not complete the ChatGPT request.'); }
