import { appLocale, formatAppNumber, tr } from '../i18n';
import type { Task, TaskDetail, TaskTurn, TimelineEvent } from '../types';
import { formatToolOutput } from './taskToolOutput';

export type ActivityKind = 'read' | 'search' | 'edit' | 'create' | 'delete' | 'copy' | 'move' | 'git' | 'command' | 'tool';
export type ToolActivity = {
  id: string;
  tool: string;
  kind: ActivityKind;
  input?: unknown;
  output?: unknown;
  status: string;
  error?: string;
  errorCode?: string;
  errorMessage?: string;
  errorDetails?: unknown;
  startedAt: string;
  finishedAt?: string;
  turnId?: string;
};
export type ProcessBlock =
  | { type: 'progress'; key: string; event: TimelineEvent }
  | { type: 'activities'; key: string; activities: ToolActivity[] };

export function buildTaskTurns(events: TimelineEvent[], task: Task): TaskTurn[] {
  const groups = new Map<string, TaskTurn>();
  events.forEach((event, index) => {
    const id = event.turnId || `legacy-${event.sessionId || 'task'}`;
    const current = groups.get(id) ?? {
      id,
      generation: task.generation ?? 1,
      actor: 'agent',
      status: 'running',
      startedAtUtc: event.occurredAt,
      events: [],
    };
    current.events!.push(event);
    if (findFinalResponse(current.events!)) {
      current.status = 'completed';
      current.completedAtUtc = event.occurredAt;
    }
    groups.set(id, current);
    if (!event.turnId && index === 0) current.startedAtUtc = event.occurredAt;
  });
  const turns = [...groups.values()];
  turns.forEach((turn, index) => {
    if (turn.status === 'completed') return;
    if (index === turns.length - 1 && task.status === 'running') turn.status = 'running';
    else if (task.status === 'failed') turn.status = 'failed';
    else turn.status = 'incomplete';
  });
  return turns;
}

export function buildProcessBlocks(events: TimelineEvent[]): ProcessBlock[] {
  const blocks: ProcessBlock[] = [];
  const pending = new Map<string, ToolActivity>();
  const pendingByTool = new Map<string, ToolActivity[]>();
  let currentActivities: ToolActivity[] | null = null;
  for (const event of events) {
    if (event.type === 'tool_call') {
      const payload = payloadObject(event);
      const tool = stringValue(payload.tool) || 'tool';
      if (isHousekeepingTool(tool)) continue;
      const activityId = stringValue(payload.activityId) || event.id;
      const existing = pending.get(activityId);
      if (existing) {
        existing.status = stringValue(payload.status) || existing.status;
        if (payload.input !== undefined) existing.input = payload.input;
        const stopReason = stringValue(payload.stopReason);
        if (stopReason) existing.error = tr('Stop reason: {reason}', { reason: stopReason });
        continue;
      }
      const activity: ToolActivity = {
        id: activityId,
        tool,
        kind: toolKind(tool),
        input: payload.input,
        status: stringValue(payload.status) || 'started',
        startedAt: event.occurredAt,
        turnId: event.turnId,
      };
      if (!currentActivities) {
        currentActivities = [];
        blocks.push({ type: 'activities', key: `tools-${event.id}`, activities: currentActivities });
      }
      currentActivities.push(activity);
      pending.set(activity.id, activity);
      const queue = pendingByTool.get(tool) ?? [];
      queue.push(activity);
      pendingByTool.set(tool, queue);
      continue;
    }
    if (event.type === 'tool_result') {
      const payload = payloadObject(event);
      const tool = stringValue(payload.tool) || 'tool';
      if (isHousekeepingTool(tool)) continue;
      const id = stringValue(payload.activityId);
      const queue = pendingByTool.get(tool) ?? [];
      const activity = (id && pending.get(id)) || queue.find((item) => !item.finishedAt);
      if (activity) {
        activity.output = payload.output;
        activity.status = stringValue(payload.status) || 'succeeded';
        activity.finishedAt = event.occurredAt;
        activity.errorMessage = stringValue(payload.errorMessage) || undefined;
        activity.errorCode = stringValue(payload.errorCode) || undefined;
        activity.errorDetails = payload.errorDetails ?? payload.details ?? payload.error;
        activity.error = activity.errorMessage || activity.errorCode;
      } else {
        currentActivities ??= [];
        if (!blocks.length || blocks.at(-1)?.type !== 'activities') {
          blocks.push({ type: 'activities', key: `tools-${event.id}`, activities: currentActivities });
        }
        currentActivities.push({
          id: id || event.id,
          tool,
          kind: toolKind(tool),
          output: payload.output,
          status: stringValue(payload.status) || 'succeeded',
          errorMessage: stringValue(payload.errorMessage) || undefined,
          errorCode: stringValue(payload.errorCode) || undefined,
          errorDetails: payload.errorDetails ?? payload.details ?? payload.error,
          error: stringValue(payload.errorMessage) || stringValue(payload.errorCode),
          startedAt: event.occurredAt,
          finishedAt: event.occurredAt,
        });
      }
      continue;
    }
    if (event.type === 'terminal_output' && currentActivities?.length) {
      const latest = currentActivities.at(-1)!;
      latest.output = appendOutput(latest.output, eventText(event));
      continue;
    }
    const payload = payloadObject(event);
    if (event.type === 'message' && stringValue(payload.role) === 'user') {
      currentActivities = null;
      continue;
    }
    if (isVisibleAgentMessage(event)) {
      currentActivities = null;
      blocks.push({ type: 'progress', key: event.id, event });
    }
  }
  return blocks.filter((block) => block.type === 'progress' || block.activities.length > 0);
}

export function findUserMessage(events: TimelineEvent[]) {
  for (const event of events) {
    const payload = payloadObject(event);
    if (event.type !== 'message' || stringValue(payload.role) !== 'user') continue;
    const text = eventText(event);
    if (text) return { event, text };
  }
  return null;
}

export function findFinalResponse(events: TimelineEvent[]) {
  for (let index = events.length - 1; index >= 0; index--) {
    const event = events[index];
    const payload = payloadObject(event);
    if (event.type === 'status' && stringValue(payload.status) === 'completed') {
      const text = stringValue(payload.content) || stringValue(payload.response) || stringValue(payload.message);
      if (text) return { event, text };
    }
  }
  return null;
}

export function eventText(event?: TimelineEvent) {
  if (!event) return '';
  if (typeof event.payload === 'string') return event.payload.trim();
  const payload = payloadObject(event);
  for (const key of ['content', 'message', 'text', 'response', 'plainText', 'errorMessage', 'error']) {
    const value = stringValue(payload[key]);
    if (value) return value;
  }
  return '';
}

export function latestMessage(events: TimelineEvent[]) {
  for (let index = events.length - 1; index >= 0; index--) {
    const payload = payloadObject(events[index]);
    if (events[index].type === 'message' && stringValue(payload.role) === 'user') continue;
    const text = eventText(events[index]);
    if (text) return text;
  }
  return '';
}

export function activityTarget(activity: ToolActivity) {
  const input = asObject(activity.input);
  const kind = activity.kind;
  if (kind === 'copy' || kind === 'move') {
    const source = stringValue(input.source) || stringValue(input.path);
    const destination = stringValue(input.destination);
    if (source && destination) return `${pathName(source)} → ${pathName(destination)}`;
  }
  for (const key of ['path', 'workingDirectory', 'query', 'command', 'source', 'destination', 'name']) {
    const value = stringValue(input[key]);
    if (!value) continue;
    if (['read', 'edit', 'create', 'delete'].includes(kind)) return pathName(value);
    return truncate(value, kind === 'git' ? 92 : 96);
  }
  return humanToolName(activity.tool);
}

export function activityLabel(activity: ToolActivity) {
  const target = activityTarget(activity);
  const running = activity.status === 'started';
  if (activity.status === 'pending_approval') return tr('Waiting for approval to {target}', { target });
  if (activity.status === 'stop_requested') return tr('Stopping {target}', { target });
  if (activity.status === 'stopped') return tr('Stopped {target}', { target });
  if (running) {
    if (activity.kind === 'read') return tr('Reading {target}', { target });
    if (activity.kind === 'search') return tr('Searching {target}', { target });
    if (activity.kind === 'edit') return tr('Editing {target}', { target });
    if (activity.kind === 'create') return tr('Creating {target}', { target });
    if (activity.kind === 'delete') return tr('Deleting {target}', { target });
    if (activity.kind === 'copy') return tr('Copying {target}', { target });
    if (activity.kind === 'move') return tr('Moving {target}', { target });
    if (activity.kind === 'git') return tr('Running Git operation: {target}', { target });
    if (activity.kind === 'command') return tr('Running {target}', { target });
    return tr('Using {target}', { target });
  }
  if (activity.status === 'failed') return tr('Error: {target}', { target });
  if (activity.kind === 'read') return tr('Read {target}', { target });
  if (activity.kind === 'search') return tr('Searched {target}', { target });
  if (activity.kind === 'edit') return tr('Edited {target}', { target });
  if (activity.kind === 'create') return tr('Created {target}', { target });
  if (activity.kind === 'delete') return tr('Deleted {target}', { target });
  if (activity.kind === 'copy') return tr('Copied {target}', { target });
  if (activity.kind === 'move') return tr('Moved {target}', { target });
  if (activity.kind === 'git') return tr('Ran Git operation: {target}', { target });
  if (activity.kind === 'command') return tr('Ran {target}', { target });
  return tr('Used {target}', { target });
}

export function summarizeActivities(activities: ToolActivity[]) {
  const counts = new Map<ActivityKind, number>();
  for (const activity of activities) counts.set(activity.kind, (counts.get(activity.kind) ?? 0) + 1);
  const phrases = (['read', 'search', 'edit', 'create', 'delete', 'copy', 'move', 'git', 'command', 'tool'] as ActivityKind[]).flatMap((kind) => {
    const count = counts.get(kind) ?? 0;
    if (!count) return [];
    const value = formatAppNumber(count);
    return [kind === 'read' ? tr('read {count} files', { count: value }) : kind === 'search' ? tr('searched {count} times', { count: value }) : kind === 'edit' ? tr('edited {count} files', { count: value }) : kind === 'create' ? tr('created {count} items', { count: value }) : kind === 'delete' ? tr('deleted {count} items', { count: value }) : kind === 'copy' ? tr('copied {count} items', { count: value }) : kind === 'move' ? tr('moved {count} items', { count: value }) : kind === 'git' ? tr('performed {count} Git operations', { count: value }) : kind === 'command' ? tr('ran {count} commands', { count: value }) : tr('used {count} tools', { count: value })];
  });
  if (phrases.length < 2) return phrases[0] ?? '';
  return tr('{items} and {last}', { items: phrases.slice(0, -1).join(', '), last: phrases.at(-1) ?? '' });
}

export function activityCommand(activity: ToolActivity) {
  const input = asObject(activity.input);
  const command = stringValue(input.command);
  if (command) return command;
  const path = stringValue(input.path);
  if (path) return `${activity.tool}: ${path}`;
  const legacy = legacyTerminalParts(activity.output);
  if (legacy.command) return legacy.command;
  return activity.input === undefined ? activity.tool : `${activity.tool} ${formatValue(activity.input)}`;
}

export function activityOutput(activity: ToolActivity) {
  if (activity.output === undefined) return activity.error ? activity.error : '';
  if (activity.tool === 'fs_search' && Array.isArray(activity.output)) {
    const paths = new Set(activity.output.map((item) => stringValue(asObject(item).path)).filter(Boolean));
    return [...paths].join('\n');
  }
  if (typeof activity.output === 'string') return stripAnsi(activity.output);
  const value = asObject(activity.output);
  if (activity.kind === 'git') {
    const stdout = stringValue(value.stdout);
    const stderr = stringValue(value.stderr);
    const commandOutput = [stdout, stderr].filter(Boolean).join('\n');
    if (commandOutput) return stripAnsi(commandOutput);
  }
  for (const key of ['code', 'content', 'text', 'plainText', 'output']) {
    const text = stringValue(value[key]);
    if (!text) continue;
    if (key === 'plainText' && activity.kind === 'command' && activity.input === undefined) {
      const legacy = legacyTerminalParts(activity.output);
      return legacy.output || stripAnsi(text);
    }
    return key === 'code' ? text : stripAnsi(text);
  }
  return formatToolOutput(activity.tool, activity.output);
}

export function fsSearchCodeViews(activity: ToolActivity) {
  if (activity.tool !== 'fs_search' || !Array.isArray(activity.output)) return [];
  return activity.output.flatMap((item) => {
    const value = asObject(item);
    const path = stringValue(value.path);
    const text = stringValue(value.text);
    if (!path || !text) return [];
    return [{ code: text, path, startLine: Math.max(1, Number(value.line) || 1) }];
  });
}

export function activityDiffView(activity: ToolActivity) {
  const output = asObject(activity.output);
  const diff = asObject(output.__chatcmdDiff);
  const path = stringValue(diff.path);
  if (!path || diff.before === undefined || diff.after === undefined) return null;
  const before = typeof diff.before === 'string' ? diff.before : '';
  const after = typeof diff.after === 'string' ? diff.after : '';
  const beforeLines = before.replace(/\r\n?/g, '\n').split('\n');
  const afterLines = after.replace(/\r\n?/g, '\n').split('\n');
  let prefix = 0;
  while (prefix < beforeLines.length && prefix < afterLines.length && beforeLines[prefix] === afterLines[prefix]) prefix++;
  let suffix = 0;
  while (suffix < beforeLines.length - prefix && suffix < afterLines.length - prefix && beforeLines[beforeLines.length - 1 - suffix] === afterLines[afterLines.length - 1 - suffix]) suffix++;
  const beforeMarks: Record<number, 'removed' | 'changed'> = {};
  const afterMarks: Record<number, 'added' | 'changed'> = {};
  const beforeChanged = Math.max(0, beforeLines.length - prefix - suffix);
  const afterChanged = Math.max(0, afterLines.length - prefix - suffix);
  for (let index = 0; index < beforeChanged; index++) beforeMarks[prefix + index + 1] = index < afterChanged ? 'changed' : 'removed';
  for (let index = 0; index < afterChanged; index++) afterMarks[prefix + index + 1] = index < beforeChanged ? 'changed' : 'added';
  return { path, before, after, beforeMarks, afterMarks };
}

export function activityCodeView(activity: ToolActivity) {
  const input = asObject(activity.input);
  const output = activityOutput(activity);
  if (!output.trim()) return null;
  if (activity.kind === 'git') {
    const isDiff = activity.tool === 'git_diff' || activity.tool === 'git_show';
    return { code: output, path: stringValue(input.path) || null, language: isDiff ? 'diff' : 'plain' };
  }
  if (activity.tool === 'fs_read_text') {
    const outputObject = asObject(activity.output);
    return { code: output, path: stringValue(outputObject.path) || stringValue(input.path) || null, startLine: Number(outputObject.startLine) || Number(input.startLine) || 1 };
  }
  if (activity.tool === 'fs_write_text' && stringValue(input.content)) return { code: stringValue(input.content), path: stringValue(input.path) || null, startLine: 1 };
  if (activity.tool === 'fs_replace_text' && stringValue(input.newText)) return { code: stringValue(input.newText), path: stringValue(input.path) || null, startLine: 1 };
  if (activity.tool === 'apply_patch' && stringValue(input.patch)) return { code: stringValue(input.patch), language: 'diff', startLine: 1 };
  return null;
}

export function activityDuration(start: string, finish: string) {
  const milliseconds = Math.max(0, new Date(finish).getTime() - new Date(start).getTime());
  if (milliseconds < 1000) return `${milliseconds}ms`;
  if (milliseconds < 10_000) return `${(milliseconds / 1000).toFixed(1)}s`;
  return duration(start, finish);
}

export function duration(start: string, finish: string) {
  const milliseconds = Math.max(0, new Date(finish).getTime() - new Date(start).getTime());
  const seconds = Math.floor(milliseconds / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  return hours ? `${hours}h ${minutes % 60}m ${seconds % 60}s` : minutes ? `${minutes}m ${seconds % 60}s` : `${seconds}s`;
}

export function formatClockTime(value: string) {
  return new Intl.DateTimeFormat(appLocale(), { hour: '2-digit', minute: '2-digit', second: '2-digit' }).format(new Date(value));
}

export function mergeTaskEvent(task: Task, event: TimelineEvent): Task {
  const payload = payloadObject(event);
  const status = stringValue(payload.status);
  const title = stringValue(payload.title);
  const finalCompleted = event.type === 'status' && status === 'completed';
  const nextStatus = taskStatusFromEvent(status, event.type, task.status);
  return {
    ...task,
    status: nextStatus,
    title: title || task.title,
    updatedAtUtc: event.occurredAt || task.updatedAtUtc,
    outputPreview: stringValue(payload.content) || stringValue(payload.message) || task.outputPreview,
    finalResponseCount: (task.finalResponseCount ?? 0) + (finalCompleted ? 1 : 0),
  };
}

export function upsertTaskEvent(tasks: Task[] | undefined, event: TimelineEvent): Task[] | undefined {
  if (!event.taskId) return tasks;
  const current = tasks ?? [];
  if (current.some((task) => task.id === event.taskId)) return current.map((task) => task.id === event.taskId ? mergeTaskEvent(task, event) : task);
  const created = taskFromRealtimeEvent(event);
  return created ? [created, ...current] : tasks;
}

export function taskFromRealtimeEvent(event: TimelineEvent): Task | null {
  if (!event.taskId) return null;
  const payload = payloadObject(event);
  const status = stringValue(payload.status);
  const finalCompleted = event.type === 'status' && status === 'completed';
  return {
    id: event.taskId,
    title: stringValue(payload.title) || undefined,
    status: taskStatusFromEvent(status, event.type, 'running'),
    createdAtUtc: event.occurredAt,
    updatedAtUtc: event.occurredAt,
    turnCount: event.turnId ? 1 : 0,
    activeSessionId: event.sessionId,
    outputPreview: stringValue(payload.content) || stringValue(payload.message) || undefined,
    finalResponseCount: finalCompleted ? 1 : 0,
  };
}

export function mergeLiveDetail(detail: TaskDetail, liveEvents: TimelineEvent[]): TaskDetail {
  if (!liveEvents.length) return detail;
  const events = [...(detail.events ?? [])];
  const seen = new Set(events.map((event) => event.id));
  for (const event of liveEvents) if (!seen.has(event.id)) { events.push(event); seen.add(event.id); }
  events.sort((left, right) => Date.parse(left.occurredAt) - Date.parse(right.occurredAt) || left.id.localeCompare(right.id));
  const task = liveEvents.reduce(mergeTaskEvent, detail.task);
  return { ...detail, task, turns: undefined, events };
}

function taskStatusFromEvent(status: string, eventType: string, fallback: string) {
  if (['pending', 'running', 'completed', 'failed', 'stopped', 'interrupted'].includes(status)) return status;
  if (eventType === 'progress' || eventType === 'tool_call') return 'running';
  return fallback;
}
function isUserMessage(event: TimelineEvent) { const payload = payloadObject(event); return event.type === 'message' && stringValue(payload.role) === 'user'; }
function isVisibleAgentMessage(event: TimelineEvent) {
  if (isUserMessage(event) || !['progress', 'message', 'warning', 'status'].includes(event.type)) return false;
  const payload = payloadObject(event);
  return !(event.type === 'status' && stringValue(payload.status) === 'completed') && Boolean(eventText(event));
}
function payloadObject(event: TimelineEvent): Record<string, unknown> { return asObject(event.payload); }
function stringValue(value: unknown) { return typeof value === 'string' && value.trim() ? value.trim() : ''; }
function asObject(value: unknown): Record<string, unknown> { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}; }
function isHousekeepingTool(tool: string) { return ['agent_user_message', 'agent_progress', 'agent_subagent_start', 'agent_subagent_wait', 'agent_turn_complete'].includes(tool); }
function toolKind(tool: string): ActivityKind {
  if (/^(?:fs_read_text|fs_list|fs_stat|fs_directory_sizes|view_image|file_download|skill_read)$/i.test(tool)) return 'read';
  if (tool === 'fs_search' || tool === 'fs_find') return 'search';
  if (/^(?:apply_patch|fs_write_text|fs_replace_text|file_upload)$/i.test(tool)) return 'edit';
  if (tool === 'fs_create_directory') return 'create';
  if (/^(?:file_delete|empty_directory_delete|workspace_temp_cleanup|fs_delete)$/i.test(tool)) return 'delete';
  if (tool === 'fs_copy') return 'copy';
  if (tool === 'fs_move') return 'move';
  if (tool.startsWith('git_')) return 'git';
  if (/shell_|terminal|execute|command/i.test(tool)) return 'command';
  return 'tool';
}
function humanToolName(value: string) { return value.replaceAll('_', ' '); }
function pathName(path: string) { return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path; }
function truncate(value: string, max: number) { return value.length <= max ? value : `${value.slice(0, max - 1)}…`; }
function formatValue(value: unknown) { return typeof value === 'string' ? value : JSON.stringify(value ?? {}, null, 2); }
function appendOutput(output: unknown, text: string) { if (!text) return output; if (typeof output === 'string') return output + text; const value = asObject(output); const current = stringValue(value.plainText) || stringValue(value.text); return { ...value, plainText: current + text }; }
function legacyTerminalParts(output: unknown) {
  const value = asObject(output);
  const raw = stringValue(value.plainText) || stringValue(value.text) || (typeof output === 'string' ? output : '');
  if (!raw) return { command: '', output: '' };
  const lines = stripAnsi(raw).replace(/\r/g, '').split('\n').map((line) => line.replace(/\s+$/g, ''));
  const meaningful = lines.map((line, index) => ({ line, index })).filter(({ line }) => line.trim() && !isTerminalPrompt(line));
  const first = meaningful[0];
  if (!first) return { command: '', output: '' };
  const command = first.line.trim();
  const rest = lines.slice(first.index + 1).filter((line) => !isTerminalPrompt(line)).join('\n').trim();
  return { command, output: rest };
}
function isTerminalPrompt(value: string) { const line = value.trim(); return !line || /^PS\s+.+?>\s*$/i.test(line) || /^>>\s*$/.test(line); }
function stripAnsi(value: string) {
  let result = '';
  for (let index = 0; index < value.length;) {
    if (value.charCodeAt(index) !== 27) { result += value[index++]; continue; }
    index++;
    const marker = value[index];
    if (marker === ']') { index++; while (index < value.length && value.charCodeAt(index) !== 7 && !(value.charCodeAt(index) === 27 && value[index + 1] === '\\')) index++; if (value.charCodeAt(index) === 27) index += 2; else if (value.charCodeAt(index) === 7) index++; continue; }
    if (marker === '[') { index++; while (index < value.length) { const code = value.charCodeAt(index++); if (code >= 64 && code <= 126) break; } continue; }
    index++;
  }
  return result;
}
