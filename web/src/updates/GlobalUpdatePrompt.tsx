import { ArrowRight, Download, FileText, ShieldCheck, Sparkles } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { api } from '../api';
import { Modal } from '../components';
import { updateCopy } from './copy';
import type { UpdateStatus } from './types';

export function GlobalUpdatePrompt() {
  const copy = updateCopy();
  const location = useLocation();
  const navigate = useNavigate();
  const [status, setStatus] = useState<UpdateStatus>();
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void api.checkForUpdate().then((next) => {
      if (!cancelled && !next.debugBuild && next.updateAvailable) setStatus(next);
    }).catch(() => {
      // Startup update checks are intentionally silent when the network is unavailable.
    });
    return () => { cancelled = true; };
  }, []);

  const alreadyOnUpdatePage = location.pathname === '/settings' && new URLSearchParams(location.search).get('tab') === 'update';
  if (!status?.updateAvailable || dismissed || alreadyOnUpdatePage) return null;

  const latestVersion = status.latestVersion ?? '—';

  return <Modal
    className="update-announcement-modal"
    title={copy.popupTitle}
    description={copy.popupDescription}
    close={() => setDismissed(true)}
  >
    <div className="update-announcement-grid">
      <section className="update-announcement-info">
        <div className="update-announcement-hero">
          <div className="update-announcement-visual" aria-hidden="true">
            <span className="update-announcement-orbit" />
            <span className="update-announcement-icon"><Sparkles /></span>
          </div>
          <div className="update-announcement-copy">
            <span className="update-announcement-badge"><span />{copy.popupBadge}</span>
            <strong className="update-announcement-version">ChatCMD {latestVersion}</strong>
            <p>{copy.popupDescription}</p>
          </div>
        </div>

        <div className="update-announcement-version-row" aria-label={`${copy.currentVersion} ${status.currentVersion}, ${copy.latestVersion} ${latestVersion}`}>
          <div><small>{copy.currentVersion}</small><strong>{status.currentVersion}</strong></div>
          <span className="update-announcement-arrow"><ArrowRight /></span>
          <div className="is-latest"><small>{copy.latestVersion}</small><strong>{latestVersion}</strong></div>
        </div>

        <div className="update-announcement-trust">
          <ShieldCheck />
          <span>{copy.popupTrust}</span>
        </div>

        <div className="update-announcement-actions">
          <button type="button" className="button secondary" onClick={() => setDismissed(true)}>{copy.close}</button>
          <button type="button" className="button primary update-announcement-primary" onClick={() => navigate('/settings?tab=update')}>
            <Download />{copy.goUpdate}<ArrowRight />
          </button>
        </div>
      </section>

      <section className="update-announcement-note-panel">
        <header className="update-announcement-note-heading">
          <span><FileText /></span>
          <div><small>{copy.releaseNotes}</small><strong>ChatCMD {latestVersion}</strong></div>
        </header>
        <div className="update-announcement-note-scroll" tabIndex={0}>
          {status.note ? <p>{status.note}</p> : <p className="is-empty">—</p>}
        </div>
      </section>
    </div>
  </Modal>;
}
