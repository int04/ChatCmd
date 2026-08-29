import { useCallback, useEffect, useRef, useState } from 'react';

import { tr, useAppLanguage } from '../i18n';
import type { Task, TimelineEvent } from '../types';

const TITLE_OUTPUT_TAIL_LIMIT = 1024;
const TITLE_FRAMES = ['|', '/', '—', '\\'];

type TitleActivity = { label: string; timestamp: number };
type CommandTitleBuffer = { sessionId: string; tail: string };

export function useTaskDocumentTitle(tasks: Task[] | undefined) {
  const language = useAppLanguage();
  const activities = useRef(new Map<string, TitleActivity>());
  const commandBuffers = useRef(new Map<string, CommandTitleBuffer>());
  const [version, setVersion] = useState(0);

  useEffect(() => {
    if (!tasks) return;
    let changed = false;
    const running = new Set(tasks.filter((task) => task.status === 'running').map((task) => task.id));
    for (const taskId of activities.current.keys()) {
      if (running.has(taskId)) continue;
      activities.current.delete(taskId);
      commandBuffers.current.delete(taskId);
      changed = true;
    }
    for (const task of tasks) {
      if (task.status !== 'running' || activities.current.has(task.id)) continue;
      activities.current.set(task.id, { label: tr('Thinking'), timestamp: Date.parse(task.updatedAtUtc) || Date.now() });
      changed = true;
    }
    if (changed) setVersion((value) => value + 1);
  }, [tasks, language]);

  const handleEvent = useCallback((event: TimelineEvent) => {
    if (!event.taskId) return;
    if (!updateTitleActivity(activities.current, commandBuffers.current, event)) return;
    setVersion((value) => value + 1);
  }, []);

  useEffect(() => {
    const active = [...activities.current.values()].sort((left, right) => right.timestamp - left.timestamp)[0];
    if (!active) {
      document.title = `ChatCMD · ${tr('Local Control')}`;
      return;
    }

    let frame = 0;
    const render = () => { document.title = `${TITLE_FRAMES[frame++ % TITLE_FRAMES.length]} ${active.label} · ChatCMD`; };
    render();
    const timer = window.setInterval(render, 320);
    return () => window.clearInterval(timer);
  }, [version, language]);

  useEffect(() => () => { document.title = `ChatCMD · ${tr('Local Control')}`; }, []);

  return handleEvent;
}

export function titleLabelForEvent(event: TimelineEvent) {
  if (!event.taskId) return null;
  const payload = asObject(event.payload);
  if (event.type === 'progress') return tr('Thinking');
  if (event.type === 'status') {
    const status = stringValue(payload.status);
    return ['completed', 'failed', 'stopped', 'interrupted'].includes(status) ? null : tr('Thinking');
  }
  if (event.type === 'tool_result') {
    const tool = stringValue(payload.tool);
    return tool === 'agent_turn_complete' ? null : tr('Thinking');
  }
  if (event.type !== 'tool_call') return null;
  const status = stringValue(payload.status);
  if (status && status !== 'started') return tr('Thinking');
  const tool = stringValue(payload.tool) || 'tool';
  const target = compactTitleTarget(payload.input);
  if (/^(?:fs_read_text|fs_list|fs_stat|fs_directory_sizes|view_image|file_download|skill_read)$/i.test(tool)) return `${tr('Reading')}${target ? ` ${target}` : ''}`;
  if (tool === 'fs_search' || tool === 'fs_find') return `${tr('Searching')}${target ? ` ${target}` : ''}`;
  if (/^(?:apply_patch|fs_write_text|fs_replace_text|file_upload)$/i.test(tool)) return `${tr('Editing')}${target ? ` ${target}` : ''}`;
  if (tool === 'fs_create_directory') return `${tr('Creating')}${target ? ` ${target}` : ''}`;
  if (/^(?:file_delete|empty_directory_delete|workspace_temp_cleanup|fs_delete)$/i.test(tool)) return `${tr('Deleting')}${target ? ` ${target}` : ''}`;
  if (tool === 'fs_copy') return `${tr('Copying')}${target ? ` ${target}` : ''}`;
  if (tool === 'fs_move') return `${tr('Moving')}${target ? ` ${target}` : ''}`;
  if (tool.startsWith('git_')) return `${tr('Working with Git')}${target ? ` · ${target}` : ''}`;
  if (isCommandInputTool(tool)) return tr('Running command');
  return tr('Using {tool}', { tool: tool || 'tool' });
}

function updateTitleActivity(activities: Map<string, TitleActivity>, commandBuffers: Map<string, CommandTitleBuffer>, event: TimelineEvent) {
  const taskId = event.taskId;
  if (!taskId) return false;
  const payload = asObject(event.payload);
  const eventStatus = stringValue(payload.status);
  if (event.type === 'status' && ['completed', 'failed', 'stopped', 'interrupted'].includes(eventStatus)) {
    return activities.delete(taskId) || commandBuffers.delete(taskId);
  }
  if (event.type === 'terminal_output') {
    const buffer = commandBuffers.get(taskId);
    if (!buffer) return false;
    if (buffer.sessionId !== 'unknown' && event.sessionId && buffer.sessionId !== event.sessionId) return false;
    const tail = `${buffer.tail}${eventText(event)}`.slice(-TITLE_OUTPUT_TAIL_LIMIT);
    if (!commandOutputFinished(tail)) {
      commandBuffers.set(taskId, { ...buffer, tail });
      return false;
    }
    commandBuffers.delete(taskId);
    activities.set(taskId, { label: tr('Thinking'), timestamp: eventTimestamp(event) });
    return true;
  }
  if (event.type === 'tool_call') {
    const tool = stringValue(payload.tool);
    if (tool === 'agent_turn_complete') return false;
    if (isCommandInputTool(tool)) commandBuffers.set(taskId, { sessionId: event.sessionId ?? 'unknown', tail: '' });
    else commandBuffers.delete(taskId);
  }
  if (event.type === 'progress') commandBuffers.delete(taskId);
  if (event.type === 'tool_result') {
    const tool = stringValue(payload.tool);
    if (tool === 'agent_turn_complete') {
      commandBuffers.delete(taskId);
      return activities.delete(taskId);
    }
    const command = commandBuffers.get(taskId);
    if (command && isCommandInputTool(tool) && command.sessionId === (event.sessionId ?? 'unknown')) return false;
  }
  const label = titleLabelForEvent(event);
  if (!label) return false;
  activities.set(taskId, { label, timestamp: eventTimestamp(event) });
  return true;
}

function compactTitleTarget(value: unknown) {
  if (typeof value === 'string') return truncateTitle(value);
  const input = asObject(value);
  const preferred = ['path', 'workingDirectory', 'query', 'source', 'destination', 'command']
    .map((key) => input[key])
    .find((item) => typeof item === 'string' && item.trim());
  return typeof preferred === 'string' ? truncateTitle(preferred) : '';
}
function truncateTitle(value: string) { const normalized = value.replace(/\s+/g, ' ').trim(); return normalized.length > 64 ? `${normalized.slice(0, 61)}…` : normalized; }
function isCommandInputTool(tool: string) { return tool === 'shell_write' || /terminal.*input|execute|command/i.test(tool); }
function commandOutputFinished(text: string) { return !!text && (/__CMDGPT_DONE_[A-Za-z0-9]+__/i.test(text) || /(?:^|\r?\n)PS\s+[^>\r\n]+>\s*$/i.test(text)); }
function eventTimestamp(event: TimelineEvent) { return Date.parse(event.occurredAt) || Date.now(); }
function eventText(event: TimelineEvent) {
  if (typeof event.payload === 'string') return event.payload;
  const payload = asObject(event.payload);
  for (const key of ['content', 'message', 'text', 'response', 'plainText', 'output']) {
    const value = payload[key];
    if (typeof value === 'string') return value;
  }
  return '';
}
function stringValue(value: unknown) { return typeof value === 'string' && value.trim() ? value.trim() : ''; }
function asObject(value: unknown): Record<string, unknown> { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}; }
