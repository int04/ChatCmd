import { describe, expect, it, vi } from 'vitest';

vi.mock('../apiCrypto', () => ({
  encryptedApiFetch: (path: string, init: RequestInit) => {
    const headers = new Headers(init.headers);
    headers.set('X-ChatCmdClient', 'local-ui');
    return fetch(path, { ...init, headers });
  },
  decodeEncryptedApiResponse: <T,>(_path: string, _method: string, response: Response) => response.json() as Promise<T>,
}));

import { api } from '../api';

describe('local API client', () => {
  it('sends local marker and no credential header', async () => {
    vi.stubGlobal('fetch', vi.fn((_path, init) => {
      const headers = new Headers(init?.headers);
      expect(headers.get('X-ChatCmdClient')).toBe('local-ui');
      expect(headers.has('Authorization')).toBe(false);
      return Promise.resolve(new Response('[]', { status: 200 }));
    }));
    await api.tasks();
  });

  it('renders RFC7807 detail and validation errors', async () => {
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(
      JSON.stringify({ title: 'Invalid request', detail: 'Port is already in use.', status: 409 }),
      { status: 409, headers: { 'Content-Type': 'application/problem+json' } },
    ))));
    await expect(api.settings()).rejects.toMatchObject({ message: 'Port is already in use.', status: 409 });
  });

  it('targets task-scoped execution mode and stop endpoints', async () => {
    const fetchMock = vi.fn((path: string | URL | Request, _init?: RequestInit) => {
      const value = String(path);
      if (value.includes('/activities/')) return Promise.resolve(new Response(null, { status: 204 }));
      if (value.endsWith('/command-execution-mode')) {
        return Promise.resolve(new Response(JSON.stringify({ mode: 'allowAll', overridden: true }), { status: 200, headers: { 'Content-Type': 'application/json' } }));
      }
      return Promise.resolve(new Response(JSON.stringify({ task: { id: 'task/with space', status: 'stopped', updatedAtUtc: '2026-08-27T00:00:00Z' }, events: [] }), { status: 200, headers: { 'Content-Type': 'application/json' } }));
    });
    vi.stubGlobal('fetch', fetchMock);
    await api.taskExecutionMode('task/with space');
    await api.setTaskExecutionMode('task/with space', 'allowAll');
    await api.stopTaskActivity('task/with space', 'activity/1', { turnId: 'turn-1', reason: 'Dừng để đổi cách làm' });
    await api.stopTask('task/with space');
    expect(fetchMock.mock.calls.map(([path]) => String(path))).toEqual([
      '/api/local/tasks/task%2Fwith%20space/command-execution-mode',
      '/api/local/tasks/task%2Fwith%20space/command-execution-mode',
      '/api/local/tasks/task%2Fwith%20space/activities/activity%2F1/stop',
      '/api/local/tasks/task%2Fwith%20space/stop',
    ]);
    expect(fetchMock.mock.calls[1]?.[1]).toMatchObject({ method: 'PUT' });
    expect(fetchMock.mock.calls[2]?.[1]).toMatchObject({ method: 'POST', body: JSON.stringify({ turnId: 'turn-1', reason: 'Dừng để đổi cách làm' }) });
    expect(fetchMock.mock.calls[3]?.[1]).toMatchObject({ method: 'POST' });
  });

  it('previews repositories and sends selected skill paths for installation', async () => {
    const fetchMock = vi.fn((path: string | URL | Request, _init?: RequestInit) => {
      const payload = String(path).endsWith('/preview')
        ? { repositoryUrl: 'https://github.com/example/skills', skills: [], skippedInvalid: 0 }
        : { skills: [] };
      return Promise.resolve(new Response(JSON.stringify(payload), { status: 200, headers: { 'Content-Type': 'application/json' } }));
    });
    vi.stubGlobal('fetch', fetchMock);

    await api.previewSkills('https://github.com/example/skills');
    await api.installSkills('https://github.com/example/skills', ['skills/one', 'skills/two']);

    expect(fetchMock.mock.calls.map(([path]) => String(path))).toEqual([
      '/api/local/skills/preview',
      '/api/local/skills/install',
    ]);
    expect(fetchMock.mock.calls[0]?.[1]).toMatchObject({ method: 'POST', body: JSON.stringify({ repositoryUrl: 'https://github.com/example/skills' }) });
    expect(fetchMock.mock.calls[1]?.[1]).toMatchObject({ method: 'POST', body: JSON.stringify({ repositoryUrl: 'https://github.com/example/skills', skillPaths: ['skills/one', 'skills/two'] }) });
  });
});
