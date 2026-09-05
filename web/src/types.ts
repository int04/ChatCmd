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
  subagents?: { activeLeases: number; expiredTotal: number; maxHeartbeatLagMs: number; averageRuntimeMs: number; retryAttempts: number };
  sessions?: { active: number; total: number };
  recentEvents?: TimelineEvent[];
}
export interface McpStatus { state: HealthState; endpoint?: string; connectedClients?: number; lastError?: string }
export interface TimelineEvent { id: Id; type: string; occurredAt: string; taskId?: Id; sessionId?: Id; turnId?: Id; payload?: unknown }
export type WorkOutcome = 'completed' | 'partial' | 'blocked';
export type VerificationState = 'passed' | 'failed' | 'notRun' | 'notApplicable' | 'stale' | 'unknown';
export interface VerificationEvidence { executionId: Id; taskId?: Id; turnId?: Id; cwd?: string; command?: { executable?: string; argumentCount?: number; argumentsSha256?: string }; startedAtMs?: number; finishedAtMs?: number; terminalState?: string; exitCode?: number | null; timedOut?: boolean; cancelled?: boolean; artifactRef?: Id | null; status: VerificationState; reason?: string }
export interface CompletionCriterion { criterion: string; evidenceRefs: Id[]; covered: boolean }
export interface CompletionQualityReport { schemaVersion: number; lifecycle: 'completed'; workOutcome: WorkOutcome; workOutcomeProvenance: string; verification: VerificationState; verificationProvenance: string; verificationReason?: string | null; verificationScope?: string | null; criteria: CompletionCriterion[]; evidence: VerificationEvidence[]; diagnostics: Array<{ code: string; evidenceRef?: Id; message?: string }>; blockers: string[]; limitations: string[] }
export type PlanQuestionKind = 'clarification' | 'executionConsent';
export type PlanQuestionAnswer = { kind: 'option'; optionIndex: 1 | 2 } | { kind: 'custom'; text: string } | { kind: 'approveExecution' } | { kind: 'denyExecution' } | { kind: 'cancel' };
export interface PlanQuestion { id: Id; taskId: Id; turnId: Id; question: string; options: [string, string]; questionKind: PlanQuestionKind; issuerAgentId: Id; scopeDigest: string; createdAtMs: number; deadlineAtMs: number }
export interface Agent { id: Id; name: string; enabled: boolean; presetId?: Id; toolIds: Id[]; secretLast4?: string; updatedAtUtc?: string }
export interface AgentInput { name: string; enabled: boolean; presetId?: Id; toolIds: Id[] }
export interface SecretResult { agent?: Agent; endpoint: string }
export interface Tunnel { id: number; baseUrl: string; createdAtUtc?: string; updatedAtUtc?: string }
export interface TunnelTestResult { ok: boolean; pong: boolean; baseUrl: string }
export interface PluginLink { tunnelId: number; baseUrl: string; maskedEndpoint: string }
export interface Tool { id: Id; name: string; description?: string; group?: string; dangerous?: boolean }
export interface ToolPreset { id: Id; name: string; description?: string; toolIds: Id[] }
export interface Task { id: Id; title?: string; source?: string; projectFolder?: string | null; allowExecute?: boolean | null; approvalDeadlineUtc?: string | null; status: string; updatedAtUtc: string; createdAtUtc?: string; generation?: number; turnCount?: number; activeSessionId?: Id; outputPreview?: string; approvalPending?: boolean; finalResponseCount?: number; isSubagent?: boolean; parentTaskId?: Id; parentTurnId?: Id; agentName?: string }
export interface TaskPage { items: Task[]; nextCursor?: string }
export interface WorkspaceProject { id: Id; name: string; path: string; chatGptProjectUrl?: string | null; createdAtUtc?: string; updatedAtUtc?: string }
export interface ChatGptRequest { id: Id; taskId?: Id; turnId: Id; agentId: Id; model: string; userContent: string; submittedContent: string; projectFolder?: string | null; status: string; conversationId?: string; conversationUrl?: string; assistantContent?: string; errorMessage?: string; hasFinalResponse?: boolean }
export interface ChatGptBridge { taskId: Id; conversationId?: string | null; conversationUrl?: string | null; model: string; taskStatus?: string; activeRequestId?: Id; activeStatus?: string; activeSubmittedContent?: string; latestRequestId?: Id; latestSubmittedContent?: string }
export interface ChatGptQueuedMessage { id: Id; taskId: Id; content: string; mode: 'queued' | 'immediate'; sortOrder: number; createdAtMs: number; updatedAtMs: number }
export interface SubagentRun { id: Id; parentTurnId: Id; taskId?: Id; name: string; request: string; status: string; createdAtUtc: string; updatedAtUtc: string; completedAtUtc?: string; workerId?: string; attempt: number; leaseExpiresAtUtc?: string; lastHeartbeatAtUtc?: string; maxRuntimeMs: number; startedAtUtc?: string; terminalReason?: string }
export interface SubagentApproval { activityId: Id; childTaskId: Id; subagentId: Id; agentName: string; parentTurnId: Id; childTurnId?: Id; tool?: string; input?: unknown; createdAtUtc: string }
export interface ApprovalGrant { id: Id; allowedTools: string[]; pathScopes: { path: string; kind: 'exact' | 'subtree' }[]; maxCalls: number; usedCalls: number; maxFilesScanned?: number; usedFilesScanned: number; maxBytesRead?: number; usedBytesRead: number; expiresAtUtc: string; state: string; inheritedFrom?: Id; childAttempt?: number }
export interface TaskDetail { task: Task; turns?: TaskTurn[]; events?: TimelineEvent[]; nextCursor?: string; subagents?: SubagentRun[]; subagentApprovals?: SubagentApproval[]; approvalGrants?: ApprovalGrant[]; executionMode?: CommandExecutionMode; executionModeSourceTaskId?: Id }
export interface TaskActivityDetail { input?: unknown; output?: unknown; status?: string; error?: string; errorCode?: string; errorMessage?: string; errorDetails?: unknown }
export interface TaskTurn { id: Id; generation?: number; actor?: string; status?: string; startedAtUtc?: string; completedAtUtc?: string; events?: TimelineEvent[] }
export interface Session { kind: 'mcp' | 'terminal'; id: Id; taskId?: Id; turnId?: Id; shell?: string; processId?: number; status: string; workingDirectory?: string; createdAtUtc?: string; updatedAtUtc?: string; closedAtUtc?: string; replayCursor?: string; cpuPercent?: number; memoryBytes?: number; busy?: boolean; lastSequence?: number }
export interface SessionDetail { session: Session; events: TimelineEvent[]; nextCursor?: string; truncated?: boolean }
export interface LiveTerminalEvent { sequence: number; occurredAtUtc: string; stream: string; data: string; encoding: 'utf-8' | 'base64' }
export interface LiveTerminalOutput { sessionId: Id; oldestAvailableSequence: number; latestAvailableSequence: number; replayTruncated: boolean; droppedBytes: number; droppedEvents: number; events: LiveTerminalEvent[] }
export type SkillOptionValue = string | number | boolean;
export interface SkillOptionChoice { value: string; label: string }
export interface UserSkillOption { key: string; label: string; description?: string | null; type: 'select' | 'boolean' | 'text' | 'number'; value: SkillOptionValue; choices?: SkillOptionChoice[] | null }
export interface UserSkill { id: Id; title: string; description?: string | null; iconUrl?: string | null; source: 'global' | 'workspace'; sourceUrl?: string | null; enabled: boolean; canDelete?: boolean; options: UserSkillOption[] }
export interface SkillInstallCandidate { name: string; title: string; description: string; path: string; installed: boolean }
export interface SkillInstallPreview { repositoryUrl: string; skills: SkillInstallCandidate[]; skippedInvalid: number }
export interface SkillInstallResult { skills: UserSkill[] }
export interface Skill { id: Id; name: string; source?: string; enabled: boolean; description?: string; content?: string }
export interface LocalSettings { bindAddress: string; port: number; mcpEndpoint: string; databasePath: string; databaseState?: HealthState; executionMode: CommandExecutionMode; approveNewConversations: boolean; terminalExecutable: string; taskConcurrency: number; sessionConcurrency: number; subagentConcurrency: number; theme: 'system' | 'light' | 'dark'; fontFamily: string; taskFontScale: number; language: 'en' | 'vi'; sound?: boolean; newAgentSound: boolean; finishedTaskSound: boolean; dataRetention: '1h' | '5h' | '10h' | '1d' | '3d' | '5d' | '10d' | 'off' }
export interface ProblemDetails { type?: string; title?: string; status?: number; detail?: string; message?: string; code?: string; instance?: string; errors?: Record<string, string[]> }
