import { Download, Sparkles } from 'lucide-react';
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
      if (!cancelled && next.updateAvailable) setStatus(next);
    }).catch(() => {
      // Startup update checks are intentionally silent when the network is unavailable.
    });
    return () => { cancelled = true; };
  }, []);

  const alreadyOnUpdatePage = location.pathname === '/settings' && new URLSearchParams(location.search).get('tab') === 'update';
  if (!status?.updateAvailable || dismissed || alreadyOnUpdatePage) return null;

  return <Modal title={copy.popupTitle} description={copy.popupDescription} close={() => setDismissed(true)}>
    <div className="update-popup-summary">
      <span className="update-popup-icon"><Sparkles /></span>
      <div><small>{copy.latestVersion}</small><strong>{status.latestVersion}</strong></div>
    </div>
    {status.note && <div className="update-popup-note"><small>{copy.releaseNotes}</small><p>{status.note}</p></div>}
    <div className="modal-actions">
      <button type="button" className="button secondary" onClick={() => setDismissed(true)}>{copy.close}</button>
      <button type="button" className="button primary" onClick={() => navigate('/settings?tab=update')}><Download />{copy.goUpdate}</button>
    </div>
  </Modal>;
}
