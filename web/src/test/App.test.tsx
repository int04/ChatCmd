import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from '../App';
import { FakeSocket } from './setup';

vi.mock('../apiCrypto', () => ({
  encryptedApiFetch: (path: string, init: RequestInit) => fetch(path, init),
  decodeEncryptedApiResponse: async <T,>(_path: string, _method: string, response: Response) => response.json() as Promise<T>,
}));

const overview = { app: { version: '1.0', startedAtUtc: '2026-01-01T00:00:00Z', state: 'ready' }, device: { id: 'device-001', name: 'Workstation', platform: 'Windows', architecture: 'x64' }, mcp: { state: 'listening', endpoint: 'http://127.0.0.1:5310/mcp/{token}', connectedClients: 1 }, database: { state: 'ready', path: 'C:\\data\\cmd.db', schemaVersion: '3' }, terminal: { defaultShell: 'pwsh', activeSessions: 1, totalSessions: 2, failedSessions: 0 }, tasks: { running: 1, completed: 4, failed: 0, approvals: 0 }, recentEvents: [] };
const json = (data: unknown, status = 200) => Promise.resolve(new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } }));
function at(path: string) { return render(<MemoryRouter initialEntries={[path]}><App /></MemoryRouter>); }
const session = { id: 'session-alpha', shell: 'pwsh', status: 'running', kind: 'terminal', createdAtUtc: '2026-01-01T00:00:00Z', updatedAtUtc: '2026-01-01T00:00:00Z' };
const defaultFetch = (input: string | URL | Request, _init?: RequestInit) => {
  const path = String(input);
  if (path.endsWith('/auth/status')) return json({ configured: true, authenticated: true, idleTimeoutSeconds: 1800 });
  if (path.endsWith('/overview')) return json(overview);
  if (path.endsWith('/system/elevation')) return json({ supported: false, elevated: false });
  if (path.endsWith('/workspaces/projects')) return json([]);
  if (path.endsWith('/tasks/approvals/pending') || path.endsWith('/plan/questions/pending') || path.endsWith('/subagents/fallback/pending')) return json([]);
  if (path.includes('/tasks?')) return json({ items: [{ id: '90071992547409931234', title: 'Lossless task', status: 'running', updatedAtUtc: '2026-01-01T00:00:00Z' }] });
  if (path.endsWith('/sessions/terminals/live')) return json([]);
  if (path.endsWith('/sessions')) return json([session]);
  if (path.includes('/sessions/')) return json({ session, events: [] });
  if (path.includes('/tasks/')) return json({ task: { id: '90071992547409931234', title: 'Lossless task', status: 'running', updatedAtUtc: '2026-01-01T00:00:00Z' }, events: [], subagents: [], subagentApprovals: [], approvalGrants: [], executionMode: 'allowAll' });
  if (path.endsWith('/mcp/agents') || path.endsWith('/mcp/tunnels') || path.endsWith('/mcp/tools') || path.endsWith('/mcp/tool-presets')) return json([]);
  if (path.endsWith('/settings')) return json({ port: 8080 });
  return json([]);
};
beforeEach(() => { FakeSocket.instances.length = 0; vi.stubGlobal('fetch', vi.fn(defaultFetch) as typeof fetch); });

describe('GUI authentication', () => {
  it('asks for password setup on first use', async () => {
    vi.mocked(fetch).mockImplementation((input, init) => String(input).endsWith('/auth/status') ? json({ configured: false, authenticated: false, idleTimeoutSeconds: 1800 }) as never : defaultFetch(input, init) as never);
    at('/');
    expect(await screen.findByRole('heading', { name: 'Create a password' })).toBeInTheDocument();
    expect(screen.getByText('Confirm password')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Runtime overview' })).not.toBeInTheDocument();
  });

  it('asks for login when the saved GUI session is absent', async () => {
    vi.mocked(fetch).mockImplementation((input, init) => String(input).endsWith('/auth/status') ? json({ configured: true, authenticated: false, idleTimeoutSeconds: 1800 }) as never : defaultFetch(input, init) as never);
    at('/');
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument();
    expect(screen.queryByText('Confirm password')).not.toBeInTheDocument();
  });
});

describe('routing and runtime states', () => {
  it.each(['/login', '/register', '/plans', '/account', '/payment'])('replaces legacy %s with overview', async (path) => { at(path); expect(await screen.findByRole('heading', { name: 'Runtime overview' })).toBeInTheDocument(); });
  it('shows degraded health from API without optimistic status', async () => { vi.mocked(fetch).mockImplementation((input, init) => String(input).endsWith('/overview') ? json({ ...overview, mcp: { state: 'error', connectedClients: 0, lastError: 'Port unavailable' } }) as never : defaultFetch(input, init) as never); at('/'); expect(await screen.findByText('Runtime is degraded')).toBeInTheDocument(); expect(screen.getByText('Port unavailable')).toBeInTheDocument(); });
  it('shows API failure and retries', async () => { let failed = false; vi.mocked(fetch).mockImplementation((input, init) => { if (String(input).endsWith('/overview') && !failed) { failed = true; return Promise.reject(new Error('down')) as never; } return defaultFetch(input, init) as never; }); at('/'); expect(await screen.findByText('Local API is unavailable. Check that ChatCMD is running.')).toBeInTheDocument(); await userEvent.click(screen.getByRole('button', { name: 'Retry' })); expect(await screen.findByText('Workstation')).toBeInTheDocument(); });
  it('navigates task and session lossless IDs', async () => { at('/tasks'); await userEvent.click(await screen.findByText('Lossless task')); expect(await screen.findByText('Activity timeline')).toBeInTheDocument(); at('/sessions'); await userEvent.click(await screen.findByRole('button', { name: /Sessions/ })); await userEvent.click(await screen.findByText('pwsh')); expect(await screen.findByText('Terminal stream')).toBeInTheDocument(); });
  it('keeps the task list and shared websocket mounted when opening a task detail', async () => { at('/tasks'); await screen.findByText('Lossless task'); const before = vi.mocked(fetch).mock.calls.filter(([input]) => String(input).includes('/tasks?')).length; expect(FakeSocket.instances).toHaveLength(1); await userEvent.click(screen.getByText('Lossless task')); expect(await screen.findByText('Activity timeline')).toBeInTheDocument(); const after = vi.mocked(fetch).mock.calls.filter(([input]) => String(input).includes('/tasks?')).length; expect(after).toBe(before); expect(FakeSocket.instances).toHaveLength(1); });
  it('updates the document title from realtime activity outside tasks', async () => { at('/agents'); await screen.findByRole('heading', { name: 'Plugin list' }); expect(FakeSocket.instances).toHaveLength(1); FakeSocket.instances[0].open(); await FakeSocket.instances[0].ready(); await FakeSocket.instances[0].message({ id: 'title-global-1', type: 'tool_call', taskId: 'task-away', occurredAt: '2026-01-01T00:00:01Z', payload: { status: 'started', tool: 'fs_read_text', input: { path: 'D:\\work\\README.md' } } }); await waitFor(() => expect(document.title).toContain('Reading D:\\work\\README.md')); });
});

describe('agent MCP URLs', () => {
  it('shows create and rotate URLs only in the focused modal', async () => {
    const agent = { id: 'agent-1', name: 'Local IDE', enabled: true, toolIds: [] };
    vi.mocked(fetch).mockImplementation((input, init) => { const path = String(input); if (path.endsWith('/agents') && init?.method === 'POST') return json({ agent, endpoint: 'http://127.0.0.1:8080/mcp/create-once-secret' }) as never; if (path.endsWith('/rotate-secret')) return json({ agent, endpoint: 'http://127.0.0.1:8080/mcp/rotate-once-secret' }) as never; if (path.endsWith('/agents')) return json([]) as never; return defaultFetch(input, init) as never; });
    at('/agents'); await userEvent.click(await screen.findByRole('button', { name: 'Create new Plugin connection' })); await userEvent.type(screen.getByLabelText('Name'), 'Local IDE'); await userEvent.click(screen.getByRole('button', { name: 'Save access profile' })); const dialog = await screen.findByRole('alertdialog'); expect(dialog).toHaveTextContent('http://127.0.0.1:8080/mcp/create-once-secret'); expect(dialog.contains(document.activeElement)).toBe(true); await userEvent.click(screen.getByRole('button', { name: 'I saved the connection link' })); expect(screen.queryByText('http://127.0.0.1:8080/mcp/create-once-secret')).not.toBeInTheDocument(); await userEvent.click(screen.getByRole('button', { name: 'Create new access code' })); await userEvent.click(screen.getByRole('button', { name: 'Confirm' })); expect(await screen.findByText('http://127.0.0.1:8080/mcp/rotate-once-secret')).toBeInTheDocument();
  });
  it('renders API problem detail near agent workflow', async () => {
    vi.mocked(fetch).mockImplementation((input, init) => { const path = String(input); if (path.endsWith('/agents') && init?.method === 'POST') return json({ title: 'Invalid agent', detail: 'Project folder is outside configured roots.' }, 400) as never; return defaultFetch(input, init) as never; });
    at('/agents'); await userEvent.click(await screen.findByRole('button', { name: 'Create new Plugin connection' })); await userEvent.type(screen.getByLabelText('Name'), 'Blocked agent'); await userEvent.click(screen.getByRole('button', { name: 'Save access profile' })); expect(await screen.findByText('Project folder is outside configured roots.')).toBeInTheDocument();
  });
});
