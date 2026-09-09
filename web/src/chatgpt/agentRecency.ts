import type { Agent } from '../types';

const RECENT_AGENT_IDS_KEY = 'chatcmd.chatgpt.recentAgentIds.v1';
const MAX_RECENT_AGENTS = 50;

export function orderAgentsByRecentUse(agents: Agent[]): Agent[] {
  const positions = new Map(readRecentAgentIds().map((id, index) => [id, index]));
  return [...agents].sort((left, right) => {
    const leftIndex = positions.get(left.id) ?? Number.MAX_SAFE_INTEGER;
    const rightIndex = positions.get(right.id) ?? Number.MAX_SAFE_INTEGER;
    return leftIndex - rightIndex;
  });
}

export function rememberAgentUse(agentId: string) {
  const trimmed = agentId.trim();
  if (!trimmed || typeof localStorage === 'undefined') return;
  try {
    const next = [trimmed, ...readRecentAgentIds().filter((id) => id !== trimmed)].slice(0, MAX_RECENT_AGENTS);
    localStorage.setItem(RECENT_AGENT_IDS_KEY, JSON.stringify(next));
  } catch { /* storage can be unavailable */ }
}

function readRecentAgentIds(): string[] {
  if (typeof localStorage === 'undefined') return [];
  try {
    const value = JSON.parse(localStorage.getItem(RECENT_AGENT_IDS_KEY) ?? '[]');
    return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string' && item.length > 0) : [];
  } catch { return []; }
}
