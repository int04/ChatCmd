export type Id = string;
export type HealthState = 'ready' | 'listening' | 'running' | 'starting' | 'degraded' | 'stopped' | 'offline' | 'error' | 'unknown';
export type RealtimeState = 'online' | 'reconnecting' | 'offline';
export type CommandExecutionMode = 'approval' | 'allowAll';

export interface Overview {
  app: { version: string; startedAtUtc: string; state: HealthState; lastError?: string };
  device: { id: Id; name: string; platform: string; osVersion?: string; architecture: string };
  mcp: { state: HealthState; endpoint?: string; connectedClients: number; lastError?: string };
  database: { state: HealthState; path: string; schemaVersion?: string; lastError?: string };
  terminal: { defaultShell: string; activeSessions: number; totalSessions: number; failedSessions: number };
  tasks: { running: number; completed: number; failed: number; approvals: number };
  sessions?: { active: number; total: number };
  recentEvents?: TimelineEvent[];
}
export interface McpStatus { state: HealthState; endpoint?: string; connectedClients?: number; lastError?: string }
export interface TimelineEvent { id: Id; type: string; occurredAt: string; taskId?: Id; sessionId?: Id; turnId?: Id; payload?: unknown }
export interface Agent { id: Id; name: string; enabled: boolean; presetId?: Id; toolIds: Id[]; secretLast4?: string; updatedAtUtc?: string }
export interface AgentInput { name: string; enabled: boolean; presetId?: Id; toolIds: Id[] }
export interface SecretResult { agent?: Agent; endpoint: string }
export interface Tool { id: Id; name: string; description?: string; group?: string; dangerous?: boolean }
export interface ToolPreset { id: Id; name: string; description?: string; toolIds: Id[] }
export interface Task { id: Id; title?: string; source?: string; projectFolder?: string | null; allowExecute?: boolean | null; approvalDeadlineUtc?: string | null; status: string; updatedAtUtc: string; createdAtUtc?: string; generation?: number; turnCount?: number; activeSessionId?: Id; outputPreview?: string; approvalPending?: boolean; finalResponseCount?: number; isSubagent?: boolean; parentTaskId?: Id; parentTurnId?: Id; agentName?: string }
export interface TaskPage { items: Task[]; nextCursor?: string }
export interface WorkspaceProject { id: Id; name: string; path: string; createdAtUtc?: string; updatedAtUtc?: string }
export interface ChatGptRequest { id: Id; taskId?: Id; turnId: Id; agentId: Id; model: string; userContent: string; submittedContent: string; projectFolder?: string | null; status: string; conversationId?: string; conversationUrl?: string; assistantContent?: string; errorMessage?: string; hasFinalResponse?: boolean }
export interface ChatGptBridge { taskId: Id; conversationId: string; conversationUrl: string; model: string; taskStatus?: string; activeRequestId?: Id; activeStatus?: string; activeSubmittedContent?: string }
export interface SubagentRun { id: Id; parentTurnId: Id; taskId?: Id; name: string; request: string; status: string; createdAtUtc: string; updatedAtUtc: string; completedAtUtc?: string }
export interface SubagentApproval { activityId: Id; childTaskId: Id; subagentId: Id; agentName: string; parentTurnId: Id; childTurnId?: Id; tool?: string; input?: unknown; createdAtUtc: string }
export interface TaskDetail { task: Task; turns?: TaskTurn[]; events?: TimelineEvent[]; nextCursor?: string; subagents?: SubagentRun[]; subagentApprovals?: SubagentApproval[]; executionMode?: CommandExecutionMode; executionModeSourceTaskId?: Id }
export interface TaskTurn { id: Id; generation?: number; actor?: string; status?: string; startedAtUtc?: string; completedAtUtc?: string; events?: TimelineEvent[] }
export interface Session { kind: 'mcp' | 'terminal'; id: Id; taskId?: Id; turnId?: Id; shell?: string; processId?: number; status: string; workingDirectory?: string; createdAtUtc?: string; updatedAtUtc?: string; closedAtUtc?: string; replayCursor?: string; cpuPercent?: number; memoryBytes?: number; busy?: boolean; lastSequence?: number }
export interface SessionDetail { session: Session; events: TimelineEvent[]; nextCursor?: string; truncated?: boolean }
export interface LiveTerminalEvent { sequence: number; occurredAtUtc: string; stream: string; data: string }
export interface LiveTerminalOutput { sessionId: Id; oldestAvailableSequence: number; latestAvailableSequence: number; replayTruncated: boolean; events: LiveTerminalEvent[] }
export type SkillOptionValue = string | number | boolean;
export interface SkillOptionChoice { value: string; label: string }
export interface UserSkillOption { key: string; label: string; description?: string | null; type: 'select' | 'boolean' | 'text' | 'number'; value: SkillOptionValue; choices?: SkillOptionChoice[] | null }
export interface UserSkill { id: Id; title: string; description?: string | null; iconUrl?: string | null; source: 'global' | 'workspace'; sourceUrl?: string | null; enabled: boolean; canDelete?: boolean; options: UserSkillOption[] }
export interface Skill { id: Id; name: string; source?: string; enabled: boolean; description?: string; content?: string }
export interface LocalSettings { bindAddress: string; port: number; mcpEndpoint: string; databasePath: string; databaseState?: HealthState; executionMode: CommandExecutionMode; approveNewConversations: boolean; terminalExecutable: string; taskConcurrency: number; sessionConcurrency: number; theme: 'system' | 'light' | 'dark'; language: 'en' | 'vi'; sound?: boolean; newAgentSound: boolean; finishedTaskSound: boolean }
export interface ProblemDetails { type?: string; title?: string; status?: number; detail?: string; message?: string; code?: string; instance?: string; errors?: Record<string, string[]> }
export interface AuthPlan { type: number; expriAt?: string | null; name: string }
export interface AuthInfo { id: number; email: string; vnd: number; plan: AuthPlan; useNextTime?: string | null; useNextReset?: string | null }
export interface AuthResult { authenticated: boolean }
