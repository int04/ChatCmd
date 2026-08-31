import { AlertTriangle, CheckCircle2, Cpu, Download, FileText, Laptop, PackageOpen, RefreshCw, RotateCcw, ShieldCheck, Sparkles } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { api } from '../api';
import { Modal } from '../components';
import { updateCopy } from './copy';
import type { UpdatePhase, UpdateStatus } from './types';
import { isActiveUpdatePhase } from './types';

export function UpdateSettings() {
  const copy = updateCopy();
  const [status, setStatus] = useState<UpdateStatus>();
  const [error, setError] = useState('');
  const [confirming, setConfirming] = useState(false);
  const [restarting, setRestarting] = useState(false);

  const refreshStatus = useCallback(async () => {
    try {
      const next = await api.updateStatus();
      setStatus(next);
      return next;
    } catch (reason) {
      setError(errorMessage(reason));
      return undefined;
    }
  }, []);

  const check = useCallback(async () => {
    setError('');
    try {
      setStatus(await api.checkForUpdate());
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void api.updateStatus().then((next) => {
      if (cancelled) return;
      setStatus(next);
      if (next.phase === 'idle') void check();
    }).catch((reason) => {
      if (!cancelled) setError(errorMessage(reason));
    });
    return () => { cancelled = true; };
  }, [check]);

  useEffect(() => {
    if (!status || !isActiveUpdatePhase(status.phase) || status.phase === 'restarting') return;
    const timer = window.setInterval(() => { void refreshStatus(); }, 450);
    return () => window.clearInterval(timer);
  }, [refreshStatus, status?.phase]);

  const beginUpdate = async () => {
    setConfirming(false);
    setError('');
    try {
      setStatus(await api.startUpdate());
    } catch (reason) {
      setError(errorMessage(reason));
    }
  };

  const restart = async () => {
    if (!status?.latestVersion) return;
    setError('');
    setRestarting(true);
    const targetVersion = status.latestVersion;
    try {
      setStatus(await api.restartForUpdate());
      await waitForRestart(targetVersion);
      window.location.reload();
    } catch (reason) {
      setRestarting(false);
      setError(errorMessage(reason));
    }
  };

  if (!status) return <div className="update-loading"><span className="spinner" />{copy.checking}</div>;

  const busy = isActiveUpdatePhase(status.phase);
  const latestVersion = status.latestVersion ?? '—';
  const hasUpdate = status.updateAvailable;

  return <div className="update-settings update-settings-pro">
    <section className={`update-settings-hero ${hasUpdate ? 'has-update' : ''}`}>
      <div className="update-settings-hero-icon" aria-hidden="true"><Sparkles /></div>
      <div className="update-settings-hero-copy">
        <span className={`update-settings-status-pill ${hasUpdate ? 'available' : 'current'}`}>
          <i />{hasUpdate ? copy.popupBadge : copy.upToDate}
        </span>
        <h2>{hasUpdate ? `ChatCMD ${latestVersion}` : copy.introTitle}</h2>
        <p>{hasUpdate ? copy.popupDescription : copy.introDescription}</p>
      </div>
      <div className="update-settings-hero-action">
        <button type="button" className="button secondary" disabled={busy || restarting} onClick={() => void check()}>
          <RefreshCw className={status.phase === 'checking' ? 'spin' : ''} />
          {status.phase === 'checking' ? copy.checking : copy.check}
        </button>
      </div>
    </section>

    {error && <div className="update-error"><AlertTriangle /><span>{error}</span></div>}

    <div className="update-settings-main-grid">
      <div className="update-settings-left">
        <section className="update-settings-version-panel">
          <header className="update-settings-section-heading">
            <div><Laptop /><span><small>{copy.currentVersion}</small><strong>{status.currentVersion}</strong></span></div>
            {hasUpdate && <span className="update-settings-version-arrow">→</span>}
            <div className={hasUpdate ? 'is-latest' : ''}><PackageOpen /><span><small>{copy.latestVersion}</small><strong>{latestVersion}</strong></span></div>
          </header>

          <div className="update-settings-device-row">
            <div><Cpu /><span><small>{copy.platform}</small><strong>{platformLabel(status.platform)} · {architectureLabel(status.architecture)}</strong></span></div>
            <div><ShieldCheck /><span><small>Updater</small><strong>{status.downloadAvailable || !hasUpdate ? 'Ready' : 'Unavailable'}</strong></span></div>
          </div>
        </section>

        <UpdateState status={status} />

        <div className="update-settings-primary-actions">
          {status.updateAvailable && status.downloadAvailable && !busy && status.phase !== 'readyToRestart' &&
            <button type="button" className="button primary update-settings-cta" onClick={() => setConfirming(true)}><Download />{copy.update}</button>}
          {status.phase === 'readyToRestart' &&
            <button type="button" className="button primary update-settings-cta" disabled={restarting} onClick={() => void restart()}>
              <RotateCcw className={restarting ? 'spin' : ''} />{restarting ? copy.restarting : copy.restart}
            </button>}
        </div>
      </div>

      <section className="update-settings-note-panel">
        <header>
          <span><FileText /></span>
          <div><small>{copy.releaseNotes}</small><strong>{hasUpdate ? `ChatCMD ${latestVersion}` : copy.currentVersion}</strong></div>
        </header>
        <div className="update-settings-note-scroll" tabIndex={0}>
          {status.note ? <p>{status.note}</p> : <div className="update-settings-note-empty"><FileText /><span>—</span></div>}
        </div>
      </section>
    </div>

    {confirming && <Modal title={copy.confirmTitle} description={copy.confirmDescription} close={() => setConfirming(false)} dangerous>
      <div className="warning-block"><AlertTriangle /><p>{copy.confirmWarning}</p></div>
      <div className="modal-actions"><button type="button" className="button secondary" onClick={() => setConfirming(false)}>{copy.cancel}</button><button type="button" className="button primary" onClick={() => void beginUpdate()}><Download />{copy.confirmUpdate}</button></div>
    </Modal>}
  </div>;
}

function UpdateState({ status }: { status: UpdateStatus }) {
  const copy = updateCopy();
  if (status.phase === 'unsupported') return <StateMessage icon={<AlertTriangle />} title={copy.unsupported} />;
  if (status.phase === 'failed') return <StateMessage icon={<AlertTriangle />} title={copy.failed} body={status.message ?? undefined} danger />;
  if (status.phase === 'upToDate') return <StateMessage icon={<CheckCircle2 />} title={status.message?.includes('published') ? copy.noPublishedVersion : copy.upToDate} />;
  if (status.updateAvailable && !status.downloadAvailable) return <StateMessage icon={<AlertTriangle />} title={copy.unavailable} />;
  if (status.phase === 'readyToRestart') return <StateMessage icon={<CheckCircle2 />} title={copy.ready} body={copy.readyInstruction} />;
  if (status.phase === 'restarting') return <StateMessage icon={<RotateCcw className="spin" />} title={copy.restarting} />;
  if (['downloading', 'extracting', 'preparing'].includes(status.phase)) return <UpdateProgress status={status} />;
  return null;
}

function UpdateProgress({ status }: { status: UpdateStatus }) {
  const copy = updateCopy();
  const title = status.phase === 'downloading' ? copy.downloading : status.phase === 'extracting' ? copy.extracting : copy.preparing;
  const percentage = status.progressPercent ?? 0;
  return <section className="update-progress-card">
    <header><div><span className="spinner" /><strong>{title}</strong></div>{status.progressPercent !== null && <b>{percentage}%</b>}</header>
    <div className={`update-progress-track ${status.progressPercent === null ? 'indeterminate' : ''}`}><i style={status.progressPercent === null ? undefined : { width: `${percentage}%` }} /></div>
    {(status.phase === 'downloading' || status.phase === 'extracting') && <small>{formatBytes(status.downloadedBytes)}{status.totalBytes ? ` / ${formatBytes(status.totalBytes)}` : ''}</small>}
    <div className="update-steps">
      <UpdateStep label={copy.downloadStep} state={stepState(status.phase, 'downloading')} icon={<Download />} />
      <UpdateStep label={copy.extractStep} state={stepState(status.phase, 'extracting')} icon={<PackageOpen />} />
      <UpdateStep label={copy.prepareStep} state={stepState(status.phase, 'preparing')} icon={<ShieldCheck />} />
      <UpdateStep label={copy.restartStep} state="pending" icon={<RotateCcw />} />
    </div>
  </section>;
}

function StateMessage({ icon, title, body, danger = false }: { icon: React.ReactNode; title: string; body?: string; danger?: boolean }) {
  return <div className={`update-state-message ${danger ? 'danger' : ''}`}><span>{icon}</span><div><strong>{title}</strong>{body && <p>{body}</p>}</div></div>;
}

function UpdateStep({ icon, label, state }: { icon: React.ReactNode; label: string; state: 'done' | 'active' | 'pending' }) {
  return <div className={`update-step ${state}`}><span>{state === 'done' ? <CheckCircle2 /> : icon}</span><small>{label}</small></div>;
}

function stepState(current: UpdatePhase, step: UpdatePhase): 'done' | 'active' | 'pending' {
  const order: UpdatePhase[] = ['downloading', 'extracting', 'preparing', 'readyToRestart'];
  const currentIndex = order.indexOf(current);
  const stepIndex = order.indexOf(step);
  if (currentIndex > stepIndex) return 'done';
  if (currentIndex === stepIndex) return 'active';
  return 'pending';
}

function formatBytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function platformLabel(value: string) { return value === 'macos' ? 'macOS' : value === 'windows' ? 'Windows' : value; }
function architectureLabel(value: string) { return value === 'x86_64' ? '64-bit' : value === 'x86' ? '32-bit' : value === 'aarch64' ? 'Apple Silicon' : value; }
function errorMessage(reason: unknown) { return reason instanceof Error ? reason.message : String(reason); }

async function waitForRestart(targetVersion: string) {
  for (let attempt = 0; attempt < 180; attempt += 1) {
    await delay(750);
    try {
      const response = await fetch('/api/info', { cache: 'no-store' });
      if (!response.ok) continue;
      const info = await response.json() as { version?: string };
      if (info.version === targetVersion) return;
    } catch {
      // ChatCMD is expected to be temporarily offline while files are replaced.
    }
  }
  throw new Error('ChatCMD did not come back online after the update. Please start ChatCMD manually.');
}

function delay(milliseconds: number) { return new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds)); }
