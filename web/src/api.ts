import { decodeEncryptedApiResponse, encryptedApiFetch } from './apiCrypto';
import { tr } from './i18n';
import type { Agent, AgentInput, ChatGptBridge, ChatGptQueuedMessage, ChatGptRequest, CommandExecutionMode, LiveTerminalOutput, LocalSettings, McpStatus, Overview, PlanQuestion, PluginLink, ProblemDetails, SecretResult, Session, SessionDetail, Skill, SkillInstallPreview, SkillInstallResult, SkillOptionValue, Task, TaskActivityDetail, TaskDetail, TaskPage, Tool, ToolPreset, Tunnel, TunnelTestResult, UserSkill, WorkspaceProject } from './types';

export class ApiError extends Error {
  constructor(message: string, public status?: number, public problem?: ProblemDetails) { super(message); this.name = 'ApiError'; }
}

export interface ElevationStatus { supported: boolean; elevated: boolean }
export interface DatabaseDiagnostics { path: string; tableCount: number; totalRows: number; fileSizeBytes: number; pageCount: number; pageSizeBytes: number; freePageCount: number; usedSizeBytes: number; tables: Array<{ name: string; rowCount: number }> }
export interface DiagnosticLogs { path: string; lineCount: number; lines: string[] }
export interface SubagentFallbackRequest {
  subagentId: string;
  parentTaskId?: string;
  parentTurnId?: string;
  childTaskId: string;
  name: string;
  submittedContent: string;
  attempt: number;
  maxAttempts: number;
  conversationId?: string | null;
  conversationUrl?: string | null;
}

export interface SubagentFallbackResult {
  accepted: boolean;
  completed?: boolean;
  retryScheduled?: boolean;
  exhausted?: boolean;
  attempt?: number;
  maxAttempts?: number;
  reason?: string;
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
    throw new ApiError(fieldErrors || problem?.message || problem?.detail || problem?.title || tr('Request failed ({status})', { status: response.status }), response.status, problem);
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
  tunnels: () => request<Tunnel[]>('/api/local/mcp/tunnels'),
  createTunnel: (baseUrl: string) => request<Tunnel>('/api/local/mcp/tunnels', { method: 'POST', body: json({ baseUrl }) }),
  deleteTunnel: (id: number) => request<{ deleted: boolean; id: number }>(`/api/local/mcp/tunnels/${id}`, { method: 'DELETE' }),
  testTunnel: (id: number) => request<TunnelTestResult>(`/api/local/mcp/tunnels/${id}/test`, { method: 'POST', body: '{}' }),
  pluginLinks: (agentId: string) => request<PluginLink[]>(`/api/local/mcp/agents/${item(agentId)}/plugin-links`),
  copyPluginLink: (agentId: string, tunnelId: number) => request<{ endpoint: string }>(`/api/local/mcp/agents/${item(agentId)}/plugin-links/${tunnelId}`, { method: 'POST', body: '{}' }),
  tools: () => request<Tool[]>('/api/local/mcp/tools'),
  presets: () => request<ToolPreset[]>('/api/local/mcp/tool-presets'),
  pickProjectFolder: () => request<{ path: string | null }>('/api/local/system/folder-picker', { method: 'POST', body: '{}' }),
  elevationStatus: () => request<ElevationStatus>('/api/local/system/elevation'),
  restartElevated: () => request<ElevationStatus>('/api/local/system/elevation/restart', { method: 'POST', body: '{}' }),
  exitApplication: () => request<{ closing: boolean }>('/api/local/system/exit', { method: 'POST', body: '{}' }),
  workspaceProjects: () => request<WorkspaceProject[]>('/api/local/workspaces/projects'),
  saveWorkspaceProject: (input: { name: string; path: string }) => request<WorkspaceProject>('/api/local/workspaces/projects', { method: 'POST', body: json(input) }),
  reorderWorkspaceProjects: (projectIds: string[]) => request<void>('/api/local/workspaces/projects/order', { method: 'PUT', body: json({ projectIds }) }),
  createChatGptRequest: (input: { agentId: string; model?: string; projectFolder?: string; content: string }) => request<ChatGptRequest>('/api/local/chatgpt/requests', { method: 'POST', body: json(input) }),
  chatGptRequest: (id: string) => request<ChatGptRequest>(`/api/local/chatgpt/requests/${item(id)}`),
  chatGptBridge: (taskId: string) => request<ChatGptBridge>(`/api/local/chatgpt/tasks/${item(taskId)}`),
  sendChatGptMessage: (taskId: string, input: { model?: string; content: string }) => request<ChatGptRequest>(`/api/local/chatgpt/tasks/${item(taskId)}/messages`, { method: 'POST', body: json(input) }),
  stopChatGptMessage: (taskId: string) => request<ChatGptRequest>(`/api/local/chatgpt/tasks/${item(taskId)}/stop`, { method: 'POST', body: '{}' }),
  chatGptQueue: (taskId: string) => request<ChatGptQueuedMessage[]>(`/api/local/chatgpt/tasks/${item(taskId)}/queue`),
  createChatGptQueuedMessage: (taskId: string, input: { content: string; mode: 'queued' | 'immediate' }) => request<ChatGptQueuedMessage>(`/api/local/chatgpt/tasks/${item(taskId)}/queue`, { method: 'POST', body: json(input) }),
  updateChatGptQueuedMessage: (taskId: string, messageId: string, input: { content?: string; mode?: 'queued' | 'immediate' }) => request<ChatGptQueuedMessage>(`/api/local/chatgpt/tasks/${item(taskId)}/queue/${item(messageId)}`, { method: 'PATCH', body: json(input) }),
  deleteChatGptQueuedMessage: (taskId: string, messageId: string) => request<void>(`/api/local/chatgpt/tasks/${item(taskId)}/queue/${item(messageId)}`, { method: 'DELETE' }),
  reorderChatGptQueue: (taskId: string, messageIds: string[]) => request<void>(`/api/local/chatgpt/tasks/${item(taskId)}/queue/order`, { method: 'PUT', body: json({ messageIds }) }),
  tasks: (cursor?: string, limit = 10, projectFolder?: string) => request<TaskPage>(`/api/local/tasks?limit=${limit}${cursor ? `&cursor=${item(cursor)}` : ''}${projectFolder ? `&projectFolder=${item(projectFolder)}` : ''}`),
  pendingConversationApprovals: () => request<Task[]>('/api/local/tasks/approvals/pending'),
  pendingSubagentFallbacks: () => request<SubagentFallbackRequest[]>('/api/local/subagents/fallback/pending'),
  reportSubagentFallbackResult: (id: string, input: { attempt: number; status: 'failed' | 'stopped' | 'completed'; errorMessage?: string; assistantContent?: string; conversationId?: string; conversationUrl?: string }) => request<SubagentFallbackResult>(`/api/local/subagents/${item(id)}/fallback/result`, { method: 'POST', body: json(input) }),
  pendingPlanQuestions: () => request<PlanQuestion[]>('/api/local/plan/questions/pending'),
  answerPlanQuestion: (id: string, answer: { kind: 'option'; optionIndex: 1 | 2 } | { kind: 'custom'; text: string }) => request<{ accepted: boolean; questionId: string; taskId: string; turnId: string }>(`/api/local/plan/questions/${item(id)}/answer`, { method: 'POST', body: json(answer) }),
  task: (id: string, cursor?: string, limit = 2) => request<TaskDetail>(`/api/local/tasks/${item(id)}?limit=${limit}${cursor ? `&cursor=${item(cursor)}` : ''}`),
  taskActivity: (taskId: string, activityId: string) => request<TaskActivityDetail>(`/api/local/tasks/${item(taskId)}/activities/${item(activityId)}`),
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
  previewSkills: (repositoryUrl: string) => request<SkillInstallPreview>('/api/local/skills/preview', { method: 'POST', body: json({ repositoryUrl }) }),
  installSkills: (repositoryUrl: string, skillPaths: string[]) => request<SkillInstallResult>('/api/local/skills/install', { method: 'POST', body: json({ repositoryUrl, skillPaths }) }),
  deleteSkill: (id: string) => request<void>(`/api/local/skills/${item(id)}`, { method: 'DELETE' }),
  settings: () => request<LocalSettings>('/api/local/settings'),
  saveSettings: (value: LocalSettings) => request<LocalSettings>('/api/local/settings', { method: 'PUT', body: json(value) }),
  databaseDiagnostics: () => request<DatabaseDiagnostics>('/api/local/diagnostics/database'),
  diagnosticLogs: () => request<DiagnosticLogs>('/api/local/diagnostics/logs'),
  deleteAllUserData: () => request<void>('/api/local/diagnostics/user-data', { method: 'DELETE' }),
};
