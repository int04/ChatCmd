import type { Agent, AgentInput, LocalSettings, McpStatus, Overview, ProblemDetails, SecretResult, Session, SessionDetail, Skill, Task, TaskDetail, Tool, ToolPreset } from './types';

export class ApiError extends Error {
  constructor(message: string, public status?: number, public problem?: ProblemDetails) { super(message); this.name = 'ApiError'; }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set('X-ChatCmdClient', 'local-ui');
  if (init.body && !headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
  let response: Response;
  try { response = await fetch(path, { ...init, headers }); }
  catch { throw new ApiError('Local API is unavailable. Check that ChatCMD is running.'); }
  if (!response.ok) {
    let problem: ProblemDetails | undefined;
    try { problem = await response.json() as ProblemDetails; } catch { /* non-JSON upstream error */ }
    const fieldErrors = problem?.errors ? Object.values(problem.errors).flat().join(' ') : '';
    throw new ApiError(fieldErrors || problem?.detail || problem?.title || `Request failed (${response.status})`, response.status, problem);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}
const json = (value: unknown) => JSON.stringify(value);
const item = (value: string) => encodeURIComponent(value);

export const api = {
  overview: () => request<Overview>('/api/local/overview'),
  mcpStatus: () => request<McpStatus>('/api/local/mcp/status'),
  agents: () => request<Agent[]>('/api/local/mcp/agents'),
  agent: (id: string) => request<Agent>(`/api/local/mcp/agents/${item(id)}`),
  createAgent: (input: AgentInput) => request<SecretResult>('/api/local/mcp/agents', { method: 'POST', body: json(input) }),
  updateAgent: (id: string, input: AgentInput) => request<Agent>(`/api/local/mcp/agents/${item(id)}`, { method: 'PUT', body: json(input) }),
  deleteAgent: (id: string) => request<void>(`/api/local/mcp/agents/${item(id)}`, { method: 'DELETE' }),
  rotateAgentSecret: (id: string) => request<SecretResult>(`/api/local/mcp/agents/${item(id)}/rotate-secret`, { method: 'POST' }),
  setAgentEnabled: (id: string, enabled: boolean) => request<Agent>(`/api/local/mcp/agents/${item(id)}/enabled`, { method: 'PATCH', body: json({ enabled }) }),
  tools: () => request<Tool[]>('/api/local/mcp/tools'),
  presets: () => request<ToolPreset[]>('/api/local/mcp/tool-presets'),
  tasks: () => request<Task[]>('/api/local/tasks'),
  task: (id: string) => request<TaskDetail>(`/api/local/tasks/${item(id)}`),
  taskAction: (id: string, action: string, body?: unknown) => request<TaskDetail>(`/api/local/tasks/${item(id)}/${action}`, { method: 'POST', body: body === undefined ? undefined : json(body) }),
  sessions: () => request<Session[]>('/api/local/sessions'),
  session: (id: string, cursor?: string) => request<SessionDetail>(`/api/local/sessions/${item(id)}${cursor ? `?cursor=${item(cursor)}` : ''}`),
  sessionAction: (id: string, action: string, body?: unknown) => request<SessionDetail>(`/api/local/sessions/${item(id)}/${action}`, { method: 'POST', body: body === undefined ? undefined : json(body) }),
  skills: () => request<Skill[]>('/api/local/skills'),
  skill: (id: string) => request<Skill>(`/api/local/skills/${item(id)}`),
  settings: () => request<LocalSettings>('/api/local/settings'),
  saveSettings: (value: LocalSettings) => request<LocalSettings>('/api/local/settings', { method: 'PUT', body: json(value) }),
};
