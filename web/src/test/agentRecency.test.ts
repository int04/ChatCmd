import { beforeEach, describe, expect, it } from 'vitest';

import { orderAgentsByRecentUse, rememberAgentUse } from '../chatgpt/agentRecency';
import type { Agent } from '../types';

const agents: Agent[] = [
  { id: 'configured-first', name: 'Configured First', enabled: true, toolIds: [] },
  { id: 'recent', name: 'Recent', enabled: true, toolIds: [] },
  { id: 'older', name: 'Older', enabled: true, toolIds: [] },
];

describe('ChatGPT MCP agent recency', () => {
  beforeEach(() => localStorage.clear());

  it('keeps configured order when there is no usage history', () => {
    expect(orderAgentsByRecentUse(agents).map((agent) => agent.id)).toEqual(['configured-first', 'recent', 'older']);
  });

  it('orders agents by most recent successful use and preserves configured order for unseen agents', () => {
    rememberAgentUse('older');
    rememberAgentUse('recent');

    expect(orderAgentsByRecentUse(agents).map((agent) => agent.id)).toEqual(['recent', 'older', 'configured-first']);
  });

  it('moves a reused agent back to the front without duplicating it', () => {
    rememberAgentUse('recent');
    rememberAgentUse('older');
    rememberAgentUse('recent');

    expect(orderAgentsByRecentUse(agents).map((agent) => agent.id)).toEqual(['recent', 'older', 'configured-first']);
  });
});
