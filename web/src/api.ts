import { decodeEncryptedApiResponse, encryptedApiFetch } from './apiCrypto';
import { tr } from './i18n';
import type { Agent, AgentInput, ChatGptBridge, ChatGptRequest, CommandExecutionMode, LocalSettings, McpStatus, Overview, ProblemDetails, SecretResult, Session, SessionDetail, Skill, SkillOptionValue, Task, TaskDetail, TaskPage, Tool, ToolPreset, UserSkill } from './types';

export class ApiError extends Error {
  constructor(message: string, public status?: number, public problem?: ProblemDetails) { super(message); this.name = 'ApiError'; }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const method = (init.method ?? 'GET').toUpperCase();
  let response: Response;
  try { response = await encryptedApiFetch(path, init); }
  catch { throw new ApiError(tr('Local API is unavailable. Check that ChatCMD is running.')); }
  if (response.status === 204) return undefined as T;

  let payload: T | ProblemDetails | undefined;
  try { payload = await decodeEncryptedApiResponse<T | ProblemDetails>(path, method, response); }
  catch { /* malformed or non-JSON upstream error */ }
  if (!response.ok) {
    const problem = payload as ProblemDetails | undefined;
    const fieldErrors = problem?.errors ? Object.values(problem.errors).flat().join(' ') : '';
    throw new ApiError(fieldErrors || problem?.detail || problem?.title || tr('Request failed ({status})', { status: response.status }), response.status, problem);
  }
  if (payload === undefined) throw new ApiError(tr('Request failed ({status})', { status: response.status }), response.status);
  return payload as T;
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
  createChatGptRequest: (input: { agentId: string; model?: string; content: string }) => request<ChatGptRequest>('/api/local/chatgpt/requests', { method: 'POST', body: json(input) }),
  chatGptRequest: (id: string) => request<ChatGptRequest>(`/api/local/chatgpt/requests/${item(id)}`),
  chatGptBridge: (taskId: string) => request<ChatGptBridge>(`/api/local/chatgpt/tasks/${item(taskId)}`),
  sendChatGptMessage: (taskId: string, input: { model?: string; content: string }) => request<ChatGptRequest>(`/api/local/chatgpt/tasks/${item(taskId)}/messages`, { method: 'POST', body: json(input) }),
  stopChatGptMessage: (taskId: string) => request<ChatGptRequest>(`/api/local/chatgpt/tasks/${item(taskId)}/stop`, { method: 'POST', body: '{}' }),
  tasks: (cursor?: string, limit = 10) => request<TaskPage>(`/api/local/tasks?limit=${limit}${cursor ? `&cursor=${item(cursor)}` : ''}`),
  pendingConversationApprovals: () => request<Task[]>('/api/local/tasks/approvals/pending'),
  task: (id: string) => request<TaskDetail>(`/api/local/tasks/${item(id)}`),
  deleteTask: (id: string) => request<void>(`/api/local/tasks/${item(id)}`, { method: 'DELETE' }),
  taskAction: (id: string, action: string, body?: unknown) => request<TaskDetail>(`/api/local/tasks/${item(id)}/${action}`, { method: 'POST', body: body === undefined ? undefined : json(body) }),
  stopTask: (id: string) => request<TaskDetail>(`/api/local/tasks/${item(id)}/stop`, { method: 'POST', body: '{}' }),
  taskExecutionMode: (id: string, signal?: AbortSignal) => request<{ mode: CommandExecutionMode; overridden: boolean }>(`/api/local/tasks/${item(id)}/command-execution-mode`, { signal }),
  setTaskExecutionMode: (id: string, mode: CommandExecutionMode) => request<{ mode: CommandExecutionMode; overridden: boolean }>(`/api/local/tasks/${item(id)}/command-execution-mode`, { method: 'PUT', body: json({ mode }) }),
  stopTaskActivity: (taskId: string, activityId: string, input: { turnId?: string; reason?: string }) => request<void>(`/api/local/tasks/${item(taskId)}/activities/${item(activityId)}/stop`, { method: 'POST', body: json(input) }),
  resolveTaskApproval: (taskId: string, activityId: string, input: { turnId?: string; decision: 'allow' | 'allowSimilar' | 'reject'; reason?: string }) => request<{ accepted: boolean; decision: string }>(`/api/local/tasks/${item(taskId)}/activities/${item(activityId)}/approval`, { method: 'POST', body: json(input) }),
  sessions: () => request<Session[]>('/api/local/sessions'),
  session: (id: string, cursor?: string) => request<SessionDetail>(`/api/local/sessions/${item(id)}${cursor ? `?cursor=${item(cursor)}` : ''}`),
  sessionAction: (id: string, action: string, body?: unknown) => request<SessionDetail>(`/api/local/sessions/${item(id)}/${action}`, { method: 'POST', body: body === undefined ? undefined : json(body) }),
  skills: () => request<UserSkill[]>('/api/local/skills'),
  skill: (id: string) => request<Skill>(`/api/local/skills/${item(id)}`),
  setSkillEnabled: (id: string, isEnabled: boolean) => request<UserSkill>(`/api/local/skills/${item(id)}/enabled`, { method: 'PATCH', body: json({ isEnabled }) }),
  updateSkillOptions: (id: string, options: Record<string, SkillOptionValue>) => request<UserSkill>(`/api/local/skills/${item(id)}/options`, { method: 'PATCH', body: json({ options }) }),
  installSkill: (repositoryUrl: string) => request<UserSkill>('/api/local/skills/install', { method: 'POST', body: json({ repositoryUrl }) }),
  deleteSkill: (id: string) => request<void>(`/api/local/skills/${item(id)}`, { method: 'DELETE' }),
  settings: () => request<LocalSettings>('/api/local/settings'),
  saveSettings: (value: LocalSettings) => request<LocalSettings>('/api/local/settings', { method: 'PUT', body: json(value) }),
};
