'use strict';

const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const vm = require('node:vm');

const source = (name) => readFileSync(join(__dirname, name), 'utf8');
const clone = (value) => value === undefined ? undefined : structuredClone(value);
const BODY = 'TASK: Preserve the existing task, user constraints, files and verified results. '
  + 'CURRENT STATE: The repository contains unfinished work; do not repeat completed commands. '
  + 'NEXT ACTIONS: Review the remaining changes and run focused regression tests. No new permission is granted.';

function job(overrides = {}) {
  return {
    id: 'compact-job-0001', taskId: 'task-chat-original', revision: 1,
    phase: 'preparing', detail: null, handoffText: null,
    oldConversationId: 'source-conversation',
    oldConversationUrl: 'https://chatgpt.com/c/source-conversation',
    oldModel: 'Thinking', oldRequestId: 'request-original',
    newConversationId: null, newConversationUrl: null,
    projectFolder: 'D:\\DEV\\CmdGPT\\ChatCmdClient', ...overrides,
  };
}

function protocol() {
  const context = vm.createContext({ URL });
  vm.runInContext(source('compact-protocol.js'), context, { filename: 'compact-protocol.js' });
  return context.ChatCmdCompactProtocol;
}

function event() {
  const listeners = new Set();
  return {
    listeners,
    addListener: (listener) => listeners.add(listener),
    removeListener: (listener) => listeners.delete(listener),
    emit: (...args) => [...listeners].map((listener) => listener(...args)),
  };
}

module.exports = { source, clone, BODY, job, protocol, event };
