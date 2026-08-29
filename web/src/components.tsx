import { AlertTriangle, CheckCircle2, ChevronDown, CircleX, LoaderCircle, RefreshCw, X } from 'lucide-react';
import { useEffect, useId, useRef } from 'react';
import { appLocale, tr, translatedStatus } from './i18n';
import type { HealthState } from './types';

export function StatusBadge({ state, label }: { state: string; label?: string }) {
  const good = ['ready', 'listening', 'running', 'online', 'completed', 'connected'].includes(state.toLowerCase());
  const bad = ['error', 'failed', 'faulted', 'offline', 'stopped'].includes(state.toLowerCase());
  return <span className={`status-badge ${good ? 'good' : bad ? 'bad' : 'warn'}`}><i aria-hidden="true" />{label ?? translatedStatus(state)}</span>;
}
export function Loading({ label = tr('Loading') }: { label?: string }) { return <div className="state-panel" role="status"><LoaderCircle className="spin" /><strong>{label}</strong></div>; }
export function Empty({ title, body }: { title: string; body: string }) { return <div className="state-panel"><CheckCircle2 /><strong>{title}</strong><span>{body}</span></div>; }
export function ErrorState({ message, retry }: { message: string; retry?: () => void }) { return <div className="state-panel error-state" role="alert"><CircleX /><strong>{tr('Could not load data')}</strong><span>{message}</span>{retry && <button className="button secondary" onClick={retry}><RefreshCw />{tr('Retry')}</button>}</div>; }
export function ProblemBanner({ message, clear }: { message?: string; clear?: () => void }) { return message ? <div className="problem-banner" role="alert"><AlertTriangle /><span>{message}</span>{clear && <button className="icon-button" aria-label={tr('Dismiss error')} onClick={clear}><X /></button>}</div> : null; }
export function PageHeading({ eyebrow, title, body, actions }: { eyebrow: string; title: string; body?: string; actions?: React.ReactNode }) { return <header className="page-heading"><div><span className="eyebrow">{eyebrow}</span><h1>{title}</h1>{body && <p>{body}</p>}</div>{actions && <div className="heading-actions">{actions}</div>}</header>; }
export function Disclosure({ title, children }: { title: string; children: React.ReactNode }) { return <details className="disclosure"><summary>{title}<ChevronDown /></summary><div>{children}</div></details>; }
export function Modal({ title, description, children, close, dangerous = false, className }: { title: string; description?: string; children: React.ReactNode; close: () => void; dangerous?: boolean; className?: string }) {
  const titleId = useId(); const panel = useRef<HTMLDivElement>(null); const previous = useRef<HTMLElement | null>(null);
  useEffect(() => {
    previous.current = document.activeElement as HTMLElement;
    const root = panel.current; root?.querySelector<HTMLElement>('button,input,select,textarea')?.focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
      if (event.key !== 'Tab' || !root) return;
      const items = [...root.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),[href]')];
      if (!items.length) return; const first = items[0]; const last = items.at(-1)!;
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', keydown); return () => { document.removeEventListener('keydown', keydown); previous.current?.focus(); };
  }, [close]);
  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) close(); }}><div ref={panel} className={`modal ${dangerous ? 'dangerous' : ''} ${className ?? ''}`} role={dangerous ? 'alertdialog' : 'dialog'} aria-modal="true" aria-labelledby={titleId}><header><div><h2 id={titleId}>{title}</h2>{description && <p>{description}</p>}</div><button className="icon-button" aria-label={tr('Close dialog')} onClick={close}><X /></button></header>{children}</div></div>;
}
export function formatTime(value?: string) { if (!value) return '—'; const date = new Date(value); return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(appLocale(), { dateStyle: 'medium', timeStyle: 'short' }).format(date); }
export function healthLabel(state: HealthState) { return state === 'unknown' ? tr('Not reported') : translatedStatus(state); }
