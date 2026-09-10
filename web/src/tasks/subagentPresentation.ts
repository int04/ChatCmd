import { getAppLanguage } from '../i18n';
import type { SubagentRun } from '../types';

const labels = {
  disabled: ['0 (Disabled)', '0 (Tắt)'],
  policy: ['0 disables new child Agents in every conversation. Existing running Agents may finish.', '0 tắt việc tạo Agent con trong mọi cuộc trò chuyện. Agent đang chạy được phép hoàn tất.'],
  capacity: ['The limit is shared by the entire Agent tree. A nested Agent works locally when all slots are occupied.', 'Giới hạn áp dụng cho toàn bộ cây Agent. Khi hết chỗ, Agent con xử lý tại chỗ thay vì gọi thêm Agent.'],
  header: ['Conversation header', 'Tiêu đề cuộc trò chuyện'],
  parent: ['Delegated by', 'Được giao bởi'],
  preview: ['Preview conversation', 'Xem trước cuộc trò chuyện'],
  previewError: ['Could not load conversation preview.', 'Không thể tải nội dung cuộc trò chuyện.'],
  goToConversation: ['Go to conversation', 'Tới đoạn trò chuyện'],
} as const;
export function subagentLabel(key: keyof typeof labels) {
  return labels[key][getAppLanguage() === 'vi' ? 1 : 0];
}

/** Stable pre-order, preserving orphaned/legacy entries rather than silently hiding them. */
export function subagentTreeRows(agents: SubagentRun[]): { agent: SubagentRun; depth: number }[] {
  const byTask = new Set(agents.flatMap((agent) => agent.taskId ? [agent.taskId] : []));
  const children = new Map<string, SubagentRun[]>();
  for (const agent of agents) {
    if (!agent.parentTaskId) continue;
    const siblings = children.get(agent.parentTaskId) ?? [];
    siblings.push(agent);
    children.set(agent.parentTaskId, siblings);
  }
  const rows: { agent: SubagentRun; depth: number }[] = [];
  const visited = new Set<string>();
  function visit(agent: SubagentRun, depth: number) {
    if (visited.has(agent.id)) return;
    visited.add(agent.id);
    rows.push({ agent, depth });
    for (const child of agent.taskId ? children.get(agent.taskId) ?? [] : []) visit(child, depth + 1);
  }
  for (const agent of agents) if (!agent.parentTaskId || !byTask.has(agent.parentTaskId)) visit(agent, 0);
  for (const agent of agents) visit(agent, 0);
  return rows;
}
