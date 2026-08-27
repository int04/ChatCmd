import { describe, expect, it, vi } from 'vitest';
import { api, ApiError } from '../api';

describe('local API client', () => {
  it('sends local marker and no credential header', async () => { vi.stubGlobal('fetch', vi.fn((_path, init) => { const headers = new Headers(init?.headers); expect(headers.get('X-ChatCmdClient')).toBe('local-ui'); expect(headers.has('Authorization')).toBe(false); return Promise.resolve(new Response('[]', { status: 200 })); })); await api.tasks(); });
  it('renders RFC7807 detail and validation errors', async () => { vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(JSON.stringify({ title: 'Invalid request', detail: 'Port is already in use.', status: 409 }), { status: 409, headers: { 'Content-Type': 'application/problem+json' } })))); await expect(api.settings()).rejects.toMatchObject({ message: 'Port is already in use.', status: 409 } satisfies Partial<ApiError>); });
});
