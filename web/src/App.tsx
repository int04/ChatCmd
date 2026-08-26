import { Activity, Braces, Radio, Send, Server, Wifi, WifiOff } from 'lucide-react';
import { FormEvent, useEffect, useMemo, useRef, useState } from 'react';

type ServerInfo = {
  name: string;
  version: string;
  api: string;
  websocket: string;
  connectedClients: number;
};

type EventItem = {
  id?: string;
  type: string;
  payload: unknown;
  receivedAt: string;
};

const initialInfo: ServerInfo = {
  name: 'ChatCmdClient',
  version: '-',
  api: '/api',
  websocket: '/ws',
  connectedClients: 0,
};

function App() {
  const [info, setInfo] = useState<ServerInfo>(initialInfo);
  const [apiOnline, setApiOnline] = useState(false);
  const [wsOnline, setWsOnline] = useState(false);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [message, setMessage] = useState('Hello from dashboard');
  const socketRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    const loadInfo = async () => {
      try {
        const health = await fetch('/api/health');
        if (!health.ok) throw new Error('API unavailable');
        setApiOnline(true);
        const response = await fetch('/api/info');
        if (response.ok) setInfo(await response.json());
      } catch {
        setApiOnline(false);
      }
    };

    void loadInfo();
    const timer = window.setInterval(loadInfo, 5000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(`${protocol}//${window.location.host}/ws`);
    socketRef.current = socket;

    socket.onopen = () => setWsOnline(true);
    socket.onclose = () => setWsOnline(false);
    socket.onerror = () => setWsOnline(false);
    socket.onmessage = (event) => {
      try {
        const parsed = JSON.parse(event.data) as Omit<EventItem, 'receivedAt'>;
        setEvents((current) => [
          { ...parsed, receivedAt: new Date().toLocaleTimeString() },
          ...current,
        ].slice(0, 100));
      } catch {
        setEvents((current) => [
          { type: 'raw.message', payload: event.data, receivedAt: new Date().toLocaleTimeString() },
          ...current,
        ].slice(0, 100));
      }
    };

    return () => socket.close();
  }, []);

  const statusText = useMemo(
    () => (apiOnline && wsOnline ? 'All systems operational' : 'Connection degraded'),
    [apiOnline, wsOnline],
  );

  const sendEvent = async (event: FormEvent) => {
    event.preventDefault();
    const payload = { source: 'dashboard', message };

    await fetch('/api/events', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ event_type: 'dashboard.message', payload }),
    });
  };

  const sendSocketMessage = () => {
    if (socketRef.current?.readyState === WebSocket.OPEN) {
      socketRef.current.send(message);
    }
  };

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand">
          <div className="brand-mark"><Braces size={22} /></div>
          <div>
            <strong>ChatCmdClient</strong>
            <span>MCP communication console</span>
          </div>
        </div>
        <div className={`system-pill ${apiOnline && wsOnline ? 'online' : 'warning'}`}>
          <span className="pulse" />
          {statusText}
        </div>
      </header>

      <section className="hero">
        <div>
          <span className="eyebrow">RUST MCP GATEWAY</span>
          <h1>API & realtime WebSocket control plane</h1>
          <p>Monitor the service, publish events, and observe live communication between AI agents and ChatCmdClient.</p>
        </div>
        <div className="hero-code">v{info.version}</div>
      </section>

      <section className="metrics-grid" aria-label="Service status">
        <article className="metric-card">
          <div className="metric-icon"><Server size={20} /></div>
          <div><span>REST API</span><strong>{apiOnline ? 'Online' : 'Offline'}</strong><small>{info.api}</small></div>
        </article>
        <article className="metric-card">
          <div className="metric-icon"><Radio size={20} /></div>
          <div><span>WebSocket</span><strong>{wsOnline ? 'Connected' : 'Disconnected'}</strong><small>{info.websocket}</small></div>
        </article>
        <article className="metric-card">
          <div className="metric-icon"><Activity size={20} /></div>
          <div><span>Connected clients</span><strong>{info.connectedClients}</strong><small>reported by API</small></div>
        </article>
        <article className="metric-card">
          <div className="metric-icon">{wsOnline ? <Wifi size={20} /> : <WifiOff size={20} />}</div>
          <div><span>Events captured</span><strong>{events.length}</strong><small>latest 100 retained</small></div>
        </article>
      </section>

      <section className="workspace-grid">
        <article className="panel composer-panel">
          <div className="panel-heading">
            <div><span className="eyebrow">EVENT TESTER</span><h2>Publish a message</h2></div>
          </div>
          <form onSubmit={sendEvent}>
            <label htmlFor="message">Payload message</label>
            <textarea id="message" value={message} onChange={(e) => setMessage(e.target.value)} rows={7} />
            <div className="actions">
              <button className="primary-button" type="submit"><Send size={17} /> POST /api/events</button>
              <button className="secondary-button" type="button" onClick={sendSocketMessage} disabled={!wsOnline}><Radio size={17} /> Send via WS</button>
            </div>
          </form>
          <div className="endpoint-list">
            <code>GET /api/health</code>
            <code>GET /api/info</code>
            <code>POST /api/events</code>
            <code>WS /ws</code>
          </div>
        </article>

        <article className="panel event-panel">
          <div className="panel-heading">
            <div><span className="eyebrow">REALTIME STREAM</span><h2>WebSocket events</h2></div>
            <span className={`connection-badge ${wsOnline ? 'connected' : ''}`}>{wsOnline ? 'LIVE' : 'OFFLINE'}</span>
          </div>
          <div className="event-list">
            {events.length === 0 ? (
              <div className="empty-state"><Radio size={28} /><strong>No events yet</strong><span>Publish an event to see it appear here.</span></div>
            ) : events.map((event, index) => (
              <div className="event-row" key={`${event.id ?? 'event'}-${index}`}>
                <div className="event-meta"><strong>{event.type}</strong><time>{event.receivedAt}</time></div>
                <pre>{JSON.stringify(event.payload, null, 2)}</pre>
              </div>
            ))}
          </div>
        </article>
      </section>
    </main>
  );
}

export default App;
