import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import { ArrowLeft, CircleStop, Cpu, HardDrive, TerminalSquare } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { api } from '../api';
import { ErrorState, Loading, ProblemBanner, StatusBadge } from '../components';
import { tr } from '../i18n';
import { decodeTerminalEvent } from '../terminalOutput';
import type { Session } from '../types';
import { useLoad } from '../useLoad';

const formatBytes = (value?: number) => value == null ? '—' : value < 1024 ** 2 ? `${Math.round(value / 1024)} KB` : value < 1024 ** 3 ? `${(value / 1024 ** 2).toFixed(1)} MB` : `${(value / 1024 ** 3).toFixed(2)} GB`;

export function LiveTerminalPage() {
  const { sessionId = '' } = useParams();
  const navigate = useNavigate();
  const metadata = useLoad(async () => (await api.liveTerminals()).find((item) => item.id === sessionId), [sessionId]);
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const lastSequenceRef = useRef(0);
  const inputQueueRef = useRef(Promise.resolve());
  const inputBufferRef = useRef('');
  const inputTimerRef = useRef<number | undefined>(undefined);
  const resizeQueueRef = useRef(Promise.resolve());
  const [problem, setProblem] = useState('');
  const [terminalReady, setTerminalReady] = useState(false);

  useEffect(() => {
    if (metadata.loading || !hostRef.current) return;
    const terminal = new Terminal({
      convertEol: false,
      cursorBlink: true,
      cursorStyle: 'block',
      fontFamily: 'SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      lineHeight: 1.35,
      scrollback: 10_000,
      allowTransparency: true,
      theme: {
        background: '#101116', foreground: '#f2f2f4', cursor: '#f2f2f4', cursorAccent: '#101116',
        black: '#1c1d22', red: '#ff5f57', green: '#28c840', yellow: '#febc2e', blue: '#4c8dff',
        magenta: '#c678dd', cyan: '#56b6c2', white: '#e6e6e6', brightBlack: '#686b73', brightRed: '#ff7b73',
        brightGreen: '#62d973', brightYellow: '#ffd36a', brightBlue: '#78a9ff', brightMagenta: '#d99bed',
        brightCyan: '#7dd8e3', brightWhite: '#ffffff', selectionBackground: '#3c5074aa'
      }
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(hostRef.current);
    fit.fit();
    terminal.focus();
    terminalRef.current = terminal;
    setTerminalReady(true);
    const flushInput = () => {
      inputTimerRef.current = undefined;
      const data = inputBufferRef.current;
      inputBufferRef.current = '';
      if (!data) return;
      inputQueueRef.current = inputQueueRef.current
        .then(() => api.writeTerminalInput(sessionId, data))
        .then(() => undefined)
        .catch((error) => { setProblem(error instanceof Error ? error.message : tr('Terminal input failed')); });
    };
    const dataDisposable = terminal.onData((data) => {
      inputBufferRef.current += data;
      if (inputTimerRef.current === undefined) inputTimerRef.current = window.setTimeout(flushInput, 4);
    });
    const resize = () => {
      fit.fit();
      const columns = terminal.cols;
      const rows = terminal.rows;
      resizeQueueRef.current = resizeQueueRef.current
        .then(() => api.resizeTerminal(sessionId, columns, rows))
        .then(() => undefined)
        .catch(() => undefined);
    };
    resize();
    window.addEventListener('resize', resize);
    const observer = new ResizeObserver(resize);
    observer.observe(hostRef.current);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', resize);
      dataDisposable.dispose();
      if (inputTimerRef.current !== undefined) window.clearTimeout(inputTimerRef.current);
      inputTimerRef.current = undefined;
      inputBufferRef.current = '';
      setTerminalReady(false);
      terminal.dispose();
      terminalRef.current = null;
    };
  }, [sessionId, metadata.loading]);

  useEffect(() => {
    if (!terminalReady) return;
    lastSequenceRef.current = 0;
    let cancelled = false;
    let timer = 0;
    const read = async () => {
      try {
        const result = await api.liveTerminalOutput(sessionId, lastSequenceRef.current);
        if (cancelled) return;
        if (result.replayTruncated && lastSequenceRef.current > 0) terminalRef.current?.writeln('\r\n\x1b[33m[ChatCMD: terminal replay was truncated]\x1b[0m');
        for (const event of result.events) terminalRef.current?.write(decodeTerminalEvent(event));
        lastSequenceRef.current = Math.max(lastSequenceRef.current, result.events.at(-1)?.sequence ?? lastSequenceRef.current);
      } catch (error) {
        if (!cancelled) setProblem(error instanceof Error ? error.message : tr('Terminal stream is unavailable'));
      } finally {
        if (!cancelled) timer = window.setTimeout(read, 0);
      }
    };
    void read();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [sessionId, terminalReady]);

  const refreshMetadata = metadata.refresh;
  useEffect(() => {
    const timer = window.setInterval(() => void refreshMetadata(), 2000);
    return () => window.clearInterval(timer);
  }, [refreshMetadata]);

  useEffect(() => {
    if (terminalRef.current) terminalRef.current.options.disableStdin = !metadata.data || Boolean(metadata.data.busy);
  }, [metadata.data]);

  const close = async () => {
    setProblem('');
    try { await api.sessionAction(sessionId, 'close'); navigate('/sessions'); }
    catch (error) { setProblem(error instanceof Error ? error.message : tr('Action failed')); }
  };

  if (metadata.loading && metadata.data === undefined) return <Loading label={tr('Loading terminal')} />;
  if (metadata.error && metadata.data === undefined) return <ErrorState message={metadata.error} retry={() => void metadata.reload()} />;
  const terminal = metadata.data as Session | undefined;

  return <div className="live-terminal-page">
    <div className="live-terminal-toolbar">
      <Link className="button secondary compact" to="/sessions"><ArrowLeft />{tr('Back to terminals')}</Link>
      <div className="live-terminal-title"><TerminalSquare /><div><strong>{terminal?.shell ?? tr('Terminal')}</strong><code>{sessionId}</code></div></div>
      <div className="live-terminal-toolbar-actions"><StatusBadge state={terminal?.status ?? 'offline'} /><button className="button danger compact" disabled={!terminal} onClick={() => void close()}><CircleStop />{tr('Close')}</button></div>
    </div>
    <ProblemBanner message={problem} clear={() => setProblem('')} />
    <section className="mac-terminal-window">
      <header className="mac-terminal-titlebar"><div className="mac-traffic-lights"><i /><i /><i /></div><strong>{terminal?.workingDirectory ?? tr('Terminal')}</strong><div className="mac-terminal-metrics"><span>PID {terminal?.processId ?? '—'}</span><span><Cpu />{terminal?.cpuPercent == null ? '—' : `${terminal.cpuPercent.toFixed(1)}%`}</span><span><HardDrive />{formatBytes(terminal?.memoryBytes)}</span></div></header>
      <div className="xterm-host" ref={hostRef} />
      <footer className="mac-terminal-footer"><span>{!terminal ? tr('This terminal is no longer active.') : terminal.busy ? tr('The Agent is currently using this terminal. Input is temporarily locked.') : tr('Interactive input is enabled for this live terminal.')}</span>{terminal?.taskId && <Link to={`/tasks/${encodeURIComponent(terminal.taskId)}`}>{tr('Open task')}</Link>}</footer>
    </section>
  </div>;
}
