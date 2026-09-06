import type { Task, TaskDetail, TimelineEvent } from '../types';
import { findFinalResponse } from './taskTimeline';
import { mergeTimelineEvents } from './timelineSnapshots';
export function mergeTaskEvent(task: Task, event: TimelineEvent): Task {
  if (event.type === 'chatgpt_think') return task;
  const payload = payloadObject(event);
  const status = stringValue(payload.status);
  const title = stringValue(payload.title);
  const finalCompleted = event.type === 'status' && status === 'completed'
    && Boolean(stringValue(payload.content) || stringValue(payload.response) || stringValue(payload.message));
  const newUserTurn = event.type === 'message' && payload.role === 'user'
    && Date.parse(event.occurredAt) >= Date.parse(task.updatedAtUtc);
  const nextStatus = newUserTurn && task.status !== 'stopped' ? 'running' : taskStatusFromEvent(status, event.type, task.status);
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
  const finalCompleted = event.type === 'status' && status === 'completed'
    && Boolean(stringValue(payload.content) || stringValue(payload.response) || stringValue(payload.message));
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
  const events = mergeTimelineEvents(detail.events ?? [], liveEvents);
  const mergedTask = liveEvents.reduce(mergeTaskEvent, detail.task);
  const task = reconcileTaskStatusFromEvents(mergedTask, events);
  if (!liveEvents.length && task === detail.task) return detail;
  return { ...detail, task, turns: undefined, events };
}

function reconcileTaskStatusFromEvents(task: Task, events: TimelineEvent[]) {
  if (task.status !== 'running') return task;
  const final = findFinalResponse(events);
  if (!final) return task;
  const finalIndex = events.indexOf(final.event);
  const finalTurnId = final.event.turnId;
  const hasLaterWork = events.slice(finalIndex + 1).some((event) => {
    if (event.type === 'chatgpt_think') return false;
    if (event.turnId && finalTurnId && event.turnId !== finalTurnId) return true;
    const payload = payloadObject(event);
    if (event.type === 'message' && stringValue(payload.role) === 'user') return true;
    if (event.type === 'progress') return true;
    if (event.type === 'tool_call') return stringValue(payload.tool) !== 'agent_turn_complete';
    return false;
  });
  return hasLaterWork ? task : { ...task, status: 'completed' };
}

function taskStatusFromEvent(status: string, eventType: string, fallback: string) {
  if (['pending', 'running', 'completed', 'failed', 'stopped', 'interrupted'].includes(status)) return status;
  if (eventType === 'progress' || eventType === 'tool_call') return 'running';
  return fallback;
}

function payloadObject(event: TimelineEvent): Record<string, unknown> { return event.payload && typeof event.payload === 'object' ? event.payload as Record<string, unknown> : {}; }
function stringValue(value: unknown): string { return typeof value === 'string' ? value.trim() : ''; }
