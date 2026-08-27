export type Id = string;
export type HealthState = 'ready' | 'listening' | 'running' | 'starting' | 'degraded' | 'stopped' | 'offline' | 'error' | 'unknown';
export type RealtimeState = 'online' | 'reconnecting' | 'offline';

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
export interface Agent { id: Id; name: string; enabled: boolean; projectFolder: string; presetId?: Id; toolIds: Id[]; secretLast4?: string; updatedAtUtc?: string }
export interface AgentInput { name: string; enabled: boolean; projectFolder: string; presetId?: Id; toolIds: Id[] }
export interface SecretResult { agent?: Agent; endpoint: string }
export interface Tool { id: Id; name: string; description?: string; group?: string; dangerous?: boolean }
export interface ToolPreset { id: Id; name: string; description?: string; toolIds: Id[] }
export interface Task { id: Id; title?: string; status: string; updatedAtUtc: string; createdAtUtc?: string; generation?: number; turnCount?: number; activeSessionId?: Id; outputPreview?: string; approvalPending?: boolean }
export interface TaskDetail { task: Task; turns?: TaskTurn[]; events?: TimelineEvent[]; executionMode?: string }
export interface TaskTurn { id: Id; generation?: number; actor?: string; status?: string; startedAtUtc?: string; completedAtUtc?: string; events?: TimelineEvent[] }
export interface Session { id: Id; taskId?: Id; shell?: string; processId?: number; status: string; workingDirectory?: string; createdAtUtc?: string; updatedAtUtc?: string; closedAtUtc?: string; replayCursor?: string }
export interface SessionDetail { session: Session; events: TimelineEvent[]; nextCursor?: string; truncated?: boolean }
export interface Skill { id: Id; name: string; source?: string; precedence?: number; enabled: boolean; shadowed?: boolean; description?: string; content?: string }
export interface LocalSettings { bindAddress: string; port: number; mcpEndpoint: string; databasePath: string; databaseState?: HealthState; executionMode: string; workspaceRoots: string[]; terminalExecutable: string; taskConcurrency: number; sessionConcurrency: number; theme: 'system' | 'light' | 'dark'; language: 'en' | 'vi'; sound: boolean }
export interface ProblemDetails { type?: string; title?: string; status?: number; detail?: string; instance?: string; errors?: Record<string, string[]> }
