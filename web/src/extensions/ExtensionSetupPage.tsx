import { CheckCircle2, Puzzle, RefreshCw, Settings2 } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import {
  chatGptExtensionStatus,
  REQUIRED_CHATGPT_EXTENSION_VERSION,
  type ChatGptExtensionStatus,
} from '../chatgptBridge';
import { PageHeading } from '../components';
import { useAppLanguage } from '../i18n';
import { extensionCopy } from './copy';

type SupportedBrowser = 'chrome' | 'edge' | 'brave';
type BraveNavigator = Navigator & { brave?: { isBrave?: () => Promise<boolean> } };

function detectedBrowser(): SupportedBrowser {
  return /Edg\//i.test(navigator.userAgent) ? 'edge' : 'chrome';
}

const browserDetails: Record<SupportedBrowser, { name: string; target: string }> = {
  chrome: { name: 'Google Chrome', target: 'chrome://extensions/' },
  edge: { name: 'Microsoft Edge', target: 'edge://extensions/' },
  brave: { name: 'Brave', target: 'brave://extensions/' },
};

export function ExtensionSetupPage() {
  const language = useAppLanguage();
  const copy = extensionCopy(language);
  const [browser, setBrowser] = useState<SupportedBrowser>(detectedBrowser);
  const { name: browserName, target: browserTarget } = browserDetails[browser];
  const [status, setStatus] = useState<ChatGptExtensionStatus>();
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    let active = true;
    const brave = (navigator as BraveNavigator).brave;
    if (brave?.isBrave) void brave.isBrave().then((isBrave) => { if (active && isBrave) setBrowser('brave'); }).catch(() => undefined);
    return () => { active = false; };
  }, []);

  const check = useCallback(async () => {
    setChecking(true);
    setStatus(await chatGptExtensionStatus());
    setChecking(false);
  }, []);

  useEffect(() => { void check(); }, [check]);

  const compatible = status?.ready === true && status.extensionVersion === REQUIRED_CHATGPT_EXTENSION_VERSION;
  const statusLabel = compatible ? copy.connected : status?.ready ? copy.outdated : copy.missing;
  const statusClass = compatible ? 'good' : 'warn';

  return <div className="extension-setup-page">
    <PageHeading
      eyebrow={copy.pageEyebrow}
      title={copy.pageTitle}
      body={copy.pageBody}
      actions={<span className={`status-badge ${statusClass}`}><i />{statusLabel}</span>}
    />
    <section className="extension-setup-grid">
      <article className="panel extension-setup-card">
        <header><span className="extension-setup-icon"><Puzzle /></span><div><small>{copy.currentVersion}</small><strong>{status?.extensionVersion || copy.notDetected}</strong></div></header>
        <div className="extension-version-target"><small>{copy.requiredVersion}</small><strong>{REQUIRED_CHATGPT_EXTENSION_VERSION}</strong></div>
        <button type="button" className="button secondary" disabled={checking} onClick={() => void check()}><RefreshCw />{checking ? copy.checking : copy.checkAgain}</button>
      </article>

      <article className="panel extension-setup-card">
        <header><span className="extension-setup-icon"><Settings2 /></span><div><small>{copy.quickActions}</small><strong>{browserName}</strong></div></header>
        <div className="extension-quick-action">
          <div><code>{browserTarget}</code><span>{copy.openBrowserHint}</span></div>
        </div>
        <div className="extension-quick-action">
          <div><code>chatgpt-extension</code><span>{copy.openFolderHint}</span></div>
        </div>
      </article>
    </section>

    <section className="panel extension-setup-guide">
      <header><CheckCircle2 /><div><span className="eyebrow">{copy.pageEyebrow}</span><h2>{copy.guideTitle}</h2></div></header>
      <ol>
        <GuideStep number="1" title={copy.step1Title} body={copy.step1Body} />
        <GuideStep number="2" title={copy.step2Title} body={copy.step2Body} />
        <GuideStep number="3" title={copy.step3Title} body={copy.step3Body} />
        <GuideStep number="4" title={copy.step4Title} body={copy.step4Body} />
      </ol>
    </section>
  </div>;
}

function GuideStep({ number, title, body }: { number: string; title: string; body: string }) {
  return <li><span>{number}</span><div><strong>{title}</strong><p>{body}</p></div></li>;
}
