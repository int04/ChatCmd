import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from '../App';
import { FakeSocket } from './setup';

const overview = { app: { version: '1.0', startedAtUtc: '2026-01-01T00:00:00Z', state: 'ready' }, device: { id: 'device-001', name: 'Workstation', platform: 'Windows', architecture: 'x64' }, mcp: { state: 'listening', endpoint: 'http://127.0.0.1:5310/mcp', connectedClients: 1 }, database: { state: 'ready', path: 'C:\\data\\cmd.db', schemaVersion: '3' }, terminal: { defaultShell: 'pwsh', activeSessions: 1, totalSessions: 2, failedSessions: 0 }, tasks: { running: 1, completed: 4, failed: 0, approvals: 0 }, recentEvents: [] };
const json = (data: unknown, status = 200) => Promise.resolve(new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } }));
function at(path: string) { return render(<MemoryRouter initialEntries={[path]}><App /></MemoryRouter>); }
beforeEach(() => { FakeSocket.instances.length = 0; vi.stubGlobal('fetch', vi.fn((input: string | URL | Request) => { const path = String(input); if (path.endsWith('/overview')) return json(overview); if (path.endsWith('/tasks')) return json([{ id: '90071992547409931234', title: 'Lossless task', status: 'running', updatedAtUtc: '2026-01-01T00:00:00Z' }]); if (path.includes('/tasks/')) return json({ task: { id: '90071992547409931234', title: 'Lossless task', status: 'running', updatedAtUtc: '2026-01-01T00:00:00Z' }, events: [] }); if (path.endsWith('/sessions')) return json([{ id: 'session-alpha', shell: 'pwsh', status: 'running', updatedAtUtc: '2026-01-01T00:00:00Z' }]); if (path.includes('/sessions/')) return json({ session: { id: 'session-alpha', shell: 'pwsh', status: 'running' }, events: [] }); return json([]); }) as typeof fetch); });

describe('routing and runtime states', () => {
  it.each(['/login', '/register', '/plans', '/account', '/payment'])('replaces legacy %s with overview', async (path) => { at(path); expect(await screen.findByRole('heading', { name: 'Runtime overview' })).toBeInTheDocument(); });
  it('shows degraded health from API without optimistic status', async () => { vi.mocked(fetch).mockImplementation(() => json({ ...overview, mcp: { state: 'error', connectedClients: 0, lastError: 'Port unavailable' } }) as never); at('/'); expect(await screen.findByText('Runtime is degraded')).toBeInTheDocument(); expect(screen.getByText('Port unavailable')).toBeInTheDocument(); });
  it('shows API failure and retries', async () => { vi.mocked(fetch).mockRejectedValueOnce(new Error('down')).mockImplementation(() => json(overview) as never); at('/'); expect(await screen.findByText('Local API is unavailable. Check that ChatCMD is running.')).toBeInTheDocument(); await userEvent.click(screen.getByRole('button', { name: 'Retry' })); expect(await screen.findByText('Workstation')).toBeInTheDocument(); });
  it('navigates task and session lossless IDs', async () => { at('/tasks'); await userEvent.click(await screen.findByText('Lossless task')); expect(await screen.findByText('Activity timeline')).toBeInTheDocument(); at('/sessions'); await userEvent.click(await screen.findByText('pwsh')); expect(await screen.findByText('Terminal stream')).toBeInTheDocument(); });
});

describe('agent secrets', () => {
  it('shows create and rotate secrets only in focused modal', async () => {
    const agent = { id: 'agent-1', name: 'Local IDE', enabled: true, projectFolder: 'D:\\work', toolIds: [] };
    vi.mocked(fetch).mockImplementation((input, init) => { const path = String(input); if (path.endsWith('/agents') && init?.method === 'POST') return json({ agent, secret: 'create-once-secret' }) as never; if (path.endsWith('/rotate-secret')) return json({ secret: 'rotate-once-secret' }) as never; if (path.endsWith('/agents')) return json([]) as never; return json([]) as never; });
    at('/agents'); await userEvent.click(await screen.findByRole('button', { name: 'New agent' })); await userEvent.type(screen.getByLabelText('Name'), 'Local IDE'); await userEvent.click(screen.getByRole('button', { name: 'Save agent' })); const dialog = await screen.findByRole('alertdialog'); expect(dialog).toHaveTextContent('create-once-secret'); expect(dialog.contains(document.activeElement)).toBe(true); await userEvent.click(screen.getByRole('button', { name: 'I saved the secret' })); expect(screen.queryByText('create-once-secret')).not.toBeInTheDocument(); await userEvent.click(screen.getByRole('button', { name: 'Rotate secret' })); expect(await screen.findByText('rotate-once-secret')).toBeInTheDocument();
  });
  it('renders API problem detail near agent workflow', async () => {
    vi.mocked(fetch).mockImplementation((input, init) => { const path = String(input); if (path.endsWith('/agents') && init?.method === 'POST') return json({ title: 'Invalid agent', detail: 'Project folder is outside configured roots.' }, 400) as never; return json([]) as never; });
    at('/agents'); await userEvent.click(await screen.findByRole('button', { name: 'New agent' })); await userEvent.type(screen.getByLabelText('Name'), 'Blocked agent'); await userEvent.click(screen.getByRole('button', { name: 'Save agent' })); expect(await screen.findByRole('alert')).toHaveTextContent('Project folder is outside configured roots.');
  });
});
