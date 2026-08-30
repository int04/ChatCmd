import { decodeEncryptedApiResponse, encryptedApiFetch } from './apiCrypto';
import { tr } from './i18n';
import type { Agent, AgentInput, AuthInfo, AuthResult, ChatGptBridge, ChatGptRequest, CommandExecutionMode, LiveTerminalOutput, LocalSettings, McpStatus, Overview, ProblemDetails, SecretResult, Session, SessionDetail, Skill, SkillOptionValue, Task, TaskDetail, TaskPage, Tool, ToolPreset, UserSkill } from './types';

export class ApiError extends Error {
  constructor(message: string, public status?: number, public problem?: ProblemDetails) { super(message); this.name = 'ApiError'; }
}

export interface GiftCodeRedeemResult {
  success: boolean;
  giftCodeId: number;
  planId: number;
  planType: number;
  planName: string;
  days: number;
  remainingUses: number;
  expiresAt: string;
}

export interface BillingBalance { vnd: number }
export interface ServicePlan { id: number; name: string; price: number; type: number; days: number }
export interface DealCheckResult {
  valid: boolean;
  dealId: number;
  code: string;
  value: number;
  remainingCount: number;
  planId: number;
  planName: string;
  originalPrice: number;
  discountAmount: number;
  finalPrice: number;
}
export interface PlanPurchaseResult {
  success: boolean;
  planId: number;
  planName: string;
  planType: number;
  days: number;
  extended: boolean;
  originalPrice: number;
  discountAmount: number;
  finalPrice: number;
  remainingBalance: number;
  dealCode?: string | null;
  remainingDealCount?: number | null;
  expiresAt: string;
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
    if (response.status === 401 && problem?.code !== 'invalid_current_password' && typeof window !== 'undefined') window.dispatchEvent(new Event('chatcmd:auth-required'));
    const fieldErrors = problem?.errors ? Object.values(problem.errors).flat().join(' ') : '';
    throw new ApiError(fieldErrors || problem?.message || problem?.detail || problem?.title || tr('Request failed ({status})', { status: response.status }), response.status, problem);
  }
  if (payload === undefined) throw new ApiError(tr('Request failed ({status})', { status: response.status }), response.status);
  return payload as T;
}
const json = (value: unknown) => JSON.stringify(value);
const item = (value: string) => encodeURIComponent(value);
const backendPath = (path: string) => {
  const normalized = path.trim().replace(/^\/+/, '');
  if (!normalized || normalized.includes('://') || normalized.startsWith('crypto/')) throw new ApiError('Invalid backend API path.');
  return `/api/local/backend/${normalized}`;
};

export const backendApi = {
  get: <T>(path: string, init: RequestInit = {}) => request<T>(backendPath(path), { ...init, method: 'GET' }),
  post: <T>(path: string, body?: unknown, init: RequestInit = {}) => request<T>(backendPath(path), { ...init, method: 'POST', body: body === undefined ? undefined : json(body) }),
  put: <T>(path: string, body?: unknown, init: RequestInit = {}) => request<T>(backendPath(path), { ...init, method: 'PUT', body: body === undefined ? undefined : json(body) }),
  patch: <T>(path: string, body?: unknown, init: RequestInit = {}) => request<T>(backendPath(path), { ...init, method: 'PATCH', body: body === undefined ? undefined : json(body) }),
  delete: <T>(path: string, init: RequestInit = {}) => request<T>(backendPath(path), { ...init, method: 'DELETE' }),
};

export const api = {
  login: (email: string, password: string) => request<AuthResult>('/api/local/auth/login', { method: 'POST', body: json({ email, password }) }),
  register: (email: string, password: string) => request<AuthResult>('/api/local/auth/register', { method: 'POST', body: json({ email, password }) }),
  authInfo: () => request<AuthInfo>('/api/local/auth/info'),
  billingBalance: () => request<BillingBalance>(backendPath('billing/balance')),
  servicePlans: () => request<ServicePlan[]>(backendPath('plans')),
  checkDeal: (code: string, planId: number) => request<DealCheckResult>(backendPath('deals/check'), { method: 'POST', body: json({ code, planId }) }),
  purchasePlan: (planId: number, dealCode: string | null) => request<PlanPurchaseResult>(backendPath('plans/purchase'), { method: 'POST', body: json({ planId, dealCode }) }),
  redeemGiftCode: (code: string) => request<GiftCodeRedeemResult>(backendPath('giftcode/redeem'), { method: 'POST', body: json({ code }) }),
  changePassword: (currentPassword: string, newPassword: string) => request<{ success: boolean; message: string }>('/api/local/auth/change-password', { method: 'POST', body: json({ currentPassword, newPassword }) }),
  logout: () => request<AuthResult>('/api/local/auth/logout', { method: 'POST', body: '{}' }),
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
  setTaskTitle: (id: string, title: string) => request<TaskDetail>(`/api/local/tasks/${item(id)}/title`, { method: 'PUT', body: json({ title }) }),
  deleteTask: (id: string) => request<void>(`/api/local/tasks/${item(id)}`, { method: 'DELETE' }),
  taskAction: (id: string, action: string, body?: unknown) => request<TaskDetail>(`/api/local/tasks/${item(id)}/${action}`, { method: 'POST', body: body === undefined ? undefined : json(body) }),
  stopTask: (id: string) => request<TaskDetail>(`/api/local/tasks/${item(id)}/stop`, { method: 'POST', body: '{}' }),
  taskExecutionMode: (id: string, signal?: AbortSignal) => request<{ mode: CommandExecutionMode; overridden: boolean }>(`/api/local/tasks/${item(id)}/command-execution-mode`, { signal }),
  setTaskExecutionMode: (id: string, mode: CommandExecutionMode) => request<{ mode: CommandExecutionMode; overridden: boolean }>(`/api/local/tasks/${item(id)}/command-execution-mode`, { method: 'PUT', body: json({ mode }) }),
  stopTaskActivity: (taskId: string, activityId: string, input: { turnId?: string; reason?: string }) => request<void>(`/api/local/tasks/${item(taskId)}/activities/${item(activityId)}/stop`, { method: 'POST', body: json(input) }),
  resolveTaskApproval: (taskId: string, activityId: string, input: { turnId?: string; decision: 'allow' | 'allowSimilar' | 'reject'; reason?: string }) => request<{ accepted: boolean; decision: string }>(`/api/local/tasks/${item(taskId)}/activities/${item(activityId)}/approval`, { method: 'POST', body: json(input) }),
  sessions: () => request<Session[]>('/api/local/sessions'),
  liveTerminals: () => request<Session[]>('/api/local/sessions/terminals/live'),
  liveTerminalOutput: (id: string, afterSequence = 0, waitMs = 20_000) => request<LiveTerminalOutput>(`/api/local/sessions/${item(id)}/live?afterSequence=${afterSequence}&waitMs=${waitMs}`),
  writeTerminalInput: (id: string, text: string) => request<{ accepted: boolean; writtenBytes: number }>(`/api/local/sessions/${item(id)}/input`, { method: 'POST', body: json({ text }) }),
  resizeTerminal: (id: string, columns: number, rows: number) => request<{ accepted: boolean; columns: number; rows: number }>(`/api/local/sessions/${item(id)}/resize`, { method: 'POST', body: json({ columns, rows }) }),
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
