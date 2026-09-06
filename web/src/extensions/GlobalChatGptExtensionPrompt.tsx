import { ArrowRight, Puzzle, RefreshCw } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { Modal } from '../components';
import {
  chatGptExtensionStatus,
  REQUIRED_CHATGPT_EXTENSION_VERSION,
  type ChatGptExtensionStatus,
} from '../chatgptBridge';
import { useAppLanguage } from '../i18n';
import { extensionCopy } from './copy';
import './extensionSetup.css';

let startupExtensionCheck: Promise<ChatGptExtensionStatus> | undefined;

async function waitForPageLoad() {
  if (document.readyState === 'complete') return;
  await new Promise<void>((resolve) => window.addEventListener('load', () => resolve(), { once: true }));
}

async function checkExtensionAfterPageLoad() {
  await waitForPageLoad();
  const first = await chatGptExtensionStatus();
  if (first.ready) return first;
  await new Promise<void>((resolve) => window.setTimeout(resolve, 250));
  return chatGptExtensionStatus();
}

function checkExtensionOnce() {
  startupExtensionCheck ??= checkExtensionAfterPageLoad();
  return startupExtensionCheck;
}

export function GlobalChatGptExtensionPrompt() {
  const language = useAppLanguage();
  const copy = extensionCopy(language);
  const location = useLocation();
  const navigate = useNavigate();
  const [status, setStatus] = useState<ChatGptExtensionStatus>();
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void checkExtensionOnce().then((next) => {
      if (!cancelled) setStatus(next);
    });
    return () => { cancelled = true; };
  }, []);

  if (!status || dismissed || location.pathname === '/extension/setup') return null;

  const missing = !status.ready;
  const outdated = status.ready && status.extensionVersion !== REQUIRED_CHATGPT_EXTENSION_VERSION;
  if (!missing && !outdated) return null;

  const title = missing ? copy.popupMissingTitle : copy.popupOutdatedTitle;
  const description = missing ? copy.popupMissingDescription : copy.popupOutdatedDescription;
  return <Modal className="extension-install-modal" title={title} description={description} close={() => setDismissed(true)}>
    <div className="extension-install-modal-body">
      <div className="extension-install-visual" aria-hidden="true"><Puzzle /></div>
      <div className="extension-install-version-row">
        <div><small>{copy.currentVersion}</small><strong>{status.extensionVersion || copy.notDetected}</strong></div>
        <RefreshCw />
        <div><small>{copy.requiredVersion}</small><strong>{REQUIRED_CHATGPT_EXTENSION_VERSION}</strong></div>
      </div>
      <div className="extension-install-modal-actions">
        <button type="button" className="button secondary" onClick={() => setDismissed(true)}>{copy.close}</button>
        <button type="button" className="button primary" onClick={() => { setDismissed(true); navigate('/extension/setup'); }}>
          {copy.goSetup}<ArrowRight />
        </button>
      </div>
    </div>
  </Modal>;
}
