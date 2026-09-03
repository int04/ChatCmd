import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import './TaskTerminalSection.css';
import { Cpu, HardDrive, TerminalSquare, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { api } from '../api';
import { ProblemBanner, StatusBadge } from '../components';
import { tr } from '../i18n';
import { useRealtime } from '../realtime';
import type { Session, TimelineEvent } from '../types';
import { useLoad } from '../useLoad';

const formatBytes = (value?: number) => value == null ? '—' : value < 1024 ** 2 ? `${Math.round(value / 1024)} KB` : value < 1024 ** 3 ? `${(value / 1024 ** 2).toFixed(1)} MB` : `${(value / 1024 ** 3).toFixed(2)} GB`;
const object = (value: unknown) => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;

export function TaskTerminalSection({ taskId, turnId }: { taskId: string; turnId?: string }) {
  const initial = useLoad(api.liveTerminals, []);
  const [terminals, setTerminals] = useState<Session[]>([]);
  const [selectedId, setSelectedId] = useState<string>();
  useEffect(() => { setTerminals(initial.data ?? []); }, [initial.data]);
  useEffect(() => { setSelectedId(undefined); }, [taskId, turnId]);

  const handleRealtime = useCallback((event: TimelineEvent) => {
    if (event.taskId !== taskId || (turnId && event.turnId !== turnId)) return;
    if (event.type === 'terminal.opened') {
      const payload = object(event.payload);
      const id = typeof payload?.id === 'string' ? payload.id : event.sessionId;
      if (!id) return;
      const next: Session = {
        kind: 'terminal', id, taskId: event.taskId, turnId: event.turnId,
        shell: typeof payload?.shell === 'string' ? payload.shell : undefined,
        processId: typeof payload?.processId === 'number' ? payload.processId : undefined,
        status: typeof payload?.status === 'string' ? payload.status : 'running',
        workingDirectory: typeof payload?.workingDirectory === 'string' ? payload.workingDirectory : undefined,
        createdAtUtc: typeof payload?.createdAtUtc === 'string' ? payload.createdAtUtc : event.occurredAt,
        busy: false,
        lastSequence: typeof payload?.lastSequence === 'number' ? payload.lastSequence : 0,
      };
      setTerminals((current) => [next, ...current.filter((item) => item.id !== id)]);
      return;
    }
    if (event.type === 'terminal.closed') {
      const id = event.sessionId ?? (typeof object(event.payload)?.sessionId === 'string' ? object(event.payload)?.sessionId as string : undefined);
      if (id) setTerminals((current) => current.filter((item) => item.id !== id));
      return;
    }
    if (event.type !== 'tool_call' && event.type !== 'tool_result') return;
    const payload = object(event.payload);
    const tool = typeof payload?.tool === 'string' ? payload.tool : '';
    if (!tool.startsWith('shell_')) return;
    const input = object(payload?.input);
    const output = object(payload?.output);
    const id = typeof input?.sessionId === 'string' ? input.sessionId : typeof output?.sessionId === 'string' ? output.sessionId : undefined;
    if (!id) return;
    const busy = event.type === 'tool_call';
    setTerminals((current) => current.map((item) => item.id === id ? { ...item, busy } : item));
  }, [taskId, turnId]);
  useRealtime(handleRealtime);

  const visible = useMemo(() => terminals.filter((item) => item.taskId === taskId && (!turnId || item.turnId === turnId)), [taskId, terminals, turnId]);
  useEffect(() => { if (selectedId && !visible.some((item) => item.id === selectedId)) setSelectedId(undefined); }, [selectedId, visible]);
  const selected = visible.find((item) => item.id === selectedId);

  return <section className="task-info-section task-terminal-section">
    <strong>{tr('Terminal / Task')}</strong>
    <div className="task-terminal-list">
      {visible.length ? visible.map((terminal) => <button className="task-terminal-item" type="button" key={terminal.id} onClick={() => setSelectedId(terminal.id)}>
        <TerminalSquare /><span><code>{terminal.shell ?? tr('Terminal')}</code><small>{terminal.workingDirectory ?? terminal.id}</small></span><i className={terminal.busy ? 'busy' : 'idle'}>{terminal.busy ? tr('In use') : tr('Idle')}</i>
      </button>) : <div className="task-info-generation"><TerminalSquare /><div><code>{tr('No active terminal')}</code><small>#{taskId}</small></div></div>}
    </div>
    {selected && <TaskTerminalModal terminal={selected} onClose={() => setSelectedId(undefined)} />}
  </section>;
}

function TaskTerminalModal({ terminal, onClose }: { terminal: Session; onClose: () => void }) {
  const sessionId = terminal.id;
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const lastSequenceRef = useRef(0);
  const snapshotReadyRef = useRef(false);
  const pendingRef = useRef(new Map<number, string>());
  const inputQueueRef = useRef(Promise.resolve());
  const inputBufferRef = useRef('');
  const inputTimerRef = useRef<number | undefined>(undefined);
  const resizeQueueRef = useRef(Promise.resolve());
  const [ready, setReady] = useState(false);
  const [problem, setProblem] = useState('');

  const handleRealtime = useCallback((event: TimelineEvent) => {
    if (event.type !== 'terminal.live_output' || event.sessionId !== sessionId) return;
    const payload = object(event.payload);
    const sequence = typeof payload?.sequence === 'number' ? payload.sequence : 0;
    const data = typeof payload?.data === 'string' ? payload.data : '';
    if (!sequence || !data || sequence <= lastSequenceRef.current) return;
    if (!snapshotReadyRef.current) { pendingRef.current.set(sequence, data); return; }
    terminalRef.current?.write(data);
    lastSequenceRef.current = sequence;
  }, [sessionId]);
  useRealtime(handleRealtime);

  useEffect(() => {
    if (!hostRef.current) return;
    const xterm = new Terminal({ convertEol: false, cursorBlink: true, cursorStyle: 'block', fontFamily: 'SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace', fontSize: 13, lineHeight: 1.35, scrollback: 10_000, allowTransparency: true, theme: { background: '#101116', foreground: '#f2f2f4', cursor: '#f2f2f4', cursorAccent: '#101116', black: '#1c1d22', red: '#ff5f57', green: '#28c840', yellow: '#febc2e', blue: '#4c8dff', magenta: '#c678dd', cyan: '#56b6c2', white: '#e6e6e6', brightBlack: '#686b73', brightRed: '#ff7b73', brightGreen: '#62d973', brightYellow: '#ffd36a', brightBlue: '#78a9ff', brightMagenta: '#d99bed', brightCyan: '#7dd8e3', brightWhite: '#ffffff', selectionBackground: '#3c5074aa' } });
    const fit = new FitAddon(); xterm.loadAddon(fit); xterm.open(hostRef.current); fit.fit(); xterm.focus(); terminalRef.current = xterm; setReady(true);
    const flushInput = () => { inputTimerRef.current = undefined; const data = inputBufferRef.current; inputBufferRef.current = ''; if (!data) return; inputQueueRef.current = inputQueueRef.current.then(() => api.writeTerminalInput(sessionId, data)).then(() => undefined).catch((error) => setProblem(error instanceof Error ? error.message : tr('Terminal input failed'))); };
    const dataDisposable = xterm.onData((data) => { inputBufferRef.current += data; if (inputTimerRef.current === undefined) inputTimerRef.current = window.setTimeout(flushInput, 4); });
    const resize = () => { fit.fit(); resizeQueueRef.current = resizeQueueRef.current.then(() => api.resizeTerminal(sessionId, xterm.cols, xterm.rows)).then(() => undefined).catch(() => undefined); };
    resize(); window.addEventListener('resize', resize); const observer = new ResizeObserver(resize); observer.observe(hostRef.current);
    return () => { observer.disconnect(); window.removeEventListener('resize', resize); dataDisposable.dispose(); if (inputTimerRef.current !== undefined) window.clearTimeout(inputTimerRef.current); inputBufferRef.current = ''; snapshotReadyRef.current = false; pendingRef.current.clear(); setReady(false); xterm.dispose(); terminalRef.current = null; };
  }, [sessionId]);

  useEffect(() => {
    if (!ready) return;
    let cancelled = false;
    const loadSnapshot = async () => {
      try {
        let cursor = 0;
        while (!cancelled) {
          const result = await api.liveTerminalOutput(sessionId, cursor, 0);
          if (cancelled) return;
          for (const event of result.events) { if (event.sequence > cursor) { terminalRef.current?.write(event.data); cursor = event.sequence; } }
          if (!result.events.length || cursor >= result.latestAvailableSequence) break;
        }
        lastSequenceRef.current = cursor;
        snapshotReadyRef.current = true;
        for (const [sequence, data] of [...pendingRef.current.entries()].sort((a, b) => a[0] - b[0])) {
          if (sequence > lastSequenceRef.current) { terminalRef.current?.write(data); lastSequenceRef.current = sequence; }
        }
        pendingRef.current.clear();
      } catch (error) { if (!cancelled) setProblem(error instanceof Error ? error.message : tr('Terminal stream is unavailable')); }
    };
    void loadSnapshot();
    return () => { cancelled = true; };
  }, [ready, sessionId]);

  useEffect(() => { if (terminalRef.current) terminalRef.current.options.disableStdin = Boolean(terminal.busy); }, [terminal.busy]);
  useEffect(() => { const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); }; window.addEventListener('keydown', onKey); return () => window.removeEventListener('keydown', onKey); }, [onClose]);

  return <div className="modal-backdrop task-terminal-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="task-terminal-modal" role="dialog" aria-modal="true" aria-label={tr('Terminal')}>
      <header className="task-terminal-modal-toolbar"><div><TerminalSquare /><span><strong>{terminal.shell ?? tr('Terminal')}</strong><code>{sessionId}</code></span></div><div><StatusBadge state={terminal.status ?? 'offline'} /><button className="plain-icon" type="button" onClick={onClose} aria-label={tr('Close')}><X /></button></div></header>
      <ProblemBanner message={problem} clear={() => setProblem('')} />
      <section className="mac-terminal-window task-terminal-window"><header className="mac-terminal-titlebar"><div className="mac-traffic-lights"><i /><i /><i /></div><strong>{terminal.workingDirectory ?? tr('Terminal')}</strong><div className="mac-terminal-metrics"><span>PID {terminal.processId ?? '—'}</span><span><Cpu />{terminal.cpuPercent == null ? '—' : `${terminal.cpuPercent.toFixed(1)}%`}</span><span><HardDrive />{formatBytes(terminal.memoryBytes)}</span></div></header><div className="xterm-host task-terminal-xterm" ref={hostRef} /><footer className="mac-terminal-footer"><span>{terminal.busy ? tr('The Agent is currently using this terminal. Input is temporarily locked.') : tr('Interactive input is enabled for this live terminal.')}</span></footer></section>
    </div>
  </div>;
}
