import { describe, expect, it } from 'vitest';
import type { TimelineEvent } from '../types';
import { responseFileChanges, responseFileChangeTrackingIncomplete } from './TaskTurnBubble';

const event = (payload: unknown): TimelineEvent => ({
  id: 'event-1',
  type: 'status',
  occurredAt: '2026-09-04T00:00:00Z',
  payload,
});

describe('typed turn file changes', () => {
  it('preserves exact previews and artifact references', () => {
    const changes = responseFileChanges(event({ fileChanges: [{
      path: '/workspace/a.txt', kind: 'modified', additions: 2, deletions: 1,
      confidence: 'exact', diffArtifactRef: 'artifact-1',
      preview: { before: 'old', after: 'new', binary: false, truncated: false },
    }] }));

    expect(changes).toHaveLength(1);
    expect(changes[0]).toMatchObject({ confidence: 'exact', additions: 2, deletions: 1, diffArtifactRef: 'artifact-1' });
    expect(changes[0].activity.output).toEqual({ __chatcmdDiff: { path: '/workspace/a.txt', before: 'old', after: 'new', beforeAvailable: true } });
  });

  it('keeps sampled and incomplete states explicit', () => {
    const value = event({
      fileChangeTrackingIncomplete: true,
      fileChanges: [{ path: '/workspace/large.bin', kind: 'modified', confidence: 'unknownDueToOverflow', preview: { binary: true, truncated: true } }],
    });

    expect(responseFileChangeTrackingIncomplete(value)).toBe(true);
    expect(responseFileChanges(value)[0]).toMatchObject({ additions: null, deletions: null, confidence: 'unknownDueToOverflow' });
  });
});
